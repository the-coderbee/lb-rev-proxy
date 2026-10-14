//! The proxy for accepting connections and handling them.
use std::io;
use std::net::SocketAddr;
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::net::{TcpListener, TcpStream};
use tokio::time::sleep;
use tracing::{debug, error, info, warn};

/// Pause before accepting again after running out of resources, giving
/// existing connections time to close and free file descriptors.
const ACCEPT_BACKOFF: Duration = Duration::from_millis(300);

/// Size of the buffer each connection reads into.
const READ_BUFFER_SIZE: usize = 1024;

/// Returns true if an `accept` error means the process or system is
/// out of resources, rather than a problem with one connection. Unix-only.
fn is_resource_exhaustion(error: &io::Error) -> bool {
    matches!(
        error.raw_os_error(),
        Some(libc::EMFILE | libc::ENFILE | libc::ENOMEM | libc::ENOBUFS)
    )
}

/// Reads from a client connection until the client disconnects.
///
/// Received data is logged and discarded. This is a placeholder
/// until forwarding to a backend is implemented.
///
/// # Errors
///
/// Returns an error if reading from the client fails, for example
/// because the connection was reset. A normal disconnect returns `Ok(())`.
async fn handle_client(mut stream: TcpStream, client_addr: SocketAddr) -> io::Result<()> {
    info!(client = %client_addr, "accepted connection");
    let mut buffer = [0u8; READ_BUFFER_SIZE];

    loop {
        let bytes_read = stream.read(&mut buffer).await?;

        if bytes_read == 0 {
            info!(client = %client_addr, "client closed connection");
            return Ok(());
        }
        debug!(client = %client_addr, bytes = bytes_read, "received data");
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
pub async fn run(listener: TcpListener) {
    loop {
        match listener.accept().await {
            Ok((stream, client_addr)) => {
                tokio::spawn(async move {
                    if let Err(error) = handle_client(stream, client_addr).await {
                        warn!(client = %client_addr, %error, "connection ended with error");
                    }
                });
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
