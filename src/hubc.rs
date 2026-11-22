//! Hub: create a server, rename/remove, first-run setup, Java install, modpack search for new servers.
use crate::http::{Req, Resp};
use crate::hub::Entry;
use crate::json::{self, n, obj, s, J};
use crate::{dl, jobs, loaders, packs, sys, sysx, App};
use std::fs;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;

static CREATE_BUSY: Mutex<bool> = Mutex::new(false);

pub fn hub_root() -> PathBuf {
    match std::env::var("HUB_ROOT") { Ok(p) if !p.is_empty() => PathBuf::from(p), _ => sysx::home().join("create-panel-servers") }
}
fn slugify(name: &str) -> String {
    let mut o = String::new(); let mut dash = false;
    for c in name.to_lowercase().chars() { if c.is_ascii_alphanumeric() { o.push(c); dash = false; } else if !dash && !o.is_empty() { o.push('-'); dash = true; } }
    let o: String = o.trim_matches('-').chars().take(32).collect(); let o = o.trim_matches('-').to_string();
    if o.is_empty() { "server".into() } else { o }
}
fn port_free(p: u16) -> bool { TcpListener::bind(("0.0.0.0", p)).is_ok() }
fn pick_port(start: u16, taken: &[i64]) -> Result<u16, String> {
    for p in start..start.saturating_add(400) { if !taken.contains(&(p as i64)) && port_free(p) { return Ok(p); } }
    Err(format!("No free port found near {start}."))
}
pub fn in_root(d: &str) -> bool {
    let (Ok(root), Ok(real)) = (fs::canonicalize(hub_root()), fs::canonicalize(d)) else { return false };
    real != root && real.starts_with(&root)
}
pub fn set_props(path: &Path, updates: &[(&str, String)]) {
    let mut lines: Vec<String> = fs::read_to_string(path).map(|t| t.lines().map(|l| l.to_string()).collect()).unwrap_or_else(|_| vec!["# Minecraft server properties. Everything not listed uses the default.".into()]);
    let mut left: Vec<(&str, String)> = updates.to_vec();
    for l in lines.iter_mut() {
        if l.starts_with('#') { continue; }
        if let Some((k, _)) = l.split_once('=') { if let Some(i) = left.iter().position(|(a, _)| *a == k.trim()) { let (a, v) = left.remove(i); *l = format!("{a}={v}"); } }
    }
    for (k, v) in left { lines.push(format!("{k}={v}")); }
    let _ = fs::write(path, lines.join("\n") + "\n");
}
fn clean_name(t: &str) -> String { t.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(40).collect() }
fn jstr(j: &J, k: &str) -> String { j.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string() }
fn setup_flag(app: &App) -> PathBuf { app.hub.as_ref().unwrap().data_dir.join(".setup-done") }

struct Opts { name: String, loader: String, mc: String, ram: i64, pack: String, mc_port: Option<u16>, panel_port: Option<u16> }

