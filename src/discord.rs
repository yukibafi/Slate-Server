//! Discord bot manager (hub only): installs the bundled bot into a private Python, runs and watches it.
//! The token lives in ~/.create-panel-discord.json (never sent to a client).
use crate::client::Client;
use crate::http::{Req, Resp};
use crate::json::{self, n, obj, s, J};
use crate::{sysx, App};
use std::fs;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const BOT_SOURCE: &str = include_str!("discord_bot.py");
const PERMS: u64 = (1 << 4) | (1 << 10) | (1 << 11) | (1 << 14) | (1 << 16) | (1 << 28);
const REQS: &str = "discord.py>=2.4\npython-dotenv>=1.0\n";

struct St { child: Option<Child>, want: bool, started: Option<Instant>, fails: u32, note: String, inst_busy: bool, inst_msg: String, last_ml: String }
static ST: Mutex<St> = Mutex::new(St { child: None, want: false, started: None, fails: 0, note: String::new(), inst_busy: false, inst_msg: String::new(), last_ml: String::new() });

fn dir() -> PathBuf { sysx::home().join("create-discord-bot") }
fn cfg_file() -> PathBuf { sysx::home().join(".create-panel-discord.json") }
fn fp_file() -> PathBuf { sysx::home().join(".create-panel-fingerprints.json") }
fn venv_py() -> PathBuf { if cfg!(windows) { dir().join("venv").join("Scripts").join("python.exe") } else { dir().join("venv").join("bin").join("python") } }
fn ready() -> PathBuf { dir().join("venv").join(".bot-ready") }
fn installed() -> bool { venv_py().is_file() && ready().is_file() }
fn now() -> u64 { SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) }

#[derive(Clone)]
struct Cfg { token: String, allowed: String, autostart: bool, modlist: bool, welcome: bool, fingerprint: bool, startcmd: bool }
fn jb(j: &J, k: &str, d: bool) -> bool { match j.get(k) { Some(J::Bool(b)) => *b, _ => d } }
fn js(j: &J, k: &str) -> String { j.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string() }
fn load_cfg() -> Cfg {
    let j = fs::read_to_string(cfg_file()).ok().and_then(|t| json::parse(&t)).unwrap_or(J::Null);
    Cfg { token: js(&j, "token"), allowed: js(&j, "allowed").chars().filter(|c| c.is_ascii_digit() || *c == ',').take(400).collect(), autostart: jb(&j, "autostart", true), modlist: jb(&j, "modlist", true), welcome: jb(&j, "welcome", true), fingerprint: jb(&j, "fingerprint", true), startcmd: jb(&j, "startcmd", true) }
}
fn save_cfg(c: &Cfg) {
    let j = obj(vec![("token", s(&c.token)), ("allowed", s(&c.allowed)), ("autostart", J::Bool(c.autostart)), ("modlist", J::Bool(c.modlist)), ("welcome", J::Bool(c.welcome)), ("fingerprint", J::Bool(c.fingerprint)), ("startcmd", J::Bool(c.startcmd))]);
    let p = cfg_file(); let tmp = p.with_extension("json.tmp");
    let _ = fs::write(&tmp, j.to_string());
    #[cfg(unix)] { use std::os::unix::fs::PermissionsExt; let _ = fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600)); }
    let _ = fs::rename(&tmp, &p);
}

fn b64val(c: u8) -> Option<u32> { match c { b'A'..=b'Z' => Some((c - b'A') as u32), b'a'..=b'z' => Some((c - b'a' + 26) as u32), b'0'..=b'9' => Some((c - b'0' + 52) as u32), b'-' | b'+' => Some(62), b'_' | b'/' => Some(63), _ => None } }
fn client_id(tok: &str) -> String {
    let part = tok.split('.').next().unwrap_or("");
    let (mut acc, mut bits, mut out) = (0u32, 0u32, Vec::new());
    for c in part.bytes() { let Some(v) = b64val(c) else { return String::new() }; acc = (acc << 6) | v; bits += 6; if bits >= 8 { bits -= 8; out.push(((acc >> bits) & 0xff) as u8); acc &= (1 << bits) - 1; } }
    let t = String::from_utf8(out).unwrap_or_default();
    if !t.is_empty() && t.chars().all(|c| c.is_ascii_digit()) { t } else { String::new() }
}

