//! Telegram bot (long polling).
//!
//! Standard Bot API only (`getUpdates` / `sendMessage` / `answerCallbackQuery` /
//! `editMessageText`). No public webhook. Token and allowlist come from encrypted settings;
//! the token is never echoed. Only allowlisted chat/user ids may search or transfer.
use crate::{
    engine::Engine,
    models::{Resource, Settings},
    store::now,
};
use reqwest::Client;
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};
use tracing::{info, warn};

const TG_API: &str = "https://api.telegram.org";
const MAX_RESULTS: usize = 12;
const PENDING_TTL_SECS: i64 = 30 * 60;

#[derive(Clone)]
struct Pending {
    resource: Resource,
    chat_id: i64,
    created: i64,
}

#[derive(Default)]
struct State {
    offset: i64,
    pending: HashMap<String, Pending>,
}

pub struct Telegram {
    http: Client,
    state: Mutex<State>,
    /// Override base URL for tests (points at the mock).
    base: String,
}

impl Default for Telegram {
    fn default() -> Self {
        Self {
            http: Client::builder()
                .connect_timeout(Duration::from_secs(8))
                .timeout(Duration::from_secs(60))
                .build()
                .expect("tg client"),
            state: Mutex::new(State::default()),
            base: std::env::var("RE0DRIVE_TG_BASE_URL").unwrap_or_else(|_| TG_API.into()),
        }
    }
}

impl Telegram {
    pub fn with_base(base: String) -> Self {
        Self {
            base,
            ..Default::default()
        }
    }
}

impl Engine {
    pub fn telegram_handle(&self) -> &Telegram {
        &self.tg
    }
}

/// Best-effort broadcast to every allowlisted chat. No-op when the bot is disabled.
pub async fn notify_all(engine: &Engine, html: &str) {
    let s = match engine.store.settings().await {
        Ok(s) => s,
        Err(_) => return,
    };
    if !s.tg_enabled || s.tg_bot_token.is_empty() {
        return;
    }
    for chat in s.tg_allowlist() {
        let _ = engine
            .tg
            .send_html(&s.tg_bot_token, chat, html, None)
            .await;
    }
}

/// Notify a single chat about a finished job.
pub async fn notify_job(engine: &Engine, chat_id: Option<i64>, job_id: &str, status: &str, message: &str, title: &str) {
    let s = match engine.store.settings().await {
        Ok(s) => s,
        Err(_) => return,
    };
    if !s.tg_enabled || s.tg_bot_token.is_empty() {
        return;
    }
    let text = format!(
        "📦 <b>{}</b>\n状态：<code>{}</code>\n{}\n任务 <code>{}</code>",
        esc(title),
        esc(status),
        esc(message),
        esc(job_id)
    );
    let chats: Vec<i64> = match chat_id {
        Some(c) if s.tg_allowlist().contains(&c) => vec![c],
        _ => s.tg_allowlist(),
    };
    for c in chats {
        let _ = engine.tg.send_html(&s.tg_bot_token, c, &text, None).await;
    }
}

impl Telegram {
    async fn api(&self, token: &str, method: &str, body: Option<Value>) -> Result<Value, String> {
        let url = format!("{}/bot{}/{}", self.base.trim_end_matches('/'), token, method);
        let mut req = self.http.post(&url);
        if let Some(b) = body {
            req = req.json(&b);
        }
        let resp = req.send().await.map_err(|e| e.to_string())?;
        let v: Value = resp.json().await.map_err(|e| e.to_string())?;
        if v.get("ok").and_then(Value::as_bool) != Some(true) {
            return Err(v
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or("telegram api error")
                .into());
        }
        Ok(v.get("result").cloned().unwrap_or(Value::Null))
    }

    pub async fn send_html(
        &self,
        token: &str,
        chat_id: i64,
        text: &str,
        reply_markup: Option<Value>,
    ) -> Result<Value, String> {
        let mut body = json!({
            "chat_id": chat_id,
            "text": text,
            "parse_mode": "HTML",
            "disable_web_page_preview": true,
        });
        if let Some(m) = reply_markup {
            body["reply_markup"] = m;
        }
        self.api(token, "sendMessage", Some(body)).await
    }

    async fn answer_callback(&self, token: &str, id: &str, text: &str) -> Result<(), String> {
        self.api(
            token,
            "answerCallbackQuery",
            Some(json!({"callback_query_id": id, "text": text, "show_alert": false})),
        )
        .await
        .map(|_| ())
    }

