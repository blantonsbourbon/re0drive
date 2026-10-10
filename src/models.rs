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
    // --- NullBR fallback (public API: https://nullbr.readthedocs.io). User-registered credentials. ---
    pub nullbr_enabled: bool,
    pub nullbr_base_url: String,
    /// The caller's own NullBR App ID (no default is shipped).
    pub nullbr_app_id: String,
    /// Secret; never echoed.
    pub nullbr_api_key: String,
    // --- 115 Cookie keep-alive ---
    pub cookie_check_enabled: bool,
    /// Minutes between validity checks.
    pub cookie_check_minutes: u32,
    /// "unknown" | "valid" | "expired" (runtime state, not a secret).
    pub cookie_status: String,
    pub cookie_checked_at: i64,
    /// An expiry alert was already sent for the current cookie (reset when it becomes valid/replaced).
    pub cookie_alerted: bool,
    // --- Telegram bot (long polling) ---
    pub tg_enabled: bool,
    /// Secret; never echoed.
    pub tg_bot_token: String,
    /// Comma-separated numeric chat/user ids allowed to use the bot.
    pub tg_allowed_chats: String,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            mode: "demo".into(),
            re0_base_url: "https://re0.tgtodrive.top".into(),
            install_id: format!("inst_{}", uuid::Uuid::new_v4().simple()),
            re0_shared_secret: String::new(),
            tmdb_api_key: String::new(),
            cookie_115: String::new(),
            default_target_cid: "0".into(),
            default_max_points: 20,
            checkin_enabled: false,
            last_checkin_day: String::new(),
            nullbr_enabled: false,
            nullbr_base_url: "https://api.nullbr.eu.org".into(),
            nullbr_app_id: String::new(),
            nullbr_api_key: String::new(),
            cookie_check_enabled: true,
            cookie_check_minutes: 360,
            cookie_status: "unknown".into(),
            cookie_checked_at: 0,
            cookie_alerted: false,
            tg_enabled: false,
            tg_bot_token: String::new(),
            tg_allowed_chats: String::new(),
        }
    }
}
impl Settings {
    /// Parsed Telegram allowlist (ignores blanks / non-numeric entries).
    pub fn tg_allowlist(&self) -> Vec<i64> {
        self.tg_allowed_chats
            .split([',', ' ', '\n'])
            .filter_map(|v| v.trim().parse().ok())
            .collect()
    }
    pub fn nullbr_ready(&self) -> bool {
        self.nullbr_enabled && !self.nullbr_app_id.is_empty() && !self.nullbr_api_key.is_empty()
    }
    pub fn public(&self) -> serde_json::Value {
        serde_json::json!({"mode":self.mode,"re0_base_url":self.re0_base_url,"install_id":self.install_id,"default_target_cid":self.default_target_cid,"default_max_points":self.default_max_points,"checkin_enabled":self.checkin_enabled,"has_re0_secret":!self.re0_shared_secret.is_empty(),"has_tmdb_key":!self.tmdb_api_key.is_empty(),"has_115_cookie":!self.cookie_115.is_empty(),"last_checkin_day":self.last_checkin_day,
            "nullbr_enabled":self.nullbr_enabled,"nullbr_base_url":self.nullbr_base_url,"nullbr_app_id":self.nullbr_app_id,"has_nullbr_key":!self.nullbr_api_key.is_empty(),
            "cookie_check_enabled":self.cookie_check_enabled,"cookie_check_minutes":self.cookie_check_minutes,"cookie_status":self.cookie_status,"cookie_checked_at":self.cookie_checked_at,
            "tg_enabled":self.tg_enabled,"has_tg_token":!self.tg_bot_token.is_empty(),"tg_allowed_chats":self.tg_allowed_chats})
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Resource {
    pub slug: String,
    pub title: String,
    /// Normalized: "115" when transferable, otherwise the lower-cased raw pan type (e.g. "quark", "ed2k").
    pub provider: String,
    pub source_url: String,
    pub unlock_points: Option<i64>,
    pub is_unlocked: bool,
    pub quality: String,
    #[serde(default)]
    pub size: String,
    #[serde(default)]
    pub tmdb_id: String,
    #[serde(default)]
    pub media_type: String,
    /// Raw relay `pan_type` (or equivalent) exactly as returned.
    #[serde(default)]
    pub pan_type: String,
    /// Only 115 resources can be queued. Non-115 results are still listed with `selectable: false`.
    #[serde(default = "yes")]
    pub selectable: bool,
    /// Raw relay values, kept verbatim (often arrays).
    #[serde(default)]
    pub share_size: serde_json::Value,
    #[serde(default)]
    pub video_resolution: serde_json::Value,
    #[serde(default)]
    pub source: serde_json::Value,
    #[serde(default)]
    pub subtitle_language: serde_json::Value,
    #[serde(default)]
    pub subtitle_type: serde_json::Value,
    #[serde(default)]
    pub remark: String,
    /// Where this result came from: "relay" (RE0 relay; needs unlock) or "nullbr"
    /// (NullBR fallback; `source_url` is already the 115 share link, so no unlock/points).
    #[serde(default = "relay_origin")]
    pub origin: String,
}
fn relay_origin() -> String {
    "relay".into()
}
fn yes() -> bool {
    true
}
impl Resource {
    /// Text used for keyword / quality matching: title + all descriptive fields.
    pub fn match_text(&self) -> String {
        let flat = |v: &serde_json::Value| match v {
            serde_json::Value::Array(a) => a.iter().map(|x| x.as_str().map(str::to_string).unwrap_or_else(|| x.to_string())).collect::<Vec<_>>().join(" "),
            serde_json::Value::Null => String::new(),
            serde_json::Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        format!(
            "{} {} {} {} {} {} {}",
            self.title,
            self.quality,
            flat(&self.video_resolution),
            flat(&self.source),
            flat(&self.subtitle_language),
            flat(&self.subtitle_type),
            self.remark
        )
        .to_lowercase()
    }
}

/// One item received into the 115 target folder, resolved to the *local* id after receive.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ReceivedItem {
    pub name: String,
    pub is_dir: bool,
    /// Share-side id that was received (snap fid for files, snap cid for folders).
    pub share_id: String,
    /// Local id in the user's 115 after receive: file fid, or the folder's own cid. Empty if not found.
    pub local_id: String,
    /// Folder that contains the received item (the job's target cid).
    pub parent_cid: String,
    pub size: i64,
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
    /// Matching versions found by the last scan. Subscriptions never unlock by themselves:
    /// the caller picks a version and creates a job.
    pub candidates: Vec<Resource>,
}
impl Default for Subscription {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            query: String::new(),
            media_type: "movie".into(),
            target_cid: "0".into(),
            max_points: 20,
            quality: String::new(),
            include_keywords: String::new(),
            exclude_keywords: String::new(),
            interval_minutes: 30,
            auto_transfer: false,
            enabled: true,
            last_run: None,
            last_result: String::new(),
            next_run: 0,
            candidates: vec![],
        }
    }
}
impl Subscription {
    pub fn matches(&self, r: &Resource) -> bool {
        if r.provider != "115" || !r.selectable {
            return false;
        }
        if !r.is_unlocked && r.unlock_points.is_none_or(|n| n < 0 || n > self.max_points) {
            return false;
        }
        let text = r.match_text();
        if !self.quality.is_empty() && !text.contains(&self.quality.to_lowercase()) {
            return false;
        }
        let keywords = |s: &str| {
            s.split([',', '，', '\n'])
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .map(str::to_lowercase)
                .collect::<Vec<_>>()
        };
        keywords(&self.include_keywords)
            .iter()
            .all(|v| text.contains(v))
            && !keywords(&self.exclude_keywords)
                .iter()
                .any(|v| text.contains(v))
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
    /// Items received into 115 with their local ids (filled after a successful receive).
    #[serde(default)]
    pub received: Vec<ReceivedItem>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Activity {
    pub id: i64,
    pub kind: String,
    pub message: String,
    pub created_at: i64,
}

pub fn valid_cid(cid: &str) -> bool {
    !cid.is_empty() && cid.len() <= 20 && cid.bytes().all(|b| b.is_ascii_digit())
}
pub fn valid_media_type(value: &str) -> bool {
    matches!(value, "movie" | "tv")
}

impl Default for Resource {
    fn default() -> Self {
        serde_json::from_value(serde_json::json!({
            "slug": "", "title": "", "provider": "115", "source_url": "",
            "unlock_points": null, "is_unlocked": false, "quality": "", "size": "",
            "tmdb_id": "", "media_type": ""
        }))
        .expect("default resource")
    }
}
