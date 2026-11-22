//! Update by upload: drop a newer build on the page, no GitHub needed.
//! Safety: the file must carry this program's marker for THIS operating system and CPU, have the right
//! executable header, be newer, and pass a real `--self-test` run. The old build is kept as `<exe>.prev`,
//! and if the new one does not answer within 25 seconds the old one is put back and keeps serving.
use crate::http::{Req, Resp};
use crate::json::{self, J};
use crate::App;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

macro_rules! marker {
    ($t:literal) => {
        pub const TARGET: &str = $t;
        pub static MARKER: &str = concat!("CREATE-PANEL-RS-MARKER:", env!("CARGO_PKG_VERSION"), ":", $t, ";");
    };
}
#[cfg(all(target_os = "macos", target_arch = "aarch64"))] marker!("macos-aarch64");
#[cfg(all(target_os = "macos", target_arch = "x86_64"))] marker!("macos-x86_64");
#[cfg(all(target_os = "windows", target_arch = "x86_64"))] marker!("windows-x86_64");
#[cfg(all(target_os = "linux", target_arch = "x86_64"))] marker!("linux-x86_64");
#[cfg(all(target_os = "linux", target_arch = "aarch64"))] marker!("linux-aarch64");
#[cfg(not(any(
    all(target_os = "macos", target_arch = "aarch64"), all(target_os = "macos", target_arch = "x86_64"),
    all(target_os = "windows", target_arch = "x86_64"), all(target_os = "linux", target_arch = "x86_64"),
    all(target_os = "linux", target_arch = "aarch64")
)))] marker!("other");

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub fn self_test_line() -> String { format!("CREATE-PANEL-RS ok {} {}", VERSION, std::hint::black_box(MARKER)) }

fn prefix() -> Vec<u8> { ["CREATE-PANEL-", "RS-MARKER:"].concat().into_bytes() }

/// Finds (version, target) in an executable's bytes.
pub fn inspect(bytes: &[u8]) -> Result<(String, String), String> {
    if bytes.len() < 100_000 { return Err("That file is too small to be a panel build.".into()); }
    let ok_magic = if cfg!(windows) { bytes.starts_with(b"MZ") }
        else if cfg!(target_os = "macos") { bytes.starts_with(&[0xcf, 0xfa, 0xed, 0xfe]) || bytes.starts_with(&[0xca, 0xfe, 0xba, 0xbe]) }
        else { bytes.starts_with(&[0x7f, b'E', b'L', b'F']) };
    if !ok_magic { return Err("That isn't a program for this computer. Pick the build made for it.".into()); }
    let p = prefix();
    let mut i = 0;
    while let Some(off) = find(&bytes[i..], &p) {
        let st = i + off + p.len();
        let end = bytes[st..].iter().take(60).position(|&c| c == b';');
        if let Some(e) = end {
            let s = String::from_utf8_lossy(&bytes[st..st + e]).to_string();
            if let Some((v, t)) = s.split_once(':') {
                if parse_ver(v).is_some() && !t.is_empty() && t.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
                    return Ok((v.to_string(), t.to_string()));
                }
            }
        }
        i = st;
    }
    Err("That file isn't a Create Server Panel build (no panel marker found).".into())
}

fn find(h: &[u8], n: &[u8]) -> Option<usize> { h.windows(n.len()).position(|w| w == n) }

pub fn parse_ver(v: &str) -> Option<(u32, u32, u32)> {
    let mut p = v.split('.');
    let r = (p.next()?.parse().ok()?, p.next()?.parse().ok()?, p.next()?.parse().ok()?);
    if p.next().is_some() { None } else { Some(r) }
}

fn exe_path() -> Result<PathBuf, String> { std::env::current_exe().map_err(|e| format!("Can't find myself: {e}")) }

fn with_ext(p: &PathBuf, ext: &str) -> PathBuf { let mut s = p.clone().into_os_string(); s.push(ext); PathBuf::from(s) }

pub fn handle(app: &Arc<App>, req: &Req) -> Resp { install(app, &req.body, req.q("check") == Some("1"), req.q("force") == Some("1")) }

/// Check (and unless `check`, install) a build given as bytes. Shared by "update from a file" and one-click updates.
pub fn install(app: &Arc<App>, body: &[u8], check: bool, force: bool) -> Resp {
    if body.is_empty() { return fail("No file was sent."); }
    let (ver, target) = match inspect(body) { Ok(x) => x, Err(e) => return fail(&e) };
    if target != TARGET { return fail(&format!("That build is for {target}, but this panel runs on {TARGET}.")); }
    let (nv, cv) = (parse_ver(&ver).unwrap(), parse_ver(VERSION).unwrap());
    let info = |extra: Vec<(&str, J)>| {
        let mut v = vec![("ok", J::Bool(true)), ("version", json::s(&ver)), ("current", json::s(VERSION)), ("target", json::s(&target)), ("newer", J::Bool(nv > cv))];
        v.extend(extra);
        Resp::json(200, json::obj(v).to_string())
    };
    if nv <= cv && !force {
        return fail(&format!("This panel is already on {VERSION}; that file is {ver}. (Add force to install an older or equal build.)"));
    }
    if check { return info(vec![("msg", json::s(&format!("Ready to update from {VERSION} to {ver}.")))]); }

    // stage next to the running program, make it executable, and really run it once
    let exe = match exe_path() { Ok(p) => p, Err(e) => return fail(&e) };
    let staged = with_ext(&exe, ".new");
    if let Err(e) = std::fs::File::create(&staged).and_then(|mut f| f.write_all(body)) { return fail(&format!("Couldn't save the update: {e}")); }
    #[cfg(unix)] { use std::os::unix::fs::PermissionsExt; let _ = std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755)); }
    match self_test(&staged, &ver) { Ok(()) => {} Err(e) => { let _ = std::fs::remove_file(&staged); return fail(&format!("The new build didn't pass its self-test: {e}")); } }

    let app2 = app.clone();
    let ver2 = ver.clone();
    thread::spawn(move || { thread::sleep(Duration::from_millis(400)); apply(&app2, exe, staged, ver2); });
    info(vec![("msg", json::s("Updating. The panel will be back in a few seconds; this page reloads on its own.")), ("restarting", J::Bool(true))])
}

