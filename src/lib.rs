//! A TCP reverse proxy and load balancer.
//!
//! Bind a [`TcpListener`] and pass it to [`run`],
//! which accepts and handles connections until the process exits.
mod proxy;
pub use proxy::run;

mod balancer;
pub mod config;
