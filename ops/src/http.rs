//! The loopback HTTP client (the family pattern: hand-rolled,
//! http:// only, short timeouts). Shared by the orchestrator and the
//! walkthrough seed.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

pub struct HttpText {
    pub status: u16,
    pub body: String,
}

pub fn request(
    method: &str,
    port: u16,
    path: &str,
    body: Option<&str>,
    timeout: Duration,
) -> Option<HttpText> {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).ok()?;
    stream.set_read_timeout(Some(timeout)).ok()?;
    stream.set_write_timeout(Some(timeout)).ok()?;
    let request = format!(
        "{} {} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{}",
        method,
        path,
        body.map(str::len).unwrap_or(0),
        body.unwrap_or("")
    );
    stream.write_all(request.as_bytes()).ok()?;
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).ok()?;
    let text = String::from_utf8_lossy(&raw).into_owned();
    let status = text.split_whitespace().nth(1)?.parse().ok()?;
    let body = text.split("\r\n\r\n").nth(1).unwrap_or("").to_string();
    Some(HttpText { status, body })
}

/// `http://127.0.0.1:port[/prefix]` → (port, path prefix). The one
/// deployment addresses modules by prefixed path on one origin; the
/// ten-service stack passed bare ports, which parse the same way.
fn split_url(url: &str) -> (u16, String) {
    let rest = url.strip_prefix("http://").unwrap_or(url);
    let (hostport, prefix) = match rest.split_once('/') {
        Some((hp, p)) => (hp, format!("/{p}")),
        None => (rest, String::new()),
    };
    let port = hostport
        .rsplit(':')
        .next()
        .and_then(|p| p.parse().ok())
        .unwrap_or(80);
    (port, prefix)
}

pub fn request_url(
    method: &str,
    url: &str,
    path: &str,
    body: Option<&str>,
    timeout: Duration,
) -> Option<HttpText> {
    let (port, prefix) = split_url(url);
    request(method, port, &format!("{prefix}{path}"), body, timeout)
}

pub fn get_url(url: &str, path: &str, timeout: Duration) -> Option<HttpText> {
    request_url("GET", url, path, None, timeout)
}

pub fn post_url(url: &str, path: &str, body: &str, timeout: Duration) -> Option<HttpText> {
    request_url("POST", url, path, Some(body), timeout)
}
