//! Move over from the old Python panel without losing anything, and stop it from auto-starting
//! alongside this one. Nothing is deleted: autostart files are renamed (undo renames them back),
//! server folders are never touched, only the list of servers is copied.
use crate::hub::{local_req, Entry};
use crate::http::{Req, Resp};
use crate::json::{self, n, obj, s, J};
use crate::{sys, App};
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const LABEL: &str = "com.createserverpanel";
const SUFFIX: &str = ".disabled-by-create-panel";

fn home() -> PathBuf { PathBuf::from(sys::home()) }
fn old_registry() -> PathBuf { home().join(".create-panel-servers.json") }
fn old_key() -> Option<String> { fs::read_to_string(home().join(".create-panel-key")).ok().map(|k| k.trim().to_string()).filter(|k| !k.is_empty()) }
fn old_hub_port() -> u16 { std::env::var("PANEL_OLD_HUB_PORT").ok().and_then(|v| v.parse().ok()).unwrap_or(8080) }
fn plist() -> PathBuf { home().join("Library/LaunchAgents").join(format!("{}.plist", LABEL)) }
fn launcher() -> PathBuf { home().join("Applications/Create Server Panel.app") }
fn guarded() -> bool { std::env::var("PANEL_NO_LAUNCHCTL").map(|v| v == "1").unwrap_or(false) || !cfg!(target_os = "macos") }

fn read_old() -> Vec<Entry> {
    let mut v = vec![];
    if let Some(d) = fs::read_to_string(old_registry()).ok().and_then(|t| json::parse(&t)) {
        if let Some(J::Arr(a)) = d.get("servers") {
            for e in a {
                let g = |k: &str| e.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
                if let Some(J::Num(p)) = e.get("port") { if !g("id").is_empty() && !g("dir").is_empty() { v.push(Entry { id: g("id"), name: if g("name").is_empty() { "Server".into() } else { g("name") }, dir: g("dir"), port: *p as u16, settings: g("settings"), created: match e.get("created") { Some(J::Num(c)) => *c, _ => 0.0 } }); } }
            }
        }
    }
    v
}
fn answering(port: u16) -> bool { std::net::TcpStream::connect_timeout(&format!("127.0.0.1:{}", port).parse().unwrap(), Duration::from_millis(400)).is_ok() }
fn state_of(port: u16, key: &str) -> Option<String> {
    let (c, b) = local_req(port, "GET", "/api/status", key, None, 4)?;
    if c != 200 { return None; }
    json::parse(&b)?.get("state").and_then(|v| v.as_str()).map(|x| x.to_string())
}

#[derive(Default)]
struct Job { state: String, log: Vec<String>, imported: usize }
static JOB: Mutex<Option<Job>> = Mutex::new(None);
fn say(m: &str) { if let Some(j) = JOB.lock().unwrap().as_mut() { j.log.push(m.to_string()); } }
fn set_state(st: &str) { if let Some(j) = JOB.lock().unwrap().as_mut() { j.state = st.into(); } }

fn marker(app: &App) -> PathBuf { app.hub.as_ref().map(|h| h.data_dir.join("import.json")).unwrap_or_else(|| PathBuf::from("import.json")) }

fn detect(app: &App) -> J {
    let ents = read_old();
    let key = old_key().unwrap_or_default();
    let list: Vec<J> = ents.iter().map(|e| { let st = if answering(e.port) { state_of(e.port, &key).unwrap_or_else(|| "panel-up".into()) } else { "stopped".into() }; obj(vec![("id", s(&e.id)), ("name", s(&e.name)), ("dir", s(&e.dir)), ("port", n(e.port as i64)), ("state", s(&st))]) }).collect();
    obj(vec![("found", J::Bool(old_registry().is_file())), ("servers", J::Arr(list)), ("old_panel_running", J::Bool(answering(old_hub_port()) && old_hub_port() != app.port)),
        ("autostart", J::Bool(plist().is_file())), ("launcher_app", J::Bool(launcher().exists())), ("done", J::Bool(marker(app).is_file()))])
}

fn rename_aside(from: &PathBuf, moved: &mut Vec<(String, String)>) {
    if !from.exists() { return; }
    let mut to = PathBuf::from(format!("{}{}", from.display(), SUFFIX));
    if to.exists() { to = PathBuf::from(format!("{}{}.{}", from.display(), SUFFIX, sys::now() as u64)); }
    if fs::rename(from, &to).is_ok() { say(&format!("Renamed {} (undo puts it back).", from.file_name().unwrap_or_default().to_string_lossy())); moved.push((from.display().to_string(), to.display().to_string())); }
}

