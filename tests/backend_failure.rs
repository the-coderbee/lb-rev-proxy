use std::{net::SocketAddr, time::Duration};

use tokio::{io::AsyncReadExt, net::TcpStream};

mod common;

#[tokio::test]
async fn closes_client_when_all_backends_are_down() {
    let mut backends: Vec<SocketAddr> = Vec::with_capacity(3);
    for _ in 0..3 {
        backends.push(common::dead_backend_address().await);
    }
    let proxy_address = common::start_proxy(backends).await;

    let bytes_read = tokio::time::timeout(Duration::from_secs(3), async {
        let mut client = TcpStream::connect(proxy_address)
            .await
            .expect("failed to connect to proxy");
        let mut buffer = vec![0u8; 16];
        client
            .read(&mut buffer)
            .await
            .expect("failed to read from proxy")
    })
    .await
    .expect("proxy did not close the connection in time");
    assert_eq!(bytes_read, 0, "proxy should close the client connection");
}

#[tokio::test]
async fn retries_next_backend_when_first_is_down() {
    let healthy_a = common::start_identifying_server().await;
    let healthy_b = common::start_identifying_server().await;
    let dead = common::dead_backend_address().await;

    let backends = vec![dead, healthy_a, healthy_b];

    let proxy_address = common::start_proxy(backends).await;

    let response = common::read_response(proxy_address).await;

    assert_eq!(
        response,
        healthy_a.to_string(),
        "first connection should be retried on the next backend"
    );
}
