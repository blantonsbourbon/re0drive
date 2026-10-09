use crate::models::{Activity, Job, Settings, Subscription};
use anyhow::Result;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use chacha20poly1305::{aead::{Aead, KeyInit}, ChaCha20Poly1305, Nonce};
use rand::RngCore;
use rusqlite::{params, OptionalExtension};
use serde::de::DeserializeOwned;
use std::{path::Path, sync::Arc};
use tokio_rusqlite::Connection;

#[derive(Clone)]
pub struct Store { db: Connection, key: Arc<[u8; 32]> }
fn decode<T: DeserializeOwned>(s: String) -> rusqlite::Result<T> {
    serde_json::from_str(&s).map_err(|e| rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e)))
}
pub fn now() -> i64 { chrono::Utc::now().timestamp() }
impl Store {
    pub async fn open(path: &Path) -> Result<Self> {
        std::fs::create_dir_all(path)?;
        let key_path = path.join("master.key");
        let key: [u8;32] = if key_path.exists() {
            std::fs::read(&key_path)?.try_into().map_err(|_| anyhow::anyhow!("master.key must be exactly 32 bytes"))?
        } else {
            let mut key = [0u8;32]; rand::thread_rng().fill_bytes(&mut key);
            let mut options = std::fs::OpenOptions::new(); options.write(true).create_new(true);
            #[cfg(unix)] { use std::os::unix::fs::OpenOptionsExt; options.mode(0o600); }
            use std::io::Write;
            options.open(&key_path)?.write_all(&key)?;
            key
        };
        let db = Connection::open(path.join("re0drive.db")).await?;
        db.call(|c| { c.execute_batch("PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000; PRAGMA foreign_keys=ON;
            CREATE TABLE IF NOT EXISTS config(id INTEGER PRIMARY KEY CHECK(id=1), encrypted TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS admin(id INTEGER PRIMARY KEY CHECK(id=1), username TEXT NOT NULL, password_hash TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS sessions(token_hash TEXT PRIMARY KEY, expires INTEGER NOT NULL);
            CREATE TABLE IF NOT EXISTS subscriptions(id TEXT PRIMARY KEY, doc TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS jobs(id TEXT PRIMARY KEY, dedup TEXT NOT NULL UNIQUE, status TEXT NOT NULL, doc TEXT NOT NULL, created_at INTEGER NOT NULL);
            CREATE INDEX IF NOT EXISTS jobs_status ON jobs(status,created_at);
            CREATE TABLE IF NOT EXISTS activity(id INTEGER PRIMARY KEY AUTOINCREMENT,kind TEXT NOT NULL,message TEXT NOT NULL,created_at INTEGER NOT NULL);")?; Ok(()) }).await?;
        let store = Self { db, key: Arc::new(key) };
        let has_config = store.db.call(|c| Ok(c.query_row("SELECT COUNT(*) FROM config", [], |r| r.get::<_,i64>(0))? > 0)).await?;
        if !has_config {
            let mut settings = Settings::default();
            if let Ok(secret) = std::env::var("RE0_GATEWAY_SECRET") { settings.re0_shared_secret = secret; settings.mode = "gateway".into(); }
            if let Ok(id) = std::env::var("RE0_INSTALL_ID") { settings.install_id = id; }
            if let Ok(url) = std::env::var("RE0_GATEWAY_URL") { settings.re0_base_url = url; }
            store.save_settings(&settings).await?;
        }
        Ok(store)
    }
    pub async fn settings(&self) -> Result<Settings> {
        let encrypted: String = self.db.call(|c| Ok(c.query_row("SELECT encrypted FROM config WHERE id=1",[],|r|r.get(0))?)).await?;
        let data = STANDARD.decode(encrypted)?;
        anyhow::ensure!(data.len()>12,"invalid encrypted settings");
        let cipher = ChaCha20Poly1305::new((&*self.key).into());
        let plaintext = cipher.decrypt(Nonce::from_slice(&data[..12]), &data[12..]).map_err(|_| anyhow::anyhow!("settings decryption failed; preserve master.key with database backup"))?;
        Ok(serde_json::from_slice(&plaintext)?)
    }
    pub async fn save_settings(&self, settings: &Settings) -> Result<()> {
        let mut nonce = [0u8;12]; rand::thread_rng().fill_bytes(&mut nonce);
        let cipher = ChaCha20Poly1305::new((&*self.key).into());
        let encrypted = cipher.encrypt(Nonce::from_slice(&nonce),serde_json::to_vec(settings)?.as_ref()).map_err(|_| anyhow::anyhow!("settings encryption failed"))?;
        let data = STANDARD.encode([nonce.as_slice(), encrypted.as_slice()].concat());
        self.db.call(move|c| { c.execute("INSERT INTO config(id,encrypted) VALUES(1,?1) ON CONFLICT(id) DO UPDATE SET encrypted=excluded.encrypted",[data])?; Ok(()) }).await?;
        Ok(())
    }
    pub async fn admin(&self) -> Result<Option<(String,String)>> {
        Ok(self.db.call(|c| Ok(c.query_row("SELECT username,password_hash FROM admin WHERE id=1",[],|r|Ok((r.get(0)?,r.get(1)?))).optional()?)).await?)
    }
    pub async fn create_admin(&self, username: String, hash: String) -> Result<bool> {
        Ok(self.db.call(move|c| Ok(c.execute("INSERT OR IGNORE INTO admin(id,username,password_hash) VALUES(1,?1,?2)",params![username,hash])? == 1)).await?)
    }
    pub async fn add_session(&self, hash: String) -> Result<()> {
        self.db.call(move|c| { c.execute("DELETE FROM sessions WHERE expires < ?1",[now()])?; c.execute("INSERT INTO sessions(token_hash,expires) VALUES(?1,?2)",params![hash,now()+86400*7])?; Ok(()) }).await?; Ok(())
    }
    pub async fn valid_session(&self, hash: String) -> Result<bool> {
        Ok(self.db.call(move|c| Ok(c.query_row("SELECT COUNT(*) FROM sessions WHERE token_hash=?1 AND expires>?2",params![hash,now()],|r|r.get::<_,i64>(0))? > 0)).await?)
    }
    pub async fn delete_session(&self, hash: String) -> Result<()> { self.db.call(move|c| { c.execute("DELETE FROM sessions WHERE token_hash=?1",[hash])?; Ok(()) }).await?; Ok(()) }
    pub async fn subscriptions(&self) -> Result<Vec<Subscription>> {
        Ok(self.db.call(|c| { let mut s=c.prepare("SELECT doc FROM subscriptions ORDER BY rowid DESC")?; let rows=s.query_map([],|r|decode(r.get(0)?))?; Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?) }).await?)
    }
    pub async fn save_subscription(&self, rule: &Subscription) -> Result<()> {
        let id=rule.id.clone();let doc=serde_json::to_string(rule)?;
        self.db.call(move|c|{c.execute("INSERT INTO subscriptions(id,doc) VALUES(?1,?2) ON CONFLICT(id) DO UPDATE SET doc=excluded.doc",params![id,doc])?;Ok(())}).await?;Ok(())
    }
    pub async fn delete_subscription(&self,id:String)->Result<()> { self.db.call(move|c|{c.execute("DELETE FROM subscriptions WHERE id=?1",[id])?;Ok(())}).await?;Ok(()) }
    pub async fn finish_subscription(&self,id:String,message:String)->Result<()> {
        self.db.call(move|c| {
            let current:Option<Subscription>=c.query_row("SELECT doc FROM subscriptions WHERE id=?1",[&id],|r|decode(r.get(0)?)).optional()?;
            if let Some(mut rule)=current {
                rule.last_run=Some(now());rule.last_result=message;rule.next_run=now()+i64::from(rule.interval_minutes)*60;
                let doc=serde_json::to_string(&rule).map_err(|e|rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
                c.execute("UPDATE subscriptions SET doc=?1 WHERE id=?2",params![doc,id])?;
            }
            Ok(())
        }).await?;Ok(())
    }
    pub async fn enqueue(&self, job:&Job, dedup:String)->Result<bool> {
        let id=job.id.clone();let doc=serde_json::to_string(job)?;let created=job.created_at;
        Ok(self.db.call(move|c|Ok(c.execute("INSERT OR IGNORE INTO jobs(id,dedup,status,doc,created_at) VALUES(?1,?2,'queued',?3,?4)",params![id,dedup,doc,created])?==1)).await?)
    }
    pub async fn jobs(&self)->Result<Vec<Job>> {
        Ok(self.db.call(|c|{let mut s=c.prepare("SELECT doc FROM jobs ORDER BY created_at DESC,rowid DESC LIMIT 200")?;let rows=s.query_map([],|r|decode(r.get(0)?))?;Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)}).await?)
    }
    pub async fn pending_job(&self)->Result<Option<Job>> {
        Ok(self.db.call(|c|Ok(c.query_row("SELECT doc FROM jobs WHERE status='queued' ORDER BY created_at,rowid LIMIT 1",[],|r|decode(r.get(0)?)).optional()?)).await?)
    }
    pub async fn job(&self,id:String)->Result<Option<Job>> {
        Ok(self.db.call(move|c|Ok(c.query_row("SELECT doc FROM jobs WHERE id=?1",[id],|r|decode(r.get(0)?)).optional()?)).await?)
    }
    pub async fn save_job(&self,job:&Job)->Result<()> {
        let id=job.id.clone();let status=job.status.clone();let doc=serde_json::to_string(job)?;
        self.db.call(move|c|{c.execute("UPDATE jobs SET status=?1,doc=?2 WHERE id=?3",params![status,doc,id])?;Ok(())}).await?;Ok(())
    }
    pub async fn recover_jobs(&self)->Result<()> {
        let active:Vec<Job>=self.db.call(|c|{let mut s=c.prepare("SELECT doc FROM jobs WHERE status IN ('checking','unlocking','transferring')")?;let rows=s.query_map([],|r|decode(r.get(0)?))?;Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)}).await?;
        for mut job in active {
            if job.status=="checking" { job.status="queued".into();job.message="重启后恢复读取任务".into();self.save_job(&job).await?; }
            else if matches!(job.status.as_str(),"unlocking"|"transferring") { job.status="needs_review".into();job.message="服务重启中断了外部操作，请核对网盘和积分后处理".into();self.save_job(&job).await?; }
        }
        Ok(())
    }
    pub async fn job_stats(&self)->Result<serde_json::Value> {
        Ok(self.db.call(|c| {
            let(pending,success,review)=c.query_row("SELECT COALESCE(SUM(status IN ('queued','checking','unlocking','transferring')),0),COALESCE(SUM(status='success' AND json_extract(doc,'$.mode')!='demo'),0),COALESCE(SUM(status='needs_review'),0) FROM jobs",[],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,i64>(1)?,r.get::<_,i64>(2)?)))?;
            Ok(serde_json::json!({"pending":pending,"success":success,"review":review}))
        }).await?)
    }
    pub async fn activity(&self,kind:&str,message:&str)->Result<()> {
        let kind=kind.to_string();let message=message.chars().take(500).collect::<String>();
        self.db.call(move|c|{c.execute("INSERT INTO activity(kind,message,created_at) VALUES(?1,?2,?3)",params![kind,message,now()])?;c.execute("DELETE FROM activity WHERE id NOT IN(SELECT id FROM activity ORDER BY id DESC LIMIT 1000)",[])?;Ok(())}).await?;Ok(())
    }
    pub async fn activities(&self)->Result<Vec<Activity>> {
        Ok(self.db.call(|c|{let mut s=c.prepare("SELECT id,kind,message,created_at FROM activity ORDER BY id DESC LIMIT 60")?;let rows=s.query_map([],|r|Ok(Activity{id:r.get(0)?,kind:r.get(1)?,message:r.get(2)?,created_at:r.get(3)?}))?;Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)}).await?)
    }
}
