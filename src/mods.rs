//! Installed mods: jar metadata, listing, upload staging, toggle/delete/flags (port of the Python panel's mod code).
use crate::envinfo::{detect_env, env_cached, load_settings, vkey};
use crate::http::{Req, Resp};
use crate::json::{self, n, obj, s, J};
use crate::rx::Regex;
use crate::{sys, App};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

const MC_TOKEN: &str = r"1\.(?:[6-9]|1\d|2[0-2])(?:\.\d{1,2})?";
const LOADER_RX: &str = r"neo[-_ ]?forge|forge|fabric|quilt";

struct Rxs { mc_full: Regex, mc_mark: Regex, loader_word: Regex, mc_near: Regex, ver_part: Regex, tag_part: Regex, jar_ext: Regex, mc_paren: Regex, cv1: Regex, cv2: Regex, mfr: Regex, up_sig: Regex, lead_mc: Regex }
fn rxs() -> &'static Rxs {
    static R: OnceLock<Rxs> = OnceLock::new();
    R.get_or_init(|| Rxs {
        mc_full: Regex::new(&format!("^(?:{})$", MC_TOKEN)),
        mc_mark: Regex::new(&format!(r"(?i)(?<![A-Za-z0-9])mc[-_ ]?({})(?!\d)", MC_TOKEN)),
        loader_word: Regex::new(&format!(r"(?i)(?<![A-Za-z])(?:{})(?![A-Za-z])", LOADER_RX)),
        mc_near: Regex::new(&format!(r"(?i)(?<![\d.])({m})(?!\d)[-_+ ]*(?<![A-Za-z])(?:{l})(?![A-Za-z])|(?<![A-Za-z])(?:{l})(?![A-Za-z])[-_+ ]*(?<![\d.])({m})(?!\d)", m = MC_TOKEN, l = LOADER_RX)),
        ver_part: Regex::new(r"^[vV]?\d+(?:\.\d+)+(?:\.?[A-Za-z][A-Za-z0-9]*)?$"),
        tag_part: Regex::new(r"(?i)^(?:alpha|beta|rc|pre|snapshot|dev|fix|hotfix|build|b|a|r)\d*(?:\.\d+)*$"),
        jar_ext: Regex::new(r"(?i)\.jar(\.disabled)?$"),
        mc_paren: Regex::new(&format!(r"[\(\[]\s*(?:{})\s*[\)\]]", MC_TOKEN)),
        cv1: Regex::new(&format!(r"^[vV]?({})[-_+ ]+(.*\d.*)$", MC_TOKEN)),
        cv2: Regex::new(&format!(r"^(.*\d.*?)[-_+ ]+({})$", MC_TOKEN)),
        mfr: Regex::new(r"(?m)^Implementation-Version:\s*(.+?)\s*$"),
        up_sig: Regex::new(r">=|~|\^|,\s*\)|\.x|\.\*"),
        lead_mc: Regex::new(&format!(r"^[vV]?({})[-_+ ]+\d+(?:\.\d+)+", MC_TOKEN)),
    })
}

fn chars(s_: &str) -> Vec<char> { s_.chars().collect() }
fn sub(c: &[char], a: usize, b: usize) -> String { c[a..b].iter().collect() }
fn ws_collapse(t: &str) -> String { t.split_whitespace().collect::<Vec<_>>().join(" ") }
fn py_title(t: &str) -> String {
    let mut out = String::new(); let mut prev = false;
    for ch in t.chars() { if ch.is_alphabetic() { if prev { out.extend(ch.to_lowercase()); } else { out.extend(ch.to_uppercase()); } prev = true; } else { out.push(ch); prev = false; } }
    out
}
pub fn pretty_name(t: &str) -> String {
    let t = t.replace('_', " "); let t = t.trim().to_string();
    let t = Regex::new(r"(?<=[a-z])(?=[A-Z])").replace_all(&t, &|_| " ".into());
    let t = ws_collapse(&t);
    if !t.is_empty() && t == t.to_lowercase() { py_title(&t) } else { t }
}
fn lstrip_v(t: &str) -> &str { t.trim_start_matches(|c| c == 'v' || c == 'V') }
fn is_digits(t: &str) -> bool { !t.is_empty() && t.chars().all(|c| c.is_ascii_digit()) }
fn split_sep(t: &str) -> Vec<String> { t.split(|c| "-_+ ".contains(c)).filter(|p| !p.is_empty()).map(|p| p.to_string()).collect() }

