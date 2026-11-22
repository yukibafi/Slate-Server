//! One-click updates. Looks at the newest GitHub release of this project and installs it with the same
//! checked, reversible path as "Update from a file". Needs `curl` (built into macOS and Windows 10+).
use crate::http::{Req, Resp};
use crate::json::{self, obj, s, J};
use crate::{sys, update, App};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;

pub const DEFAULT_REPO: &str = "yukibafi/Slate-Server";

#[derive(Clone, Default)]
pub struct Source { pub repo: String, pub token: String }

#[derive(Clone)]
pub struct Asset { pub name: String, pub url: String, pub api_url: String }

#[derive(Clone)]
pub struct Latest { pub version: String, pub notes: String, pub assets: Vec<Asset>, pub newer: bool }

fn file() -> PathBuf { crate::theme::data_dir().join("update-source.json") }

pub fn source() -> Source {
    let mut src = Source { repo: DEFAULT_REPO.into(), token: String::new() };
    if let Some(j) = std::fs::read_to_string(file()).ok().and_then(|t| json::parse(&t)) {
        if let Some(r) = j.get("repo").and_then(|x| x.as_str()) { if r.contains('/') && !r.trim().is_empty() { src.repo = r.trim().to_string(); } }
        if let Some(t) = j.get("token").and_then(|x| x.as_str()) { src.token = t.trim().to_string(); }
    }
    if let Ok(r) = std::env::var("CREATE_PANEL_REPO") { if r.contains('/') { src.repo = r; } }
    src
}

pub fn save_source(repo: &str, token: &str) -> Result<(), String> {
    let cur = source();
    let repo = if repo.trim().contains('/') { repo.trim().to_string() } else { cur.repo };
    let _ = std::fs::create_dir_all(crate::theme::data_dir());
    std::fs::write(file(), obj(vec![("repo", s(&repo)), ("token", s(token.trim()))]).to_string()).map_err(|e| e.to_string())
}

/// The name this computer's build goes by in release file names.
pub fn platform() -> Option<&'static str> {
    if let Ok(p) = std::env::var("CREATE_PANEL_PLATFORM") { return Some(Box::leak(p.into_boxed_str())); }
    match update::TARGET { "aarch64-apple-darwin" => Some("mac-apple-silicon"), "x86_64-apple-darwin" => Some("mac-intel"), "x86_64-pc-windows-msvc" => Some("windows"), _ => None }
}

fn text(j: &J, k: &str) -> String { j.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string() }

/// Ask GitHub for the newest release. `current` is the version of whatever is asking (engine or app).
pub fn latest(current: &str) -> Result<Latest, String> {
    let src = source();
    let base = std::env::var("CREATE_PANEL_API").unwrap_or_else(|_| "https://api.github.com".into());
    let url = format!("{}/repos/{}/releases/latest", base, src.repo);
    let auth = format!("Bearer {}", src.token);
    let mut h: Vec<(&str, &str)> = vec![("User-Agent", "create-panel-rs"), ("Accept", "application/vnd.github+json")];
    if !src.token.is_empty() { h.push(("Authorization", &auth)); }
    let (code, body) = sys::http("GET", &url, &h, None, 25);
    match code {
        0 => return Err("Couldn't reach GitHub. Check the internet connection.".into()),
        200 => {}
        404 => return Err(format!("No release found for {} yet. Either the build hasn't finished, or the repository is private (then add a GitHub token below).", src.repo)),
        401 => return Err("GitHub didn't accept the token. Paste a new one below.".into()),
        403 => return Err("GitHub is limiting requests right now. Try again in a few minutes.".into()),
        c => return Err(format!("GitHub answered {c}.")),
    }
    let j = json::parse(&String::from_utf8_lossy(&body)).ok_or("GitHub's answer wasn't readable.")?;
    let tag = text(&j, "tag_name");
    let version = tag.trim_start_matches('v').to_string();
    let mut assets = vec![];
    if let Some(J::Arr(a)) = j.get("assets") {
        for x in a { assets.push(Asset { name: text(x, "name"), url: text(x, "browser_download_url"), api_url: text(x, "url") }); }
    }
    let newer = match (update::parse_ver(&version), update::parse_ver(current)) { (Some(n), Some(c)) => n > c, _ => false };
    Ok(Latest { version, notes: text(&j, "body"), assets, newer })
}

