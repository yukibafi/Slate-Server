//! Java detection/install, and other machine-level helpers.
use crate::envinfo;
use crate::http::{Req, Resp};
use crate::json::{self, n, obj, s, J};
use crate::{jobs, sys, App};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;

pub fn home() -> PathBuf { PathBuf::from(std::env::var(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).unwrap_or_else(|_| ".".into())) }
pub fn java_dir() -> PathBuf { home().join(".create-panel-java") }
fn exe(n: &str) -> String { if cfg!(windows) { format!("{n}.exe") } else { n.to_string() } }

pub fn required_java(mc: &str) -> i64 {
    let p: Vec<i64> = mc.split(|c: char| !c.is_ascii_digit()).filter(|x| !x.is_empty()).filter_map(|x| x.parse().ok()).collect();
    if p.is_empty() { return 21; }
    if p[0] == 1 {
        let (minor, patch) = (p.get(1).copied().unwrap_or(0), p.get(2).copied().unwrap_or(0));
        return if minor >= 21 || (minor == 20 && patch >= 5) { 21 } else if minor >= 18 { 17 } else if minor == 17 { 16 } else { 8 };
    }
    if p[0] >= 26 { 25 } else { 21 }
}

pub fn java_major(path: &str) -> Option<i64> {
    let (code, out) = sys::run_both(path, &["-version"])?;
    let t = String::from_utf8_lossy(&out).to_string();
    let i = t.find("version \"")? + 9;
    let rest = &t[i..];
    let mut it = rest.split(|c: char| !c.is_ascii_digit());
    let a: i64 = it.next()?.parse().ok()?;
    if code != 0 { return None; }
    if a == 1 { Some(it.next().and_then(|x| x.parse().ok()).unwrap_or(0)) } else { Some(a) }
}

pub fn java_candidates() -> Vec<String> {
    let mut c: Vec<String> = vec![];
    if let Ok(jh) = std::env::var("JAVA_HOME") { c.push(Path::new(&jh).join("bin").join(exe("java")).to_string_lossy().to_string()); }
    if let Some(w) = sys::which("java") { c.push(w); }
    let h = home().to_string_lossy().to_string();
    let mut pats: Vec<String> = vec![
        "/opt/homebrew/opt/openjdk*/bin/java".into(), "/opt/homebrew/opt/openjdk*/libexec/openjdk.jdk/Contents/Home/bin/java".into(),
        "/usr/local/opt/openjdk*/bin/java".into(), "/usr/local/opt/openjdk*/libexec/openjdk.jdk/Contents/Home/bin/java".into(),
        "/Library/Java/JavaVirtualMachines/*/Contents/Home/bin/java".into(), format!("{h}/Library/Java/JavaVirtualMachines/*/Contents/Home/bin/java"),
        format!("{h}/.sdkman/candidates/java/*/bin/java"), "/usr/lib/jvm/*/bin/java".into(),
        format!("{h}/.create-panel-java/*/Contents/Home/bin/java"), format!("{h}/.create-panel-java/*/*/Contents/Home/bin/java"),
        format!("{h}/.create-panel-java/*/bin/java"), format!("{h}/.create-panel-java/*/*/bin/java"),
    ];
    if cfg!(windows) { pats = vec!["C:/Program Files/Eclipse Adoptium/*/bin/java.exe".into(), "C:/Program Files/Java/*/bin/java.exe".into(), format!("{h}/.create-panel-java/*/bin/java.exe"), format!("{h}/.create-panel-java/*/*/bin/java.exe")]; }
    for p in pats { for g in sys::glob(&p) { c.push(g.to_string_lossy().to_string()); } }
    let mut seen = vec![]; let mut out = vec![];
    for p in c {
        let Ok(r) = fs::canonicalize(&p) else { continue };
        if r.is_file() && !seen.contains(&r) { seen.push(r); out.push(p); }
    }
    out
}