fn running() -> bool { let mut g = ST.lock().unwrap(); match g.child.as_mut() { Some(c) => matches!(c.try_wait(), Ok(None)), None => false } }
fn log_tail(lines: usize) -> String {
    let b = fs::read(dir().join("bot.log")).unwrap_or_default();
    let b = if b.len() > 24000 { &b[b.len() - 24000..] } else { &b[..] };
    let mut t = String::from_utf8_lossy(b).to_string();
    let tok = load_cfg().token; if !tok.is_empty() { t = t.replace(&tok, "***"); }
    let v: Vec<&str> = t.lines().collect(); v[v.len().saturating_sub(lines)..].join("\n")
}
fn kill_stale() {
    let pf = dir().join("bot.pid");
    if let Some(pid) = fs::read_to_string(&pf).ok().and_then(|t| t.trim().parse::<u32>().ok()) {
        #[cfg(unix)] {
            let out = Command::new("ps").args(["-p", &pid.to_string(), "-o", "command="]).output().map(|o| String::from_utf8_lossy(&o.stdout).to_string()).unwrap_or_default();
            if out.contains(dir().join("bot.py").to_string_lossy().as_ref()) { let _ = Command::new("kill").arg(pid.to_string()).status(); thread::sleep(Duration::from_millis(600)); let _ = Command::new("kill").args(["-9", &pid.to_string()]).status(); }
        }
        #[cfg(windows)] { let _ = Command::new("taskkill").args(["/F", "/PID", &pid.to_string()]).stdout(Stdio::null()).stderr(Stdio::null()).status(); }
    }
    let _ = fs::remove_file(pf);
}

// ---- modlist.json (+ AutoModpack fingerprints) for the bot's /modlist and /fingerprint
fn hexish(t: &str) -> bool {
    let t = t.trim_matches(|c: char| !c.is_ascii_hexdigit() && c != ':');
    if t.contains(':') { let p: Vec<&str> = t.split(':').collect(); p.len() >= 32 && p.iter().all(|x| x.len() == 2 && x.chars().all(|c| c.is_ascii_hexdigit())) }
    else { t.len() >= 40 && t.chars().all(|c| c.is_ascii_hexdigit()) }
}
pub fn fp_scan(folder: &str) -> String {
    let Ok(b) = fs::read(PathBuf::from(folder).join("logs").join("latest.log")) else { return String::new() };
    let b = if b.len() > 3 << 20 { &b[b.len() - (3 << 20)..] } else { &b[..] };
    let t = String::from_utf8_lossy(b).to_string(); let lines: Vec<&str> = t.lines().collect();
    let mut found = String::new();
    for (i, ln) in lines.iter().enumerate() {
        if !ln.to_lowercase().contains("fingerprint") { continue; }
        if !lines[i.saturating_sub(3)..=i].iter().any(|x| x.to_lowercase().contains("automodpack")) { continue; }
        let pick = |l: &str| l.split_whitespace().map(|w| w.trim_matches(|c: char| !c.is_ascii_hexdigit() && c != ':').to_string()).find(|w| hexish(w));
        if let Some(h) = pick(ln).or_else(|| lines.get(i + 1).and_then(|l| pick(l))) { found = h; }
    }
    found
}
fn write_modlist(app: &Arc<App>, force: bool) {
    let Some(hub) = app.hub.as_ref() else { return };
    if !dir().is_dir() { return; }
    let mut known = fs::read_to_string(fp_file()).ok().and_then(|t| json::parse(&t)).unwrap_or(J::Null);
    let mut kv: Vec<(String, String)> = match &known { J::Obj(v) => v.iter().map(|(k, v)| (k.clone(), v.as_str().unwrap_or("").to_string())).collect(), _ => vec![] };
    let mut servers = vec![]; let mut dirty = false;
    for e in hub.list() {
        let c = Client::new("127.0.0.1", e.port, &app.key);
        let Ok(d) = c.get("/api/mods") else { continue };
        let mods: Vec<J> = match d.get("mods") { Some(J::Arr(a)) => a.iter().filter(|m| jb(m, "enabled", false) || jb(m, "client_only", false)).map(|m| {
            let name = { let a = js(m, "name"); if a.is_empty() { js(m, "file") } else { a } };
            let u = js(m, "url"); let u = if u.starts_with("https://") { u } else { String::new() };
            obj(vec![("n", s(&name.chars().take(80).collect::<String>())), ("v", s(&js(m, "version").chars().take(30).collect::<String>())), ("u", s(&u))]) }).collect(), _ => vec![] };
        let fp = fp_scan(&e.dir);
        if !fp.is_empty() { match kv.iter_mut().find(|(k, _)| *k == e.id) { Some(x) => { if x.1 != fp { x.1 = fp.clone(); dirty = true; } } None => { kv.push((e.id.clone(), fp.clone())); dirty = true; } } }
        let fpv = kv.iter().find(|(k, _)| *k == e.id).map(|x| x.1.clone()).unwrap_or_default();
        servers.push(obj(vec![("id", s(&e.id)), ("name", s(&e.name)), ("mods", J::Arr(mods)), ("fp", s(&fpv))]));
    }
    if dirty { known = J::Obj(kv.iter().map(|(k, v)| (k.clone(), s(v))).collect()); let _ = fs::write(fp_file(), known.to_string()); }
    let body = J::Arr(servers.clone()).to_string();
    { let mut g = ST.lock().unwrap(); if !force && g.last_ml == body { return; } g.last_ml = body; }
    let doc = obj(vec![("updated", n(now() as i64)), ("servers", J::Arr(servers))]);
    let tmp = dir().join("modlist.json.tmp");
    if fs::write(&tmp, doc.to_string()).is_ok() { let _ = fs::rename(&tmp, dir().join("modlist.json")); }
}