/// 'create-1.21.1-6.0.8.jar' -> (name, version, mc)
pub fn parse_filename(fname: &str, mc_known: Option<&str>) -> (String, Option<String>, Option<String>) {
    let r = rxs();
    let stem0 = r.jar_ext.replace_all(fname, &|_| String::new());
    let mut stem = stem0.clone();
    let mut mc: Option<String> = None;
    if let Some(m) = r.mc_mark.find(&stem) { mc = m.get(1); let c = chars(&stem); stem = format!("{} {}", sub(&c, 0, m.start()), sub(&c, m.end(), c.len())); }
    if mc.is_none() { if let Some(m) = r.mc_near.find(&stem) { mc = m.get(1).or_else(|| m.get(2)); } }
    let s2 = r.loader_word.replace_all(&stem, &|_| " ".into());
    let parts = split_sep(&s2);
    let ver_idx: Vec<usize> = (0..parts.len()).filter(|&i| r.ver_part.is_match(&parts[i])).collect();
    let mut mc_idx: Option<usize> = None;
    if mc.is_none() {
        let mcs: Vec<usize> = ver_idx.iter().copied().filter(|&i| r.mc_full.is_match(lstrip_v(&parts[i]))).collect();
        if let Some(k) = mc_known { for &i in &mcs { if lstrip_v(&parts[i]) == k { mc = Some(lstrip_v(&parts[i]).into()); mc_idx = Some(i); break; } } }
        if mc.is_none() && !mcs.is_empty() && !ver_idx.is_empty() && mcs[0] == ver_idx[0] && ver_idx.len() >= 2 { mc = Some(lstrip_v(&parts[mcs[0]]).into()); mc_idx = Some(mcs[0]); }
    }
    if mc.is_some() && mc_idx.is_none() { for &i in &ver_idx { if Some(lstrip_v(&parts[i]).to_string()) == mc { mc_idx = Some(i); break; } } }
    let mut rest: Vec<usize> = ver_idx.iter().copied().filter(|&i| Some(i) != mc_idx).collect();
    let mut ver: Option<String> = None;
    if mc.is_none() && ver_idx.len() == 1 && r.mc_full.is_match(lstrip_v(&parts[ver_idx[0]])) && ver_idx[0] + 1 < parts.len() && is_digits(&parts[ver_idx[0] + 1]) {
        mc = Some(lstrip_v(&parts[ver_idx[0]]).into()); ver = Some(parts[ver_idx[0] + 1].clone()); rest.clear();
    }
    if let Some(&i) = rest.first() {
        let mut v = lstrip_v(&parts[i]).to_string();
        let mut j = i + 1;
        while j < parts.len() && (r.tag_part.is_match(&parts[j]) || (is_digits(&parts[j]) && r.tag_part.is_match(&parts[j - 1]))) { v.push('-'); v.push_str(&parts[j]); j += 1; }
        ver = Some(v);
    }
    let cut = ver_idx.first().copied().unwrap_or(parts.len());
    let name = pretty_name(&parts[..cut].join(" "));
    let name = if name.is_empty() { pretty_name(&stem) } else { name };
    (name, ver, mc)
}

pub fn clean_version(v: &str, mc: Option<&str>, mc_hint: Option<&str>) -> Option<String> {
    let r = rxs();
    if v.is_empty() { return None; }
    let raw = v.trim().to_string();
    let mut t = r.mc_mark.replace_all(&raw, &|_| " ".into());
    t = r.loader_word.replace_all(&t, &|_| " ".into());
    t = r.mc_paren.replace_all(&t, &|_| " ".into());
    let mcs: Vec<&str> = [mc, mc_hint].iter().filter_map(|x| *x).collect();
    if let Some(m) = r.cv1.find(t.trim()) {
        let (g1, g2) = (m.get(1).unwrap_or_default(), m.get(2).unwrap_or_default());
        if mcs.contains(&g1.as_str()) || Regex::new(r"^[vV]?\d+(?:\.\d+)+").is_match(&g2) { t = g2; }
    }
    if let Some(m) = r.cv2.find(t.trim()) { if mcs.contains(&m.get(2).unwrap_or_default().as_str()) { t = m.get(1).unwrap_or_default(); } }
    let t = ws_collapse(&t);
    let t = t.trim_matches(|c| " -_+.".contains(c)).to_string();
    let t = if t.len() > 1 && (t.starts_with('v') || t.starts_with('V')) && t.chars().nth(1).map(|c| c.is_ascii_digit()).unwrap_or(false) { t[1..].to_string() } else { t };
    Some(if t.is_empty() { raw } else { t })
}

