use crate::{
    error::{AppError, Result},
    models::{Resource, Settings},
};
use hmac::{Hmac, Mac};
use reqwest::{Client, Method, Response};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, sync::Arc, time::Duration};
use url::Url;

#[derive(Clone)]
pub struct Providers {
    pub http: Client,
    share_base: String,
    tmdb_base: String,
    gateway_slots: Arc<tokio::sync::Semaphore>,
}
impl Providers {
    /// Production endpoints, overridable for offline testing / self-hosted proxies:
    /// `RE0DRIVE_115_BASE_URL` (default https://webapi.115.com) and
    /// `RE0DRIVE_TMDB_BASE_URL` (default https://api.themoviedb.org).
    pub fn new() -> Self {
        let env = |k: &str, d: &str| {
            std::env::var(k)
                .ok()
                .map(|v| v.trim().trim_end_matches('/').to_string())
                .filter(|v| !v.is_empty())
                .unwrap_or_else(|| d.into())
        };
        Self::with_bases(
            env("RE0DRIVE_115_BASE_URL", "https://webapi.115.com"),
            env("RE0DRIVE_TMDB_BASE_URL", "https://api.themoviedb.org"),
        )
    }
    pub fn with_share_base(share_base: String) -> Self {
        Self::with_bases(share_base, "https://api.themoviedb.org".into())
    }
    pub fn with_bases(share_base: String, tmdb_base: String) -> Self {
        Self {
            http: Client::builder()
                .connect_timeout(Duration::from_secs(8))
                .timeout(Duration::from_secs(25))
                .pool_idle_timeout(Duration::from_secs(90))
                .redirect(reqwest::redirect::Policy::none())
                .user_agent("RE0Drive/0.1")
                .build()
                .expect("HTTP client"),
            share_base,
            tmdb_base,
            gateway_slots: Arc::new(tokio::sync::Semaphore::new(4)),
        }
    }
    pub async fn gateway(
        &self,
        s: &Settings,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<Value> {
        if s.re0_shared_secret.is_empty() {
            return Err(AppError::bad("请先在设置中配置中转 API 签名密钥"));
        }
        let _slot = self
            .gateway_slots
            .acquire()
            .await
            .map_err(|_| AppError::upstream("中转请求队列已关闭"))?;
        let mut url = Url::parse(&s.re0_base_url).map_err(|_| AppError::bad("中转地址无效"))?;
        url.set_path(path);
        url.set_query(None);
        url.set_fragment(None);
        let bytes = body
            .map(|v| serde_json::to_vec(&v).expect("JSON value"))
            .unwrap_or_default();
        let timestamp = chrono::Utc::now().timestamp().to_string();
        let nonce = uuid::Uuid::new_v4().simple().to_string();
        let sig = sign(
            &s.re0_shared_secret,
            &canonical(
                method.as_str(),
                &url,
                &s.install_id,
                &timestamp,
                &nonce,
                &bytes,
            ),
        );
        let mut request = self
            .http
            .request(method.clone(), url)
            .header("Accept", "application/json")
            .header("X-Install-Id", &s.install_id)
            .header("X-Timestamp", timestamp)
            .header("X-Nonce", nonce)
            .header("X-Signature", sig);
        if !bytes.is_empty() {
            request = request
                .header("Content-Type", "application/json")
                .body(bytes);
        }
        let response = request.send().await.map_err(|_| {
            AppError::upstream(if method == Method::GET {
                "中转 API 请求失败，请检查地址和网络"
            } else {
                "外部操作响应未收到，结果需要核对"
            })
        })?;
        let value = read_json(response, "RE0 中转 API").await?;
        check_success(&value, "RE0 中转 API")?;
        Ok(value)
    }
    pub fn auth_url(&self, s: &Settings) -> Result<String> {
        if s.re0_shared_secret.is_empty() {
            return Err(AppError::bad("请先配置中转 API 签名密钥"));
        }
        let mut url = Url::parse(&s.re0_base_url).map_err(|_| AppError::bad("中转地址无效"))?;
        url.set_path("/auth/start");
        url.set_query(None);
        url.set_fragment(None);
        let timestamp = chrono::Utc::now().timestamp().to_string();
        let nonce = uuid::Uuid::new_v4().simple().to_string();
        url.query_pairs_mut()
            .append_pair("install_id", &s.install_id)
            .append_pair("ts", &timestamp)
            .append_pair("nonce", &nonce);
        let signature = sign(
            &s.re0_shared_secret,
            &canonical("GET", &url, &s.install_id, &timestamp, &nonce, b""),
        );
        url.query_pairs_mut().append_pair("sig", &signature);
        Ok(url.into())
    }
    pub async fn status(&self, s: &Settings) -> Result<Value> {
        if s.mode == "demo" {
            return Ok(
                json!({"demo":true,"authorized":false,"message":"演示数据 · 尚未连接真实账号"}),
            );
        }
        let token = self
            .gateway(s, Method::GET, "/api/token/status", None)
            .await?;
        let data = token.get("data").unwrap_or(&token);
        let authorized = data
            .get("has_access_token")
            .or_else(|| token.get("has_access_token"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let user = if authorized {
            let raw = self.gateway(s, Method::GET, "/api/me", None).await?;
            let data = raw.get("data").unwrap_or(&raw);
            let mut safe = serde_json::Map::new();
            for key in [
                "nickname",
                "username",
                "name",
                "points",
                "point",
                "credit",
                "credits",
                "level",
                "group",
                "user_group",
            ] {
                if let Some(v) = data.get(key).filter(|v| v.is_string() || v.is_number()) {
                    safe.insert(key.into(), v.clone());
                }
            }
            Value::Object(safe)
        } else {
            Value::Null
        };
        Ok(json!({"demo":false,"authorized":authorized,"user":user}))
    }
    pub async fn checkin(&self, s: &Settings) -> Result<Value> {
        if s.mode == "demo" {
            return Ok(json!({"demo":true,"message":"演示模式未发送签到请求"}));
        }
        self.gateway(
            s,
            Method::POST,
            "/api/checkin",
            Some(json!({"is_gambler":false})),
        )
        .await
    }
    pub async fn search(
        &self,
        s: &Settings,
        query: &str,
        media_type: &str,
    ) -> Result<Vec<Resource>> {
        if s.mode == "demo" {
            return Ok(demo_resources(query, media_type));
        }
        let mut ids = vec![];
        let query = query.trim();
        let id = query.strip_prefix("tmdb:").unwrap_or(query);
        if !id.is_empty() && id.len() <= 12 && id.bytes().all(|b| b.is_ascii_digit()) {
            ids.push(id.to_string());
        } else {
            if s.tmdb_api_key.is_empty() {
                return Err(AppError::bad(
                    "按片名搜索需要 TMDB API Key；也可以直接输入 tmdb:数字ID",
                ));
            }
            let response = self
                .http
                .get(format!(
                    "{}/3/search/{media_type}",
                    self.tmdb_base.trim_end_matches('/')
                ))
                .query(&[
                    ("api_key", s.tmdb_api_key.as_str()),
                    ("query", query),
                    ("language", "zh-CN"),
                ])
                .send()
                .await
                .map_err(|_| AppError::upstream("TMDB 搜索请求失败"))?;
            let v = read_json(response, "TMDB").await?;
            if let Some(items) = v.get("results").and_then(Value::as_array) {
                for item in items.iter().take(4) {
                    if let Some(id) = item.get("id").and_then(Value::as_u64) {
                        ids.push(id.to_string());
                    }
                }
            }
        }
        let mut tasks = tokio::task::JoinSet::new();
        for id in ids {
            let p = self.clone();
            let s = s.clone();
            let ty = media_type.to_string();
            tasks.spawn(async move {
                let v = p
                    .gateway(&s, Method::GET, &format!("/api/resources/{ty}/{id}"), None)
                    .await?;
                let mut resources = normalize_resources(&v);
                for r in &mut resources {
                    r.tmdb_id = id.clone();
                    r.media_type = ty.clone();
                }
                Ok::<_, AppError>(resources)
            });
        }
        let mut result = vec![];
        let mut seen = BTreeSet::new();
        while let Some(res) = tasks.join_next().await {
            for item in res.map_err(|_| AppError::upstream("资源查询任务失败"))?? {
                if seen.insert(item.slug.clone()) {
                    result.push(item);
                }
            }
        }
        result.sort_by_key(|r| {
            (
                !r.is_unlocked,
                r.unlock_points.unwrap_or(i64::MAX),
                r.title.clone(),
            )
        });
        Ok(result)
    }
    pub async fn detail(&self, s: &Settings, slug: &str) -> Result<Value> {
        if s.mode == "demo" {
            return Ok(json!({"data":{"unlock_points":0,"is_unlocked":true}}));
        }
        self.gateway(
            s,
            Method::GET,
            &format!("/api/shares/{}", encode_segment(slug)),
            None,
        )
        .await
    }
    pub async fn unlock(&self, s: &Settings, slug: &str) -> Result<String> {
        if s.mode == "demo" {
            return Ok("https://115.com/s/demo_share?password=demo".into());
        }
        let v = self
            .gateway(
                s,
                Method::POST,
                "/api/resources/unlock",
                Some(json!({"slug":slug})),
            )
            .await?;
        let data = v.get("data").unwrap_or(&v);
        let raw = text(data, &["full_url", "url"]);
        if raw.is_empty() {
            return Err(AppError::upstream("解锁响应未包含分享链接，请核对资源状态"));
        }
        let mut url =
            Url::parse(raw.trim()).map_err(|_| AppError::upstream("解锁返回的分享地址无效"))?;
        let code = text(data, &["access_code", "receive_code", "password"]);
        if !code.is_empty()
            && !url.query_pairs().any(|(k, _)| {
                matches!(
                    k.as_ref(),
                    "password" | "pwd" | "receive_code" | "access_code"
                )
            })
        {
            url.query_pairs_mut().append_pair("password", &code);
        }
        Ok(url.into())
    }
    pub async fn receive_115(
        &self,
        s: &Settings,
        share_url: &str,
        target_cid: &str,
    ) -> Result<usize> {
        let (share_code, receive_code) = parse_115_share(share_url)?;
        if s.mode == "demo" {
            return Ok(3);
        }
        if s.cookie_115.is_empty() {
            return Err(AppError::bad("请先配置 115 Cookie"));
        }
        let mut file_ids = BTreeSet::new();
        let mut offset = 0usize;
        for _ in 0..120 {
            let url = format!("{}/share/snap", self.share_base.trim_end_matches('/'));
            let response = self
                .http
                .get(&url)
                .header("Referer", &self.share_base)
                .query(&[
                    ("share_code", share_code.as_str()),
                    ("receive_code", receive_code.as_str()),
                    ("cid", "0"),
                    ("offset", &offset.to_string()),
                    ("limit", "100"),
                    ("app_ver", "99.99.99.99"),
                ])
                .send()
                .await
                .map_err(|_| AppError::upstream("读取 115 分享列表失败"))?;
            let value = read_json(response, "115 分享").await?;
            check_success(&value, "115 分享")?;
            let data = value.get("data").unwrap_or(&value);
            let list = data
                .get("list")
                .and_then(Value::as_array)
                .ok_or_else(|| AppError::upstream("115 分享响应缺少文件列表"))?;
            for item in list {
                let id = ["fid", "cid", "file_id"]
                    .iter()
                    .map(|key| text(item, &[*key]))
                    .find(|id| id != "0" && !id.is_empty())
                    .unwrap_or_default();
                if !id.is_empty() {
                    if !crate::models::valid_cid(&id) {
                        return Err(AppError::upstream("115 分享返回了无效文件 ID"));
                    }
                    file_ids.insert(id);
                }
            }
            offset += list.len();
            if list.len() < 100
                || data
                    .get("count")
                    .and_then(Value::as_u64)
                    .is_some_and(|n| offset >= n as usize)
            {
                break;
            }
            if offset >= 12000 {
                return Err(AppError::bad("分享超过 12000 个顶层条目，请拆分转存"));
            }
        }
        if file_ids.is_empty() {
            return Err(AppError::bad("分享没有可转存的文件"));
        }
        let ids = file_ids.iter().cloned().collect::<Vec<_>>().join(",");
        let response = self
            .http
            .post(format!(
                "{}/share/receive",
                self.share_base.trim_end_matches('/')
            ))
            .header("Cookie", &s.cookie_115)
            .header("Referer", &self.share_base)
            .form(&[
                ("share_code", share_code.as_str()),
                ("receive_code", receive_code.as_str()),
                ("file_id", ids.as_str()),
                ("cid", target_cid),
            ])
            .send()
            .await
            .map_err(|_| AppError::upstream("115 接收响应未收到，请在网盘中核对结果"))?;
        let v = read_json(response, "115 转存").await?;
        if !text(&v, &["error", "message", "msg"]).contains("无需重复接收") {
            check_success(&v, "115 转存")?;
            if v.get("state").and_then(Value::as_bool) != Some(true) {
                return Err(AppError::upstream(
                    "115 接收响应缺少成功标识，请在网盘中核对结果",
                ));
            }
        }
        Ok(file_ids.len())
    }
}
impl Default for Providers {
    fn default() -> Self {
        Self::new()
    }
}

pub fn canonical(
    method: &str,
    url: &Url,
    install_id: &str,
    timestamp: &str,
    nonce: &str,
    body: &[u8],
) -> String {
    let mut pairs = url
        .query_pairs()
        .filter(|(k, _)| k != "sig" && k != "signature")
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect::<Vec<_>>();
    pairs.sort();
    let mut serializer = url::form_urlencoded::Serializer::new(String::new());
    for (k, v) in pairs {
        serializer.append_pair(&k, &v);
    }
    let query = serializer.finish();
    format!(
        "{}\n{}\n{}\n{}\n{}\n{}\n{}",
        method.to_uppercase(),
        url.path(),
        query,
        install_id,
        timestamp,
        nonce,
        hex::encode(Sha256::digest(body))
    )
}
pub fn sign(secret: &str, canonical: &str) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("HMAC key");
    mac.update(canonical.as_bytes());
    hex::encode(mac.finalize().into_bytes())
}
fn encode_segment(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes())
        .collect::<String>()
        .replace('+', "%20")
}
pub fn valid_slug(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 200
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}
pub fn parse_115_share(raw: &str) -> Result<(String, String)> {
    let u = Url::parse(raw).map_err(|_| AppError::bad("115 分享地址无效"))?;
    let host = u.host_str().unwrap_or("");
    if !["115.com", "115cdn.com", "anxia.com"]
        .iter()
        .any(|d| host == *d || host.ends_with(&format!(".{d}")))
        || !matches!(u.scheme(), "https" | "http")
        || !u.username().is_empty()
        || u.password().is_some()
    {
        return Err(AppError::bad("解锁返回的链接不属于 115 分享"));
    }
    let segments = u
        .path_segments()
        .map(|s| s.collect::<Vec<_>>())
        .unwrap_or_default();
    if segments.len() < 2 || segments[0] != "s" || !valid_slug(segments[1]) {
        return Err(AppError::bad("115 分享码无效"));
    }
    let code = ["password", "pwd", "receive_code", "access_code"]
        .iter()
        .find_map(|key| {
            u.query_pairs()
                .find(|(k, _)| k == *key)
                .map(|(_, v)| v.into_owned())
        })
        .unwrap_or_default();
    Ok((segments[1].into(), code))
}
pub fn text(v: &Value, keys: &[&str]) -> String {
    keys.iter()
        .find_map(|k| {
            v.get(*k).and_then(|v| match v {
                Value::String(s) if !s.is_empty() => Some(s.clone()),
                Value::Number(n) => Some(n.to_string()),
                _ => None,
            })
        })
        .unwrap_or_default()
}
pub fn points(v: &Value) -> Option<i64> {
    for key in [
        "actual_unlock_points",
        "unlock_points",
        "need_points",
        "cost_points",
    ] {
        if let Some(value) = v.get(key) {
            if let Some(n) = value
                .as_i64()
                .or_else(|| value.as_str().and_then(|s| s.parse().ok()))
            {
                return Some(n);
            }
        }
    }
    None
}
pub fn owned(v: &Value) -> bool {
    v.get("is_unlocked")
        .and_then(Value::as_bool)
        .unwrap_or(false)
        || v.get("already_owned")
            .and_then(Value::as_bool)
            .unwrap_or(false)
}
pub fn normalize_resources(v: &Value) -> Vec<Resource> {
    fn walk(v: &Value, hint: &str, out: &mut Vec<Resource>, depth: usize) {
        if depth > 12 || out.len() >= 500 {
            return;
        }
        if let Some(items) = v.as_array() {
            for item in items {
                walk(item, hint, out, depth + 1);
            }
            return;
        }
        let Some(map) = v.as_object() else {
            return;
        };
        let mut slug = text(v, &["slug", "resource_slug", "share_slug"]);
        let source = text(v, &["hdhive_url", "resource_url", "source_url"]);
        if slug.is_empty() && !source.is_empty() {
            if let Ok(u) = Url::parse(&source) {
                slug = u
                    .path_segments()
                    .and_then(|s| s.filter(|v| !v.is_empty()).next_back())
                    .unwrap_or("")
                    .into();
            }
        }
        if valid_slug(&slug) {
            let mut provider = text(
                v,
                &[
                    "website",
                    "website_code",
                    "resource_website",
                    "provider",
                    "pan_type",
                    "disk_type",
                ],
            );
            if provider.is_empty() {
                provider = hint.into();
            }
            if provider.contains("115") {
                provider = "115".into();
            }
            if provider != "115" {
                return;
            }
            out.push(Resource {
                slug: slug.clone(),
                title: text(v, &["title", "name", "resource_title", "file_name"]),
                provider,
                source_url: source,
                unlock_points: points(v),
                is_unlocked: owned(v),
                quality: text(v, &["quality", "resolution", "version"]),
                size: text(v, &["size", "file_size"]),
                tmdb_id: String::new(),
                media_type: String::new(),
            });
            return;
        }
        for (k, item) in map {
            if item.is_object() || item.is_array() {
                walk(
                    item,
                    if k.contains("115") { "115" } else { hint },
                    out,
                    depth + 1,
                );
            }
        }
    }
    let mut result = vec![];
    walk(v, "", &mut result, 0);
    result
}
async fn read_json(mut response: Response, label: &str) -> Result<Value> {
    let status = response.status();
    let mut bytes = vec![];
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| AppError::upstream(format!("{label} 响应读取失败")))?
    {
        if bytes.len() + chunk.len() > 4 * 1024 * 1024 {
            return Err(AppError::upstream(format!("{label} 响应过大")));
        }
        bytes.extend_from_slice(&chunk);
    }
    if !status.is_success() {
        return Err(AppError::upstream(format!(
            "{label} 返回 HTTP {}",
            status.as_u16()
        )));
    }
    serde_json::from_slice(&bytes)
        .map_err(|_| AppError::upstream(format!("{label} 返回了非 JSON 内容")))
}
fn check_success(v: &Value, label: &str) -> Result<()> {
    let failed = v.get("success").and_then(Value::as_bool) == Some(false)
        || v.get("state").and_then(Value::as_bool) == Some(false);
    let code = text(v, &["code"]);
    let explicit_success = v.get("success").and_then(Value::as_bool) == Some(true)
        || v.get("state").and_then(Value::as_bool) == Some(true);
    if failed || (!explicit_success && !code.is_empty() && code != "200" && code != "0") {
        let code = if code.len() < 50 { code } else { String::new() };
        return Err(AppError::upstream(format!(
            "{label} 操作未成功（{code}），请检查授权、积分或账号状态"
        )));
    }
    Ok(())
}
pub fn demo_resources(query: &str, media_type: &str) -> Vec<Resource> {
    let title = if query.contains("星际") || query.to_lowercase().contains("interstellar") {
        "星际穿越"
    } else if query.contains("绝命") || media_type == "tv" {
        "绝命毒师"
    } else {
        "沙丘：第二部"
    };
    [
        ("demo_2160", "2160p REMUX · Dolby Vision", 0, "82.4 GB"),
        ("demo_4k", "2160p WEB-DL · HDR10", 2, "24.6 GB"),
        ("demo_1080", "1080p BluRay · x265", 4, "9.8 GB"),
    ]
    .into_iter()
    .map(|(slug, quality, p, size)| Resource {
        slug: slug.into(),
        title: format!("{title} · {quality}"),
        provider: "115".into(),
        source_url: String::new(),
        unlock_points: Some(p),
        is_unlocked: p == 0,
        quality: quality.into(),
        size: size.into(),
        tmdb_id: "693134".into(),
        media_type: media_type.into(),
    })
    .collect()
}
