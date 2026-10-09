//! QR-code logins.
//!
//! * 115: the standard qrcodeapi.115.com flow used by p115client / TgtoDrive:
//!   `GET /api/1.0/web/1.0/token/` → poll `GET /get/status/?uid&time&sign`
//!   (0 waiting, 1 scanned, 2 confirmed, -1 expired, -2 canceled) →
//!   `POST /app/1.0/{app}/1.0/login/qrcode/` (form `account=<uid>`) which returns `data.cookie`.
//!   The resulting cookie is stored encrypted exactly like a pasted cookie and never returned to the browser.
//! * RE0: the relay has no native QR flow; its OAuth entry is the signed `/auth/start` link. We render that
//!   link as a QR code and poll the relay's `/api/token/status` until `has_access_token` becomes true.
use crate::{
    engine::Engine,
    error::{AppError, Result},
    providers::{Providers, read_json, text},
    store::now,
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use std::{collections::HashMap, time::Duration};
use tokio::sync::Mutex;

/// Device slots a 115 QR login can bind to. Logging in kicks any existing session of the same slot.
pub const APPS_115: &[(&str, &str)] = &[
    ("alipaymini", "115生活 · 支付宝小程序"),
    ("wechatmini", "115生活 · 微信小程序"),
    ("harmony", "115 · 鸿蒙端"),
    ("115android", "115 · 安卓端"),
    ("115ios", "115 · 苹果端"),
    ("115ipad", "115 · iPad 端"),
    ("tv", "115生活 · 安卓电视端"),
    ("os_windows", "115生活 · Windows"),
    ("os_mac", "115生活 · macOS"),
    ("os_linux", "115生活 · Linux"),
    ("web", "115生活 · 网页端"),
];
const QR_TTL: i64 = 600;
const BROWSER_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/129.0 Safari/537.36";

#[derive(Clone)]
struct Pending115 {
    time: String,
    sign: String,
    app: String,
    created: i64,
}
#[derive(Default)]
pub struct QrSessions {
    s115: Mutex<HashMap<String, Pending115>>,
    re0: Mutex<Option<i64>>,
}

/// Render text as an SVG QR code data URI (allowed by the page CSP: `img-src 'self' data:`).
pub fn qr_data_uri(data: &str) -> Result<String> {
    let code = qrcode::QrCode::new(data.as_bytes()).map_err(|_| AppError::bad("二维码内容过长"))?;
    let svg = code
        .render::<qrcode::render::svg::Color>()
        .min_dimensions(240, 240)
        .quiet_zone(true)
        .dark_color(qrcode::render::svg::Color("#111"))
        .light_color(qrcode::render::svg::Color("#fff"))
        .build();
    Ok(format!("data:image/svg+xml;base64,{}", STANDARD.encode(svg)))
}

fn ok_115(v: &Value) -> bool {
    match v.get("state") {
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_i64() == Some(1),
        _ => v.get("data").is_some(),
    }
}
fn msg_115(v: &Value) -> String {
    let m = text(v, &["message", "error", "msg"]);
    if m.is_empty() { text(v.get("data").unwrap_or(&Value::Null), &["msg", "message"]) } else { m }
}
/// Build `UID=..; CID=..; SEID=..; KID=..` from the login result's cookie object.
pub fn cookie_header(cookie: &Value) -> Option<String> {
    let map = cookie.as_object()?;
    let mut keys: Vec<&String> = map.keys().collect();
    let rank = |k: &str| ["UID", "CID", "SEID", "KID"].iter().position(|x| *x == k).unwrap_or(9);
    keys.sort_by_key(|k| (rank(k), (*k).clone()));
    let mut parts = vec![];
    for k in keys {
        let Some(v) = map[k].as_str() else { continue };
        let ok = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_graphic() && b != b';' && b != b',');
        if ok(k) && ok(v) && !k.contains('=') {
            parts.push(format!("{k}={v}"));
        }
    }
    if !map.get("UID").and_then(Value::as_str).is_some_and(|v| !v.is_empty()) {
        return None;
    }
    Some(parts.join("; "))
}