    /// One long-poll iteration. Safe to call repeatedly from the engine loop.
    pub async fn poll_once(&self, engine: &Engine) -> anyhow::Result<()> {
        let s = engine.store.settings().await?;
        if !s.tg_enabled || s.tg_bot_token.is_empty() || s.tg_allowlist().is_empty() {
            return Ok(());
        }
        let offset = self.state.lock().unwrap().offset;
        let result = match self
            .api(
                &s.tg_bot_token,
                "getUpdates",
                Some(json!({"offset": offset, "timeout": 25, "allowed_updates": ["message","callback_query"]})),
            )
            .await
        {
            Ok(v) => v,
            Err(e) => {
                warn!(error=%e, "telegram getUpdates failed");
                tokio::time::sleep(Duration::from_secs(3)).await;
                return Ok(());
            }
        };
        let updates = result.as_array().cloned().unwrap_or_default();
        for u in updates {
            let update_id = u.get("update_id").and_then(Value::as_i64).unwrap_or(0);
            {
                let mut st = self.state.lock().unwrap();
                st.offset = update_id + 1;
                // Drop expired pending transfers.
                let cutoff = now() - PENDING_TTL_SECS;
                st.pending.retain(|_, p| p.created >= cutoff);
            }
            if let Some(cb) = u.get("callback_query") {
                self.handle_callback(engine, &s, cb).await;
            } else if let Some(msg) = u.get("message") {
                self.handle_message(engine, &s, msg).await;
            }
        }
        Ok(())
    }

    fn allowed(s: &Settings, chat_id: i64, user_id: Option<i64>) -> bool {
        let list = s.tg_allowlist();
        list.contains(&chat_id) || user_id.is_some_and(|u| list.contains(&u))
    }

    async fn handle_message(&self, engine: &Engine, s: &Settings, msg: &Value) {
        let chat_id = msg
            .pointer("/chat/id")
            .and_then(Value::as_i64)
            .unwrap_or(0);
        let user_id = msg.pointer("/from/id").and_then(Value::as_i64);
        let text = msg
            .get("text")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        if text.is_empty() {
            return;
        }
        if !Self::allowed(s, chat_id, user_id) {
            let _ = self
                .send_html(
                    &s.tg_bot_token,
                    chat_id,
                    "⛔ 未授权。请在 RE0Drive 设置中把你的 chat id 加入 Telegram 白名单。",
                    None,
                )
                .await;
            return;
        }
        let (cmd, rest) = split_cmd(&text);
        match cmd.as_str() {
            "/start" | "/help" => {
                let _ = self
                    .send_html(
                        &s.tg_bot_token,
                        chat_id,
                        "🤖 <b>RE0Drive Bot</b>\n\
/search &lt;片名或 TMDB ID&gt; — 搜索电影（默认）\n\
/movie &lt;…&gt; / /tv &lt;…&gt; — 指定类型\n\
点击版本按钮确认转存（仅 115，不会自动解锁）\n\
/checkin — RE0 签到\n\
/cookie — 检查 115 Cookie\n\
/jobs — 最近任务",
                        None,
                    )
                    .await;
            }
            "/search" | "/s" | "/movie" | "/tv" => {
                let media = if cmd == "/tv" { "tv" } else { "movie" };
                if rest.is_empty() {
                    let _ = self
                        .send_html(&s.tg_bot_token, chat_id, "用法：/search 片名 或 /movie 550", None)
                        .await;
                    return;
                }
                let _ = self
                    .send_html(
                        &s.tg_bot_token,
                        chat_id,
                        &format!("🔍 正在搜索 <b>{}</b>…", esc(&rest)),
                        None,
                    )
                    .await;
                let settings = match engine.store.settings().await {
                    Ok(v) => v,
                    Err(e) => {
                        let _ = self
                            .send_html(&s.tg_bot_token, chat_id, &format!("配置读取失败：{}", esc(&e.to_string())), None)
                            .await;
                        return;
                    }
                };
                let resources = match engine
                    .providers
                    .search_with(&settings, &rest, None, media)
                    .await
                {
                    Ok(r) => r,
                    Err(e) => {
                        let _ = self
                            .send_html(&s.tg_bot_token, chat_id, &format!("搜索失败：{}", esc(&e.message)), None)
                            .await;
                        return;
                    }
                };
                if resources.is_empty() {
                    let _ = self
                        .send_html(
                            &s.tg_bot_token,
                            chat_id,
                            "未找到资源。",
                            None,
                        )
                        .await;
                    return;
                }
                let _ = self
                    .send_results(engine, s, chat_id, &resources)
                    .await;
            }
            "/checkin" => match engine.checkin().await {
                Ok(v) => {
                    let msg = v
                        .pointer("/data/message")
                        .or_else(|| v.get("message"))
                        .and_then(Value::as_str)
                        .unwrap_or("签到完成");
                    let _ = self
                        .send_html(&s.tg_bot_token, chat_id, &format!("✅ {}", esc(msg)), None)
                        .await;
                }
                Err(e) => {
                    let _ = self
                        .send_html(&s.tg_bot_token, chat_id, &format!("签到失败：{}", esc(&e.message)), None)
                        .await;
                }
            },
            "/cookie" => {
                let probe = engine.probe_cookie_115().await;
                let text = match probe {
                    crate::cookie::CookieProbe::Valid => "✅ 115 Cookie 有效".into(),
                    crate::cookie::CookieProbe::Expired => "⚠️ 115 Cookie 已失效，请扫码重新登录".into(),
                    crate::cookie::CookieProbe::Empty => "⚠️ 尚未配置 115 Cookie".into(),
                    crate::cookie::CookieProbe::Error(m) => {
                        format!("❓ 检查失败：{}", esc(&m))
                    }
                };
                let _ = self.send_html(&s.tg_bot_token, chat_id, &text, None).await;
            }
            "/jobs" => match engine.store.jobs().await {
                Ok(jobs) => {
                    if jobs.is_empty() {
                        let _ = self
                            .send_html(&s.tg_bot_token, chat_id, "暂无任务。", None)
                            .await;
                        return;
                    }
                    let mut lines = vec!["📋 <b>最近任务</b>".into()];
                    for j in jobs.into_iter().take(8) {
                        lines.push(format!(
                            "• <code>{}</code> {} — {}",
                            esc(&j.status),
                            esc(&j.resource.title),
                            esc(&j.message)
                        ));
                    }
                    let _ = self
                        .send_html(&s.tg_bot_token, chat_id, &lines.join("\n"), None)
                        .await;
                }
                Err(e) => {
                    let _ = self
                        .send_html(&s.tg_bot_token, chat_id, &format!("读取失败：{}", esc(&e.to_string())), None)
                        .await;
                }
            },
            _ => {
                // Bare text that looks like a search query → treat as /search.
                if !text.starts_with('/') {
                    let mut fake = msg.clone();
                    if let Some(o) = fake.as_object_mut() {
                        o.insert("text".into(), json!(format!("/search {text}")));
                    }
                    Box::pin(self.handle_message(engine, s, &fake)).await;
                }
            }
        }
    }

