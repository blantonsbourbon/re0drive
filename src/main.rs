use re0drive::{engine::Engine, store::Store, web};
use std::{path::PathBuf, sync::Arc};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "re0drive=info,tower_http=info".into()),
        )
        .init();
    let data_dir = PathBuf::from(std::env::var("RE0DRIVE_DATA").unwrap_or_else(|_| "data".into()));
    let store = Store::open(&data_dir).await?;
    let engine = Arc::new(Engine::new(store));
    if std::env::args().any(|a| a == "--check-gateway") {
        let settings = engine.store.settings().await?;
        let status = engine
            .providers
            .status(&settings)
            .await
            .map_err(|e| anyhow::anyhow!(e.message))?;
        println!("{}", serde_json::to_string(&status)?);
        return Ok(());
    }
    engine.store.recover_jobs().await?;
    let stop = tokio::sync::watch::channel(false);
    let (worker_done_tx, worker_done_rx) = tokio::sync::oneshot::channel();
    let worker_engine = engine.clone();
    let worker_rx = stop.1.clone();
    let worker = tokio::spawn(async move {
        let result = worker_engine.run(worker_rx).await;
        let _ = worker_done_tx.send(());
        result
    });
    re0drive::telegram::spawn(engine.clone(), stop.1.clone());
    let addr = std::env::var("RE0DRIVE_BIND").unwrap_or_else(|_| "127.0.0.1:8787".into());
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    tracing::info!(%addr, "RE0Drive ready");
    axum::serve(listener, web::router(engine)).with_graceful_shutdown(async move {
        tokio::select!{_ = shutdown_signal()=>{},_ = worker_done_rx=>{tracing::error!("background worker stopped; shutting down");}}
        let _ = stop.0.send(true);
    }).await?;
    worker.await??;
    Ok(())
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("SIGTERM handler");
        tokio::select! {_ = tokio::signal::ctrl_c()=>{},_ = term.recv()=>{}}
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}
