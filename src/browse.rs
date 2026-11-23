//! Mod browser (Modrinth + CurseForge), installs, and update checking.
use crate::dl;
use crate::envinfo;
use crate::http::{Req, Resp};
use crate::json::{self, n, obj, s, J};
use crate::mods::{self, clean_version, parse_filename};
use crate::{jobs, sys, App};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

const MR: &str = "https://api.modrinth.com/v2";
const CF: &str = "https://api.curseforge.com/v1";
const CF_GAME: i64 = 432;
const DL_HOSTS: &[&str] = &["cdn.modrinth.com", "edge.forgecdn.net", "mediafilez.forgecdn.net", "maven.neoforged.net"];
const MR_CATS: &[&str] = &["adventure", "decoration", "economy", "equipment", "food", "game-mechanics", "library", "magic", "management", "mobs", "optimization", "social", "storage", "technology", "transportation", "utility", "worldgen"];
const LOADER_NAMES: &[&str] = &["forge", "fabric", "quilt", "neoforge"];
const MR_UA: &str = "create-server-panel/3.0 (personal Minecraft server panel)";

pub fn cf_loader_id(l: &str) -> Option<i64> { match l { "forge" => Some(1), "fabric" => Some(4), "quilt" => Some(5), "neoforge" => Some(6), _ => None } }
pub fn cf_loader_tag(l: &str) -> Option<&'static str> { match l { "forge" => Some("Forge"), "fabric" => Some("Fabric"), "quilt" => Some("Quilt"), "neoforge" => Some("NeoForge"), _ => None } }
pub fn loaders_for(l: &str) -> Vec<String> { if l == "quilt" { vec!["quilt".into(), "fabric".into()] } else { vec![l.to_string()] } }
pub fn loaders_browse(l: &str) -> Vec<String> { let mut b = loaders_for(l); if l == "neoforge" { b.push("forge".into()); } b }

// ---- tiny JSON helpers
pub fn sv(j: &J, k: &str) -> String { match j.get(k) { Some(J::Str(t)) => t.clone(), Some(J::Num(x)) => format!("{}", *x as i64), _ => String::new() } }
pub fn so(j: &J, k: &str) -> Option<String> { match j.get(k) { Some(J::Str(t)) => Some(t.clone()), Some(J::Num(x)) => Some(format!("{}", *x as i64)), _ => None } }
pub fn iv(j: &J, k: &str) -> i64 { match j.get(k) { Some(J::Num(x)) => *x as i64, _ => 0 } }
pub fn av(j: &J, k: &str) -> Vec<J> { match j.get(k) { Some(J::Arr(a)) => a.clone(), _ => vec![] } }
pub fn sa(j: &J, k: &str) -> Vec<String> { av(j, k).iter().filter_map(|x| x.as_str().map(|t| t.to_string())).collect() }
pub fn jo(o: Option<String>) -> J { o.map(|x| s(&x)).unwrap_or(J::Null) }
pub fn reply(ok: bool, m: &str) -> Resp { Resp::json(200, obj(vec![("ok", J::Bool(ok)), ("msg", s(m))]).to_string()) }
pub fn enc(t: &str) -> String { t.chars().map(|c| if c.is_ascii_alphanumeric() || "-._~".contains(c) { c.to_string() } else { c.to_string().bytes().map(|b| format!("%{:02X}", b)).collect() }).collect() }
pub fn qs(v: &[(&str, String)]) -> String { v.iter().map(|(k, x)| format!("{}={}", k, enc(x))).collect::<Vec<_>>().join("&") }
pub fn jarr_str(v: &[String]) -> String { J::Arr(v.iter().map(|x| s(x)).collect()).to_string() }

// ---- HTTP to the two services
fn http_json(method: &str, url: &str, body: Option<&J>, who: &str) -> Result<J, String> {
    let b = body.map(|j| j.to_string());
    let (code, resp) = sys::http(method, &dl::map_url(url), &[("User-Agent", MR_UA), ("Accept", "application/json"), ("Content-Type", "application/json")], b.as_deref().map(|x| x.as_bytes()), 40);
    match code {
        0 => Err(format!("Couldn't reach {who}. Check the Mac's internet.")),
        200..=299 => json::parse(&String::from_utf8_lossy(&resp)).ok_or_else(|| format!("Couldn't reach {who}. Check the Mac's internet. (bad reply)")),
        404 => Err(format!("{who} couldn't find that (HTTP 404).")),
        429 => Err(format!("{who} is asking us to slow down. Wait a minute and try again.")),
        c => Err(format!("{who} said no (HTTP {c}). Try again in a minute.")),
    }
}
pub fn mr(method: &str, path: &str, body: Option<&J>) -> Result<J, String> { http_json(method, &format!("{MR}{path}"), body, "Modrinth") }

fn cf_key_file() -> PathBuf { crate::sysx::home().join(".create-panel-curseforge-key") }
pub fn cf_clean_key(k: &str) -> String {
    let strip = |t: &str| t.trim_matches(|c: char| "\"'`<> \t\r\n".contains(c)).to_string();
    let mut k = strip(k);
    let low = k.to_lowercase();
    for p in ["x-api-key", "api key", "api_key", "api-key", "apikey"] {
        if low.starts_with(p) { let rest = k[p.len()..].trim_start(); if rest.starts_with(':') || rest.starts_with('=') { k = rest[1..].trim().to_string(); break; } }
    }
    strip(&k)
}
static CFK: Mutex<(bool, Option<String>)> = Mutex::new((false, None));
pub fn cf_key() -> Option<String> {
    if let Ok(e) = std::env::var("CURSEFORGE_API_KEY") { let k = cf_clean_key(&e); if !k.is_empty() { return Some(k); } }
    let mut g = CFK.lock().unwrap();
    if !g.0 { g.0 = true; g.1 = fs::read_to_string(cf_key_file()).ok().map(|t| cf_clean_key(&t)).filter(|k| !k.is_empty()); }
    g.1.clone()
}
fn cf_http_error(code: u16, raw: &str) -> String {
    let snippet: String = { let mut o = String::new(); let mut intag = false; for c in raw.chars() { match c { '<' => intag = true, '>' => { intag = false; o.push(' ') } _ if !intag => o.push(c), _ => {} } } o.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(140).collect() };
    let low = raw.to_lowercase();
    match code {
        401 => "CurseForge rejected the API key (HTTP 401). Copy it again from console.curseforge.com > API keys.".into(),
        403 => {
            if low.contains("cloudflare") || low.contains("<html") || low.contains("attention required") || low.contains("just a moment") { format!("CurseForge's firewall blocked this request (HTTP 403), which isn't about your key. Turn off any VPN or ad-blocking DNS, or try another network, then press Connect again. {snippet}").trim().to_string() }
            else if low.contains("missing or invalid") { "CurseForge says 'API Key missing or invalid' (HTTP 403) for mod search, even though the key is fine for other things. This is a known CurseForge-side problem with some keys, not something in the panel. Fixes that have worked for others: (1) at console.curseforge.com > API keys, delete the key and generate a new one (sometimes it takes a couple of tries); (2) make sure the key comes from an organization on the developer console (make one if you don't have it); (3) if you've been retrying a lot, wait an hour since CurseForge rate limits; (4) as a last resort make a new CurseForge account and create the key there. Modrinth works without any key.".into() }
            else { format!("CurseForge refused the request (HTTP 403{}). Usually the key was copied with a missing or extra character, or it was just created and isn't active yet. Make a fresh key at console.curseforge.com > API keys, paste it, and wait a minute.", if snippet.is_empty() { String::new() } else { format!(": {snippet}") }).trim().to_string() }
        }
        404 => "CurseForge couldn't find that (HTTP 404).".into(),
        429 => "CurseForge is asking us to slow down. Wait a minute and try again.".into(),
        c => format!("CurseForge said no (HTTP {c}){}. Try again in a minute.", if snippet.is_empty() { String::new() } else { format!(": {snippet}") }),
    }
}
fn cf_request(method: &str, path: &str, key: &str, body: Option<&J>) -> Result<J, String> {
    let b = body.map(|j| j.to_string());
    let mut last = (403u16, String::new());
    for ua in [MR_UA, dl::BROWSER_UA] {
        let (code, resp) = sys::http(method, &dl::map_url(&format!("{CF}{path}")), &[("User-Agent", ua), ("Accept", "application/json"), ("x-api-key", key), ("Content-Type", "application/json")], b.as_deref().map(|x| x.as_bytes()), 30);
        if code == 0 { return Err("Couldn't reach CurseForge. Check the Mac's internet.".into()); }
        if (200..300).contains(&code) { return json::parse(&String::from_utf8_lossy(&resp)).ok_or_else(|| "Couldn't reach CurseForge. (bad reply)".to_string()); }
        last = (code, String::from_utf8_lossy(&resp).chars().take(2000).collect());
        if code == 403 && ua == MR_UA { continue; }
        return Err(cf_http_error(code, &last.1));
    }
    Err(cf_http_error(last.0, &last.1))
}
pub fn cf_json(method: &str, path: &str, body: Option<&J>) -> Result<J, String> {
    let k = cf_key().ok_or("Add your CurseForge API key first.")?;
    cf_request(method, path, &cf_clean_key(&k), body)
}
fn cf_set_key(k: &str) -> (bool, String) {
    let k = cf_clean_key(k);
    if k.is_empty() { let _ = fs::remove_file(cf_key_file()); *CFK.lock().unwrap() = (true, None); return (true, "CurseForge key removed.".into()); }
    if k.len() < 20 || k.len() > 200 || k.chars().any(char::is_whitespace) { return (false, "That doesn't look like a CurseForge API key.".into()); }
    if let Err(e) = cf_request("GET", &format!("/games/{CF_GAME}"), &k, None).and_then(|_| cf_request("GET", &format!("/mods/search?{}", qs(&[("gameId", CF_GAME.to_string()), ("classId", "6".into()), ("pageSize", "1".into())])), &k, None)) { return (false, e); }
    #[cfg(unix)] { use std::os::unix::fs::OpenOptionsExt; use std::io::Write; match fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(cf_key_file()).and_then(|mut f| f.write_all(k.as_bytes())) { Ok(_) => {} Err(e) => return (false, format!("Couldn't save the key: {e}")) } }
    #[cfg(not(unix))] { if let Err(e) = fs::write(cf_key_file(), &k) { return (false, format!("Couldn't save the key: {e}")); } }
    *CFK.lock().unwrap() = (true, Some(k));
    (true, "CurseForge connected.".into())
}

