//! The hub: a list of servers, one panel process per server, and the homepage that shows them.
//! Each server panel is this same program started with PANEL_ROLE=server; the hub starts them,
//! watches them, and passes start/stop/restart on. Panels keep running if the hub restarts.
use crate::json::{self, n, obj, s, J};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

#[derive(Clone)]
pub struct Entry { pub id: String, pub name: String, pub dir: String, pub port: u16, pub settings: String, pub created: f64 }

#[derive(Clone, Default)]
struct Summary { state: String, players: Vec<String>, mem_mb: Option<i64>, uptime: Option<i64>, mc_port: Option<i64>, motd: String, has_start: bool, version: String }

pub struct Hub {
    pub data_dir: PathBuf,
    entries: Mutex<Vec<Entry>>,
    summary: Mutex<HashMap<String, Summary>>,
    procs: Mutex<HashMap<String, Child>>,
    quit: Mutex<HashMap<String, bool>>,
    why: Mutex<HashMap<String, String>>,
    started: Mutex<HashMap<String, Instant>>,
    key: String,
    version: String,
}

/// One tiny HTTP request to a local panel. Returns (status, body).
pub fn local_req(port: u16, method: &str, path: &str, key: &str, body: Option<&str>, secs: u64) -> Option<(u16, String)> {
    let mut st = TcpStream::connect_timeout(&format!("127.0.0.1:{port}").parse().ok()?, Duration::from_millis(600)).ok()?;
    st.set_read_timeout(Some(Duration::from_secs(secs))).ok()?;
    let b = body.unwrap_or("");
    let req = format!("{method} {path} HTTP/1.1\r\nHost: localhost\r\nX-Key: {key}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{b}", b.len());
    st.write_all(req.as_bytes()).ok()?;
    let mut out = Vec::new();
    st.read_to_end(&mut out).ok()?;
    let t = String::from_utf8_lossy(&out).to_string();
    let code: u16 = t.get(9..12)?.parse().ok()?;
    let body = t.split_once("\r\n\r\n").map(|x| x.1.to_string()).unwrap_or_default();
    Some((code, body))
}

impl Hub {
    pub fn new(data_dir: PathBuf, key: String, version: String) -> Hub {
        let _ = std::fs::create_dir_all(&data_dir);
        let h = Hub { data_dir, entries: Mutex::new(Vec::new()), summary: Mutex::new(HashMap::new()), procs: Mutex::new(HashMap::new()), quit: Mutex::new(HashMap::new()), why: Mutex::new(HashMap::new()), started: Mutex::new(HashMap::new()), key, version };
        h.load();
        h
    }

    fn file(&self) -> PathBuf { self.data_dir.join("servers.json") }

    fn load(&self) {
        let txt = std::fs::read_to_string(self.file()).unwrap_or_default();
        let mut v = Vec::new();
        if let Some(J::Obj(_)) = json::parse(&txt) {
            if let Some(J::Arr(a)) = json::parse(&txt).unwrap().get("servers") {
                for e in a {
                    let g = |k: &str| e.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
                    let port = match e.get("port") { Some(J::Num(p)) => *p as u16, _ => 0 };
                    if !g("id").is_empty() && port != 0 { v.push(Entry { id: g("id"), name: g("name"), dir: g("dir"), port, settings: g("settings"), created: match e.get("created") { Some(J::Num(c)) => *c, _ => 0.0 } }); }
                }
            }
        }
        *self.entries.lock().unwrap() = v;
    }

    fn save(&self) {
        let list = self.entries.lock().unwrap().iter().map(|e| { let mut f = vec![("id", s(&e.id)), ("name", s(&e.name)), ("dir", s(&e.dir)), ("port", n(e.port as i64))]; if !e.settings.is_empty() { f.push(("settings", s(&e.settings))); } if e.created > 0.0 { f.push(("created", J::Num(e.created))); } obj(f) }).collect();
        let tmp = self.data_dir.join("servers.json.tmp");
        if std::fs::write(&tmp, obj(vec![("servers", J::Arr(list))]).to_string()).is_ok() { let _ = std::fs::rename(&tmp, self.file()); }
    }

