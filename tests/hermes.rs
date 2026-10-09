//! Acceptance tests for the Hermes handoff (feat/hermes-handoff). Everything runs against an
//! in-process fixture; no request reaches re0.tgtodrive.top, 115 or TMDB.
use axum::{
    Router,
    body::{Body, to_bytes},
    extract::{Request, State},
    http::Method,
    response::Response,
    routing::any,
};
use re0drive::{
    engine::Engine,
    models::Subscription,
    providers::{Providers, canonical, normalize_resources, sign},
    store::Store,
    web,
};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};
use url::Url;

const MOVIE: &str = "0123456789abcdef0123456789abcdef";
const QUARK: &str = "fedcba9876543210fedcba9876543210";
const ED2K: &str = "aaaaaaaaaaaaaaaabbbbbbbbbbbbbbbb";
const SHARE_FID: &str = "111"; // share-side fid of the single file
const SHARE_DIR: &str = "222"; // share-side cid of the folder
const LOCAL_FID: &str = "9001"; // fid after landing in the target folder
const LOCAL_DIR: &str = "9002";

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
        task: tokio::spawn(async move { axum::serve(listener, router).await.unwrap() }),
    }
}
#[derive(Default)]
struct Calls(Mutex<Vec<String>>);
impl Calls {
    fn all(&self) -> Vec<String> {
        self.0.lock().unwrap().clone()
    }
    fn count(&self, prefix: &str) -> usize {
        self.all().iter().filter(|c| c.starts_with(prefix)).count()
    }
}
fn j(v: Value) -> Response {
    Response::builder()
        .header("content-type", "application/json")
        .body(Body::from(v.to_string()))
        .unwrap()
}
fn relay_movie() -> Value {
    json!({"success":true,"data":[
        {"slug":MOVIE,"title":"Fixture Movie","pan_type":"115","share_size":"58.3 GB",
         "video_resolution":["2160P","1080P"],"source":["REMUX","Blu-ray"],
         "subtitle_language":["简中","繁中","英语"],"subtitle_type":["内封","特效"],
         "unlock_points":2,"is_unlocked":false,"remark":"杜比视界 + HDR10 双版本"},
        {"slug":QUARK,"title":"Fixture Movie quark","pan_type":"quark","share_size":"20 GB",
         "video_resolution":["1080P"],"source":["WEB-DL"],"subtitle_language":["简中"],
         "subtitle_type":["外挂"],"unlock_points":0,"remark":"夸克"},
        {"slug":ED2K,"title":"Fixture Movie ed2k","pan_type":"ed2k","share_size":"9 GB",
         "video_resolution":["720P"],"source":["HDTV"],"subtitle_language":[],
         "subtitle_type":[],"unlock_points":1,"remark":"电驴"}
    ],"total":3})
}
async fn fixture(State(calls): State<Arc<Calls>>, req: Request) -> Response {
    let method = req.method().clone();
    let uri = req.uri().clone();
    let headers = req.headers().clone();
    let body = to_bytes(req.into_body(), 64 * 1024).await.unwrap();
    let url = Url::parse(&format!("http://fixture{uri}")).unwrap();
    let q: HashMap<String, String> = url.query_pairs().into_owned().collect();
    let form: HashMap<String, String> = url::form_urlencoded::parse(&body).into_owned().collect();
    if uri.path().starts_with("/api/") {
        let head = |k| headers.get(k).unwrap().to_str().unwrap();
        assert_eq!(
            head("x-signature"),
            sign("fixture-key", &canonical(method.as_str(), &url, head("x-install-id"), head("x-timestamp"), head("x-nonce"), &body))
        );
        assert!(headers.get("cookie").is_none());
    }
    let tag = match uri.path() {
        "/share/receive" => format!("POST /share/receive cid={}", form.get("cid").cloned().unwrap_or_default()),
        "/files" => format!("GET /files cid={}", q.get("cid").cloned().unwrap_or_default()),
        p => format!("{method} {p}"),
    };
    calls.0.lock().unwrap().push(tag);
    match (method, uri.path()) {
        (Method::GET, "/3/search/movie") => j(json!({"results":[{"id":550},{"id":551},{"id":552},{"id":553},{"id":554}]})),
        (Method::GET, "/api/resources/movie/550") => j(relay_movie()),
        (Method::GET, p) if p.starts_with("/api/resources/movie/") => j(json!({"success":true,"data":[],"total":0})),
        (Method::GET, p) if p.starts_with("/api/shares/") => j(json!({"success":true,"data":{"unlock_points":2,"is_unlocked":false,"pan_type":"115"}})),
        (Method::POST, "/api/resources/unlock") => j(json!({"success":true,"data":{"full_url":"https://115.com/s/swfixture","access_code":"ab12"}})),
        (Method::GET, "/share/snap") => j(json!({"state":true,"data":{"count":2,"list":[
            {"fid":SHARE_FID,"cid":"0","n":"Fixture.Movie.2160p.mkv","s":"62599999999"},
            {"cid":SHARE_DIR,"n":"Extras"}
        ]}})),
        (Method::POST, "/share/receive") => j(json!({"state":true})),
        (Method::GET, "/files") => {
            let cid = q["cid"].clone();
            j(json!({"state":true,"data":[
                {"fid":LOCAL_FID,"cid":cid,"n":"Fixture.Movie.2160p.mkv","s":62599999999u64},
                {"cid":LOCAL_DIR,"pid":cid,"n":"Extras"},
                {"fid":"9003","cid":cid,"n":"unrelated.mkv","s":1}
            ]}))
        }
        _ => Response::builder().status(404).body(Body::empty()).unwrap(),
    }
}
async fn harness() -> (Temp, Server, Arc<Engine>, Arc<Calls>) {
    let dir = Temp(std::env::temp_dir().join(format!("re0drive-hermes-{}", uuid::Uuid::new_v4())));
    std::fs::create_dir_all(&dir.0).unwrap();
    let calls = Arc::new(Calls::default());
    let srv = serve(Router::new().fallback(any(fixture)).with_state(calls.clone())).await;
    let store = Store::open(&dir.0).await.unwrap();
    let mut s = store.settings().await.unwrap();
    s.mode = "gateway".into();
    s.re0_base_url = srv.base.clone();
    s.install_id = "fixture-install".into();
    s.re0_shared_secret = "fixture-key".into();
    s.tmdb_api_key = "fixture-tmdb".into();
    s.cookie_115 = "UID=test; CID=test; SEID=test".into();
    store.save_settings(&s).await.unwrap();
    let mut p = Providers::with_bases(srv.base.clone(), srv.base.clone());
    p.resolve_delay = Duration::from_millis(10);
    (dir, srv, Arc::new(Engine::with_providers(store, p)), calls)
}
fn client() -> reqwest::Client {
    reqwest::Client::builder().no_proxy().build().unwrap()
}
/// Starts the web app and returns (base, session cookie).
async fn app(engine: Arc<Engine>) -> (Server, String) {
    let srv = serve(web::router(engine)).await;
    let r = client()
        .post(format!("{}/api/auth/setup", srv.base))
        .header("x-re0drive-request", "1")
        .json(&json!({"username":"tester","password":"fixture-password-123"}))
        .send()
        .await
        .unwrap();
    assert!(r.status().is_success());
    let cookie = r.headers()["set-cookie"].to_str().unwrap().split(';').next().unwrap().to_string();
    (srv, cookie)
}