fn create_worker(app: Arc<App>, job: jobs::Job, o: Opts) {
    let hub = app.hub.as_ref().unwrap().clone();
    let mut made: Option<PathBuf> = None;
    let r = (|| -> Result<(String, String), String> {
        let (mut loader, mut mc) = (o.loader.clone(), o.mc.clone());
        let entries = hub.list();
        let base = slugify(&o.name); let (mut sid, mut i) = (base.clone(), 2);
        while entries.iter().any(|e| e.id == sid) || sid == "main" || hub_root().join(&sid).exists() { sid = format!("{base}-{i}"); i += 1; }
        let d = hub_root().join(&sid);
        let mut pack: Option<packs::Pack> = None;
        if !o.pack.is_empty() {
            fs::create_dir_all(&d).map_err(|e| e.to_string())?; made = Some(d.clone());
            jobs::step(&job, "Downloading the modpack...", 2);
            let p = packs::pack_fetch(&o.pack, &d)?; loader = p.loader.clone(); mc = p.mc.clone();
            jobs::log(&job, &format!("Modpack: {} (Minecraft {mc}, {})", p.title, loaders::name(&loader))); pack = Some(p);
        }
        jobs::step(&job, "Finding Java (downloading it if needed)...", 3);
        let (java, jm) = sysx::find_java(sysx::required_java(&mc))?;
        jobs::log(&job, &format!("Using Java {jm} ({java})"));
        jobs::step(&job, &format!("Checking {} for Minecraft {mc}...", loaders::name(&loader)), 6);
        let res = loaders::resolve(&loader, &mc, pack.as_ref().map(|p| p.lv.as_str()))?;
        if made.is_none() { fs::create_dir_all(&d).map_err(|e| e.to_string())?; made = Some(d.clone()); }
        jobs::step(&job, &format!("Downloading {}...", loaders::name(&loader)), 10);
        let server_type = ["paper", "purpur", "vanilla"].contains(&loader.as_str());
        let jar = d.join(if server_type { "server.jar" } else if loader == "fabric" { "fabric-server-launch.jar" } else { "installer.jar" });
        dl::download(&res.url, &jar, dl::LOADER_HOSTS, true, None, None, 600)?;
        let launch: String;
        let ds = d.to_string_lossy().to_string();
        let tail = |j: &jobs::Job| { let l = jobs::get_obj(j).and_then(|v| v.get("lines").cloned()); match l { Some(J::Arr(a)) => a.iter().rev().take(3).rev().filter_map(|x| x.as_str().map(|t| t.to_string())).collect::<Vec<_>>().join(" | "), _ => String::new() } };
        if server_type { launch = "-jar server.jar".into(); }
        else if loader == "fabric" { launch = "-jar fabric-server-launch.jar".into(); }
        else if loader == "quilt" {
            jobs::step(&job, "Installing Quilt (downloads Minecraft, a minute or two)...", 55);
            let rc = loaders::run_logged(&job, &java, &["-jar", &jar.to_string_lossy(), "install", "server", &mc, "--download-server", "--install-dir=."], &d, 1200);
            if rc != 0 || !d.join("quilt-server-launch.jar").is_file() { return Err(format!("The Quilt installer failed. {}", tail(&job))); }
            let _ = fs::remove_file(&jar); launch = "-jar quilt-server-launch.jar".into();
        } else {
            jobs::step(&job, &format!("Installing {} (downloads Minecraft and libraries, a few minutes)...", loaders::name(&loader)), 55);
            let rc = loaders::run_logged(&job, &java, &["-jar", &jar.to_string_lossy(), "--installServer"], &d, 1800);
            let lib = if loader == "neoforge" { d.join("libraries/net/neoforged/neoforge").join(&res.version).join("unix_args.txt") } else { d.join("libraries/net/minecraftforge/forge").join(format!("{mc}-{}", res.version)).join("unix_args.txt") };
            if rc != 0 || !lib.is_file() { return Err(format!("The {} installer failed. {}", loaders::name(&loader), tail(&job))); }
            let _ = fs::remove_file(d.join("installer.jar")); let _ = fs::remove_file(d.join("installer.jar.log"));
            launch = format!("@{}", lib.strip_prefix(&d).unwrap_or(&lib).display());
        }
        let _ = ds;
        if let Some(p) = &pack { packs::pack_apply(&job, &d, p, false)?; }
        jobs::step(&job, "Writing the server files...", 96);
        let (ports, mcports) = hub.taken_ports();
        let mcport = match o.mc_port { Some(p) => p, None => pick_port(25565, &mcports)? };
        let taken: Vec<i64> = ports.iter().map(|p| *p as i64).chain([app.port as i64]).collect();
        let pport = match o.panel_port { Some(p) => p, None => pick_port(app.port + 1, &taken)? };
        fs::write(d.join("eula.txt"), "# Accepted in the Create Server Panel when this server was created: https://aka.ms/MinecraftEULA\neula=true\n").map_err(|e| e.to_string())?;
        fs::write(d.join("user_jvm_args.txt"), format!("# Written by the Create Server Panel. One Java option per line.\n-Xms{}G\n-Xmx{}G\n", (o.ram / 2).max(1), o.ram)).map_err(|e| e.to_string())?;
        let mut up: Vec<(&str, String)> = vec![("server-port", mcport.to_string()), ("query.port", mcport.to_string())];
        if !(pack.is_some() && d.join("server.properties").is_file()) { up.push(("motd", o.name.replace(['\r', '\n', '\\'], " "))); }
        set_props(&d.join("server.properties"), &up);
        loaders::write_start_script(&d, &java, &launch)?;
        if server_type { loaders::write_type_marker(&d, &loader, &mc, if loader == "vanilla" { "" } else { &res.version }); }
        hub.push_entry(Entry { id: sid.clone(), name: o.name.clone(), dir: d.display().to_string(), port: pport, settings: d.join(".panel-settings.json").display().to_string(), created: sys::now() });
        Ok((sid, o.name.clone()))
    })();
    match r {
        Ok((sid, name)) => { jobs::step(&job, "Done", 100); jobs::set(&job, "id", s(&sid)); jobs::finish(&job, true, &format!("{name} is ready. Press Start. The first start takes a few minutes while Minecraft sets up the world.")); }
        Err(e) => { if let Some(d) = made { if in_root(&d.to_string_lossy()) { let _ = fs::remove_dir_all(&d); } } jobs::finish(&job, false, &e); }
    }
    *CREATE_BUSY.lock().unwrap() = false;
}

