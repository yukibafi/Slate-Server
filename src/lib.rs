//! Create Server Panel, Rust port. Phase 1: one server panel (overview, console, start/stop,
//! update-by-upload). Everything else answers "not in the Rust version yet" until it is ported.
pub mod auto;
pub mod browse;
pub mod client;
pub mod config;
pub mod envinfo;
pub mod files;
pub mod hubc;
pub mod hubx;
pub mod jobs;
pub mod loaders;
pub mod dl;
pub mod discord;
pub mod hubbk;
pub mod import;
pub mod am;
pub mod misc;
pub mod packs;
pub mod modupd;
pub mod net;
pub mod mods;
pub mod sysx;
pub mod http;
pub mod hub;
pub mod json;
pub mod mc;
pub mod rx;
pub mod sys;
pub mod theme;
pub mod update;
pub mod guisync;
pub mod selfupdate;

use http::{read_request, write_response, Req, Resp};
use json::{n, obj, s, J};
use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::net::{TcpListener, UdpSocket};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

const INFO_PAGE: &str = "<!doctype html><meta charset=utf-8><title>Create Panel</title><body style=\"font:16px system-ui;background:#16181c;color:#dfe3ea;padding:40px\"><h2>Create Panel host is running</h2><p>This is the engine behind the Create Panel app. Open the app on your Mac or Windows PC and connect to this address.</p>";

#[derive(Clone, Copy, PartialEq)]
pub enum Role { Hub, Server }

pub struct App {
    pub mc: mc::Mc,
    pub key: String,
    pub port: u16,
    pub dir: PathBuf,
    pub paused: AtomicBool, // set while an update hands the port to the new build
    pub role: Role,
    pub hub: Option<Arc<hub::Hub>>,
    pub boot: String,
}

