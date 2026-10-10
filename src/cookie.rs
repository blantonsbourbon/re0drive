//! 115 Cookie keep-alive.
//!
//! Periodically probes a cheap authenticated endpoint (`GET /files?cid=0&limit=1`) with the stored
//! Cookie. On success, marks the cookie `valid`. On auth failure, marks it `expired` (once) so the
//! Telegram bot / UI can alert the user to re-login via the existing QR flow. The Cookie itself
//! stays encrypted in settings and is never echoed.
use crate::{
    engine::Engine,
    models::Settings,
    store::now,
};
use tracing::{info, warn};

/// Probe result that does not expose the Cookie.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CookieProbe {
    Valid,
    Expired,
    Empty,
    Error(String),
}

impl Engine {
    /// Check the stored 115 Cookie without mutating settings. Safe to call from the UI.
    pub async fn probe_cookie_115(&self) -> CookieProbe {
        let s = match self.store.settings().await {
            Ok(s) => s,
            Err(e) => return CookieProbe::Error(e.to_string()),
        };
        Self::probe_with(&self.providers, &s).await
    }

    async fn probe_with(providers: &crate::providers::Providers, s: &Settings) -> CookieProbe {
        if s.cookie_115.trim().is_empty() {
            return CookieProbe::Empty;
        }
        if s.mode == "demo" {
            return CookieProbe::Valid;
        }
        match providers.list_115_dir(s, "0").await {
            Ok(_) => CookieProbe::Valid,
            Err(e) => {
                let m = e.message.to_lowercase();
                // Typical 115 auth failures: state=false with errno 990001 / "请先登录" / HTTP 401.
                if m.contains("登录")
                    || m.contains("login")
                    || m.contains("auth")
                    || m.contains("401")
                    || m.contains("990001")
                    || m.contains("未登录")
                    || m.contains("请先")
                {
                    CookieProbe::Expired
                } else {
                    CookieProbe::Error(e.message)
                }
            }
        }
    }

    /// Run a keep-alive cycle: probe, persist status, and (once) notify on expiry.
    pub async fn cookie_keepalive(&self) -> anyhow::Result<()> {
        let s = self.store.settings().await?;
        if !s.cookie_check_enabled {
            return Ok(());
        }
        let interval = i64::from(s.cookie_check_minutes.max(5)) * 60;
        if s.cookie_checked_at > 0 && now() - s.cookie_checked_at < interval {
            return Ok(());
        }
        let probe = Self::probe_with(&self.providers, &s).await;
        let _guard = self.config_lock.lock().await;
        let mut s = self.store.settings().await?;
        s.cookie_checked_at = now();
        match probe {
            CookieProbe::Valid => {
                let was = s.cookie_status.clone();
                s.cookie_status = "valid".into();
                s.cookie_alerted = false;
                self.store.save_settings(&s).await?;
                if was != "valid" {
                    info!("115 cookie probe: valid");
                    let _ = self
                        .store
                        .activity("cookie", "115 Cookie 有效")
                        .await;
                }
            }
            CookieProbe::Expired | CookieProbe::Empty => {
                let first = !s.cookie_alerted || s.cookie_status != "expired";
                s.cookie_status = "expired".into();
                if first {
                    s.cookie_alerted = true;
                }
                self.store.save_settings(&s).await?;
                if first {
                    warn!("115 cookie probe: expired / empty");
                    let _ = self
                        .store
                        .activity("cookie", "115 Cookie 已失效，请扫码重新登录")
                        .await;
                    // Notify allowlisted Telegram chats (best-effort; no-op when bot is off).
                    crate::telegram::notify_all(
                        self,
                        "⚠️ <b>115 Cookie 已失效</b>\n请打开 RE0Drive 设置页，扫码重新登录 115。失效期间转存任务会失败。",
                    )
                    .await;
                }
            }
            CookieProbe::Error(msg) => {
                // Transient: don't flip to expired, but bump the timestamp so we back off.
                self.store.save_settings(&s).await?;
                warn!(error = %msg, "115 cookie probe failed (transient)");
            }
        }
        Ok(())
    }
}