fn remove(app: &App, id: &str, delete_files: bool) -> (bool, String) {
    let hub = app.hub.as_ref().unwrap();
    let Some(e) = hub.get(id) else { return (false, "That server isn't in the list any more.".into()) };
    if hub.state_of(id) != "stopped" && hub.state_of(id) != "panel_off" { return (false, "Stop the server first, then remove it.".into()); }
    if delete_files && !in_root(&e.dir) { return (false, "I only delete folders I created myself. Remove it from the list and delete the folder in Finder.".into()); }
    hub.stop_child(id, 60);
    if delete_files { let _ = crate::hubbk::snapshot_deleted(&e); }
    hub.forget(id);
    if delete_files {
        let trash = sysx::home().join(".Trash"); let mut moved = false;
        if trash.is_dir() { let base = Path::new(&e.dir).file_name().map(|x| x.to_string_lossy().to_string()).unwrap_or_else(|| "server".into()); moved = fs::rename(&e.dir, trash.join(format!("{base}-{}", sys::now() as i64))).is_ok(); }
        if !moved { let _ = fs::remove_dir_all(&e.dir); }
        return (true, format!("Removed {} and {} its files.", e.name, if moved { "moved" } else { "deleted" }));
    }
    (true, format!("Removed {} from the list. Its files are still in {}.", e.name, e.dir))
}

