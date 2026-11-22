//! Loader/server-software catalog (NeoForge, Forge, Fabric, Quilt, Paper, Purpur, Vanilla), installers, and the in-place updaters.
use crate::dl::{self, LOADER_HOSTS as H};
use crate::envinfo::{self, vkey};
use crate::http::{Req, Resp};
use crate::json::{self, n, obj, s, J};
use crate::{jobs, sys, sysx, App};
use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

pub fn name(l: &str) -> &'static str { match l { "neoforge" => "NeoForge", "forge" => "Forge", "fabric" => "Fabric", "quilt" => "Quilt", "paper" => "Paper", "purpur" => "Purpur", "vanilla" => "Vanilla", _ => "Server" } }
pub const TYPES: &[&str] = &["paper", "purpur", "vanilla"];
fn enc(t: &str) -> String { t.chars().map(|c| if c.is_ascii_alphanumeric() || "-._~".contains(c) { c.to_string() } else { format!("%{:02X}", c as u32) }).collect() }
fn jstr(j: &J, k: &str) -> Option<String> { j.get(k).and_then(|x| x.as_str().map(|v| v.to_string())) }
fn arr(j: &J) -> Vec<J> { if let J::Arr(a) = j { a.clone() } else { vec![] } }
fn mc_ok(t: &str) -> bool { let p: Vec<&str> = t.split('.').collect(); (p.len() == 2 || p.len() == 3) && p.iter().all(|x| !x.is_empty() && x.chars().all(|c| c.is_ascii_digit())) }

static VC: Mutex<Option<HashMap<String, (f64, Vec<(String, bool)>)>>> = Mutex::new(None);
static NEO: Mutex<(f64, Vec<String>)> = Mutex::new((0.0, vec![]));

/// NeoForge version -> Minecraft version. 21.1.172 -> 1.21.1, 26.3.0.86 -> 26.3
pub fn neo_mc(v: &str) -> Option<String> {
    let base = v.split('-').next()?.split('+').next()?;
    let nums: Vec<i64> = base.split('.').map(|x| x.parse().ok()).collect::<Option<Vec<_>>>()?;
    if nums.len() < 2 { return None; }
    if (20..=25).contains(&nums[0]) { return Some(format!("1.{}{}", nums[0], if nums[1] == 0 { String::new() } else { format!(".{}", nums[1]) })); }
    if nums[0] >= 26 && nums.len() >= 3 { return Some(format!("{}.{}{}", nums[0], nums[1], if nums[2] == 0 { String::new() } else { format!(".{}", nums[2]) })); }
    None
}
fn neo_versions() -> Result<Vec<String>, String> {
    { let c = NEO.lock().unwrap(); if sys::now() - c.0 < 600.0 && !c.1.is_empty() { return Ok(c.1.clone()); } }
    let d = dl::get_json("https://maven.neoforged.net/api/maven/versions/releases/net/neoforged/neoforge", H)?;
    let vs: Vec<String> = d.get("versions").map(arr).unwrap_or_default().iter().filter_map(|v| v.as_str().map(|x| x.to_string())).filter(|v| !v.contains("alpha") && !v.contains("snapshot")).collect();
    *NEO.lock().unwrap() = (sys::now(), vs.clone());
    Ok(vs)
}
fn sw_versions(kind: &str) -> Result<Vec<String>, String> {
    Ok(match kind {
        "paper" => { let d = dl::get_json("https://fill.papermc.io/v3/projects/paper", H)?; let mut v = vec![]; if let Some(J::Obj(m)) = d.get("versions") { for (_, g) in m { for x in arr(g) { if let Some(t) = x.as_str() { v.push(t.to_string()); } } } } v }
        "purpur" => { let d = dl::get_json("https://api.purpurmc.org/v2/purpur", H)?; let mut v: Vec<String> = d.get("versions").map(arr).unwrap_or_default().iter().filter_map(|x| x.as_str().map(|t| t.to_string())).collect(); v.reverse(); v }
        _ => { let d = dl::get_json("https://piston-meta.mojang.com/mc/game/version_manifest_v2.json", H)?; d.get("versions").map(arr).unwrap_or_default().iter().filter(|v| jstr(v, "type").as_deref() == Some("release")).filter_map(|v| jstr(v, "id")).collect() }
    })
}