fn start_bot(app: &Arc<App>) -> (bool, String) {
    if running() { return (true, "The bot is already running.".into()); }
    let cfg = load_cfg();
    if cfg.token.is_empty() { return (false, "Add your bot token first.".into()); }
    if !installed() { return (false, "Install the bot first.".into()); }
    kill_stale();
    let d = dir(); let _ = fs::create_dir_all(&d);
    let _ = fs::write(d.join("features.json"), obj(vec![("modlist", J::Bool(cfg.modlist)), ("welcome", J::Bool(cfg.welcome)), ("fingerprint", J::Bool(cfg.fingerprint)), ("startcmd", J::Bool(cfg.startcmd))]).to_string());
    write_modlist(app, true);
    let py = d.join("bot.py");
    if fs::read_to_string(&py).map(|t| t != BOT_SOURCE).unwrap_or(true) { if let Err(e) = fs::write(&py, BOT_SOURCE) { return (false, format!("Couldn't write the bot: {e}")); } }
    let lp = d.join("bot.log");
    if fs::metadata(&lp).map(|m| m.len() > 1 << 20).unwrap_or(false) { let _ = fs::rename(&lp, d.join("bot.log.old")); }
    let Ok(mut lf) = fs::OpenOptions::new().create(true).append(true).open(&lp) else { return (false, "Couldn't open the bot log.".into()) };
    use std::io::Write; let _ = writeln!(lf, "\n--- starting (unix time {}) ---", now());
    let Ok(lf2) = lf.try_clone() else { return (false, "Couldn't open the bot log.".into()) };
    let mut cmd = Command::new(venv_py());
    cmd.args(["-u", "bot.py"]).current_dir(&d).env("DISCORD_TOKEN", &cfg.token).env("ALLOWED_USER_IDS", &cfg.allowed).env("HUB_PORT", app.port.to_string()).env("BOT_KEY", bot_key()).env("PYTHONUNBUFFERED", "1").stdin(Stdio::null()).stdout(lf).stderr(lf2);
    #[cfg(windows)] { use std::os::windows::process::CommandExt; cmd.creation_flags(0x08000000); }
    match cmd.spawn() {
        Ok(ch) => { let _ = fs::write(d.join("bot.pid"), ch.id().to_string()); let mut g = ST.lock().unwrap(); g.child = Some(ch); g.want = true; g.started = Some(Instant::now()); g.note.clear(); (true, "Starting the bot...".into()) }
        Err(e) => (false, format!("Couldn't start the bot: {e}")),
    }
}
fn stop_bot() {
    let ch = { let mut g = ST.lock().unwrap(); g.want = false; g.note.clear(); g.child.take() };
    if let Some(mut c) = ch { let _ = c.kill(); let _ = c.wait(); }
    kill_stale();
}

