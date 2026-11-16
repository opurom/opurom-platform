//! The socket capability: synchronous TCP over the standard library. One flat
//! contract — [`TcpClient::connect`], [`TcpListener::bind`] + `accept`,
//! `send`, `recv`, `close` — mirrored byte-for-byte by the Android
//! `SocketBridge`, so one .opu program speaks the same primitives on desktop
//! and on a device. Zero dependencies beyond `std`.
//!
//! Determinism: with a fixed peer and bytes, the round-trip yields the same
//! data; only the network decides timing. Every blocking call takes an explicit
//! timeout, and a timeout is a typed [`SocketError::Timeout`], never a silent
//! empty success.

use std::io::{self, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpStream};
use std::time::Duration;

/// Operation timeouts and result types for one TCP endpoint.
pub type Result<T> = std::result::Result<T, SocketError>;

/// Typed socket failures. The Android bridge maps `java.net` exceptions onto
/// these same four variants so callers classify errors identically on both
/// platforms.
#[derive(Debug)]
pub enum SocketError {
    /// The operation exceeded its configured timeout.
    Timeout,
    /// Nothing is listening on the target (ECONNREFUSED).
    Refused,
    /// The peer closed the connection while an operation was pending.
    Closed,
    /// Any other IO failure, carrying the OS message.
    Io(String),
}

impl std::fmt::Display for SocketError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SocketError::Timeout => f.write_str("socket operation timed out"),
            SocketError::Refused => f.write_str("connection refused"),
            SocketError::Closed => f.write_str("connection closed by peer"),
            SocketError::Io(message) => write!(f, "socket io error: {message}"),
        }
    }
}

impl std::error::Error for SocketError {}

fn map_io(error: io::Error) -> SocketError {
    match error.kind() {
        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock => SocketError::Timeout,
        io::ErrorKind::ConnectionRefused => SocketError::Refused,
        io::ErrorKind::UnexpectedEof | io::ErrorKind::ConnectionReset => SocketError::Closed,
        _ => SocketError::Io(error.to_string()),
    }
}

fn map_connect(error: io::Error) -> SocketError {
    if error.kind() == io::ErrorKind::TimedOut || error.kind() == io::ErrorKind::WouldBlock {
        SocketError::Timeout
    } else {
        map_io(error)
    }
}

/// A connected TCP stream: send, receive and close with per-call timeouts.
///
/// Ownership: `close` shuts both directions down; dropping the client closes
/// the socket as a safety net, so a forgotten `close` leaks nothing.
pub struct TcpClient {
    stream: TcpStream,
    closed: bool,
}

impl std::fmt::Debug for TcpClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.stream.peer_addr() {
            Ok(addr) => write!(f, "TcpClient({addr})"),
            Err(_) => f.write_str("TcpClient(closed)"),
        }
    }
}

impl TcpClient {
    /// Connect to `host:port`, resolving the name first, then waiting at most
    /// `timeout` (in milliseconds) for the handshake. `0` means no timeout.
    pub fn connect(host: &str, port: u16, timeout_ms: u64) -> Result<TcpClient> {
        let timeout = if timeout_ms == 0 {
            None
        } else {
            Some(Duration::from_millis(timeout_ms))
        };
        let addrs = std::net::ToSocketAddrs::to_socket_addrs(&(host, port)).map_err(map_io)?;
        let mut last_refused = None;
        for addr in addrs {
            let attempt = match timeout {
                Some(limit) => TcpStream::connect_timeout(&addr, limit),
                None => TcpStream::connect(addr),
            };
            match attempt {
                Ok(stream) => {
                    return Ok(TcpClient {
                        stream,
                        closed: false,
                    })
                }
                Err(error) => {
                    let mapped = map_connect(error);
                    if matches!(mapped, SocketError::Refused) {
                        last_refused = Some(mapped);
                        continue; // try the next resolved address
                    }
                    return Err(mapped);
                }
            }
        }
        Err(last_refused.unwrap_or_else(|| SocketError::Io("no address to connect".into())))
    }