/// Minecraft versions a server can be built for, newest first: (mc, beta)
pub fn versions(loader: &str) -> Result<Vec<(String, bool)>, String> {
    { let g = VC.lock().unwrap(); if let Some((t, v)) = g.as_ref().and_then(|m| m.get(loader)) { if sys::now() - t < 600.0 { return Ok(v.clone()); } } }
    let mut out: Vec<(String, bool)> = vec![];
    match loader {
        "fabric" | "quilt" => {
            let url = if loader == "fabric" { "https://meta.fabricmc.net/v2/versions/game" } else { "https://meta.quiltmc.org/v3/versions/game" };
            for g in arr(&dl::get_json(url, H)?) { if matches!(g.get("stable"), Some(J::Bool(true))) { if let Some(v) = jstr(&g, "version") { out.push((v, false)); } } }
        }
        "neoforge" => {
            let mut best: Vec<(String, bool)> = vec![];
            for v in neo_versions()? {
                let Some(mc) = neo_mc(&v) else { continue };
                let beta = v.contains("beta");
                match best.iter_mut().find(|b| b.0 == mc) { Some(b) => { if b.1 && !beta { b.1 = false; } } None => best.push((mc, beta)) }
            }
            out = best;
        }
        "forge" => {
            let d = dl::get_json("https://files.minecraftforge.net/net/minecraftforge/forge/promotions_slim.json", H)?;
            if let Some(J::Obj(m)) = d.get("promos") { for (k, _) in m { for suf in ["-latest", "-recommended"] { if let Some(mc) = k.strip_suffix(suf) { if !out.iter().any(|x| x.0 == mc) { out.push((mc.to_string(), false)); } } } } }
        }
        l if TYPES.contains(&l) => { out = sw_versions(l)?.into_iter().filter(|v| mc_ok(v)).map(|v| (v, false)).collect(); }
        _ => return Err("Unknown loader.".into()),
    }
    out.retain(|x| !x.0.is_empty() && x.0.chars().all(|c| c.is_ascii_digit() || c == '.') && { let k = vkey(&x.0); (k.len() >= 2 && k[0] == 1 && k[1] >= 17) || k.first().map(|a| *a >= 26).unwrap_or(false) });
    out.sort_by(|a, b| vkey(&b.0).cmp(&vkey(&a.0)));
    VC.lock().unwrap().get_or_insert_with(HashMap::new).insert(loader.to_string(), (sys::now(), out.clone()));
    Ok(out)
}

pub struct Res { pub version: String, pub url: String }
pub fn resolve(loader: &str, mc: &str, want: Option<&str>) -> Result<Res, String> {
    let want = want.filter(|w| !w.is_empty());
    match loader {
        "fabric" => {
            let lst = arr(&dl::get_json(&format!("https://meta.fabricmc.net/v2/versions/loader/{}", enc(mc)), H)?);
            let lv = want.map(|w| w.to_string()).or_else(|| lst.iter().find(|x| matches!(x.get("loader").and_then(|l| l.get("stable")), Some(J::Bool(true)))).and_then(|x| x.get("loader").and_then(|l| jstr(l, "version")))).or_else(|| lst.first().and_then(|x| x.get("loader").and_then(|l| jstr(l, "version"))));
            let inst = arr(&dl::get_json("https://meta.fabricmc.net/v2/versions/installer", H)?);
            let iv = inst.iter().find(|x| matches!(x.get("stable"), Some(J::Bool(true)))).and_then(|x| jstr(x, "version")).or_else(|| inst.first().and_then(|x| jstr(x, "version")));
            match (lv, iv) { (Some(l), Some(i)) => Ok(Res { url: format!("https://meta.fabricmc.net/v2/versions/loader/{}/{}/{}/server/jar", enc(mc), enc(&l), enc(&i)), version: l }), _ => Err(format!("Fabric doesn't have a server for Minecraft {mc}.")) }
        }
        "quilt" => {
            let inst = arr(&dl::get_json("https://meta.quiltmc.org/v3/versions/installer", H)?);
            let u = inst.first().and_then(|x| jstr(x, "url")).ok_or("Couldn't find the Quilt installer.")?;
            Ok(Res { version: "latest".into(), url: u })
        }
        "neoforge" => {
            let p: Vec<i64> = envinfo::vkey(mc).iter().map(|x| *x as i64).collect();
            if p.len() < 2 { return Err(format!("NeoForge doesn't have a build for Minecraft {mc}.")); }
            let prefix = if p[0] == 1 { format!("{}.{}.", p[1], p.get(2).copied().unwrap_or(0)) } else { format!("{}.{}.{}.", p[0], p[1], p.get(2).copied().unwrap_or(0)) };
            let mk = |v: &str| Res { version: v.to_string(), url: format!("https://maven.neoforged.net/releases/net/neoforged/neoforge/{v}/neoforge-{v}-installer.jar") };
            if let Some(w) = want { return Ok(mk(w)); }
            let cand: Vec<String> = neo_versions()?.into_iter().filter(|v| v.starts_with(&prefix)).collect();
            if cand.is_empty() { return Err(format!("NeoForge doesn't have a build for Minecraft {mc}.")); }
            let stable: Vec<&String> = cand.iter().filter(|v| !v.contains("beta")).collect();
            Ok(mk(stable.last().copied().unwrap_or(cand.last().unwrap())))
        }
        "forge" => {
            let d = dl::get_json("https://files.minecraftforge.net/net/minecraftforge/forge/promotions_slim.json", H)?;
            let pr = d.get("promos").cloned().unwrap_or(J::Null);
            let v = want.map(|w| w.to_string()).or_else(|| jstr(&pr, &format!("{mc}-recommended"))).or_else(|| jstr(&pr, &format!("{mc}-latest"))).ok_or_else(|| format!("Forge doesn't have a build for Minecraft {mc}."))?;
            Ok(Res { url: format!("https://maven.minecraftforge.net/net/minecraftforge/forge/{mc}-{v}/forge-{mc}-{v}-installer.jar"), version: v })
        }
        l if TYPES.contains(&l) => sw_resolve(l, mc),
        _ => Err("Unknown loader.".into()),
    }
}
fn sw_resolve(kind: &str, mc: &str) -> Result<Res, String> {
    let miss = || format!("{} doesn't have a server for Minecraft {mc}.", name(kind));
    let fix = |e: String| if e.contains("HTTP 404") { miss() } else { e };
    match kind {
        "paper" => {
            let bl = arr(&dl::get_json(&format!("https://fill.papermc.io/v3/projects/paper/versions/{}/builds", enc(mc)), H).map_err(fix)?);
            let b = bl.iter().find(|x| jstr(x, "channel").as_deref() == Some("STABLE")).or_else(|| bl.first()).ok_or_else(miss)?;
            let url = b.get("downloads").and_then(|d| d.get("server:default")).and_then(|d| jstr(d, "url")).ok_or_else(miss)?;
            Ok(Res { version: match b.get("id") { Some(J::Num(x)) => format!("{}", *x as i64), Some(J::Str(t)) => t.clone(), _ => String::new() }, url })
        }
        "purpur" => {
            let d = dl::get_json(&format!("https://api.purpurmc.org/v2/purpur/{}", enc(mc)), H).map_err(fix)?;
            let latest = match d.get("builds").and_then(|b| b.get("latest")) { Some(J::Str(t)) => t.clone(), Some(J::Num(x)) => format!("{}", *x as i64), _ => return Err(miss()) };
            Ok(Res { url: format!("https://api.purpurmc.org/v2/purpur/{}/{latest}/download", enc(mc)), version: latest })
        }
        _ => {
            let man = dl::get_json("https://piston-meta.mojang.com/mc/game/version_manifest_v2.json", H)?;
            let ent = man.get("versions").map(arr).unwrap_or_default().into_iter().find(|v| jstr(v, "id").as_deref() == Some(mc)).ok_or_else(miss)?;
            let d = dl::get_json(&jstr(&ent, "url").unwrap_or_default(), H)?;
            let url = d.get("downloads").and_then(|x| x.get("server")).and_then(|x| jstr(x, "url")).ok_or_else(miss)?;
            Ok(Res { version: mc.to_string(), url })
        }
    }
}
pub fn write_type_marker(dir: &Path, kind: &str, mc: &str, build: &str) { let _ = fs::write(dir.join(".server-type.json"), obj(vec![("type", s(kind)), ("mc", s(mc)), ("build", s(build))]).to_string()); }