    /// Bring in servers from another registry (the old Python panel). Keeps id, name and port when they are free,
    /// otherwise picks new ones. Returns (added ids, skipped because the folder was already listed).
    pub fn import_entries(&self, list: Vec<Entry>) -> (Vec<String>, usize) {
        let mut es = self.entries.lock().unwrap();
        let (mut added, mut skipped) = (vec![], 0usize);
        for mut e in list {
            if es.iter().any(|x| x.dir == e.dir) { skipped += 1; continue; }
            if es.iter().any(|x| x.id == e.id) { let mut i = 1; while es.iter().any(|x| x.id == format!("s{}", i)) { i += 1; } e.id = format!("s{}", i); }
            if es.iter().any(|x| x.port == e.port) { let mut p = e.port.max(8101); while es.iter().any(|x| x.port == p) { p += 1; } e.port = p; }
            added.push(e.id.clone());
            es.push(e);
        }
        drop(es);
        self.save();
        (added, skipped)
    }

    pub fn get(&self, id: &str) -> Option<Entry> { self.entries.lock().unwrap().iter().find(|e| e.id == id).cloned() }
    pub fn list(&self) -> Vec<Entry> { self.entries.lock().unwrap().clone() }
    pub fn rename(&self, id: &str, name: &str) -> bool { let ok = { let mut g = self.entries.lock().unwrap(); match g.iter_mut().find(|e| e.id == id) { Some(e) => { e.name = name.to_string(); true } None => false } }; if ok { self.save(); } ok }
    pub fn has_players(&self, id: &str) -> bool { self.summary.lock().unwrap().get(id).map(|x| !x.players.is_empty()).unwrap_or(false) }
    pub fn state_of(&self, id: &str) -> String { self.summary.lock().unwrap().get(id).map(|x| x.state.clone()).unwrap_or_else(|| "stopped".into()) }
    pub fn push_entry(&self, e: Entry) { self.entries.lock().unwrap().push(e.clone()); self.save(); self.spawn(&e); }
    pub fn taken_ports(&self) -> (Vec<u16>, Vec<i64>) { let g = self.entries.lock().unwrap(); (g.iter().map(|e| e.port).collect(), g.iter().map(|e| props_port(&e.dir)).collect()) }
    pub fn key(&self) -> &str { &self.key }
    /// Asks a server panel to shut down (it stops Minecraft safely first) and waits for it to go away.
    pub fn stop_child(&self, id: &str, wait_s: u64) {
        let Some(e) = self.get(id) else { return };
        self.quit.lock().unwrap().insert(id.to_string(), true);
        let _ = local_req(e.port, "POST", "/api/quit", &self.key, Some("{}"), 5);
        let end = Instant::now() + Duration::from_secs(wait_s);
        while Instant::now() < end && self.alive(&e).is_some() { std::thread::sleep(Duration::from_millis(300)); }
        self.procs.lock().unwrap().remove(id);
    }
    pub fn forget(&self, id: &str) { self.remove_ids(&[id.to_string()]); self.summary.lock().unwrap().remove(id); self.procs.lock().unwrap().remove(id); }
    pub fn remove_ids(&self, ids: &[String]) { self.entries.lock().unwrap().retain(|e| !ids.contains(&e.id)); self.save(); }

    pub fn add(&self, dir: &str, name: &str) -> Result<Entry, String> {
        let d = PathBuf::from(dir.trim());
        if !d.is_dir() { return Err("That folder doesn't exist.".into()); }
        let dir = d.canonicalize().map_err(|e| e.to_string())?.display().to_string();
        let mut es = self.entries.lock().unwrap();
        if es.iter().any(|e| e.dir == dir) { return Err("That folder is already in the list.".into()); }
        let mut port = std::env::var("PANEL_CHILD_BASE").ok().and_then(|v| v.parse::<u16>().ok()).unwrap_or(8101u16);
        while es.iter().any(|e| e.port == port) { port += 1; }
        let mut i = es.len() + 1;
        while es.iter().any(|e| e.id == format!("s{i}")) { i += 1; }
        let nm = if name.trim().is_empty() { d.file_name().map(|x| x.to_string_lossy().to_string()).unwrap_or_else(|| "Server".into()) } else { name.trim().to_string() };
        let e = Entry { id: format!("s{i}"), name: nm, dir, port, settings: String::new(), created: crate::sys::now() };
        es.push(e.clone());
        drop(es);
        self.save();
        self.spawn(&e);
        Ok(e)
    }

