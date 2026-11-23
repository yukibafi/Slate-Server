//! After an engine update the new engine carries the matching desktop app (built in by CI). On start it
//! replaces the app file next to it when that one is older, so a single update file updates both.
use crate::update;
use std::path::{Path, PathBuf};
use std::{fs, thread, time::Duration};

static BLOB: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/gui_blob.bin"));

fn gui_path() -> Option<PathBuf> {
    let dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
    ["Slate Server", "Create Panel", "create-panel-app.exe", "create-panel-app"].iter().map(|n| dir.join(n)).find(|p| p.is_file())
}

fn app_bundle(p: &Path) -> Option<PathBuf> { p.ancestors().find(|a| a.extension().map(|e| e == "app").unwrap_or(false)).map(|a| a.to_path_buf()) }

fn with_ext(p: &Path, ext: &str) -> PathBuf { let mut s = p.as_os_str().to_os_string(); s.push(ext); PathBuf::from(s) }

fn sync() -> Result<bool, String> {
    let (bv, bt) = update::inspect(BLOB)?;
    if bt != update::TARGET { return Ok(false); }
    let Some(p) = gui_path() else { return Ok(false) };
    let cur = fs::read(&p).map_err(|e| e.to_string())?;
    if let Ok((cv, _)) = update::inspect(&cur) {
        if update::parse_ver(&bv) <= update::parse_ver(&cv) { return Ok(false); }
    }
    let staged = with_ext(&p, ".new");
    fs::write(&staged, BLOB).map_err(|e| format!("can't stage the app: {e}"))?;
    #[cfg(unix)] { use std::os::unix::fs::PermissionsExt; let _ = fs::set_permissions(&staged, fs::Permissions::from_mode(0o755)); }
    let old = with_ext(&p, ".old");
    let _ = fs::remove_file(&old);
    if cfg!(windows) { fs::rename(&p, &old).map_err(|e| format!("can't move the old app: {e}"))?; }
    if let Err(e) = fs::rename(&staged, &p) { if cfg!(windows) { let _ = fs::rename(&old, &p); } return Err(format!("can't put the new app in place: {e}")); }
    // a changed file inside a signed .app needs a fresh (ad-hoc) signature or macOS refuses to open it
    if cfg!(target_os = "macos") { if let Some(b) = app_bundle(&p) { let _ = std::process::Command::new("codesign").args(["--force", "--deep", "--sign", "-"]).arg(&b).output(); } }
    Ok(true)
}

/// Whether this engine carries an app build (for the ping/about info).
pub fn carries_app() -> bool { BLOB.len() > 100_000 }

pub fn start() {
    if !carries_app() { return; }
    thread::spawn(|| {
        thread::sleep(Duration::from_secs(4));
        match sync() { Ok(true) => println!("[update] The app window was updated to match. Reopen the app to use it."), Ok(false) => {}, Err(e) => eprintln!("[update] App sync skipped: {e}") }
    });
}