pub fn write_start_script(dir: &Path, java: &str, args: &str) -> Result<(), String> {
    let body = format!("#!/bin/bash\n# Written by Create Panel. Starts this Minecraft server. Memory is set in user_jvm_args.txt.\ncd \"$(dirname \"$0\")\"\nJAVA={}\n[ -x \"$JAVA\" ] || JAVA=java\nexec \"$JAVA\" @user_jvm_args.txt {args} nogui \"$@\"\n", J::Str(java.to_string()).to_string());
    let p = dir.join("start.sh");
    fs::write(&p, body).map_err(|e| e.to_string())?;
    #[cfg(unix)] { use std::os::unix::fs::PermissionsExt; let _ = fs::set_permissions(&p, fs::Permissions::from_mode(0o755)); }
    Ok(())
}

/// Run a command, streaming its output lines into `job`. Returns the exit code.
pub fn run_logged(job: &jobs::Job, prog: &str, args: &[&str], cwd: &Path, timeout_s: u64) -> i32 {
    let Ok(mut ch) = Command::new(prog).args(args).current_dir(cwd).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn() else { return -1 };
    let pid = ch.id();
    let done = Arc::new(std::sync::atomic::AtomicBool::new(false));
    { let d = done.clone(); thread::spawn(move || { thread::sleep(Duration::from_secs(timeout_s)); if !d.load(std::sync::atomic::Ordering::SeqCst) { let _ = Command::new("kill").args(["-9", &pid.to_string()]).output(); } }); }
    let so = ch.stdout.take(); let se = ch.stderr.take();
    let j2 = job.clone();
    let t = se.map(|e| thread::spawn(move || { for l in BufReader::new(e).lines().flatten() { jobs::log(&j2, &l); } }));
    if let Some(o) = so { for l in BufReader::new(o).lines().flatten() { jobs::log(job, &l); } }
    if let Some(t) = t { let _ = t.join(); }
    let rc = ch.wait().ok().and_then(|s| s.code()).unwrap_or(-1);
    done.store(true, std::sync::atomic::Ordering::SeqCst);
    rc
}

// ---------------- the updater (per server)
struct Lu { phase: &'static str, msg: String, result: Option<J>, job: Option<jobs::Job> }
static LU: Mutex<Lu> = Mutex::new(Lu { phase: "idle", msg: String::new(), result: None, job: None });
const LOCKED: &str = "Updates are locked for this server. Turn off \"Lock updates\" under Overview > Options first.";
fn lu_jar_name(dir: &Path) -> String {
    let t = fs::read_to_string(dir.join("start.sh")).unwrap_or_default();
    if let Some(i) = t.find("-jar") {
        let r = t[i + 4..].trim_start().trim_start_matches('"');
        let f: String = r.chars().take_while(|c| c.is_ascii_alphanumeric() || "._ -".contains(*c)).collect();
        if f.ends_with(".jar") { return f; }
    }
    "server.jar".into()
}
fn swap(dest: &Path, new: &Path) -> Result<(), String> {
    if dest.exists() { fs::rename(dest, format!("{}.old", dest.display())).map_err(|e| e.to_string())?; }
    fs::rename(new, dest).map_err(|e| e.to_string())
}
fn replace_between(txt: &str, prefix: &str, with: &str) -> String {
    // replace the version segment after `prefix` (up to next '/' or whitespace) when it starts with a digit
    let mut out = String::new(); let mut rest = txt;
    while let Some(i) = rest.find(prefix) {
        out.push_str(&rest[..i + prefix.len()]);
        let after = &rest[i + prefix.len()..];
        let seg: String = after.chars().take_while(|c| *c != '/' && !c.is_whitespace()).collect();
        if seg.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false) && after[seg.len()..].starts_with('/') { out.push_str(with); rest = &after[seg.len()..]; } else { rest = after; }
    }
    out.push_str(rest); out
}