    /// Send every byte, retrying short writes until done or the timeout expires.
    pub fn send(&mut self, bytes: &[u8], timeout_ms: u64) -> Result<usize> {
        if self.closed {
            return Err(SocketError::Closed);
        }
        apply_timeout(&self.stream, timeout_ms)?;
        let mut written = 0;
        while written < bytes.len() {
            written += self.stream.write(&bytes[written..]).map_err(map_io)?;
        }
        Ok(written)
    }

    /// Receive up to `max` bytes. Returns an empty vector only when the peer
    /// closed cleanly; a timeout is `SocketError::Timeout`, never a zero-length
    /// value (macOS reports a timed-out read as WouldBlock, mapped here).
    pub fn recv(&mut self, max: usize, timeout_ms: u64) -> Result<Vec<u8>> {
        if self.closed {
            return Err(SocketError::Closed);
        }
        apply_timeout(&self.stream, timeout_ms)?;
        let mut buffer = vec![0u8; max];
        let read = self.stream.read(&mut buffer).map_err(map_io)?;
        buffer.truncate(read);
        Ok(buffer)
    }

    /// Shut the connection down and release the socket. Idempotent in effect:
    /// a closed stream is harmless, and later sends report `Closed`.
    pub fn close(&mut self) -> Result<()> {
        if !self.closed {
            let _ = self.stream.shutdown(Shutdown::Both);
            self.closed = true;
        }
        Ok(())
    }

    /// The peer's address once connected.
    pub fn peer_addr(&self) -> Result<SocketAddr> {
        self.stream.peer_addr().map_err(map_io)
    }
}

/// A bound, listening TCP socket with an `accept` that honors a timeout.
pub struct TcpListener {
    listener: std::net::TcpListener,
}

impl std::fmt::Debug for TcpListener {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.listener.local_addr() {
            Ok(addr) => write!(f, "TcpListener({addr})"),
            Err(_) => f.write_str("TcpListener(?)"),
        }
    }
}

impl TcpListener {
    /// Bind and listen on `host:port`; port `0` binds an ephemeral port, then
    /// [`TcpListener::local_port`] reports which one was chosen.
    pub fn bind(host: &str, port: u16) -> Result<TcpListener> {
        let listener = std::net::TcpListener::bind((host, port)).map_err(map_io)?;
        Ok(TcpListener { listener })
    }

    /// Wait up to `timeout` (0 = block) for the next connection.
    pub fn accept(&self, timeout_ms: u64) -> Result<TcpClient> {
        if timeout_ms > 0 {
            let _ = self.listener.set_nonblocking(true).map_err(map_io)?;
            let deadline = std::time::Instant::now() + Duration::from_millis(timeout_ms);
            loop {
                match self.listener.accept() {
                    Ok((stream, _)) => {
                        stream.set_nonblocking(false).map_err(map_io)?;
                        return Ok(TcpClient {
                            stream,
                            closed: false,
                        });
                    }
                    Err(ref error) if error.kind() == io::ErrorKind::WouldBlock => {
                        if std::time::Instant::now() >= deadline {
                            return Err(SocketError::Timeout);
                        }
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(error) => return Err(map_io(error)),
                }
            }
        }
        let _ = self.listener.set_nonblocking(false).map_err(map_io)?;
        let (stream, _) = self.listener.accept().map_err(map_io)?;
        Ok(TcpClient {
            stream,
            closed: false,
        })
    }

    /// The local address actually bound, for an ephemeral port.
    pub fn local_port(&self) -> Result<u16> {
        Ok(self.listener.local_addr().map_err(map_io)?.port())
    }
}

fn apply_timeout(stream: &TcpStream, timeout_ms: u64) -> Result<()> {
    let read = if timeout_ms == 0 {
        None
    } else {
        Some(Duration::from_millis(timeout_ms))
    };
    stream
        .set_read_timeout(read)
        .and_then(|()| stream.set_write_timeout(read))
        .map_err(map_io)
}
