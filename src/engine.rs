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
        }
    }
    pub async fn enqueue(
        &self,
        resource: Resource,
        target_cid: String,
        max_points: i64,
        subscription_id: Option<String>,
    ) -> Result<Option<Job>> {
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
        self.store.save_job(job).await
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
        if let Some(ref id) = job.subscription_id {
            if !self
                .store
                .subscriptions()
                .await?
                .iter()
                .any(|r| &r.id == id && r.enabled && r.auto_transfer)
            {
                self.update(&mut job, "skipped", "订阅已停用或删除").await?;
                return Ok(());
            }
        }
        if settings.mode != "demo" && settings.cookie_115.is_empty() {
            self.update(&mut job, "failed", "尚未配置 115 Cookie，没有发起解锁")
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
            Ok(count) => {
                let message = if settings.mode == "demo" {
                    format!("演示完成：模拟接收 {count} 项，没有发起外部操作")
                } else {
                    format!("已接收 {count} 个文件或目录")
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
    async fn scan(&self, rule: Subscription) -> anyhow::Result<()> {
        let settings = self.store.settings().await?;
        let result = self
            .providers
            .search(&settings, &rule.query, &rule.media_type)
            .await;
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
                let mut queued = 0;
                if rule.auto_transfer {
                    for resource in matches.into_iter().take(10) {
                        if self
                            .enqueue(
                                resource,
                                rule.target_cid.clone(),
                                rule.max_points,
                                Some(rule.id.clone()),
                            )
                            .await?
                            .is_some()
                        {
                            queued += 1;
                        }
                    }
                }
                format!("匹配 {count} 项 · 新增 {queued} 个任务")
            }
        };
        self.store
            .finish_subscription(rule.id, message.clone())
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
