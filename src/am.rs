//! AutoModpack compatibility: client-only mods stay off in mods/ (so they can't crash the server), but a copy
//! goes into AutoModpack's host folder so players still receive them. Only copies the panel made are touched.
use crate::http::{Req, Resp};
use crate::json::{self, n, obj, s, J};
use crate::{mods, App};
use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

static LOCK: Mutex<()> = Mutex::new(());
static INST: Mutex<Option<(Instant, bool)>> = Mutex::new(None);
static LAST: Mutex<(u64, usize, String)> = Mutex::new((0, 0, String::new()));

fn strip(f: &str) -> String { if f.to_lowercase().ends_with(".disabled") { f[..f.len() - 9].to_string() } else { f.to_string() } }
fn state_file(app: &App) -> PathBuf { app.dir.join(".panel-am.json") }
pub fn installed(app: &App) -> bool {
    { let g = INST.lock().unwrap(); if let Some((t, v)) = *g { if t.elapsed() < Duration::from_secs(5) { return v; } } }
    let mut v = app.dir.join("automodpack").is_dir();
    if !v { if let Ok(rd) = fs::read_dir(app.dir.join("mods")) { for e in rd.flatten() { let l = e.file_name().to_string_lossy().to_lowercase(); if l.contains("automodpack") && (l.ends_with(".jar") || l.ends_with(".jar.disabled")) { v = true; break; } } } }
    *INST.lock().unwrap() = Some((Instant::now(), v));
    v
}
pub fn mods_dir(app: &App) -> PathBuf {
    let base = app.dir.join("automodpack/host-modpack");
    let mut name = "main".to_string();
    if let Ok(rd) = fs::read_dir(&base) {
        let mut subs: Vec<String> = rd.flatten().filter(|e| e.path().is_dir()).map(|e| e.file_name().to_string_lossy().to_string()).collect(); subs.sort();
        if !subs.is_empty() && !subs.contains(&"main".to_string()) { name = subs[0].clone(); }
    }
    base.join(name).join("mods")
}
fn load(app: &App) -> BTreeSet<String> {
    match fs::read_to_string(state_file(app)).ok().and_then(|t| json::parse(&t)) { Some(J::Obj(m)) => m.iter().find(|(k, _)| k == "files").and_then(|(_, v)| if let J::Arr(a) = v { Some(a.iter().filter_map(|x| x.as_str().map(|t| t.to_string())).collect()) } else { None }).unwrap_or_default(), _ => BTreeSet::new() }
}
fn save(app: &App, f: &BTreeSet<String>) { let _ = fs::write(state_file(app), obj(vec![("files", J::Arr(f.iter().map(|x| s(x)).collect()))]).to_string()); }

pub fn sync(app: &App) -> Vec<String> {
    let _g = LOCK.lock().unwrap();
    if !installed(app) { return vec![]; }
    let mut want: HashMap<String, PathBuf> = HashMap::new();
    for m in mods::list_mods_raw(app) {
        let g = |k: &str| matches!(m.get(k), Some(J::Bool(true)));
        if g("client_only") && !g("server_only") { let f = m.get("file").and_then(|x| x.as_str()).unwrap_or(""); want.insert(strip(f), app.dir.join("mods").join(f)); }
    }
    let mut mine = load(app);
    if want.is_empty() && mine.is_empty() { return vec![]; }
    let dest = mods_dir(app);
    if fs::create_dir_all(&dest).is_err() { return vec![]; }
    let mut changed = vec![];
    let mut names: Vec<&String> = want.keys().collect(); names.sort();
    for name in names {
        let (src, dp) = (&want[name], dest.join(name));
        let Ok(size) = fs::metadata(src).map(|m| m.len()) else { continue };
        if dp.exists() && !mine.contains(name) { continue; }
        if !dp.exists() || fs::metadata(&dp).map(|m| m.len()).unwrap_or(0) != size {
            let part = dest.join(format!("{name}.part"));
            if fs::copy(src, &part).and_then(|_| fs::rename(&part, &dp)).is_err() { let _ = fs::remove_file(&part); continue; }
            changed.push(format!("+{name}"));
        }
        mine.insert(name.clone());
    }
    for name in mine.clone() {
        if !want.contains_key(&name) {
            match fs::remove_file(dest.join(&name)) { Ok(_) => {}, Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}, Err(_) => continue }
            mine.remove(&name); changed.push(format!("-{name}"));
        }
    }
    save(app, &mine);
    if !changed.is_empty() {
        app.mc.log(&format!("[panel] AutoModpack: {} client-side mod change{} synced to the players' pack.", changed.len(), if changed.len() == 1 { "" } else { "s" }));
        if app.mc.snapshot(0).state == "online" { app.mc.send("automodpack generate"); }
    }
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    *LAST.lock().unwrap() = (now, want.len(), format!("{} change{}", changed.len(), if changed.len() == 1 { "" } else { "s" }));
    changed
}