pub fn handle(app: &Arc<App>, req: &Req) -> Option<Resp> {
    let hub = app.hub.as_ref()?.clone();
    let body = || json::parse(&String::from_utf8_lossy(&req.body)).unwrap_or(J::Null);
    let rep = |ok: bool, m: &str, extra: Vec<(&str, J)>| { let mut v = vec![("ok", J::Bool(ok)), ("msg", s(m))]; v.extend(extra); Resp::json(200, obj(v).to_string()) };
    let err = |code: u16, m: &str| Resp::json(code, obj(vec![("ok", J::Bool(false)), ("error", s(m)), ("msg", s(m))]).to_string());
    Some(match (req.method.as_str(), req.path.as_str()) {
        ("GET", "/api/hub/versions") => match loaders::versions(req.q("loader").unwrap_or("")) { Ok(v) => Resp::json(200, obj(vec![("ok", J::Bool(true)), ("versions", J::Arr(v.iter().map(|(m, b)| obj(vec![("mc", s(m)), ("beta", J::Bool(*b))])).collect()))]).to_string()), Err(e) => err(502, &e) },
        ("GET", "/api/hub/job") => match jobs::get(req.q("id").unwrap_or("")) { Some(j) => Resp::json(200, j.to_string()), None => err(404, "Unknown job.") },
        ("GET", "/api/hub/packs/search") | ("GET", "/api/hub/packs/versions") => { let mut r2 = req.clone(); r2.path = req.path.replace("/api/hub/packs", "/api/packs"); return packs::handle(app, &r2); }
        ("GET", "/api/hub/setup") => {
            if !setup_flag(app).is_file() && !hub.list().is_empty() { let _ = fs::write(setup_flag(app), "existing"); }
            let mut majors: Vec<i64> = sysx::java_candidates().iter().filter_map(|p| sysx::java_major(p)).collect(); majors.sort(); majors.dedup();
            let brew = ["/opt/homebrew/bin/brew", "/usr/local/bin/brew"].iter().any(|p| Path::new(p).is_file()) || sys::which("brew").is_some();
            let desk = sysx::home().join("Desktop");
            Resp::json(200, obj(vec![("first_run", J::Bool(!setup_flag(app).is_file())), ("bundles", J::Arr(crate::hubbk::find_bundles())), ("java", J::Arr(majors.iter().map(|x| n(*x)).collect())), ("brew", J::Bool(brew)), ("mac", J::Bool(cfg!(target_os = "macos"))), ("hub_root", s(&hub_root().display().to_string())), ("servers", n(hub.list().len() as i64)), ("desktop", s(&(if desk.is_dir() { desk } else { sysx::home() }).display().to_string()))]).to_string())
        }
        ("POST", "/api/hub/setup/done") => { let _ = fs::write(setup_flag(app), "wizard"); rep(true, "Done.", vec![]) }
        ("POST", "/api/hub/rename") => {
            let b = body(); let name = clean_name(&jstr(&b, "name"));
            if name.is_empty() || hub.get(&jstr(&b, "id")).is_none() { rep(false, "Give it a name.", vec![]) } else { hub.rename(&jstr(&b, "id"), &name); rep(true, "Renamed.", vec![]) }
        }
        ("POST", "/api/hub/remove") => { let b = body(); let (ok, m) = remove(app, &jstr(&b, "id"), matches!(b.get("delete_files"), Some(J::Bool(true)))); rep(ok, &m, vec![]) }
        ("POST", "/api/hub/java/install") => {
            let b = body(); let mut vs: Vec<i64> = vec![];
            let given = match b.get("versions") { Some(J::Arr(a)) if !a.is_empty() => a.clone(), _ => vec![n(21)] };
            for x in given { let v = match x { J::Num(f) => f as i64, J::Str(t) => t.parse().unwrap_or(0), _ => 0 }; if [8, 17, 21, 25].contains(&v) && !vs.contains(&v) { vs.push(v); } }
            if vs.is_empty() { return Some(rep(false, "Pick a Java version.", vec![])); }
            let (id, job) = jobs::create("Install Java", true);
            thread::spawn(move || sysx::install_java(&job, &vs, "jdk"));
            rep(true, "Installing Java...", vec![("job", s(&id))])
        }
        ("POST", "/api/hub/brew/install") => {
            if !cfg!(target_os = "macos") { return Some(rep(false, "Homebrew is for macOS.", vec![])); }
            if ["/opt/homebrew/bin/brew", "/usr/local/bin/brew"].iter().any(|p| Path::new(p).is_file()) { return Some(rep(true, "Homebrew is already installed.", vec![])); }
            let cmd = "/bin/bash -c \"$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)\"";
            let scr = format!("tell application \"Terminal\"\nactivate\ndo script \"{}\"\nend tell", cmd.replace('\\', "\\\\").replace('"', "\\\""));
            match std::process::Command::new("osascript").args(["-e", &scr]).spawn() { Ok(_) => rep(true, "Opened Terminal. Follow the steps there.", vec![]), Err(e) => rep(false, &format!("Couldn't open Terminal: {e}"), vec![]) }
        }
        ("POST", "/api/hub/create") => {
            let b = body(); let name = clean_name(&jstr(&b, "name")); let mut loader = jstr(&b, "loader").to_lowercase(); let mut mc = jstr(&b, "mc").trim().to_string();
            if name.is_empty() { return Some(rep(false, "Give the server a name.", vec![])); }
            let pack = jstr(&b, "pack").trim().to_string();
            if !pack.is_empty() { if pack.len() < 6 || pack.len() > 12 || !pack.chars().all(|c| c.is_ascii_alphanumeric()) { return Some(rep(false, "Pick a modpack version.", vec![])); } loader = "neoforge".into(); mc = "1.21".into(); }
            else if !["neoforge", "forge", "fabric", "quilt", "paper", "purpur", "vanilla"].contains(&loader.as_str()) { return Some(rep(false, "Pick a mod loader.", vec![])); }
            else { let p: Vec<&str> = mc.split('.').collect(); if !((p.len() == 2 || p.len() == 3) && p.iter().all(|x| !x.is_empty() && x.chars().all(|c| c.is_ascii_digit()))) { return Some(rep(false, "Pick a Minecraft version.", vec![])); } }
            if !matches!(b.get("eula"), Some(J::Bool(true))) { return Some(rep(false, "You need to agree to the Minecraft EULA to run a server.", vec![])); }
            let ram = match b.get("ram") { Some(J::Num(x)) => *x as i64, Some(J::Str(t)) => t.parse().unwrap_or(4), _ => 4 }.clamp(1, 128);
            let mut ports: [Option<u16>; 2] = [None, None];
            for (i, k) in ["mc_port", "panel_port"].iter().enumerate() {
                let v = match b.get(k) { Some(J::Num(x)) => Some(*x as i64), Some(J::Str(t)) if !t.is_empty() => match t.parse::<i64>() { Ok(v) => Some(v), Err(_) => return Some(rep(false, "Ports must be numbers.", vec![])) }, _ => None };
                if let Some(v) = v {
                    if !(1024..=65535).contains(&v) { return Some(rep(false, "Ports must be between 1024 and 65535.", vec![])); }
                    if i == 1 && (v == app.port as i64 || hub.list().iter().any(|e| e.port as i64 == v)) { return Some(rep(false, &format!("Panel port {v} is already used."), vec![])); }
                    ports[i] = Some(v as u16);
                }
            }
            { let mut g = CREATE_BUSY.lock().unwrap(); if *g { return Some(rep(false, "Another server is still being created. Wait for it to finish.", vec![])); } *g = true; }
            let (id, job) = jobs::create(&name, true);
            let a = app.clone(); let o = Opts { name, loader, mc, ram, pack, mc_port: ports[0], panel_port: ports[1] };
            thread::spawn(move || create_worker(a, job, o));
            rep(true, "Creating...", vec![("job", s(&id))])
        }
        _ => return None,
    })
}
pub fn start(_app: &Arc<App>) {}