    fn log_path(&self, id: &str) -> PathBuf { self.data_dir.join("logs").join(format!("panel-{id}.log")) }
    fn log_tail(&self, id: &str) -> String {
        let t = std::fs::read_to_string(self.log_path(id)).unwrap_or_default();
        let l: Vec<&str> = t.lines().filter(|x| !x.trim().is_empty()).collect();
        l[l.len().saturating_sub(3)..].join(" | ").chars().take(300).collect()
    }
    fn port_free(port: u16) -> bool { std::net::TcpListener::bind(("0.0.0.0", port)).is_ok() }
    /// Gives a server's panel a different internal port (something else is using the old one). Returns the updated entry.
    fn move_port(&self, id: &str) -> Option<Entry> {
        let mut es = self.entries.lock().unwrap();
        let used: Vec<u16> = es.iter().map(|x| x.port).collect();
        let mut p = std::env::var("PANEL_CHILD_BASE").ok().and_then(|v| v.parse::<u16>().ok()).unwrap_or(8101u16);
        while used.contains(&p) || !Self::port_free(p) { p = p.checked_add(1)?; }
        let e = es.iter_mut().find(|x| x.id == id)?;
        let old = e.port; e.port = p;
        let out = e.clone();
        drop(es);
        self.save();
        self.set_why(id, &format!("Something else was using this panel's port {old}, so it was moved to {p}."));
        Some(out)
    }
    pub fn why_of(&self, id: &str) -> String { self.why.lock().unwrap().get(id).cloned().unwrap_or_default() }
    fn set_why(&self, id: &str, w: &str) { let mut m = self.why.lock().unwrap(); if w.is_empty() { m.remove(id); } else { m.insert(id.to_string(), w.to_string()); } }

    fn alive(&self, e: &Entry) -> Option<String> {
        let (c, b) = local_req(e.port, "GET", "/api/ping", &self.key, None, 2)?;
        if c != 200 { return None; }
        json::parse(&b)?.get("version")?.as_str().map(|x| x.to_string())
    }

    /// Starts the panel for one server unless one is already answering on its port (then it is adopted).
    pub fn spawn(&self, e: &Entry) -> (bool, String) {
        if self.alive(e).is_some() { return (true, "Already running.".into()); }
        let moved;
        let e = if Self::port_free(e.port) { e } else { match self.move_port(&e.id) { Some(n) => { moved = n; &moved } None => e } };
        self.started.lock().unwrap().insert(e.id.clone(), Instant::now());
        if !PathBuf::from(&e.dir).is_dir() { return (false, "That server's folder is missing.".into()); }
        let exe = match std::env::current_exe() { Ok(x) => x, Err(x) => return (false, x.to_string()) };
        self.quit.lock().unwrap().insert(e.id.clone(), false);
        let (out, err) = match std::fs::create_dir_all(self.log_path(&e.id).parent().unwrap_or(std::path::Path::new("."))).ok().and_then(|_| std::fs::File::create(self.log_path(&e.id)).ok()) {
            Some(f) => { let f2 = f.try_clone().ok(); (Stdio::from(f), f2.map(Stdio::from).unwrap_or_else(Stdio::null)) }
            None => (Stdio::null(), Stdio::null()),
        };
        match Command::new(exe).env("PANEL_ROLE", "server").env("PANEL_SETTINGS_FILE", if e.settings.is_empty() { PathBuf::from(&e.dir).join(".panel-settings.json").display().to_string() } else { e.settings.clone() }).env("SERVER_DIR", &e.dir).env("PANEL_PORT", e.port.to_string()).env("PANEL_KEY", &self.key)
            .stdin(Stdio::null()).stdout(out).stderr(err).spawn() {
            Ok(c) => { self.procs.lock().unwrap().insert(e.id.clone(), c); (true, "Panel starting...".into()) }
            Err(x) => (false, format!("Couldn't start that panel: {x}")),
        }
    }