// ---- fingerprints (CurseForge murmur2 over whitespace-stripped bytes)
pub fn murmur2(data: &[u8], seed: u32) -> u32 {
    let (m, r) = (0x5bd1e995u32, 24);
    let len = data.len();
    let mut h = seed ^ (len as u32);
    let n4 = len - (len & 3);
    for c in data[..n4].chunks_exact(4) {
        let mut k = u32::from_le_bytes([c[0], c[1], c[2], c[3]]);
        k = k.wrapping_mul(m); k ^= k >> r; k = k.wrapping_mul(m);
        h = h.wrapping_mul(m); h ^= k;
    }
    let t = &data[n4..];
    if t.len() == 3 { h ^= (t[2] as u32) << 16; }
    if t.len() >= 2 { h ^= (t[1] as u32) << 8; }
    if !t.is_empty() { h ^= t[0] as u32; h = h.wrapping_mul(m); }
    h ^= h >> 13; h = h.wrapping_mul(m); h ^= h >> 15;
    h
}
pub fn cf_fingerprint(p: &Path) -> Option<u32> { let d = fs::read(p).ok()?; let f: Vec<u8> = d.into_iter().filter(|b| !matches!(b, b'\t' | b'\n' | b'\r' | b' ')).collect(); Some(murmur2(&f, 1)) }

// ---- in-memory mod metadata, shown in the mod list
static META: Mutex<Vec<(String, J, u64, u64)>> = Mutex::new(Vec::new()); // file, meta, size, mtime
pub fn meta_for(file: &str, size: u64, mtime: u128) -> Option<J> { META.lock().unwrap().iter().find(|m| m.0 == file && m.2 == size && m.3 == mtime as u64).map(|m| m.1.clone()) }
fn meta_put(file: &str, meta: J, size: u64, mtime: u64) { let mut g = META.lock().unwrap(); g.retain(|m| m.0 != file); g.push((file.to_string(), meta, size, mtime)); }
pub fn file_sig(p: &Path) -> Option<(u64, u64)> { let m = fs::metadata(p).ok()?; Some((m.len(), m.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs())) }
fn save_origin(app: &App, fname: &str, data: J) {
    let f = app.dir.join(".panel-origins.json");
    let mut m = match fs::read_to_string(&f).ok().and_then(|t| json::parse(&t)) { Some(J::Obj(m)) => m, _ => vec![] };
    let key = if fname.to_lowercase().ends_with(".disabled") { fname[..fname.len() - 9].to_string() } else { fname.to_string() };
    m.retain(|(k, _)| *k != key); m.push((key, data));
    let _ = fs::write(&f, J::Obj(m).to_string());
}