impl Providers {
    fn qr_url(&self, path: &str) -> String {
        format!("{}{path}", self.qr_base.trim_end_matches('/'))
    }
    /// Returns (uid, time, sign, qrcode text).
    pub async fn qr115_token(&self) -> Result<(String, String, String, String)> {
        let r = self
            .http
            .get(self.qr_url("/api/1.0/web/1.0/token/"))
            .header("User-Agent", BROWSER_UA)
            .send()
            .await
            .map_err(|_| AppError::upstream("获取 115 登录二维码失败，请检查网络"))?;
        let v = read_json(r, "115 二维码").await?;
        let d = v.get("data").unwrap_or(&Value::Null);
        let (uid, time, sign) = (text(d, &["uid"]), text(d, &["time"]), text(d, &["sign"]));
        if !ok_115(&v) || uid.is_empty() || time.is_empty() || sign.is_empty() {
            return Err(AppError::upstream(format!("115 二维码接口未返回令牌 {}", msg_115(&v))));
        }
        let mut qr = text(d, &["qrcode"]);
        if qr.is_empty() {
            qr = format!("https://115.com/scan/dg-{uid}");
        }
        Ok((uid, time, sign, qr))
    }
    /// One (long-)poll of the scan status. `Ok(None)` = transient/timeout, keep waiting.
    pub async fn qr115_status(&self, uid: &str, time: &str, sign: &str) -> Result<Option<i64>> {
        let r = match self
            .http
            .get(self.qr_url("/get/status/"))
            .header("User-Agent", BROWSER_UA)
            .query(&[("uid", uid), ("time", time), ("sign", sign)])
            .timeout(Duration::from_secs(20))
            .send()
            .await
        {
            Ok(r) => r,
            Err(_) => return Ok(None),
        };
        let Ok(v) = read_json(r, "115 二维码状态").await else { return Ok(None) };
        if let Some(st) = v.get("data").and_then(|d| d.get("status")).and_then(Value::as_i64) {
            return Ok(Some(st));
        }
        if ok_115(&v) { Ok(None) } else { Ok(Some(-1)) }
    }
    /// Exchange a confirmed scan for cookies. Returns (cookie header, safe user summary).
    pub async fn qr115_login(&self, uid: &str, app: &str) -> Result<(String, Value)> {
        let r = self
            .http
            .post(self.qr_url(&format!("/app/1.0/{app}/1.0/login/qrcode/")))
            .header("User-Agent", BROWSER_UA)
            .form(&[("account", uid)])
            .send()
            .await
            .map_err(|_| AppError::upstream("115 登录结果未收到，请重新扫码"))?;
        let v = read_json(r, "115 登录").await?;
        let d = v.get("data").unwrap_or(&Value::Null);
        if !ok_115(&v) {
            return Err(AppError::upstream(format!("115 登录未成功：{}", msg_115(&v))));
        }
        let cookie = d
            .get("cookie")
            .and_then(cookie_header)
            .ok_or_else(|| AppError::upstream("115 登录响应缺少 Cookie"))?;
        let user = json!({"user_name": text(d, &["user_name", "username", "nickname"]), "user_id": text(d, &["user_id", "uid"])});
        Ok((cookie, user))
    }
}

impl Engine {
    pub async fn qr115_start(&self, app: &str) -> Result<Value> {
        if !APPS_115.iter().any(|(a, _)| *a == app) {
            return Err(AppError::bad("不支持的 115 登录端"));
        }
        let (uid, time, sign, qr) = self.providers.qr115_token().await?;
        let mut map = self.qr.s115.lock().await;
        map.retain(|_, p| now() - p.created < QR_TTL);
        if map.len() > 20 {
            map.clear();
        }
        map.insert(uid.clone(), Pending115 { time, sign, app: app.into(), created: now() });
        Ok(json!({"uid": uid, "qr": qr_data_uri(&qr)?, "app": app, "expires_in": QR_TTL}))
    }
    pub async fn qr115_status(&self, uid: &str) -> Result<Value> {
        let pending = self.qr.s115.lock().await.get(uid).cloned();
        let Some(p) = pending else {
            return Ok(json!({"status":"expired","message":"二维码已失效，请刷新"}));
        };
        if now() - p.created >= QR_TTL {
            self.qr.s115.lock().await.remove(uid);
            return Ok(json!({"status":"expired","message":"二维码已过期，请刷新"}));
        }
        let st = self.providers.qr115_status(uid, &p.time, &p.sign).await?;
        let out = match st {
            None | Some(0) => json!({"status":"waiting","message":"请使用 115 App 扫码"}),
            Some(1) => json!({"status":"scanned","message":"已扫码，请在手机上确认登录"}),
            Some(2) => {
                // Take the session first so concurrent polls cannot log in twice.
                if self.qr.s115.lock().await.remove(uid).is_none() {
                    return Ok(json!({"status":"confirmed","message":"已登录"}));
                }
                let (cookie, user) = match self.providers.qr115_login(uid, &p.app).await {
                    Ok(v) => v,
                    Err(e) => return Ok(json!({"status":"error","message": e.message})),
                };
                {
                    let _guard = self.config_lock.lock().await;
                    let mut s = self.store.settings().await?;
                    s.cookie_115 = cookie;
                    self.store.save_settings(&s).await?;
                }
                let name = user["user_name"].as_str().unwrap_or("").to_string();
                self.store
                    .activity("settings", &format!("115 扫码登录成功{}", if name.is_empty() { String::new() } else { format!(" · {name}") }))
                    .await?;
                json!({"status":"confirmed","message":"115 登录成功，Cookie 已加密保存","user":user})
            }
            Some(-2) => {
                self.qr.s115.lock().await.remove(uid);
                json!({"status":"canceled","message":"已在手机上取消，请刷新二维码"})
            }
            Some(_) => {
                self.qr.s115.lock().await.remove(uid);
                json!({"status":"expired","message":"二维码已过期，请刷新"})
            }
        };
        Ok(out)
    }
    pub async fn re0_qr_start(&self) -> Result<Value> {
        let s = self.store.settings().await?;
        if s.mode == "demo" {
            return Err(AppError::bad("请先切换到中转 API 模式并保存签名密钥"));
        }
        let url = self.providers.auth_url(&s)?;
        *self.qr.re0.lock().await = Some(now());
        Ok(json!({"url": url, "qr": qr_data_uri(&url)?, "expires_in": QR_TTL}))
    }
    pub async fn re0_qr_status(&self) -> Result<Value> {
        let started = *self.qr.re0.lock().await;
        let s = self.store.settings().await?;
        let st = self.providers.status(&s).await?;
        if st["authorized"].as_bool() == Some(true) {
            *self.qr.re0.lock().await = None;
            return Ok(json!({"status":"confirmed","message":"RE0 已授权","user":st["user"]}));
        }
        match started {
            Some(t) if now() - t < QR_TTL => Ok(json!({"status":"waiting","message":"请用手机扫码，在 RE0 页面完成授权"})),
            _ => Ok(json!({"status":"expired","message":"授权二维码已过期，请刷新"})),
        }
    }
}
