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
    let (worker_done_tx,worker_done_rx)=tokio::sync::oneshot::channel();
    let worker_engine=engine.clone();
    let worker = tokio::spawn(async move {let result=worker_engine.run(stop.1).await;let _=worker_done_tx.send(());result});
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
    {let mut term=tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).expect("SIGTERM handler");tokio::select!{_ = tokio::signal::ctrl_c()=>{},_ = term.recv()=>{}}}
    #[cfg(not(unix))]
    {let _=tokio::signal::ctrl_c().await;}
}

