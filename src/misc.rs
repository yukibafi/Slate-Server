//! Backups, player lists, bans, world actions, skins, settings, and the small status helpers.
use crate::config::read_props;
use crate::envinfo::{self, load_settings, save_settings};
use crate::http::{Req, Resp};
use crate::json::{self, n, obj, s, J};
use crate::{sys, App};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

/// Unix time (secs) the mods folder was last changed through the panel (for "restart to apply").
pub static MODS_CHANGED: AtomicU64 = AtomicU64::new(0);
pub fn mods_touched() { MODS_CHANGED.store(sys::now() as u64, Ordering::SeqCst); }

struct Bk { running: bool, msg: String, ok: Option<bool>, pct: i64 }
static BACKUP: Mutex<Bk> = Mutex::new(Bk { running: false, msg: String::new(), ok: None, pct: 0 });
pub fn backup_json() -> J {
    let b = BACKUP.lock().unwrap();
    obj(vec![("running", J::Bool(b.running)), ("msg", s(&b.msg)), ("ok", b.ok.map(J::Bool).unwrap_or(J::Null)), ("pct", n(b.pct))])
}

fn reply(ok: bool, msg: &str, extra: Vec<(&str, J)>) -> Resp {
    let mut v = vec![("ok", J::Bool(ok)), ("msg", s(msg))]; v.extend(extra);
    Resp::json(200, obj(v).to_string())
}
fn name_ok(t: &str) -> bool { !t.is_empty() && t.len() <= 17 && t.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.') }
fn backup_dir(app: &App) -> PathBuf { app.dir.join("panel-backups") }
pub fn world_dir(dir: &Path) -> PathBuf {
    let ln = read_props(dir).into_iter().find(|(k, _)| k == "level-name").map(|(_, v)| v).filter(|v| !v.is_empty()).unwrap_or_else(|| "world".into());
    dir.join(ln)
}
pub fn max_ram_mb(dir: &Path) -> Option<i64> {
    for l in fs::read_to_string(dir.join("user_jvm_args.txt")).ok()?.lines() {
        let t = l.trim_start();
        if let Some(r) = t.strip_prefix("-Xmx") {
            let d: String = r.chars().take_while(|c| c.is_ascii_digit()).collect();
            let u = r[d.len()..].chars().next();
            if let (Ok(v), Some(u)) = (d.parse::<i64>(), u) { if "GgMm".contains(u) { return Some(if u.to_ascii_lowercase() == 'g' { v * 1024 } else { v }); } }
        }
    }
    None
}

pub fn list_backups(app: &App) -> Vec<(String, u64, i64)> {
    let mut v = vec![];
    if let Ok(rd) = fs::read_dir(backup_dir(app)) {
        for e in rd.flatten() {
            let fnm = e.file_name().to_string_lossy().to_string();
            if !fnm.ends_with(".zip") { continue; }
            if let Ok(m) = e.metadata() {
                let t = m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs() as i64).unwrap_or(0);
                v.push((fnm, m.len(), t));
            }
        }
    }
    v.sort_by(|a, b| b.2.cmp(&a.2));
    v
}

pub fn backup_async(app: &Arc<App>) -> (bool, String) {
    {
        let b = BACKUP.lock().unwrap();
        if b.running { return (false, "A backup is already running.".into()); }
    }
    let world = world_dir(&app.dir);
    if !world.is_dir() { return (false, format!("Couldn't find the world folder ({}). Start the server once first.", world.file_name().map(|x| x.to_string_lossy().to_string()).unwrap_or_default())); }
    *BACKUP.lock().unwrap() = Bk { running: true, msg: "Saving the world...".into(), ok: None, pct: 3 };
    let a = app.clone();
    thread::spawn(move || run_backup(&a, &world));
    (true, "Backup started.".into())
}