fn adoptium() -> String { std::env::var("PANEL_ADOPTIUM_API").unwrap_or_else(|_| "https://api.adoptium.net".into()) }

/// Download + unpack private Java copies into ~/.create-panel-java. Fills `job`.
pub fn install_java(job: &jobs::Job, versions: &[i64], kind: &str) {
    let kind = if kind == "jre" { "jre" } else { "jdk" };
    let res = (|| -> Result<Vec<i64>, String> {
        let osn = if cfg!(target_os = "macos") { "mac" } else if cfg!(windows) { "windows" } else { "linux" };
        let arch = if std::env::consts::ARCH == "aarch64" { "aarch64" } else { "x64" };
        let jd = java_dir();
        fs::create_dir_all(&jd).map_err(|e| e.to_string())?;
        let mut done = vec![];
        for (i, &v) in versions.iter().enumerate() {
            let base = 100 * i as i64 / versions.len() as i64; let span = 100.0 / versions.len() as f64;
            let have = java_candidates().into_iter().find(|p| java_major(p) == Some(v) && (kind == "jdk" || p.contains(&format!("/{kind}-{v}/"))));
            if have.is_some() { jobs::log(job, &format!("Java {v} is already installed.")); done.push(v); continue; }
            for nm in [format!("jdk-{v}"), format!("jre-{v}")] { let f = jd.join(&nm); if f.is_dir() { let _ = fs::remove_dir_all(&f); jobs::log(job, &format!("Removed old copy: {nm}")); } }
            let ext = if cfg!(windows) { "zip" } else { "tar.gz" };
            let url = format!("{}/v3/binary/latest/{v}/ga/{osn}/{arch}/hotspot/normal/eclipse", adoptium());
            let tmp = jd.join(format!("download-{kind}-{v}.{ext}"));
            jobs::step(job, &format!("Downloading Java {v}..."), base + 2);
            sys::download(&url, &tmp, 900).map_err(|_| format!("Couldn't download Java {v}. Check the internet connection."))?;
            jobs::step(job, &format!("Unpacking Java {v}..."), base + (span * 0.8) as i64);
            let dest = jd.join(format!("{kind}-{v}"));
            let _ = fs::remove_dir_all(&dest); fs::create_dir_all(&dest).map_err(|e| e.to_string())?;
            let tp = tmp.to_string_lossy().to_string();
            let list = sys::run("tar", &["-tf", &tp], None).map(|x| String::from_utf8_lossy(&x.1).to_string()).unwrap_or_default();
            if list.lines().any(|l| l.starts_with('/') || l.split('/').any(|p| p == "..") || l.contains(":\\")) { let _ = fs::remove_file(&tmp); return Err("The Java download had an unsafe file in it.".into()); }
            let r = sys::run("tar", &["-xf", &tp, "-C", &dest.to_string_lossy()], None);
            let _ = fs::remove_file(&tmp);
            if !matches!(r, Some((0, _))) { return Err(format!("Couldn't unpack Java {v}.")); }
            let dd = dest.to_string_lossy().to_string();
            if !java_candidates().iter().any(|p| p.starts_with(&dd) && java_major(p) == Some(v)) { return Err(format!("Java {v} was unpacked but doesn't run on this computer.")); }
            done.push(v); jobs::log(job, &format!("Installed Java {v}."));
        }
        Ok(done)
    })();
    match res {
        Ok(d) => { jobs::step(job, "Done", 100); jobs::finish(job, true, &format!("Java {} is ready.", d.iter().map(|x| x.to_string()).collect::<Vec<_>>().join(" and "))); }
        Err(e) => jobs::finish(job, false, &e),
    }
}