    pub fn action(&self, id: &str, action: &str) -> (bool, String) {
        let e = match self.entries.lock().unwrap().iter().find(|e| e.id == id).cloned() { Some(e) => e, None => return (false, "That server isn't in the list any more.".into()) };
        if action == "panel-start" { return self.spawn(&e); }
        if !matches!(action, "start" | "stop" | "restart") { return (false, "Unknown action.".into()); }
        if self.alive(&e).is_none() { return (false, "This server's panel is stopped. Press \"Start panel\" first.".into()); }
        if action == "start" {
            let mine = self.mc_port(&e);
            for o in self.entries.lock().unwrap().iter().filter(|o| o.id != e.id) {
                let sm = self.summary.lock().unwrap().get(&o.id).cloned().unwrap_or_default();
                if matches!(sm.state.as_str(), "starting" | "online" | "stopping") && sm.mc_port.unwrap_or_else(|| props_port(&o.dir)) == mine {
                    return (false, format!("{} is using port {} right now. Stop it first, or give this server its own port in its settings.", o.name, mine));
                }
            }
        }
        match local_req(e.port, "POST", &format!("/api/{action}"), &self.key, Some("{}"), 15) {
            Some((_, b)) => { let j = json::parse(&b); (matches!(j.as_ref().and_then(|j| j.get("ok")), Some(J::Bool(true))), j.and_then(|j| j.get("msg").and_then(|m| m.as_str().map(|x| x.to_string()))).unwrap_or_default()) }
            None => (false, "Couldn't reach that server's panel. Give it a few seconds.".into()),
        }
    }

    fn mc_port(&self, e: &Entry) -> i64 { self.summary.lock().unwrap().get(&e.id).and_then(|s| s.mc_port).unwrap_or_else(|| props_port(&e.dir)) }

