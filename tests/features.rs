//! 115 Cookie keep-alive + Telegram bot — in-process mocks only.
//! Nothing reaches api.telegram.org or real 115.
use axum::{
    Router,
    body::{Body, to_bytes},
    extract::{Request, State},
    http::Method,
    response::Response,
    routing::any,
};
use re0drive::{
    cookie::CookieProbe,
    engine::Engine,
    providers::Providers,
    store::Store,
    telegram::Telegram,
};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use url::Url;

struct Temp(PathBuf);
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
struct Server {
    base: String,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}
async fn serve(router: Router) -> Server {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    Server {
        base,
        task: tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        }),
    }
}
fn j(v: Value) -> Response {
    Response::builder()
        .header("content-type", "application/json")
        .body(Body::from(v.to_string()))
        .unwrap()
}

#[derive(Default)]
struct Calls {
    log: Mutex<Vec<String>>,
    unlocks: AtomicUsize,
    cookie_ok: Mutex<bool>,
    tg_sent: Mutex<Vec<Value>>,
    tg_updates: Mutex<Vec<Value>>,
}
impl Calls {
    fn push(&self, s: impl Into<String>) {
        self.log.lock().unwrap().push(s.into());
    }
    fn all(&self) -> Vec<String> {
        self.log.lock().unwrap().clone()
    }
}

async fn fixture(State(c): State<Arc<Calls>>, req: Request) -> Response {
    let method = req.method().clone();
    let uri = req.uri().clone();
    let headers = req.headers().clone();
    let body = to_bytes(req.into_body(), 64 * 1024).await.unwrap();
    let path = uri.path().to_string();
    let q: HashMap<String, String> = Url::parse(&format!("http://x{uri}"))
        .unwrap()
        .query_pairs()
        .into_owned()
        .collect();

    // --- Telegram Bot API (path /bot<token>/<method>) ---
    if let Some(rest) = path.strip_prefix("/bot") {
        let method_name = rest.split('/').nth(1).unwrap_or("");
        c.push(format!("TG {method_name}"));
        if method_name == "getUpdates" {
            let mut u = c.tg_updates.lock().unwrap();
            let out = u.drain(..).collect::<Vec<_>>();
            return j(json!({"ok": true, "result": out}));
        }
        if method_name == "sendMessage" {
            let v: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
            // Never echo the token via URL into stored messages — just keep the body.
            c.tg_sent.lock().unwrap().push(v.clone());
            return j(json!({"ok": true, "result": {"message_id": 1}}));
        }
        if method_name == "answerCallbackQuery" {
            return j(json!({"ok": true, "result": true}));
        }
        return j(json!({"ok": false, "description": "unknown tg method"}));
    }


    // --- RE0 relay (HMAC not required here — we only care about empty fallback) ---
    if path.starts_with("/api/resources/") && method == Method::GET {
        c.push(format!("RELAY {path}"));
        if path.ends_with("/999") {
            return j(json!({"success":true,"data":[],"total":0}));
        }
        return j(json!({"success":true,"data":{"115":[{
            "slug":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","title":"Fixture 2160p",
            "unlock_points":0,"is_unlocked":true,"pan_type":"115",
            "share_size":"40 GB","video_resolution":["2160P"],"source":["REMUX"],
            "subtitle_language":["简中"],"subtitle_type":["内封"],"remark":"fixture"
        }]},"total":1}));
    }
    if path == "/api/resources/unlock" {
        c.unlocks.fetch_add(1, Ordering::SeqCst);
        c.push("RELAY unlock");
        return j(json!({"success":true,"data":{"full_url":"https://115.com/s/should-not","access_code":"xx"}}));
    }
    if path.starts_with("/api/shares/") {
        return j(json!({"success":true,"data":{"unlock_points":0,"is_unlocked":true,"pan_type":"115"}}));
    }

    // --- 115 ---
    if path == "/files" {
        c.push(format!("FILES cid={}", q.get("cid").cloned().unwrap_or_default()));
        if !*c.cookie_ok.lock().unwrap() {
            return j(json!({"state":false,"error":"请先登录","errno":990001}));
        }
        if headers.get("cookie").is_none() {
            return j(json!({"state":false,"error":"请先登录","errno":990001}));
        }
        return j(json!({"state":true,"data":[{"fid":"1","cid":"0","n":"x"}]}));
    }
    if path == "/share/snap" {
        c.push("SNAP");
        return j(json!({"state":true,"data":{"count":1,"list":[{"fid":"111","cid":"0","n":"NB.mkv","s":"1"}]}}));
    }
    if path == "/share/receive" {
        c.push("RECEIVE");
        return j(json!({"state":true}));
    }
    Response::builder().status(404).body(Body::empty()).unwrap()
}