fn rng_text(v: &J) -> String { match v { J::Str(x) => x.clone(), J::Arr(a) => a.iter().map(|x| match x { J::Str(t) => t.clone(), o => o.to_string() }).collect::<Vec<_>>().join(" "), _ => String::new() } }
pub fn mc_in_range(rng: &str, mc_hint: Option<&str>) -> Option<String> {
    if rng.is_empty() { return None; }
    let vs: Vec<String> = Regex::new(MC_TOKEN).find_all(rng).iter().map(|m| m.whole()).collect();
    if vs.is_empty() { return None; }
    if let Some(h) = mc_hint {
        let k = vkey(h); let lo = vkey(&vs[0]);
        if vs.len() == 1 {
            if vs[0] == h || h.starts_with(&format!("{}.", vs[0])) || (k >= lo && rxs().up_sig.is_match(rng)) { return Some(h.into()); }
        } else {
            let hi = vkey(&vs[1]);
            let (a, b) = (&k[..k.len().min(hi.len())], &hi[..hi.len().min(k.len())]);
            if (lo <= k && k < hi) || (lo <= k && a == b && rng.contains(']')) { return Some(h.into()); }
        }
    }
    Some(vs[0].clone())
}
pub fn mc_from_version_string(raw: &str) -> Option<String> {
    if raw.is_empty() { return None; }
    let r = rxs();
    if let Some(m) = r.mc_mark.find(raw) { return m.get(1); }
    r.lead_mc.find(raw.trim()).and_then(|m| if m.start() == 0 { m.get(1) } else { None })
}
fn guess_id(fname: &str) -> String {
    let stem = rxs().jar_ext.replace_all(fname, &|_| String::new());
    let c = chars(&stem);
    let mut cut = c.len();
    for i in 0..c.len().saturating_sub(1) { if "-_+ ".contains(c[i]) && (c[i + 1].is_ascii_digit() || (c[i + 1] == 'v' && i + 2 < c.len() && c[i + 2].is_ascii_digit())) { cut = i; break; } }
    let base = sub(&c, 0, cut);
    (if base.is_empty() { stem } else { base }).to_lowercase()
}