fn loader_update_worker(app: Arc<App>, kind: String, mc: String, want: String, backup: bool, job: jobs::Job) {
    let dir = app.dir.clone();
    let old_label = envinfo::detect_env(&dir, &envinfo::load_settings(&dir), false).2;
    let say = |m: &str| { LU.lock().unwrap().msg = m.to_string(); jobs::log(&job, m); };
    let mut tmp: Option<PathBuf> = None;
    let res = (|| -> Result<(Res, i64), String> {
        if backup && crate::misc::world_dir(&dir).is_dir() {
            say("Backing up the world first...");
            let (ok, _) = crate::misc::backup_async(&app);
            if ok { while crate::misc::backup_running() { thread::sleep(Duration::from_secs(1)); } if !crate::misc::backup_ok().unwrap_or(true) { return Err("The world backup failed, so I stopped before changing anything.".into()); } }
        }
        let (java, jm) = sysx::find_java(sysx::required_java(&mc))?;
        say(&format!("Looking up {} for Minecraft {mc}...", name(&kind)));
        let r = resolve(&kind, &mc, if want.is_empty() { None } else { Some(&want) })?;
        let t = dir.join(format!(".panel-update-{}", jobs::new_id())); fs::create_dir_all(&t).map_err(|e| e.to_string())?; tmp = Some(t.clone());
        match kind.as_str() {
            "forge" => {
                let inst = t.join("installer.jar");
                say("Downloading the Forge installer..."); dl::download(&r.url, &inst, H, true, None, None, 400)?;
                say(&format!("Installing Forge {} for Minecraft {mc} (downloads libraries, a few minutes)...", r.version));
                let rc = run_logged(&job, &java, &["-jar", &inst.to_string_lossy(), "--installServer"], &dir, 1200);
                let args = dir.join("libraries/net/minecraftforge/forge").join(format!("{mc}-{}", r.version)).join("unix_args.txt");
                if rc != 0 || !args.is_file() { return Err("The Forge installer failed. Nothing in start.sh was changed.".into()); }
                if let Ok(txt) = fs::read_to_string(dir.join("start.sh")) {
                    let nt = replace_between(&txt, "net/minecraftforge/forge/", &format!("{mc}-{}", r.version));
                    if nt != txt { let _ = fs::write(dir.join("start.sh"), nt); say("start.sh now launches the new Forge."); }
                    else if !txt.contains("minecraftforge/forge") { say("start.sh doesn't name a Forge version file, so I left it alone. If it launches run.sh, that now uses the new version."); }
                }
            }
            "quilt" => {
                let inst = t.join("installer.jar");
                say("Downloading the Quilt installer..."); dl::download(&r.url, &inst, H, true, None, None, 400)?;
                say(&format!("Installing Quilt for Minecraft {mc} (a minute or two)..."));
                let rc = run_logged(&job, &java, &["-jar", &inst.to_string_lossy(), "install", "server", &mc, "--download-server", "--install-dir=."], &dir, 1200);
                if rc != 0 || !dir.join("quilt-server-launch.jar").is_file() { return Err("The Quilt installer failed. Nothing else was changed.".into()); }
            }
            _ => {
                let new = t.join("new.jar");
                say(&format!("Downloading {}...", name(&kind))); dl::download(&r.url, &new, H, true, None, None, 400)?;
                let dest = dir.join(if kind == "fabric" { "fabric-server-launch.jar".to_string() } else { lu_jar_name(&dir) });
                say(&format!("Swapping in the new jar (the old one is kept as {}.old)...", dest.file_name().unwrap().to_string_lossy()));
                swap(&dest, &new)?;
                if TYPES.contains(&kind.as_str()) { write_type_marker(&dir, &kind, &mc, if kind == "vanilla" { "" } else { &r.version }); }
            }
        }
        // a new Minecraft version can need a newer Java
        if let Ok(txt) = fs::read_to_string(dir.join("start.sh")) {
            if let Some(l) = txt.lines().find(|l| l.starts_with("JAVA=")) {
                let raw = l[5..].trim(); let cur = if raw.starts_with('"') { json::parse(raw).and_then(|j| j.as_str().map(|x| x.to_string())) } else { Some(raw.to_string()) };
                if !cur.map(|c| sysx::java_major(&c).unwrap_or(0) >= sysx::required_java(&mc)).unwrap_or(false) {
                    let nt: Vec<String> = txt.split('\n').map(|x| if x.starts_with("JAVA=") { format!("JAVA={}", J::Str(java.clone()).to_string()) } else { x.to_string() }).collect();
                    let _ = fs::write(dir.join("start.sh"), nt.join("\n")); say(&format!("start.sh now uses Java {jm}."));
                }
            }
        }
        Ok((r, jm))
    })();
    if let Some(t) = tmp { let _ = fs::remove_dir_all(t); }
    envinfo::invalidate();
    match res {
        Ok((r, _)) => {
            let new_label = envinfo::detect_env(&dir, &envinfo::load_settings(&dir), false).2.unwrap_or_else(|| format!("{}, Minecraft {mc}", name(&kind)));
            app.mc.log(&format!("[panel] {}: {} -> Minecraft {mc}{}.", name(&kind), old_label.clone().unwrap_or_else(|| "unknown".into()), if !["latest", &mc[..]].contains(&r.version.as_str()) { format!(", {}", r.version) } else { String::new() }));
            let mut l = LU.lock().unwrap(); l.phase = "done"; l.msg = "Done".into();
            l.result = Some(obj(vec![("ok", J::Bool(true)), ("old", old_label.map(|x| s(&x)).unwrap_or(J::Null)), ("new", s(&new_label)), ("mc", s(&mc)), ("build", s(&r.version))]));
        }
        Err(e) => { app.mc.log(&format!("[panel] {} update failed: {e}", name(&kind))); let mut l = LU.lock().unwrap(); l.phase = "done"; l.msg = e.clone(); l.result = Some(obj(vec![("ok", J::Bool(false)), ("msg", s(&e))])); }
    }
}