/// Find a Java >= need, downloading a private one if none fits.
pub fn find_java(need: i64) -> Result<(String, i64), String> {
    let scan = || { let mut best: Option<(String, i64)> = None; let mut found = vec![];
        for p in java_candidates() { if let Some(m) = java_major(&p) { found.push(m); if m >= need && best.as_ref().map(|b| m < b.1).unwrap_or(true) { best = Some((p, m)); } } }
        (best, found) };
    let (best, found) = scan();
    if let Some(b) = best { return Ok(b); }
    if let Some(v) = [8, 17, 21, 25].into_iter().find(|x| *x >= need) {
        let (_, j) = jobs::create("Install Java", false);
        install_java(&j, &[v], "jdk");
        if jobs::is_ok(&j) { if let (Some(b), _) = scan() { return Ok(b); } }
        let m = jobs::get_str(&j, "msg");
        if !m.is_empty() { return Err(format!("This Minecraft version needs Java {need} and I couldn't download it automatically. {m}")); }
    }
    let mut f = found.clone(); f.sort(); f.dedup();
    let have = if f.is_empty() { "I couldn't find Java at all. ".to_string() } else { format!("The Java versions I found: {}. ", f.iter().map(|x| x.to_string()).collect::<Vec<_>>().join(", ")) };
    Err(format!("This Minecraft version needs Java {need} or newer. {have}Install it (for example in Terminal: brew install openjdk@{need}), then try again."))
}

pub fn apply_java_to_start(dir: &Path, java: &str) -> Result<(), String> {
    let sh = dir.join("start.sh");
    let txt = fs::read_to_string(&sh).map_err(|e| e.to_string())?;
    let line = format!("JAVA={}", J::Str(java.to_string()).to_string());
    let out = if let Some(pos) = txt.lines().position(|l| l.starts_with("JAVA=")) {
        txt.split('\n').enumerate().map(|(i, l)| if i == pos { line.clone() } else { l.to_string() }).collect::<Vec<_>>().join("\n")
    } else {
        // drop an earlier PATH-style insertion
        let mut lines: Vec<&str> = txt.split('\n').collect();
        if let Some(i) = lines.iter().position(|l| *l == "# create-panel-java-path") { let end = (i + 3).min(lines.len()); lines.drain(i..end); }
        let jh = fs::canonicalize(java).ok().and_then(|p| p.parent().and_then(|x| x.parent()).map(|x| x.to_path_buf())).unwrap_or_default();
        let ins = format!("# create-panel-java-path\nexport JAVA_HOME={}\nexport PATH=\"$JAVA_HOME/bin:$PATH\"", J::Str(jh.to_string_lossy().to_string()).to_string());
        let at = if lines.first().map(|l| l.starts_with("#!")).unwrap_or(false) { 1 } else { 0 };
        lines.insert(at, &ins);
        lines.join("\n")
    };
    fs::write(&sh, out).map_err(|e| e.to_string())?;
    #[cfg(unix)] { use std::os::unix::fs::PermissionsExt; let _ = fs::set_permissions(&sh, fs::Permissions::from_mode(0o755)); }
    Ok(())
}
fn current_java(dir: &Path) -> Option<String> {
    let txt = fs::read_to_string(dir.join("start.sh")).ok()?;
    if let Some(l) = txt.lines().find(|l| l.starts_with("JAVA=")) {
        let raw = l[5..].trim();
        return Some(if raw.starts_with('"') { json::parse(raw).and_then(|j| j.as_str().map(|x| x.to_string())).unwrap_or_else(|| raw.to_string()) } else { raw.to_string() });
    }
    let l = txt.lines().find(|l| l.starts_with("export JAVA_HOME="))?;
    Some(Path::new(l[17..].trim().trim_matches('"')).join("bin").join("java").to_string_lossy().to_string())
}

