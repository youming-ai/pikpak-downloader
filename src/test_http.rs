//! Primitives shared by this crate's hand-rolled HTTP mocks.
//!
//! Each test module keeps its own server — they route differently — but every
//! one of them reads a request and writes a response the same way, so those
//! two halves live here. The mocks speak just enough HTTP for reqwest: one
//! request per connection, closed by the server after each response.

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

/// The token response every mock auth endpoint answers with.
///
/// The rotation tests in `auth` deliberately answer with *per-request* tokens
/// instead, so they keep their own literals.
pub(crate) const TOKEN_JSON: &str =
    r#"{"access_token":"access","refresh_token":"rotated","expires_in":7200,"sub":"user"}"#;

/// One parsed HTTP request: just the parts the mocks look at.
pub(crate) struct Request {
    /// Request target: path plus query.
    pub target: String,
    /// Body, lossily decoded as UTF-8 (empty for the GETs the drive API uses).
    pub body: String,
}

/// Read one HTTP request: the headers, then the body `Content-Length` declares.
///
/// Every mock client sends small bodies, so buffering is fine. Reading by
/// `Content-Length` (rather than "whatever arrived in one segment") is what
/// makes POST bodies parse reliably.
pub(crate) async fn read_request(sock: &mut TcpStream) -> Request {
    let mut raw = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        match sock.read(&mut buf).await {
            Ok(0) => break,
            Ok(n) => {
                raw.extend_from_slice(&buf[..n]);
                if let Some(split) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
                    let head = String::from_utf8_lossy(&raw[..split]).to_ascii_lowercase();
                    let declared = head
                        .lines()
                        .find_map(|line| {
                            line.strip_prefix("content-length:")
                                .map(|value| value.trim().to_string())
                        })
                        .and_then(|value| value.parse::<usize>().ok())
                        .unwrap_or(0);
                    if raw.len() >= split + 4 + declared {
                        break;
                    }
                }
            }
            Err(_) => break,
        }
    }
    let text = String::from_utf8_lossy(&raw).into_owned();
    let target = text.split_whitespace().nth(1).unwrap_or("/").to_string();
    let body = text.split("\r\n\r\n").nth(1).unwrap_or("").to_string();
    Request { target, body }
}

/// Write a JSON response with `Content-Length`, closing the connection.
pub(crate) async fn respond_json(sock: &mut TcpStream, status: u16, reason: &str, body: &str) {
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = sock.write_all(head.as_bytes()).await;
    let _ = sock.write_all(body.as_bytes()).await;
    let _ = sock.flush().await;
}
