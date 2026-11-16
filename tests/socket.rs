//! Real loopback tests for the socket capability: a client/server round trip
//! over 127.0.0.1, plus the timeout and refusal error paths. Everything runs
//! with real sockets on every host (macOS included); no fixed port numbers.

use opurom_platform::socket::{SocketError, TcpClient, TcpListener};

/// Round trip: bind an ephemeral listener, echo server in a thread, then
/// connect, send and recv the same bytes back.
#[test]
fn loopback_echo_round_trip() {
    let listener = TcpListener::bind("127.0.0.1", 0).expect("bind ephemeral port");
    let port = listener.local_port().expect("local port");

    let server = std::thread::spawn(move || {
        let mut conn = listener.accept(5000).expect("accept");
        let received = conn.recv(1024, 5000).expect("server recv");
        conn.send(&received, 5000).expect("server send");
        // Keep the connection open until the client's data is echoed back.
        received
    });

    let mut client = TcpClient::connect("127.0.0.1", port, 5000).expect("connect");
    let payload = b"lan chat hello";
    let sent = client.send(payload, 5000).expect("client send");
    assert_eq!(sent, payload.len());
    let echoed = client.recv(1024, 5000).expect("client recv");
    assert_eq!(&echoed, payload, "the server echoes the same bytes");

    client.close().expect("close");
    // A later send on a closed connection is an error, never a silent success.
    let after_close = client.send(b"x", 100);
    assert!(after_close.is_err(), "send after close must fail");

    let served = server.join().expect("server thread");
    assert_eq!(&served, payload);
}

/// A send that spans multiple writes is assembled in order at the peer.
#[test]
fn large_payload_arrives_intact() {
    let listener = TcpListener::bind("127.0.0.1", 0).expect("bind");
    let port = listener.local_port().expect("port");

    let server = std::thread::spawn(move || {
        let mut conn = listener.accept(5000).expect("accept");
        let mut collected = Vec::new();
        while collected.len() < 4096 {
            let chunk = conn.recv(1024, 5000).expect("recv");
            if chunk.is_empty() {
                break;
            }
            collected.extend_from_slice(&chunk);
        }
        collected
    });

    let mut client = TcpClient::connect("127.0.0.1", port, 5000).expect("connect");
    let payload: Vec<u8> = (0..4096).map(|i| (i % 251) as u8).collect();
    client.send(&payload, 5000).expect("send");

    let received = server.join().expect("server thread");
    assert_eq!(received, payload);
    client.close().expect("close");
}

/// accept with no pending connection hits its deadline as a typed Timeout.
#[test]
fn accept_times_out_without_a_client() {
    let listener = TcpListener::bind("127.0.0.1", 0).expect("bind");
    let started = std::time::Instant::now();
    let result = listener.accept(100);
    assert!(
        matches!(result, Err(SocketError::Timeout)),
        "got {result:?}"
    );
    assert!(
        started.elapsed() >= std::time::Duration::from_millis(80),
        "the deadline must actually elapse"
    );
}

/// recv with a silent peer hits its read timeout, never an empty success.
#[test]
fn recv_times_out_without_server_data() {
    let listener = TcpListener::bind("127.0.0.1", 0).expect("bind");
    let port = listener.local_port().expect("port");
    // Hold the accepted side open but send nothing.
    let _server = std::thread::spawn(move || {
        let conn = listener.accept(5000).expect("accept");
        std::thread::sleep(std::time::Duration::from_millis(400));
        drop(conn);
    });

    let mut client = TcpClient::connect("127.0.0.1", port, 5000).expect("connect");
    let result = client.recv(64, 100);
    assert!(
        matches!(result, Err(SocketError::Timeout)),
        "got {result:?}"
    );
    client.close().expect("close");
}

/// Connecting to a port nothing listens on reports Refused.
#[test]
fn connect_to_dead_port_is_refused() {
    // Reserve a port, drop the listener, then connect: nothing answers.
    let port = {
        let held = TcpListener::bind("127.0.0.1", 0).expect("bind");
        held.local_port().expect("port")
    };
    let result = TcpClient::connect("127.0.0.1", port, 3000);
    assert!(
        matches!(result, Err(SocketError::Refused)),
        "got {result:?}"
    );
}

/// The peer closing cleanly yields an empty read, distinct from a timeout.
#[test]
fn peer_close_yields_empty_read_not_timeout() {
    let listener = TcpListener::bind("127.0.0.1", 0).expect("bind");
    let port = listener.local_port().expect("port");
    let server = std::thread::spawn(move || {
        let conn = listener.accept(5000).expect("accept");
        drop(conn); // close immediately after accept
    });

    let mut client = TcpClient::connect("127.0.0.1", port, 5000).expect("connect");
    server.join().expect("server thread");
    let received = client.recv(64, 2000).expect("recv after peer close");
    assert!(received.is_empty(), "a clean peer close reads zero bytes");
    client.close().expect("close");
}
