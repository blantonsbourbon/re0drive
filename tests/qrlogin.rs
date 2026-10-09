//! QR login flows against local fixtures (no real 115 / RE0 traffic).
use axum::{
    Form, Json, Router,
    extract::{Path, Query, State},
    routing::{get, post},
};
use re0drive::{engine::Engine, providers::Providers, qrlogin::cookie_header, store::Store};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

#[derive(Default)]
struct Fx {
    polls: Mutex<HashMap<String, usize>>,
    script: Mutex<Vec<i64>>,
    logins: Mutex<Vec<(String, String)>>,
}
async fn token() -> Json<Value> {
    Json(json!({"state":1,"code":0,"data":{"uid":"fxuid1","time":1780000000,"sign":"fxsign","qrcode":"https://115.com/scan/dg-fxuid1"}}))
}
async fn status(State(fx): State<Arc<Fx>>, Query(q): Query<HashMap<String, String>>) -> Json<Value> {
    assert_eq!(q.get("sign").map(String::as_str), Some("fxsign"));
    assert_eq!(q.get("time").map(String::as_str), Some("1780000000"));
    let n = {
        let mut p = fx.polls.lock().unwrap();
        let e = p.entry(q["uid"].clone()).or_default();
        *e += 1;
        *e - 1
    };
    let script = fx.script.lock().unwrap();
    let st = script.get(n).copied().unwrap_or(*script.last().unwrap());
    Json(json!({"state":1,"code":0,"data":{"status":st,"msg":""}}))
}
async fn login(State(fx): State<Arc<Fx>>, Path(app): Path<String>, Form(f): Form<HashMap<String, String>>) -> Json<Value> {
    fx.logins.lock().unwrap().push((app, f["account"].clone()));
    Json(json!({"state":1,"data":{"user_id":42,"user_name":"fixture-user","cookie":{"UID":"42_A1_1","CID":"cidv","SEID":"seidv","KID":"kidv"}}}))
}

async fn setup(script: Vec<i64>) -> (Arc<Engine>, Arc<Fx>, tokio::task::JoinHandle<()>, std::path::PathBuf) {
    let fx = Arc::new(Fx { script: Mutex::new(script), ..Default::default() });
    let app = Router::new()
        .route("/api/1.0/web/1.0/token/", get(token))
        .route("/get/status/", get(status))
        .route("/app/1.0/{app}/1.0/login/qrcode/", post(login))
        .with_state(fx.clone());
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", l.local_addr().unwrap());
    let task = tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    let dir = std::env::temp_dir().join(format!("re0drive-qr-{}", uuid::Uuid::new_v4()));
    let store = Store::open(&dir).await.unwrap();
    let providers = Providers::with_bases(base.clone(), base.clone()).with_qr_base(base);
    (Arc::new(Engine::with_providers(store, providers)), fx, task, dir)
}

#[tokio::test]
async fn qr115_waiting_scanned_confirmed_saves_encrypted_cookie() {
    let (engine, fx, task, dir) = setup(vec![0, 1, 2]).await;
    let start = engine.qr115_start("alipaymini").await.unwrap();
    assert_eq!(start["uid"], "fxuid1");
    let qr = start["qr"].as_str().unwrap();
    assert!(qr.starts_with("data:image/svg+xml;base64,"));
    assert_eq!(engine.qr115_status("fxuid1").await.unwrap()["status"], "waiting");
    assert_eq!(engine.qr115_status("fxuid1").await.unwrap()["status"], "scanned");
    let done = engine.qr115_status("fxuid1").await.unwrap();
    assert_eq!(done["status"], "confirmed", "{done}");
    assert!(!done.to_string().contains("seidv"), "cookie must not be returned");
    assert_eq!(fx.logins.lock().unwrap().as_slice(), &[("alipaymini".to_string(), "fxuid1".to_string())]);
    let s = engine.store.settings().await.unwrap();
    assert_eq!(s.cookie_115, "UID=42_A1_1; CID=cidv; SEID=seidv; KID=kidv");
    assert_eq!(s.public()["has_115_cookie"], true);
    // A repeated poll after success must not log in twice.
    assert_eq!(engine.qr115_status("fxuid1").await.unwrap()["status"], "expired");
    assert_eq!(fx.logins.lock().unwrap().len(), 1);
    for f in std::fs::read_dir(&dir).unwrap() {
        let bytes = std::fs::read(f.unwrap().path()).unwrap();
        assert!(!bytes.windows(5).any(|w| w == b"seidv"), "cookie stored in plaintext");
    }
    task.abort();
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn qr115_expired_and_canceled_do_not_login() {
    for (code, want) in [(-1, "expired"), (-2, "canceled")] {
        let (engine, fx, task, dir) = setup(vec![code]).await;
        engine.qr115_start("web").await.unwrap();
        assert_eq!(engine.qr115_status("fxuid1").await.unwrap()["status"], want);
        assert!(fx.logins.lock().unwrap().is_empty());
        assert!(engine.store.settings().await.unwrap().cookie_115.is_empty());
        task.abort();
        let _ = std::fs::remove_dir_all(dir);
    }
}

#[tokio::test]
async fn qr115_rejects_unknown_app_and_unknown_uid() {
    let (engine, _fx, task, dir) = setup(vec![0]).await;
    assert!(engine.qr115_start("evil/../x").await.is_err());
    assert_eq!(engine.qr115_status("nope").await.unwrap()["status"], "expired");
    task.abort();
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn re0_qr_requires_gateway_mode_and_renders_signed_link() {
    let (engine, _fx, task, dir) = setup(vec![0]).await;
    assert!(engine.re0_qr_start().await.is_err(), "demo mode must refuse");
    let mut s = engine.store.settings().await.unwrap();
    s.mode = "gateway".into();
    s.re0_shared_secret = "fixture-key".into();
    s.re0_base_url = "http://127.0.0.1:9".into();
    engine.store.save_settings(&s).await.unwrap();
    let r = engine.re0_qr_start().await.unwrap();
    assert!(r["url"].as_str().unwrap().contains("/auth/start?install_id="));
    assert!(r["url"].as_str().unwrap().contains("&sig="));
    assert!(r["qr"].as_str().unwrap().starts_with("data:image/svg+xml;base64,"));
    task.abort();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn cookie_header_orders_and_sanitizes() {
    assert_eq!(cookie_header(&json!({"KID":"k","UID":"u","SEID":"s","CID":"c"})).unwrap(), "UID=u; CID=c; SEID=s; KID=k");
    assert_eq!(cookie_header(&json!({"UID":"u","bad":"a;b","X":"y"})).unwrap(), "UID=u; X=y");
    assert!(cookie_header(&json!({"CID":"c"})).is_none());
}
