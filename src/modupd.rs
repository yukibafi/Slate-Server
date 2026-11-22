//! Mod-update schedule: when to check, optional safe auto-install, "mods to leave alone".
use crate::http::{Req, Resp};
use crate::json::{self, n, obj, s, J};
use crate::{browse, envinfo, sys, App};
use std::collections::HashSet;
use std::fs;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::Duration;

const MODES: &[&str] = &["always", "boot", "morning", "daily", "manual"];
const GUIDE: &str = include_str!("updates_guide.txt");

#[derive(Clone)]
struct Cfg { mode: String, time: String, install: bool, last_day: String, last_check: f64, last_result: String, pending: bool, skip: Vec<String> }
static CFG: Mutex<Option<Cfg>> = Mutex::new(None);
static BUSY: Mutex<bool> = Mutex::new(false);
static PREV: Mutex<&'static str> = Mutex::new("stopped");
static EVT: (Mutex<bool>, Condvar) = (Mutex::new(false), Condvar::new());

pub fn nudge() { *EVT.0.lock().unwrap() = true; EVT.1.notify_all(); }
/// Waits up to `secs` for a nudge; true if nudged.
pub fn wait_event(secs: u64) -> bool {
    let g = EVT.0.lock().unwrap();
    let (mut g, _) = EVT.1.wait_timeout_while(g, Duration::from_secs(secs), |x| !*x).unwrap();
    let r = *g; *g = false; r
}

fn valid_time(t: &str) -> bool { let b = t.as_bytes(); b.len() == 5 && b[2] == b':' && matches!(b[0], b'0'..=b'2') && b[1].is_ascii_digit() && matches!(b[3], b'0'..=b'5') && b[4].is_ascii_digit() && (t[..2].parse::<u32>().unwrap_or(99) < 24) }
fn clean(d: &J) -> Cfg {
    let t = d.get("time").and_then(|x| x.as_str()).unwrap_or("");
    let g = |k: &str| d.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
    let mode = g("mode");
    let mut skip: Vec<String> = match d.get("skip") { Some(J::Arr(a)) => a.iter().filter_map(|x| x.as_str()).map(|x| x.trim().to_lowercase().chars().take(100).collect::<String>()).filter(|x| !x.is_empty()).collect(), _ => vec![] };
    skip.sort(); skip.dedup(); skip.truncate(500);
    Cfg { mode: if MODES.contains(&mode.as_str()) { mode } else { "always".into() }, time: if valid_time(t) { t.into() } else { "05:00".into() }, install: matches!(d.get("install"), Some(J::Bool(true))),
        last_day: g("last_day").chars().take(10).collect(), last_check: match d.get("last_check") { Some(J::Num(x)) => *x, _ => 0.0 }, last_result: g("last_result").chars().take(160).collect(), pending: matches!(d.get("pending"), Some(J::Bool(true))), skip }
}
fn to_j(c: &Cfg) -> J {
    obj(vec![("mode", s(&c.mode)), ("time", s(&c.time)), ("install", J::Bool(c.install)), ("last_day", s(&c.last_day)), ("last_check", J::Num(c.last_check)), ("last_result", s(&c.last_result)), ("pending", J::Bool(c.pending)), ("skip", J::Arr(c.skip.iter().map(|x| s(x)).collect()))])
}
fn file(app: &App) -> std::path::PathBuf { app.dir.join(".panel-modupdates.json") }
fn cfg(app: &App) -> Cfg {
    let mut g = CFG.lock().unwrap();
    if g.is_none() { *g = Some(clean(&fs::read_to_string(file(app)).ok().and_then(|t| json::parse(&t)).unwrap_or(J::Null))); }
    g.clone().unwrap()
}
fn save(app: &App, c: Cfg) {
    let tmp = app.dir.join(".panel-modupdates.json.tmp");
    if fs::write(&tmp, to_j(&c).to_string()).is_ok() { let _ = fs::rename(&tmp, file(app)); }
    *CFG.lock().unwrap() = Some(c);
}
pub fn skipped(app: &App) -> HashSet<String> { cfg(app).skip.into_iter().collect() }

/// (YYYY-MM-DD, minutes since midnight) in local time
fn local_now() -> (String, u32) {
    let st = sys::stamp(); // YYYYMMDD-HHMMSS
    (format!("{}-{}-{}", &st[0..4], &st[4..6], &st[6..8]), st[9..11].parse::<u32>().unwrap_or(0) * 60 + st[11..13].parse::<u32>().unwrap_or(0))
}
fn minutes(t: &str) -> u32 { t[..2].parse::<u32>().unwrap_or(0) * 60 + t[3..].parse::<u32>().unwrap_or(0) }