// ---- update state
struct Upd { phase: &'static str, msg: String, updates: Vec<J>, unknown: Vec<String>, nocompat: Vec<String>, uptodate: i64, done: i64, total: i64, result: J, env: String, checked_at: f64, bg_error: String }
static UPD: Mutex<Upd> = Mutex::new(Upd { phase: "idle", msg: String::new(), updates: Vec::new(), unknown: Vec::new(), nocompat: Vec::new(), uptodate: 0, done: 0, total: 0, result: J::Null, env: String::new(), checked_at: 0.0, bg_error: String::new() });
pub fn update_for(file: &str) -> J {
    let g = UPD.lock().unwrap();
    match g.updates.iter().find(|u| sv(u, "file") == file) { Some(u) => obj(vec![("from", u.get("from").cloned().unwrap_or(J::Null)), ("to", u.get("to").cloned().unwrap_or(J::Null)), ("type", u.get("type").cloned().unwrap_or(J::Null)), ("source", u.get("source").cloned().unwrap_or(J::Null)), ("manual", J::Bool(matches!(u.get("manual"), Some(J::Bool(true)))))]), None => J::Null }
}
pub fn update_summary() -> (usize, f64) { let g = UPD.lock().unwrap(); (g.updates.len(), g.checked_at) }
fn upd_set<F: FnOnce(&mut Upd)>(f: F) { f(&mut UPD.lock().unwrap()); }
pub fn file_hash(p: &Path) -> String { sys::sha1_file(p).unwrap_or_default() }
pub fn mods_dir(app: &App) -> PathBuf { app.dir.join("mods") }
fn mu_skipped(app: &App) -> HashSet<String> { crate::modupd::skipped(app) }

fn update_check_worker(app: Arc<App>) {
    let vis = || UPD.lock().unwrap().phase == "checking";
    let prog = |m: String| { if vis() { upd_set(|u| u.msg = m); } };
    let r = (|| -> Result<(), String> {
        let (loader, mc, _, _) = envinfo::env_cached(&app.dir);
        let (Some(loader), Some(mc)) = (loader, mc) else {
            if vis() { upd_set(|u| { u.phase = "error"; u.msg = "I can't tell which Minecraft version or loader this server uses yet. Set them in Settings > Panel (or start the server once), then try again.".into(); }); }
            return Ok(());
        };
        let mods: Vec<J> = mods::list_mods(&app).into_iter().filter(|m| matches!(m.get("enabled"), Some(J::Bool(true))) || matches!(m.get("client_only"), Some(J::Bool(true)))).collect();
        if mods.is_empty() { upd_set(|u| { u.updates.clear(); u.unknown.clear(); u.nocompat.clear(); u.uptodate = 0; u.checked_at = sys::now(); if u.phase == "checking" { u.phase = "error"; u.msg = "There are no mods to check.".into(); } }); return Ok(()); }
        prog(format!("Reading {} mods...", mods.len()));
        upd_set(|u| u.env = format!("{loader}, Minecraft {mc}"));
        let mut by_hash: Vec<(String, J)> = vec![];
        for m in &mods { by_hash.push((file_hash(&mods_dir(&app).join(sv(m, "file"))), m.clone())); }
        let hashes: Vec<String> = by_hash.iter().map(|x| x.0.clone()).collect();
        let (mut current, mut latest): (HashMap<String, J>, HashMap<String, J>) = (HashMap::new(), HashMap::new());
        for (i, ch) in hashes.chunks(100).enumerate() {
            prog(format!("Asking Modrinth about {}-{} of {} mods...", i * 100 + 1, (i * 100 + 100).min(hashes.len()), hashes.len()));
            let hs = J::Arr(ch.iter().map(|x| s(x)).collect());
            if let J::Obj(m) = mr("POST", "/version_files", Some(&obj(vec![("hashes", hs.clone()), ("algorithm", s("sha1"))])))? { for (k, v) in m { current.insert(k, v); } }
            let body = obj(vec![("hashes", hs), ("algorithm", s("sha1")), ("loaders", J::Arr(loaders_for(&loader).iter().map(|x| s(x)).collect())), ("game_versions", J::Arr(vec![s(&mc)]))]);
            if let J::Obj(m) = mr("POST", "/version_files/update", Some(&body))? { for (k, v) in m { latest.insert(k, v); } }
        }
        let mut pids: Vec<String> = current.values().filter_map(|c| so(c, "project_id")).collect(); pids.sort(); pids.dedup();
        let mut projects: HashMap<String, J> = HashMap::new();
        for ch in pids.chunks(80) { match mr("GET", &format!("/projects?ids={}", enc(&jarr_str(ch))), None) { Ok(J::Arr(a)) => for p in a { if let Some(id) = so(&p, "id") { projects.insert(id, p); } }, _ => break } }
        let (mut updates, mut unknown, mut nocompat, mut uptodate) = (vec![], vec![], vec![], 0i64);
        let mut fresh: Vec<(String, J, u64, u64)> = vec![];
        for (h, m) in &by_hash {
            let (cur, new) = (current.get(h), latest.get(h));
            let Some(cur) = cur else { unknown.push(m.clone()); continue };
            let pj = projects.get(&sv(cur, "project_id")).cloned().unwrap_or(J::Null);
            let file = sv(m, "file");
            if let Some((sz, mt)) = file_sig(&mods_dir(&app).join(&file)) { fresh.push((file.clone(), obj(vec![("source", s("modrinth")), ("project_id", jo(so(cur, "project_id"))), ("slug", pj.get("slug").cloned().unwrap_or(J::Null)), ("title", pj.get("title").cloned().unwrap_or(J::Null)), ("icon", pj.get("icon_url").cloned().unwrap_or(J::Null)), ("version_number", cur.get("version_number").cloned().unwrap_or(J::Null))]), sz, mt)); }
            let Some(new) = new else { nocompat.push(sv(m, "name")); continue };
            let files = av(new, "files");
            if sv(new, "id") == sv(cur, "id") || files.iter().any(|f| f.get("hashes").map(|x| sv(x, "sha1")) == Some(h.clone())) { uptodate += 1; continue; }
            if sv(new, "date_published") <= sv(cur, "date_published") { uptodate += 1; continue; }
            if sv(new, "version_type") == "alpha" && sv(cur, "version_type") != "alpha" { uptodate += 1; continue; }
            let pf = files.iter().find(|f| matches!(f.get("primary"), Some(J::Bool(true)))).or(files.first());
            let Some(pf) = pf.filter(|f| sv(f, "filename").to_lowercase().ends_with(".jar")) else { nocompat.push(sv(m, "name")); continue };
            let hs = pf.get("hashes").cloned().unwrap_or(J::Null);
            let ver = |x: &J| clean_version(&sv(x, "version_number"), Some(&mc), Some(&mc));
            updates.push(obj(vec![("file", s(&file)), ("name", s(&sv(m, "name"))), ("modid", s(&sv(m, "modid"))), ("old_sha1", s(h)), ("source", s("modrinth")),
                ("from", s(&ver(cur).or_else(|| so(m, "version")).unwrap_or_else(|| "?".into()))), ("to", s(&ver(new).unwrap_or_else(|| "?".into()))),
                ("type", s(&{ let t = sv(new, "version_type"); if t.is_empty() { "release".into() } else { t } })), ("url", s(&sv(pf, "url"))), ("filename", s(&sv(pf, "filename"))),
                ("sha1", jo(so(&hs, "sha1"))), ("sha512", jo(so(&hs, "sha512"))), ("size", n(iv(pf, "size"))), ("project_id", jo(so(cur, "project_id"))),
                ("slug", pj.get("slug").cloned().unwrap_or(J::Null)), ("title", pj.get("title").cloned().unwrap_or(J::Null)), ("icon", pj.get("icon_url").cloned().unwrap_or(J::Null)), ("manual", J::Bool(false))]));
        }
        let mut unknown_names: Vec<String>;
        if !unknown.is_empty() && cf_key().is_some() {
            match cf_check_unknown(&app, &unknown, &loader, &mc, &prog) {
                Ok((ups, meta, seen)) => {
                    let upfiles: HashSet<String> = ups.iter().map(|u| sv(u, "file")).collect();
                    uptodate += seen.iter().filter(|f| !upfiles.contains(*f)).count() as i64;
                    unknown_names = unknown.iter().filter(|m| !seen.contains(&sv(m, "file"))).map(|m| sv(m, "name")).collect();
                    updates.extend(ups); fresh.extend(meta);
                }
                Err(_) => unknown_names = unknown.iter().map(|m| sv(m, "name")).collect(),
            }
        } else { unknown_names = unknown.iter().map(|m| sv(m, "name")).collect(); }
        updates.sort_by_key(|u| sv(u, "name").to_lowercase());
        let skipped = mu_skipped(&app);
        updates.retain(|u| !skipped.contains(&sv(u, "modid").to_lowercase()));
        unknown_names.sort_by_key(|x| x.to_lowercase()); nocompat.sort_by_key(|x| x.to_lowercase());
        { let mut g = META.lock().unwrap(); g.clear(); g.extend(fresh); }
        upd_set(|u| if u.phase != "installing" { u.updates = updates; u.unknown = unknown_names; u.nocompat = nocompat; u.uptodate = uptodate; u.env = format!("{loader}, Minecraft {mc}"); u.checked_at = sys::now(); u.bg_error.clear(); if ["checking", "idle", "error", "ready"].contains(&u.phase) { u.phase = "ready"; u.msg.clear(); } });
        Ok(())
    })();
    if let Err(e) = r { upd_set(|u| if u.phase == "checking" { u.phase = "error"; u.msg = e } else { u.bg_error = e }); }
}

pub fn cf_type(f: &J) -> &'static str { match iv(f, "releaseType") { 2 => "beta", 3 => "alpha", _ => "release" } }
fn cf_check_unknown(app: &App, mods: &[J], loader: &str, mc: &str, prog: &dyn Fn(String)) -> Result<(Vec<J>, Vec<(String, J, u64, u64)>, HashSet<String>), String> {
    let mods = &mods[..mods.len().min(60)];
    let tid = cf_loader_id(loader);
    let mut fps: HashMap<i64, &J> = HashMap::new();
    for (i, m) in mods.iter().enumerate() {
        prog(format!("Looking at {} for CurseForge ({} of {})...", sv(m, "name"), i + 1, mods.len()));
        if let Some(f) = cf_fingerprint(&mods_dir(app).join(sv(m, "file"))) { fps.insert(f as i64, m); }
    }
    if fps.is_empty() { return Ok((vec![], vec![], HashSet::new())); }
    prog("Asking CurseForge...".into());
    let j = cf_json("POST", &format!("/fingerprints/{CF_GAME}"), Some(&obj(vec![("fingerprints", J::Arr(fps.keys().map(|k| n(*k)).collect()))])))?;
    let matches = j.get("data").map(|d| av(d, "exactMatches")).unwrap_or_default();
    let mut cur: Vec<(String, i64, i64, &J)> = vec![];
    for em in &matches { let f = em.get("file").cloned().unwrap_or(J::Null); if let Some(m) = fps.get(&iv(&f, "fileFingerprint")) { if iv(em, "id") != 0 { cur.push((sv(m, "file"), iv(em, "id"), iv(&f, "id"), *m)); } } }
    if cur.is_empty() { return Ok((vec![], vec![], HashSet::new())); }
    let mut ids: Vec<i64> = cur.iter().map(|c| c.1).collect(); ids.sort(); ids.dedup();
    let mut infos: HashMap<i64, J> = HashMap::new();
    for ch in ids.chunks(50) { for md in cf_json("POST", "/mods", Some(&obj(vec![("modIds", J::Arr(ch.iter().map(|x| n(*x)).collect()))])))?.get("data").map(|d| if let J::Arr(a) = d { a.clone() } else { vec![] }).unwrap_or_default() { infos.insert(iv(&md, "id"), md); } }
    let (mut meta, mut want): (Vec<(String, J, u64, u64)>, Vec<(String, i64)>) = (vec![], vec![]);
    for (fname, mid, fid, _) in &cur {
        let md = infos.get(mid).cloned().unwrap_or(J::Null);
        if let Some((sz, mt)) = file_sig(&mods_dir(app).join(fname)) { meta.push((fname.clone(), obj(vec![("source", s("curseforge")), ("project_id", s(&mid.to_string())), ("slug", md.get("slug").cloned().unwrap_or(J::Null)), ("title", md.get("name").cloned().unwrap_or(J::Null)), ("icon", md.get("logo").and_then(|l| l.get("thumbnailUrl").cloned()).unwrap_or(J::Null)), ("url", md.get("links").and_then(|l| l.get("websiteUrl").cloned()).unwrap_or(J::Null)), ("version_number", J::Null)]), sz, mt)); }
        let cands: Vec<J> = av(&md, "latestFilesIndexes").into_iter().filter(|x| sv(x, "gameVersion") == mc && (tid.is_none() || { let l = x.get("modLoader"); matches!(l, Some(J::Num(v)) if Some(*v as i64) == tid) || matches!(l, None | Some(J::Null)) || (loader == "quilt" && matches!(l, Some(J::Num(v)) if *v as i64 == 4)) })).collect();
        let rel: Vec<&J> = { let r: Vec<&J> = cands.iter().filter(|x| iv(x, "releaseType") == 1).collect(); if r.is_empty() { cands.iter().collect() } else { r } };
        if let Some(best) = rel.iter().max_by_key(|x| iv(x, "fileId")) { if iv(best, "fileId") > *fid { want.push((fname.clone(), iv(best, "fileId"))); } }
    }
    let seen: HashSet<String> = cur.iter().map(|c| c.0.clone()).collect();
    let mut updates = vec![];
    if !want.is_empty() {
        let mut fids: Vec<i64> = want.iter().map(|w| w.1).collect(); fids.sort(); fids.dedup();
        let mut files: HashMap<i64, J> = HashMap::new();
        for ch in fids.chunks(50) { for fd in cf_json("POST", "/mods/files", Some(&obj(vec![("fileIds", J::Arr(ch.iter().map(|x| n(*x)).collect()))])))?.get("data").map(|d| if let J::Arr(a) = d { a.clone() } else { vec![] }).unwrap_or_default() { files.insert(iv(&fd, "id"), fd); } }
        for (fname, fid) in &want {
            let Some(fd) = files.get(fid) else { continue };
            let c = cur.iter().find(|c| &c.0 == fname).unwrap(); let m = c.3; let md = infos.get(&c.1).cloned().unwrap_or(J::Null);
            let sha1 = av(fd, "hashes").iter().find(|h| iv(h, "algo") == 1).and_then(|h| so(h, "value"));
            let fnm = sv(fd, "fileName");
            let to = clean_version(&parse_filename(&fnm, Some(mc)).1.unwrap_or_default(), Some(mc), Some(mc)).or_else(|| clean_version(&sv(fd, "displayName"), Some(mc), Some(mc))).unwrap_or_else(|| "?".into());
            let url = sv(fd, "downloadUrl");
            updates.push(obj(vec![("file", s(fname)), ("name", s(&sv(m, "name"))), ("modid", s(&sv(m, "modid"))), ("old_sha1", s(&file_hash(&mods_dir(app).join(fname)))), ("source", s("curseforge")), ("from", s(&so(m, "version").unwrap_or_else(|| "?".into()))), ("to", s(&to)), ("type", s(cf_type(fd))),
                ("url", s(&url)), ("filename", s(&fnm)), ("sha1", jo(sha1)), ("sha512", J::Null), ("size", n(iv(fd, "fileLength"))), ("project_id", s(&c.1.to_string())), ("slug", md.get("slug").cloned().unwrap_or(J::Null)), ("title", md.get("name").cloned().unwrap_or(J::Null)),
                ("icon", md.get("logo").and_then(|l| l.get("thumbnailUrl").cloned()).unwrap_or(J::Null)), ("manual", J::Bool(url.is_empty())), ("page", md.get("links").and_then(|l| l.get("websiteUrl").cloned()).unwrap_or(J::Null))]));
        }
    }
    Ok((updates, meta, seen))
}

pub fn safe_name(f: &str) -> String { f.rsplit(['/', '\\']).next().unwrap_or("").chars().map(|c| if c.is_alphanumeric() || "_.+-[]() ".contains(c) { c } else { '_' }).collect::<String>().trim().to_string() }

fn update_apply_worker(app: Arc<App>, files: HashSet<String>) {
    let ups: Vec<J> = UPD.lock().unwrap().updates.iter().filter(|u| files.contains(&sv(u, "file")) && !matches!(u.get("manual"), Some(J::Bool(true)))).cloned().collect();
    let (mut ok, mut failed) = (vec![], vec![]);
    upd_set(|u| { u.phase = "installing"; u.total = ups.len() as i64; u.done = 0; u.msg = "Starting...".into(); u.result = J::Null; });
    let stg = app.dir.join(".panel-staging"); let _ = fs::create_dir_all(&stg);
    for (i, u) in ups.iter().enumerate() {
        upd_set(|g| { g.msg = format!("Downloading {} ({} of {})...", sv(u, "name"), i + 1, ups.len()); g.done = i as i64; });
        let tmp = stg.join(format!("upd-{}.jar", jobs::new_id()));
        let r = (|| -> Result<(String, J), String> {
            dl::download(&sv(u, "url"), &tmp, DL_HOSTS, true, so(u, "sha1").as_deref(), so(u, "sha512").as_deref(), 400)?;
            let mut name = safe_name(&sv(u, "filename"));
            if !name.to_lowercase().ends_with(".jar") || name.starts_with('.') { return Err(format!("Odd file name from {}, skipped.", if sv(u, "source") == "curseforge" { "CurseForge" } else { "Modrinth" })); }
            let _g = mods::MODS_LOCK.lock().unwrap();
            let old = mods::safe_mod_name(&app, &sv(u, "file")).ok_or("The old file is gone, skipped.")?;
            if file_hash(&mods_dir(&app).join(&old)) != sv(u, "old_sha1") { return Err("The file changed since the check, skipped.".into()); }
            if old.to_lowercase().ends_with(".disabled") || mods::parse_jar(&tmp, &name, None).client_only { name.push_str(".disabled"); }
            let clash = mods_dir(&app).join(&name);
            if clash.exists() && name != old { return Err(format!("{name} already exists, skipped.")); }
            mods::trash_files(&app, &[old]);
            fs::rename(&tmp, &clash).map_err(|e| e.to_string())?;
            if let Some((sz, mt)) = file_sig(&clash) { meta_put(&name, obj(vec![("source", u.get("source").cloned().unwrap_or(s("modrinth"))), ("project_id", u.get("project_id").cloned().unwrap_or(J::Null)), ("slug", u.get("slug").cloned().unwrap_or(J::Null)), ("title", u.get("title").cloned().unwrap_or(J::Null)), ("icon", u.get("icon").cloned().unwrap_or(J::Null)), ("version_number", u.get("to").cloned().unwrap_or(J::Null))]), sz, mt); }
            Ok((name.clone(), obj(vec![("name", s(&sv(u, "name"))), ("from", s(&sv(u, "from"))), ("to", s(&sv(u, "to"))), ("file", s(&name)), ("old", s(&sv(u, "file")))])))
        })();
        let _ = fs::remove_file(&tmp);
        match r { Ok((_, j)) => ok.push(j), Err(e) => failed.push(obj(vec![("name", s(&sv(u, "name"))), ("error", s(&e))])) }
    }
    if !ok.is_empty() { crate::misc::mods_touched(); }
    app.mc.log(&format!("[panel] Mod update: {} updated, {} failed. Old files are in mods-trash.", ok.len(), failed.len()));
    let gone: HashSet<String> = ok.iter().map(|o| sv(o, "old")).collect();
    upd_set(|g| { g.updates.retain(|u| !gone.contains(&sv(u, "file"))); g.phase = "done"; g.done = ups.len() as i64; g.msg.clear(); g.result = obj(vec![("ok", J::Arr(ok)), ("failed", J::Arr(failed))]); });
    crate::modupd::nudge();
}
fn locked_msg() -> &'static str { "Updates are locked for this server. Turn off \"Lock updates\" under Overview > Options first." }
pub fn start_check(app: &Arc<App>) -> (bool, String) {
    if envinfo::load_settings(&app.dir).updates_lock { return (false, locked_msg().into()); }
    { let mut g = UPD.lock().unwrap(); if g.phase == "installing" { return (false, "An update is being installed right now.".into()); } if g.phase == "checking" { return (true, "Checking...".into()); } g.phase = "checking"; g.msg = "Starting...".into(); g.done = 0; g.total = 0; g.result = J::Null; }
    let a = app.clone(); thread::spawn(move || update_check_worker(a));
    (true, "Checking...".into())
}
pub fn pending_files() -> Vec<String> { let g = UPD.lock().unwrap(); if g.phase != "ready" && g.phase != "done" { return vec![]; } g.updates.iter().filter(|u| !matches!(u.get("manual"), Some(J::Bool(true)))).map(|u| sv(u, "file")).collect() }
pub fn start_apply(app: &Arc<App>, files: HashSet<String>) -> (bool, String) {
    if envinfo::load_settings(&app.dir).updates_lock { return (false, locked_msg().into()); }
    { let mut g = UPD.lock().unwrap(); if g.phase != "ready" && g.phase != "done" { return (false, "Run the check first.".into()); }
      if !g.updates.iter().any(|u| files.contains(&sv(u, "file")) && !matches!(u.get("manual"), Some(J::Bool(true)))) { return (false, "Nothing selected.".into()); }
      g.phase = "installing"; }
    let a = app.clone(); thread::spawn(move || update_apply_worker(a, files));
    (true, "Installing...".into())
}
pub fn last_result_counts() -> (usize, usize) { let g = UPD.lock().unwrap(); (g.result.get("ok").map(|x| av(&obj(vec![("a", x.clone())]), "a").len()).unwrap_or(0), g.result.get("failed").map(|x| av(&obj(vec![("a", x.clone())]), "a").len()).unwrap_or(0)) }
pub fn drop_skipped(sk: &HashSet<String>) { UPD.lock().unwrap().updates.retain(|u| !sk.contains(&sv(u, "modid").to_lowercase())); }
pub fn phase() -> &'static str { UPD.lock().unwrap().phase }
pub fn pending_count() -> usize { UPD.lock().unwrap().updates.iter().filter(|u| !matches!(u.get("manual"), Some(J::Bool(true)))).count() }
fn update_public(app: &App) -> J {
    let g = UPD.lock().unwrap();
    let locked = envinfo::load_settings(&app.dir).updates_lock;
    let ups: Vec<J> = if locked { vec![] } else { g.updates.iter().map(|u| match u { J::Obj(m) => J::Obj(m.iter().filter(|(k, _)| !["url", "sha1", "sha512", "old_sha1"].contains(&k.as_str())).cloned().collect()), o => o.clone() }).collect() };
    obj(vec![("phase", s(g.phase)), ("msg", s(&g.msg)), ("updates", J::Arr(ups)), ("unknown", J::Arr(g.unknown.iter().map(|x| s(x)).collect())), ("nocompat", J::Arr(g.nocompat.iter().map(|x| s(x)).collect())), ("uptodate", n(g.uptodate)), ("done", n(g.done)), ("total", n(g.total)), ("result", g.result.clone()), ("env", s(&g.env)), ("checked_at", J::Num(g.checked_at)), ("bg_error", s(&g.bg_error)), ("cf", J::Bool(cf_key().is_some()))])
}
pub fn auto_loop_tick(app: &Arc<App>) {
    let st = envinfo::load_settings(&app.dir);
    let busy = matches!(phase(), "checking" | "installing");
    if st.update_check && !st.updates_lock && !busy && mods_dir(app).is_dir() && envinfo::env_cached(&app.dir).0.is_some() { update_check_worker(app.clone()); }
}

// ---- browse
fn compat_mr(v: &J, loader: &Option<String>, mc: &Option<String>) -> bool {
    let ls = sa(v, "loaders"); let gv = sa(v, "game_versions");
    loader.as_ref().map(|l| loaders_browse(l).iter().any(|x| ls.contains(x))).unwrap_or(true) && mc.as_ref().map(|m| gv.contains(m)).unwrap_or(true)
}
fn mc_versions(vs: &[String]) -> Vec<String> {
    let mut o: Vec<String> = vs.iter().filter(|v| { let p: Vec<&str> = v.split('.').collect(); (p.len() == 2 || p.len() == 3) && p.iter().all(|x| !x.is_empty() && x.chars().all(|c| c.is_ascii_digit())) }).cloned().collect();
    o.sort(); o.dedup(); o.sort_by(|a, b| envinfo::vkey(b).cmp(&envinfo::vkey(a))); o
}
pub fn installed_index(app: &App) -> HashMap<(String, String), J> {
    let mut idx = HashMap::new();
    for m in mods::list_mods(app) { if let (Some(p), Some(src)) = (so(&m, "project_id"), so(&m, "source")) { idx.insert((src, p), m); } }
    idx
}
pub fn arg(req: &Req, k: &str) -> String { req.q(k).unwrap_or("").to_string() }
fn browse_search(app: &App, req: &Req) -> Result<J, String> {
    let source = { let t = arg(req, "source"); if t.is_empty() { "modrinth".into() } else { t } };
    let (loader, mc, label, _) = envinfo::env_cached(&app.dir);
    let only = arg(req, "compat") != "0" && (loader.is_some() || mc.is_some());
    let limit = arg(req, "limit").parse::<i64>().unwrap_or(24).clamp(1, 40); let offset = arg(req, "offset").parse::<i64>().unwrap_or(0).max(0);
    let q: String = arg(req, "q").trim().chars().take(100).collect();
    let sort = { let t = arg(req, "sort"); if t.is_empty() { "relevance".into() } else { t } };
    let category: String = arg(req, "category").chars().take(40).collect();
    let (mut results, total): (Vec<J>, i64);
    if source == "curseforge" {
        let sorts = |x: &str| match x { "downloads" => 6, "updated" => 3, "newest" => 11, "name" => 4, _ => 2 };
        let mut p: Vec<(&str, String)> = vec![("gameId", CF_GAME.to_string()), ("classId", "6".into()), ("pageSize", limit.to_string()), ("index", offset.to_string()), ("sortField", sorts(&sort).to_string()), ("sortOrder", if sort == "name" { "asc".into() } else { "desc".into() })];
        if !q.is_empty() { p.push(("searchFilter", q.clone())); }
        if only { if let Some(m) = &mc { p.push(("gameVersion", m.clone())); } if let Some(id) = loader.as_deref().and_then(cf_loader_id) { p.push(("modLoaderType", id.to_string())); } }
        if !category.is_empty() && category.chars().all(|c| c.is_ascii_digit()) { p.push(("categoryId", category.clone())); }
        let j = cf_json("GET", &format!("/mods/search?{}", qs(&p)), None)?;
        let tid = loader.as_deref().and_then(cf_loader_id);
        results = av(&j, "data").iter().map(|m| {
            let idx = av(m, "latestFilesIndexes");
            let mut lds: Vec<&str> = idx.iter().filter_map(|x| match iv(x, "modLoader") { 1 => Some("forge"), 4 => Some("fabric"), 5 => Some("quilt"), 6 => Some("neoforge"), _ => None }).collect(); lds.sort(); lds.dedup();
            let compat = only || mc.as_ref().map(|mm| idx.iter().any(|x| sv(x, "gameVersion") == *mm && (tid.is_none() || matches!(x.get("modLoader"), Some(J::Num(v)) if Some(*v as i64) == tid) || matches!(x.get("modLoader"), None | Some(J::Null))))).unwrap_or(true);
            let authors = av(m, "authors");
            obj(vec![("source", s("curseforge")), ("id", s(&sv(m, "id"))), ("slug", m.get("slug").cloned().unwrap_or(J::Null)), ("name", m.get("name").cloned().unwrap_or(J::Null)), ("summary", m.get("summary").cloned().unwrap_or(J::Null)), ("icon", m.get("logo").and_then(|l| l.get("thumbnailUrl").cloned()).unwrap_or(J::Null)),
                ("author", authors.first().and_then(|a| a.get("name").cloned()).unwrap_or(J::Null)), ("downloads", n(iv(m, "downloadCount"))), ("updated", m.get("dateModified").cloned().unwrap_or(J::Null)), ("categories", J::Arr(av(m, "categories").iter().take(3).map(|c| c.get("name").cloned().unwrap_or(J::Null)).collect())),
                ("loaders", J::Arr(lds.iter().map(|x| s(x)).collect())), ("client", J::Null), ("server", J::Null), ("compat", J::Bool(compat)), ("manual", J::Bool(matches!(m.get("allowModDistribution"), Some(J::Bool(false))))),
                ("url", s(&m.get("links").and_then(|l| so(l, "websiteUrl")).filter(|u| !u.is_empty()).unwrap_or_else(|| format!("https://www.curseforge.com/minecraft/mc-mods/{}", sv(m, "slug")))))])
        }).collect();
        total = j.get("pagination").map(|p| iv(p, "totalCount")).unwrap_or(results.len() as i64).min(9900);
    } else {
        let mut facets: Vec<J> = vec![J::Arr(vec![s("project_type:mod")])];
        if only { if let Some(l) = &loader { facets.push(J::Arr(loaders_browse(l).iter().map(|x| s(&format!("categories:{x}"))).collect())); } if let Some(m) = &mc { facets.push(J::Arr(vec![s(&format!("versions:{m}"))])); } }
        if !category.is_empty() { facets.push(J::Arr(vec![s(&format!("categories:{category}"))])); }
        let sort = if ["relevance", "downloads", "follows", "updated", "newest"].contains(&sort.as_str()) { sort } else { "relevance".into() };
        let j = mr("GET", &format!("/search?{}", qs(&[("query", q.clone()), ("facets", J::Arr(facets).to_string()), ("limit", limit.to_string()), ("offset", offset.to_string()), ("index", sort)])), None)?;
        let skip = ["forge", "fabric", "quilt", "neoforge", "paper", "spigot", "bukkit", "purpur", "folia", "sponge", "velocity", "bungeecord", "waterfall", "datapack", "iris", "optifine", "minecraft", "vanilla", "canvas", "modloader", "rift", "liteloader"];
        results = av(&j, "hits").iter().map(|h| {
            let cats = sa(h, "categories");
            let lds: Vec<&String> = cats.iter().filter(|c| LOADER_NAMES.contains(&c.as_str())).collect();
            let compat = only || (loader.as_ref().map(|l| loaders_browse(l).iter().any(|x| cats.contains(x))).unwrap_or(true) && mc.as_ref().map(|m| sa(h, "versions").contains(m)).unwrap_or(true));
            let dc = { let d = sa(h, "display_categories"); if d.is_empty() { cats.clone() } else { d } };
            let shown: Vec<J> = dc.iter().filter(|c| !skip.contains(&c.as_str())).take(3).map(|c| s(c)).collect();
            obj(vec![("source", s("modrinth")), ("id", h.get("project_id").cloned().unwrap_or(J::Null)), ("slug", h.get("slug").cloned().unwrap_or(J::Null)), ("name", h.get("title").cloned().unwrap_or(J::Null)), ("summary", h.get("description").cloned().unwrap_or(J::Null)), ("icon", h.get("icon_url").cloned().unwrap_or(J::Null)), ("author", h.get("author").cloned().unwrap_or(J::Null)),
                ("downloads", n(iv(h, "downloads"))), ("updated", h.get("date_modified").cloned().unwrap_or(J::Null)), ("categories", J::Arr(shown)), ("loaders", J::Arr(lds.iter().map(|x| s(x)).collect())), ("client", h.get("client_side").cloned().unwrap_or(J::Null)), ("server", h.get("server_side").cloned().unwrap_or(J::Null)),
                ("compat", J::Bool(compat)), ("manual", J::Bool(false)), ("url", s(&format!("https://modrinth.com/mod/{}", so(h, "slug").or_else(|| so(h, "project_id")).unwrap_or_default())))])
        }).collect();
        total = j.get("total_hits").map(|_| iv(&j, "total_hits")).unwrap_or(results.len() as i64);
    }
    let inst = installed_index(app);
    for r in results.iter_mut() {
        let key = (sv(r, "source"), sv(r, "id"));
        let v = match inst.get(&key) { Some(m) => obj(vec![("file", m.get("file").cloned().unwrap_or(J::Null)), ("version", m.get("version").cloned().unwrap_or(J::Null)), ("enabled", m.get("enabled").cloned().unwrap_or(J::Null)), ("update", m.get("update").cloned().unwrap_or(J::Null))]), None => J::Null };
        if let J::Obj(m) = r { m.push(("installed".into(), v)); }
    }
    Ok(obj(vec![("results", J::Arr(results)), ("total", n(total)), ("offset", n(offset)), ("env", obj(vec![("loader", jo(loader)), ("mc", jo(mc)), ("label", jo(label)), ("filtered", J::Bool(only))]))]))
}
fn cf_files_ok(f: &J, mc: &Option<String>, loader: &Option<String>) -> bool {
    let gvs = sa(f, "gameVersions");
    mc.as_ref().map(|m| gvs.contains(m)).unwrap_or(true) && loader.as_ref().map(|l| cf_loader_tag(l).map(|t| gvs.iter().any(|g| g == t)).unwrap_or(false) || (l == "quilt" && gvs.iter().any(|g| g == "Fabric"))).unwrap_or(true)
}
/// Markdown / HTML -> short readable plain text (std only).
fn plain(t: &str, max: usize) -> String {
    let mut out = String::new(); let mut in_tag = false; let mut last_blank = false;
    let t = t.replace("\r", "");
    for raw in t.lines() {
        let mut line = String::new(); in_tag = in_tag && !raw.contains('>');
        if in_tag { continue; }
        for ch in raw.chars() { match ch { '<' => in_tag = true, '>' if in_tag => in_tag = false, _ if in_tag => {} _ => line.push(ch) } }
        // images and links: ![alt](url) -> "", [text](url) -> text
        let mut clean = String::new(); let cs: Vec<char> = line.chars().collect(); let mut i = 0;
        while i < cs.len() {
            if cs[i] == '!' && cs.get(i + 1) == Some(&'[') { if let Some(c) = cs[i..].iter().position(|&c| c == ']') { if cs.get(i + c + 1) == Some(&'(') { if let Some(e) = cs[i + c..].iter().position(|&c| c == ')') { i += c + e + 1; continue; } } } }
            if cs[i] == '[' { if let Some(c) = cs[i..].iter().position(|&c| c == ']') { if cs.get(i + c + 1) == Some(&'(') { if let Some(e) = cs[i + c..].iter().position(|&c| c == ')') { clean.extend(cs[i + 1..i + c].iter()); i += c + e + 1; continue; } } } }
            clean.push(cs[i]); i += 1;
        }
        let l = clean.trim().trim_start_matches('#').trim().replace("**", "").replace("__", "").replace('`', "").replace("&nbsp;", " ").replace("&amp;", "&").replace("&quot;", "\"");
        let l = if l.starts_with("- ") || l.starts_with("* ") { format!("\u{2022} {}", &l[2..]) } else { l };
        if l.is_empty() || l.chars().all(|c| matches!(c, '-' | '=' | '_' | '*' | '|' | ' ')) { if !last_blank && !out.is_empty() { out.push('\n'); last_blank = true; } continue; }
        last_blank = false; out.push_str(&l); out.push('\n');
        if out.chars().count() > max { break; }
    }
    let o = out.trim().to_string();
    if o.chars().count() > max { let cut: String = o.chars().take(max).collect(); format!("{}...", cut.trim_end()) } else { o }
}

fn browse_detail(req: &Req) -> Result<J, String> {
    let source = { let t = arg(req, "source"); if t.is_empty() { "modrinth".into() } else { t } };
    let id = arg(req, "id"); if id.is_empty() { return Err("Missing mod id.".into()); }
    if source == "curseforge" {
        let m = cf_json("GET", &format!("/mods/{}", enc(&id)), None)?; let d = m.get("data").cloned().unwrap_or(J::Null);
        let html = cf_json("GET", &format!("/mods/{}/description", enc(&id)), None).ok().map(|j| sv(&j, "data")).unwrap_or_default();
        let mut gv: Vec<String> = av(&d, "latestFilesIndexes").iter().map(|x| sv(x, "gameVersion")).filter(|x| !x.is_empty()).collect(); gv.sort(); gv.dedup(); gv.reverse();
        return Ok(obj(vec![("name", s(&sv(&d, "name"))), ("summary", s(&sv(&d, "summary"))), ("body", s(&plain(&html, 2200))), ("categories", J::Arr(av(&d, "categories").iter().map(|c| s(&sv(c, "name"))).collect())), ("loaders", J::Arr(vec![])), ("mc", J::Arr(gv.iter().take(10).map(|x| s(x)).collect())), ("client", J::Null), ("server", J::Null), ("downloads", d.get("downloadCount").cloned().unwrap_or(J::Null)), ("updated", d.get("dateModified").cloned().unwrap_or(J::Null)), ("url", s(&d.get("links").map(|l| sv(l, "websiteUrl")).unwrap_or_default()))]));
    }
    let p = mr("GET", &format!("/project/{}", enc(&id)), None)?;
    let mut gv = sa(&p, "game_versions"); gv.reverse();
    Ok(obj(vec![("name", s(&sv(&p, "title"))), ("summary", s(&sv(&p, "description"))), ("body", s(&plain(&sv(&p, "body"), 2200))), ("categories", J::Arr(sa(&p, "categories").iter().filter(|c| !LOADER_NAMES.contains(&c.as_str())).map(|x| s(x)).collect())), ("loaders", J::Arr(sa(&p, "loaders").iter().filter(|x| LOADER_NAMES.contains(&x.as_str())).map(|x| s(x)).collect())), ("mc", J::Arr(gv.iter().take(10).map(|x| s(x)).collect())), ("client", p.get("client_side").cloned().unwrap_or(J::Null)), ("server", p.get("server_side").cloned().unwrap_or(J::Null)), ("downloads", p.get("downloads").cloned().unwrap_or(J::Null)), ("updated", p.get("updated").cloned().unwrap_or(J::Null)), ("url", s(&format!("https://modrinth.com/mod/{}", so(&p, "slug").unwrap_or(id.clone()))))]))
}

fn browse_versions(app: &App, req: &Req) -> Result<J, String> {
    let source = { let t = arg(req, "source"); if t.is_empty() { "modrinth".into() } else { t } };
    let pid = arg(req, "id"); if pid.is_empty() { return Err("Missing mod id.".into()); }
    let (loader, mc, label, _) = envinfo::env_cached(&app.dir);
    let only = arg(req, "compat") != "0" && (loader.is_some() || mc.is_some());
    let inst = installed_index(app);
    let mut out = vec![];
    if source == "curseforge" {
        let mut p: Vec<(&str, String)> = vec![("pageSize", "40".into())];
        if only { if let Some(m) = &mc { p.push(("gameVersion", m.clone())); } if let Some(id) = loader.as_deref().and_then(cf_loader_id) { p.push(("modLoaderType", id.to_string())); } }
        let files = av(&cf_json("GET", &format!("/mods/{}/files?{}", enc(&pid), qs(&p)), None)?, "data");
        let mut dep_ids: Vec<i64> = files.iter().flat_map(|f| av(f, "dependencies")).filter(|d| iv(d, "relationType") == 3).map(|d| iv(&d, "modId")).collect(); dep_ids.sort(); dep_ids.dedup();
        let mut names: HashMap<String, String> = HashMap::new();
        for ch in dep_ids.chunks(50) { for m in av(&cf_json("POST", "/mods", Some(&obj(vec![("modIds", J::Arr(ch.iter().map(|x| n(*x)).collect()))])))?, "data") { names.insert(sv(&m, "id"), sv(&m, "name")); } }
        for f in &files {
            let gvs = sa(f, "gameVersions");
            let ldrs: Vec<J> = LOADER_NAMES.iter().filter(|l| cf_loader_tag(l).map(|t| gvs.iter().any(|g| g == t)).unwrap_or(false)).map(|l| s(l)).collect();
            let fnm = sv(f, "fileName");
            let ver = clean_version(&parse_filename(&fnm, mc.as_deref()).1.unwrap_or_default(), mc.as_deref(), mc.as_deref()).unwrap_or_else(|| fnm.clone());
            out.push(obj(vec![("id", s(&sv(f, "id"))), ("name", s(&so(f, "displayName").filter(|x| !x.is_empty()).unwrap_or_else(|| fnm.clone()))), ("version", s(&ver)), ("type", s(cf_type(f))), ("mc", J::Arr(mc_versions(&gvs).iter().take(8).map(|x| s(x)).collect())),
                ("loaders", J::Arr(ldrs)), ("date", f.get("fileDate").cloned().unwrap_or(J::Null)), ("downloads", f.get("downloadCount").cloned().unwrap_or(J::Null)), ("size", f.get("fileLength").cloned().unwrap_or(J::Null)), ("filename", s(&fnm)),
                ("can_download", J::Bool(!sv(f, "downloadUrl").is_empty())), ("compatible", J::Bool(cf_files_ok(f, &mc, &loader))),
                ("deps", J::Arr(av(f, "dependencies").iter().filter(|d| iv(d, "relationType") == 3).map(|d| { let id = sv(d, "modId"); obj(vec![("id", s(&id)), ("name", s(&names.get(&id).cloned().filter(|x| !x.is_empty()).unwrap_or_else(|| format!("Mod {id}")))), ("installed", J::Bool(inst.contains_key(&("curseforge".to_string(), id.clone()))))]) }).collect()))]));
        }
    } else {
        let mut p: Vec<(&str, String)> = vec![];
        if only { if let Some(l) = &loader { p.push(("loaders", jarr_str(&loaders_browse(l)))); } if let Some(m) = &mc { p.push(("game_versions", jarr_str(&[m.clone()]))); } }
        let vs = match mr("GET", &format!("/project/{}/version{}", enc(&pid), if p.is_empty() { String::new() } else { format!("?{}", qs(&p)) }), None)? { J::Arr(a) => a, _ => vec![] };
        let mut dep_ids: Vec<String> = vs.iter().take(40).flat_map(|v| av(v, "dependencies")).filter(|d| sv(d, "dependency_type") == "required").filter_map(|d| so(&d, "project_id")).collect(); dep_ids.sort(); dep_ids.dedup();
        let mut names: HashMap<String, String> = HashMap::new();
        for ch in dep_ids.chunks(80) { if let J::Arr(a) = mr("GET", &format!("/projects?ids={}", enc(&jarr_str(ch))), None)? { for p in a { names.insert(sv(&p, "id"), so(&p, "title").or_else(|| so(&p, "slug")).unwrap_or_default()); } } }
        for v in vs.iter().take(40) {
            let files = av(v, "files"); let pf = files.iter().find(|f| matches!(f.get("primary"), Some(J::Bool(true)))).or(files.first()).cloned().unwrap_or(J::Null);
            let vn = sv(v, "version_number");
            out.push(obj(vec![("id", v.get("id").cloned().unwrap_or(J::Null)), ("name", v.get("name").cloned().unwrap_or(J::Null)), ("version", s(&clean_version(&vn, mc.as_deref(), mc.as_deref()).unwrap_or(vn))), ("type", s(&{ let t = sv(v, "version_type"); if t.is_empty() { "release".into() } else { t } })),
                ("mc", J::Arr(mc_versions(&sa(v, "game_versions")).iter().take(8).map(|x| s(x)).collect())), ("loaders", J::Arr(sa(v, "loaders").iter().filter(|x| LOADER_NAMES.contains(&x.as_str())).map(|x| s(x)).collect())), ("date", v.get("date_published").cloned().unwrap_or(J::Null)), ("downloads", v.get("downloads").cloned().unwrap_or(J::Null)),
                ("size", pf.get("size").cloned().unwrap_or(J::Null)), ("filename", pf.get("filename").cloned().unwrap_or(J::Null)), ("can_download", J::Bool(!sv(&pf, "url").is_empty())), ("compatible", J::Bool(compat_mr(v, &loader, &mc))),
                ("deps", J::Arr(av(v, "dependencies").iter().filter(|d| sv(d, "dependency_type") == "required" && so(d, "project_id").is_some()).map(|d| { let id = sv(d, "project_id"); obj(vec![("id", s(&id)), ("name", s(&names.get(&id).cloned().filter(|x| !x.is_empty()).unwrap_or_else(|| id.clone()))), ("installed", J::Bool(inst.contains_key(&("modrinth".to_string(), id.clone()))))]) }).collect()))]));
        }
    }
    Ok(obj(vec![("versions", J::Arr(out)), ("env", obj(vec![("loader", jo(loader)), ("mc", jo(mc)), ("label", jo(label)), ("filtered", J::Bool(only))]))]))
}

// ---- install from the browser
pub struct Ins { phase: &'static str, msg: String, done: i64, total: i64, result: J }
pub static INS: Mutex<Ins> = Mutex::new(Ins { phase: "idle", msg: String::new(), done: 0, total: 0, result: J::Null });
#[derive(Clone)]
pub struct Item { pub source: String, pub project_id: String, pub version_id: String, pub name: String, pub version: String, pub filename: String, pub url: String, pub sha1: Option<String>, pub sha512: Option<String>, pub dep: bool, pub client_only: bool, pub icon: Option<String>, pub slug: Option<String>, pub page: Option<String>, pub deps: Vec<J> }
fn mr_item(v: &J, proj: &J, dep: bool, mc: &Option<String>) -> Result<Item, String> {
    let files: Vec<J> = av(v, "files").into_iter().filter(|f| sv(f, "filename").to_lowercase().ends_with(".jar")).collect();
    let pf = files.iter().find(|f| matches!(f.get("primary"), Some(J::Bool(true)))).or(files.first()).cloned().ok_or_else(|| format!("{} has no downloadable .jar in that version.", so(proj, "title").or_else(|| so(v, "name")).unwrap_or_else(|| "That mod".into())))?;
    let hs = pf.get("hashes").cloned().unwrap_or(J::Null); let slug = so(proj, "slug");
    Ok(Item { source: "modrinth".into(), project_id: sv(v, "project_id"), version_id: sv(v, "id"), name: so(proj, "title").or_else(|| so(v, "name")).unwrap_or_else(|| sv(v, "project_id")), version: clean_version(&sv(v, "version_number"), mc.as_deref(), mc.as_deref()).unwrap_or_default(),
        filename: sv(&pf, "filename"), url: sv(&pf, "url"), sha1: so(&hs, "sha1"), sha512: so(&hs, "sha512"), dep, client_only: sv(proj, "server_side") == "unsupported", icon: so(proj, "icon_url"), slug: slug.clone(), page: slug.map(|x| format!("https://modrinth.com/mod/{x}")), deps: av(v, "dependencies") })
}
fn cf_item(f: &J, m: &J, dep: bool, mc: &Option<String>) -> Item {
    let sha1 = av(f, "hashes").iter().find(|h| iv(h, "algo") == 1).and_then(|h| so(h, "value")); let fnm = sv(f, "fileName");
    Item { source: "curseforge".into(), project_id: so(f, "modId").or_else(|| so(m, "id")).unwrap_or_default(), version_id: sv(f, "id"), name: so(m, "name").or_else(|| so(f, "displayName")).unwrap_or_else(|| fnm.clone()), version: clean_version(&parse_filename(&fnm, mc.as_deref()).1.unwrap_or_default(), mc.as_deref(), mc.as_deref()).unwrap_or_default(),
        filename: fnm, url: sv(f, "downloadUrl"), sha1, sha512: None, dep, client_only: false, icon: m.get("logo").and_then(|l| so(l, "thumbnailUrl")), slug: so(m, "slug"), page: m.get("links").and_then(|l| so(l, "websiteUrl")), deps: av(f, "dependencies") }
}
fn best<'a>(vs: &'a [J], date: &str, ty: &dyn Fn(&J) -> String) -> Option<&'a J> {
    if vs.is_empty() { return None; }
    let pick = |t: &str| -> Vec<&J> { vs.iter().filter(|v| ty(v) == t).collect() };
    let mut c = pick("release"); if c.is_empty() { c = pick("beta"); } if c.is_empty() { c = vs.iter().collect(); }
    c.into_iter().max_by_key(|v| sv(v, date))
}
pub fn plan_modrinth(app: &App, vid: &str, with_deps: bool, loader: &Option<String>, mc: &Option<String>, force: bool, warnings: &mut Vec<String>, label: &Option<String>) -> Result<Vec<Item>, String> {
    let v = mr("GET", &format!("/version/{}", enc(vid)), None)?; let proj = mr("GET", &format!("/project/{}", enc(&sv(&v, "project_id"))), None)?;
    if !force && (loader.is_some() || mc.is_some()) && !compat_mr(&v, loader, mc) { return Err(format!("That file isn't built for {}. Pick a version marked compatible.", label.clone().unwrap_or_else(|| "this server".into()))); }
    let first = mr_item(&v, &proj, false, mc)?;
    let mut seen: HashSet<String> = [sv(&v, "project_id")].into(); let mut plan = vec![first.clone()];
    if with_deps {
        let have = installed_index(app); let mut queue: Vec<J> = first.deps.clone();
        while !queue.is_empty() && plan.len() < 15 {
            let d = queue.remove(0);
            if sv(&d, "dependency_type") != "required" { continue; }
            let mut dpid = so(&d, "project_id"); let mut dv: Option<J> = None;
            let r = (|| -> Result<Option<(J, J)>, String> {
                if let Some(vi) = so(&d, "version_id") { let x = mr("GET", &format!("/version/{}", enc(&vi)), None)?; if dpid.is_none() { dpid = so(&x, "project_id"); } dv = Some(x); }
                let Some(p) = dpid.clone() else { return Ok(None) };
                if seen.contains(&p) || have.contains_key(&("modrinth".to_string(), p.clone())) { return Ok(None); }
                seen.insert(p.clone());
                if dv.as_ref().map(|x| !compat_mr(x, loader, mc)).unwrap_or(true) {
                    let mut q: Vec<(&str, String)> = vec![]; if let Some(l) = loader { q.push(("loaders", jarr_str(&loaders_browse(l)))); } if let Some(m) = mc { q.push(("game_versions", jarr_str(&[m.clone()]))); }
                    let vs = match mr("GET", &format!("/project/{}/version?{}", enc(&p), qs(&q)), None)? { J::Arr(a) => a, _ => vec![] };
                    dv = best(&vs, "date_published", &|x| sv(x, "version_type")).cloned();
                }
                let dproj = mr("GET", &format!("/project/{}", enc(&p)), None)?;
                Ok(Some((dv.clone().unwrap_or(J::Null), dproj)))
            })();
            match r {
                Err(_) => warnings.push(format!("Couldn't look up a required mod ({}). Install it by hand.", dpid.unwrap_or_else(|| "unknown".into()))),
                Ok(None) => {}
                Ok(Some((dvv, dproj))) => {
                    if matches!(dvv, J::Null) { warnings.push(format!("{} is required but has no version for this server. Install it by hand.", so(&dproj, "title").or(dpid).unwrap_or_default())); continue; }
                    match mr_item(&dvv, &dproj, true, mc) { Ok(it) => { queue.extend(it.deps.clone()); plan.push(it); } Err(e) => warnings.push(e) }
                }
            }
        }
    }
    Ok(plan)
}
pub fn plan_curseforge(app: &App, mod_id: &str, file_id: &str, with_deps: bool, loader: &Option<String>, mc: &Option<String>, force: bool, warnings: &mut Vec<String>, label: &Option<String>) -> Result<Vec<Item>, String> {
    let f = cf_json("GET", &format!("/mods/{}/files/{}", enc(mod_id), enc(file_id)), None)?.get("data").cloned().unwrap_or(J::Null);
    let m = cf_json("GET", &format!("/mods/{}", enc(mod_id)), None)?.get("data").cloned().unwrap_or(J::Null);
    if !force && (loader.is_some() || mc.is_some()) && !cf_files_ok(&f, mc, loader) { return Err(format!("That file isn't built for {}. Pick a version marked compatible.", label.clone().unwrap_or_else(|| "this server".into()))); }
    let first = cf_item(&f, &m, false, mc); let mut seen: HashSet<String> = [mod_id.to_string()].into(); let mut plan = vec![first.clone()];
    if with_deps {
        let have = installed_index(app);
        let mut queue: Vec<String> = first.deps.iter().filter(|d| iv(d, "relationType") == 3).map(|d| sv(d, "modId")).collect();
        while !queue.is_empty() && plan.len() < 15 {
            let dm = queue.remove(0);
            if seen.contains(&dm) || have.contains_key(&("curseforge".to_string(), dm.clone())) { continue; }
            seen.insert(dm.clone());
            let mut q: Vec<(&str, String)> = vec![("pageSize", "15".into())]; if let Some(x) = mc { q.push(("gameVersion", x.clone())); } if let Some(id) = loader.as_deref().and_then(cf_loader_id) { q.push(("modLoaderType", id.to_string())); }
            let r = (|| -> Result<(Vec<J>, J), String> { Ok((av(&cf_json("GET", &format!("/mods/{}/files?{}", enc(&dm), qs(&q)), None)?, "data"), cf_json("GET", &format!("/mods/{}", enc(&dm)), None)?.get("data").cloned().unwrap_or(J::Null))) })();
            let Ok((fl, dmod)) = r else { warnings.push(format!("Couldn't look up a required mod ({dm}). Install it by hand.")); continue };
            let Some(b) = best(&fl, "fileDate", &|x| cf_type(x).to_string()) else { warnings.push(format!("{} is required but has no version for this server. Install it by hand.", so(&dmod, "name").unwrap_or(dm))); continue };
            let it = cf_item(b, &dmod, true, mc);
            queue.extend(it.deps.iter().filter(|d| iv(d, "relationType") == 3).map(|d| sv(d, "modId"))); plan.push(it);
        }
    }
    Ok(plan)
}
pub fn install_one(app: &App, it: &Item) -> Result<J, String> {
    let stg = app.dir.join(".panel-staging"); let _ = fs::create_dir_all(&stg);
    let label = if it.source == "curseforge" { "CurseForge" } else { "Modrinth" };
    if it.url.is_empty() { return Err(format!("{} only lets people download this one from its own website. Open the page, download the jar, then drop it on this page.", it.name)); }
    let name = safe_name(&it.filename);
    if !name.to_lowercase().ends_with(".jar") || name.starts_with('.') { return Err(format!("Odd file name from {label}, skipped.")); }
    let tmp = stg.join(format!("dl-{}.jar", jobs::new_id()));
    let res = (|| -> Result<J, String> {
        dl::download(&it.url, &tmp, DL_HOSTS, true, it.sha1.as_deref(), it.sha512.as_deref(), 400)?;
        let info = mods::parse_jar(&tmp, &name, None); let new_sha = file_hash(&tmp);
        let _g = mods::MODS_LOCK.lock().unwrap();
        let existing: Vec<J> = mods::list_mods(app).into_iter().filter(|m| { let f = sv(m, "file"); let same_file = f == name || f == format!("{name}.disabled"); let same_proj = !it.project_id.is_empty() && sv(m, "project_id") == it.project_id && sv(m, "source") == it.source; same_file || same_proj || (info.reliable && sv(m, "modid") == info.modid) }).collect();
        for m in &existing { if file_hash(&mods_dir(app).join(sv(m, "file"))) == new_sha { return Ok(obj(vec![("status", s("already")), ("project_id", s(&it.project_id)), ("source", s(&it.source)), ("name", s(&it.name)), ("version", s(&it.version)), ("file", s(&sv(m, "file"))), ("dep", J::Bool(it.dep))])); } }
        let to_trash: Vec<String> = existing.iter().map(|m| sv(m, "file")).collect();
        let client_only = it.client_only || info.client_only;
        let mut dest = name.clone(); if client_only || to_trash.iter().any(|f| f.to_lowercase().ends_with(".disabled")) { dest.push_str(".disabled"); }
        let clash = mods_dir(app).join(&dest);
        if clash.exists() && !to_trash.contains(&dest) { return Err(format!("{dest} already exists, skipped.")); }
        let _ = fs::create_dir_all(mods_dir(app));
        if !to_trash.is_empty() { mods::trash_files(app, &to_trash); }
        fs::rename(&tmp, &clash).map_err(|e| e.to_string())?;
        let (sz, mt) = file_sig(&clash).unwrap_or((0, 0));
        let og = obj(vec![("source", s(&it.source)), ("project_id", s(&it.project_id)), ("slug", jo(it.slug.clone())), ("title", s(&it.name)), ("icon", jo(it.icon.clone())), ("url", jo(it.page.clone())), ("version_number", s(&it.version)), ("size", n(sz as i64))]);
        save_origin(app, &dest, og.clone()); meta_put(&dest, og, sz, mt);
        Ok(obj(vec![("status", s(if to_trash.is_empty() { "added" } else { "replaced" })), ("project_id", s(&it.project_id)), ("source", s(&it.source)), ("name", s(&it.name)), ("version", s(&it.version)), ("file", s(&dest)), ("dep", J::Bool(it.dep)), ("client_only", J::Bool(client_only)), ("replaced", J::Arr(to_trash.iter().map(|x| s(x)).collect()))]))
    })();
    let _ = fs::remove_file(&tmp);
    res
}
fn install_worker(app: Arc<App>, source: String, pid: String, vid: String, with_deps: bool, force: bool, loader: Option<String>, mc: Option<String>, label: Option<String>) {
    let (mut results, mut warnings) = (vec![], vec![]);
    let plan = if source == "curseforge" { plan_curseforge(&app, &pid, &vid, with_deps, &loader, &mc, force, &mut warnings, &label) } else { plan_modrinth(&app, &vid, with_deps, &loader, &mc, force, &mut warnings, &label) };
    match plan {
        Err(e) => { let mut g = INS.lock().unwrap(); g.phase = "done"; g.msg.clear(); g.result = obj(vec![("items", J::Arr(results)), ("warnings", J::Arr(warnings.iter().map(|x| s(x)).collect())), ("error", s(&e))]); }
        Ok(plan) => {
            INS.lock().unwrap().total = plan.len() as i64;
            for (i, it) in plan.iter().enumerate() {
                { let mut g = INS.lock().unwrap(); g.msg = format!("Downloading {} ({} of {})...", it.name, i + 1, plan.len()); g.done = i as i64; }
                results.push(match install_one(&app, it) { Ok(r) => r, Err(e) => obj(vec![("status", s("failed")), ("name", s(&it.name)), ("error", s(&e)), ("dep", J::Bool(it.dep)), ("page", jo(it.page.clone()))]) });
            }
            let okn: Vec<String> = results.iter().filter(|r| matches!(sv(r, "status").as_str(), "added" | "replaced")).map(|r| format!("{} {}", sv(r, "name"), sv(r, "version")).trim().to_string()).collect();
            if !okn.is_empty() { crate::misc::mods_touched(); app.mc.log(&format!("[panel] Mod browser: installed {}.", okn.join(", "))); }
            let mut g = INS.lock().unwrap(); g.phase = "done"; g.done = plan.len() as i64; g.msg.clear(); g.result = obj(vec![("items", J::Arr(results)), ("warnings", J::Arr(warnings.iter().map(|x| s(x)).collect()))]);
        }
    }
}
fn cf_categories() -> Result<Vec<J>, String> {
    let j = cf_json("GET", &format!("/categories?{}", qs(&[("gameId", CF_GAME.to_string()), ("classId", "6".into())])), None)?;
    let mut v: Vec<(String, String)> = av(&j, "data").iter().filter(|c| iv(c, "parentCategoryId") == 6 && !matches!(c.get("isClass"), Some(J::Bool(true)))).map(|c| (sv(c, "name"), sv(c, "id"))).collect();
    v.sort(); Ok(v.into_iter().map(|(nm, id)| obj(vec![("id", s(&id)), ("name", s(&nm))])).collect())
}


