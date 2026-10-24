//! Helpers shared by the integration tests.
#![allow(dead_code)]

use std::time::Duration;
use std::{net::SocketAddr, sync::Arc};

use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};

use lb_rev_proxy::{ProxyConfig, RoundRobin};

/// Starts the proxy with the given backends on a free port and returns
/// the proxy's address.
///
/// The proxy runs in the background for the rest
/// of the test.
pub async fn start_proxy(backends: Vec<SocketAddr>) -> SocketAddr {
    let balancer = RoundRobin::new(backends).expect("failed to create balancer");
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("failed to bind proxy listener");
    let proxy_address = listener.local_addr().expect("failed to read proxy address");
    tokio::spawn(lb_rev_proxy::run(
        listener,
        ProxyConfig::default(),
        Arc::new(balancer),
    ));
    proxy_address
}

/// Starts an echo server on a free port and returns its address.
///
/// The server runs in the background for the rest of the test.
pub async fn start_echo_server() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("failed to bind echo server");
    let address = listener
        .local_addr()
        .expect("failed to read echo server address");
    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            tokio::spawn(async move {
                let (mut reader, mut writer) = stream.split();
                let _ = tokio::io::copy(&mut reader, &mut writer).await;
            });
        }
    });
    address
}

/// Starts a server on a free port and returns its address.
///
/// Writes its address to every connection and then closes it.
pub async fn start_identifying_server() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("failed to bind identifying server");
    let address = listener
        .local_addr()
        .expect("failed to read identifying server address");
    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            let _ = stream.write_all(address.to_string().as_bytes()).await;
        }
    });
    address
}

/// Returns an address on which nothing is listening, so connecting to
/// it is refused.
pub async fn dead_backend_address() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("failed to bind temporary listener");
    listener
        .local_addr()
        .expect("failed to read temporary address")
}

/// Connects to the proxy and reads until the connection closes.
///
/// Returns what was received.
pub async fn read_response(proxy_address: SocketAddr) -> String {
    tokio::time::timeout(Duration::from_secs(3), async {
        let mut client = TcpStream::connect(proxy_address)
            .await
            .expect("failed to connect to proxy");

        let mut response = String::new();
        client
            .read_to_string(&mut response)
            .await
            .expect("failed to read from proxy");
        response
    })
    .await
    .expect("timed out waiting for response")
}
