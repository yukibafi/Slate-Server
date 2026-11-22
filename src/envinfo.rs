//! Per-server settings file and loader / Minecraft version detection (ports of load_settings, detect_env).
use crate::json::{self, n, obj, s, J};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Instant;

pub fn vkey(v: &str) -> Vec<u64> {
    let mut out = vec![]; let mut cur = String::new();
    for c in v.chars() { if c.is_ascii_digit() { cur.push(c); } else if !cur.is_empty() { out.push(cur.parse().unwrap_or(0)); cur.clear(); } }
    if !cur.is_empty() { out.push(cur.parse().unwrap_or(0)); }
    out
}

#[derive(Clone, Debug)]
pub struct Settings { pub auto_restart: bool, pub backup_keep: i64, pub mc_version: String, pub loader: String, pub update_check: bool, pub autostart_server: bool, pub updates_lock: bool }
impl Default for Settings { fn default() -> Self { Settings { auto_restart: false, backup_keep: 5, mc_version: String::new(), loader: String::new(), update_check: true, autostart_server: false, updates_lock: false } } }
impl Settings {
    pub fn to_json(&self) -> J {
        obj(vec![("auto_restart", J::Bool(self.auto_restart)), ("backup_keep", n(self.backup_keep)), ("mc_version", s(&self.mc_version)), ("loader", s(&self.loader)), ("update_check", J::Bool(self.update_check)), ("autostart_server", J::Bool(self.autostart_server)), ("updates_lock", J::Bool(self.updates_lock))])
    }
}
pub fn settings_path(dir: &Path) -> PathBuf {
    match std::env::var("PANEL_SETTINGS_FILE") { Ok(p) if !p.is_empty() => PathBuf::from(p), _ => dir.join(".panel-settings.json") }
}
pub fn load_settings(dir: &Path) -> Settings {
    let mut st = Settings::default();
    if let Some(d) = fs::read_to_string(settings_path(dir)).ok().and_then(|t| json::parse(&t)) {
        let b = |k: &str, def: bool| match d.get(k) { Some(J::Bool(x)) => *x, Some(J::Num(x)) => *x != 0.0, _ => def };
        st.auto_restart = b("auto_restart", false); st.update_check = b("update_check", true); st.autostart_server = b("autostart_server", false); st.updates_lock = b("updates_lock", false);
        if let Some(J::Num(k)) = d.get("backup_keep") { st.backup_keep = (*k as i64).clamp(1, 100); }
        if let Some(mv) = d.get("mc_version").and_then(|v| v.as_str()) { let mv = mv.trim(); if !mv.is_empty() && mv.len() <= 24 && mv.chars().all(|c| c.is_ascii_alphanumeric() || ".-".contains(c)) { st.mc_version = mv.into(); } }
        if let Some(l) = d.get("loader").and_then(|v| v.as_str()) { let l = l.trim().to_lowercase(); if ["neoforge", "forge", "fabric", "quilt"].contains(&l.as_str()) { st.loader = l; } }
    }
    st
}
pub fn save_settings(dir: &Path, st: &Settings) -> bool {
    let p = settings_path(dir);
    // keep keys we don't know about (other modules may store theirs here)
    let mut m: Vec<(String, J)> = match fs::read_to_string(&p).ok().and_then(|t| json::parse(&t)) { Some(J::Obj(m)) => m, _ => vec![] };
    if let J::Obj(new) = st.to_json() { for (k, v) in new { if let Some(slot) = m.iter_mut().find(|(a, _)| *a == k) { slot.1 = v; } else { m.push((k, v)); } } }
    let tmp = PathBuf::from(format!("{}.tmp", p.display()));
    fs::write(&tmp, J::Obj(m).to_string()).and_then(|_| fs::rename(&tmp, &p)).is_ok()
}

fn newest_dir(d: &Path) -> Option<String> {
    let mut vs: Vec<String> = fs::read_dir(d).ok()?.flatten().filter(|e| e.path().is_dir()).map(|e| e.file_name().to_string_lossy().to_string()).filter(|n| n.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false)).collect();
    vs.sort_by_key(|v| vkey(v));
    vs.pop()
}
/// The NeoForge version start.sh (or run.sh) points at.
pub fn nf_script_version(dir: &Path) -> Option<String> {
    for f in ["start.sh", "run.sh"] {
        if let Ok(t) = fs::read_to_string(dir.join(f)) {
            if let Some(i) = t.find("net/neoforged/neoforge/") {
                let rest = &t[i + "net/neoforged/neoforge/".len()..];
                let v: String = rest.chars().take_while(|c| *c != '/' && !c.is_whitespace()).collect();
                if v.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false) { return Some(v); }
            }
        }
    }
    None
}
fn server_type_marker(dir: &Path) -> Option<(String, String, String)> {
    if let Some(o) = fs::read_to_string(dir.join(".server-type.json")).ok().and_then(|t| json::parse(&t)) {
        if let Some(t) = o.get("type").and_then(|v| v.as_str()) { if ["paper", "purpur", "vanilla"].contains(&t) { return Some((t.into(), o.get("mc").and_then(|v| v.as_str()).unwrap_or("").into(), o.get("build").and_then(|v| v.as_str()).unwrap_or("").into())); } }
    }
    if dir.join("purpur.yml").is_file() { return Some(("purpur".into(), String::new(), String::new())); }
    if dir.join("config/paper-global.yml").is_file() || dir.join("paper.yml").is_file() { return Some(("paper".into(), String::new(), String::new())); }
    None
}

