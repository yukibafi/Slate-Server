//! Minimal HTTP/1.1 server pieces (std only). One request per connection ("Connection: close").
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::time::Duration;

#[derive(Clone)]
pub struct Req {
    pub method: String,
    pub path: String,
    pub query: Vec<(String, String)>,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Req {
    pub fn header(&self, k: &str) -> Option<&str> {
        let k = k.to_ascii_lowercase();
        self.headers.iter().find(|(a, _)| *a == k).map(|(_, v)| v.as_str())
    }
    pub fn q(&self, k: &str) -> Option<&str> { self.query.iter().find(|(a, _)| a == k).map(|(_, v)| v.as_str()) }
}

pub struct Resp {
    pub code: u16,
    pub ctype: &'static str,
    pub body: Vec<u8>,
}

impl Resp {
    pub fn json(code: u16, body: String) -> Resp { Resp { code, ctype: "application/json; charset=utf-8", body: body.into_bytes() } }
    pub fn html(body: &str) -> Resp { Resp { code: 200, ctype: "text/html; charset=utf-8", body: body.as_bytes().to_vec() } }
    pub fn empty(code: u16) -> Resp { Resp { code, ctype: "text/plain", body: Vec::new() } }
}

const MAX_HEADER: usize = 64 * 1024;
const MAX_BODY: usize = 1024 * 1024 * 1024; // panel updates and mod uploads can be large

pub fn read_request(stream: &mut TcpStream) -> Option<Req> {
    stream.set_read_timeout(Some(Duration::from_secs(120))).ok()?;
    let mut r = BufReader::new(stream.try_clone().ok()?);
    let mut line = String::new();
    r.read_line(&mut line).ok()?;
    let mut it = line.trim_end().splitn(3, ' ');
    let method = it.next()?.to_string();
    let target = it.next()?.to_string();
    let mut headers = Vec::new();
    let mut total = line.len();
    loop {
        let mut h = String::new();
        let n = r.read_line(&mut h).ok()?;
        total += n;
        if n == 0 || total > MAX_HEADER { return None; }
        let h = h.trim_end();
        if h.is_empty() { break; }
        if let Some((k, v)) = h.split_once(':') { headers.push((k.trim().to_ascii_lowercase(), v.trim().to_string())); }
    }
    let len: usize = headers.iter().find(|(k, _)| k == "content-length").and_then(|(_, v)| v.parse().ok()).unwrap_or(0);
    if len > MAX_BODY { return None; }
    let mut body = vec![0u8; len];
    r.read_exact(&mut body).ok()?;
    let (path, qs) = match target.split_once('?') { Some((p, q)) => (p.to_string(), q.to_string()), None => (target, String::new()) };
    let query = qs.split('&').filter(|s| !s.is_empty()).map(|kv| {
        let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
        (url_decode(k), url_decode(v))
    }).collect();
    Some(Req { method, path, query, headers, body })
}

pub fn url_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut o = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => { o.push(b' '); i += 1; }
            b'%' if i + 2 < b.len() => {
                match std::str::from_utf8(&b[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok()) { Some(v) => { o.push(v); i += 3; } None => { o.push(b'%'); i += 1; } }
            }
            c => { o.push(c); i += 1; }
        }
    }
    String::from_utf8_lossy(&o).into_owned()
}

pub fn write_response(stream: &mut TcpStream, r: &Resp) {
    let reason = match r.code { 200 => "OK", 204 => "No Content", 400 => "Bad Request", 401 => "Unauthorized", 403 => "Forbidden", 404 => "Not Found", 413 => "Payload Too Large", 500 => "Internal Server Error", 501 => "Not Implemented", _ => "OK" };
    let head = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nReferrer-Policy: no-referrer\r\nConnection: close\r\n\r\n",
        r.code, reason, r.ctype, r.body.len()
    );
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(&r.body);
    let _ = stream.flush();
}
