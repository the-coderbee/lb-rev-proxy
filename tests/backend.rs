use std::time::Duration;

use tokio::{
    io::AsyncReadExt,
    net::{TcpListener, TcpStream},
};

#[tokio::test]
async fn closes_client_when_backend_is_down() {
    let backend_address = {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("failed to bind temporary listener");
        listener
            .local_addr()
            .expect("failed to read temporary address")
    };
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("failed to bind proxy listener");
    let proxy_address = listener.local_addr().expect("failed to read proxy address");
    tokio::spawn(async move { lb_rev_proxy::run(listener, backend_address).await });
    let bytes_read = tokio::time::timeout(Duration::from_secs(3), async {
        let mut client = TcpStream::connect(proxy_address)
            .await
            .expect("failed to connect to proxy");
        let mut buffer = vec![0u8; 16];
        client
            .read(&mut buffer)
            .await
            .expect("failed to read echoed data")
    })
    .await
    .expect("proxy did not close the connection in time");
    assert_eq!(bytes_read, 0, "proxy should close the client connection");
}