    async fn send_results(
        &self,
        _engine: &Engine,
        s: &Settings,
        chat_id: i64,
        resources: &[Resource],
    ) -> Result<(), String> {
        let mut lines = vec![format!(
            "🎬 找到 <b>{}</b> 个版本（点击按钮确认转存；非 115 不可选）：",
            resources.len()
        )];
        let mut keyboard: Vec<Vec<Value>> = vec![];
        for (i, r) in resources.iter().take(MAX_RESULTS).enumerate() {
            let n = i + 1;
            let origin = "RE0";
            let pts = if r.is_unlocked {
                "已拥有".into()
            } else {
                match r.unlock_points {
                    Some(p) => format!("{p} 分"),
                    None => "积分未知".into(),
                }
            };
            let res = flat(&r.video_resolution);
            let src = flat(&r.source);
            let subs = [flat(&r.subtitle_language), flat(&r.subtitle_type)]
                .into_iter()
                .filter(|x| !x.is_empty())
                .collect::<Vec<_>>()
                .join(" · ");
            lines.push(format!(
                "\n<b>{n}.</b> {} [{}]\n📦 {} · 🖥 {} · 🎞 {}\n💬 {} · 💰 {}{}",
                esc(&r.title),
                origin,
                esc(if r.size.is_empty() { "大小未知" } else { &r.size }),
                esc(if res.is_empty() { "—" } else { &res }),
                esc(if src.is_empty() { "—" } else { &src }),
                esc(if subs.is_empty() { "—" } else { &subs }),
                pts,
                if r.remark.is_empty() {
                    String::new()
                } else {
                    format!("\n📝 {}", esc(&r.remark))
                }
            ));
            if r.selectable {
                let key = short_key();
                self.state.lock().unwrap().pending.insert(
                    key.clone(),
                    Pending {
                        resource: r.clone(),
                        chat_id,
                        created: now(),
                    },
                );
                keyboard.push(vec![json!({
                    "text": format!("转存 #{n} · {}", truncate(&r.title, 28)),
                    "callback_data": format!("xfer:{key}"),
                })]);
            } else {
                keyboard.push(vec![json!({
                    "text": format!("不可转存 · {}", r.pan_type),
                    "callback_data": "noop",
                })]);
            }
        }
        self.send_html(
            &s.tg_bot_token,
            chat_id,
            &lines.join(""),
            Some(json!({"inline_keyboard": keyboard})),
        )
        .await
        .map(|_| ())
    }