fn check_now(app: &Arc<App>, why: &str) -> bool {
    if envinfo::load_settings(&app.dir).updates_lock || !app.dir.join("mods").is_dir() || envinfo::env_cached(&app.dir).0.is_none() { return false; }
    if matches!(browse::phase(), "checking" | "installing") { return false; }
    if !browse::start_check(app).0 { return false; }
    for _ in 0..600 { thread::sleep(Duration::from_secs(1)); if browse::phase() != "checking" { break; } }
    let cnt = browse::pending_count();
    let mut c = cfg(app);
    c.last_check = sys::now();
    c.last_result = if browse::phase() == "error" { format!("The check didn't finish ({why}).") } else if cnt > 0 { format!("{cnt} mod update{} found.", if cnt == 1 { "" } else { "s" }) } else { "Everything is up to date.".into() };
    c.pending = cnt > 0 && c.install;
    save(app, c);
    true
}
fn pending_files() -> Vec<String> { browse::pending_files() }

fn safe_install(app: &Arc<App>, files: Vec<String>, how: &'static str) -> (bool, String) {
    { let mut b = BUSY.lock().unwrap(); if *b { return (false, "An update install is already in progress.".into()); } *b = true; }
    let was = app.mc.running();
    let a = app.clone();
    thread::spawn(move || {
        let note = |msg: String, pending: Option<bool>| { let mut c = cfg(&a); c.last_result = msg.chars().take(160).collect(); if let Some(p) = pending { c.pending = p; } save(&a, c); };
        let r = (|| -> Result<(), String> {
            if was {
                if !a.mc.snapshot(0).players.is_empty() {
                    a.mc.send("say Mod updates are being installed. The server restarts in 60 seconds."); thread::sleep(Duration::from_secs(50));
                    a.mc.send("say Server restarting for mod updates in 10 seconds. Your progress is saved."); thread::sleep(Duration::from_secs(10));
                }
                a.mc.log(&format!("[panel] Restarting to install {} mod update{}...", files.len(), if files.len() == 1 { "" } else { "s" }));
                a.mc.send("save-all flush"); thread::sleep(Duration::from_secs(3));
                a.mc.stop(); a.mc.wait_stopped(120);
            }
            let (ok, msg) = browse::start_apply(&a, files.into_iter().collect());
            if !ok { note(format!("Couldn't install: {msg}"), Some(false)); return Ok(()); }
            for _ in 0..1800 { thread::sleep(Duration::from_secs(1)); if browse::phase() != "installing" { break; } }
            let (okn, bad) = browse::last_result_counts();
            let (d, m) = local_now();
            let _ = d;
            note(format!("Installed {okn} update{}{} at {:02}:{:02} ({how}).", if okn == 1 { "" } else { "s" }, if bad > 0 { format!(", {bad} skipped") } else { String::new() }, m / 60, m % 60), Some(false));
            Ok(())
        })();
        if let Err(e) = r { note(format!("Update install failed: {e}"), Some(false)); }
        if was && !a.mc.running() { a.mc.log("[panel] Starting the server again after the mod update."); a.mc.start(); }
        *BUSY.lock().unwrap() = false; nudge();
    });
    (true, format!("Installing updates{}.", if was { " - the server will restart" } else { "" }))
}

fn tick(app: &Arc<App>) {
    let c = cfg(app);
    let st = app.mc.snapshot(0).state;
    let prev = { let mut p = PREV.lock().unwrap(); let o = *p; *p = st; o };
    let live = |x: &str| x == "starting" || x == "online";
    let started = live(st) && !live(prev);
    let (today, mins) = local_now();
    let due = match c.mode.as_str() {
        "boot" if started => Some("server started"),
        "morning" if started && c.last_day != today && mins >= minutes(&c.time) => Some("first start of the day"),
        "daily" if c.last_day != today && mins >= minutes(&c.time) => Some("daily check"),
        _ => None };
    let mut c = c;
    if let Some(w) = due { c.last_day = today; save(app, c); check_now(app, w); c = cfg(app); }
    if c.pending && c.install && !*BUSY.lock().unwrap() && !envinfo::load_settings(&app.dir).updates_lock && (st == "stopped" || st == "online") {
        let f = pending_files();
        if !f.is_empty() { safe_install(app, f, "automatic"); }
        else if browse::phase() == "ready" || browse::phase() == "done" { c.pending = false; save(app, c); }
    }
}
fn next_text(c: &Cfg) -> String {
    match c.mode.as_str() {
        "always" => "Checking in the background every 30 minutes.".into(),
        "manual" => "Only when you press Check now.".into(),
        "boot" => "Checks every time the server starts.".into(),
        "morning" => format!("Checks on the first server start each day, from {} on.", c.time),
        _ => { let (today, _) = local_now(); format!("Next check: {} at {}.", if c.last_day == today { "tomorrow" } else { "today" }, c.time) }
    }
}

