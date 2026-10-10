//! NullBR fallback search.
//!
//! Contract taken from the public NullBR Python SDK docs
//! (https://nullbr.readthedocs.io/zh-cn/latest/reference/api.html):
//!
//!   GET {base}/{movie|tv}/{tmdbid}/115?page=N
//!   Headers: X-APP-ID, X-API-KEY, Accept: application/json
//!   Response: { id, media_type, page, total_page, items:[{title,size,share_link,resolution,quality,season_list}] }
//!
//! Credentials are the caller's own App ID / API Key (never shipped). Cloudflare sits in front of
//! `api.nullbr.eu.org`; a plain User-Agent of `re0drive/<version>` is enough for the documented API.
use crate::{
    error::{AppError, Result},
    models::{Resource, Settings},
    providers::{parse_115_share, text},
};
use reqwest::{Client, Method};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use url::Url;

const MAX_PAGES: u32 = 20;

/// Query NullBR for 115-share resources of a TMDB id. Non-115 links are kept as
/// `selectable:false`. Returns an empty Vec when nothing is found (never demo data).
pub async fn search_115(
    http: &Client,
    s: &Settings,
    tmdb_id: &str,
    media_type: &str,
) -> Result<Vec<Resource>> {
    if !s.nullbr_ready() {
        return Ok(vec![]);
    }
    if !matches!(media_type, "movie" | "tv") {
        return Err(AppError::bad("NullBR 仅支持 movie / tv"));
    }
    if tmdb_id.is_empty() || tmdb_id.len() > 12 || !tmdb_id.bytes().all(|b| b.is_ascii_digit()) {
        return Err(AppError::bad("NullBR 需要数字 TMDB ID"));
    }
    let base = s.nullbr_base_url.trim().trim_end_matches('/');
    let mut page: u32 = 1;
    let mut total: u32 = 1;
    let mut out = vec![];
    let mut seen = BTreeSet::new();
    while page <= total && page <= MAX_PAGES {
        let url = format!("{base}/{media_type}/{tmdb_id}/115");
        let response = http
            .request(Method::GET, &url)
            .header("User-Agent", format!("re0drive/{}", env!("CARGO_PKG_VERSION")))
            .header("X-APP-ID", &s.nullbr_app_id)
            .header("X-API-KEY", &s.nullbr_api_key)
            .header("Accept", "application/json")
            .query(&[("page", page.to_string())])
            .send()
            .await
            .map_err(|_| AppError::upstream("NullBR 请求失败"))?;
        let status = response.status();
        if status.as_u16() == 401 || status.as_u16() == 403 {
            return Err(AppError::upstream(format!(
                "NullBR 鉴权失败 ({})",
                status.as_u16()
            )));
        }
        if status.as_u16() == 429 {
            return Err(AppError::upstream("NullBR 速率超限 (429)"));
        }
        if !status.is_success() {
            return Err(AppError::upstream(format!(
                "NullBR HTTP {}",
                status.as_u16()
            )));
        }
        let v: Value = response
            .json()
            .await
            .map_err(|_| AppError::upstream("NullBR 响应不是 JSON"))?;
        total = v
            .get("total_page")
            .and_then(Value::as_u64)
            .unwrap_or(1)
            .min(MAX_PAGES as u64) as u32;
        // Official SDK uses `items`; some older clients used a bare `"115"` array — accept both.
        let items = v
            .get("items")
            .or_else(|| v.get("115"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        for item in items {
            if let Some(r) = normalize_item(&item, tmdb_id, media_type) {
                if seen.insert(r.slug.clone()) {
                    out.push(r);
                }
            }
        }
        page += 1;
    }
    Ok(out)
}

fn normalize_item(v: &Value, tmdb_id: &str, media_type: &str) -> Option<Resource> {
    let share = text(v, &["share_link", "url", "link"]);
    if share.is_empty() {
        return None;
    }
    let url = Url::parse(share.trim()).ok()?;
    let host = url.host_str()?.to_ascii_lowercase();
    let is_115 = ["115.com", "115cdn.com", "anxia.com"]
        .iter()
        .any(|d| host == *d || host.ends_with(&format!(".{d}")));
    // Only 115-share links are selectable; everything else is listed for visibility.
    let selectable = is_115 && parse_115_share(share.trim()).is_ok();
    let size = text(v, &["size", "share_size"]);
    let resolution = v
        .get("resolution")
        .cloned()
        .unwrap_or(Value::Null);
    let quality_v = v.get("quality").cloned().unwrap_or(Value::Null);
    let quality = match &quality_v {
        Value::Array(a) => a
            .iter()
            .filter_map(|x| x.as_str())
            .collect::<Vec<_>>()
            .join(" / "),
        Value::String(s) => s.clone(),
        _ => text(v, &["resolution"]),
    };
    let slug = slug_for(share.trim());
    Some(Resource {
        slug,
        title: text(v, &["title", "name"]),
        provider: if selectable { "115".into() } else { "unknown".into() },
        pan_type: if selectable { "115".into() } else { "other".into() },
        selectable,
        source_url: share.trim().into(),
        unlock_points: Some(0),
        is_unlocked: true, // NullBR already returns the share link; no RE0 unlock needed.
        quality,
        size: size.clone(),
        tmdb_id: tmdb_id.into(),
        media_type: media_type.into(),
        share_size: json!(size),
        video_resolution: if resolution.is_null() {
            Value::Null
        } else if resolution.is_array() {
            resolution
        } else {
            json!([resolution])
        },
        source: quality_v,
        subtitle_language: Value::Null,
        subtitle_type: Value::Null,
        remark: text(v, &["remark", "note"]),
        origin: "nullbr".into(),
    })
}

/// Stable slug derived from the share URL so NullBR results can be de-duplicated and queued.
fn slug_for(share: &str) -> String {
    use sha2::{Digest, Sha256};
    format!("nullbr_{}", &hex::encode(Sha256::digest(share.as_bytes()))[..24])
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn keeps_only_115_selectable() {
        let item = json!({
            "title": "Fixture 2160p",
            "size": "58 GB",
            "share_link": "https://115.com/s/swfixture?password=ab12",
            "resolution": "2160P",
            "quality": ["REMUX", "Blu-ray"]
        });
        let r = normalize_item(&item, "550", "movie").unwrap();
        assert!(r.selectable && r.origin == "nullbr" && r.is_unlocked);
        assert_eq!(r.unlock_points, Some(0));
        assert!(r.slug.starts_with("nullbr_"));
        let bad = normalize_item(
            &json!({"title":"x","share_link":"https://pan.quark.cn/s/abc","size":"1G"}),
            "1",
            "movie",
        )
        .unwrap();
        assert!(!bad.selectable);
    }
}