/// The release file for this computer: `prefix` is "create-panel-update-" (engine plus app) or "create-panel-app-" (app only).
pub fn pick<'a>(l: &'a Latest, prefix: &str) -> Option<&'a Asset> {
    let p = platform()?;
    l.assets.iter().find(|a| a.name.starts_with(prefix) && a.name.contains(p))
}

pub fn download(a: &Asset, dest: &Path) -> Result<(), String> {
    let src = source();
    let tmp = dest.with_extension("part");
    let mut args: Vec<String> = ["-sS", "-L", "-f", "--max-time", "900", "-A", "create-panel-rs"].iter().map(|x| x.to_string()).collect();
    let url = if src.token.is_empty() { a.url.clone() } else {
        args.extend(["-H".to_string(), format!("Authorization: Bearer {}", src.token), "-H".to_string(), "Accept: application/octet-stream".to_string()]);
        a.api_url.clone()
    };
    args.extend(["-o".to_string(), tmp.to_string_lossy().to_string(), url]);
    let ok = Command::new("curl").args(&args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status().map(|s| s.success()).unwrap_or(false);
    if !ok { let _ = std::fs::remove_file(&tmp); return Err("The download didn't finish. Check the internet connection and try again.".into()); }
    std::fs::rename(&tmp, dest).map_err(|e| e.to_string())
}

// ---------------------------------------------------------------- engine side
static STATUS: Mutex<(String, String)> = Mutex::new((String::new(), String::new()));
fn set_status(state: &str, msg: &str) { *STATUS.lock().unwrap() = (state.to_string(), msg.to_string()); }

fn ok(v: Vec<(&str, J)>) -> Resp { let mut a = vec![("ok", J::Bool(true))]; a.extend(v); Resp::json(200, obj(a).to_string()) }
fn bad(msg: &str) -> Resp { Resp::json(200, obj(vec![("ok", J::Bool(false)), ("error", s(msg)), ("msg", s(msg))]).to_string()) }

pub fn check_resp() -> Resp {
    match latest(update::VERSION) {
        Ok(l) => {
            let has = pick(&l, "create-panel-update-").is_some();
            ok(vec![("current", s(update::VERSION)), ("latest", s(&l.version)), ("newer", J::Bool(l.newer && has)), ("notes", s(&l.notes)), ("file_missing", J::Bool(l.newer && !has)), ("repo", s(&source().repo))])
        }
        Err(e) => bad(&e),
    }
}

pub fn status_resp() -> Resp { let g = STATUS.lock().unwrap().clone(); ok(vec![("state", s(&g.0)), ("msg", s(&g.1))]) }

pub fn install_resp(app: &Arc<App>) -> Resp {
    { let g = STATUS.lock().unwrap(); if g.0 == "downloading" || g.0 == "installing" { return bad("An update is already in progress."); } }
    set_status("downloading", "Downloading the new version...");
    let app = app.clone();
    thread::spawn(move || {
        let l = match latest(update::VERSION) { Ok(l) => l, Err(e) => return set_status("error", &e) };
        if !l.newer { return set_status("error", &format!("Already on the newest version ({}).", update::VERSION)); }
        let Some(a) = pick(&l, "create-panel-update-") else { return set_status("error", "The new release has no file for this computer yet.") };
        let dest = crate::theme::data_dir().join("incoming-update.bin");
        let _ = std::fs::create_dir_all(crate::theme::data_dir());
        if let Err(e) = download(a, &dest) { return set_status("error", &e); }
        let bytes = std::fs::read(&dest).unwrap_or_default();
        let _ = std::fs::remove_file(&dest);
        set_status("installing", &format!("Installing {}...", l.version));
        let r = update::install(&app, &bytes, false, false);
        if r.code != 200 {
            let t = String::from_utf8_lossy(&r.body).to_string();
            let m = json::parse(&t).and_then(|j| j.get("msg").and_then(|x| x.as_str().map(|y| y.to_string()))).unwrap_or(t);
            set_status("error", &m);
        }
    });
    ok(vec![("msg", s("Started."))])
}

pub fn source_resp(req: &Req) -> Resp {
    let j = json::parse(&String::from_utf8_lossy(&req.body)).unwrap_or(J::Null);
    let cur = source();
    let token = j.get("token").and_then(|x| x.as_str()).map(|t| t.to_string()).unwrap_or(cur.token);
    let repo = j.get("repo").and_then(|x| x.as_str()).map(|t| t.to_string()).unwrap_or(cur.repo);
    match save_source(&repo, &token) { Ok(()) => ok(vec![("msg", s("Saved."))]), Err(e) => bad(&e) }
}