/// Blocking backup (also used by scheduled backups). Returns the zip name or an error.
pub fn run_backup(app: &Arc<App>, world: &Path) -> Result<String, String> {
    let was_running = app.mc.running();
    let res = (|| -> Result<String, String> {
        if was_running {
            app.mc.clear_saved();
            app.mc.send("save-off");
            app.mc.send("save-all flush");
            if !app.mc.wait_saved(90) { app.mc.log("[panel] World save took too long, backing up anyway."); }
        }
        BACKUP.lock().unwrap().msg = "Compressing...".into();
        let bd = backup_dir(app);
        fs::create_dir_all(&bd).map_err(|e| e.to_string())?;
        let wname = world.file_name().map(|x| x.to_string_lossy().to_string()).unwrap_or_else(|| "world".into());
        let name = format!("{}_{}.zip", wname, stamp_dashed());
        let part = bd.join(format!("{name}.part"));
        // progress: compare the growing .part file with the world's size (zip is usually 50-90% of it)
        let total = dir_bytes(world).max(1);
        let fin = Arc::new(std::sync::atomic::AtomicBool::new(false));
        { let (fin, part) = (fin.clone(), part.clone()); thread::spawn(move || { while !fin.load(std::sync::atomic::Ordering::SeqCst) { let sz = fs::metadata(&part).map(|m| m.len()).unwrap_or(0) as f64; let pct = (5.0 + 90.0 * (sz / (total as f64 * 0.8)).min(1.0)) as i64; { let mut b = BACKUP.lock().unwrap(); if b.running { b.pct = pct.min(95); b.msg = format!("Compressing... {}%", pct.min(95)); } } thread::sleep(Duration::from_millis(700)); } }); }
        let zr = sys::zip_folder(world.parent().unwrap_or(Path::new(".")), &wname, &part, &["session.lock"]);
        fin.store(true, std::sync::atomic::Ordering::SeqCst);
        zr?;
        fs::rename(&part, bd.join(&name)).map_err(|e| e.to_string())?;
        let keep = load_settings(&app.dir).backup_keep.max(1) as usize;
        for old in list_backups(app).into_iter().skip(keep) { let _ = fs::remove_file(bd.join(old.0)); }
        app.mc.log(&format!("[panel] Backup saved: {name}"));
        Ok(name)
    })();
    match &res {
        Ok(name) => *BACKUP.lock().unwrap() = Bk { running: false, msg: format!("Saved {name}"), ok: Some(true), pct: 100 },
        Err(e) => { app.mc.log(&format!("[panel] Backup failed: {e}")); *BACKUP.lock().unwrap() = Bk { running: false, msg: format!("Backup failed: {e}"), ok: Some(false), pct: 0 }; }
    }
    if was_running && app.mc.running() { app.mc.send("save-on"); }
    res
}
fn dir_bytes(p: &Path) -> u64 { let mut n = 0; if let Ok(rd) = fs::read_dir(p) { for e in rd.flatten() { let q = e.path(); if q.is_dir() { n += dir_bytes(&q); } else if let Ok(m) = e.metadata() { n += m.len(); } } } n }
fn stamp_dashed() -> String { let t = sys::stamp(); format!("{}-{}-{}_{}-{}-{}", &t[0..4], &t[4..6], &t[6..8], &t[9..11], &t[11..13], &t[13..15]) }

fn read_list(app: &App, f: &str) -> Vec<J> {
    match fs::read_to_string(app.dir.join(f)).ok().and_then(|t| json::parse(&t)) { Some(J::Arr(a)) => a, _ => vec![] }
}
fn names(app: &App, f: &str) -> J {
    let mut v: Vec<String> = read_list(app, f).iter().filter_map(|e| e.get("name").and_then(|x| x.as_str()).filter(|x| !x.is_empty()).map(|x| x.to_string())).collect();
    v.sort_by_key(|x| x.to_lowercase());
    J::Arr(v.iter().map(|x| s(x)).collect())
}
fn gs(j: &J, k: &str) -> String { j.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string() }
fn write_list(app: &App, f: &str, v: Vec<J>) -> Result<(), String> { fs::write(app.dir.join(f), J::Arr(v).to_string()).map_err(|e| e.to_string()) }

const WORLD: &[(&str, &str)] = &[("day", "time set day"), ("noon", "time set noon"), ("night", "time set night"), ("midnight", "time set midnight"), ("clear", "weather clear"), ("rain", "weather rain"), ("thunder", "weather thunder"), ("peaceful", "difficulty peaceful"), ("easy", "difficulty easy"), ("normal", "difficulty normal"), ("hard", "difficulty hard"), ("clear_items", "kill @e[type=item]"), ("save", "save-all"), ("wl_on", "whitelist on"), ("wl_off", "whitelist off"), ("wl_reload", "whitelist reload")];

