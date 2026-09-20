//! The loopback HTTP client (the family pattern: hand-rolled, http://
//! only, short timeouts). The durability program reads the log's tree
//! head and the services' discovery documents through it.

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

pub fn get(port: u16, path: &str, timeout: Duration) -> Option<HttpText> {
    request("GET", port, path, None, timeout)
}
