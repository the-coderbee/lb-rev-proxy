use std::{net::SocketAddr, time::Duration};

use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};

mod common;

#[tokio::test]
async fn forwards_data_to_backend_and_back() {
    let backends: Vec<SocketAddr> = vec![common::start_echo_server().await];
    let proxy_address = common::start_proxy(backends).await;
    let message = b"hello through the proxy";
    let echoed = tokio::time::timeout(Duration::from_secs(3), async {
        let mut client = TcpStream::connect(proxy_address)
            .await
            .expect("failed to connect to proxy");
        client
            .write_all(message)
            .await
            .expect("failed to write to proxy");

        let mut buffer = vec![0u8; message.len()];
        client
            .read_exact(&mut buffer)
            .await
            .expect("failed to read echoed data");
        buffer
    })
    .await
    .expect("timed out waiting for echoed data");
    assert_eq!(echoed, message);
}

#[tokio::test]
async fn connections_rotate_across_backends() {
    let mut backends: Vec<SocketAddr> = Vec::with_capacity(3);
    for _ in 0..3 {
        backends.push(common::start_identifying_server().await);
    }
    let proxy_address = common::start_proxy(backends.clone()).await;

    for expected in backends.iter().cycle().take(4) {
        let response = common::read_response(proxy_address).await;

        assert_eq!(response, expected.to_string());
    }
}