fn enc(t: &str) -> String { t.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-._~".contains(&b) { (b as char).to_string() } else { format!("%{:02X}", b) }).collect() }
/// `/api/s/<id>/<rest>` is forwarded to that server's own panel, so a remote app only needs the hub's address.
pub fn proxy(app: &Arc<App>, req: &Req) -> Option<Resp> {
    let rest = req.path.strip_prefix("/api/s/")?;
    let (id, tail) = rest.split_once('/').unwrap_or((rest, ""));
    let err = |code: u16, m: &str| Resp::json(code, obj(vec![("ok", J::Bool(false)), ("error", s(m)), ("msg", s(m))]).to_string());
    let hub = app.hub.as_ref()?;
    let Some(e) = hub.get(id) else { return Some(err(404, "That server isn't in the list any more.")) };
    let mut path = format!("/{tail}");
    if !req.query.is_empty() { path.push('?'); path.push_str(&req.query.iter().map(|(k, v)| format!("{}={}", enc(k), enc(v))).collect::<Vec<_>>().join("&")); }
    let c = crate::client::Client::new("127.0.0.1", e.port, hub.key());
    let xh: Vec<(String, String)> = ["x-path", "x-filename", "x-overwrite", "x-name"].iter().filter_map(|k| req.header(k).map(|v| (k.to_string(), v.to_string()))).collect();
    match c.raw_h(&req.method, &path, if req.method == "GET" { None } else { Some(&req.body) }, &xh) {
        Ok((code, body)) => {
            let ct: &'static str = match body.first() { Some(b'{') | Some(b'[') => "application/json; charset=utf-8", Some(0x50) if body.get(1) == Some(&0x4b) => "application/zip", Some(0x89) => "image/png", _ => "application/octet-stream" };
            Some(Resp { code, ctype: ct, body })
        }
        Err(_) => { let w = hub.why_of(id); Some(err(502, &if w.is_empty() { "Couldn't reach that server's panel. Give it a few seconds.".to_string() } else { format!("Couldn't reach that server's panel. {w}") })) }
    }
}