async fn harness(cookie_ok: bool) -> (Temp, Server, Arc<Engine>, Arc<Calls>) {
    let dir = Temp(std::env::temp_dir().join(format!("re0drive-feat-{}", uuid::Uuid::new_v4())));
    std::fs::create_dir_all(&dir.0).unwrap();
    let calls = Arc::new(Calls {
        cookie_ok: Mutex::new(cookie_ok),
        ..Default::default()
    });
    let srv = serve(Router::new().fallback(any(fixture)).with_state(calls.clone())).await;
    let store = Store::open(&dir.0).await.unwrap();
    let mut s = store.settings().await.unwrap();
    s.mode = "gateway".into();
    s.re0_base_url = srv.base.clone();
    s.install_id = "fixture".into();
    s.re0_shared_secret = "fixture-key".into();
    s.cookie_115 = "UID=test; CID=test; SEID=test".into();
    s.cookie_check_enabled = true;
    s.cookie_check_minutes = 5;
    s.tg_enabled = true;
    s.tg_bot_token = "123:ABC".into();
    s.tg_allowed_chats = "42".into();
    store.save_settings(&s).await.unwrap();
    let mut p = Providers::with_bases(srv.base.clone(), srv.base.clone());
    p.resolve_delay = Duration::from_millis(5);
    let mut engine = Engine::with_providers(store, p);
    engine.tg = Telegram::with_base(srv.base.clone());
    (dir, srv, Arc::new(engine), calls)
}



#[tokio::test]
async fn cookie_keepalive_detects_expiry_and_alerts_once() {
    let (_d, _s, e, c) = harness(false).await; // cookie rejected
    e.cookie_keepalive().await.unwrap();
    let s = e.store.settings().await.unwrap();
    assert_eq!(s.cookie_status, "expired");
    assert!(s.cookie_alerted);
    let sent = c.tg_sent.lock().unwrap().clone();
    assert_eq!(sent.len(), 1);
    assert!(sent[0]["text"].as_str().unwrap().contains("Cookie 已失效"));
    // Second cycle: no duplicate alert
    {
        let mut s = e.store.settings().await.unwrap();
        s.cookie_checked_at = 0;
        e.store.save_settings(&s).await.unwrap();
    }
    e.cookie_keepalive().await.unwrap();
    assert_eq!(c.tg_sent.lock().unwrap().len(), 1);
    // Recovery
    *c.cookie_ok.lock().unwrap() = true;
    {
        let mut s = e.store.settings().await.unwrap();
        s.cookie_checked_at = 0;
        e.store.save_settings(&s).await.unwrap();
    }
    e.cookie_keepalive().await.unwrap();
    let s = e.store.settings().await.unwrap();
    assert_eq!(s.cookie_status, "valid");
    assert!(!s.cookie_alerted);
    assert!(matches!(e.probe_cookie_115().await, CookieProbe::Valid));
}

