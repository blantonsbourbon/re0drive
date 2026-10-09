use axum::{
    Router,
    body::{Body, to_bytes},
    extract::{Request, State},
    http::{Method, StatusCode},
    response::Response,
    routing::any,
};
use re0drive::{
    engine::Engine,
    models::{Resource, Subscription},
    providers::{Providers, canonical, sign},
    store::Store,
    web,
};
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};
use url::Url;

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!("re0drive-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&p).unwrap();
        Self(p)
    }
}
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
struct Fixture {
    cost: Option<i64>,
    bad_unlock: bool,
    bad_receive: bool,
    unlocks: AtomicUsize,
    receives: AtomicUsize,
    snaps: AtomicUsize,
    checkins: AtomicUsize,
}
impl Fixture {
    fn new(cost: Option<i64>, bad_unlock: bool, bad_receive: bool) -> Arc<Self> {
        Arc::new(Self {
            cost,
            bad_unlock,
            bad_receive,
            unlocks: AtomicUsize::new(0),
            receives: AtomicUsize::new(0),
            snaps: AtomicUsize::new(0),
            checkins: AtomicUsize::new(0),
        })
    }
}
fn json_response(v: Value) -> Response {
    Response::builder()
        .header("content-type", "application/json")
        .body(Body::from(v.to_string()))
        .unwrap()
}
async fn fixture(State(f): State<Arc<Fixture>>, req: Request) -> Response {
    let method = req.method().clone();
    let uri = req.uri().clone();
    let headers = req.headers().clone();
    let body = to_bytes(req.into_body(), 32 * 1024).await.unwrap();
    if uri.path().starts_with("/api/") {
        let head = |k| headers.get(k).unwrap().to_str().unwrap();
        let url = Url::parse(&format!("http://fixture{uri}")).unwrap();
        assert_eq!(head("x-install-id"), "fixture-install");
        assert_eq!(
            head("x-signature"),
            sign(
                "fixture-key",
                &canonical(
                    method.as_str(),
                    &url,
                    head("x-install-id"),
                    head("x-timestamp"),
                    head("x-nonce"),
                    &body
                )
            )
        );
        assert!(
            headers.get("cookie").is_none(),
            "115 cookie must not be sent to gateway"
        );
    }
    match (method, uri.path()) {
        (Method::GET, "/api/token/status") => {
            json_response(json!({"success":true,"data":{"has_access_token":true}}))
        }
        (Method::GET, "/api/me") => json_response(
            json!({"success":true,"data":{"username":"fixture","points":9,"access_token":"must-not-return","refresh_token":"must-not-return"}}),
        ),
        (Method::GET, "/api/resources/movie/123") => json_response(
            json!({"success":true,"data":{"115":[{"slug":"fixture-share","title":"Test 2160p HDR","unlock_points":2,"quality":"2160p"}],"quark":[{"slug":"q","title":"Other"}]}}),
        ),
        (Method::GET, "/api/shares/fixture-share") => json_response(
            json!({"success":true,"data":{"actual_unlock_points":f.cost,"unlock_points":f.cost,"is_unlocked":false}}),
        ),
        (Method::POST, "/api/resources/unlock") => {
            f.unlocks.fetch_add(1, Ordering::SeqCst);
            assert_eq!(
                serde_json::from_slice::<Value>(&body).unwrap(),
                json!({"slug":"fixture-share"})
            );
            if f.bad_unlock {
                return Response::new(Body::from("operation accepted, truncated response"));
            }
            json_response(
                json!({"success":true,"data":{"full_url":"https://115.com/s/fixture-share","access_code":"a1b2"}}),
            )
        }
        (Method::POST, "/api/checkin") => {
            f.checkins.fetch_add(1, Ordering::SeqCst);
            assert_eq!(&body[..], br#"{"is_gambler":false}"#);
            json_response(json!({"success":true,"data":{"points":3}}))
        }
        (Method::GET, "/share/snap") => {
            f.snaps.fetch_add(1, Ordering::SeqCst);
            assert!(headers.get("cookie").is_none());
            let q = Url::parse(&format!("http://fixture{uri}"))
                .unwrap()
                .query_pairs()
                .into_owned()
                .collect::<std::collections::HashMap<_, _>>();
            assert_eq!(q["share_code"], "fixture-share");
            assert_eq!(q["receive_code"], "a1b2");
            assert_eq!(q["cid"], "0");
            assert_eq!(q["limit"], "100");
            let offset = q["offset"].parse::<usize>().unwrap();
            let items = (offset + 1..=(offset + 100).min(102))
                .map(|id| {
                    if id == 100 {
                        json!({"fid":0,"cid":id.to_string(),"n":"directory"})
                    } else {
                        json!({"fid":id.to_string()})
                    }
                })
                .collect::<Vec<_>>();
            json_response(json!({"state":true,"data":{"list":items,"count":102}}))
        }
        (Method::POST, "/share/receive") => {
            f.receives.fetch_add(1, Ordering::SeqCst);
            assert_eq!(
                headers.get("cookie").unwrap(),
                "UID=test; CID=test; SEID=test"
            );
            let form = url::form_urlencoded::parse(&body)
                .into_owned()
                .collect::<std::collections::HashMap<_, _>>();
            assert_eq!(form["share_code"], "fixture-share");
            assert_eq!(form["receive_code"], "a1b2");
            assert_eq!(form["cid"], "42");
            let mut ids = form["file_id"]
                .split(',')
                .map(|s| s.parse::<usize>().unwrap())
                .collect::<Vec<_>>();
            ids.sort();
            assert_eq!(ids, (1..=102).collect::<Vec<_>>());
            if f.bad_receive {
                Response::builder()
                    .status(StatusCode::BAD_GATEWAY)
                    .body(Body::from("upstream result unknown"))
                    .unwrap()
            } else {
                json_response(json!({"state":true}))
            }
        }
        _ => Response::builder().status(404).body(Body::empty()).unwrap(),
    }
}
async fn harness(f: Arc<Fixture>) -> (Temp, Server, Engine) {
    let dir = Temp::new();
    let srv = serve(Router::new().fallback(any(fixture)).with_state(f)).await;
    let store = Store::open(&dir.0).await.unwrap();
    let mut s = store.settings().await.unwrap();
    s.mode = "gateway".into();
    s.re0_base_url = srv.base.clone();
    s.install_id = "fixture-install".into();
    s.re0_shared_secret = "fixture-key".into();
    s.cookie_115 = "UID=test; CID=test; SEID=test".into();
    store.save_settings(&s).await.unwrap();
    let engine = Engine::with_providers(store, Providers::with_share_base(srv.base.clone()));
    (dir, srv, engine)
}
fn resource() -> Resource {
    Resource {
        slug: "fixture-share".into(),
        title: "Fixture 2160p HDR".into(),
        provider: "115".into(),
        source_url: String::new(),
        unlock_points: Some(1),
        is_unlocked: false,
        quality: "2160p".into(),
        size: String::new(),
        tmdb_id: "123".into(),
        media_type: "movie".into(),
        ..Default::default()
    }
}
#[tokio::test]
async fn signed_search_unlock_and_paginated_115_receive() {
    let f = Fixture::new(Some(3), false, false);
    let (_dir, _srv, e) = harness(f.clone()).await;
    let s = e.store.settings().await.unwrap();
    let result = e.providers.search(&s, "tmdb:123", "movie").await.unwrap();
    assert_eq!(result.len(), 2, "non-115 results are kept");
    assert_eq!(result[0].tmdb_id, "123");
    assert!(result[0].selectable && !result[1].selectable);
    let status = e.providers.status(&s).await.unwrap();
    assert!(status["authorized"].as_bool().unwrap());
    assert!(status["user"].get("access_token").is_none());
    let j = e
        .enqueue(resource(), "42".into(), 4, None)
        .await
        .unwrap()
        .unwrap();
    let id = j.id.clone();
    assert!(
        e.enqueue(resource(), "42".into(), 4, None)
            .await
            .unwrap()
            .is_none()
    );
    e.execute_job(j).await.unwrap();
    assert_eq!(e.store.job(id).await.unwrap().unwrap().status, "success");
    assert_eq!(f.unlocks.load(Ordering::SeqCst), 1);
    assert_eq!(f.receives.load(Ordering::SeqCst), 1);
    assert_eq!(f.snaps.load(Ordering::SeqCst), 2);
    assert_eq!(e.store.job_stats().await.unwrap()["success"], 1);
    e.checkin().await.unwrap();
    assert_eq!(f.checkins.load(Ordering::SeqCst), 1);
    assert!(
        !e.store
            .settings()
            .await
            .unwrap()
            .last_checkin_day
            .is_empty()
    );
}
#[tokio::test]
async fn fresh_cost_over_cap_or_unknown_never_unlocks() {
    for cost in [Some(8), None, Some(-1)] {
        let f = Fixture::new(cost, false, false);
        let (_dir, _srv, e) = harness(f.clone()).await;
        let j = e
            .enqueue(resource(), "42".into(), 4, None)
            .await
            .unwrap()
            .unwrap();
        let id = j.id.clone();
        e.execute_job(j).await.unwrap();
        assert_eq!(e.store.job(id).await.unwrap().unwrap().status, "skipped");
        assert_eq!(f.unlocks.load(Ordering::SeqCst), 0);
        assert_eq!(f.receives.load(Ordering::SeqCst), 0);
    }
}
#[tokio::test]
async fn uncertain_effects_are_held_for_review() {
    for (bad_unlock, bad_receive) in [(true, false), (false, true)] {
        let f = Fixture::new(Some(2), bad_unlock, bad_receive);
        let (_dir, _srv, e) = harness(f.clone()).await;
        let j = e
            .enqueue(resource(), "42".into(), 4, None)
            .await
            .unwrap()
            .unwrap();
        let id = j.id.clone();
        e.execute_job(j).await.unwrap();
        assert_eq!(
            e.store.job(id).await.unwrap().unwrap().status,
            "needs_review"
        );
        assert!(e.store.pending_job().await.unwrap().is_none());
        assert_eq!(f.unlocks.load(Ordering::SeqCst), 1);
        assert_eq!(f.receives.load(Ordering::SeqCst), usize::from(bad_receive));
    }
}
#[tokio::test]
async fn credentials_or_subscription_changes_stop_queued_jobs() {
    let f = Fixture::new(Some(2), false, false);
    let (_dir, _srv, e) = harness(f.clone()).await;
    let j = e
        .enqueue(resource(), "42".into(), 4, None)
        .await
        .unwrap()
        .unwrap();
    let id = j.id.clone();
    let mut s = e.store.settings().await.unwrap();
    s.install_id = "another-install".into();
    e.store.save_settings(&s).await.unwrap();
    e.execute_job(j).await.unwrap();
    assert_eq!(
        e.store.job(id).await.unwrap().unwrap().status,
        "needs_review"
    );
    let rule = Subscription {
        id: "deleted".into(),
        name: "old".into(),
        query: "tmdb:123".into(),
        ..Default::default()
    };
    e.store.save_subscription(&rule).await.unwrap();
    let j = e
        .enqueue(resource(), "43".into(), 4, Some(rule.id.clone()))
        .await
        .unwrap()
        .unwrap();
    let id = j.id.clone();
    e.store.delete_subscription(rule.id).await.unwrap();
    e.execute_job(j).await.unwrap();
    assert_eq!(e.store.job(id).await.unwrap().unwrap().status, "skipped");
    assert_eq!(f.unlocks.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn restart_recovers_jobs_beyond_the_display_limit_and_keeps_config_encrypted() {
    let dir = Temp::new();
    let store = Store::open(&dir.0).await.unwrap();
    let e = Engine::new(store.clone());
    let mut s = store.settings().await.unwrap();
    s.cookie_115 = "DO-NOT-STORE-IN-PLAINTEXT".into();
    store.save_settings(&s).await.unwrap();
    let mut early = e
        .enqueue(resource(), "42".into(), 4, None)
        .await
        .unwrap()
        .unwrap();
    early.status = "unlocking".into();
    store.save_job(&early).await.unwrap();
    for n in 0..205 {
        let mut r = resource();
        r.slug = format!("resource-{n}");
        e.enqueue(r, "42".into(), 4, None).await.unwrap();
    }
    assert_eq!(store.jobs().await.unwrap().len(), 200);
    store.recover_jobs().await.unwrap();
    assert_eq!(
        store.job(early.id).await.unwrap().unwrap().status,
        "needs_review"
    );
    drop(e);
    drop(store);
    let reopened = Store::open(&dir.0).await.unwrap();
    assert_eq!(
        reopened.settings().await.unwrap().cookie_115,
        "DO-NOT-STORE-IN-PLAINTEXT"
    );
    assert_eq!(reopened.job_stats().await.unwrap()["pending"], 205);
    let marker = b"DO-NOT-STORE-IN-PLAINTEXT";
    for path in [dir.0.join("re0drive.db"), dir.0.join("re0drive.db-wal")] {
        if let Ok(bytes) = std::fs::read(path) {
            assert!(!bytes.windows(marker.len()).any(|w| w == marker));
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(dir.0.join("master.key"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
}
#[tokio::test]
async fn admin_session_csrf_and_write_only_settings() {
    let dir = Temp::new();
    let e = Arc::new(Engine::new(Store::open(&dir.0).await.unwrap()));
    let srv = serve(web::router(e)).await;
    let http = reqwest::Client::new();
    assert_eq!(
        http.get(format!("{}/api/settings", srv.base))
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    let setup = format!("{}/api/auth/setup", srv.base);
    assert_eq!(
        http.post(&setup)
            .json(&json!({"username":"admin","password":"fixture-password"}))
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    let response = http
        .post(&setup)
        .header("X-RE0Drive-Request", "1")
        .json(&json!({"username":"admin","password":"fixture-password"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let cookie = response
        .headers()
        .get("set-cookie")
        .unwrap()
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string();
    assert!(response.headers().get("content-security-policy").is_some());
    let v = http
        .get(format!("{}/api/settings", srv.base))
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    assert!(v.get("cookie_115").is_none());
    assert!(v.get("re0_shared_secret").is_none());
    let updated = json!({"mode":"demo","re0_base_url":v["re0_base_url"],"install_id":v["install_id"],"default_target_cid":"42","default_max_points":2,"checkin_enabled":false,"cookie_115":"fixture-private-cookie"});
    let response = http
        .put(format!("{}/api/settings", srv.base))
        .header("cookie", &cookie)
        .header("X-RE0Drive-Request", "1")
        .json(&updated)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let text = response.text().await.unwrap();
    assert!(!text.contains("fixture-private-cookie"));
    let response = http
        .post(format!("{}/api/auth/logout", srv.base))
        .header("cookie", &cookie)
        .header("X-RE0Drive-Request", "1")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(
        http.get(format!("{}/api/settings", srv.base))
            .header("cookie", cookie)
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
}

#[tokio::test]
async fn symbolic_gateway_status_is_valid_and_bootstrap_is_consumed() {
    let dir = Temp::new();
    let srv = serve(Router::new().route(
        "/api/token/status",
        axum::routing::get(|| async {
            axum::Json(
                json!({"success":true,"code":"REAUTH_REQUIRED","data":{"has_access_token":false}}),
            )
        }),
    ))
    .await;
    let bootstrap = json!({"re0_shared_secret":"imported-fixture-key","re0_base_url":srv.base,"install_id":"imported-install"});
    std::fs::write(dir.0.join("bootstrap.json"), bootstrap.to_string()).unwrap();
    let store = Store::open(&dir.0).await.unwrap();
    assert!(!dir.0.join("bootstrap.json").exists());
    let settings = store.settings().await.unwrap();
    assert_eq!(settings.mode, "gateway");
    assert_eq!(settings.install_id, "imported-install");
    assert_eq!(settings.re0_shared_secret, "imported-fixture-key");
    let status = Providers::new().status(&settings).await.unwrap();
    assert_eq!(status["authorized"], false);
    assert_eq!(status["demo"], false);
}

#[tokio::test]
async fn skipped_resources_can_be_requeued_without_duplicating_success() {
    let dir = Temp::new();
    let e = Engine::new(Store::open(&dir.0).await.unwrap());
    let mut job = e
        .enqueue(resource(), "42".into(), 4, None)
        .await
        .unwrap()
        .unwrap();
    job.status = "skipped".into();
    e.store.save_job(&job).await.unwrap();
    let new = e
        .enqueue(resource(), "42".into(), 4, None)
        .await
        .unwrap()
        .unwrap();
    assert_ne!(job.id, new.id);
    assert_eq!(e.store.jobs().await.unwrap().len(), 1);
    e.execute_job(new).await.unwrap();
    assert!(
        e.enqueue(resource(), "42".into(), 4, None)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn empty_115_confirmation_is_not_reported_as_success() {
    let srv = serve(
        Router::new()
            .route(
                "/share/snap",
                axum::routing::get(|| async {
                    axum::Json(json!({"state":true,"data":{"list":[{"fid":"1"}],"count":1}}))
                }),
            )
            .route(
                "/share/receive",
                axum::routing::post(|| async { axum::Json(json!({})) }),
            ),
    )
    .await;
    let settings = re0drive::models::Settings {
        mode: "gateway".into(),
        cookie_115: "fixture-cookie".into(),
        ..Default::default()
    };
    let error = Providers::with_share_base(srv.base.clone())
        .receive_115(&settings, "https://115.com/s/abc?password=fixture", "42")
        .await
        .unwrap_err();
    assert!(error.message.contains("成功标识"));
}