/// (loader, minecraft version, label, loader version)
pub fn detect_env(dir: &Path, st: &Settings, ignore_override: bool) -> (Option<String>, Option<String>, Option<String>, Option<String>) {
    let (mut loader, mut mc, mut ver): (Option<String>, Option<String>, Option<String>) = (None, None, None);
    let base = dir.join("libraries");
    if let Some(mut v) = newest_dir(&base.join("net/neoforged/neoforge")) {
        if let Some(sv) = nf_script_version(dir) { if base.join("net/neoforged/neoforge").join(&sv).is_dir() { v = sv; } }
        loader = Some("neoforge".into()); ver = Some(v.clone());
        let k = vkey(&v);
        if k.len() >= 2 {
            if (19..=25).contains(&k[0]) { mc = Some(format!("1.{}{}", k[0], if k[1] == 0 { String::new() } else { format!(".{}", k[1]) })); }
            else if k[0] >= 26 { let pv: Vec<&str> = v.split('-').next().unwrap_or("").split('.').collect(); if pv.len() >= 2 { mc = Some(format!("{}.{}{}", pv[0], pv[1], if pv.len() < 3 || pv[2] == "0" { String::new() } else { format!(".{}", pv[2]) })); } }
        }
    }
    if loader.is_none() {
        if let Some(v) = newest_dir(&base.join("net/minecraftforge/forge")) {
            loader = Some("forge".into());
            if let Some((a, b)) = v.split_once('-') { mc = Some(a.into()); ver = Some(b.into()); } else { ver = Some(v); }
        }
    }
    if loader.is_none() {
        if base.join("org/quiltmc/quilt-loader").is_dir() { loader = Some("quilt".into()); }
        else if dir.join(".fabric").is_dir() || base.join("net/fabricmc").is_dir() { loader = Some("fabric".into()); }
        if loader.is_some() { if let Some(i) = newest_dir(&base.join("net/fabricmc/intermediary")) { mc = Some(i); } }
    }
    if loader.is_none() {
        if let Some((t, m, b)) = server_type_marker(dir) { loader = Some(t); if !m.is_empty() { mc = Some(m); } if !b.is_empty() { ver = Some(b); } }
    }
    if mc.is_none() {
        if let Ok(t) = fs::read_to_string(dir.join("logs/latest.log")) {
            for line in t.lines().take(400) { if let Some(i) = line.find("Starting minecraft server version ") { let v: String = line[i + 34..].chars().take_while(|c| !c.is_whitespace()).collect(); if !v.is_empty() { mc = Some(v); break; } } }
        }
    }
    if !ignore_override { if !st.loader.is_empty() { loader = Some(st.loader.clone()); } if !st.mc_version.is_empty() { mc = Some(st.mc_version.clone()); } }
    let names = [("neoforge", "NeoForge"), ("forge", "Forge"), ("fabric", "Fabric"), ("quilt", "Quilt"), ("paper", "Paper"), ("purpur", "Purpur"), ("vanilla", "Vanilla")];
    let label = match (&loader, &mc) {
        (Some(l), m) => { let nm = names.iter().find(|x| x.0 == l).map(|x| x.1).unwrap_or("Server"); let mut s_ = nm.to_string(); if let Some(v) = &ver { s_ += &format!(" {}", v); } if let Some(m) = m { s_ += &format!(", Minecraft {}", m); } Some(s_) }
        (None, Some(m)) => Some(format!("Minecraft {}", m)), _ => None,
    };
    (loader, mc, label, ver)
}

static CACHE: Mutex<Option<(Instant, String, (Option<String>, Option<String>, Option<String>, Option<String>))>> = Mutex::new(None);
/// Cached for 15 s like Python's env_cached().
pub fn env_cached(dir: &Path) -> (Option<String>, Option<String>, Option<String>, Option<String>) {
    let key = dir.display().to_string();
    let mut g = CACHE.lock().unwrap();
    if let Some((t, k, v)) = g.as_ref() { if *k == key && t.elapsed().as_secs() < 15 { return v.clone(); } }
    let v = detect_env(dir, &load_settings(dir), false);
    *g = Some((Instant::now(), key, v.clone()));
    v
}
pub fn invalidate() { *CACHE.lock().unwrap() = None; }