fn find_python() -> Option<(String, Vec<&'static str>)> {
    for (c, a) in [("python3", vec![]), ("python", vec![]), ("py", vec!["-3"])] {
        if Command::new(c).args(&a).arg("--version").stdout(Stdio::null()).stderr(Stdio::null()).status().map(|s| s.success()).unwrap_or(false) { return Some((c.to_string(), a)); }
    }
    None
}
fn last_line(b: &[u8]) -> String { String::from_utf8_lossy(b).lines().last().unwrap_or("unknown error").chars().take(160).collect() }
fn install_bot() -> (bool, String) {
    { let mut g = ST.lock().unwrap(); if g.inst_busy { return (false, "Already installing, one moment.".into()); } g.inst_busy = true; g.inst_msg = "Starting...".into(); }
    thread::spawn(|| {
        let say = |m: &str| ST.lock().unwrap().inst_msg = m.to_string();
        let run = || -> Result<(), String> {
            let d = dir(); fs::create_dir_all(&d).map_err(|e| e.to_string())?;
            fs::write(d.join("requirements.txt"), REQS).map_err(|e| e.to_string())?;
            if !venv_py().is_file() {
                say("Making a private Python for the bot...");
                let (c, a) = find_python().ok_or("Python 3 isn't installed. Install it from python.org (tick \"Add to PATH\"), then try again.")?;
                let o = Command::new(&c).args(&a).args(["-m", "venv"]).arg(d.join("venv")).output().map_err(|e| e.to_string())?;
                if !o.status.success() { return Err(format!("Couldn't set up Python: {}", last_line(&o.stderr))); }
            }
            say("Downloading discord.py (about a minute)...");
            let o = Command::new(venv_py()).args(["-m", "pip", "install", "-r"]).arg(d.join("requirements.txt")).env("PIP_DISABLE_PIP_VERSION_CHECK", "1").output().map_err(|e| e.to_string())?;
            if !o.status.success() { return Err(format!("Install failed: {}", last_line(&o.stderr))); }
            fs::write(ready(), now().to_string()).map_err(|e| e.to_string())?;
            Ok(())
        };
        match run() { Ok(()) => say("Installed."), Err(e) => say(&e) }
        ST.lock().unwrap().inst_busy = false;
    });
    (true, "Installing...".into())
}

fn status() -> J {
    let c = load_cfg(); let cid = if c.token.is_empty() { String::new() } else { client_id(&c.token) };
    let run = running();
    let (busy, msg, want, up, note) = { let g = ST.lock().unwrap(); (g.inst_busy, g.inst_msg.clone(), g.want, g.started.map(|t| t.elapsed().as_secs() as i64).unwrap_or(0), g.note.clone()) };
    obj(vec![("installed", J::Bool(installed())), ("installing", J::Bool(busy)), ("install_msg", s(&msg)), ("token_set", J::Bool(!c.token.is_empty())),
        ("invite", s(&if cid.is_empty() { String::new() } else { format!("https://discord.com/oauth2/authorize?client_id={cid}&scope=bot%20applications.commands&permissions={PERMS}") })),
        ("running", J::Bool(run)), ("want", J::Bool(want)), ("uptime", n(if run { up } else { 0 })), ("note", s(&note)), ("autostart", J::Bool(c.autostart)), ("allowed", s(&c.allowed)),
        ("modlist", J::Bool(c.modlist)), ("welcome", J::Bool(c.welcome)), ("fingerprint", J::Bool(c.fingerprint)), ("startcmd", J::Bool(c.startcmd)), ("log", s(&log_tail(60)))])
}