/// Adds `am_host` / `am` to each mod so the list can show which client-only mods reach players.
pub fn decorate(app: &App, mut out: Vec<J>) -> Vec<J> {
    let have = installed(app);
    let dest = if have { mods_dir(app) } else { PathBuf::new() };
    for m in out.iter_mut() {
        let g = |k: &str| matches!(m.get(k), Some(J::Bool(true)));
        let base = strip(m.get("file").and_then(|x| x.as_str()).unwrap_or(""));
        let am = have && g("client_only") && !g("server_only") && dest.join(&base).is_file();
        if let J::Obj(v) = m { v.push(("am_host".into(), J::Bool(have))); v.push(("am".into(), J::Bool(am))); }
    }
    out
}

pub fn handle(app: &Arc<App>, req: &Req) -> Option<Resp> {
    match (req.method.as_str(), req.path.as_str()) {
        ("GET", "/api/am/fingerprint") => {
            let fp = crate::discord::fp_scan(&app.dir.to_string_lossy());
            Some(Resp::json(200, obj(vec![("fingerprint", s(&fp)), ("online", J::Bool(app.mc.running()))]).to_string()))
        }
        ("GET", "/api/am/status") => {
            let inst = installed(app);
            let sent = if inst { mods::list_mods(app).iter().filter(|m| matches!(m.get("am"), Some(J::Bool(true)))).count() } else { 0 };
            let l = LAST.lock().unwrap().clone();
            let dir = if inst { mods_dir(app).strip_prefix(&app.dir).map(|p| p.display().to_string()).unwrap_or_default() } else { String::new() };
            Some(Resp::json(200, obj(vec![("installed", J::Bool(inst)), ("dir", s(&dir)), ("sent", n(sent as i64)), ("last", obj(vec![("t", n(l.0 as i64)), ("n", n(l.1 as i64)), ("msg", s(&l.2))]))]).to_string()))
        }
        ("POST", "/api/am/sync") => {
            *INST.lock().unwrap() = None;
            let rep = |ok: bool, m: &str| Some(Resp::json(200, obj(vec![("ok", J::Bool(ok)), ("msg", s(m))]).to_string()));
            if !installed(app) { return rep(false, "AutoModpack isn't on this server yet. Install it from the Mods tab first."); }
            let ch = sync(app);
            if ch.is_empty() && app.mc.snapshot(0).state == "online" { app.mc.send("automodpack generate"); }
            if ch.is_empty() { rep(true, "Already up to date. Asked AutoModpack to refresh its pack.") } else { rep(true, &format!("Synced {} change{} to the players' pack.", ch.len(), if ch.len() == 1 { "" } else { "s" })) }
        }
        _ => None,
    }
}
pub fn start(app: &Arc<App>) {
    let a = app.clone();
    thread::spawn(move || { thread::sleep(Duration::from_secs(15)); loop { sync(&a); thread::sleep(Duration::from_secs(20)); } });
}
