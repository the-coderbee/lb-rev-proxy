//! A TCP reverse proxy and load balancer.
//!
//! Bind a [`TcpListener`](tokio::net::TcpListener) and pass it to [`run`],
//! which accepts and handles connections until the process exits.
mod proxy;
pub use proxy::run;

mod balancer;
pub use balancer::RoundRobin;

mod config;
pub use config::ProxyConfig;

mod backend;
mod health;
pub use health::{InvalidThresholds, Thresholds};
