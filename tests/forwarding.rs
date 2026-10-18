use std::{net::SocketAddr, time::Duration};

use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};

/// Starts an echo server on a free port and returns its address.
/// The server runs in the background for the rest of the test.
async fn start_echo_server() -> SocketAddr {
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

#[tokio::test]
async fn forwards_data_to_backend_and_back() {
    let backend_address = start_echo_server().await;
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("failed to bind proxy listener");
    let proxy_address = listener.local_addr().expect("failed to read proxy address");
    tokio::spawn(async move {
        lb_rev_proxy::run(listener, backend_address).await;
    });
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