static SKINS: Mutex<Option<HashMap<String, (f64, Option<Vec<u8>>)>>> = Mutex::new(None);
fn good_png(d: &[u8]) -> bool {
    d.len() > 32 && d[..8] == *b"\x89PNG\r\n\x1a\n" && &d[12..16] == b"IHDR" && u32::from_be_bytes([d[16], d[17], d[18], d[19]]) == 64 && matches!(u32::from_be_bytes([d[20], d[21], d[22], d[23]]), 32 | 64)
}
fn api(var: &str, def: &str) -> String { std::env::var(var).unwrap_or_else(|_| def.to_string()) }
fn get_skin(name: &str) -> Option<Vec<u8>> {
    let key = name.to_lowercase();
    let now = sys::now();
    if let Some((t, d)) = SKINS.lock().unwrap().as_ref().and_then(|m| m.get(&key)).cloned() { if now - t < if d.is_some() { 3600.0 } else { 300.0 } { return d; } }
    let mut data: Option<Vec<u8>> = None;
    let mojang = api("PANEL_MOJANG_API", "https://api.mojang.com");
    let sess = api("PANEL_MOJANG_SESSION", "https://sessionserver.mojang.com");
    if let (200, b) = sys::get(&format!("{mojang}/users/profiles/minecraft/{name}"), 8) {
        if let Some(uid) = json::parse(&String::from_utf8_lossy(&b)).and_then(|j| j.get("id").and_then(|x| x.as_str().map(|x| x.to_string()))) {
            if let (200, b) = sys::get(&format!("{sess}/session/minecraft/profile/{uid}"), 8) {
                if let Some(J::Arr(props)) = json::parse(&String::from_utf8_lossy(&b)).and_then(|j| j.get("properties").cloned()) {
                    for p in props { if gs(&p, "name") == "textures" {
                        if let Some(dec) = b64(&gs(&p, "value")).and_then(|d| json::parse(&String::from_utf8_lossy(&d))) {
                            let url = dec.get("textures").and_then(|t| t.get("SKIN")).map(|k| gs(k, "url")).unwrap_or_default();
                            if url.starts_with("https://textures.minecraft.net/") || url.starts_with("http://127.0.0.1") { if let (200, d) = sys::get(&url, 10) { data = Some(d); } }
                        }
                    } }
                }
            }
        }
    }
    if !data.as_deref().map(good_png).unwrap_or(false) {
        data = match sys::get(&format!("{}/skin/{name}", api("PANEL_MINOTAR", "https://minotar.net")), 10) { (200, d) => Some(d), _ => None };
    }
    if !data.as_deref().map(good_png).unwrap_or(false) { data = None; }
    let mut g = SKINS.lock().unwrap();
    let m = g.get_or_insert_with(HashMap::new);
    if m.len() > 300 { m.clear(); }
    m.insert(key, (now, data.clone()));
    data
}
fn b64(t: &str) -> Option<Vec<u8>> {
    let mut out = vec![]; let (mut buf, mut bits) = (0u32, 0);
    for c in t.bytes() {
        let v = match c { b'A'..=b'Z' => c - b'A', b'a'..=b'z' => c - b'a' + 26, b'0'..=b'9' => c - b'0' + 52, b'+' | b'-' => 62, b'/' | b'_' => 63, b'=' => break, _ => continue } as u32;
        buf = (buf << 6) | v; bits += 6;
        if bits >= 8 { bits -= 8; out.push((buf >> bits) as u8); buf &= (1 << bits) - 1; }
    }
    Some(out)
}