fn best_version_for(source: &str, pid: &str, loader: &Option<String>, mc: &Option<String>) -> Result<Option<String>, String> {
    if source == "curseforge" {
        let mut q: Vec<(&str, String)> = vec![("pageSize", "40".into())];
        if let Some(m) = mc { q.push(("gameVersion", m.clone())); }
        if let Some(id) = loader.as_deref().and_then(cf_loader_id) { q.push(("modLoaderType", id.to_string())); }
        let files: Vec<J> = av(&cf_json("GET", &format!("/mods/{}/files?{}", enc(pid), qs(&q)), None)?, "data").into_iter().filter(|f| !sv(f, "downloadUrl").is_empty()).collect();
        let rel: Vec<&J> = { let r: Vec<&J> = files.iter().filter(|f| iv(f, "releaseType") == 1).collect(); if r.is_empty() { files.iter().collect() } else { r } };
        return Ok(rel.iter().max_by_key(|f| iv(f, "id")).map(|f| iv(f, "id").to_string()));
    }
    let mut q: Vec<(&str, String)> = vec![];
    if let Some(l) = loader { q.push(("loaders", jarr_str(&loaders_browse(l)))); }
    if let Some(m) = mc { q.push(("game_versions", jarr_str(&[m.clone()]))); }
    let vs = match mr("GET", &format!("/project/{}/version{}", enc(pid), if q.is_empty() { String::new() } else { format!("?{}", qs(&q)) }), None)? { J::Arr(a) => a, _ => vec![] };
    Ok(best(&vs, "date_published", &|x| sv(x, "version_type")).map(|v| sv(v, "id")))
}
fn multi_worker(app: Arc<App>, items: Vec<(String, String)>, with_deps: bool, loader: Option<String>, mc: Option<String>, label: Option<String>) {
    let (mut results, mut warnings): (Vec<J>, Vec<String>) = (vec![], vec![]);
    { let mut g = INS.lock().unwrap(); g.msg = "Finding the right version of each mod...".into(); g.total = items.len() as i64; g.done = 0; }
    let (mut plan, mut seen): (Vec<Item>, HashSet<(String, String)>) = (vec![], HashSet::new());
    for (src, pid) in &items {
        match best_version_for(src, pid, &loader, &mc) {
            Err(e) => warnings.push(e),
            Ok(None) => warnings.push(format!("One mod ({pid}) has no version for this server, skipped.")),
            Ok(Some(vid)) => {
                let mut ws = vec![];
                let p = if src == "curseforge" { plan_curseforge(&app, pid, &vid, with_deps, &loader, &mc, false, &mut ws, &label) } else { plan_modrinth(&app, &vid, with_deps, &loader, &mc, false, &mut ws, &label) };
                warnings.extend(ws);
                match p { Ok(p) => for it in p { if seen.insert((it.source.clone(), it.project_id.clone())) { plan.push(it); } }, Err(e) => warnings.push(e) }
            }
        }
    }
    INS.lock().unwrap().total = plan.len() as i64;
    for (i, it) in plan.iter().enumerate() {
        { let mut g = INS.lock().unwrap(); g.msg = format!("Downloading {} ({} of {})...", it.name, i + 1, plan.len()); g.done = i as i64; }
        results.push(match install_one(&app, it) { Ok(r) => r, Err(e) => obj(vec![("status", s("failed")), ("name", s(&it.name)), ("error", s(&e)), ("dep", J::Bool(it.dep)), ("page", jo(it.page.clone()))]) });
    }
    let okn: Vec<String> = results.iter().filter(|r| matches!(sv(r, "status").as_str(), "added" | "replaced")).map(|r| format!("{} {}", sv(r, "name"), sv(r, "version")).trim().to_string()).collect();
    if !okn.is_empty() { crate::misc::mods_touched(); app.mc.log(&format!("[panel] Mod browser: installed {}.", okn.join(", "))); }
    let mut g = INS.lock().unwrap(); g.phase = "done"; g.done = plan.len() as i64; g.msg.clear();
    g.result = obj(vec![("items", J::Arr(results)), ("warnings", J::Arr(warnings.iter().map(|x| s(x)).collect()))]);
}
pub fn multi_install(app: &Arc<App>, b: &J) -> Resp {
    let mut items = vec![];
    for x in av(b, "items").iter().take(60) { let (src, id) = (sv(x, "source"), sv(x, "id").trim().to_string()); if (src == "modrinth" || src == "curseforge") && !id.is_empty() { items.push((src, id)); } }
    if items.is_empty() { return reply(false, "Tick at least one mod first."); }
    if items.iter().any(|(s_, _)| s_ == "curseforge") && cf_key().is_none() { return reply(false, "Add your CurseForge API key first."); }
    let (loader, mc, label, _) = envinfo::env_cached(&app.dir);
    { let mut g = INS.lock().unwrap(); if g.phase == "running" { return reply(false, "Another install is still running."); } *g = Ins { phase: "running", msg: "Getting ready...".into(), done: 0, total: items.len() as i64, result: J::Null }; }
    let wd = !matches!(b.get("with_deps"), Some(J::Bool(false)));
    let a = app.clone(); thread::spawn(move || multi_worker(a, items, wd, loader, mc, label));
    reply(true, "Installing...")
}