    /// Runs forever on its own thread: keeps every server's panel alive and its summary fresh.
    pub fn supervise(&self) {
        let mut fails: HashMap<String, Vec<Instant>> = HashMap::new();
        loop {
            let entries = self.entries.lock().unwrap().clone();
            for e in &entries {
                match self.alive(e) {
                    Some(v) if v != self.version => {
                        // an older panel left over from before an update: let it save and exit, then start the new one
                        let r = local_req(e.port, "POST", "/api/quit", &self.key, Some("{}"), 5);
                        if !matches!(r, Some((200, _))) { self.set_why(&e.id, &format!("An older panel (v{v}) is still holding port {} and won't close. Quit the app fully and open it again.", e.port)); }
                    }
                    Some(_) => {
                        self.set_why(&e.id, "");
                        if let Some((200, b)) = local_req(e.port, "GET", "/api/status?since=2000000000", &self.key, None, 4) {
                            if let Some(j) = json::parse(&b) {
                                let num = |k: &str| match j.get(k) { Some(J::Num(x)) => Some(*x as i64), _ => None };
                                let sm = Summary {
                                    state: j.get("state").and_then(|x| x.as_str()).unwrap_or("stopped").to_string(),
                                    players: match j.get("players") { Some(J::Arr(a)) => a.iter().filter_map(|x| x.as_str().map(|t| t.to_string())).collect(), _ => vec![] },
                                    mem_mb: num("mem_mb"), uptime: num("uptime"), mc_port: num("port"),
                                    motd: j.get("motd").and_then(|x| x.as_str()).unwrap_or("").to_string(),
                                    has_start: matches!(j.get("has_start"), Some(J::Bool(true))), version: self.version.clone(),
                                };
                                self.summary.lock().unwrap().insert(e.id.clone(), sm);
                            }
                        }
                    }
                    None => {
                        self.summary.lock().unwrap().remove(&e.id);
                        let gone = match self.procs.lock().unwrap().get_mut(&e.id) { Some(c) => matches!(c.try_wait(), Ok(Some(_))), None => true };
                        if !gone {
                            let stuck = self.started.lock().unwrap().get(&e.id).map(|t| t.elapsed() > Duration::from_secs(15)).unwrap_or(false);
                            if stuck {
                                if let Some(mut c) = self.procs.lock().unwrap().remove(&e.id) { let _ = c.kill(); let _ = c.wait(); }
                                self.move_port(&e.id);
                                continue;
                            }
                        }
                        let wanted = !self.quit.lock().unwrap().get(&e.id).copied().unwrap_or(false);
                        if gone && wanted {
                            let f = fails.entry(e.id.clone()).or_default();
                            f.retain(|t| t.elapsed() < Duration::from_secs(60));
                            if f.len() < 3 { f.push(Instant::now()); self.spawn(e); }
                            else { let t = self.log_tail(&e.id); self.set_why(&e.id, &if t.is_empty() { "Its panel keeps closing right after it starts (no message). Details are in the panel log.".to_string() } else { format!("Its panel keeps closing: {t}") }); }
                        }
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(2500));
        }
    }

    pub fn state(&self, ips: Vec<String>, boot: &str) -> J {
        let entries = self.entries.lock().unwrap().clone();
        let sums = self.summary.lock().unwrap().clone();
        let servers: Vec<J> = entries.iter().map(|e| {
            let sm = sums.get(&e.id);
            let alive = sm.is_some();
            let state = match sm { Some(x) => x.state.clone(), None => if self.procs.lock().unwrap().contains_key(&e.id) { "connecting".into() } else { "panel_off".into() } };
            let mc_port = sm.and_then(|x| x.mc_port).unwrap_or_else(|| props_port(&e.dir));
            let clash: Vec<J> = entries.iter().filter(|o| o.id != e.id && sums.get(&o.id).and_then(|x| x.mc_port).unwrap_or_else(|| props_port(&o.dir)) == mc_port).map(|o| s(&o.name)).collect();
            let opt = |v: Option<i64>| v.map(n).unwrap_or(J::Null);
            obj(vec![
                ("id", s(&e.id)), ("name", s(&e.name)), ("port", n(e.port as i64)), ("dir", s(&e.dir)), ("mc_port", n(mc_port)), ("state", s(&state)),
                ("players", J::Arr(sm.map(|x| x.players.iter().map(|p| s(p)).collect()).unwrap_or_default())),
                ("mem_mb", opt(sm.and_then(|x| x.mem_mb))), ("max_mb", J::Null), ("uptime", opt(sm.and_then(|x| x.uptime))),
                ("env", J::Null), ("updates", n(0)), ("locked", J::Bool(false)),
                ("has_start", J::Bool(if alive { sm.map(|x| x.has_start).unwrap_or(false) } else { PathBuf::from(&e.dir).join("start.sh").is_file() })),
                ("motd", s(sm.map(|x| x.motd.as_str()).unwrap_or(""))), ("gave_up", J::Bool(false)), ("can_delete", J::Bool(crate::hubc::in_root(&e.dir))),
                ("why", s(&self.why.lock().unwrap().get(&e.id).cloned().unwrap_or_default())), ("log", s(&self.log_path(&e.id).display().to_string())), ("clash", J::Arr(clash)), ("exists", J::Bool(PathBuf::from(&e.dir).is_dir())),
            ])
        }).collect();
        obj(vec![("servers", J::Arr(servers)), ("hub", obj(vec![
            ("version", s(&self.version)), ("ips", J::Arr(ips.iter().map(|x| s(x)).collect())), ("host", { let h = local_host(); if h.is_empty() { J::Null } else { s(&h) } }), ("ram_gb", J::Null), ("login_item", J::Bool(false)),
            ("background", J::Bool(false)), ("mac", J::Bool(cfg!(target_os = "macos"))), ("root", s(&self.data_dir.display().to_string())), ("admin", J::Bool(true)),
            ("admin_default", J::Bool(false)), ("boot", s(boot)), ("public_ip", J::Null), ("public_ip_state", s("off")),
        ]))])
    }
}

pub fn props_port(dir: &str) -> i64 {
    std::fs::read_to_string(PathBuf::from(dir).join("server.properties")).unwrap_or_default().lines()
        .find_map(|l| l.strip_prefix("server-port=").and_then(|v| v.trim().parse().ok())).unwrap_or(25565)
}

pub fn props_get(dir: &str, key: &str) -> String {
    let pre = format!("{key}=");
    std::fs::read_to_string(PathBuf::from(dir).join("server.properties")).unwrap_or_default().lines().find_map(|l| l.strip_prefix(pre.as_str()).map(|v| v.trim().to_string())).unwrap_or_default()
}

/// A steady name for this computer (survives IP changes). "name.local" on a Mac, the plain host name elsewhere.
pub fn local_host() -> String {
    use std::sync::OnceLock;
    static H: OnceLock<String> = OnceLock::new();
    H.get_or_init(|| {
        let run = |c: &str, a: &[&str]| Command::new(c).args(a).output().ok().filter(|o| o.status.success()).map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default();
        let clean = |t: String| t.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '.').collect::<String>();
        if cfg!(target_os = "macos") { let n = clean(run("scutil", &["--get", "LocalHostName"])); if !n.is_empty() { return format!("{n}.local"); } }
        clean(run("hostname", &[]))
    }).clone()
}
