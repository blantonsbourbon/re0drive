use crate::{
    error::{AppError, Result},
    models::{Job, Resource, Settings, Subscription, valid_cid},
    providers::{Providers, owned, points, valid_slug},
    store::{Store, now},
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::sync::Arc;
use tokio::sync::Mutex;

pub struct Engine {
    pub store: Store,
    pub providers: Providers,
    pub config_lock: Mutex<()>,
    pub qr: crate::qrlogin::QrSessions,
    pub tg: crate::telegram::Telegram,
}
impl Engine {
    pub fn new(store: Store) -> Self {
        Self::with_providers(store, Providers::new())
    }
    pub fn with_providers(store: Store, providers: Providers) -> Self {
        Self {
            store,
            providers,
            config_lock: Mutex::new(()),
            qr: Default::default(),
            tg: Default::default(),
        }
    }
    /// Queue a transfer of a caller-selected version.
    /// * `target_cid`: empty = 115 root (`0`).
    /// * `max_points`: `None` = the price the caller saw for this version (re-checked right before
    ///   unlocking; a higher live price skips the job), or the settings default when the price was unknown.
    pub async fn enqueue_opt(
        &self,
        resource: Resource,
        target_cid: Option<String>,
        max_points: Option<i64>,
        subscription_id: Option<String>,
    ) -> Result<Option<Job>> {
        let target_cid = target_cid.map(|c| c.trim().to_string()).filter(|c| !c.is_empty()).unwrap_or_else(|| "0".into());
        let max_points = match max_points {
            Some(v) => v,
            None => match resource.unlock_points {
                Some(p) if p >= 0 => p,
                _ => self.store.settings().await?.default_max_points,
            },
        };
        self.enqueue(resource, target_cid, max_points, subscription_id).await
    }
    pub async fn enqueue(
        &self,
        resource: Resource,
        target_cid: String,
        max_points: i64,
        subscription_id: Option<String>,
    ) -> Result<Option<Job>> {
        if !resource.selectable {
            return Err(AppError::bad(format!("{} 资源不能转存到 115", if resource.pan_type.is_empty() { resource.provider.as_str() } else { resource.pan_type.as_str() })));
        }
        if resource.provider != "115"
            || !valid_slug(&resource.slug)
            || !valid_cid(&target_cid)
            || !(0..=1000).contains(&max_points)
        {
            return Err(AppError::bad("资源标识、网盘类型或转存配置无效"));
        }
        let s = self.store.settings().await?;
        let key = account_key(&s);
        let dedup = format!("{key}:{}:{target_cid}", resource.slug);
        let job = Job {
            id: uuid::Uuid::new_v4().to_string(),
            resource,
            target_cid,
            max_points,
            subscription_id,
            status: "queued".into(),
            message: "等待处理".into(),
            created_at: now(),
            updated_at: now(),
            mode: s.mode,
            account_key: key,
            received: vec![],
        };
        if self.store.enqueue(&job, dedup).await? {
            self.store
                .activity("queued", &format!("已加入队列：{}", job.resource.title))
                .await?;
            Ok(Some(job))
        } else {
            Ok(None)
        }
    }
    async fn update(
        &self,
        job: &mut Job,
        status: &str,
        message: impl Into<String>,
    ) -> anyhow::Result<()> {
        job.status = status.into();
        job.message = message.into();
        job.updated_at = now();
        self.store.save_job(job).await?;
        if matches!(status, "success" | "failed" | "skipped" | "needs_review") {
            let chat = self.tg.chat_for_job(&job.id);
            crate::telegram::notify_job(
                self,
                chat,
                &job.id,
                status,
                &job.message,
                &job.resource.title,
            )
            .await;
        }
        Ok(())
    }
    pub async fn execute_job(&self, mut job: Job) -> anyhow::Result<()> {
        let settings = self.store.settings().await?;
        if account_key(&settings) != job.account_key {
            self.update(
                &mut job,
                "needs_review",
                "账号或连接模式已变化，请核对后重新提交",
            )
            .await?;
            return Ok(());
        }
        if job.subscription_id.is_some() {
            // Subscriptions only collect candidates; legacy auto-created jobs never unlock.
            self.update(&mut job, "skipped", "订阅不会自动解锁，请在候选版本中确认后转存")
                .await?;
            return Ok(());
        }
        if settings.mode != "demo" && settings.cookie_115.is_empty() {
            self.update(&mut job, "failed", "尚未配置 115 Cookie，没有发起解锁")
                .await?;
            return Ok(());
        }
        if job.resource.origin == "nullbr" {
            // Feature removed: do not unlock or receive leftover NullBR-queued jobs.
            self.update(&mut job, "skipped", "NullBR 回退已移除，请重新搜索后转存")
                .await?;
            return Ok(());
        }
        self.update(&mut job, "checking", "检查资源积分和解锁状态")
            .await?;
        let detail = match self.providers.detail(&settings, &job.resource.slug).await {
            Ok(v) => v,
            Err(e) => {
                self.update(&mut job, "failed", e.message).await?;
                return Ok(());
            }
        };
        let data = detail.get("data").unwrap_or(&detail);
        // Never spend points on a version that cannot land in 115.
        let pan = crate::providers::text(data, &["pan_type", "website", "provider"]);
        if !pan.is_empty() && !pan.contains("115") {
            self.update(&mut job, "skipped", format!("资源网盘类型为 {pan}，不能转存到 115，未解锁"))
                .await?;
            return Ok(());
        }
        if !owned(data) {
            match points(data) {
                None => {
                    self.update(&mut job, "skipped", "积分未知，已跳过解锁")
                        .await?;
                    return Ok(());
                }
                Some(cost) if cost < 0 || cost > job.max_points => {
                    let cap = job.max_points;
                    self.update(
                        &mut job,
                        "skipped",
                        format!("需要 {cost} 积分，超过上限 {cap}"),
                    )
                    .await?;
                    return Ok(());
                }
                _ => {}
            }
        }
        self.update(&mut job, "unlocking", "正在取得分享链接")
            .await?;
        let link = match self.providers.unlock(&settings, &job.resource.slug).await {
            Ok(link) => link,
            Err(e) => {
                self.update(&mut job, "needs_review", e.message).await?;
                return Ok(());
            }
        };
        self.update(&mut job, "transferring", "正在向 115 接收分享")
            .await?;
        match self
            .providers
            .receive_115(&settings, &link, &job.target_cid)
            .await
        {
            Ok(entries) => {
                let count = entries.len();
                // Receive succeeded; now resolve local ids in the target folder (read-only listing).
                let note = match self.providers.resolve_received(&settings, &entries, &job.target_cid).await {
                    Ok(items) => {
                        let found = items.iter().filter(|r| !r.local_id.is_empty()).count();
                        job.received = items;
                        format!("本地已定位 {found}/{count} 项")
                    }
                    Err(e) => {
                        job.received = entries
                            .iter()
                            .map(|e| crate::models::ReceivedItem { name: e.name.clone(), is_dir: e.is_dir, share_id: e.id.clone(), parent_cid: job.target_cid.clone(), size: e.size, ..Default::default() })
                            .collect();
                        format!("未能列出目标目录：{}", e.message)
                    }
                };
                let message = if settings.mode == "demo" {
                    format!("演示完成：模拟接收 {count} 项，没有发起外部操作")
                } else {
                    format!("已接收 {count} 个文件或目录 · {note}")
                };
                self.update(&mut job, "success", &message).await?;
                self.store
                    .activity("success", &format!("{} · {message}", job.resource.title))
                    .await?;
            }
            Err(e) => {
                self.update(&mut job, "needs_review", e.message).await?;
                self.store
                    .activity(
                        "review",
                        &format!("{} · 需要核对外部操作结果", job.resource.title),
                    )
                    .await?;
            }
        }
        Ok(())
    }
    pub async fn scan(&self, rule: Subscription) -> anyhow::Result<()> {
        let settings = self.store.settings().await?;
        let result = self
            .providers
            .search(&settings, &rule.query, &rule.media_type)
            .await;
        let mut candidates = None;
        let message = match result {
            Err(e) => format!("检查失败：{}", e.message),
            Ok(items) => {
                if account_key(&settings) != account_key(&self.store.settings().await?) {
                    return Ok(());
                }
                let current = self
                    .store
                    .subscriptions()
                    .await?
                    .into_iter()
                    .find(|r| r.id == rule.id);
                let Some(rule) = current.filter(|r| {
                    r.enabled && r.query == rule.query && r.media_type == rule.media_type
                }) else {
                    return Ok(());
                };
                let matches = items
                    .into_iter()
                    .filter(|r| rule.matches(r))
                    .collect::<Vec<_>>();
                let count = matches.len();
                // Subscriptions never unlock by themselves (not even 0-point or auto_transfer rules):
                // they record candidate versions; the caller picks one and creates a job.
                candidates = Some(matches.into_iter().take(20).collect());
                format!("匹配 {count} 个候选版本 · 待选择")
            }
        };
        self.store
            .finish_subscription(rule.id, message.clone(), candidates)
            .await?;
        self.store
            .activity("scan", &format!("{} · {message}", rule.name))
            .await?;
        Ok(())
    }
    pub async fn checkin(&self) -> Result<Value> {
        let _guard = self.config_lock.lock().await;
        let mut s = self.store.settings().await?;
        let result = self.providers.checkin(&s).await?;
        s.last_checkin_day = chrono::Local::now().format("%Y-%m-%d").to_string();
        self.store.save_settings(&s).await?;
        self.store
            .activity(
                "checkin",
                if s.mode == "demo" {
                    "演示签到，没有发起外部操作"
                } else {
                    "签到接口已返回成功"
                },
            )
            .await?;
        Ok(result)
    }
    pub async fn run(
        self: Arc<Self>,
        mut stop: tokio::sync::watch::Receiver<bool>,
    ) -> anyhow::Result<()> {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(2));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut next_checkin_retry = 0i64;
        loop {
            tokio::select! {_ = interval.tick()=>{},_ = stop.changed()=>{return Ok(());}}
            if let Some(job) = self.store.pending_job().await? {
                if let Err(e) = self.execute_job(job).await {
                    tracing::error!(error=%e,"job persistence failed");
                    return Err(e);
                }
            }
            for rule in self
                .store
                .subscriptions()
                .await?
                .into_iter()
                .filter(|r| r.enabled && r.next_run <= now())
                .take(3)
            {
                if let Err(e) = self.scan(rule).await {
                    tracing::error!(error=%e,"subscription scan failed");
                }
            }
            if let Err(e) = self.cookie_keepalive().await {
                tracing::warn!(error=%e, "cookie keep-alive failed");
            }
            let s = self.store.settings().await?;
            let day = chrono::Local::now().format("%Y-%m-%d").to_string();
            if s.checkin_enabled && s.last_checkin_day != day && now() >= next_checkin_retry {
                next_checkin_retry = now() + 3600;
                if let Err(e) = self.checkin().await {
                    let _ = self
                        .store
                        .activity("error", &format!("签到失败：{}", e.message))
                        .await;
                }
            }
        }
    }
}
pub fn account_key(s: &Settings) -> String {
    hex::encode(Sha256::digest(
        format!(
            "{}\n{}\n{}\n{}\n{}",
            s.mode, s.re0_base_url, s.install_id, s.cookie_115, s.re0_shared_secret
        )
        .as_bytes(),
    ))
}