fn fail(msg: &str) -> Resp { Resp::json(400, json::obj(vec![("ok", J::Bool(false)), ("error", json::s(msg)), ("msg", json::s(msg))]).to_string()) }

fn self_test(path: &PathBuf, want: &str) -> Result<(), String> {
    let mut c = Command::new(path).arg("--self-test").stdout(Stdio::piped()).stderr(Stdio::null()).stdin(Stdio::null()).spawn().map_err(|e| format!("can't run it: {e}"))?;
    let end = Instant::now() + Duration::from_secs(8);
    loop {
        match c.try_wait() {
            Ok(Some(st)) => {
                let mut out = String::new();
                if let Some(mut o) = c.stdout.take() { let _ = o.read_to_string(&mut out); }
                if st.success() && out.starts_with("CREATE-PANEL-RS ok ") && out.split_whitespace().nth(2) == Some(want) { return Ok(()); }
                return Err(format!("unexpected answer ({})", out.trim()));
            }
            Ok(None) if Instant::now() < end => thread::sleep(Duration::from_millis(50)),
            _ => { let _ = c.kill(); return Err("it didn't finish in time".into()); }
        }
    }
}

fn apply(app: &Arc<App>, exe: PathBuf, staged: PathBuf, ver: String) {
    // 1. stop the Minecraft server safely; the new panel starts it again
    let was_running = app.mc.running();
    if was_running {
        app.mc.log("[panel] Updating the panel: stopping the server first...");
        app.mc.send("say Panel update in a moment, saving the world.");
        app.mc.send("save-all");
        thread::sleep(Duration::from_secs(2));
        app.mc.stop();
        if !app.mc.wait_stopped(120) { app.mc.kill(); app.mc.wait_stopped(10); }
        let _ = std::fs::write(app.dir.join(".panel-resume"), "start");
    }
    // 2. keep the old build, swap in the new one
    let prev = with_ext(&exe, ".prev");
    let _ = std::fs::remove_file(&prev);
    if std::fs::rename(&exe, &prev).is_err() || std::fs::rename(&staged, &exe).is_err() {
        let _ = std::fs::rename(&prev, &exe);
        app.mc.log("[panel] Update failed: couldn't swap the files. Nothing changed.");
        if was_running { let _ = std::fs::remove_file(app.dir.join(".panel-resume")); app.mc.start(); }
        return;
    }
    // 3. let go of the port, start the new build, wait for it to answer
    app.paused.store(true, Ordering::SeqCst);
    thread::sleep(Duration::from_millis(700));
    let args: Vec<String> = std::env::args().skip(1).filter(|a| a != "--after-update").collect();
    let child = Command::new(&exe).args(&args).arg("--after-update").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
    let mut child = match child { Ok(c) => c, Err(_) => { rollback(app, &exe, &prev, None, was_running); return; } };
    let end = Instant::now() + Duration::from_secs(25);
    while Instant::now() < end {
        if matches!(child.try_wait(), Ok(Some(_))) { break; }
        if ping(app.port, &app.key, &ver) { std::process::exit(0); }
        thread::sleep(Duration::from_millis(400));
    }
    rollback(app, &exe, &prev, Some(child), was_running);
}

fn rollback(app: &Arc<App>, exe: &PathBuf, prev: &PathBuf, child: Option<std::process::Child>, was_running: bool) {
    if let Some(mut c) = child { let _ = c.kill(); let _ = c.wait(); }
    let _ = std::fs::remove_file(exe);
    let _ = std::fs::rename(prev, exe);
    app.paused.store(false, Ordering::SeqCst); // the old panel takes the port back
    let _ = std::fs::remove_file(app.dir.join(".panel-resume"));
    app.mc.log("[panel] The new build didn't start, so the old one is back. Nothing was lost.");
    if was_running { thread::sleep(Duration::from_secs(2)); app.mc.start(); }
}

fn ping(port: u16, key: &str, ver: &str) -> bool {
    let Ok(mut s) = TcpStream::connect_timeout(&format!("127.0.0.1:{port}").parse().unwrap(), Duration::from_millis(500)) else { return false };
    let _ = s.set_read_timeout(Some(Duration::from_secs(2)));
    if s.write_all(format!("GET /api/ping HTTP/1.1\r\nHost: x\r\nX-Key: {key}\r\nConnection: close\r\n\r\n").as_bytes()).is_err() { return false; }
    let mut out = String::new();
    let _ = s.read_to_string(&mut out);
    out.starts_with("HTTP/1.1 200") && out.contains(&format!("\"version\":\"{ver}\""))
}