pub fn handle(app: &Arc<App>, req: &Req) -> Option<Resp> {
    let body = || json::parse(&String::from_utf8_lossy(&req.body)).unwrap_or(J::Null);
    let rep = |ok: bool, m: &str| Resp::json(200, obj(vec![("ok", J::Bool(ok)), ("msg", s(m))]).to_string());
    Some(match (req.method.as_str(), req.path.as_str()) {
        ("GET", "/api/modupd") => {
            let c = cfg(app); let (cnt, chk) = browse::update_summary();
            let mut j = to_j(&c);
            if let J::Obj(m) = &mut j { for (k, v) in [("ok", J::Bool(true)), ("next", s(&next_text(&c))), ("count", n(cnt as i64)), ("checked_at", J::Num(if chk > 0.0 { chk } else { c.last_check })), ("locked", J::Bool(envinfo::load_settings(&app.dir).updates_lock))] { m.retain(|(a, _)| a != k); m.push((k.into(), v)); } }
            Resp::json(200, j.to_string())
        }
        ("POST", "/api/modupd") => {
            let b = body(); let mut c = cfg(app);
            if let Some(m) = b.get("mode").and_then(|x| x.as_str()) { if MODES.contains(&m) { c.mode = m.into(); } }
            if let Some(t) = b.get("time").and_then(|x| x.as_str()) { if valid_time(t) { c.time = t.into(); } }
            if let Some(i) = b.get("install") { c.install = matches!(i, J::Bool(true)); if !c.install { c.pending = false; } }
            let truthy = |k: &str| match b.get(k) { Some(J::Str(x)) => !x.is_empty(), Some(J::Bool(x)) => *x, Some(J::Num(x)) => *x != 0.0, _ => false };
            if truthy("mode") || truthy("time") { c.last_day.clear(); }
            let mut removed = false;
            if let Some(J::Arr(a)) = b.get("skip") {
                let old: HashSet<String> = c.skip.iter().cloned().collect();
                let mut t = to_j(&c); if let J::Obj(m) = &mut t { m.retain(|(k, _)| k != "skip"); m.push(("skip".into(), J::Arr(a.clone()))); }
                c = clean(&t); removed = old.iter().any(|x| !c.skip.contains(x));
            }
            c = clean(&to_j(&c)); save(app, c.clone());
            if b.get("skip").is_some() { browse::drop_skipped(&c.skip.iter().cloned().collect()); if removed { let a = app.clone(); thread::spawn(move || { check_now(&a, "skip list changed"); }); } }
            let mut st = envinfo::load_settings(&app.dir); st.update_check = c.mode == "always"; envinfo::save_settings(&app.dir, &st);
            if c.mode == "always" { nudge(); }
            rep(true, "Saved.")
        }
        ("GET", "/api/modupd/guide") => Resp::json(200, obj(vec![("ok", J::Bool(true)), ("text", s(GUIDE))]).to_string()),
        ("POST", "/api/modupd/install-now") => {
            if envinfo::load_settings(&app.dir).updates_lock { return Some(rep(false, "Updates are locked for this server. Turn off \"Lock updates\" under Overview > Options first.")); }
            let f = pending_files();
            if f.is_empty() { return Some(rep(false, "No updates are waiting. Press Check now first.")); }
            let (ok, m) = safe_install(app, f, "manual"); rep(ok, &m)
        }
        _ => return None,
    })
}
pub fn start(app: &Arc<App>) {
    let a = app.clone();
    thread::spawn(move || { thread::sleep(Duration::from_secs(20)); loop { tick(&a); thread::sleep(Duration::from_secs(30)); } });
}
