//! The proxy for accepting connections and handling them.
use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;
use tokio::io::copy_bidirectional;
use tokio::net::{TcpListener, TcpStream};
use tokio::time::{sleep, timeout};
use tracing::{Instrument, debug, error, info, info_span, warn};

use crate::balancer::RoundRobin;
use crate::config::ProxyConfig;

/// Pause before accepting again after running out of resources, giving
/// existing connections time to close and free file descriptors.
const ACCEPT_BACKOFF: Duration = Duration::from_millis(300);

/// Errors that can occur while connecting to a backend.
///
/// No client data has been sent to the backend when one of these occurs,
/// so trying another backend is always safe.
#[derive(Error, Debug)]
enum ConnectError {
    /// The backend did not accept the connection within the connect timeout.
    #[error("timed out connecting to backend {backend}")]
    TimedOut { backend: SocketAddr },

    /// Connecting failed with an operating system error, such as the connection
    /// being refused or the backend being unreachable.
    /// `source` holds the specific cause.
    #[error("failed to connect to backend {backend}")]
    Failed {
        backend: SocketAddr,
        #[source]
        source: io::Error,
    },
}

/// Errors that end a client connection.
#[derive(Debug, Error)]
enum ConnectionError {
    /// Connecting to all the backends failed.
    #[error("all {attempted} backends failed")]
    AllBackendsFailed { attempted: usize },

    /// An I/O operation on an established connection failed.
    #[error(transparent)]
    Io(#[from] io::Error),
}

/// Returns true if an `accept` error means the process or system is
/// out of resources, rather than a problem with one connection. Unix-only.
fn is_resource_exhaustion(error: &io::Error) -> bool {
    matches!(
        error.raw_os_error(),
        Some(libc::EMFILE | libc::ENFILE | libc::ENOMEM | libc::ENOBUFS)
    )
}

/// Opens a TCP connection to `backend`, giving up after connect timeout.
///
/// # Errors
///
/// Returns [`ConnectError::TimedOut`] if the backend does not respond in time,
/// and [`ConnectError::Failed`] if the connection attempt fails with an operating
/// system error.
async fn connect_to_backend(
    backend: SocketAddr,
    connect_timeout: Duration,
) -> Result<TcpStream, ConnectError> {
    match timeout(connect_timeout, TcpStream::connect(backend)).await {
        Ok(Ok(stream)) => Ok(stream),
        Ok(Err(source)) => Err(ConnectError::Failed { backend, source }),
        Err(_elapsed) => Err(ConnectError::TimedOut { backend }),
    }
}

/// Connects the client to the first available backend and forwards
/// traffic in both directions until either side closes.
///
/// Backends are tried in the order given by [`RoundRobin::candidates`],
/// each at most once. A failed attempt is logged and the next candidate
/// is tried.
///
/// # Errors
///
/// Returns [`ConnectionError::AllBackendsFailed`] if no backend could be
/// connected, and [`ConnectionError::Io`] if forwarding fails. A normal
/// close returns `Ok(())`.
async fn handle_client(
    mut client_stream: TcpStream,
    balancer: &RoundRobin,
    config: ProxyConfig,
) -> Result<(), ConnectionError> {
    info!("accepted connection");
    let mut attempted = 0;
    for backend in balancer.candidates() {
        match connect_to_backend(backend, config.connect_timeout).await {
            Ok(mut backend_stream) => {
                info!(%backend, "connected to backend");
                let (bytes_to_backend, bytes_to_client) =
                    copy_bidirectional(&mut client_stream, &mut backend_stream).await?;
                info!(bytes_to_backend, bytes_to_client, "connection ended");
                return Ok(());
            }
            Err(error) => {
                warn!(
                    error = &error as &dyn std::error::Error,
                    "backend connection failed"
                );
                attempted += 1;
            }
        }
    }
    Err(ConnectionError::AllBackendsFailed { attempted })
}

/// Logs a connection error at a level reflecting whether the operator
/// needs to act: backend failures at `warn`, client-side I/O errors at `debug`.
fn log_connection_error(error: &ConnectionError) {
    match error {
        ConnectionError::AllBackendsFailed { attempted } => error!(
            error = error as &dyn std::error::Error,
            "all {attempted} backends failed, no backends available"
        ),
        ConnectionError::Io(_) => debug!(
            error = error as &dyn std::error::Error,
            "connection ended with error"
        ),
    }
}

/// Accepts connections from `listener` and handles each one on its own task.
///
/// This function runs until the process exits and never returns.
///
/// Errors from `accept` never stop the loop. If the process or system runs
/// out of resources (such as file descriptors), the error is logged and the
/// loop pauses briefly (300 ms) before accepting again, so the
/// proxy does not spin while overloaded. Other accept errors are logged
/// and accepting connection continues immediately.
pub async fn run(listener: TcpListener, config: ProxyConfig, balancer: Arc<RoundRobin>) {
    loop {
        match listener.accept().await {
            Ok((stream, client_addr)) => {
                let span = info_span!("connection", client = %client_addr);
                let balancer = Arc::clone(&balancer);
                tokio::spawn(
                    async move {
                        if let Err(error) = handle_client(stream, &balancer, config).await {
                            log_connection_error(&error);
                        }
                    }
                    .instrument(span),
                );
            }
            Err(error) if is_resource_exhaustion(&error) => {
                error!(%error, "accept failed: out of resources, pausing");
                sleep(ACCEPT_BACKOFF).await;
            }
            Err(error) => {
                warn!(%error, "accept failed");
            }
        }
    }
}