pub fn handle(app: &Arc<App>, req: &Req) -> Option<Resp> {
    let body = || json::parse(&String::from_utf8_lossy(&req.body)).unwrap_or(J::Null);
    let fail = |e: String| Resp::json(502, obj(vec![("ok", J::Bool(false)), ("error", s(&e)), ("msg", s(&e))]).to_string());
    Some(match (req.method.as_str(), req.path.as_str()) {
        ("GET", "/api/browse/config") => { let (l, m, lb, _) = envinfo::env_cached(&app.dir); Resp::json(200, obj(vec![("loader", jo(l)), ("mc", jo(m)), ("label", jo(lb)), ("cf", J::Bool(cf_key().is_some())), ("cf_env", J::Bool(std::env::var("CURSEFORGE_API_KEY").map(|x| !x.trim().is_empty()).unwrap_or(false))), ("mr_cats", J::Arr(MR_CATS.iter().map(|x| s(x)).collect()))]).to_string()) }
        ("GET", "/api/img") => {
            let u = req.q("u").unwrap_or("");
            match dl::get_bytes(u, &["cdn.modrinth.com", "media.forgecdn.net", "edge.forgecdn.net", "cdn.curseforge.com", "avatars.githubusercontent.com"], 15) {
                Ok(b) if b.len() > 16 && b.len() < 4_000_000 => {
                    let ct = if b.starts_with(&[0x89, b'P', b'N', b'G']) { "image/png" } else if b.starts_with(&[0xff, 0xd8]) { "image/jpeg" } else if b.starts_with(b"GIF8") { "image/gif" } else if b.len() > 12 && &b[8..12] == b"WEBP" { "image/webp" } else { "application/octet-stream" };
                    Resp { code: 200, ctype: ct, body: b }
                }
                _ => Resp::empty(404),
            }
        }
        ("GET", "/api/browse/search") => match browse_search(app, req) { Ok(j) => Resp::json(200, j.to_string()), Err(e) => fail(e) },
        ("GET", "/api/browse/detail") => match browse_detail(req) { Ok(j) => Resp::json(200, j.to_string()), Err(e) => fail(e) },
        ("GET", "/api/browse/versions") => match browse_versions(app, req) { Ok(j) => Resp::json(200, j.to_string()), Err(e) => fail(e) },
        ("GET", "/api/browse/categories") => {
            let cats = if req.q("source") == Some("curseforge") { match cf_categories() { Ok(c) => c, Err(e) => return Some(fail(e)) } } else { MR_CATS.iter().map(|c| { let t: String = c.split('-').map(|w| { let mut ch = w.chars(); ch.next().map(|f| f.to_uppercase().collect::<String>() + ch.as_str()).unwrap_or_default() }).collect::<Vec<_>>().join(" "); obj(vec![("id", s(c)), ("name", s(&t))]) }).collect() };
            Resp::json(200, obj(vec![("categories", J::Arr(cats))]).to_string())
        }
        ("GET", "/api/browse/install/status") => { let g = INS.lock().unwrap(); Resp::json(200, obj(vec![("phase", s(g.phase)), ("msg", s(&g.msg)), ("done", n(g.done)), ("total", n(g.total)), ("result", g.result.clone())]).to_string()) }
        ("POST", "/api/browse/install") => {
            let b = body(); let source = sv(&b, "source"); let (pid, vid) = (sv(&b, "project_id"), sv(&b, "version_id"));
            if source != "modrinth" && source != "curseforge" { return Some(reply(false, "Unknown source.")); }
            if source == "curseforge" && cf_key().is_none() { return Some(reply(false, "Add your CurseForge API key first.")); }
            if pid.is_empty() || vid.is_empty() { return Some(reply(false, "Pick a version first.")); }
            let (loader, mc, label, _) = envinfo::env_cached(&app.dir);
            { let mut g = INS.lock().unwrap(); if g.phase == "running" { return Some(reply(false, "Another install is still running.")); } *g = Ins { phase: "running", msg: "Getting ready...".into(), done: 0, total: 1, result: J::Null }; }
            let (wd, force) = (matches!(b.get("with_deps"), Some(J::Bool(true))), matches!(b.get("force"), Some(J::Bool(true))));
            let a = app.clone(); thread::spawn(move || install_worker(a, source, pid, vid, wd, force, loader, mc, label));
            reply(true, "Installing...")
        }
        ("POST", "/api/mods/multi-install") => multi_install(app, &body()),
        ("POST", "/api/cf/key") => { let (ok, m) = cf_set_key(&sv(&body(), "key")); reply(ok, &m) }
        ("GET", "/api/mods/update/status") | ("GET", "/api/mods/updates") => Resp::json(200, update_public(app).to_string()),
        ("POST", "/api/mods/update/check") => { let (ok, m) = start_check(app); reply(ok, &m) }
        ("POST", "/api/mods/update/apply") => {
            let files: HashSet<String> = av(&body(), "files").iter().filter_map(|x| x.as_str().map(|t| t.to_string())).collect();
            let (ok, m) = start_apply(app, files); reply(ok, &m)
        }
        _ => return None,
    })
}
pub fn start(app: &Arc<App>) {
    let a = app.clone();
    thread::spawn(move || { thread::sleep(Duration::from_secs(10)); loop { auto_loop_tick(&a); if crate::modupd::wait_event(1800) { thread::sleep(Duration::from_secs(4)); } } });
}

#[cfg(test)]
mod plain_tests {
    use super::plain;
    #[test]
    fn strips_markdown_and_html() {
        let t = "# Title\n\n![img](http://x/y.png)\nSome **bold** [link](http://a.b) text.\n<p>Hello <b>world</b></p>\n- one\n- two\n\n\n\nEnd";
        let o = plain(t, 500);
        assert!(!o.contains("http"), "{o}");
        assert!(o.contains("Some bold link text."), "{o}");
        assert!(o.contains("Hello world"), "{o}");
        assert!(o.contains("\u{2022} one"), "{o}");
        assert!(!o.contains("\n\n\n"), "{o}");
        assert!(plain(&"word ".repeat(1000), 100).ends_with("..."));
    }
}
