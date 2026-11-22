//! Hub: world safety net (automatic backups of every server), full hub backup / restore ("move to another computer"),
//! and world import (upload a zip, or point at a folder/zip on this computer).
use crate::client::Client;
use crate::http::{Req, Resp};
use crate::hub::Entry;
use crate::json::{self, n, obj, s, J};
use crate::{hubc, jobs, sys, sysx, App};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

struct Bk { busy: bool, msg: String, last_run: i64, last: HashMap<String, i64>, had: HashMap<String, bool>, t0: i64 }
static BK: Mutex<Option<Bk>> = Mutex::new(None);
static MIG_LAST: Mutex<String> = Mutex::new(String::new());

fn st<R>(f: impl FnOnce(&mut Bk) -> R) -> R { let mut g = BK.lock().unwrap(); f(g.get_or_insert_with(|| Bk { busy: false, msg: String::new(), last_run: 0, last: HashMap::new(), had: HashMap::new(), t0: sys::now() as i64 })) }
fn bk_root() -> PathBuf { hubc::hub_root().join("world-backups") }
fn cfg_file() -> PathBuf { hubc::hub_root().join(".hub-backup.json") }
fn slug(t: &str) -> String { let r: String = t.chars().map(|c| if c.is_ascii_alphanumeric() || "._-".contains(c) { c } else { '_' }).collect(); let r = r.trim_matches('_').chars().take(60).collect::<String>(); if r.is_empty() { "server".into() } else { r } }
fn stamp() -> String { let t = sys::now() as i64; let (d, rem) = (t.div_euclid(86400), t.rem_euclid(86400)); let (y, m, dd) = civil(d); format!("{y:04}-{m:02}-{dd:02}_{:02}-{:02}-{:02}", rem / 3600, rem % 3600 / 60, rem % 60) }
fn civil(z: i64) -> (i64, i64, i64) { let z = z + 719468; let era = z.div_euclid(146097); let doe = z.rem_euclid(146097); let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365; let y = yoe + era * 400; let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); let mp = (5 * doy + 2) / 153; let d = doy - (153 * mp + 2) / 5 + 1; let m = if mp < 10 { mp + 3 } else { mp - 9 }; (if m <= 2 { y + 1 } else { y }, m, d) }

#[derive(Clone, Copy)]
struct Cfg { auto: bool, hours: i64, keep: i64 }
fn cfg() -> Cfg {
    let j = fs::read_to_string(cfg_file()).ok().and_then(|t| json::parse(&t)).unwrap_or(J::Null);
    let num = |k: &str, d: i64, lo: i64, hi: i64| match j.get(k) { Some(J::Num(x)) => (*x as i64).clamp(lo, hi), _ => d };
    Cfg { auto: !matches!(j.get("auto"), Some(J::Bool(false))) && !matches!(j.get("auto"), Some(J::Num(x)) if *x == 0.0), hours: num("hours", 6, 1, 168), keep: num("keep", 8, 1, 100) }
}
fn cfg_json(c: Cfg) -> J { obj(vec![("auto", n(c.auto as i64)), ("hours", n(c.hours)), ("keep", n(c.keep))]) }

