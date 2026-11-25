//! One-click updates from the app. Finds the newest release on GitHub, updates the engine (this Mac, or the
//! Mac you are connected to) and, on Windows, replaces this app window too, so nothing is reinstalled by hand.
use crate::*;
use create_panel::selfupdate as su;
use create_panel::update;
use std::path::{Path, PathBuf};

fn app_ver() -> &'static str { env!("CARGO_PKG_VERSION") }
fn with_ext(p: &Path, ext: &str) -> PathBuf { let mut s = p.as_os_str().to_os_string(); s.push(ext); PathBuf::from(s) }

fn msg(u: &Weak<App>, m: &str) { let m = m.to_string(); with_ui(u, move |a| a.set_upd_msg(SharedString::from(m))); }
fn busy(u: &Weak<App>, b: bool) { with_ui(u, move |a| a.set_upd_busy(b)); }
fn avail(u: &Weak<App>, v: &str, need_token: bool) { let v = v.to_string(); with_ui(u, move |a| { a.set_upd_avail(SharedString::from(v)); a.set_upd_need_token(need_token); }); }

/// Windows leaves the old app file behind while it runs; remove it on the next start.
pub fn cleanup() {
    if let Ok(exe) = std::env::current_exe() { let _ = std::fs::remove_file(with_ext(&exe, ".old")); }
}

/// What the engine (this Mac, or the one connected) says about updates. It looks at its own platform's files,
/// so a Windows window talking to a Mac engine still asks the right question.
struct EngineSay { newer: bool, file_missing: bool, latest: String, err: String }

fn ask_engine(u: &su::Source, st: &Shared) -> Option<EngineSay> {
    let c = client(st)?;
    let mut tried_push = false;
    loop {
        let j = match c.get("/api/panel/update-check") { Ok(j) => j, Err(e) => return Some(EngineSay { newer: false, file_missing: false, latest: String::new(), err: e.to_string() }) };
        if j.get("ok").and_then(|x| x.as_bool()).unwrap_or(false) {
            return Some(EngineSay { newer: j.get("newer").and_then(|x| x.as_bool()).unwrap_or(false), file_missing: j.get("file_missing").and_then(|x| x.as_bool()).unwrap_or(false), latest: gs(&j, "latest"), err: String::new() });
        }
        let err = { let e = gs(&j, "error"); if e.is_empty() { gs(&j, "msg") } else { e } };
        // an older engine may still look at a previous repository: hand it ours once and ask again
        if !tried_push {
            tried_push = true;
            let _ = c.post("/api/panel/update-source", &obj(vec![("repo", s(&u.repo)), ("token", s(&u.token))]));
            continue;
        }
        return Some(EngineSay { newer: false, file_missing: false, latest: String::new(), err });
    }
}