// Acceptance 1: array fields kept; non-115 kept with selectable=false.
#[test]
fn acceptance1_array_fields_and_non_115_kept() {
    let r = normalize_resources(&relay_movie());
    assert_eq!(r.len(), 3, "nothing dropped");
    let m = r.iter().find(|r| r.slug == MOVIE).unwrap();
    assert_eq!(m.slug.len(), 32);
    assert_eq!(m.pan_type, "115");
    assert!(m.selectable);
    assert_eq!(m.share_size, json!("58.3 GB"));
    assert_eq!(m.video_resolution, json!(["2160P", "1080P"]));
    assert_eq!(m.source, json!(["REMUX", "Blu-ray"]));
    assert_eq!(m.subtitle_language, json!(["简中", "繁中", "英语"]));
    assert_eq!(m.subtitle_type, json!(["内封", "特效"]));
    assert_eq!(m.remark, "杜比视界 + HDR10 双版本");
    assert_eq!((m.unlock_points, m.is_unlocked), (Some(2), false));
    for (slug, pan) in [(QUARK, "quark"), (ED2K, "ed2k")] {
        let x = r.iter().find(|r| r.slug == slug).unwrap();
        assert_eq!(x.pan_type, pan);
        assert!(!x.selectable, "{pan} must not be selectable");
        assert!(x.video_resolution.is_array() && x.source.is_array());
        assert!(!x.remark.is_empty());
    }
    // serialized field names Hermes relies on
    let v = serde_json::to_value(m).unwrap();
    for k in ["slug","title","pan_type","share_size","video_resolution","source","subtitle_language","subtitle_type","unlock_points","is_unlocked","remark","selectable"] {
        assert!(v.get(k).is_some(), "missing {k}");
    }
}