pub fn start(app: &Arc<App>) {
    let _ = bot_key();
    let a = app.clone();
    thread::spawn(move || {
        thread::sleep(Duration::from_secs(3));
        let c = load_cfg(); if c.autostart && !c.token.is_empty() && installed() { start_bot(&a); }
        let mut tick = 0u32;
        loop {
            thread::sleep(Duration::from_secs(5)); tick += 1;
            if tick % 24 == 4 { write_modlist(&a, false); }
            let dead = { let mut g = ST.lock().unwrap();
                let exited = match g.child.as_mut() { Some(c) => !matches!(c.try_wait(), Ok(None)), None => false };
                if !exited { if g.child.is_some() && g.started.map(|t| t.elapsed().as_secs() > 60).unwrap_or(false) { g.fails = 0; } false }
                else { let up = g.started.map(|t| t.elapsed().as_secs()).unwrap_or(0); g.child = None; let _ = fs::remove_file(dir().join("bot.pid"));
                    if !g.want { false } else { g.fails = if up < 60 { g.fails + 1 } else { 1 };
                        if g.fails >= 5 { g.want = false; g.note = "The bot stopped 5 times in a row, so I gave up. The log below usually says why (often a wrong token).".into(); false } else { true } } } };
            if dead { start_bot(&a); }
        }
    });
}

pub fn handle(app: &Arc<App>, req: &Req) -> Option<Resp> {
    let body = || json::parse(&String::from_utf8_lossy(&req.body)).unwrap_or(J::Null);
    let rep = |ok: bool, m: &str| Resp::json(200, obj(vec![("ok", J::Bool(ok)), ("msg", s(m))]).to_string());
    Some(match (req.method.as_str(), req.path.as_str()) {
        ("GET", "/api/hub/discord") => Resp::json(200, status().to_string()),
        ("POST", "/api/hub/discord/install") => { let (o, m) = install_bot(); rep(o, &m) }
        ("POST", "/api/hub/discord/token") => {
            let mut t = js(&body(), "token").trim().to_string();
            if t.to_lowercase().starts_with("bot ") { t = t[4..].trim().to_string(); }
            let mut c = load_cfg();
            if t.is_empty() { stop_bot(); c.token.clear(); save_cfg(&c); return Some(rep(true, "Token removed and the bot stopped.")); }
            let parts: Vec<&str> = t.split('.').collect();
            let okc = |x: &&str| !x.is_empty() && x.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
            if parts.len() != 3 || !parts.iter().all(okc) || t.len() < 40 || t.len() > 140 { return Some(rep(false, "That doesn't look like a bot token. Copy it again from the Bot page (Reset Token).")); }
            c.token = t; save_cfg(&c);
            if running() { stop_bot(); ST.lock().unwrap().fails = 0; let (o, m) = start_bot(app); rep(o, &m) } else { rep(true, "Token saved.") }
        }
        ("POST", "/api/hub/discord/action") => match js(&body(), "action").as_str() {
            "start" => { ST.lock().unwrap().fails = 0; let (o, m) = start_bot(app); rep(o, &m) }
            "stop" => { stop_bot(); rep(true, "Bot stopped.") }
            "restart" => { stop_bot(); ST.lock().unwrap().fails = 0; let (o, m) = start_bot(app); rep(o, &m) }
            _ => rep(false, "Unknown action."),
        },
        ("POST", "/api/hub/discord/config") => {
            let b = body(); let mut c = load_cfg();
            if b.get("autostart").is_some() { c.autostart = jb(&b, "autostart", true); }
            let mut feat = false;
            if b.get("modlist").is_some() { c.modlist = jb(&b, "modlist", true); feat = true; }
            if b.get("welcome").is_some() { c.welcome = jb(&b, "welcome", true); feat = true; }
            if b.get("fingerprint").is_some() { c.fingerprint = jb(&b, "fingerprint", true); feat = true; }
            if b.get("startcmd").is_some() { c.startcmd = jb(&b, "startcmd", true); feat = true; }
            let mut ids = false;
            if let Some(v) = b.get("allowed").and_then(|v| v.as_str()) {
                ids = true;
                let l: Vec<&str> = v.split(|c: char| !c.is_ascii_digit()).filter(|x| x.len() >= 15 && x.len() <= 22).collect(); c.allowed = l.join(",");
            }
            save_cfg(&c);
            let note = if running() && feat { " Restart the bot to add or remove the command." } else if running() && ids { " Restart the bot to use the new list." } else { "" };
            rep(true, &format!("Saved.{note}"))
        }
        // ---- the Discord bot's own, start-only access (it never gets the full panel key) ----
        ("GET", "/api/hub/bot/servers") => {
            let hub = app.hub.as_ref()?;
            let list: Vec<J> = hub.list().iter().map(|e| obj(vec![("id", s(&e.id)), ("name", s(&e.name)), ("state", s(&hub.state_of(&e.id)))])).collect();
            Resp::json(200, obj(vec![("ok", J::Bool(true)), ("enabled", J::Bool(load_cfg().startcmd)), ("servers", J::Arr(list))]).to_string())
        }
        ("POST", "/api/hub/bot/start") => {
            let hub = app.hub.as_ref()?;
            if !load_cfg().startcmd { return Some(Resp::json(403, obj(vec![("ok", J::Bool(false)), ("msg", s("The /start command is switched off in the panel."))]).to_string())); }
            let want = js(&body(), "id");
            let Some(e) = hub.list().into_iter().find(|e| e.id == want) else { return Some(rep(false, "That server isn't in the panel any more.")) };
            let st = hub.state_of(&e.id);
            if st == "online" || st == "starting" { return Some(Resp::json(200, obj(vec![("ok", J::Bool(true)), ("state", s(&st)), ("msg", s(&format!("{} is already {}.", e.name, if st == "online" { "online" } else { "starting" })))]).to_string())); }
            let (mut ok, mut m) = hub.action(&e.id, "start");
            if !ok && m.contains("panel is stopped") {
                let _ = hub.action(&e.id, "panel-start");
                for _ in 0..20 { thread::sleep(Duration::from_millis(500)); let r = hub.action(&e.id, "start"); ok = r.0; m = r.1; if ok || !(m.contains("Couldn't reach") || m.contains("panel is stopped")) { break; } }
            }
            Resp::json(200, obj(vec![("ok", J::Bool(ok)), ("state", s(&hub.state_of(&e.id))), ("msg", s(&m))]).to_string())
        }
        _ => return None,
    })
}