pub fn check(u: &Weak<App>, st: &Shared, quiet: bool) {
    let src = su::source();
    let local = su::latest(app_ver());
    let eng = ask_engine(&src, st);
    // engine side
    let mut e_new = false;
    let mut e_missing = false;
    let mut e_err = String::new();
    let mut version = String::new();
    if let Some(e) = &eng {
        e_new = e.newer;
        e_missing = e.file_missing;
        e_err = e.err.clone();
        if e.newer || e.file_missing { version = e.latest.clone(); }
    }
    // app side (Windows only; on a Mac the engine update carries the new app)
    let mut a_new = false;
    let mut a_missing = false;
    let mut l_err = String::new();
    match &local {
        Ok(l) => {
            if cfg!(windows) && l.newer {
                if su::pick(l, "create-panel-app-").is_some() { a_new = true; } else { a_missing = true; }
                if version.is_empty() { version = l.version.clone(); }
            }
        }
        Err(e) => l_err = e.clone(),
    }
    let need_token = (l_err.contains("token") || e_err.contains("token")) && !l_err.is_empty();
    if (e_new || a_new) && !e_missing && !a_missing {
        avail(u, &version, false);
        if !quiet { msg(u, &format!("Version {} is ready. Press Update now.", version)); }
    } else if e_missing || a_missing {
        avail(u, "", false);
        let what = if e_missing && a_missing { "the engine and app files" } else if e_missing { "the engine file for the computer running it" } else { "the Windows app file" };
        if !quiet { msg(u, &format!("Version {} is on GitHub but {} isn't uploaded yet. The build is still finishing; try again in a few minutes.", version, what)); }
    } else if !l_err.is_empty() && eng.as_ref().map(|e| !e.err.is_empty()).unwrap_or(true) {
        avail(u, "", need_token);
        if !quiet { msg(u, &l_err); }
    } else if !e_err.is_empty() {
        avail(u, "", false);
        if !quiet { msg(u, &format!("The engine couldn't check for updates: {e_err}")); }
    } else {
        avail(u, "", false);
        if !quiet { msg(u, &format!("You're on the newest version ({}).", app_ver())); }
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
    let src = su::source();
    let eng = ask_engine(&src, st);
    let ne = eng.as_ref().map(|e| e.newer).unwrap_or(false);
    let target = eng.as_ref().map(|e| e.latest.clone()).unwrap_or_default();
    // Windows window: look at the newest release for the app file
    let local = su::latest(app_ver());
    let na = cfg!(windows) && local.as_ref().map(|l| l.newer && su::pick(l, "create-panel-app-").is_some()).unwrap_or(false);
    if !ne && !na { msg(u, &format!("Nothing to install right now ({}). Press Check for updates again.", app_ver())); busy(u, false); return; }
    // 1. the engine (it restarts itself; a running Minecraft server is saved and stopped first, then started again)
    if ne {
        let Some(c) = client(st) else { msg(u, "Not connected to the engine."); busy(u, false); return };
        msg(u, &format!("Downloading {} for the engine...", target));
        match c.post("/api/panel/update-install", &obj(vec![])) {
            Err(e) => { msg(u, &e.to_string()); busy(u, false); return; }
            Ok(j) => { if !j.get("ok").and_then(|x| x.as_bool()).unwrap_or(true) { msg(u, &gs(&j, "msg")); busy(u, false); return; } }
        }
        let mut done = false;
        for _ in 0..180 {
            thread::sleep(Duration::from_secs(2));
            if let Ok(j) = c.get("/api/ping") { if gs(&j, "version") == target { done = true; break; } }
            if let Ok(j) = c.get("/api/panel/update-status") {
                let state = gs(&j, "state");
                if state == "error" { msg(u, &gs(&j, "msg")); busy(u, false); return; }
                if state == "installing" { msg(u, &format!("Installing {}. The engine restarts, so this takes a moment...", target)); }
            }
        }
        if !done { msg(u, "The engine didn't answer in time. If it rolled back, the old version is still running."); busy(u, false); return; }
    }
    // 2. this app window (Windows). On a Mac the engine update already carried the new app.
    if na {
        if let Ok(l) = &local {
            msg(u, "Updating this app...");
            if let Err(e) = swap_self(l) { msg(u, &e); busy(u, false); return; }
        }
    }
    let shown = if !target.is_empty() { target } else { local.map(|l| l.version).unwrap_or_default() };
    msg(u, &format!("Updated to {}. Close this app and open it again to use the new version.", shown));
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
    { let (u, s_) = (ui.clone(), st.clone()); app.on_upd_save_repo(move |r| {
        let (r, u, s_) = (r.to_string().trim().trim_start_matches("https://github.com/").trim_end_matches('/').to_string(), u.clone(), s_.clone());
        bg(move || {
            if !r.contains('/') || r.contains(' ') { return msg(&u, "Type the repository as owner/name, for example yukibafi/Slate-Server."); }
            let tok = su::source().token;
            if let Err(e) = su::save_source(&r, &tok) { return msg(&u, &e); }
            if let Some(c) = client(&s_) { let _ = c.post("/api/panel/update-source", &obj(vec![("repo", s(&r))])); }
            let rr = r.clone(); with_ui(&u, move |a| a.set_upd_repo(SharedString::from(rr)));
            msg(&u, "Saved. Checking again..."); check(&u, &s_, false);
        });
    }); }
    app.set_upd_repo(SharedString::from(su::source().repo));
    app.set_upd_cur(SharedString::from(app_ver()));
    // look quietly now and then, so a banner shows up by itself
    { let (u, s_) = (ui.clone(), st.clone()); thread::spawn(move || { thread::sleep(Duration::from_secs(10)); loop { check(&u, &s_, true); thread::sleep(Duration::from_secs(3600)); } }); }
}