fn find_worlds(dir: &str) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = fs::read_dir(dir).map(|r| r.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.join("level.dat").is_file()).collect()).unwrap_or_default();
    v.sort(); v
}
fn newest_mtime(paths: &[PathBuf]) -> i64 {
    fn walk(p: &Path, m: &mut i64) { if let Ok(rd) = fs::read_dir(p) { for e in rd.flatten() { let q = e.path(); if q.is_dir() { walk(&q, m); } else if let Ok(md) = e.metadata() { if let Ok(t) = md.modified() { *m = (*m).max(t.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)); } } } } }
    let mut m = 0; for p in paths { walk(p, &mut m); } m
}
fn zip_world(world: &Path, dest: &Path) -> Result<PathBuf, String> {
    fs::create_dir_all(dest).map_err(|e| e.to_string())?;
    let name = world.file_name().map(|x| x.to_string_lossy().to_string()).unwrap_or_else(|| "world".into());
    let out = dest.join(format!("{}_{}.zip", name, stamp()));
    let part = dest.join(format!("{}.part", out.file_name().unwrap().to_string_lossy()));
    sys::zip_folder(world.parent().unwrap_or(Path::new(".")), &name, &part, &["*/session.lock", "*session.lock"])?;
    fs::rename(&part, &out).map_err(|e| e.to_string())?;
    Ok(out)
}
fn rotate(dir: &Path, keep: usize) {
    let mut fs_: Vec<(PathBuf, std::time::SystemTime)> = fs::read_dir(dir).map(|r| r.flatten().filter(|e| e.file_name().to_string_lossy().ends_with(".zip")).filter_map(|e| Some((e.path(), e.metadata().ok()?.modified().ok()?))).collect()).unwrap_or_default();
    fs_.sort_by(|a, b| b.1.cmp(&a.1));
    for (p, _) in fs_.into_iter().skip(keep) { let _ = fs::remove_file(p); }
}
/// Instant backup of every world in an instance that is about to be removed (kept outside the instance).
pub fn snapshot_deleted(e: &Entry) -> Option<PathBuf> {
    let worlds = find_worlds(&e.dir);
    let pb = PathBuf::from(&e.dir).join("panel-backups");
    let own: Vec<PathBuf> = fs::read_dir(&pb).map(|r| r.flatten().map(|x| x.path()).filter(|p| p.extension().map(|x| x == "zip").unwrap_or(false)).collect()).unwrap_or_default();
    if worlds.is_empty() && own.is_empty() { return None; }
    let dest = bk_root().join("deleted").join(format!("{}_{}", slug(&e.name), stamp()));
    let _ = fs::create_dir_all(&dest);
    for w in &worlds { let _ = zip_world(w, &dest); }
    for f in own { let _ = fs::copy(&f, dest.join(format!("earlier_{}", f.file_name().unwrap().to_string_lossy()))); }
    Some(dest)
}

fn child_backup_wait(app: &App, e: &Entry) -> bool {
    let c = Client::new("127.0.0.1", e.port, &app.key);
    if !matches!(c.post("/api/backup", &obj(vec![])), Ok(j) if matches!(j.get("ok"), Some(J::Bool(true)))) { return false; }
    thread::sleep(Duration::from_secs(2));
    for _ in 0..600 {
        if let Ok(j) = c.get("/api/status") { let b = j.get("backup").cloned().unwrap_or(J::Null); if !matches!(b.get("running"), Some(J::Bool(true))) { return matches!(b.get("ok"), Some(J::Bool(true))); } }
        thread::sleep(Duration::from_secs(3));
    }
    false
}
fn listdir_zips(d: &Path) -> Vec<String> { fs::read_dir(d).map(|r| r.flatten().map(|e| e.file_name().to_string_lossy().to_string()).filter(|n| n.ends_with(".zip")).collect()).unwrap_or_default() }

fn bk_pass(app: &Arc<App>, manual: bool) {
    if st(|b| { if b.busy { true } else { b.busy = true; false } }) { return; }
    let hub = app.hub.as_ref().unwrap().clone(); let c = cfg(); let mut done = 0;
    for e in hub.list() {
        let worlds = find_worlds(&e.dir); if worlds.is_empty() { continue; }
        let state = hub.state_of(&e.id); if state == "starting" || state == "stopping" { continue; }
        let dest = bk_root().join("auto").join(slug(&e.id));
        st(|b| b.msg = format!("Backing up {}...", e.name));
        if state == "online" {
            let pl = hub.has_players(&e.id);
            let had = st(|b| *b.had.get(&e.id).unwrap_or(&false));
            if !manual && !pl && !had { continue; }
            st(|b| { b.had.insert(e.id.clone(), pl); });
            let pb = PathBuf::from(&e.dir).join("panel-backups"); let before = listdir_zips(&pb);
            if !child_backup_wait(app, &e) { continue; }
            let new: Vec<String> = listdir_zips(&pb).into_iter().filter(|f| !before.contains(f)).collect();
            if new.is_empty() { continue; }
            let _ = fs::create_dir_all(&dest);
            for f in new { let _ = fs::copy(pb.join(&f), dest.join(&f)); }
        } else {
            if !manual && newest_mtime(&worlds) <= st(|b| *b.last.get(&e.id).unwrap_or(&0)) { continue; }
            let mut ok = true; for w in &worlds { if zip_world(w, &dest).is_err() { ok = false; } } if !ok { continue; }
        }
        st(|b| { b.last.insert(e.id.clone(), sys::now() as i64); });
        rotate(&dest, (c.keep as usize) * worlds.len().max(1));
        done += 1;
    }
    st(|b| { b.last_run = sys::now() as i64; b.busy = false; b.msg = if done > 0 { format!("Backed up {done} server{}.", if done == 1 { "" } else { "s" }) } else { "Nothing needed backing up.".into() }; });
}

