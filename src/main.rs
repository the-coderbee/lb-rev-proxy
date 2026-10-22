use std::{net::SocketAddr, sync::Arc};

use anyhow::{Context, Result};
use lb_rev_proxy::{ProxyConfig, RoundRobin};
use rlimit::Resource;
use tokio::net::TcpListener;
use tracing::{info, warn};
use tracing_subscriber::{EnvFilter, filter::LevelFilter, fmt, prelude::*};

const LISTENING_ADDRESS: &str = "127.0.0.1:8000";
const BACKEND_ADDRESSES: [&str; 3] = ["127.0.0.1:8001", "127.0.0.1:8002", "127.0.0.1:8003"];

#[tokio::main]
async fn main() -> Result<()> {
    let filter = EnvFilter::builder()
        .with_default_directive(LevelFilter::INFO.into())
        .from_env_lossy();
    tracing_subscriber::registry()
        .with(fmt::layer().with_filter(filter))
        .init();
    let config = ProxyConfig::default();
    let backends: Vec<SocketAddr> = BACKEND_ADDRESSES
        .iter()
        .map(|addr| {
            addr.parse::<SocketAddr>()
                .with_context(|| format!("invalid backend address: {addr}"))
        })
        .collect::<Result<_>>()?;
    info!(?backends, "backends configured");
    let balancer = RoundRobin::new(backends).context("failed to create the balancer")?;
    match Resource::NOFILE.get() {
        Ok((soft, hard)) => info!(soft, hard, "file descriptor limit"),
        Err(error) => warn!(%error, "failed to read file descriptor limit"),
    }
    let listener = TcpListener::bind(LISTENING_ADDRESS)
        .await
        .with_context(|| format!("error binding address: {LISTENING_ADDRESS}"))?;
    info!(listen = %listener.local_addr()?, "proxy started");

    lb_rev_proxy::run(listener, config, Arc::new(balancer)).await;
    Ok(())
}