pub fn run_cli() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--self-test") { println!("{}", update::self_test_line()); return; }
    if args.iter().any(|a| a == "--version") { println!("create-panel {} ({})", update::VERSION, update::TARGET); return; }
    let after_update = args.iter().any(|a| a == "--after-update");

    let role = if std::env::var("PANEL_ROLE").map(|r| r == "server").unwrap_or(false) || args.iter().any(|a| a == "--server") { Role::Server } else { Role::Hub };
    let dir = if role == Role::Server { PathBuf::from(std::env::var("SERVER_DIR").unwrap_or_else(|_| ".".into())) } else { theme::data_dir() };
    let port: u16 = std::env::var("PANEL_PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(8080);
    let bind = std::env::var("PANEL_BIND").unwrap_or_else(|_| "0.0.0.0".into());
    let key = load_key();
    if let Ok(me) = std::env::current_exe() { let mut p = me.into_os_string(); p.push(".new"); let _ = std::fs::remove_file(p); }

    let boot = format!("{:08x}", { let mut h = RandomState::new().build_hasher(); h.write_u64(std::process::id() as u64); h.finish() as u32 });
    let hub = if role == Role::Hub { Some(Arc::new(hub::Hub::new(dir.clone(), key.clone(), update::VERSION.to_string()))) } else { None };
    let app = Arc::new(App { mc: mc::Mc::new(dir.clone()), key: key.clone(), port, dir: dir.clone(), paused: AtomicBool::new(false), role, hub: hub.clone(), boot });
    println!("Create Server Panel (Rust) {} {} on {}:{}  key: {}  folder: {}", update::VERSION, if role == Role::Hub { "hub" } else { "server panel" }, bind, port, key, dir.display());
    if let Some(h) = hub { thread::spawn(move || h.supervise()); }
    if role == Role::Hub { guisync::start(); hubx::start(&app); hubc::start(&app); discord::start(&app); hubbk::start(&app); } else { misc::start(&app); auto::start(&app); sysx::start(&app); browse::start(&app); modupd::start(&app); mods::start(&app); files::start(&app); am::start(&app); }

    // a server that was running before an update comes back up
    let resume = dir.join(".panel-resume");
    if resume.is_file() { let _ = std::fs::remove_file(&resume); let a = app.clone(); thread::spawn(move || { thread::sleep(Duration::from_secs(1)); a.mc.start(); }); }

    let mut listener: Option<TcpListener> = None;
    let bind_addr = format!("{bind}:{port}");
    let started = std::time::Instant::now();
    loop {
        if app.paused.load(Ordering::SeqCst) { listener = None; thread::sleep(Duration::from_millis(100)); continue; }
        if listener.is_none() {
            match TcpListener::bind(&bind_addr) {
                Ok(l) => { l.set_nonblocking(true).ok(); listener = Some(l); }
                Err(e) => {
                    // right after an update the old build may still hold the port for a moment
                    if after_update || started.elapsed() < Duration::from_secs(30) { thread::sleep(Duration::from_millis(300)); continue; }
                    eprintln!("Can't use port {port}: {e}"); std::process::exit(1);
                }
            }
        }
        match listener.as_ref().unwrap().accept() {
            Ok((mut st, _)) => {
                st.set_nonblocking(false).ok();
                let app = app.clone();
                thread::spawn(move || {
                    if let Some(req) = read_request(&mut st) { let r = route(&app, &req); write_response(&mut st, &r); }
                });
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => thread::sleep(Duration::from_millis(15)),
            Err(_) => thread::sleep(Duration::from_millis(50)),
        }
    }
}

fn load_key() -> String {
    if let Ok(k) = std::env::var("PANEL_KEY") { if !k.is_empty() { return k; } }
    let _ = std::fs::create_dir_all(theme::data_dir());
    let f = theme::data_dir().join("key");
    if let Ok(k) = std::fs::read_to_string(&f) { let k = k.trim().to_string(); if k.len() >= 8 { return k; } }
    let mut k = String::new();
    for _ in 0..3 { let mut h = RandomState::new().build_hasher(); h.write_u64(std::process::id() as u64); k.push_str(&format!("{:08x}", h.finish() as u32)); }
    let _ = std::fs::write(&f, &k);
    k
}

fn authed(app: &App, req: &Req) -> bool {
    let given = req.header("x-key").unwrap_or("");
    let a = given.as_bytes();
    let b = app.key.as_bytes();
    // constant-time compare
    let mut d = (a.len() ^ b.len()) as u8;
    for i in 0..b.len() { d |= a.get(i).copied().unwrap_or(0) ^ b[i]; }
    d == 0
}

fn err(code: u16, msg: &str) -> Resp { Resp::json(code, obj(vec![("ok", J::Bool(false)), ("error", s(msg)), ("msg", s(msg))]).to_string()) }
fn okmsg(ok: bool, msg: &str) -> Resp { Resp::json(200, obj(vec![("ok", J::Bool(ok)), ("msg", s(msg))]).to_string()) }

fn route(app: &Arc<App>, req: &Req) -> Resp {
    if req.method == "GET" && (req.path == "/" || req.path == "/index.html") { return Resp::html(INFO_PAGE); }
    if req.path == "/favicon.ico" { return Resp::empty(204); }
    if !req.path.starts_with("/api/") { return err(404, "Not found."); }
    if !authed(app, req) && !(app.role == Role::Hub && discord::bot_request_ok(req)) { thread::sleep(Duration::from_millis(500)); return Resp::json(401, "{\"error\":\"bad key\"}".into()); }
    match (req.method.as_str(), req.path.as_str()) {
        ("GET", "/api/ping") => Resp::json(200, obj(vec![("ok", J::Bool(true)), ("version", s(update::VERSION)), ("role", s(if app.role == Role::Hub { "hub" } else { "server" }))]).to_string()),
        ("GET", "/api/theme") => Resp::json(200, theme::get().to_string()),
        ("POST", "/api/theme") => Resp::json(200, theme::set(&String::from_utf8_lossy(&req.body)).to_string()),
        ("POST", "/api/panel/upgrade") => update::handle(app, req),
        ("GET", "/api/panel/update-check") => selfupdate::check_resp(),
        ("POST", "/api/panel/update-install") => selfupdate::install_resp(app),
        ("GET", "/api/panel/update-status") => selfupdate::status_resp(),
        ("POST", "/api/panel/update-source") => selfupdate::source_resp(req),
        (_, p) if app.role == Role::Hub && p.starts_with("/api/s/") => hubc::proxy(app, req).unwrap_or_else(|| err(404, "Not found.")),
        (m, p) if app.role == Role::Hub && p.starts_with("/api/hub/") => hub_route(app, m, p, req),
        ("GET", "/api/config") if app.role == Role::Server => Resp::json(200, config::get(&app.dir, app.mc.running(), &obj(vec![("mc_version", s("")), ("loader", s("")), ("detected", s("not detected yet")), ("detected_mc", s("")), ("detected_loader", s(""))])).to_string()),
        ("POST", "/api/config") if app.role == Role::Server => {
            let body = json::parse(&String::from_utf8_lossy(&req.body)).unwrap_or(J::Null);
            let (ok, m) = config::apply(&app.dir, &body);
            if ok { app.mc.log("[panel] Settings saved."); }
            okmsg(ok, &m)
        }
        ("GET", "/api/status") if app.role == Role::Server => Resp::json(200, status(app, req.q("since").and_then(|v| v.parse().ok()).unwrap_or(0)).to_string()),
        ("POST", "/api/start") if app.role == Role::Server => { let (ok, m) = app.mc.start(); okmsg(ok, &m) }
        ("POST", "/api/stop") if app.role == Role::Server => { let (ok, m) = app.mc.stop(); okmsg(ok, &m) }
        ("POST", "/api/restart") if app.role == Role::Server => { let (ok, m) = app.mc.restart(); okmsg(ok, &m) }
        ("POST", "/api/kill") if app.role == Role::Server => { let (ok, m) = app.mc.kill(); okmsg(ok, &m) }
        ("POST", "/api/command") if app.role == Role::Server => {
            let body = String::from_utf8_lossy(&req.body).to_string();
            let cmd = json::parse(&body).and_then(|j| j.get("cmd").and_then(|c| c.as_str().map(|x| x.to_string()))).unwrap_or_default();
            let cmd = cmd.replace(['\n', '\r'], " ").trim().chars().take(500).collect::<String>();
            if cmd.is_empty() { return okmsg(false, "Empty command."); }
            app.mc.log(&format!("> {cmd}"));
            if app.mc.send(&cmd) { okmsg(true, "Sent.") } else { okmsg(false, "Server isn't running.") }
        }
        ("POST", "/api/quit") => {
            let a = app.clone();
            thread::spawn(move || {
                thread::sleep(Duration::from_millis(300));
                if a.role == Role::Server && a.mc.running() { let _ = std::fs::write(a.dir.join(".panel-resume"), "start"); a.mc.stop(); a.mc.wait_stopped(120); }
                std::process::exit(0); // a hub leaves its server panels running
            });
            okmsg(true, "Shutting down.")
        }
        _ => files::handle(app, req).or_else(|| mods::handle(app, req)).or_else(|| misc::handle(app, req)).or_else(|| auto::handle(app, req)).or_else(|| sysx::handle(app, req)).or_else(|| loaders::handle(app, req)).or_else(|| browse::handle(app, req)).or_else(|| packs::handle(app, req)).or_else(|| am::handle(app, req)).or_else(|| modupd::handle(app, req)).unwrap_or_else(|| err(501, "This part isn't in the Rust version yet.")),
    }
}

fn props(dir: &PathBuf) -> Vec<(String, String)> {
    std::fs::read_to_string(dir.join("server.properties")).unwrap_or_default().lines()
        .filter(|l| !l.starts_with('#')).filter_map(|l| l.split_once('=').map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))).collect()
}

