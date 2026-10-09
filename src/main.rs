use re0drive::{engine::Engine, store::Store, web};
use std::{path::PathBuf, sync::Arc};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().with_env_filter(
        tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "re0drive=info,tower_http=info".into()),
    ).init();
    let data_dir = PathBuf::from(std::env::var("RE0DRIVE_DATA").unwrap_or_else(|_| "data".into()));
    let store = Store::open(&data_dir).await?;
    let engine = Arc::new(Engine::new(store));
    engine.store.recover_jobs().await?;
    let stop = tokio::sync::watch::channel(false);
    let worker = tokio::spawn(engine.clone().run(stop.1));
    let addr = std::env::var("RE0DRIVE_BIND").unwrap_or_else(|_| "127.0.0.1:8787".into());
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    tracing::info!(%addr, "RE0Drive ready");
    axum::serve(listener, web::router(engine)).with_graceful_shutdown(async move {
        let _ = tokio::signal::ctrl_c().await;
        let _ = stop.0.send(true);
    }).await?;
    worker.await??;
    Ok(())
}

