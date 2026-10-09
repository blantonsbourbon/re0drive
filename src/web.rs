use crate::{engine::{account_key, Engine}, error::{AppError, Result}, models::{valid_cid, valid_media_type, Resource, Subscription}};
use argon2::{password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString}, Argon2};
use axum::{extract::{DefaultBodyLimit, Path, Request, State}, http::{HeaderMap, HeaderValue, StatusCode}, middleware::{self, Next}, response::{Html, IntoResponse, Response}, routing::{get, post}, Json, Router};
use rand::RngCore;
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::sync::{Arc, OnceLock};
use tower_http::{compression::CompressionLayer, trace::TraceLayer};

type App = Arc<Engine>;
pub fn router(engine:App)->Router {
    let protected=Router::new()
        .route("/api/dashboard",get(dashboard))
        .route("/api/settings",get(settings).put(save_settings))
        .route("/api/resources/search",post(search))
        .route("/api/accounts/re0/status",get(re0_status))
        .route("/api/accounts/re0/authorize",post(authorize))
        .route("/api/accounts/checkin",post(checkin))
        .route("/api/subscriptions",get(subscriptions).post(save_subscription))
        .route("/api/subscriptions/{id}",post(update_subscription).delete(delete_subscription))
        .route("/api/subscriptions/{id}/run",post(run_subscription))
        .route("/api/jobs",get(jobs).post(enqueue))
        .route("/api/jobs/{id}/retry",post(retry_job))
        .route("/api/auth/logout",post(logout))
        .route_layer(middleware::from_fn_with_state(engine.clone(),require_auth));
    Router::new().merge(protected)
        .route("/",get(||async{Html(include_str!("../web/index.html"))}))
        .route("/app.js",get(||async{([("content-type","text/javascript; charset=utf-8")],include_str!("../web/app.js"))}))
        .route("/style.css",get(||async{([("content-type","text/css; charset=utf-8")],include_str!("../web/style.css"))}))
        .route("/health",get(||async{Json(json!({"status":"ok","version":env!("CARGO_PKG_VERSION")}))}))
        .route("/api/auth/status",get(auth_status))
        .route("/api/auth/setup",post(setup))
        .route("/api/auth/login",post(login))
        .layer(middleware::from_fn(headers_and_csrf))
        .layer(DefaultBodyLimit::max(32*1024))
        .layer(CompressionLayer::new())
        .layer(TraceLayer::new_for_http())
        .with_state(engine)
}
async fn headers_and_csrf(request:Request,next:Next)->Response {
    if request.uri().path().starts_with("/api/")&&!matches!(request.method().as_str(),"GET"|"HEAD"|"OPTIONS")&&request.headers().get("x-re0drive-request").and_then(|v|v.to_str().ok())!=Some("1"){
        return AppError{status:StatusCode::FORBIDDEN,message:"请求来源校验失败".into()}.into_response();
    }
    let mut response=next.run(request).await;
    for(k,v)in [("x-content-type-options","nosniff"),("referrer-policy","no-referrer"),("cache-control","no-store"),("content-security-policy","default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' data:; connect-src 'self'; base-uri 'none'; frame-ancestors 'none'; form-action 'self'")] {response.headers_mut().insert(axum::http::header::HeaderName::from_static(k),HeaderValue::from_static(v));}
    response
}
fn session_token(headers:&HeaderMap)->Option<String>{headers.get("cookie")?.to_str().ok()?.split(';').find_map(|s|s.trim().strip_prefix("re0drive_session=").map(str::to_string))}
fn token_hash(s:&str)->String{hex::encode(Sha256::digest(s.as_bytes()))}
async fn require_auth(State(app):State<App>,request:Request,next:Next)->Result<Response>{
    let token=session_token(request.headers()).ok_or_else(AppError::unauthorized)?;
    if token.len()!=64||!app.store.valid_session(token_hash(&token)).await?{return Err(AppError::unauthorized());}
    Ok(next.run(request).await)
}
async fn auth_status(State(app):State<App>,headers:HeaderMap)->Result<Json<Value>>{
    let configured=app.store.admin().await?.is_some();
    let authenticated=if let Some(t)=session_token(&headers){t.len()==64&&app.store.valid_session(token_hash(&t)).await?}else{false};
    Ok(Json(json!({"configured":configured,"authenticated":authenticated})))
}
#[derive(Deserialize)]struct Login{username:String,password:String}
async fn password_hash(password:String)->Result<String>{
    tokio::task::spawn_blocking(move||{let mut salt=[0u8;16];rand::thread_rng().fill_bytes(&mut salt);let salt=SaltString::encode_b64(&salt).map_err(|_|AppError::bad("密码处理失败"))?;Argon2::default().hash_password(password.as_bytes(),&salt).map(|v|v.to_string()).map_err(|_|AppError::bad("密码处理失败"))}).await.map_err(|_|AppError::bad("密码处理失败"))?
}
fn validate_login(v:&Login)->Result<()> {if v.username.trim().is_empty()||v.username.len()>64||v.password.len()<8||v.password.len()>128 {return Err(AppError::bad("用户名需为 1–64 字符，密码需为 8–128 字符"));}Ok(())}
fn login_rate_limit()->Result<()> {
    static ATTEMPTS:OnceLock<std::sync::Mutex<(i64,u32)>>=OnceLock::new();let mut value=ATTEMPTS.get_or_init(||std::sync::Mutex::new((0,0))).lock().expect("login limiter");
    if crate::store::now()-value.0>60{*value=(crate::store::now(),0);}value.1+=1;
    if value.1>20{return Err(AppError{status:StatusCode::TOO_MANY_REQUESTS,message:"登录尝试过多，请稍后重试".into()});}Ok(())
}
async fn session_response(app:&App,headers:&HeaderMap)->Result<Response>{
    let mut bytes=[0u8;32];rand::thread_rng().fill_bytes(&mut bytes);let token=hex::encode(bytes);app.store.add_session(token_hash(&token)).await?;
    let secure=headers.get("x-forwarded-proto").and_then(|v|v.to_str().ok())==Some("https")||std::env::var("RE0DRIVE_COOKIE_SECURE").as_deref()==Ok("1");
    let cookie=format!("re0drive_session={token}; Path=/; HttpOnly; SameSite=Strict; Max-Age=604800{}",if secure{"; Secure"}else{""});
    Ok(([("set-cookie",cookie)],Json(json!({"ok":true}))).into_response())
}
async fn setup(State(app):State<App>,headers:HeaderMap,Json(v):Json<Login>)->Result<Response>{
    login_rate_limit()?;validate_login(&v)?;
    if app.store.admin().await?.is_some(){return Err(AppError::bad("管理员已创建，请登录"));}
    let hash=password_hash(v.password).await?;
    if !app.store.create_admin(v.username.trim().into(),hash).await?{return Err(AppError::bad("管理员已创建，请登录"));}
    session_response(&app,&headers).await
}
async fn login(State(app):State<App>,headers:HeaderMap,Json(v):Json<Login>)->Result<Response>{
    login_rate_limit()?;validate_login(&v)?;
    let Some((username,hash))=app.store.admin().await? else{return Err(AppError::bad("请先创建管理员"));};
    let valid=tokio::task::spawn_blocking(move||username==v.username&&PasswordHash::new(&hash).is_ok_and(|hash|Argon2::default().verify_password(v.password.as_bytes(),&hash).is_ok())).await.map_err(|_|AppError::bad("登录处理失败"))?;
    if !valid{return Err(AppError::unauthorized());}session_response(&app,&headers).await
}
async fn logout(State(app):State<App>,headers:HeaderMap)->Result<Response>{if let Some(t)=session_token(&headers){app.store.delete_session(token_hash(&t)).await?;}Ok(([("set-cookie","re0drive_session=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0")],Json(json!({"ok":true}))).into_response())}
async fn settings(State(app):State<App>)->Result<Json<Value>>{Ok(Json(app.store.settings().await?.public()))}
#[derive(Deserialize)]#[serde(deny_unknown_fields)]struct SettingsUpdate{
    mode:String,re0_base_url:String,install_id:String,default_target_cid:String,default_max_points:i64,checkin_enabled:bool,
    #[serde(default)]re0_shared_secret:String,#[serde(default)]tmdb_api_key:String,#[serde(default)]cookie_115:String,
    #[serde(default)]clear_re0_secret:bool,#[serde(default)]clear_tmdb_key:bool,#[serde(default)]clear_115_cookie:bool,
}
async fn save_settings(State(app):State<App>,Json(v):Json<SettingsUpdate>)->Result<Json<Value>>{
    if !matches!(v.mode.as_str(),"demo"|"gateway"){return Err(AppError::bad("连接模式无效"));}
    let parsed=url::Url::parse(v.re0_base_url.trim()).map_err(|_|AppError::bad("中转 API 地址无效"))?;
    if !matches!(parsed.scheme(),"https"|"http")||parsed.host_str().is_none()||!parsed.username().is_empty()||parsed.password().is_some()||parsed.query().is_some()||parsed.fragment().is_some(){return Err(AppError::bad("请填写不含账号、查询或片段的中转 API 地址"));}
    if !valid_cid(&v.default_target_cid)||!(0..=1000).contains(&v.default_max_points)||v.install_id.is_empty()||v.install_id.len()>200||v.install_id.contains(['\r','\n'])||v.cookie_115.contains(['\r','\n']){return Err(AppError::bad("目录、积分上限或账号配置无效"));}
    let _guard=app.config_lock.lock().await;let mut s=app.store.settings().await?;
    if s.mode!=v.mode||s.install_id!=v.install_id||s.re0_base_url!=v.re0_base_url.trim().trim_end_matches('/') {s.last_checkin_day.clear();}
    s.mode=v.mode;s.re0_base_url=v.re0_base_url.trim().trim_end_matches('/').into();s.install_id=v.install_id;s.default_target_cid=v.default_target_cid;s.default_max_points=v.default_max_points;s.checkin_enabled=v.checkin_enabled;
    for(target,value,clear)in[(&mut s.re0_shared_secret,v.re0_shared_secret,v.clear_re0_secret),(&mut s.tmdb_api_key,v.tmdb_api_key,v.clear_tmdb_key),(&mut s.cookie_115,v.cookie_115,v.clear_115_cookie)]{if clear{target.clear();}else if !value.trim().is_empty(){*target=value.trim().into();}}
    app.store.save_settings(&s).await?;app.store.activity("settings","账号和默认规则已更新").await?;Ok(Json(s.public()))
}
async fn dashboard(State(app):State<App>)->Result<Json<Value>>{
    let rules=app.store.subscriptions().await?;let jobs=app.store.jobs().await?;let logs=app.store.activities().await?;let s=app.store.settings().await?;
    let mut stats=app.store.job_stats().await?;stats["subscriptions"]=json!(rules.iter().filter(|r|r.enabled).count());
    Ok(Json(json!({"settings":s.public(),"subscriptions":rules,"jobs":jobs,"activity":logs,"stats":stats})))
}
#[derive(Deserialize)]struct Search{query:String,media_type:String}
async fn search(State(app):State<App>,Json(v):Json<Search>)->Result<Json<Value>>{
    if v.query.trim().is_empty()||v.query.len()>300||!valid_media_type(&v.media_type){return Err(AppError::bad("请填写片名或 TMDB ID，并选择电影/剧集"));}
    let s=app.store.settings().await?;let resources=app.providers.search(&s,&v.query,&v.media_type).await?;Ok(Json(json!({"mode":s.mode,"resources":resources})))
}
async fn re0_status(State(app):State<App>)->Result<Json<Value>>{Ok(Json(app.providers.status(&app.store.settings().await?).await?))}
async fn authorize(State(app):State<App>)->Result<Json<Value>>{let s=app.store.settings().await?;if s.mode=="demo"{return Err(AppError::bad("请切换到中转 API 模式并配置签名密钥"));}Ok(Json(json!({"url":app.providers.auth_url(&s)?})))}
async fn checkin(State(app):State<App>)->Result<Json<Value>>{Ok(Json(app.checkin().await?))}
async fn subscriptions(State(app):State<App>)->Result<Json<Vec<Subscription>>>{Ok(Json(app.store.subscriptions().await?))}
fn validate_rule(v:&Subscription)->Result<()> {if v.name.trim().is_empty()||v.name.len()>120||v.query.trim().is_empty()||v.query.len()>300||!valid_media_type(&v.media_type)||!valid_cid(&v.target_cid)||!(0..=1000).contains(&v.max_points)||!(5..=1440).contains(&v.interval_minutes)||v.include_keywords.len()>500||v.exclude_keywords.len()>500||v.quality.len()>80{return Err(AppError::bad("请检查订阅名称、搜索条件、目录和检查间隔（5–1440 分钟）"));}Ok(())}
async fn save_subscription(State(app):State<App>,Json(mut v):Json<Subscription>)->Result<Json<Subscription>>{validate_rule(&v)?;v.id=uuid::Uuid::new_v4().to_string();v.next_run=0;v.last_run=None;v.last_result="等待首次检查".into();app.store.save_subscription(&v).await?;Ok(Json(v))}
async fn update_subscription(State(app):State<App>,Path(id):Path<String>,Json(mut v):Json<Subscription>)->Result<Json<Subscription>>{
    validate_rule(&v)?;let old=app.store.subscriptions().await?.into_iter().find(|r|r.id==id).ok_or_else(||AppError::bad("订阅不存在"))?;
    v.id=id;v.last_run=old.last_run;v.last_result=old.last_result;v.next_run=if v.query!=old.query||v.enabled&&!old.enabled{0}else{old.next_run};app.store.save_subscription(&v).await?;Ok(Json(v))
}
async fn delete_subscription(State(app):State<App>,Path(id):Path<String>)->Result<Json<Value>>{app.store.delete_subscription(id).await?;Ok(Json(json!({"ok":true})))}
async fn run_subscription(State(app):State<App>,Path(id):Path<String>)->Result<Json<Value>>{let mut r=app.store.subscriptions().await?.into_iter().find(|r|r.id==id).ok_or_else(||AppError::bad("订阅不存在"))?;if !r.enabled{return Err(AppError::bad("请先启用订阅"));}r.next_run=0;app.store.save_subscription(&r).await?;Ok(Json(json!({"ok":true,"message":"已安排检查"})))}
async fn jobs(State(app):State<App>)->Result<Json<Value>>{Ok(Json(json!({"jobs":app.store.jobs().await?})))}
#[derive(Deserialize)]struct Transfer{resource:Resource,target_cid:String,max_points:i64}
async fn enqueue(State(app):State<App>,Json(v):Json<Transfer>)->Result<Json<Value>>{if !valid_cid(&v.target_cid)||!(0..=1000).contains(&v.max_points){return Err(AppError::bad("目标目录或积分上限无效"));}let job=app.enqueue(v.resource,v.target_cid,v.max_points,None).await?;Ok(Json(json!({"job":job,"duplicate":job.is_none()})))}
#[derive(Deserialize)]struct Retry{#[serde(default)]reviewed:bool}
async fn retry_job(State(app):State<App>,Path(id):Path<String>,Json(v):Json<Retry>)->Result<Json<Value>>{
    let mut job=app.store.job(id).await?.ok_or_else(||AppError::bad("任务不存在"))?;
    if job.status!="failed"&&job.status!="needs_review"{return Err(AppError::bad("这个任务不需要重试"));}
    if job.status=="needs_review"&&!v.reviewed{return Err(AppError::bad("请先核对积分和网盘中的结果"));}
    let s=app.store.settings().await?;if account_key(&s)!=job.account_key{return Err(AppError::bad("原任务账号已变化，请重新搜索并创建任务"));}
    job.status="queued".into();job.message="已安排重新处理".into();job.updated_at=crate::store::now();app.store.save_job(&job).await?;Ok(Json(json!({"ok":true})))
}