fn walk_zips(base: &Path, out: &mut Vec<PathBuf>) { if let Ok(rd) = fs::read_dir(base) { for e in rd.flatten() { let p = e.path(); if p.is_dir() { walk_zips(&p, out); } else if p.extension().map(|x| x == "zip").unwrap_or(false) { out.push(p); } } } }
fn bk_list(app: &App) -> J {
    let hub = app.hub.as_ref().unwrap(); let root = bk_root(); let mut items: Vec<(i64, J)> = vec![];
    for kind in ["auto", "deleted"] {
        let mut zs = vec![]; walk_zips(&root.join(kind), &mut zs);
        for p in zs {
            let Ok(md) = fs::metadata(&p) else { continue };
            let mt = md.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs() as i64).unwrap_or(0);
            let rel = p.strip_prefix(&root).unwrap_or(&p).to_string_lossy().replace('\\', "/");
            let parts: Vec<&str> = rel.split('/').collect();
            let who = if parts.len() > 2 { parts[1].to_string() } else { String::new() };
            let (who, gone) = if kind == "auto" { match hub.get(&who) { Some(e) => (e.name, false), None => (who.replace('-', " "), true) } } else { (who.rsplitn(3, '_').last().unwrap_or(&who).to_string(), true) };
            items.push((mt, obj(vec![("rel", s(&rel)), ("kind", s(kind)), ("server", s(&who)), ("gone", J::Bool(gone)), ("name", s(&p.file_name().unwrap().to_string_lossy())), ("size", n(md.len() as i64)), ("mtime", n(mt))])));
        }
    }
    items.sort_by(|a, b| b.0.cmp(&a.0));
    let (busy, msg, last) = st(|b| (b.busy, b.msg.clone(), b.last_run));
    obj(vec![("cfg", cfg_json(cfg())), ("items", J::Arr(items.into_iter().map(|x| x.1).collect())), ("busy", J::Bool(busy)), ("msg", s(&msg)), ("last_run", n(last)), ("free", sys::free_space(&hubc::hub_root()).map(|x| n(x as i64)).unwrap_or(J::Null)), ("root", s(&root.display().to_string()))])
}
fn bk_path(rel: &str) -> Option<PathBuf> {
    if rel.contains("..") { return None; }
    let p = bk_root().join(rel.replace('/', std::path::MAIN_SEPARATOR_STR));
    let (Ok(rp), Ok(rr)) = (fs::canonicalize(&p), fs::canonicalize(bk_root())) else { return None };
    if rp.starts_with(&rr) && rp.is_file() && rp.extension().map(|x| x == "zip").unwrap_or(false) { Some(rp) } else { None }
}
fn open_folder(p: &Path) {
    #[cfg(target_os = "macos")] let _ = std::process::Command::new("open").arg(p).spawn();
    #[cfg(windows)] let _ = std::process::Command::new("explorer").arg(p).spawn();
    #[cfg(all(unix, not(target_os = "macos")))] let _ = std::process::Command::new("xdg-open").arg(p).spawn();
}