// Acceptance 1 + 2 over HTTP with a machine token; numeric id queries only that id.
#[tokio::test]
async fn acceptance2_numeric_tmdb_id_queries_only_that_id() {
    let (_d, _u, e, calls) = harness().await;
    let (srv, cookie) = app(e).await;
    let token = client().post(format!("{}/api/machine-token", srv.base)).header("cookie", &cookie)
        .header("x-re0drive-request", "1").send().await.unwrap().json::<Value>().await.unwrap()["token"]
        .as_str().unwrap().to_string();
    for body in [json!({"query":"550","media_type":"movie"}), json!({"tmdb_id":550,"media_type":"movie"}), json!({"query":"Fixture","tmdb_id":"550","media_type":"movie"})] {
        let before = calls.all().len();
        let v: Value = client().post(format!("{}/api/resources/search", srv.base)).bearer_auth(&token)
            .json(&body).send().await.unwrap().json().await.unwrap();
        let new: Vec<String> = calls.all()[before..].to_vec();
        assert_eq!(new, vec!["GET /api/resources/movie/550".to_string()], "{body}");
        let res = v["resources"].as_array().unwrap();
        assert_eq!(res.len(), 3);
        assert_eq!(res[0]["slug"], MOVIE, "selectable first");
        assert_eq!(res[0]["video_resolution"], json!(["2160P", "1080P"]));
        assert_eq!(res.iter().filter(|r| r["selectable"] == false).count(), 2);
    }
    // Title search merges the top 4 TMDB ids only.
    let before = calls.all().len();
    client().post(format!("{}/api/resources/search", srv.base)).bearer_auth(&token)
        .json(&json!({"query":"Fixture","media_type":"movie"})).send().await.unwrap();
    let mut new: Vec<String> = calls.all()[before..].iter().filter(|c| c.starts_with("GET /api/resources")).cloned().collect();
    new.sort();
    assert_eq!(new, ["550", "551", "552", "553"].map(|i| format!("GET /api/resources/movie/{i}")));
}

// P2: empty relay result -> explicit empty list, never demo data.
#[tokio::test]
async fn empty_relay_result_is_empty_list() {
    let (_d, _u, e, _calls) = harness().await;
    let s = e.store.settings().await.unwrap();
    let r = e.providers.search_with(&s, "", Some("999"), "movie").await.unwrap();
    assert!(r.is_empty());
}

