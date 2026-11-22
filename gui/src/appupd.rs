//! One-click updates from the app. Finds the newest release on GitHub, updates the engine (this Mac, or the
//! Mac you are connected to) and, on Windows, replaces this app window too, so nothing is reinstalled by hand.
use crate::*;
use create_panel::selfupdate as su;
use create_panel::update;
use std::path::{Path, PathBuf};

fn app_ver() -> &'static str { env!("CARGO_PKG_VERSION") }
fn newer(a: &str, b: &str) -> bool { matches!((update::parse_ver(a), update::parse_ver(b)), (Some(x), Some(y)) if x > y) }
fn with_ext(p: &Path, ext: &str) -> PathBuf { let mut s = p.as_os_str().to_os_string(); s.push(ext); PathBuf::from(s) }

fn msg(u: &Weak<App>, m: &str) { let m = m.to_string(); with_ui(u, move |a| a.set_upd_msg(SharedString::from(m))); }
fn busy(u: &Weak<App>, b: bool) { with_ui(u, move |a| a.set_upd_busy(b)); }
fn avail(u: &Weak<App>, v: &str, need_token: bool) { let v = v.to_string(); with_ui(u, move |a| { a.set_upd_avail(SharedString::from(v)); a.set_upd_need_token(need_token); }); }

fn engine_ver(st: &Shared) -> Option<String> {
    let c = client(st)?;
    let v = gs(&c.get("/api/ping").ok()?, "version");
    if v.is_empty() { None } else { Some(v) }
}

/// Windows leaves the old app file behind while it runs; remove it on the next start.
pub fn cleanup() {
    if let Ok(exe) = std::env::current_exe() { let _ = std::fs::remove_file(with_ext(&exe, ".old")); }
}

/// What is there to install? (engine: this Mac or the one connected; app window: Windows only)
fn work(l: &su::Latest, ev: Option<&str>) -> (bool, bool, bool) {
    let need_engine = ev.map(|v| newer(&l.version, v)).unwrap_or(false);
    let need_app = cfg!(windows) && newer(&l.version, app_ver());
    let files = (!need_engine || su::pick(l, "create-panel-update-").is_some()) && (!need_app || su::pick(l, "create-panel-app-").is_some());
    (need_engine, need_app, files)
}

pub fn check(u: &Weak<App>, st: &Shared, quiet: bool) {
    match su::latest(app_ver()) {
        Err(e) => {
            avail(u, "", e.contains("token"));
            if !quiet { msg(u, &e); }
        }
        Ok(l) => {
            let ev = engine_ver(st);
            let (ne, na, files) = work(&l, ev.as_deref());
            if (ne || na) && files {
                avail(u, &l.version, false);
                if !quiet { msg(u, &format!("Version {} is ready. Press Update now.", l.version)); }
            } else if (ne || na) && !files {
                avail(u, "", false);
                if !quiet { msg(u, &format!("Version {} is still being built. Try again in a few minutes.", l.version)); }
            } else {
                avail(u, "", false);
                if !quiet { msg(u, &format!("You're on the newest version ({}).", app_ver())); }
            }
        }
    }
}

fn swap_self(l: &su::Latest) -> Result<(), String> {
    let a = su::pick(l, "create-panel-app-").ok_or("The new release has no app file for this computer yet.")?;
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let new = with_ext(&exe, ".new");
    su::download(a, &new)?;
    let bytes = std::fs::read(&new).map_err(|e| e.to_string())?;
    match update::inspect(&bytes) {
        Ok((v, t)) if t == update::TARGET && v == l.version => {}
        _ => { let _ = std::fs::remove_file(&new); return Err("The downloaded app file didn't look right, so nothing was changed.".into()); }
    }
    let old = with_ext(&exe, ".old");
    let _ = std::fs::remove_file(&old);
    std::fs::rename(&exe, &old).map_err(|e| format!("Couldn't move the old app aside: {e}"))?;
    if let Err(e) = std::fs::rename(&new, &exe) { let _ = std::fs::rename(&old, &exe); return Err(format!("Couldn't put the new app in place: {e}")); }
    std::process::Command::new(&exe).spawn().map_err(|e| format!("Updated, but couldn't reopen the app: {e}. Open it again yourself."))?;
    std::process::exit(0);
}

pub fn install(u: &Weak<App>, st: &Shared) {
    busy(u, true);
    let l = match su::latest(app_ver()) { Ok(l) => l, Err(e) => { msg(u, &e); busy(u, false); return; } };
    let ev = engine_ver(st);
    let (ne, na, files) = work(&l, ev.as_deref());
    if !files { msg(u, "That version is still being built. Try again in a few minutes."); busy(u, false); return; }
    if !ne && !na { msg(u, &format!("You're on the newest version ({}).", app_ver())); busy(u, false); return; }
    // 1. the engine (it restarts itself; a running Minecraft server is saved and stopped first, then started again)
    if ne {
        let Some(c) = client(st) else { msg(u, "Not connected to the engine."); busy(u, false); return };
        msg(u, &format!("Downloading {} for the engine...", l.version));
        if let Err(e) = c.post("/api/panel/update-install", &obj(vec![])) { msg(u, &e.to_string()); busy(u, false); return; }
        let mut done = false;
        for _ in 0..180 {
            thread::sleep(Duration::from_secs(2));
            if let Ok(j) = c.get("/api/ping") { if gs(&j, "version") == l.version { done = true; break; } }
            if let Ok(j) = c.get("/api/panel/update-status") {
                let state = gs(&j, "state");
                if state == "error" { msg(u, &gs(&j, "msg")); busy(u, false); return; }
                if state == "installing" { msg(u, &format!("Installing {}. The engine restarts, so this takes a moment...", l.version)); }
            }
        }
        if !done { msg(u, "The engine didn't answer in time. If it rolled back, the old version is still running."); busy(u, false); return; }
    }
    // 2. this app window (Windows). On a Mac the engine update already carried the new app.
    if na {
        msg(u, "Updating this app...");
        if let Err(e) = swap_self(&l) { msg(u, &e); busy(u, false); return; }
    }
    msg(u, &format!("Updated to {}. Close this app and open it again to use the new version.", l.version));
    avail(u, "", false);
    busy(u, false);
}

pub fn wire(app: &App, ui: &Weak<App>, st: &Shared) {
    { let (u, s_) = (ui.clone(), st.clone()); app.on_upd_check(move || { let (u, s_) = (u.clone(), s_.clone()); bg(move || { msg(&u, "Looking for a new version..."); check(&u, &s_, false); }); }); }
    { let (u, s_) = (ui.clone(), st.clone()); app.on_upd_install(move || { let (u, s_) = (u.clone(), s_.clone()); bg(move || install(&u, &s_)); }); }
    { let (u, s_) = (ui.clone(), st.clone()); app.on_upd_save_token(move |t| {
        let (t, u, s_) = (t.to_string(), u.clone(), s_.clone());
        bg(move || {
            if let Err(e) = su::save_source("", &t) { return msg(&u, &e); }
            if let Some(c) = client(&s_) { let _ = c.post("/api/panel/update-source", &obj(vec![("token", s(&t))])); }
            msg(&u, "Saved. Checking again..."); check(&u, &s_, false);
        });
    }); }
    // look quietly now and then, so a banner shows up by itself
    { let (u, s_) = (ui.clone(), st.clone()); thread::spawn(move || { thread::sleep(Duration::from_secs(10)); loop { check(&u, &s_, true); thread::sleep(Duration::from_secs(3600)); } }); }
}