// ---- full hub backup (servers + optionally their backups) as one zip, and restore from it
fn stage_dir() -> PathBuf { hubc::hub_root().join(".migrate-stage") }
fn mig_backup(app: Arc<App>, job: jobs::Job, stop: bool, with_backups: bool) {
    let hub = app.hub.as_ref().unwrap().clone();
    let res = (|| -> Result<String, String> {
        let entries = hub.list(); if entries.is_empty() { return Err("There are no servers to back up yet.".into()); }
        let running: Vec<Entry> = entries.iter().filter(|e| matches!(hub.state_of(&e.id).as_str(), "starting" | "online" | "stopping")).cloned().collect();
        if !running.is_empty() {
            if !stop { return Err("Some servers are running. Stop them first, or tick \"Stop running servers first\", so the worlds are saved cleanly.".into()); }
            jobs::step(&job, "Stopping running servers (worlds are saved first)...", 2);
            for e in &running { hub.action(&e.id, "stop"); }
            for _ in 0..80 { if running.iter().all(|e| matches!(hub.state_of(&e.id).as_str(), "stopped" | "panel_off")) { break; } thread::sleep(Duration::from_secs(3)); }
        }
        let stage = stage_dir(); let _ = fs::remove_dir_all(&stage);
        let files = stage.join("hub-backup").join("files"); fs::create_dir_all(&files).map_err(|e| e.to_string())?;
        let mut man = vec![]; let total = entries.len();
        for (i, e) in entries.iter().enumerate() {
            jobs::step(&job, &format!("Copying {} ({} of {total})...", e.name, i + 1), 5 + (80 * i / total) as i64);
            let folder = format!("{}-{}", slug(&e.name), e.id);
            copy_tree(Path::new(&e.dir), &files.join(&folder), with_backups).map_err(|x| format!("Couldn't copy {}: {x}", e.name))?;
            man.push(obj(vec![("id", s(&e.id)), ("name", s(&e.name)), ("folder", s(&folder)), ("port", n(e.port as i64))]));
        }
        fs::write(stage.join("hub-backup").join("manifest.json"), obj(vec![("created", n(sys::now() as i64)), ("panel_version", s(crate::update::VERSION)), ("servers", J::Arr(man))]).to_string()).map_err(|e| e.to_string())?;
        jobs::step(&job, "Zipping everything (this takes a while for big worlds)...", 88);
        let desk = sysx::home().join("Desktop"); let out_dir = if desk.is_dir() { desk } else { sysx::home() };
        let out = out_dir.join(format!("Server Hub Backup {}.zip", stamp()));
        sys::zip_dir(&stage, &out, &[]).map_err(|e| format!("Couldn't make the zip: {e}"))?;
        let _ = fs::remove_dir_all(&stage);
        *MIG_LAST.lock().unwrap() = out.display().to_string();
        Ok(format!("Saved to {}", out.display()))
    })();
    match res { Ok(m) => { jobs::step(&job, "Done", 100); jobs::finish(&job, true, &m); } Err(e) => { let _ = fs::remove_dir_all(stage_dir()); jobs::finish(&job, false, &e); } }
}
fn copy_tree(src: &Path, dst: &Path, with_backups: bool) -> std::io::Result<()> {
    fs::create_dir_all(dst)?;
    for e in fs::read_dir(src)?.flatten() {
        let name = e.file_name().to_string_lossy().to_string(); let p = e.path();
        if name == "session.lock" || name == ".DS_Store" || name.ends_with(".part") || (!with_backups && name == "panel-backups") || name == ".world-import" { continue; }
        let ft = e.file_type()?; if ft.is_symlink() { continue; }
        if ft.is_dir() { copy_tree(&p, &dst.join(&name), with_backups)?; } else { fs::copy(&p, dst.join(&name))?; }
    }
    Ok(())
}
pub fn find_bundles() -> Vec<J> {
    let h = sysx::home(); let mut out = vec![];
    for base in ["Downloads", "Desktop", "Documents", ""] {
        let b = if base.is_empty() { h.clone() } else { h.join(base) };
        let Ok(rd) = fs::read_dir(&b) else { continue };
        for e in rd.flatten() {
            let nm = e.file_name().to_string_lossy().to_string();
            if nm.starts_with("Server Hub Backup") && (nm.ends_with(".zip") || e.path().is_dir()) {
                let md = e.metadata().ok(); let size = md.as_ref().map(|m| m.len() as i64).unwrap_or(0);
                out.push(obj(vec![("path", s(&e.path().display().to_string())), ("name", s(&nm)), ("size", n(size))]));
            }
        }
    }
    out
}
fn mig_restore(app: Arc<App>, job: jobs::Job, src: PathBuf) {
    let hub = app.hub.as_ref().unwrap().clone();
    let res = (|| -> Result<String, String> {
        let mut root = src.clone(); let mut tmp: Option<PathBuf> = None;
        if src.is_file() {
            jobs::step(&job, "Unzipping the backup...", 5);
            let t = hubc::hub_root().join(".migrate-restore"); let _ = fs::remove_dir_all(&t);
            sys::unzip_to(&src, &t)?; root = t.clone(); tmp = Some(t);
        }
        let man = fs::read_to_string(root.join("hub-backup").join("manifest.json")).ok().and_then(|t| json::parse(&t)).ok_or("That doesn't look like a Server Hub Backup.")?;
        let svs = match man.get("servers") { Some(J::Arr(a)) => a.clone(), _ => vec![] };
        if svs.is_empty() { return Err("There are no servers in that backup.".into()); }
        let existing = hub.list(); let mut taken: Vec<i64> = existing.iter().map(|e| e.port as i64).collect();
        let (_, mc_taken) = hub.taken_ports(); taken.extend(mc_taken);
        let mut added = 0; let total = svs.len();
        for (i, sv) in svs.iter().enumerate() {
            let name = sv.get("name").and_then(|x| x.as_str()).unwrap_or("Server").to_string(); let folder = sv.get("folder").and_then(|x| x.as_str()).unwrap_or("");
            let sd = root.join("hub-backup").join("files").join(folder);
            if folder.contains("..") || !sd.is_dir() { continue; }
            jobs::step(&job, &format!("Restoring {name} ({} of {total})...", i + 1), 10 + (85 * i / total) as i64);
            let mut nm = name.clone(); let mut k = 2; while existing.iter().any(|e| e.name.eq_ignore_ascii_case(&nm)) { nm = format!("{name} {k}"); k += 1; }
            let dest = hubc::hub_root().join(format!("{}-{}", slug(&nm), &jobs::new_id()[..4]));
            copy_tree(&sd, &dest, true).map_err(|e| format!("Couldn't restore {name}: {e}"))?;
            // the old Minecraft port may be taken here: give the restored server a free one if so
            let pp = dest.join("server.properties");
            let old = crate::hub::props_port(&dest.display().to_string());
            if old > 0 && taken.contains(&old) { for p in (old + 1)..(old + 200) { if !taken.contains(&p) { hubc::set_props(&pp, &[("server-port", p.to_string())]); taken.push(p); break; } } } else if old > 0 { taken.push(old); }
            let _ = hub.add(&dest.display().to_string(), &nm).map(|_| ()); added += 1;
        }
        if let Some(t) = tmp { let _ = fs::remove_dir_all(t); }
        if added == 0 { return Err("Nothing could be restored from that backup.".into()); }
        Ok(format!("Restored {added} server{}. They're on the Servers page now. Start each one when you're ready.", if added == 1 { "" } else { "s" }))
    })();
    match res { Ok(m) => { jobs::step(&job, "Done", 100); jobs::finish(&job, true, &m); } Err(e) => jobs::finish(&job, false, &e) }
}