// Acceptance 3: after receive, the job carries the LOCAL fid from the target folder listing.
#[tokio::test]
async fn acceptance3_job_result_contains_local_fid() {
    let (_d, _u, e, calls) = harness().await;
    let (srv, cookie) = app(e.clone()).await;
    let token = client().post(format!("{}/api/machine-token", srv.base)).header("cookie", &cookie)
        .header("x-re0drive-request", "1").send().await.unwrap().json::<Value>().await.unwrap()["token"]
        .as_str().unwrap().to_string();
    let s = e.store.settings().await.unwrap();
    let resource = e.providers.search_with(&s, "", Some("550"), "movie").await.unwrap().into_iter().find(|r| r.slug == MOVIE).unwrap();
    let v: Value = client().post(format!("{}/api/jobs", srv.base)).bearer_auth(&token)
        .json(&json!({"resource":resource,"target_cid":"3482983666605360884"})).send().await.unwrap().json().await.unwrap();
    assert!(v["job"]["account_key"].is_null(), "account fingerprint not exposed");
    assert_eq!(v["job"]["max_points"], 2, "cap defaults to the selected version's price");
    let id = v["job"]["id"].as_str().unwrap().to_string();
    let job = e.store.pending_job().await.unwrap().unwrap();
    e.execute_job(job).await.unwrap();
    let v: Value = client().get(format!("{}/api/jobs/{id}", srv.base)).bearer_auth(&token).send().await.unwrap().json().await.unwrap();
    let job = &v["job"];
    assert_eq!(job["status"], "success", "{job}");
    let rec = job["received"].as_array().unwrap();
    assert_eq!(rec.len(), 2);
    let file = rec.iter().find(|r| r["is_dir"] == false).unwrap();
    assert_eq!(file["share_id"], SHARE_FID);
    assert_eq!(file["local_id"], LOCAL_FID);
    assert_ne!(file["local_id"], file["share_id"]);
    assert_ne!(file["local_id"], "3482983666605360884", "listed cid is the parent, not the file id");
    assert_eq!(file["parent_cid"], "3482983666605360884");
    assert_eq!(file["name"], "Fixture.Movie.2160p.mkv");
    assert_eq!(file["size"], 62599999999i64);
    let dir = rec.iter().find(|r| r["is_dir"] == true).unwrap();
    assert_eq!((dir["share_id"].as_str(), dir["local_id"].as_str()), (Some(SHARE_DIR), Some(LOCAL_DIR)));
    let all = calls.all();
    assert!(all.contains(&"POST /share/receive cid=3482983666605360884".to_string()));
    assert!(all.contains(&"GET /files cid=3482983666605360884".to_string()));
    assert_eq!(calls.count("POST /api/resources/unlock"), 1);
}

// P1: target_cid is optional; empty = root (0).
#[tokio::test]
async fn empty_target_cid_receives_into_root() {
    let (_d, _u, e, calls) = harness().await;
    let s = e.store.settings().await.unwrap();
    let r = e.providers.search_with(&s, "", Some("550"), "movie").await.unwrap().remove(0);
    let job = e.enqueue_opt(r.clone(), Some("".into()), None, None).await.unwrap().unwrap();
    assert_eq!(job.target_cid, "0");
    e.execute_job(job).await.unwrap();
    assert!(calls.all().contains(&"POST /share/receive cid=0".to_string()));
    assert!(calls.all().contains(&"GET /files cid=0".to_string()));
    // non-115 versions are refused before anything is queued
    let q = e.providers.search_with(&s, "", Some("550"), "movie").await.unwrap().into_iter().find(|r| r.slug == QUARK).unwrap();
    assert!(e.enqueue_opt(q, None, None, None).await.is_err());
    let mut forged = r;
    forged.slug = ED2K.into();
    forged.selectable = false;
    assert!(e.enqueue_opt(forged, None, None, None).await.is_err());
}