fn lan_ip() -> Option<String> { net::lan_ips().into_iter().next() }

fn status(app: &Arc<App>, since: usize) -> J {
    let sn = app.mc.snapshot(since);
    let p = props(&app.dir);
    let get = |k: &str| p.iter().find(|(a, _)| a == k).map(|(_, v)| v.clone());
    let mc_port: i64 = get("server-port").and_then(|v| v.parse().ok()).unwrap_or(25565);
    let ip = lan_ip();
    let opt = |v: Option<u64>| v.map(|x| n(x as i64)).unwrap_or(J::Null);
    obj(vec![
        ("state", s(sn.state)), ("restarting", J::Bool(sn.restarting)),
        ("players", J::Arr(sn.players.iter().map(|x| s(x)).collect())),
        ("mem_mb", opt(sn.mem_mb)), ("max_mb", misc::max_ram_mb(&app.dir).map(n).unwrap_or(J::Null)), ("uptime", opt(sn.uptime)),
        ("lines", J::Arr(sn.lines.iter().map(|x| s(x)).collect())), ("total", n(sn.total as i64)),
        ("ip", ip.as_ref().map(|x| s(x)).unwrap_or(J::Null)), ("ips", J::Arr(ip.iter().map(|x| s(x)).collect())), ("host", J::Null), ("port", n(mc_port)),
        ("settings", envinfo::load_settings(&app.dir).to_json()),
        ("backup", misc::backup_json()),
        ("mods_dirty", J::Bool(sn.state != "stopped" && misc::MODS_CHANGED.load(std::sync::atomic::Ordering::SeqCst) > app.mc.started_unix())), ("has_start", J::Bool(app.mc.has_start())),
        ("folder", s(&app.dir.display().to_string())), ("motd", get("motd").map(|m| s(&m)).unwrap_or(J::Null)),
        ("login_item", J::Bool(false)), ("version", s(update::VERSION)), ("addons", J::Arr(vec![s("rust-phase-1")])),
        ("boot", s(&app.boot)), ("background", J::Bool(false)), ("env", envinfo::env_cached(&app.dir).2.map(|e| s(&e)).unwrap_or(J::Null)), ("updates", n(0)), ("upd_sig", s("")), ("upd_names", J::Arr(vec![])),
        ("name", s("")), ("hub_port", n(app.port as i64)), ("role", s("server")), ("admin", J::Bool(true)), ("admin_default", J::Bool(false)),
    ])
}