// ---- world import
fn import_dir(e: &Entry) -> PathBuf { PathBuf::from(&e.dir).join(".world-import") }
fn world_commit(app: Arc<App>, job: jobs::Job, e: Entry, local: Option<PathBuf>) {
    let _ = &app;
    let res = (|| -> Result<String, String> {
        let stg = import_dir(&e);
        let mut search = stg.clone();
        if let Some(p) = &local {
            if p.is_dir() { search = p.clone(); }
            else if p.is_file() { let _ = fs::remove_dir_all(&stg); jobs::step(&job, "Unzipping...", 10); sys::unzip_to(p, &stg)?; }
            else { return Err("I can't find that path on this computer.".into()); }
        } else {
            let zips: Vec<PathBuf> = fs::read_dir(&stg).map(|r| r.flatten().map(|x| x.path()).filter(|p| p.extension().map(|x| x == "zip").unwrap_or(false)).collect()).unwrap_or_default();
            if let Some(z) = zips.first() { jobs::step(&job, "Unzipping...", 10); let out = stg.join("unzipped"); let _ = fs::remove_dir_all(&out); sys::unzip_to(z, &out)?; search = out; }
        }
        // shallowest folder holding level.dat
        let mut best: Option<(usize, PathBuf)> = None;
        fn walk(d: &Path, depth: usize, best: &mut Option<(usize, PathBuf)>) { if depth > 6 { return; } if d.join("level.dat").is_file() && best.as_ref().map(|b| depth < b.0).unwrap_or(true) { *best = Some((depth, d.to_path_buf())); } if let Ok(rd) = fs::read_dir(d) { for x in rd.flatten() { let p = x.path(); if p.is_dir() && x.file_name().to_string_lossy() != "__MACOSX" { walk(&p, depth + 1, best); } } } }
        walk(&search, 0, &mut best);
        let (_, found) = best.ok_or("I couldn't find a level.dat in that. A world folder has level.dat and a region folder inside it. Pick the folder (or a .zip of it) that contains them.")?;
        jobs::step(&job, "Putting the world in place...", 75);
        let lvl: String = { let l = crate::hub::props_get(&e.dir, "level-name"); let l: String = l.chars().map(|c| if c.is_ascii_alphanumeric() || " _.-".contains(c) { c } else { '_' }).collect(); if l.is_empty() { "world".into() } else { l } };
        let target = PathBuf::from(&e.dir).join(&lvl); let mut kept = String::new();
        if target.exists() { kept = format!("{lvl}-before-import-{}", sys::now() as i64); fs::rename(&target, PathBuf::from(&e.dir).join(&kept)).map_err(|x| x.to_string())?; }
        if fs::rename(&found, &target).is_err() { copy_tree(&found, &target, true).map_err(|x| x.to_string())?; }
        let _ = fs::remove_file(target.join("session.lock")); let _ = fs::remove_dir_all(&stg);
        Ok(format!("Your world is in place. Press Start.{}", if kept.is_empty() { String::new() } else { format!(" The old world was kept as \"{kept}\".") }))
    })();
    match res { Ok(m) => { jobs::step(&job, "Done", 100); jobs::finish(&job, true, &m); } Err(e2) => { let _ = fs::remove_dir_all(import_dir(&e)); jobs::finish(&job, false, &e2); } }
}

