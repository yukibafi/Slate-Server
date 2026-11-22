//! Tiny HTTP/1.1 client the desktop app uses to talk to an engine, either its own on this
//! computer or the Mac's over the network (that is how the Windows app "syncs" to the Mac).
//! Plain http on a trusted LAN, key sent in `X-Key`. std only.
use crate::json::{self, J};
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

#[derive(Clone, Debug)]
pub struct Client {
    pub host: String, // "192.168.1.20" or "localhost"
    pub port: u16,
    pub key: String,
    pub timeout: Duration,
}

#[derive(Debug)]
pub enum Err { Unreachable(String), BadReply(String), Http(u16, String) }
impl std::fmt::Display for Err {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Err::Unreachable(m) => write!(f, "Can't reach the computer ({})", m),
            Err::BadReply(m) => write!(f, "Unexpected reply ({})", m),
            Err::Http(401, _) => write!(f, "Wrong key"),
            Err::Http(c, m) => write!(f, "{} (code {})", m, c),
        }
    }
}

impl Client {
    pub fn new(host: &str, port: u16, key: &str) -> Client { Client { host: host.into(), port, key: key.into(), timeout: Duration::from_secs(8) } }
    /// Same computer, different port (a server's own panel), same key.
    pub fn on_port(&self, port: u16) -> Client { Client { port, ..self.clone() } }

    fn request(&self, method: &str, path: &str, body: Option<&[u8]>) -> Result<(u16, Vec<u8>), Err> { self.request_h(method, path, body, &[]) }
    fn request_h(&self, method: &str, path: &str, body: Option<&[u8]>, extra: &[(String, String)]) -> Result<(u16, Vec<u8>), Err> {
        let addr = (self.host.as_str(), self.port).to_socket_addrs().map_err(|e| Err::Unreachable(e.to_string()))?.next().ok_or_else(|| Err::Unreachable("no address".into()))?;
        let mut st = TcpStream::connect_timeout(&addr, self.timeout).map_err(|e| Err::Unreachable(e.to_string()))?;
        let _ = st.set_read_timeout(Some(self.timeout.max(Duration::from_secs(60))));
        let _ = st.set_write_timeout(Some(self.timeout));
        let b = body.unwrap_or(&[]);
        let xh: String = extra.iter().map(|(k, v)| format!("{}: {}\r\n", k.replace(['\r', '\n'], ""), v.replace(['\r', '\n'], ""))).collect();
        let head = format!("{} {} HTTP/1.1\r\nHost: {}:{}\r\nX-Key: {}\r\nContent-Type: application/json\r\n{}Content-Length: {}\r\nConnection: close\r\n\r\n", method, path, self.host, self.port, self.key, xh, b.len());
        st.write_all(head.as_bytes()).and_then(|_| st.write_all(b)).map_err(|e| Err::Unreachable(e.to_string()))?;
        let mut buf = Vec::new();
        st.read_to_end(&mut buf).map_err(|e| Err::Unreachable(e.to_string()))?;
        let split = buf.windows(4).position(|w| w == b"\r\n\r\n").ok_or_else(|| Err::BadReply("no headers".into()))?;
        let head = String::from_utf8_lossy(&buf[..split]).to_string();
        let code: u16 = head.split_whitespace().nth(1).and_then(|c| c.parse().ok()).ok_or_else(|| Err::BadReply("no status".into()))?;
        Ok((code, buf[split + 4..].to_vec()))
    }
    pub fn raw_h(&self, method: &str, path: &str, body: Option<&[u8]>, extra: &[(String, String)]) -> Result<(u16, Vec<u8>), Err> { self.request_h(method, path, body, extra) }
    pub fn raw(&self, method: &str, path: &str, body: Option<&[u8]>) -> Result<(u16, Vec<u8>), Err> { self.request(method, path, body) }

    fn json_call(&self, method: &str, path: &str, body: Option<&J>) -> Result<J, Err> {
        let bs = body.map(|b| b.to_string());
        let (code, data) = self.request(method, path, bs.as_ref().map(|s| s.as_bytes()))?;
        let text = String::from_utf8_lossy(&data).to_string();
        let j = json::parse(&text);
        if code >= 400 {
            let m = j.as_ref().and_then(|j| j.get("msg").or_else(|| j.get("error"))).and_then(|v| v.as_str()).unwrap_or("Request failed").to_string();
            return Err(Err::Http(code, m));
        }
        j.ok_or_else(|| Err::BadReply(text.chars().take(80).collect()))
    }
    pub fn get(&self, path: &str) -> Result<J, Err> { self.json_call("GET", path, None) }
    pub fn post(&self, path: &str, body: &J) -> Result<J, Err> { self.json_call("POST", path, Some(body)) }

    /// Is something answering here, and which role/version? (no key needed)
    pub fn ping(&self) -> Result<J, Err> { self.get("/api/ping") }
}