fn hub_route(app: &Arc<App>, method: &str, path: &str, req: &Req) -> Resp {
    let hub = app.hub.as_ref().unwrap();
    let body = json::parse(&String::from_utf8_lossy(&req.body));
    let field = |k: &str| body.as_ref().and_then(|b| b.get(k)).and_then(|v| v.as_str()).unwrap_or("").to_string();
    match (method, path) {
        ("GET", "/api/hub/state") => Resp::json(200, hub.state(lan_ip().into_iter().collect(), &app.boot).to_string()),
        ("POST", "/api/hub/action") => { let (ok, m) = hub.action(&field("id"), &field("action")); okmsg(ok, &m) }
        ("POST", "/api/hub/add") => match hub.add(&field("dir"), &field("name")) { Ok(e) => okmsg(true, &format!("Added {}.", e.name)), Err(m) => okmsg(false, &m) },
        ("GET", "/api/hub/key") => Resp::json(200, obj(vec![("key", s(&app.key)), ("url", s(&format!("http://{}:{}/", lan_ip().unwrap_or_else(|| "localhost".into()), app.port))), ("name_url", s(""))]).to_string()),
        _ => hubx::handle(app, req).or_else(|| discord::handle(app, req)).or_else(|| hubbk::handle(app, req)).or_else(|| import::handle(app, req)).or_else(|| hubc::handle(app, req)).unwrap_or_else(|| err(501, "This part isn't in the Rust version yet.")),
    }
}