pub fn handle(app: &Arc<App>, req: &Req) -> Option<Resp> {
    let body = || json::parse(&String::from_utf8_lossy(&req.body)).unwrap_or(J::Null);
    Some(match (req.method.as_str(), req.path.as_str()) {
        ("GET", "/api/summary") => {
            let sn = app.mc.snapshot(usize::MAX);
            let p = read_props(&app.dir);
            let port = p.iter().find(|(k, _)| k == "server-port").and_then(|(_, v)| v.parse::<i64>().ok()).unwrap_or(25565);
            let env = envinfo::env_cached(&app.dir).2;
            let st = load_settings(&app.dir);
            Resp::json(200, obj(vec![("state", s(sn.state)), ("players", J::Arr(sn.players.iter().map(|x| s(x)).collect())), ("mem_mb", sn.mem_mb.map(|x| n(x as i64)).unwrap_or(J::Null)),
                ("max_mb", max_ram_mb(&app.dir).map(n).unwrap_or(J::Null)), ("uptime", sn.uptime.map(|x| n(x as i64)).unwrap_or(J::Null)), ("port", n(port)),
                ("motd", s(&p.iter().find(|(k, _)| k == "motd").map(|x| x.1.clone()).unwrap_or_default())), ("env", env.map(|e| s(&e)).unwrap_or(J::Null)), ("updates", n(0)),
                ("has_start", J::Bool(app.mc.has_start())), ("folder", s(&app.dir.display().to_string())), ("updates_locked", J::Bool(st.updates_lock)),
                ("version", s(crate::update::VERSION)), ("boot", s(&app.boot)), ("pid", n(std::process::id() as i64))]).to_string())
        }
        ("GET", "/api/backups") => {
            let free = sys::free_space(&app.dir);
            Resp::json(200, obj(vec![("backups", J::Arr(list_backups(app).into_iter().map(|(a, b, c)| obj(vec![("name", s(&a)), ("size", n(b as i64)), ("mtime", n(c))])).collect())), ("free", free.map(|x| n(x as i64)).unwrap_or(J::Null))]).to_string())
        }
        ("GET", "/api/backup/download") => {
            let name = req.q("name").unwrap_or("").to_string(); let p = backup_dir(app).join(&name);
            if name.contains(['/', '\\']) || !name.ends_with(".zip") || !p.is_file() { return Some(Resp::empty(404)); }
            match fs::read(&p) { Ok(b) => Resp { code: 200, ctype: "application/zip", body: b }, Err(_) => Resp::empty(500) }
        }
        ("POST", "/api/backup") => { let (ok, m) = backup_async(app); reply(ok, &m, vec![]) }
        ("POST", "/api/backup/delete") => {
            let name = gs(&body(), "name");
            let p = backup_dir(app).join(&name);
            if name.contains(['/', '\\']) || !name.ends_with(".zip") || !p.is_file() { return Some(reply(false, "Backup not found.", vec![])); }
            match fs::remove_file(&p) { Ok(_) => reply(true, "Deleted.", vec![]), Err(e) => reply(false, &e.to_string(), vec![]) }
        }
        ("GET", "/api/lists") => {
            let wl = read_props(&app.dir).into_iter().find(|(k, _)| k == "white-list").map(|x| x.1 == "true").unwrap_or(false);
            Resp::json(200, obj(vec![("whitelist", names(app, "whitelist.json")), ("ops", names(app, "ops.json")), ("whitelist_on", J::Bool(wl)), ("banned", names(app, "banned-players.json"))]).to_string())
        }
        ("POST", "/api/settings") => {
            let b = body(); let mut st = load_settings(&app.dir);
            let bl = |k: &str| match b.get(k) { Some(J::Bool(x)) => Some(*x), Some(J::Num(x)) => Some(*x != 0.0), Some(J::Str(t)) => Some(!t.is_empty()), Some(J::Null) => Some(false), _ => None };
            if let Some(v) = bl("auto_restart") { st.auto_restart = v; }
            if let Some(v) = bl("autostart_server") { st.autostart_server = v; }
            if let Some(v) = bl("update_check") { st.update_check = v; }
            if let Some(v) = bl("updates_lock") { st.updates_lock = v; }
            if let Some(J::Num(v)) = b.get("backup_keep") { st.backup_keep = (*v as i64).clamp(1, 100); }
            save_settings(&app.dir, &st);
            reply(true, "Saved.", vec![])
        }
        ("GET", "/api/skin") => {
            let name = req.q("name").unwrap_or("").to_string();
            if !name_ok(&name) { return Some(Resp::json(400, obj(vec![("ok", J::Bool(false)), ("error", s("Bad name.")), ("msg", s("Bad name."))]).to_string())); }
            match get_skin(&name) { Some(d) => Resp { code: 200, ctype: "image/png", body: d }, None => Resp::json(404, obj(vec![("ok", J::Bool(false)), ("error", s("No skin.")), ("msg", s("No skin."))]).to_string()) }
        }
        ("GET", "/api/bans") => {
            let pl = read_list(app, "banned-players.json").iter().filter(|b| !gs(b, "name").is_empty()).map(|b| obj(vec![("name", s(&gs(b, "name"))), ("reason", s(&gs(b, "reason"))), ("created", s(&gs(b, "created"))), ("source", s(&gs(b, "source")))])).collect();
            let ips = read_list(app, "banned-ips.json").iter().filter(|b| !gs(b, "ip").is_empty()).map(|b| obj(vec![("ip", s(&gs(b, "ip"))), ("reason", s(&gs(b, "reason")))])).collect();
            Resp::json(200, obj(vec![("players", J::Arr(pl)), ("ips", J::Arr(ips)), ("running", J::Bool(app.mc.running()))]).to_string())
        }
        ("POST", "/api/bans/ban") => {
            let b = body(); let name = gs(&b, "name").trim().to_string();
            let reason: String = gs(&b, "reason").replace(['\r', '\n'], " ").chars().take(100).collect();
            if !name_ok(&name) { return Some(reply(false, "That isn't a valid player name.", vec![])); }
            if !app.mc.running() { return Some(reply(false, "Start the server first. Bans need it running to look the player up.", vec![])); }
            app.mc.log(&format!("> ban {name}"));
            app.mc.send(format!("ban {name} {reason}").trim());
            reply(true, &format!("Banned {name}."), vec![])
        }
        ("POST", "/api/bans/unban") => {
            let b = body(); let name = gs(&b, "name").trim().to_string(); let ip = gs(&b, "ip").trim().to_string();
            let ip_ok = ip.len() >= 3 && ip.len() <= 45 && ip.chars().all(|c| c.is_ascii_hexdigit() || c == ':' || c == '.');
            if !name.is_empty() && name_ok(&name) {
                if app.mc.running() { app.mc.send(&format!("pardon {name}")); return Some(reply(true, &format!("Unbanned {name}."), vec![])); }
                let new: Vec<J> = read_list(app, "banned-players.json").into_iter().filter(|e| gs(e, "name").to_lowercase() != name.to_lowercase()).collect();
                return Some(match write_list(app, "banned-players.json", new) { Ok(_) => reply(true, &format!("Unbanned {name}."), vec![]), Err(e) => reply(false, &format!("Couldn't edit the ban list: {e}"), vec![]) });
            }
            if ip_ok {
                if app.mc.running() { app.mc.send(&format!("pardon-ip {ip}")); return Some(reply(true, &format!("Unbanned {ip}."), vec![])); }
                let new: Vec<J> = read_list(app, "banned-ips.json").into_iter().filter(|e| gs(e, "ip") != ip).collect();
                return Some(match write_list(app, "banned-ips.json", new) { Ok(_) => reply(true, &format!("Unbanned {ip}."), vec![]), Err(e) => reply(false, &format!("Couldn't edit the ban list: {e}"), vec![]) });
            }
            reply(false, "Nothing to unban.", vec![])
        }
        ("POST", "/api/world/do") => {
            let act = gs(&body(), "action");
            let Some(cmd) = WORLD.iter().find(|x| x.0 == act).map(|x| x.1) else { return Some(reply(false, "Unknown action.", vec![])); };
            if !app.mc.running() { return Some(reply(false, "The server isn't running.", vec![])); }
            app.mc.log(&format!("> {cmd}"));
            if app.mc.send(cmd) { reply(true, &format!("Done: {cmd}"), vec![]) } else { reply(false, "Couldn't send it.", vec![]) }
        }
        _ => return None,
    })
}
pub fn start(_app: &Arc<App>) {}
#[allow(dead_code)] fn _unused() { let _ = Duration::from_secs(1); }
pub fn backup_running() -> bool { BACKUP.lock().unwrap().running }
pub fn backup_ok() -> Option<bool> { BACKUP.lock().unwrap().ok }