fn reply(ok: bool, m: &str) -> Resp { Resp::json(200, obj(vec![("ok", J::Bool(ok)), ("msg", s(m))]).to_string()) }
fn err(code: u16, m: &str) -> Resp { Resp::json(code, obj(vec![("ok", J::Bool(false)), ("error", s(m)), ("msg", s(m))]).to_string()) }

// ---------------- NeoForge-specific updater (/api/neoforge/*)
const NF_MAVEN: &str = "https://maven.neoforged.net/releases/net/neoforged/neoforge";
struct Nf { phase: &'static str, msg: String, log: Vec<String>, result: Option<J>, version: String }
static NF: Mutex<Nf> = Mutex::new(Nf { phase: "idle", msg: String::new(), log: Vec::new(), result: None, version: String::new() });
static NFC: Mutex<(f64, Vec<String>)> = Mutex::new((0.0, vec![]));
fn nf_log(l: &str) { let mut g = NF.lock().unwrap(); g.log.push(l.chars().take(300).collect()); let n = g.log.len(); if n > 400 { g.log.drain(..100); } }
fn nf_set(m: &str) { NF.lock().unwrap().msg = m.to_string(); }
fn nf_type(v: &str) -> &'static str { let l = v.to_lowercase(); if l.contains("alpha") { "alpha" } else if l.contains("beta") { "beta" } else { "release" } }
fn nf_series(v: &str) -> String { let k: Vec<&str> = v.split('.').take(2).collect(); if k.len() == 2 && k.iter().all(|x| !x.is_empty() && x.chars().next().unwrap().is_ascii_digit()) { format!("{}.{}", k[0].chars().take_while(|c| c.is_ascii_digit()).collect::<String>(), k[1].chars().take_while(|c| c.is_ascii_digit()).collect::<String>()) } else { String::new() } }
fn nf_mc(v: &str) -> Option<String> { let k = vkey(v); if k.len() < 2 { return None; } if (19..=25).contains(&k[0]) { Some(format!("1.{}{}", k[0], if k[1] == 0 { String::new() } else { format!(".{}", k[1]) })) } else { None } }
fn nf_all(force: bool) -> Result<Vec<String>, String> {
    { let c = NFC.lock().unwrap(); if !force && !c.1.is_empty() && sys::now() - c.0 < 600.0 { return Ok(c.1.clone()); } }
    let data = match dl::get_bytes(&format!("{NF_MAVEN}/maven-metadata.xml"), H, 25) { Ok(d) => d, Err(e) => { let c = NFC.lock().unwrap(); if !c.1.is_empty() { return Ok(c.1.clone()); } return Err(format!("Couldn't reach the NeoForge download site. Check the internet connection. ({e})")); } };
    let t = String::from_utf8_lossy(&data).to_string();
    let mut vs: Vec<String> = vec![]; let mut rest = t.as_str();
    while let Some(i) = rest.find("<version>") { let a = &rest[i + 9..]; let Some(j) = a.find("</version>") else { break }; let v = a[..j].trim().to_string(); if v.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false) && v.contains('.') && !vs.contains(&v) { vs.push(v); } rest = &a[j..]; }
    vs.sort_by(|a, b| (vkey(b), if nf_type(b) != "release" { 0 } else { 1 }).cmp(&(vkey(a), if nf_type(a) != "release" { 0 } else { 1 })));
    if vs.is_empty() { return Err("The NeoForge version list came back empty. Try again in a minute.".into()); }
    *NFC.lock().unwrap() = (sys::now(), vs.clone());
    Ok(vs)
}
fn nf_current(dir: &Path) -> Option<String> {
    let base = dir.join("libraries/net/neoforged/neoforge");
    if let Some(sv) = envinfo::nf_script_version(dir) { if base.join(&sv).is_dir() { return Some(sv); } }
    let mut vs: Vec<String> = fs::read_dir(&base).ok()?.flatten().filter(|e| e.path().is_dir()).map(|e| e.file_name().to_string_lossy().to_string()).filter(|v| v.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false)).collect();
    vs.sort_by_key(|v| vkey(v)); vs.pop()
}
fn nf_worker(app: Arc<App>, version: String, backup: bool) {
    let dir = app.dir.clone();
    let mut tmp: Option<PathBuf> = None;
    let res = (|| -> Result<(Option<String>, bool, String), String> {
        if app.mc.running() { return Err("Stop the Minecraft server first.".into()); }
        let script = dir.join("start.sh");
        if !script.is_file() { return Err("Couldn't find start.sh in the server folder.".into()); }
        nf_set("Checking the version list...");
        if !nf_all(true)?.contains(&version) { return Err(format!("NeoForge doesn't list a version {version}.")); }
        let old = nf_current(&dir);
        nf_set("Looking for Java...");
        let mcv = nf_mc(&version).or_else(|| envinfo::env_cached(&dir).1).unwrap_or_else(|| "1.21".into());
        let (java, _) = sysx::find_java(sysx::required_java(&mcv))?;
        nf_log(&format!("Using Java: {java}"));
        if backup { let world = crate::misc::world_dir(&dir); if world.is_dir() { nf_set("Backing up the world first..."); if let Err(e) = crate::misc::run_backup(&app, &world) { return Err(format!("The world backup failed, so nothing was changed. ({e})")); } nf_log("World backup done."); } }
        let url = format!("{NF_MAVEN}/{}/neoforge-{version}-installer.jar", enc(&version));
        let sha1 = dl::get_bytes(&format!("{url}.sha1"), H, 20).ok().and_then(|b| String::from_utf8_lossy(&b).split_whitespace().next().map(|x| x.to_string())).filter(|h| h.len() == 40 && h.chars().all(|c| c.is_ascii_hexdigit()));
        if sha1.is_none() { nf_log("No checksum available for the installer, skipping that check."); }
        let t = dir.join(".panel-staging").join(format!("nf-{}", jobs::new_id())); fs::create_dir_all(&t).map_err(|e| e.to_string())?; tmp = Some(t.clone());
        let jar = t.join(format!("neoforge-{version}-installer.jar"));
        nf_set(&format!("Downloading the NeoForge {version} installer..."));
        dl::download(&url, &jar, H, true, sha1.as_deref(), None, 400)?; nf_log("Installer downloaded.");
        let jvm_file = dir.join("user_jvm_args.txt"); let jvm_before = fs::read(&jvm_file).ok();
        nf_set("Running the installer. This downloads libraries and can take a few minutes...");
        let (_, jb) = jobs::create("nf", false);
        let rc = { let jb2 = jb.clone(); let jar_s = jar.to_string_lossy().to_string(); let d2 = dir.clone(); let java2 = java.clone();
            let h = thread::spawn(move || run_logged(&jb2, &java2, &["-jar", &jar_s, "--installServer"], &d2, 1500));
            let mut seen = 0usize;
            loop { thread::sleep(Duration::from_millis(400)); if let Some(J::Arr(l)) = jobs::get_obj(&jb).and_then(|o| o.get("lines").cloned()) { for x in l.iter().skip(seen.min(l.len())) { if let Some(t) = x.as_str() { nf_log(t); } } seen = l.len(); } if h.is_finished() { break; } }
            h.join().unwrap_or(-1) };
        if rc != 0 { return Err(format!("The NeoForge installer failed (exit code {rc}). The log below shows why. Nothing in start.sh was changed.")); }
        if !dir.join("libraries/net/neoforged/neoforge").join(&version).join("unix_args.txt").is_file() { return Err(format!("The installer finished but its launch files for {version} are missing. start.sh was not changed.")); }
        if let Some(b) = jvm_before { if fs::read(&jvm_file).ok().as_ref() != Some(&b) { let _ = fs::write(&jvm_file, b); nf_log("Restored your user_jvm_args.txt."); } }
        nf_set(&format!("Pointing start.sh at NeoForge {version}..."));
        let txt = fs::read_to_string(&script).map_err(|e| e.to_string())?;
        let new = replace_between(&txt, "net/neoforged/neoforge/", &version);
        let (mut patched, mut note) = (false, String::new());
        if new != txt { let _ = fs::copy(&script, dir.join("start.sh.panel-bak")); fs::write(&script, new).map_err(|e| e.to_string())?; patched = true; nf_log("start.sh updated (the old one is kept as start.sh.panel-bak)."); }
        else if !txt.contains("neoforged/neoforge") { note = "start.sh doesn't name a NeoForge version file, so I left it alone. If it launches run.sh, that now uses the new version.".into(); }
        else { note = "start.sh already pointed at this version.".into(); }
        if let Ok(rd) = fs::read_dir(&dir) { for e in rd.flatten() { let f = e.file_name().to_string_lossy().to_string(); if f.starts_with("neoforge-") && f.ends_with("-installer.jar.log") { let _ = fs::remove_file(e.path()); } } }
        Ok((old, patched, note))
    })();
    if let Some(t) = tmp { let _ = fs::remove_dir_all(t); }
    envinfo::invalidate();
    let mut g = NF.lock().unwrap();
    match res {
        Ok((old, patched, note)) => { crate::misc::mods_touched(); drop(g); app.mc.log(&format!("[panel] NeoForge: {} -> {version}.", old.clone().unwrap_or_else(|| "none".into()))); let mut g = NF.lock().unwrap(); g.phase = "done"; g.msg = "Done.".into(); g.result = Some(obj(vec![("ok", J::Bool(true)), ("old", old.map(|x| s(&x)).unwrap_or(J::Null)), ("new", s(&version)), ("patched", J::Bool(patched)), ("note", s(&note))])); }
        Err(e) => { g.log.push(format!("ERROR: {e}")); g.phase = "done"; g.msg = "Failed.".into(); g.result = Some(obj(vec![("ok", J::Bool(false)), ("error", s(&e))])); drop(g); app.mc.log(&format!("[panel] NeoForge update failed: {e}")); }
    }
}