pub fn start(app: &Arc<App>) {
    let a = app.clone();
    thread::spawn(move || {
        thread::sleep(Duration::from_secs(120));
        loop {
            let c = cfg(); let (last, t0) = st(|b| (b.last_run, b.t0));
            if c.auto && sys::now() as i64 - if last > 0 { last } else { t0 } >= c.hours * 3600 { bk_pass(&a, false); }
            thread::sleep(Duration::from_secs(60));
        }
    });
}

pub fn handle(app: &Arc<App>, req: &Req) -> Option<Resp> {
    let hub = app.hub.as_ref()?.clone();
    let body = || json::parse(&String::from_utf8_lossy(&req.body)).unwrap_or(J::Null);
    let rep = |ok: bool, m: &str, extra: Vec<(&str, J)>| { let mut v = vec![("ok", J::Bool(ok)), ("msg", s(m))]; v.extend(extra); Resp::json(200, obj(v).to_string()) };
    let gs = |j: &J, k: &str| j.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
    let gb = |j: &J, k: &str, d: bool| match j.get(k) { Some(J::Bool(b)) => *b, Some(J::Num(x)) => *x != 0.0, _ => d };
    Some(match (req.method.as_str(), req.path.as_str()) {
        ("GET", "/api/hub/backups") => Resp::json(200, bk_list(app).to_string()),
        ("POST", "/api/hub/backups/config") => {
            let b = body(); let mut c = cfg();
            if b.get("auto").is_some() { c.auto = gb(&b, "auto", true); }
            if let Some(J::Num(x)) = b.get("hours") { c.hours = (*x as i64).clamp(1, 168); }
            if let Some(J::Num(x)) = b.get("keep") { c.keep = (*x as i64).clamp(1, 100); }
            let _ = fs::create_dir_all(hubc::hub_root()); let _ = fs::write(cfg_file(), cfg_json(c).to_string());
            rep(true, "Saved.", vec![("cfg", cfg_json(c))])
        }
        ("POST", "/api/hub/backups/run") => {
            if st(|b| b.busy) { return Some(rep(false, "A backup is already running.", vec![])); }
            let a = app.clone(); thread::spawn(move || bk_pass(&a, true)); rep(true, "Backing up every server...", vec![])
        }
        ("POST", "/api/hub/backups/delete") => match bk_path(&gs(&body(), "rel")) {
            Some(p) => { let ok = fs::remove_file(&p).is_ok(); if let Some(d) = p.parent() { if fs::canonicalize(d).ok() != fs::canonicalize(bk_root()).ok() { let _ = fs::remove_dir(d); } } if ok { rep(true, "Deleted.", vec![]) } else { rep(false, "Couldn't delete it.", vec![]) } }
            None => rep(false, "Backup not found.", vec![]),
        },
        ("POST", "/api/hub/backups/restore") => match bk_path(&gs(&body(), "rel")) {
            Some(p) => {
                let out = hubc::hub_root().join("restored-worlds").join(format!("{}_{}", p.file_stem().unwrap().to_string_lossy(), &jobs::new_id()[..6]));
                match sys::unzip_to(&p, &out) { Ok(_) => { open_folder(&out); rep(true, &format!("Unpacked to {}. To use it, open the server, go to Settings and choose Import world, then pick that folder (or the backup zip).", out.display()), vec![("path", s(&out.display().to_string()))]) } Err(e) => rep(false, &format!("Couldn't unpack it: {e}"), vec![]) }
            }
            None => rep(false, "Backup not found.", vec![]),
        },
        ("GET", "/api/hub/backups/download") => match bk_path(req.q("rel").unwrap_or("")) {
            Some(p) => match fs::read(&p) { Ok(b) => Resp { code: 200, ctype: "application/zip", body: b }, Err(_) => Resp::empty(500) },
            None => Resp::empty(404),
        },
        ("POST", "/api/hub/migrate/backup") => {
            let b = body(); let (id, job) = jobs::create("Full backup", true);
            let a = app.clone(); let (stop, wb) = (gb(&b, "stop", true), gb(&b, "backups", true));
            thread::spawn(move || mig_backup(a, job, stop, wb)); rep(true, "Backing up...", vec![("job", s(&id))])
        }
        ("GET", "/api/hub/migrate/last") => { let p = MIG_LAST.lock().unwrap().clone(); Resp::json(200, obj(vec![("path", s(&p)), ("exists", J::Bool(Path::new(&p).is_file()))]).to_string()) }
        ("GET", "/api/hub/migrate/download") => { let p = MIG_LAST.lock().unwrap().clone(); match fs::read(&p) { Ok(b) => Resp { code: 200, ctype: "application/zip", body: b }, Err(_) => Resp::empty(404) } }
        ("GET", "/api/hub/migrate/find") => Resp::json(200, obj(vec![("bundles", J::Arr(find_bundles()))]).to_string()),
        ("POST", "/api/hub/migrate/restore") => {
            let path = gs(&body(), "path"); if path.is_empty() { return Some(rep(false, "Pick a backup first.", vec![])); }
            let (id, job) = jobs::create("Restore", true); let a = app.clone();
            thread::spawn(move || mig_restore(a, job, PathBuf::from(path))); rep(true, "Restoring...", vec![("job", s(&id))])
        }
        ("POST", "/api/hub/world/upload") => {
            let Some(e) = hub.get(req.q("id").unwrap_or("")) else { return Some(rep(false, "That server isn't in the list.", vec![])) };
            if hub.state_of(&e.id) != "stopped" { return Some(rep(false, "Stop the server first.", vec![])); }
            let fname = req.header("x-filename").unwrap_or("world.zip"); if !fname.to_lowercase().ends_with(".zip") { return Some(rep(false, "Pick a .zip of the world folder.", vec![])); }
            let d = import_dir(&e); let _ = fs::remove_dir_all(&d); let _ = fs::create_dir_all(&d);
            match fs::write(d.join("world.zip"), &req.body) { Ok(_) => rep(true, "Uploaded.", vec![]), Err(x) => rep(false, &format!("Couldn't save it: {x}"), vec![]) }
        }
        ("POST", "/api/hub/world/commit") => {
            let b = body(); let Some(e) = hub.get(&gs(&b, "id")) else { return Some(rep(false, "That server isn't in the list any more.", vec![])) };
            if hub.state_of(&e.id) != "stopped" { return Some(rep(false, "Stop the server first.", vec![])); }
            let local = { let p = gs(&b, "path"); if p.is_empty() { None } else { Some(PathBuf::from(p)) } };
            let (id, job) = jobs::create(&e.name, true); let a = app.clone();
            thread::spawn(move || world_commit(a, job, e, local)); rep(true, "Working...", vec![("job", s(&id))])
        }
        _ => return None,
    })
}
