use create_panel::client::Client;
use create_panel::json::{obj, s};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

struct Guard(Child);
impl Drop for Guard { fn drop(&mut self) { let _ = self.0.kill(); let _ = self.0.wait(); } }

fn free_port() -> u16 { std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port() }

#[test]
fn hub_roundtrip() {
    let dir = std::env::temp_dir().join(format!("cp-test-{}", std::process::id()));
    let data = dir.join("data"); let srv = dir.join("srv1");
    std::fs::create_dir_all(&data).unwrap(); std::fs::create_dir_all(&srv).unwrap();
    std::fs::write(srv.join("start.sh"), "#!/bin/bash\nsleep 1\n").unwrap();
    let port = free_port();
    let child = Command::new(env!("CARGO_BIN_EXE_create-panel"))
        .env("PANEL_DATA", &data).env("PANEL_KEY", "k-test").env("PANEL_PORT", port.to_string()).env("PANEL_CHILD_BASE", free_port().to_string())
        .stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap();
    let _g = Guard(child);
    let c = Client::new("127.0.0.1", port, "k-test");
    for _ in 0..50 { if c.ping().is_ok() { break; } std::thread::sleep(Duration::from_millis(100)); }
    let p = c.ping().expect("engine answers");
    assert_eq!(p.get("role").and_then(|v| v.as_str()), Some("hub"));
    // wrong key is rejected
    assert!(Client::new("127.0.0.1", port, "nope").get("/api/hub/state").is_err());
    c.post("/api/hub/add", &obj(vec![("dir", s(&srv.to_string_lossy())), ("name", s("Test"))])).expect("add");
    let st = c.get("/api/hub/state").expect("state");
    let servers = st.get("servers").expect("servers");
    assert!(servers.to_string().contains("Test"));
    // theme is shared and validated
    c.post("/api/theme", &obj(vec![("palette", s("rose"))])).expect("theme");
    assert!(c.get("/api/theme").unwrap().to_string().contains("rose"));
    // unreachable reports cleanly
    assert!(Client::new("127.0.0.1", free_port(), "x").ping().is_err());
}

fn raw(port: u16, method: &str, path: &str, key: &str, body: &str) -> (u16, String) {
    use std::io::{Read, Write};
    let mut st = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    st.set_read_timeout(Some(Duration::from_secs(40))).unwrap();
    write!(st, "{method} {path} HTTP/1.1\r\nHost: x\r\nX-Key: {key}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
    let mut out = String::new(); let _ = st.read_to_string(&mut out);
    let code = out.split_whitespace().nth(1).and_then(|c| c.parse().ok()).unwrap_or(0);
    (code, out.split("\r\n\r\n").nth(1).unwrap_or("").to_string())
}

#[test]
fn discord_bot_key_can_only_list_and_start() {
    let dir = std::env::temp_dir().join(format!("cp-bot-{}", std::process::id()));
    let data = dir.join("data"); let home = dir.join("home"); let srv = dir.join("srv1");
    for d in [&data, &home, &srv] { std::fs::create_dir_all(d).unwrap(); }
    std::fs::write(srv.join("start.sh"), "#!/bin/bash\nsleep 1\n").unwrap();
    let port = free_port();
    let child = Command::new(env!("CARGO_BIN_EXE_create-panel"))
        .env("HOME", &home).env("USERPROFILE", &home).env("PANEL_DATA", &data).env("PANEL_KEY", "k-test").env("PANEL_PORT", port.to_string()).env("PANEL_CHILD_BASE", free_port().to_string())
        .stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap();
    let _g = Guard(child);
    let c = Client::new("127.0.0.1", port, "k-test");
    for _ in 0..50 { if c.ping().is_ok() { break; } std::thread::sleep(Duration::from_millis(100)); }
    c.post("/api/hub/add", &obj(vec![("dir", s(&srv.to_string_lossy())), ("name", s("Test"))])).expect("add");
    let bk = std::fs::read_to_string(home.join(".create-panel-botkey")).expect("bot key file exists").trim().to_string();
    assert!(bk.len() >= 32 && bk != "k-test");
    // the bot key can list servers
    let (code, body) = raw(port, "GET", "/api/hub/bot/servers", &bk, "");
    assert_eq!(code, 200, "{body}"); assert!(body.contains("Test") && body.contains("\"enabled\":true"), "{body}");
    // ...but nothing else: not state, not actions (so no stop), not settings, not the server proxy, not the bot's own token
    for (m, p) in [("GET", "/api/hub/state"), ("POST", "/api/hub/action"), ("POST", "/api/hub/discord/token"), ("POST", "/api/hub/discord/config"), ("GET", "/api/hub/key"), ("GET", "/api/s/x/api/status"), ("GET", "/api/ping")] {
        assert_eq!(raw(port, m, p, &bk, "{}").0, 401, "{m} {p} must reject the bot key");
    }
    // a wrong key is rejected on the bot routes too
    assert_eq!(raw(port, "GET", "/api/hub/bot/servers", "nope", "").0, 401);
    assert_eq!(raw(port, "POST", "/api/hub/bot/start", "nope", "{}").0, 401);
    // starting an unknown id is refused cleanly
    let (code, body) = raw(port, "POST", "/api/hub/bot/start", &bk, "{\"id\":\"nope\"}");
    assert_eq!(code, 200); assert!(body.contains("\"ok\":false"), "{body}");
    // a real start goes through (200, not 401/403)
    let id = { let st = c.get("/api/hub/state").unwrap(); st.get("servers").and_then(|v| match v { create_panel::json::J::Arr(a) => a.first().cloned(), _ => None }).and_then(|e| e.get("id").and_then(|i| i.as_str().map(|x| x.to_string()))).expect("id") };
    let (code, body) = raw(port, "POST", "/api/hub/bot/start", &bk, &format!("{{\"id\":\"{id}\"}}"));
    assert_eq!(code, 200, "{body}"); assert!(body.contains("\"state\""), "{body}");
    // switching /start off in the panel makes the engine refuse it too
    c.post("/api/hub/discord/config", &obj(vec![("startcmd", create_panel::json::J::Bool(false))])).expect("config");
    let (code, _) = raw(port, "POST", "/api/hub/bot/start", &bk, &format!("{{\"id\":\"{id}\"}}"));
    assert_eq!(code, 403);
    assert!(raw(port, "GET", "/api/hub/bot/servers", &bk, "").1.contains("\"enabled\":false"));
}