pub fn handle(app: &Arc<App>, req: &Req) -> Option<Resp> {
    let body = || json::parse(&String::from_utf8_lossy(&req.body)).unwrap_or(J::Null);
    let dir = &app.dir;
    Some(match (req.method.as_str(), req.path.as_str()) {
        ("GET", "/api/loader/info") => {
            let (loader, mc, label, _) = envinfo::env_cached(dir);
            let lstr = loader.clone().unwrap_or_default();
            if !["fabric", "quilt", "forge", "paper", "purpur", "vanilla"].contains(&lstr.as_str()) { return Some(Resp::json(200, obj(vec![("supported", J::Bool(false)), ("loader", loader.map(|x| s(&x)).unwrap_or(J::Null)), ("label", label.map(|x| s(&x)).unwrap_or(J::Null))]).to_string())); }
            let base = vec![("supported", J::Bool(true)), ("loader", s(&lstr)), ("mc", mc.map(|x| s(&x)).unwrap_or(J::Null)), ("label", label.map(|x| s(&x)).unwrap_or(J::Null)), ("name", s(name(&lstr)))];
            match versions(&lstr) {
                Ok(v) => { let mut o = base; o.push(("mcs", J::Arr(v.iter().take(120).map(|x| s(&x.0)).collect()))); o.push(("running", J::Bool(app.mc.running()))); Resp::json(200, obj(o).to_string()) }
                Err(e) => { let mut o = base; o.push(("mcs", J::Arr(vec![]))); o.push(("error", s(&e))); Resp::json(200, obj(o).to_string()) }
            }
        }
        ("GET", "/api/loader/builds") => {
            let loader = envinfo::env_cached(dir).0.unwrap_or_default();
            let mc = req.q("mc").unwrap_or("").trim().to_string();
            if !["fabric", "forge"].contains(&loader.as_str()) || !mc_ok(&mc) { return Some(Resp::json(200, obj(vec![("builds", J::Arr(vec![]))]).to_string())); }
            let r = (|| -> Result<Vec<J>, String> {
                if loader == "forge" {
                    let meta = dl::get_json("https://files.minecraftforge.net/net/minecraftforge/forge/maven-metadata.json", H)?;
                    let promos = dl::get_json("https://files.minecraftforge.net/net/minecraftforge/forge/promotions_slim.json", H)?;
                    let rec = promos.get("promos").and_then(|p| jstr(p, &format!("{mc}-recommended")));
                    let mut out = vec![];
                    for full in arr(&meta.get(&mc).cloned().unwrap_or(J::Null)).iter().rev() { let f = full.as_str().unwrap_or(""); let v = f.split_once('-').map(|x| x.1).unwrap_or(f).to_string(); out.push(obj(vec![("v", s(&v)), ("stable", J::Bool(Some(&v) == rec.as_ref()))])); }
                    out.truncate(25); return Ok(out);
                }
                let lst = arr(&dl::get_json(&format!("https://meta.fabricmc.net/v2/versions/loader/{}", enc(&mc)), H)?);
                Ok(lst.iter().take(25).filter_map(|x| x.get("loader").map(|l| obj(vec![("v", s(&jstr(l, "version").unwrap_or_default())), ("stable", J::Bool(matches!(l.get("stable"), Some(J::Bool(true)))))]))).collect())
            })();
            match r { Ok(b) => Resp::json(200, obj(vec![("builds", J::Arr(b))]).to_string()), Err(e) => Resp::json(200, obj(vec![("builds", J::Arr(vec![])), ("error", s(&e))]).to_string()) }
        }
        ("GET", "/api/loader/status") => {
            let g = LU.lock().unwrap();
            let log: Vec<J> = g.job.as_ref().and_then(|j| jobs::get_obj(j)).and_then(|o| o.get("lines").cloned()).map(|l| arr(&l)).unwrap_or_default();
            let k = log.len().saturating_sub(40);
            Resp::json(200, obj(vec![("phase", s(g.phase)), ("msg", s(&g.msg)), ("log", J::Arr(log[k..].to_vec())), ("result", g.result.clone().unwrap_or(J::Null))]).to_string())
        }
        ("POST", "/api/loader/update") => {
            if envinfo::load_settings(dir).updates_lock { return Some(reply(false, LOCKED)); }
            if app.mc.running() { return Some(reply(false, "Stop the server first, then update it.")); }
            if LU.lock().unwrap().phase == "running" { return Some(reply(false, "An update is already running.")); }
            let (loader, cur_mc, _, _) = envinfo::env_cached(dir);
            let loader = loader.unwrap_or_default();
            if !["fabric", "quilt", "forge", "paper", "purpur", "vanilla"].contains(&loader.as_str()) { return Some(reply(false, "I can't update this kind of server.")); }
            let b = body();
            let mc = jstr(&b, "mc").filter(|x| !x.is_empty()).or(cur_mc).unwrap_or_default().trim().to_string();
            if !mc_ok(&mc) { return Some(reply(false, "Pick a Minecraft version.")); }
            let mut want = jstr(&b, "version").unwrap_or_default().trim().to_string();
            if !want.is_empty() && !(want.len() <= 40 && want.chars().all(|c| c.is_ascii_alphanumeric() || ".+_-".contains(c))) { return Some(reply(false, "That version doesn't look right.")); }
            if !["fabric", "forge"].contains(&loader.as_str()) { want.clear(); }
            let backup = !matches!(b.get("backup"), Some(J::Bool(false)));
            let (_, job) = jobs::create("Update", false);
            { let mut l = LU.lock().unwrap(); l.phase = "running"; l.msg = "Starting...".into(); l.result = None; l.job = Some(job.clone()); }
            let a = app.clone(); thread::spawn(move || loader_update_worker(a, loader, mc, want, backup, job));
            reply(true, "Updating...")
        }
        ("GET", "/api/neoforge/status") => {
            let g = NF.lock().unwrap(); let k = g.log.len().saturating_sub(60);
            Resp::json(200, obj(vec![("phase", s(g.phase)), ("msg", s(&g.msg)), ("log", J::Arr(g.log[k..].iter().map(|x| s(x)).collect())), ("result", g.result.clone().unwrap_or(J::Null)), ("version", s(&g.version))]).to_string())
        }
        ("GET", "/api/neoforge/versions") => {
            let all_mc = req.q("all") == Some("1");
            let cur = nf_current(dir);
            let loader = envinfo::detect_env(dir, &envinfo::load_settings(dir), true).0;
            let ser = cur.as_deref().map(nf_series).unwrap_or_default();
            match nf_all(false) {
                Err(e) => err(502, &e),
                Ok(vs) => {
                    let mut items: Vec<J> = vec![]; let mut rec: Option<String> = None;
                    for v in vs { if !ser.is_empty() && !all_mc && nf_series(&v) != ser { continue; }
                        let ty = nf_type(&v); if rec.is_none() && ty == "release" && (ser.is_empty() || nf_series(&v) == ser) { rec = Some(v.clone()); }
                        items.push(obj(vec![("v", s(&v)), ("series", s(&nf_series(&v))), ("mc", nf_mc(&v).map(|x| s(&x)).unwrap_or(J::Null)), ("type", s(ty)), ("installed", J::Bool(Some(&v) == cur.as_ref()))]));
                        if items.len() >= 80 { break; } }
                    let rec = rec.or_else(|| items.first().and_then(|i| jstr(i, "v")));
                    Resp::json(200, obj(vec![("ok", J::Bool(true)), ("current", cur.clone().map(|x| s(&x)).unwrap_or(J::Null)), ("series", s(&ser)), ("mc", cur.as_deref().and_then(nf_mc).or_else(|| envinfo::env_cached(dir).1).map(|x| s(&x)).unwrap_or(J::Null)), ("loader", loader.map(|x| s(&x)).unwrap_or(J::Null)), ("versions", J::Arr(items)), ("recommended", rec.map(|x| s(&x)).unwrap_or(J::Null)), ("running", J::Bool(app.mc.running())), ("has_start", J::Bool(dir.join("start.sh").is_file()))]).to_string())
                }
            }
        }
        ("POST", "/api/neoforge/update") => {
            if envinfo::load_settings(dir).updates_lock { return Some(reply(false, LOCKED)); }
            let b = body(); let v = jstr(&b, "version").unwrap_or_default().trim().to_string();
            if v.is_empty() || v.len() > 41 || !v.chars().next().unwrap().is_ascii_digit() || !v.chars().all(|c| c.is_ascii_alphanumeric() || ".-+".contains(c)) { return Some(reply(false, "Pick a version first.")); }
            if NF.lock().unwrap().phase == "running" { return Some(reply(false, "An update is already running.")); }
            if app.mc.running() { return Some(reply(false, "Stop the Minecraft server first, then try again.")); }
            if crate::misc::backup_running() { return Some(reply(false, "A backup is still running. Try again in a minute.")); }
            { let mut g = NF.lock().unwrap(); g.phase = "running"; g.msg = "Starting...".into(); g.log.clear(); g.result = None; g.version = v.clone(); }
            let backup = !matches!(b.get("backup"), Some(J::Bool(false)));
            let a = app.clone(); thread::spawn(move || nf_worker(a, v, backup));
            reply(true, "Updating...")
        }
        _ => return None,
    })
}
pub fn start(_app: &Arc<App>) {}
