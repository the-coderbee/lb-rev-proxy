use std::net::SocketAddr;

use anyhow::{Context, Result};
use tokio::net::TcpListener;
use tracing::info;
use tracing_subscriber::{EnvFilter, filter::LevelFilter, fmt, prelude::*};

const LISTENING_ADDRESS: &str = "127.0.0.1:8000";
const BACKEND_ADDRESS: &str = "127.0.0.1:8001";

#[tokio::main]
async fn main() -> Result<()> {
    let filter = EnvFilter::builder()
        .with_default_directive(LevelFilter::INFO.into())
        .from_env_lossy();
    tracing_subscriber::registry()
        .with(fmt::layer().with_filter(filter))
        .init();
    let listener = TcpListener::bind(LISTENING_ADDRESS)
        .await
        .with_context(|| format!("error binding address: {LISTENING_ADDRESS}"))?;
    let backend: SocketAddr = BACKEND_ADDRESS
        .parse()
        .with_context(|| format!("error parsing backend address: {BACKEND_ADDRESS}"))?;
    info!(listen = %listener.local_addr()?, %backend, "proxy started");
    lb_rev_proxy::run(listener, backend).await;
    Ok(())
}
