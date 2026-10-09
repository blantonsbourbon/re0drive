use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub mode: String,
    pub re0_base_url: String,
    pub install_id: String,
    pub re0_shared_secret: String,
    pub tmdb_api_key: String,
    pub cookie_115: String,
    pub default_target_cid: String,
    pub default_max_points: i64,
    pub checkin_enabled: bool,
    pub last_checkin_day: String,
}
impl Default for Settings {
    fn default() -> Self {
        Self { mode: "demo".into(), re0_base_url: "https://re0.tgtodrive.top".into(), install_id: format!("inst_{}", uuid::Uuid::new_v4().simple()), re0_shared_secret: String::new(), tmdb_api_key: String::new(), cookie_115: String::new(), default_target_cid: "0".into(), default_max_points: 4, checkin_enabled: false, last_checkin_day: String::new() }
    }
}
impl Settings {
    pub fn public(&self) -> serde_json::Value {
        serde_json::json!({"mode":self.mode,"re0_base_url":self.re0_base_url,"install_id":self.install_id,"default_target_cid":self.default_target_cid,"default_max_points":self.default_max_points,"checkin_enabled":self.checkin_enabled,"has_re0_secret":!self.re0_shared_secret.is_empty(),"has_tmdb_key":!self.tmdb_api_key.is_empty(),"has_115_cookie":!self.cookie_115.is_empty(),"last_checkin_day":self.last_checkin_day})
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Resource {
    pub slug: String,
    pub title: String,
    pub provider: String,
    pub source_url: String,
    pub unlock_points: Option<i64>,
    pub is_unlocked: bool,
    pub quality: String,
    #[serde(default)] pub size: String,
    #[serde(default)] pub tmdb_id: String,
    #[serde(default)] pub media_type: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Subscription {
    pub id: String,
    pub name: String,
    pub query: String,
    pub media_type: String,
    pub target_cid: String,
    pub max_points: i64,
    pub quality: String,
    pub include_keywords: String,
    pub exclude_keywords: String,
    pub interval_minutes: u32,
    pub auto_transfer: bool,
    pub enabled: bool,
    pub last_run: Option<i64>,
    pub last_result: String,
    pub next_run: i64,
}
impl Default for Subscription {
    fn default() -> Self { Self { id:String::new(),name:String::new(),query:String::new(),media_type:"movie".into(),target_cid:"0".into(),max_points:4,quality:String::new(),include_keywords:String::new(),exclude_keywords:String::new(),interval_minutes:30,auto_transfer:true,enabled:true,last_run:None,last_result:String::new(),next_run:0 } }
}
impl Subscription {
    pub fn matches(&self, r: &Resource) -> bool {
        if r.provider != "115" { return false; }
        if !r.is_unlocked && r.unlock_points.is_none_or(|n| n < 0 || n > self.max_points) { return false; }
        let text = format!("{} {}",r.title,r.quality).to_lowercase();
        if !self.quality.is_empty() && !text.contains(&self.quality.to_lowercase()) { return false; }
        let keywords = |s: &str| s.split([',','，','\n']).map(str::trim).filter(|v| !v.is_empty()).map(str::to_lowercase).collect::<Vec<_>>();
        keywords(&self.include_keywords).iter().all(|v| text.contains(v)) && !keywords(&self.exclude_keywords).iter().any(|v| text.contains(v))
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Job {
    pub id: String,
    pub resource: Resource,
    pub target_cid: String,
    pub max_points: i64,
    pub subscription_id: Option<String>,
    pub status: String,
    pub message: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub mode: String,
    pub account_key: String,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Activity { pub id: i64, pub kind: String, pub message: String, pub created_at: i64 }

pub fn valid_cid(cid: &str) -> bool { !cid.is_empty() && cid.len() <= 20 && cid.bytes().all(|b| b.is_ascii_digit()) }
pub fn valid_media_type(value: &str) -> bool { matches!(value, "movie"|"tv") }