#[tokio::test]
async fn telegram_allowlist_search_and_transfer() {
    let (_d, _s, e, c) = harness(true).await;
    // Unauthorized user
    c.tg_updates.lock().unwrap().push(json!({
        "update_id": 1,
        "message": {"message_id":1,"chat":{"id":99},"from":{"id":99},"text":"/search 550"}
    }));
    e.tg.poll_once(&e).await.unwrap();
    let sent = c.tg_sent.lock().unwrap().clone();
    assert!(sent.iter().any(|m| m["chat_id"] == 99 && m["text"].as_str().unwrap().contains("未授权")));

    // Authorized search
    c.tg_sent.lock().unwrap().clear();
    c.tg_updates.lock().unwrap().push(json!({
        "update_id": 2,
        "message": {"message_id":2,"chat":{"id":42},"from":{"id":42},"text":"/movie 550"}
    }));
    e.tg.poll_once(&e).await.unwrap();
    let sent = c.tg_sent.lock().unwrap().clone();
    let result_msg = sent
        .iter()
        .find(|m| m["chat_id"] == 42 && m["reply_markup"].is_object())
        .expect("results with keyboard");
    assert!(result_msg["text"].as_str().unwrap().contains("版本"));
    let kb = result_msg["reply_markup"]["inline_keyboard"].as_array().unwrap();
    let xfer = kb
        .iter()
        .flat_map(|row| row.as_array().into_iter().flatten())
        .find(|b| b["callback_data"].as_str().unwrap_or("").starts_with("xfer:"))
        .unwrap()["callback_data"]
        .as_str()
        .unwrap()
        .to_string();

    // Confirm transfer via callback
    c.tg_sent.lock().unwrap().clear();
    c.tg_updates.lock().unwrap().push(json!({
        "update_id": 3,
        "callback_query": {
            "id": "cb1",
            "from": {"id": 42},
            "message": {"chat": {"id": 42}},
            "data": xfer
        }
    }));
    e.tg.poll_once(&e).await.unwrap();
    let job = e.store.pending_job().await.unwrap().expect("job queued");
    assert_eq!(job.resource.origin, "relay");
    e.execute_job(job).await.unwrap();
    assert_eq!(c.unlocks.load(Ordering::SeqCst), 1); // owned relay resource still fetches the share link
    // Job notification went out
    let sent = c.tg_sent.lock().unwrap().clone();
    assert!(sent.iter().any(|m| {
        m["chat_id"] == 42
            && m["text"]
                .as_str()
                .unwrap_or("")
                .contains("success")
    }));
}

#[tokio::test]
async fn secrets_never_echoed_in_public_settings() {
    let (_d, _s, e, _c) = harness(true).await;
    let pub_ = e.store.settings().await.unwrap().public();
    let text = pub_.to_string();
    assert!(!text.contains("123:ABC"));
    assert!(!text.contains("UID=test"));
    assert_eq!(pub_["has_tg_token"], true);
    assert!(!pub_.as_object().unwrap().contains_key("nullbr_enabled"));
    assert!(!pub_.as_object().unwrap().contains_key("has_nullbr_key"));
}

#[tokio::test]
async fn empty_relay_stays_empty() {
    let (_d, _s, e, c) = harness(true).await;
    let settings = e.store.settings().await.unwrap();
    let r = e.providers.search_with(&settings, "", Some("999"), "movie").await.unwrap();
    assert!(r.is_empty());
    assert!(c.all().iter().any(|x| x.contains("RELAY") && x.contains("999")));
    assert!(!c.all().iter().any(|x| x.contains("NULLBR") || x.contains("/movie/999/115")));
}

#[tokio::test]
async fn leftover_nullbr_settings_ignored_on_load() {
    // Simulate an older encrypted doc that still carries nullbr_* keys.
    let dir = Temp(std::env::temp_dir().join(format!("re0drive-legacy-{}", uuid::Uuid::new_v4())));
    std::fs::create_dir_all(&dir.0).unwrap();
    let store = Store::open(&dir.0).await.unwrap();
    let mut s = store.settings().await.unwrap();
    // Round-trip through JSON with extra fields the way an old blob would.
    let mut v = serde_json::to_value(&s).unwrap();
    v.as_object_mut().unwrap().insert("nullbr_enabled".into(), json!(true));
    v.as_object_mut().unwrap().insert("nullbr_api_key".into(), json!("should-be-ignored"));
    v.as_object_mut().unwrap().insert("nullbr_app_id".into(), json!("old"));
    let s2: re0drive::models::Settings = serde_json::from_value(v).unwrap();
    store.save_settings(&s2).await.unwrap();
    let loaded = store.settings().await.unwrap();
    let pub_ = loaded.public();
    assert!(!pub_.as_object().unwrap().contains_key("nullbr_enabled"));
    assert!(!pub_.to_string().contains("should-be-ignored"));
}