struct Jv { phase: &'static str, msg: String, ok: Option<bool>, job: Option<jobs::Job> }
static JV: Mutex<Jv> = Mutex::new(Jv { phase: "idle", msg: String::new(), ok: None, job: None });
fn reply(ok: bool, m: &str) -> Resp { Resp::json(200, obj(vec![("ok", J::Bool(ok)), ("msg", s(m))]).to_string()) }
fn gs(j: &J, k: &str) -> String { match j.get(k) { Some(J::Str(t)) => t.clone(), Some(J::Num(x)) => format!("{}", *x as i64), _ => String::new() } }

pub fn handle(app: &Arc<App>, req: &Req) -> Option<Resp> {
    let body = || json::parse(&String::from_utf8_lossy(&req.body)).unwrap_or(J::Null);
    Some(match (req.method.as_str(), req.path.as_str()) {
        ("GET", "/api/java/info") => {
            let mc = envinfo::env_cached(&app.dir).1;
            let cur = current_java(&app.dir);
            let cm = cur.as_ref().filter(|p| Path::new(p).is_file()).and_then(|p| java_major(p));
            let jd = java_dir().to_string_lossy().to_string();
            let items: Vec<J> = java_candidates().into_iter().filter_map(|p| java_major(&p).map(|m| obj(vec![("major", n(m)), ("kind", s(if p.contains("/jre-") { "JRE" } else { "JDK" })), ("private", J::Bool(p.starts_with(&jd))), ("path", s(&p))]))).collect();
            Resp::json(200, obj(vec![("need", mc.as_ref().map(|m| n(required_java(m))).unwrap_or(J::Null)), ("mc", mc.map(|m| s(&m)).unwrap_or(J::Null)),
                ("current", cur.map(|p| obj(vec![("path", s(&p)), ("major", cm.map(n).unwrap_or(J::Null))])).unwrap_or(J::Null)),
                ("installed", J::Arr(items)), ("running", J::Bool(app.mc.running())), ("scripted", J::Bool(app.dir.join("start.sh").is_file())), ("phase", s(JV.lock().unwrap().phase))]).to_string())
        }
        ("GET", "/api/java/status") => {
            let g = JV.lock().unwrap();
            let (step, pct, log) = match g.job.as_ref().and_then(|j| jobs::get_obj(j)) { Some(o) => (gs(&o, "step"), match o.get("pct") { Some(J::Num(x)) => *x as i64, _ => 0 }, match o.get("lines") { Some(J::Arr(a)) => a.iter().rev().take(20).rev().cloned().collect(), _ => vec![] }), None => (String::new(), 0, vec![]) };
            Resp::json(200, obj(vec![("phase", s(g.phase)), ("msg", s(&g.msg)), ("ok", g.ok.map(J::Bool).unwrap_or(J::Null)), ("step", s(&step)), ("pct", n(pct)), ("log", J::Arr(log))]).to_string())
        }
        ("POST", "/api/java/install") => {
            if JV.lock().unwrap().phase == "running" { return Some(reply(false, "A Java download is already running.")); }
            let b = body();
            let v = match b.get("version") { Some(J::Num(x)) => *x as i64, Some(J::Str(t)) => t.parse().unwrap_or(0), _ => 0 };
            if ![8, 17, 21, 25].contains(&v) { return Some(reply(false, "Pick a Java version.")); }
            let kind = if gs(&b, "kind") == "jre" { "jre" } else { "jdk" };
            let useit = matches!(b.get("use"), Some(J::Bool(true)));
            if useit && app.mc.running() { return Some(reply(false, "Stop the server first to switch its Java. You can still download it.")); }
            let (_, job) = jobs::create("Install Java", false);
            *JV.lock().unwrap() = Jv { phase: "running", msg: String::new(), ok: None, job: Some(job.clone()) };
            let dir = app.dir.clone();
            thread::spawn(move || {
                install_java(&job, &[v], kind);
                let (mut ok, mut msg) = (jobs::is_ok(&job), jobs::get_str(&job, "msg"));
                if ok && useit {
                    let jd = java_dir().to_string_lossy().to_string();
                    let best = java_candidates().into_iter().filter(|p| p.starts_with(&jd) && java_major(p) == Some(v) && (kind == "jdk" || p.contains(&format!("/jre-{v}/"))) && (kind == "jre" || !p.contains("/jre-"))).last();
                    match best { Some(p) => match apply_java_to_start(&dir, &p) { Ok(_) => msg.push_str(" This server now uses it."), Err(e) => { ok = false; msg = e; } }, None => { ok = false; msg = "Java downloaded but I couldn't find it to use.".into(); } }
                } else if !ok && msg.is_empty() { msg = "The download failed.".into(); }
                let mut g = JV.lock().unwrap(); g.phase = "done"; g.ok = Some(ok); g.msg = msg;
            });
            reply(true, "Downloading Java...")
        }
        ("POST", "/api/java/use") => {
            if app.mc.running() { return Some(reply(false, "Stop the server first, then switch its Java.")); }
            let p = gs(&body(), "path");
            if !java_candidates().contains(&p) || java_major(&p).is_none() { return Some(reply(false, "I don't know that Java.")); }
            match apply_java_to_start(&app.dir, &p) { Ok(_) => reply(true, "This server will use that Java next time it starts."), Err(e) => reply(false, &format!("Couldn't change start.sh: {e}")) }
        }
        ("GET", "/api/recommend") => Resp::json(200, obj(vec![("mac", crate::config::mac_info()), ("recommend", crate::config::recommend(&app.dir)), ("jvm", crate::config::jvm_json(&app.dir))]).to_string()),
        ("GET", "/api/netcheck") => Resp::json(200, obj(vec![("ips", J::Arr(crate::net::lan_ips().iter().map(|x| s(x)).collect())), ("fw", crate::net::fw_status().unwrap_or(J::Null)), ("netmask_bad", J::Bool(!crate::net::bad_netmask().is_empty())), ("netmask_help", s(crate::net::NETMASK_HELP))]).to_string()),
        ("POST", "/api/netfix") => { let (ok, m) = crate::net::fw_fix(); reply(ok, &m) }
        ("GET", "/api/publicip") => { let (ip, st) = crate::net::public_ip(); Resp::json(200, obj(vec![("ip", ip.map(|x| s(&x)).unwrap_or(J::Null)), ("state", s(st))]).to_string()) }
        ("GET", "/api/autostart") => Resp::json(200, obj(vec![("enabled", J::Bool(plist_path().is_file())), ("background", J::Bool(std::env::var("PANEL_BACKGROUND").is_ok())), ("log", s(&agent_log().display().to_string()))]).to_string()),
        ("POST", "/api/autostart") => { let on = matches!(body().get("enable"), Some(J::Bool(true))); let (ok, m) = if on { autostart_enable() } else { autostart_disable() }; reply(ok, &m) }
        ("GET", "/api/lid/status") => {
            let mac = cfg!(target_os = "macos");
            let (ac, batt) = if mac { lid_on_ac() } else { (false, false) };
            Resp::json(200, obj(vec![("supported", J::Bool(mac)), ("enabled", J::Bool(lid_load().get("enabled").map(|x| matches!(x, J::Bool(true))).unwrap_or(false))), ("helper", J::Bool(mac && lid_helper_ok())), ("on_ac", J::Bool(ac)), ("battery", J::Bool(batt)), ("active", J::Bool(mac && lid_sleep_disabled()))]).to_string())
        }
        ("POST", "/api/lid/setup") => lid_setup(matches!(body().get("remove"), Some(J::Bool(true)))),
        ("POST", "/api/lid/set") => {
            let en = matches!(body().get("enabled"), Some(J::Bool(true)));
            if en && !lid_helper_ok() { return Some(reply(false, "Set it up first (it asks for your Mac password once).")); }
            lid_save("enabled", J::Bool(en));
            if cfg!(target_os = "macos") { lid_tick(); }
            reply(true, if en { "On: it stays awake with the lid closed while plugged in." } else { "Off." })
        }
        _ => return None,
    })
}

// ---- start at login (macOS LaunchAgent running this engine)
const LABEL: &str = "com.createpanel.engine";
fn plist_path() -> PathBuf { home().join("Library/LaunchAgents").join(format!("{LABEL}.plist")) }
fn agent_log() -> PathBuf { home().join("Library/Logs/Create-Panel.log") }
fn xml(t: &str) -> String { t.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;") }
fn uid() -> String { sys::run("id", &["-u"], None).map(|x| String::from_utf8_lossy(&x.1).trim().to_string()).unwrap_or_else(|| "501".into()) }
fn autostart_enable() -> (bool, String) {
    if !cfg!(target_os = "macos") { return (false, "Start at login is for the Mac.".into()); }
    let Ok(exe) = std::env::current_exe().and_then(fs::canonicalize) else { return (false, "I can't find my own file.".into()) };
    let mut env = String::from("<key>PATH</key><string>/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin</string><key>PANEL_BACKGROUND</key><string>1</string>");
    for k in ["PANEL_DATA", "PANEL_PORT", "CURSEFORGE_API_KEY", "JAVA_HOME"] { if let Ok(v) = std::env::var(k) { env.push_str(&format!("<key>{k}</key><string>{}</string>", xml(&v))); } }
    let plist = format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\"><dict><key>Label</key><string>{LABEL}</string><key>ProgramArguments</key><array><string>{}</string></array><key>RunAtLoad</key><true/><key>KeepAlive</key><dict><key>SuccessfulExit</key><false/></dict><key>ThrottleInterval</key><integer>20</integer><key>ExitTimeOut</key><integer>120</integer><key>StandardOutPath</key><string>{}</string><key>StandardErrorPath</key><string>{}</string><key>EnvironmentVariables</key><dict>{env}</dict><key>WorkingDirectory</key><string>{}</string></dict></plist>\n", xml(&exe.to_string_lossy()), xml(&agent_log().to_string_lossy()), xml(&agent_log().to_string_lossy()), xml(&home().to_string_lossy()));
    let p = plist_path();
    if let Some(d) = p.parent() { let _ = fs::create_dir_all(d); }
    if let Some(d) = agent_log().parent() { let _ = fs::create_dir_all(d); }
    if let Err(e) = fs::write(&p, plist) { return (false, format!("Couldn't set that up: {e}")); }
    (true, "Done. The Create Panel engine will start by itself whenever you log in to this Mac. Your servers keep running even when the app window is closed.".into())
}
fn autostart_disable() -> (bool, String) {
    let p = plist_path();
    if p.is_file() {
        if std::env::var("PANEL_BACKGROUND").is_err() && !std::env::var("PANEL_NO_LAUNCHCTL").is_ok() { let _ = sys::run("launchctl", &["bootout", &format!("gui/{}/{LABEL}", uid())], None); }
        if let Err(e) = fs::remove_file(&p) { return (false, format!("Couldn't remove it: {e}")); }
    }
    (true, format!("Okay. It won't start with the Mac any more.{}", if std::env::var("PANEL_BACKGROUND").is_ok() { " It keeps running until you quit it." } else { "" }))
}

// ---- keep running with the lid closed (macOS pmset disablesleep, via a one-time sudoers helper)
const LID_SUDOERS: &str = "/etc/sudoers.d/create-panel";
static LID_OURS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
fn lid_file() -> PathBuf { home().join("Library/Application Support/Create Panel/lid_awake.json") }
fn lid_load() -> J { fs::read_to_string(lid_file()).ok().and_then(|t| json::parse(&t)).filter(|j| matches!(j, J::Obj(_))).unwrap_or(J::Obj(vec![])) }
fn lid_save(k: &str, v: J) {
    let mut m = match lid_load() { J::Obj(m) => m, _ => vec![] };
    if let Some(e) = m.iter_mut().find(|(a, _)| a == k) { e.1 = v; } else { m.push((k.to_string(), v)); }
    if let Some(d) = lid_file().parent() { let _ = fs::create_dir_all(d); }
    let _ = fs::write(lid_file(), J::Obj(m).to_string());
}
fn lid_out(prog: &str, args: &[&str]) -> (i32, String) { sys::run_both(prog, args).map(|(c, o)| (c, String::from_utf8_lossy(&o).to_string())).unwrap_or((1, String::new())) }
fn lid_on_ac() -> (bool, bool) { let (_, o) = lid_out("/usr/bin/pmset", &["-g", "batt"]); (o.contains("AC Power"), o.contains("InternalBattery")) }
fn lid_sleep_disabled() -> bool { let (_, o) = lid_out("/usr/bin/pmset", &["-g"]); o.lines().any(|l| { let t = l.trim(); t.starts_with("SleepDisabled") && t.split_whitespace().nth(1) == Some("1") }) }
fn lid_helper_ok() -> bool { lid_out("/usr/bin/sudo", &["-n", "-l", "/usr/bin/pmset", "-a", "disablesleep", "1"]).0 == 0 }
fn lid_apply(v: bool) -> bool {
    let ok = lid_out("/usr/bin/sudo", &["-n", "/usr/bin/pmset", "-a", "disablesleep", if v { "1" } else { "0" }]).0 == 0;
    if ok { LID_OURS.store(v, std::sync::atomic::Ordering::SeqCst); lid_save("applied", J::Bool(v)); }
    ok
}
fn lid_tick() {
    let d = lid_load();
    let want = matches!(d.get("enabled"), Some(J::Bool(true))) && lid_on_ac().0;
    let cur = lid_sleep_disabled();
    if want && !cur { lid_apply(true); }
    else if !want && cur && (matches!(d.get("applied"), Some(J::Bool(true))) || LID_OURS.load(std::sync::atomic::Ordering::SeqCst)) { lid_apply(false); }
}
fn lid_setup(remove: bool) -> Resp {
    if !cfg!(target_os = "macos") { return reply(false, "This only works on a Mac."); }
    let user = std::env::var("USER").unwrap_or_default();
    let cmd;
    let tmp = std::env::temp_dir().join(format!("create-panel-{}.sudoers", std::process::id()));
    if remove { cmd = format!("/usr/bin/pmset -a disablesleep 0; /bin/rm -f {LID_SUDOERS}"); }
    else {
        if user.is_empty() || !user.chars().all(|c| c.is_ascii_alphanumeric() || "._-".contains(c)) { return reply(false, &format!("I can't set this up for the user name {user:?}.")); }
        if fs::write(&tmp, format!("{user} ALL=(root) NOPASSWD: /usr/bin/pmset -a disablesleep 0, /usr/bin/pmset -a disablesleep 1\n")).is_err() { return reply(false, "Couldn't write a temp file."); }
        let q = tmp.to_string_lossy();
        cmd = format!("/usr/sbin/visudo -cf '{q}' && /usr/bin/install -m 0440 -o root -g wheel '{q}' {LID_SUDOERS}");
    }
    let script = format!("do shell script \"{}\" with administrator privileges", cmd.replace('\\', "\\\\").replace('"', "\\\""));
    let (rc, out) = lid_out("/usr/bin/osascript", &["-e", &script]);
    let _ = fs::remove_file(&tmp);
    if rc != 0 { return reply(false, &if out.contains("-128") { "Cancelled.".to_string() } else { format!("That didn't work: {}", out.trim().chars().take(200).collect::<String>()) }); }
    if remove { lid_save("enabled", J::Bool(false)); lid_save("applied", J::Bool(false)); }
    reply(true, "Done.")
}
pub fn start(app: &Arc<App>) {
    if app.role == crate::Role::Hub && cfg!(target_os = "macos") {
        thread::spawn(|| {
            let d = lid_load();
            if matches!(d.get("applied"), Some(J::Bool(true))) && !matches!(d.get("enabled"), Some(J::Bool(true))) && lid_sleep_disabled() { lid_apply(false); }
            loop { lid_tick(); thread::sleep(std::time::Duration::from_secs(10)); }
        });
    }
}