// ---- toml helpers
fn toml_blocks(txt: &str) -> Vec<(String, String)> {
    let hdr = Regex::new(r"(?m)^\s*\[\[dependencies\.([^\]\s]+)\]\]\s*$");
    let next = Regex::new(r"(?m)^\s*\[\[");
    let c = chars(txt);
    let mut out = vec![];
    for m in hdr.find_all(txt) {
        let owner = m.get(1).unwrap_or_default().trim_matches(|c| c == '"' || c == '\'').to_string();
        let rest: String = c[m.end()..].iter().collect();
        let block = match next.find(&rest) { Some(nm) => rest.chars().take(nm.start()).collect(), None => rest };
        out.push((owner, block));
    }
    out
}
fn toml_val(block: &str, key: &str) -> Option<String> {
    let m = Regex::new(&format!(r#"(?m)^\s*{}\s*=\s*(?:"([^"]*)"|'([^']*)')"#, key)).find(block)?;
    match m.get(1) { Some(a) if !a.is_empty() => Some(a), _ => m.get(2).filter(|x| !x.is_empty()) }
}
fn toml_block(txt: &str) -> String {
    let m = match Regex::new(r"(?m)^\s*\[\[mods\]\]\s*$").find(txt) { Some(m) => m, None => return txt.into() };
    let c = chars(txt); let rest: String = c[m.end()..].iter().collect();
    match Regex::new(r"(?m)^\s*\[\[").find(&rest) { Some(nm) => rest.chars().take(nm.start()).collect(), None => rest }
}
fn toml_mc_range(txt: &str, modid: Option<&str>) -> Option<String> {
    for (owner, block) in toml_blocks(txt) {
        if let Some(id) = modid { if owner != id { continue; } }
        if toml_val(&block, "modId").as_deref() == Some("minecraft") { return toml_val(&block, "versionRange"); }
    }
    None
}
fn client_only_toml(txt: &str, modid: Option<&str>) -> bool {
    for (owner, block) in toml_blocks(txt) {
        if let Some(id) = modid { if owner != id { continue; } }
        if matches!(toml_val(&block, "modId").as_deref(), Some("minecraft") | Some("neoforge") | Some("forge")) && toml_val(&block, "side").unwrap_or_default().to_uppercase() == "CLIENT" { return true; }
    }
    false
}

const KNOWN_CLIENT_ONLY: &[&str] = &["sodium", "rubidium", "embeddium", "oculus", "iris", "irisshaders", "optifine", "modmenu", "dynamic_fps", "dynamicfps", "entityculling", "immediatelyfast", "sodiumextra", "sodium_extra", "reeses-sodium-options", "reeses_sodium_options", "continuity", "lambdynlights", "betterf3", "borderlessmining", "sodiumdynamiclights", "sodium-dynamic-lights", "embeddiumextras", "rubidiumextra", "xaerominimap", "xaeroworldmap", "notenoughanimations", "not_enough_animations", "skinlayers3d", "enhancedblockentities", "moreculling", "cullleaves", "fancymenu", "drippyloadingscreen", "konkrete", "legendarytooltips", "controlling", "searchables", "configured_client", "mousetweaks", "inventoryhud", "shaderslist", "visuality", "soundphysics", "sound_physics_remastered"];

#[derive(Clone, Debug, Default)]
pub struct Info { pub modid: String, pub name: String, pub version: Option<String>, pub version_raw: Option<String>, pub mc: Option<String>, pub mc_src: Option<&'static str>, pub reliable: bool, pub client_only: bool }

fn jstr(j: &J, k: &str) -> Option<String> { match j.get(k) { Some(J::Str(x)) => Some(x.clone()), _ => None } }

pub fn parse_jar(path: &Path, fname: &str, mc_hint: Option<&str>) -> Info {
    let (mut modid, mut name, mut version): (Option<String>, Option<String>, Option<String>) = (None, None, None);
    let (mut client_only, mut rng): (bool, Option<String>) = (false, None);
    'read: {
        for tn in ["META-INF/neoforge.mods.toml", "META-INF/mods.toml"] {
            if modid.is_some() { break; }
            if let Some(b) = sys::zip_read(path, tn) {
                let txt = String::from_utf8_lossy(&b).to_string();
                let block = toml_block(&txt);
                modid = toml_val(&block, "modId"); name = toml_val(&block, "displayName"); version = toml_val(&block, "version");
                rng = toml_mc_range(&txt, modid.as_deref());
                if client_only_toml(&txt, modid.as_deref()) { client_only = true; }
            }
        }
        if modid.is_none() {
            if let Some(b) = sys::zip_read(path, "fabric.mod.json") {
                let d = match json::parse(&String::from_utf8_lossy(&b)) { Some(d) => d, None => break 'read };
                modid = jstr(&d, "id"); name = jstr(&d, "name"); version = jstr(&d, "version");
                if let Some(dep @ J::Obj(_)) = d.get("depends") { if let Some(v) = dep.get("minecraft") { rng = Some(rng_text(v)); } }
                if let Some(J::Str(e)) = d.get("environment") { if e.to_lowercase() == "client" { client_only = true; } }
            }
        }
        if modid.is_none() {
            if let Some(b) = sys::zip_read(path, "quilt.mod.json") {
                let d = match json::parse(&String::from_utf8_lossy(&b)) { Some(d) => d, None => break 'read };
                if let Some(q) = d.get("quilt_loader") {
                    modid = jstr(q, "id"); version = jstr(q, "version");
                    name = q.get("metadata").and_then(|m| jstr(m, "name"));
                    if let Some(J::Arr(a)) = q.get("depends") { for dp in a { if jstr(dp, "id").as_deref() == Some("minecraft") { rng = dp.get("versions").map(rng_text); } } }
                }
            }
        }
        let bad = |v: &Option<String>| v.as_ref().map(|x| x.is_empty() || x.contains("${")).unwrap_or(true);
        if bad(&version) {
            if let Some(b) = sys::zip_read(path, "META-INF/MANIFEST.MF") {
                version = rxs().mfr.find(&String::from_utf8_lossy(&b)).and_then(|m| m.get(1));
            } else { version = None; }
        }
        if version.as_ref().map(|v| v.contains("${")).unwrap_or(false) { version = None; }
    }
    let reliable = modid.is_some();
    let (fn_name, fn_ver, fn_mc) = parse_filename(fname, mc_hint);
    let raw = version.clone().filter(|v| !v.is_empty()).or(fn_ver);
    let mut mc = fn_mc.or_else(|| version.as_deref().and_then(mc_from_version_string));
    let mut src = if mc.is_some() { Some("file") } else { None };
    if mc.is_none() { mc = rng.as_deref().and_then(|r| mc_in_range(r, mc_hint)); src = if mc.is_some() { Some("meta") } else { None }; }
    let modid = modid.unwrap_or_else(|| guess_id(fname));
    let name = match name { Some(n_) if !n_.is_empty() && !n_.contains("${") => n_, _ => if !fn_name.is_empty() { fn_name } else { pretty_name(&modid) } };
    let version = raw.as_deref().and_then(|r| clean_version(r, mc.as_deref(), mc_hint));
    if KNOWN_CLIENT_ONLY.contains(&modid.to_lowercase().as_str()) { client_only = true; }
    Info { modid, name, version, version_raw: raw, mc, mc_src: src, reliable, client_only }
}

type CacheKey = (PathBuf, u128, u64, String);
fn cache() -> &'static Mutex<HashMap<CacheKey, Info>> { static C: OnceLock<Mutex<HashMap<CacheKey, Info>>> = OnceLock::new(); C.get_or_init(|| Mutex::new(HashMap::new())) }
fn mtime_ns(m: &fs::Metadata) -> u128 { m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_nanos()).unwrap_or(0) }
pub fn parse_jar_cached(dir: &Path, path: &Path, fname: &str) -> Info {
    let mc = env_cached(dir).1;
    let md = match fs::metadata(path) { Ok(m) => m, Err(_) => return parse_jar(path, fname, mc.as_deref()) };
    let key = (path.to_path_buf(), mtime_ns(&md), md.len(), mc.clone().unwrap_or_default());
    if let Some(i) = cache().lock().unwrap().get(&key) { return i.clone(); }
    let info = parse_jar(path, fname, mc.as_deref());
    let mut c = cache().lock().unwrap();
    if c.len() > 3000 { c.clear(); }
    c.insert(key, info.clone());
    info
}

// ---- state shared by the routes
fn mods_dir(app: &App) -> PathBuf { app.dir.join("mods") }
fn flags_file(app: &App) -> PathBuf { app.dir.join(".panel-mods.json") }
fn load_flags(app: &App) -> Vec<String> {
    match fs::read_to_string(flags_file(app)).ok().and_then(|t| json::parse(&t)) { Some(d) => match d.get("server_only") { Some(J::Arr(a)) => a.iter().filter_map(|x| match x { J::Str(t) => Some(t.clone()), other => Some(other.to_string()) }).collect(), _ => vec![] }, None => vec![] }
}
fn save_flags(app: &App, v: &[String]) { let _ = fs::write(flags_file(app), obj(vec![("server_only", J::Arr(v.iter().map(|x| s(x)).collect()))]).to_string()); }
fn origins(app: &App) -> Vec<(String, J)> { match fs::read_to_string(app.dir.join(".panel-origins.json")).ok().and_then(|t| json::parse(&t)) { Some(J::Obj(m)) => m, _ => vec![] } }
fn site_url(meta: &J) -> J {
    let src = jstr(meta, "source");
    match src.as_deref() {
        Some("modrinth") => match jstr(meta, "slug") { Some(sl) if !sl.is_empty() => s(&format!("https://modrinth.com/mod/{}", sl)), _ => J::Null },
        Some("curseforge") => match jstr(meta, "url").filter(|u| !u.is_empty()) { Some(u) => s(&u), None => match jstr(meta, "slug").filter(|u| !u.is_empty()) { Some(sl) => s(&format!("https://www.curseforge.com/minecraft/mc-mods/{}", sl)), None => J::Null } },
        _ => J::Null,
    }
}
fn strip_disabled(f: &str) -> String { if f.to_lowercase().ends_with(".disabled") { f[..f.len() - 9].to_string() } else { f.to_string() } }

pub fn list_mods(app: &App) -> Vec<J> { crate::am::decorate(app, list_mods_raw(app)) }
pub fn list_mods_raw(app: &App) -> Vec<J> {
    let md = mods_dir(app);
    if !md.is_dir() { return vec![]; }
    let flags: HashSet<String> = load_flags(app).into_iter().collect();
    let og = origins(app);
    let updates_locked = crate::envinfo::load_settings(&app.dir).updates_lock;
    let mut names: Vec<String> = fs::read_dir(&md).map(|r| r.flatten().map(|e| e.file_name().to_string_lossy().to_string()).collect()).unwrap_or_default();
    names.sort_by_key(|x| x.to_lowercase());
    let jars: Vec<(String, PathBuf, fs::Metadata)> = names.into_iter().filter(|f| { let l = f.to_lowercase(); l.ends_with(".jar") || l.ends_with(".jar.disabled") }).filter_map(|f| { let p = md.join(&f); fs::metadata(&p).ok().map(|m| (f, p, m)) }).collect();
    // parse uncached jars in parallel (each parse spawns unzip a few times)
    let infos: Vec<Info> = {
        let slots: Mutex<Vec<Option<Info>>> = Mutex::new(vec![None; jars.len()]);
        let next = Mutex::new(0usize);
        std::thread::scope(|sc| {
            for _ in 0..8.min(jars.len().max(1)) {
                sc.spawn(|| loop {
                    let i = { let mut g = next.lock().unwrap(); let i = *g; *g += 1; i };
                    if i >= jars.len() { break; }
                    let info = parse_jar_cached(&app.dir, &jars[i].1, &jars[i].0);
                    slots.lock().unwrap()[i] = Some(info);
                });
            }
        });
        slots.into_inner().unwrap().into_iter().map(|x| x.unwrap_or_default()).collect()
    };
    jars.iter().zip(infos.iter()).map(|((f, _p, st), info)| {
        let live = crate::browse::meta_for(f, st.len(), mtime_ns(st) / 1_000_000_000);
        let meta = live.as_ref().or_else(|| og.iter().find(|(k, _)| *k == strip_disabled(f)).map(|(_, v)| v).filter(|v| matches!(v.get("size"), Some(J::Num(x)) if *x as u64 == st.len())));
        let g = |k: &str| meta.and_then(|m| m.get(k)).cloned().unwrap_or(J::Null);
        let opt = |x: &Option<String>| x.as_ref().map(|v| s(v)).unwrap_or(J::Null);
        obj(vec![("file", s(f)), ("name", s(&info.name)), ("modid", s(&info.modid)), ("version", opt(&info.version)), ("version_raw", opt(&info.version_raw)), ("mc", opt(&info.mc)), ("mc_src", info.mc_src.map(s).unwrap_or(J::Null)),
            ("reliable", J::Bool(info.reliable)), ("size", n(st.len() as i64)), ("mtime", n((mtime_ns(st) / 1_000_000_000) as i64)), ("enabled", J::Bool(f.to_lowercase().ends_with(".jar"))),
            ("server_only", J::Bool(flags.contains(&info.modid))), ("client_only", J::Bool(info.client_only)), ("icon", g("icon")), ("source", g("source")), ("project_id", g("project_id")),
            ("url", meta.map(site_url).unwrap_or(J::Null)), ("update", if updates_locked { J::Null } else { crate::browse::update_for(f) })])
    }).collect()
}

pub fn safe_mod_name(app: &App, name: &str) -> Option<String> {
    if name.contains('/') || name.contains('\\') || name.starts_with('.') { return None; }
    let l = name.to_lowercase();
    if !(l.ends_with(".jar") || l.ends_with(".jar.disabled")) { return None; }
    if !mods_dir(app).join(name).is_file() { return None; }
    Some(name.to_string())
}
pub fn trash_files(app: &App, names: &[String]) -> Vec<String> {
    let dest = app.dir.join("mods-trash").join(sys::stamp());
    let _ = fs::create_dir_all(&dest);
    let mut moved = vec![];
    for nm in names {
        let src = mods_dir(app).join(nm);
        if !src.is_file() { continue; }
        let mut target = dest.join(nm); let mut i = 1;
        while target.exists() { target = dest.join(format!("{}-{}", i, nm)); i += 1; }
        if fs::rename(&src, &target).or_else(|_| fs::copy(&src, &target).and_then(|_| fs::remove_file(&src))).is_ok() { moved.push(nm.clone()); }
    }
    moved
}

struct Staged { path: PathBuf, name: String, info: Info, time: f64 }
fn staged() -> &'static Mutex<HashMap<String, Staged>> { static S: OnceLock<Mutex<HashMap<String, Staged>>> = OnceLock::new(); S.get_or_init(|| Mutex::new(HashMap::new())) }
pub static MODS_LOCK: Mutex<()> = Mutex::new(());
fn rid() -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u64((sys::now() * 1e6) as u64);
    format!("{:012x}", h.finish() & 0xffff_ffff_ffff)
}
fn clean_staging(app: &App, max_age: f64) {
    let now = sys::now();
    let mut g = staged().lock().unwrap();
    let old: Vec<String> = g.iter().filter(|(_, v)| now - v.time > max_age).map(|(k, _)| k.clone()).collect();
    for k in old { if let Some(v) = g.remove(&k) { let _ = fs::remove_file(v.path); } }
    let keep: HashSet<String> = g.values().filter_map(|v| v.path.file_name().map(|x| x.to_string_lossy().to_string())).collect();
    if let Ok(rd) = fs::read_dir(app.dir.join(".panel-staging")) {
        for e in rd.flatten() {
            let fnm = e.file_name().to_string_lossy().to_string();
            if !keep.contains(&fnm) && e.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).map(|d| d.as_secs_f64() > max_age).unwrap_or(false) { let _ = fs::remove_file(e.path()); }
        }
    }
}
fn vcmp(a: Option<&str>, b: Option<&str>) -> Option<i32> {
    let (ka, kb) = (vkey(a.unwrap_or("")), vkey(b.unwrap_or("")));
    if ka.is_empty() || kb.is_empty() { return None; }
    Some(match ka.cmp(&kb) { std::cmp::Ordering::Less => -1, std::cmp::Ordering::Equal => 0, _ => 1 })
}
fn conflicts_for(app: &App, modid: &str, filename: &str) -> Vec<J> {
    let dis = format!("{}.disabled", filename);
    list_mods(app).into_iter().filter(|m| { let f = jstr(m, "file").unwrap_or_default(); f == filename || f == dis || jstr(m, "modid").as_deref() == Some(modid) }).collect()
}

fn err(code: u16, m: &str) -> Resp { Resp::json(code, obj(vec![("ok", J::Bool(false)), ("error", s(m)), ("msg", s(m))]).to_string()) }
fn reply(ok: bool, msg: &str, extra: Vec<(&str, J)>) -> Resp { let mut v = vec![("ok", J::Bool(ok)), ("msg", s(msg))]; v.extend(extra); Resp::json(200, obj(v).to_string()) }

fn stage_upload(app: &App, req: &Req) -> Resp {
    let raw = req.header("x-filename").map(|v| crate::http::url_decode(&v)).unwrap_or_default();
    let base = raw.rsplit(['/', '\\']).next().unwrap_or("").to_string();
    let name: String = base.chars().map(|c| if c.is_alphanumeric() || "_.+-[]() ".contains(c) { c } else { '_' }).collect::<String>().trim().to_string();
    if !name.to_lowercase().ends_with(".jar") || name.starts_with('.') { return err(400, "Only .jar files can be added."); }
    if req.body.is_empty() { return err(400, "Empty file."); }
    let stg = app.dir.join(".panel-staging");
    let _ = fs::create_dir_all(&stg);
    clean_staging(app, 3600.0);
    let sid = rid();
    let path = stg.join(format!("{}.jar", sid));
    if let Err(e) = fs::write(&path, &req.body) { return err(500, &format!("Couldn't save the upload: {}", e)); }
    if !req.body.starts_with(b"PK") { let _ = fs::remove_file(&path); return err(400, "That doesn't look like a valid .jar file."); }
    let mc = env_cached(&app.dir).1;
    let info = parse_jar(&path, &name, mc.as_deref());
    let _g = MODS_LOCK.lock().unwrap();
    let conflicts: Vec<J> = conflicts_for(app, &info.modid, &name).iter().map(|m| {
        let f = jstr(m, "file").unwrap_or_default(); let v = jstr(m, "version");
        obj(vec![("file", s(&f)), ("name", m.get("name").cloned().unwrap_or(J::Null)), ("version", v.as_deref().map(s).unwrap_or(J::Null)), ("enabled", m.get("enabled").cloned().unwrap_or(J::Null)),
            ("same_file", J::Bool(f == name || f == format!("{}.disabled", name))), ("older", J::Bool(vcmp(info.version.as_deref(), v.as_deref()) == Some(-1))), ("same_version", J::Bool(vcmp(info.version.as_deref(), v.as_deref()) == Some(0)))])
    }).collect();
    let out = obj(vec![("ok", J::Bool(true)), ("id", s(&sid)), ("name", s(&name)), ("modname", s(&info.name)), ("modid", s(&info.modid)), ("version", info.version.as_deref().map(s).unwrap_or(J::Null)), ("size", n(req.body.len() as i64)), ("conflicts", J::Arr(conflicts))]);
    staged().lock().unwrap().insert(sid, Staged { path, name, info, time: sys::now() });
    Resp::json(200, out.to_string())
}

fn commit(app: &App, items: &[J]) -> J {
    let (mut added, mut replaced, mut skipped, mut errors, mut client_only): (Vec<J>, Vec<J>, Vec<J>, Vec<J>, Vec<J>) = Default::default();
    let _ = fs::create_dir_all(mods_dir(app));
    let _g = MODS_LOCK.lock().unwrap();
    for it in items {
        let sid = jstr(it, "id").unwrap_or_default();
        let action = jstr(it, "action").unwrap_or_default();
        let st = staged().lock().unwrap().remove(&sid);
        let st = match st { Some(x) => x, None => { errors.push(s("An upload expired, please try it again.")); continue; } };
        if action != "add" && action != "replace" { let _ = fs::remove_file(&st.path); skipped.push(s(&st.name)); continue; }
        let existing = conflicts_for(app, &st.info.modid, &st.name);
        if !existing.is_empty() && action == "add" { let _ = fs::remove_file(&st.path); skipped.push(s(&format!("{} (would duplicate {})", st.name, jstr(&existing[0], "file").unwrap_or_default()))); continue; }
        let mut extras: Vec<String> = vec![];
        if action == "replace" { if let Some(J::Arr(a)) = it.get("replace_files") { for f in a { if let Some(nm) = f.as_str().and_then(|f| safe_mod_name(app, f)) { extras.push(nm); } } } }
        let mut to_trash: Vec<String> = vec![];
        for f in existing.iter().filter_map(|m| jstr(m, "file")).chain(extras.into_iter()) { if !to_trash.contains(&f) { to_trash.push(f); } }
        let mut dest = st.name.clone();
        if matches!(it.get("keep_disabled"), Some(J::Bool(true))) && to_trash.iter().any(|f| f.to_lowercase().ends_with(".disabled")) { dest += ".disabled"; }
        if st.info.client_only && !dest.to_lowercase().ends_with(".disabled") { dest += ".disabled"; }
        if st.info.client_only { client_only.push(s(&st.info.name)); }
        if !to_trash.is_empty() { trash_files(app, &to_trash); }
        let target = mods_dir(app).join(&dest);
        match fs::rename(&st.path, &target).or_else(|_| fs::copy(&st.path, &target).and_then(|_| fs::remove_file(&st.path))) {
            Ok(()) => { if !to_trash.is_empty() { replaced.push(obj(vec![("new", s(&dest)), ("old", J::Arr(to_trash.iter().map(|x| s(x)).collect()))])); } else { added.push(s(&dest)); } }
            Err(e) => errors.push(s(&format!("{}: {}", st.name, e))),
        }
    }
    obj(vec![("added", J::Arr(added)), ("replaced", J::Arr(replaced)), ("skipped", J::Arr(skipped)), ("errors", J::Arr(errors)), ("client_only", J::Arr(client_only))])
}

pub fn handle(app: &Arc<App>, req: &Req) -> Option<Resp> {
    let body = || json::parse(&String::from_utf8_lossy(&req.body)).unwrap_or(J::Null);
    match (req.method.as_str(), req.path.as_str()) {
        ("GET", "/api/mods") => {
            let st = load_settings(&app.dir);
            let label = detect_env(&app.dir, &st, false).2;
            Some(Resp::json(200, obj(vec![("mods", J::Arr(list_mods(app))), ("loader", label.as_deref().map(s).unwrap_or(J::Null)), ("dir_exists", J::Bool(mods_dir(app).is_dir())), ("updates", n(0)), ("checked_at", J::Null)]).to_string()))
        }
        ("POST", "/api/mods/stage") => Some(stage_upload(app, req)),
        ("POST", "/api/mods/commit") => {
            let b = body();
            let items: Vec<J> = match b.get("items") { Some(J::Arr(a)) => a.iter().filter(|x| matches!(x, J::Obj(_))).cloned().collect(), _ => vec![] };
            let res = commit(app, &items);
            crate::misc::mods_touched();
            let (a, r) = (res.get("added").map(|x| matches!(x, J::Arr(v) if !v.is_empty())).unwrap_or(false), res.get("replaced").map(|x| matches!(x, J::Arr(v) if !v.is_empty())).unwrap_or(false));
            let cnt = |k: &str| match res.get(k) { Some(J::Arr(v)) => v.len(), _ => 0 };
            let mut parts = vec![]; if a { parts.push(format!("added {}", cnt("added"))); } if r { parts.push(format!("replaced {}", cnt("replaced"))); }
            if !parts.is_empty() { app.mc.log(&format!("[panel] Mods: {}.", parts.join(", "))); }
            Some(reply(true, "Done.", vec![("result", res)]))
        }
        ("POST", p @ ("/api/mods/delete" | "/api/mods/toggle" | "/api/mods/flag")) => {
            let b = body();
            let names: Vec<String> = match b.get("files") { Some(J::Arr(a)) => a.iter().filter_map(|f| f.as_str().and_then(|f| safe_mod_name(app, f))).collect(), _ => vec![] };
            if names.is_empty() { return Some(reply(false, "No matching mods.", vec![])); }
            let _g = MODS_LOCK.lock().unwrap();
            crate::misc::mods_touched();
            if p == "/api/mods/delete" {
                let moved = trash_files(app, &names);
                if !moved.is_empty() { app.mc.log(&format!("[panel] Mods removed (moved to mods-trash): {}", moved.join(", "))); }
                return Some(reply(true, &format!("Removed {}.", moved.len()), vec![("removed", J::Arr(moved.iter().map(|x| s(x)).collect()))]));
            }
            if p == "/api/mods/toggle" {
                let enable = matches!(b.get("enabled"), Some(J::Bool(true))); let force = matches!(b.get("force"), Some(J::Bool(true)));
                let (mut changed, mut skipped_client) = (0, 0);
                for nm in &names {
                    let low = nm.to_lowercase(); let src = mods_dir(app).join(nm);
                    if enable && !force && parse_jar_cached(&app.dir, &src, nm).client_only { skipped_client += 1; continue; }
                    let dst = if enable && low.ends_with(".jar.disabled") { mods_dir(app).join(&nm[..nm.len() - 9]) } else if !enable && low.ends_with(".jar") { mods_dir(app).join(format!("{}.disabled", nm)) } else { continue };
                    if dst.exists() { continue; }
                    if fs::rename(&src, &dst).is_ok() { changed += 1; }
                }
                let mut msg = format!("{} {}.", if enable { "Enabled" } else { "Disabled" }, changed);
                if skipped_client > 0 { msg += &format!(" {} client-only mod{} left off (they would crash the server).", skipped_client, if skipped_client == 1 { "" } else { "s" }); }
                return Some(reply(true, &msg, vec![]));
            }
            let want = matches!(b.get("server_only"), Some(J::Bool(true)));
            let mut cur: Vec<String> = load_flags(app);
            for nm in &names { let mid = parse_jar_cached(&app.dir, &mods_dir(app).join(nm), nm).modid; if want { if !cur.contains(&mid) { cur.push(mid); } } else { cur.retain(|x| *x != mid); } }
            cur.sort(); save_flags(app, &cur);
            Some(reply(true, "Updated.", vec![]))
        }
        _ => None,
    }
}
pub fn start(_app: &Arc<App>) {}
