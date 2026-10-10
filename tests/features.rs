//! NullBR fallback, 115 Cookie keep-alive, Telegram bot — all against in-process mocks.
//! Nothing reaches api.nullbr.eu.org, api.telegram.org, or real 115.
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

    // --- NullBR ---
    if path.starts_with("/movie/") || path.starts_with("/tv/") {
        assert_eq!(
            headers.get("x-app-id").and_then(|v| v.to_str().ok()),
            Some("test-app")
        );
        assert_eq!(
            headers.get("x-api-key").and_then(|v| v.to_str().ok()),
            Some("test-key")
        );
        assert!(
            headers
                .get("user-agent")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("")
                .starts_with("re0drive/")
        );
        c.push(format!("NULLBR {path}?page={}", q.get("page").cloned().unwrap_or_default()));
        if path.ends_with("/999/115") {
            return j(json!({"id":999,"media_type":"movie","page":1,"total_page":1,"items":[]}));
        }
        if path.ends_with("/550/115") {
            let page: u32 = q.get("page").and_then(|p| p.parse().ok()).unwrap_or(1);
            if page == 1 {
                return j(json!({
                    "id":550,"media_type":"movie","page":1,"total_page":2,
                    "items":[
                        {"title":"NB 2160p","size":"40 GB","share_link":"https://115.com/s/swnullbr1?password=ab12","resolution":"2160P","quality":["REMUX"]},
                        {"title":"NB quark","size":"1 GB","share_link":"https://pan.quark.cn/s/abc","resolution":"1080P"}
                    ]
                }));
            }
            return j(json!({
                "id":550,"media_type":"movie","page":2,"total_page":2,
                "items":[
                    {"title":"NB page2","size":"9 GB","share_link":"https://115.com/s/swnullbr2?password=cd34","resolution":"1080P","quality":"WEB-DL"}
                ]
            }));
        }
        return j(json!({"id":0,"page":1,"total_page":1,"items":[]}));
    }

    // --- RE0 relay (HMAC not required here — we only care about empty fallback) ---
    if path.starts_with("/api/resources/") && method == Method::GET {
        c.push(format!("RELAY {path}"));
        // Always empty so NullBR kicks in.
        return j(json!({"success":true,"data":[],"total":0}));
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
    s.nullbr_enabled = true;
    s.nullbr_base_url = srv.base.clone();
    s.nullbr_app_id = "test-app".into();
    s.nullbr_api_key = "test-key".into();
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
async fn nullbr_fallback_when_relay_empty() {
    let (_d, _s, e, c) = harness(true).await;
    let s = e.store.settings().await.unwrap();
    let r = e
        .providers
        .search_with(&s, "", Some("550"), "movie")
        .await
        .unwrap();
    assert!(c.all().iter().any(|x| x.starts_with("RELAY /api/resources/movie/550")));
    assert!(c.all().iter().any(|x| x.contains("NULLBR /movie/550/115?page=1")));
    assert!(c.all().iter().any(|x| x.contains("NULLBR /movie/550/115?page=2")), "paginates");
    assert_eq!(r.len(), 3);
    assert!(r.iter().filter(|x| x.selectable).count() == 2);
    assert!(r.iter().any(|x| !x.selectable && x.source_url.contains("quark")));
    assert!(r.iter().all(|x| x.origin == "nullbr"));
    // Both empty → explicit empty list
    let empty = e
        .providers
        .search_with(&s, "", Some("999"), "movie")
        .await
        .unwrap();
    assert!(empty.is_empty());
}

#[tokio::test]
async fn nullbr_transfer_skips_unlock() {
    let (_d, _s, e, c) = harness(true).await;
    let s = e.store.settings().await.unwrap();
    let r = e
        .providers
        .search_with(&s, "", Some("550"), "movie")
        .await
        .unwrap()
        .into_iter()
        .find(|x| x.selectable)
        .unwrap();
    let job = e
        .enqueue_opt(r, Some("0".into()), Some(0), None)
        .await
        .unwrap()
        .unwrap();
    e.execute_job(job).await.unwrap();
    assert_eq!(c.unlocks.load(Ordering::SeqCst), 0, "must not call RE0 unlock");
    assert!(c.all().iter().any(|x| x == "RECEIVE"));
    let jobs = e.store.jobs().await.unwrap();
    assert_eq!(jobs[0].status, "success");
    assert!(!jobs[0].received.is_empty());
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
    assert!(result_msg["text"].as_str().unwrap().contains("NullBR") || result_msg["text"].as_str().unwrap().contains("版本"));
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
    assert_eq!(job.resource.origin, "nullbr");
    e.execute_job(job).await.unwrap();
    assert_eq!(c.unlocks.load(Ordering::SeqCst), 0);
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
    assert!(!text.contains("test-key"));
    assert!(!text.contains("123:ABC"));
    assert!(!text.contains("UID=test"));
    assert_eq!(pub_["has_nullbr_key"], true);
    assert_eq!(pub_["has_tg_token"], true);
    assert_eq!(pub_["nullbr_app_id"], "test-app"); // app id is not secret
}