fn run(app: Arc<App>, stop_old: bool) {
    let hub = match app.hub.as_ref() { Some(h) => h.clone(), None => { set_state("error"); say("Only the hub can import."); return; } };
    let ents = read_old();
    if ents.is_empty() { set_state("error"); say("No servers found in the old panel's list."); return; }
    let key = old_key().unwrap_or_default();
    // 1. stop the old panel cleanly (Minecraft first, saving the world), then the old panels themselves
    if stop_old {
        for e in &ents {
            if !answering(e.port) { continue; }
            match state_of(e.port, &key).as_deref() {
                Some("online") | Some("starting") => {
                    say(&format!("Stopping {} and saving its world...", e.name));
                    let _ = local_req(e.port, "POST", "/api/stop", &key, Some("{}"), 10);
                    let t0 = Instant::now();
                    while t0.elapsed() < Duration::from_secs(150) { if state_of(e.port, &key).as_deref() == Some("stopped") { break; } std::thread::sleep(Duration::from_millis(500)); }
                    if state_of(e.port, &key).as_deref() != Some("stopped") { say(&format!("{} didn't stop in time. Nothing was changed. Stop it in the old panel and try again.", e.name)); set_state("error"); return; }
                }
                _ => {}
            }
        }
        for e in &ents { if answering(e.port) { let _ = local_req(e.port, "POST", "/api/quit", &key, Some("{}"), 5); } }
        if answering(old_hub_port()) && old_hub_port() != app.port { let _ = local_req(old_hub_port(), "POST", "/api/quit", &key, Some("{}"), 5); }
        let t0 = Instant::now();
        while t0.elapsed() < Duration::from_secs(20) && (ents.iter().any(|e| answering(e.port))) { std::thread::sleep(Duration::from_millis(300)); }
        if ents.iter().any(|e| answering(e.port)) { say("An old panel is still running, so I stopped before changing anything."); set_state("error"); return; }
        say("Old panel stopped.");
    }
    // 2. switch off the old panel's autostart (renamed, never deleted)
    let mut moved = vec![];
    if !guarded() {
        let uid = Command::new("id").arg("-u").output().ok().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default();
        if plist().is_file() { let _ = Command::new("launchctl").args(["bootout", &format!("gui/{}/{}", uid, LABEL)]).output(); }
        for needle in ["Server-Panel", "Create-Server-Panel"] { let _ = Command::new("osascript").args(["-e", &format!("tell application \"System Events\" to delete (every login item whose path contains \"{}\")", needle)]).output(); }
    }
    rename_aside(&plist(), &mut moved);
    rename_aside(&launcher(), &mut moved);
    // 3. copy the server list
    let (added, skipped) = hub.import_entries(ents);
    say(&format!("Imported {} server(s){}. Your server folders were not touched.", added.len(), if skipped > 0 { format!(", {} already listed", skipped) } else { String::new() }));
    let m = obj(vec![("when", n(sys::now() as i64)), ("moved", J::Arr(moved.iter().map(|(a, b)| J::Arr(vec![s(a), s(b)])).collect())), ("added", J::Arr(added.iter().map(|x| s(x)).collect()))]);
    let _ = fs::write(marker(&app), m.to_string());
    if let Some(j) = JOB.lock().unwrap().as_mut() { j.imported = added.len(); }
    set_state("done");
}

fn undo(app: &App) -> (bool, String) {
    let mf = marker(app);
    let d = match fs::read_to_string(&mf).ok().and_then(|t| json::parse(&t)) { Some(d) => d, None => return (false, "Nothing to undo.".into()) };
    if let Some(J::Arr(m)) = d.get("moved") { for p in m { if let J::Arr(pair) = p { if let (Some(a), Some(b)) = (pair.get(0).and_then(|x| x.as_str()), pair.get(1).and_then(|x| x.as_str())) { if !PathBuf::from(a).exists() { let _ = fs::rename(b, a); } } } } }
    if !guarded() && plist().is_file() {
        let uid = Command::new("id").arg("-u").output().ok().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default();
        let _ = Command::new("launchctl").args(["bootstrap", &format!("gui/{}", uid), &plist().display().to_string()]).output();
    }
    let _ = fs::remove_file(&mf);
    (true, "Put the old panel's autostart back. Stop the servers here before starting the old panel, so two copies never run the same server.".into())
}

pub fn handle(app: &Arc<App>, req: &Req) -> Option<Resp> {
    let ok = |b: bool, m: &str| Resp::json(if b { 200 } else { 400 }, obj(vec![("ok", J::Bool(b)), ("msg", s(m))]).to_string());
    match (req.method.as_str(), req.path.as_str()) {
        ("GET", "/api/hub/import/detect") => Some(Resp::json(200, detect(app).to_string())),
        ("GET", "/api/hub/import/status") => {
            let g = JOB.lock().unwrap();
            let (st, log, n_) = match g.as_ref() { Some(j) => (j.state.clone(), j.log.clone(), j.imported), None => ("idle".into(), vec![], 0) };
            Some(Resp::json(200, obj(vec![("state", s(&st)), ("log", J::Arr(log.iter().map(|x| s(x)).collect())), ("imported", n(n_ as i64))]).to_string()))
        }
        ("POST", "/api/hub/import/run") => {
            let body = json::parse(&String::from_utf8_lossy(&req.body)).unwrap_or(J::Null);
            if !matches!(body.get("confirm"), Some(J::Bool(true))) { return Some(ok(false, "Please confirm first.")); }
            { let mut g = JOB.lock().unwrap(); if matches!(g.as_ref(), Some(j) if j.state == "running") { return Some(ok(false, "Already running.")); } *g = Some(Job { state: "running".into(), ..Default::default() }); }
            let stop_old = !matches!(body.get("stop_old"), Some(J::Bool(false)));
            let a = app.clone();
            std::thread::spawn(move || run(a, stop_old));
            Some(ok(true, "Started."))
        }
        ("POST", "/api/hub/import/undo") => { let (b, m) = undo(app); Some(ok(b, &m)) }
        _ => None,
    }
}