// Subscriptions only collect candidates — never unlock, even with auto_transfer and 0-point items.
#[tokio::test]
async fn subscriptions_never_unlock() {
    let (_d, _u, e, calls) = harness().await;
    let rule = Subscription {
        id: "r1".into(),
        name: "fixture".into(),
        query: "550".into(),
        media_type: "movie".into(),
        auto_transfer: true,
        enabled: true,
        ..Default::default()
    };
    assert!(Subscription::default().max_points >= 20);
    assert!(!Subscription::default().auto_transfer);
    e.store.save_subscription(&rule).await.unwrap();
    e.scan(rule).await.unwrap();
    let saved = e.store.subscriptions().await.unwrap().remove(0);
    assert_eq!(saved.candidates.len(), 1, "only the selectable 115 version");
    assert_eq!(saved.candidates[0].slug, MOVIE);
    assert!(e.store.pending_job().await.unwrap().is_none());
    assert_eq!(calls.count("POST /api/resources/unlock"), 0);
    assert_eq!(e.store.settings().await.unwrap().default_max_points, 20);
}

// Machine token: hashed, shown once, rotation invalidates, limited scope, no cookie fallback.
#[tokio::test]
async fn machine_token_auth() {
    let (d, _u, e, _calls) = harness().await;
    let (srv, cookie) = app(e).await;
    let c = client();
    let search = |t: &str| c.post(format!("{}/api/resources/search", srv.base)).bearer_auth(t.to_string()).json(&json!({"tmdb_id":"999","media_type":"movie"})).send();
    // no token yet
    assert_eq!(search("r0d_0000000000000000000000000000000000000000000000000000000000000000").await.unwrap().status(), 401);
    // requires session + CSRF header to mint
    assert_eq!(c.post(format!("{}/api/machine-token", srv.base)).header("x-re0drive-request", "1").send().await.unwrap().status(), 401);
    let mint = || c.post(format!("{}/api/machine-token", srv.base)).header("cookie", &cookie).header("x-re0drive-request", "1").send();
    let t1 = mint().await.unwrap().json::<Value>().await.unwrap()["token"].as_str().unwrap().to_string();
    assert!(t1.starts_with("r0d_") && t1.len() == 68);
    assert_eq!(search(&t1).await.unwrap().status(), 200);
    // info never echoes the token
    let info = c.get(format!("{}/api/machine-token", srv.base)).header("cookie", &cookie).send().await.unwrap().text().await.unwrap();
    assert!(!info.contains(&t1) && info.contains(&t1[..10]));
    // stored hashed only
    let db = std::fs::read(d.0.join("re0drive.db")).unwrap_or_default();
    let wal = std::fs::read(d.0.join("re0drive.db-wal")).unwrap_or_default();
    assert!(!String::from_utf8_lossy(&db).contains(&t1) && !String::from_utf8_lossy(&wal).contains(&t1));
    // scope: token cannot read settings or mint tokens
    assert_eq!(c.get(format!("{}/api/settings", srv.base)).bearer_auth(&t1).send().await.unwrap().status(), 401);
    assert_eq!(c.post(format!("{}/api/machine-token", srv.base)).bearer_auth(&t1).header("x-re0drive-request", "1").send().await.unwrap().status(), 401);
    // bad bearer + valid cookie: no fallback
    assert_eq!(c.get(format!("{}/api/jobs", srv.base)).bearer_auth("nope").header("cookie", &cookie).send().await.unwrap().status(), 401);
    // cookie session still works on these routes (with CSRF for writes)
    assert_eq!(c.get(format!("{}/api/jobs", srv.base)).header("cookie", &cookie).send().await.unwrap().status(), 200);
    assert_eq!(c.post(format!("{}/api/resources/search", srv.base)).header("cookie", &cookie).json(&json!({"tmdb_id":"999","media_type":"movie"})).send().await.unwrap().status(), 403);
    // rotation invalidates the old token
    let t2 = mint().await.unwrap().json::<Value>().await.unwrap()["token"].as_str().unwrap().to_string();
    assert_ne!(t1, t2);
    assert_eq!(search(&t1).await.unwrap().status(), 401);
    assert_eq!(search(&t2).await.unwrap().status(), 200);
    // revoke
    c.delete(format!("{}/api/machine-token", srv.base)).header("cookie", &cookie).header("x-re0drive-request", "1").send().await.unwrap();
    assert_eq!(search(&t2).await.unwrap().status(), 401);
}