fn ct_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    let mut d = (a.len() ^ b.len()) as u8;
    for i in 0..b.len() { d |= a.get(i).copied().unwrap_or(0) ^ b[i]; }
    d == 0
}

/// True when this request is one of the two routes the bot may use AND it carries the bot's key.
pub fn bot_request_ok(req: &Req) -> bool {
    let route = (req.method == "GET" && req.path == "/api/hub/bot/servers") || (req.method == "POST" && req.path == "/api/hub/bot/start");
    route && ct_eq(req.header("x-key").unwrap_or(""), &bot_key())
}

/// A separate secret for the bot. It can list servers and start them, nothing else.
pub fn bot_key() -> String {
    use std::sync::OnceLock;
    static K: OnceLock<String> = OnceLock::new();
    K.get_or_init(|| {
        let f = sysx::home().join(".create-panel-botkey");
        if let Ok(k) = fs::read_to_string(&f) { let k = k.trim().to_string(); if k.len() >= 32 { return k; } }
        let mut bytes = [0u8; 16];
        let got = fs::File::open("/dev/urandom").and_then(|mut r| { use std::io::Read; r.read_exact(&mut bytes) }).is_ok();
        let mut k = String::new();
        if got { for b in bytes { k.push_str(&format!("{b:02x}")); } }
        else {
            use std::collections::hash_map::RandomState; use std::hash::{BuildHasher, Hasher};
            for i in 0..4u64 { let mut h = RandomState::new().build_hasher(); h.write_u64(now()); h.write_u64(std::process::id() as u64 + i); k.push_str(&format!("{:08x}", h.finish() as u32)); }
        }
        let _ = fs::write(&f, &k);
        #[cfg(unix)] { use std::os::unix::fs::PermissionsExt; let _ = fs::set_permissions(&f, fs::Permissions::from_mode(0o600)); }
        k
    }).clone()
}