    async fn handle_callback(&self, engine: &Engine, s: &Settings, cb: &Value) {
        let id = cb.get("id").and_then(Value::as_str).unwrap_or("");
        let data = cb.get("data").and_then(Value::as_str).unwrap_or("");
        let chat_id = cb
            .pointer("/message/chat/id")
            .and_then(Value::as_i64)
            .unwrap_or(0);
        let user_id = cb.pointer("/from/id").and_then(Value::as_i64);
        if !Self::allowed(s, chat_id, user_id) {
            let _ = self.answer_callback(&s.tg_bot_token, id, "未授权").await;
            return;
        }
        if data == "noop" {
            let _ = self.answer_callback(&s.tg_bot_token, id, "该版本不可转存").await;
            return;
        }
        let Some(key) = data.strip_prefix("xfer:") else {
            let _ = self.answer_callback(&s.tg_bot_token, id, "未知操作").await;
            return;
        };
        let pending = self.state.lock().unwrap().pending.remove(key);
        let Some(p) = pending else {
            let _ = self
                .answer_callback(&s.tg_bot_token, id, "已过期，请重新搜索")
                .await;
            return;
        };
        if p.chat_id != chat_id {
            let _ = self.answer_callback(&s.tg_bot_token, id, "会话不匹配").await;
            return;
        }
        let _ = self
            .answer_callback(&s.tg_bot_token, id, "已加入转存队列")
            .await;
        match engine
            .enqueue_opt(p.resource.clone(), None, None, None)
            .await
        {
            Ok(Some(job)) => {
                let _ = self
                    .send_html(
                        &s.tg_bot_token,
                        chat_id,
                        &format!(
                            "✅ 已排队：<b>{}</b>\n任务 <code>{}</code>（确认版本后才会解锁）",
                            esc(&job.resource.title),
                            esc(&job.id)
                        ),
                        None,
                    )
                    .await;
                // Remember which chat to notify when the job finishes.
                let mut st = self.state.lock().unwrap();
                st.pending.insert(
                    format!("job:{}", job.id),
                    Pending {
                        resource: p.resource,
                        chat_id,
                        created: now(),
                    },
                );
            }
            Ok(None) => {
                let _ = self
                    .send_html(&s.tg_bot_token, chat_id, "该资源在目标目录已有任务。", None)
                    .await;
            }
            Err(e) => {
                let _ = self
                    .send_html(
                        &s.tg_bot_token,
                        chat_id,
                        &format!("排队失败：{}", esc(&e.message)),
                        None,
                    )
                    .await;
            }
        }
    }

    /// Look up the chat that queued a job (if any).
    pub fn chat_for_job(&self, job_id: &str) -> Option<i64> {
        self.state
            .lock()
            .unwrap()
            .pending
            .get(&format!("job:{job_id}"))
            .map(|p| p.chat_id)
    }
}

fn split_cmd(text: &str) -> (String, String) {
    let mut parts = text.splitn(2, char::is_whitespace);
    let cmd = parts.next().unwrap_or("").to_string();
    // Strip @BotName suffix.
    let cmd = cmd.split('@').next().unwrap_or(&cmd).to_string();
    let rest = parts.next().unwrap_or("").trim().to_string();
    (cmd, rest)
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn flat(v: &Value) -> String {
    match v {
        Value::Array(a) => a
            .iter()
            .filter_map(|x| x.as_str())
            .collect::<Vec<_>>()
            .join(" / "),
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

fn truncate(s: &str, n: usize) -> String {
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if i >= n {
            out.push('…');
            break;
        }
        out.push(ch);
    }
    out
}

fn short_key() -> String {
    use rand::RngCore;
    let mut b = [0u8; 8];
    rand::thread_rng().fill_bytes(&mut b);
    hex::encode(b)
}

/// Background task: long-poll Telegram forever until `stop` flips.
pub async fn run(engine: Arc<Engine>, mut stop: tokio::sync::watch::Receiver<bool>) {
    loop {
        if *stop.borrow() {
            return;
        }
        tokio::select! {
            _ = stop.changed() => return,
            _ = async {
                if let Err(e) = engine.tg.poll_once(&engine).await {
                    warn!(error=%e, "telegram poll");
                }
                // When bot is disabled, don't spin.
                let enabled = engine
                    .store
                    .settings()
                    .await
                    .map(|s| s.tg_enabled && !s.tg_bot_token.is_empty())
                    .unwrap_or(false);
                if !enabled {
                    tokio::time::sleep(Duration::from_secs(5)).await;
                }
            } => {}
        }
    }
}

pub fn spawn(engine: Arc<Engine>, stop: tokio::sync::watch::Receiver<bool>) {
    info!("telegram bot task starting");
    tokio::spawn(run(engine, stop));
}
