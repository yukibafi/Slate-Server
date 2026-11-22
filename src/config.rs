//! Settings page backend: server.properties + user_jvm_args.txt (ported from the Python panel).
use crate::json::{self, n, obj, s, J};
use std::fs;
use std::path::Path;
use std::process::Command;

const SPEC: &str = include_str!("props.json");
const FLAGS: &[&str] = &[
    "-XX:+UseG1GC", "-XX:+ParallelRefProcEnabled", "-XX:MaxGCPauseMillis=200",
    "-XX:+UnlockExperimentalVMOptions", "-XX:+DisableExplicitGC", "-XX:G1NewSizePercent=30",
    "-XX:G1MaxNewSizePercent=40", "-XX:G1HeapRegionSize=8M", "-XX:G1ReservePercent=20",
    "-XX:G1HeapWastePercent=5", "-XX:G1MixedGCCountTarget=4", "-XX:InitiatingHeapOccupancyPercent=15",
    "-XX:G1MixedGCLiveThresholdPercent=90", "-XX:G1RSetUpdatingPauseTimePercent=5",
    "-XX:SurvivorRatio=32", "-XX:+PerfDisableSharedMem", "-XX:MaxTenuringThreshold=1",
];

fn groups() -> Vec<J> { match json::parse(SPEC) { Some(J::Arr(a)) => a, _ => vec![] } }
fn gs<'a>(j: &'a J, k: &str) -> &'a str { j.get(k).and_then(|v| v.as_str()).unwrap_or("") }
fn fields(g: &J) -> Vec<J> { match g.get("fields") { Some(J::Arr(a)) => a.clone(), _ => vec![] } }
fn num(j: &J, k: &str) -> Option<f64> { if let Some(J::Num(x)) = j.get(k) { Some(*x) } else { None } }

fn unesc(v: &str) -> String {
    let c: Vec<char> = v.chars().collect();
    let mut o = String::new();
    let mut i = 0;
    while i < c.len() {
        if c[i] == '\\' && i + 1 < c.len() {
            if c[i + 1] == 'u' && i + 5 < c.len() + 0 && i + 6 <= c.len() {
                let h: String = c[i + 2..i + 6].iter().collect();
                if let Ok(cp) = u32::from_str_radix(&h, 16) {
                    if (0xD800..0xDC00).contains(&cp) && i + 12 <= c.len() && c[i + 6] == '\\' && c[i + 7] == 'u' {
                        let h2: String = c[i + 8..i + 12].iter().collect();
                        if let Ok(lo) = u32::from_str_radix(&h2, 16) {
                            if let Some(ch) = char::from_u32(0x10000 + ((cp - 0xD800) << 10) + (lo.wrapping_sub(0xDC00) & 0x3FF)) { o.push(ch); i += 12; continue; }
                        }
                    }
                    if let Some(ch) = char::from_u32(cp) { o.push(ch); i += 6; continue; }
                }
            }
            match c[i + 1] { 'n' => o.push('\n'), 't' => o.push('\t'), x => o.push(x) }
            i += 2;
        } else { o.push(c[i]); i += 1; }
    }
    o
}
fn esc(v: &str) -> String {
    let mut o = String::new();
    for ch in v.chars() {
        let cp = ch as u32;
        if ch == '\\' { o.push_str("\\\\"); }
        else if ":=#!".contains(ch) { o.push('\\'); o.push(ch); }
        else if cp > 126 {
            if cp > 0xFFFF { let x = cp - 0x10000; o.push_str(&format!("\\u{:04x}\\u{:04x}", 0xD800 + (x >> 10), 0xDC00 + (x & 0x3FF))); }
            else { o.push_str(&format!("\\u{:04x}", cp)); }
        } else { o.push(ch); }
    }
    if o.starts_with(' ') { o.insert(0, '\\'); }
    o
}

pub fn read_props(dir: &Path) -> Vec<(String, String)> {
    let mut out = vec![];
    if let Ok(t) = fs::read_to_string(dir.join("server.properties")) {
        for l in t.lines() {
            let l = l.trim();
            if l.is_empty() || l.starts_with('#') || l.starts_with('!') { continue; }
            if let Some((k, v)) = l.split_once('=') { out.push((k.trim().to_string(), unesc(v.trim()))); }
        }
    }
    out
}

fn write_props(dir: &Path, updates: &[(String, String)]) -> std::io::Result<()> {
    let path = dir.join("server.properties");
    let mut lines: Vec<String> = match fs::read_to_string(&path) {
        Ok(t) => { let _ = fs::copy(&path, dir.join("server.properties.panel-bak")); t.lines().map(|x| x.to_string()).collect() }
        Err(_) => vec!["#Minecraft server properties".to_string()],
    };
    let mut done: Vec<String> = vec![];
    for line in lines.iter_mut() {
        let st = line.trim().to_string();
        if st.is_empty() || st.starts_with('#') || st.starts_with('!') { continue; }
        if let Some((k, v)) = st.split_once('=') {
            let k = k.trim();
            if let Some((_, nv)) = updates.iter().find(|(a, _)| a == k) {
                if !done.iter().any(|d| d == k) {
                    if unesc(v.trim()) != *nv { *line = format!("{}={}", k, esc(nv)); }
                    done.push(k.to_string());
                }
            }
        }
    }
    for (k, v) in updates { if !done.iter().any(|d| d == k) { lines.push(format!("{}={}", k, esc(v))); } }
    let tmp = dir.join("server.properties.panel-tmp");
    fs::write(&tmp, lines.join("\n") + "\n")?;
    fs::rename(tmp, path)
}

pub fn sys_ram_mb() -> Option<i64> {
    if cfg!(target_os = "macos") {
        let o = Command::new("sysctl").args(["-n", "hw.memsize"]).output().ok()?;
        return String::from_utf8_lossy(&o.stdout).trim().parse::<i64>().ok().map(|b| b / 1048576);
    }
    let t = fs::read_to_string("/proc/meminfo").ok()?;
    let l = t.lines().find(|l| l.starts_with("MemTotal:"))?;
    l.split_whitespace().nth(1)?.parse::<i64>().ok().map(|k| k / 1024)
}
fn sysctl(k: &str) -> J {
    if !cfg!(target_os = "macos") { return J::Null; }
    Command::new("sysctl").args(["-n", k]).output().ok().map(|o| s(String::from_utf8_lossy(&o.stdout).trim())).unwrap_or(J::Null)
}
/// Memory that is free right now (Mac: free + inactive + speculative + purgeable pages; Linux: MemAvailable).
pub fn avail_ram_mb() -> Option<i64> {
    if cfg!(target_os = "macos") {
        let o = Command::new("vm_stat").output().ok()?; let t = String::from_utf8_lossy(&o.stdout).to_string();
        let ps: i64 = t.split("page size of ").nth(1).and_then(|x| x.split_whitespace().next()).and_then(|x| x.parse().ok()).unwrap_or(4096);
        let mut pages = 0i64;
        for label in ["Pages free", "Pages inactive", "Pages speculative", "Pages purgeable"] {
            if let Some(l) = t.lines().find(|l| l.starts_with(label)) { pages += l.split(':').nth(1).map(|v| v.trim().trim_end_matches('.')).and_then(|v| v.parse::<i64>().ok()).unwrap_or(0); }
        }
        return Some(pages * ps / 1048576);
    }
    let t = fs::read_to_string("/proc/meminfo").ok()?;
    t.lines().find(|l| l.starts_with("MemAvailable:"))?.split_whitespace().nth(1)?.parse::<i64>().ok().map(|k| k / 1024)
}
pub fn mac_info() -> J {
    let chip = sysctl("machdep.cpu.brand_string");
    let apple = matches!(&chip, J::Str(c) if c.starts_with("Apple"));
    let cores = std::thread::available_parallelism().map(|c| c.get() as i64).unwrap_or(1);
    obj(vec![("total_mb", sys_ram_mb().map(n).unwrap_or(J::Null)), ("chip", chip), ("model", sysctl("hw.model")), ("cores", n(cores)), ("avail_mb", avail_ram_mb().map(n).unwrap_or(J::Null)), ("apple", J::Bool(apple))])
}

fn to_mb(num: i64, unit: &str) -> i64 { if unit.eq_ignore_ascii_case("g") { num * 1024 } else { num } }
fn jvm(dir: &Path) -> (Option<i64>, Option<i64>, Vec<String>, bool) {
    let (mut xms, mut xmx, mut extra) = (None, None, vec![]);
    if let Ok(t) = fs::read_to_string(dir.join("user_jvm_args.txt")) {
        for l in t.lines() {
            let l = l.trim();
            if l.is_empty() || l.starts_with('#') { continue; }
            for tok in l.split_whitespace() {
                let parsed = tok.strip_prefix("-Xm").and_then(|r| {
                    let which = r.chars().next()?;
                    let rest = &r[1..];
                    let unit = rest.chars().last()?;
                    let digits = &rest[..rest.len() - 1];
                    if !"GgMm".contains(unit) || digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit()) { return None; }
                    Some((which, to_mb(digits.parse().ok()?, &unit.to_string())))
                });
                match parsed { Some(('s', mb)) => xms = Some(mb), Some(('x', mb)) => xmx = Some(mb), _ => extra.push(tok.to_string()) }
            }
        }
    }
    let used = fs::read_to_string(dir.join("start.sh")).map(|t| t.contains("user_jvm_args")).unwrap_or(false);
    (xms, xmx, extra, used)
}

fn count_mods(dir: &Path) -> (i64, i64, Vec<String>) {
    let (mut c, mut b) = (0, 0u64); let mut ids = vec![];
    if let Ok(rd) = fs::read_dir(dir.join("mods")) {
        for e in rd.flatten() {
            let f = e.file_name().to_string_lossy().to_string();
            if f.ends_with(".jar") { c += 1; b += e.metadata().map(|m| m.len()).unwrap_or(0); ids.push(crate::mods::parse_jar_cached(dir, &e.path(), &f).modid.to_lowercase()); }
        }
    }
    (c, (b as f64 / 1048576.0) as i64, ids)
}
const HEAVY: &[&str] = &["create", "mekanism", "ae2", "terralith", "biomesoplenty", "twilightforest", "immersiveengineering", "thermal", "tconstruct", "botania", "alexsmobs", "ars_nouveau", "tfc", "industrialforegoing"];
fn g(x: f64) -> String { if x == x.trunc() { format!("{}", x as i64) } else { format!("{}", x) } }

pub fn recommend(dir: &Path) -> J {
    let (cnt, mb, ids) = count_mods(dir);
    let mut base = 12.0;
    for (lim, gb) in [(20, 4.0), (60, 5.0), (100, 6.0), (150, 8.0), (250, 10.0)] { if cnt <= lim { base = gb; break; } }
    let mut why: Vec<String> = vec![];
    if cnt > 0 { why.push(format!("{} mods ({} MB of jars) usually want about {} GB.", cnt, mb, base)); } else { why.push("No mods found yet, so this is a light starting point.".into()); }
    let mut want: f64 = base;
    let mut heavy: Vec<&str> = HEAVY.iter().copied().filter(|h| ids.iter().any(|i| i == h)).collect(); heavy.sort();
    if !heavy.is_empty() {
        let add = (0.5 * heavy.len() as f64).min(2.0); want += add;
        why.push(format!("{} big mods ({}) add about {} GB.", heavy.len(), heavy.iter().take(4).cloned().collect::<Vec<_>>().join(", ") + if heavy.len() > 4 { "..." } else { "" }, g(add)));
    }
    let players = fs::read_to_string(dir.join("whitelist.json")).ok().and_then(|t| crate::json::parse(&t)).map(|j| if let J::Arr(a) = j { a.len() } else { 0 }).unwrap_or(0);
    if players >= 16 { want += 2.0; why.push(format!("{players} whitelisted players adds 2 GB.")); }
    else if players >= 8 { want += 1.0; why.push(format!("{players} whitelisted players adds 1 GB.")); }
    let props = read_props(dir);
    let vd: i64 = props.iter().find(|(k, _)| k == "view-distance").and_then(|(_, v)| v.parse().ok()).unwrap_or(10);
    if vd >= 14 { want += 1.0; why.push(format!("View distance {} adds 1 GB.", vd)); }
    let tm = sys_ram_mb();
    let cap = tm.map(|t| { let tg = t as f64 / 1024.0; let c = if tg >= 12.0 { tg * 0.65 } else { (tg - 4.0).max(2.0) }; (c * 2.0).floor() / 2.0 });
    let rec = match cap { Some(c) => want.min(c), None => want };
    let rec = ((rec * 2.0).round() / 2.0).max(2.0);
    let apple = matches!(mac_info().get("apple"), Some(J::Bool(true)));
    let os = if cfg!(target_os = "macos") { "macOS" } else { "the system" };
    if let (Some(c), Some(t)) = (cap, tm) {
        if want > c {
            why.push(format!("That wants {} GB but this {} only has {} GB, so I kept the rest for {}{}.", g(want), if cfg!(target_os = "macos") { "Mac" } else { "computer" }, g((t as f64 / 1024.0).round()), os, if apple { " and the GPU, which shares this memory" } else { "" }));
            why.push("Fewer mods or a computer with more RAM would run these better.".into());
        } else { why.push(format!("That leaves plenty of room for {os}.")); }
    }
    obj(vec![("max_gb", J::Num(rec)), ("start_gb", J::Num(rec)), ("wanted_gb", J::Num(want)), ("cap_gb", cap.map(J::Num).unwrap_or(J::Null)), ("mod_count", n(cnt)), ("mod_mb", n(mb)), ("reasons", J::Arr(why.iter().map(|x| s(x)).collect()))])
}

pub fn jvm_json(dir: &Path) -> J {
    let (xms, xmx, extra, used) = jvm(dir);
    obj(vec![("xms_mb", xms.map(n).unwrap_or(J::Null)), ("xmx_mb", xmx.map(n).unwrap_or(J::Null)), ("extra", J::Arr(extra.iter().map(|x| s(x)).collect())), ("exists", J::Bool(dir.join("user_jvm_args.txt").is_file())), ("file_used", J::Bool(used))])
}

pub fn get(dir: &Path, running: bool, panel: &J) -> J {
    let cur = read_props(dir);
    let mut known: Vec<String> = vec![];
    let mut gl = vec![];
    for g in groups() {
        let mut fl = vec![];
        for f in fields(&g) {
            let k = gs(&f, "key").to_string();
            known.push(k.clone());
            let v = cur.iter().find(|(a, _)| *a == k).map(|(_, v)| v.clone()).unwrap_or_else(|| gs(&f, "default").to_string());
            let mut o = match f { J::Obj(m) => m, _ => vec![] };
            o.push(("value".into(), s(&v)));
            fl.push(J::Obj(o));
        }
        gl.push(obj(vec![("name", s(gs(&g, "name"))), ("fields", J::Arr(fl))]));
    }
    let other: Vec<J> = cur.iter().filter(|(k, _)| !known.contains(k)).map(|(k, v)| {
        let kl = k.to_lowercase();
        let sec = kl.contains("password") || kl.contains("secret") || kl.contains("token");
        obj(vec![("key", s(k)), ("value", s(if sec { "" } else { v })), ("secret", J::Bool(sec))])
    }).collect();
    obj(vec![("groups", J::Arr(gl)), ("other", J::Arr(other)), ("props_exists", J::Bool(dir.join("server.properties").is_file())), ("jvm", jvm_json(dir)),
        ("sys_mb", sys_ram_mb().map(n).unwrap_or(J::Null)), ("running", J::Bool(running)), ("recommended_flags", J::Arr(FLAGS.iter().map(|x| s(x)).collect())),
        ("mac", mac_info()), ("recommend", recommend(dir)), ("panel", panel.clone())])
}

fn validate(spec: &J, val: &J, cur: Option<&String>) -> Result<String, String> {
    let t = gs(spec, "type");
    let raw = match val { J::Str(x) => x.clone(), J::Bool(b) => b.to_string(), J::Num(x) => if x.fract() == 0.0 { (*x as i64).to_string() } else { x.to_string() }, _ => String::new() };
    if t == "bool" { let l = raw.trim().to_lowercase(); return if l == "true" || l == "false" { Ok(l) } else { Err("must be on or off".into()) }; }
    if t == "int" {
        let v: i64 = raw.trim().parse().map_err(|_| "must be a whole number".to_string())?;
        let (lo, hi) = (num(spec, "min").unwrap_or(-2147483648.0) as i64, num(spec, "max").unwrap_or(2147483647.0) as i64);
        if v < lo || v > hi { return Err(format!("must be between {} and {}", lo, hi)); }
        return Ok(v.to_string());
    }
    if raw.contains('\n') || raw.contains('\r') || raw.chars().count() > 600 { return Err("must be one short line".into()); }
    if t == "select" {
        let ok = matches!(spec.get("options"), Some(J::Arr(a)) if a.iter().any(|o| o.as_str() == Some(raw.as_str()))) || cur.map(|c| *c == raw).unwrap_or(false);
        if !ok { return Err("pick one of the listed choices".into()); }
    }
    Ok(raw)
}

/// Returns (ok, message). Panel (mc_version/loader) options are accepted but kept by the caller.
pub fn apply(dir: &Path, body: &J) -> (bool, String) {
    let cur = read_props(dir);
    let mut errors: Vec<String> = vec![];
    let mut updates: Vec<(String, String)> = vec![];
    let specs: Vec<J> = groups().iter().flat_map(fields).collect();
    if let Some(J::Obj(m)) = body.get("props") {
        for (k, v) in m {
            match specs.iter().find(|f| gs(f, "key") == k) {
                None => errors.push(format!("Unknown setting: {}", k)),
                Some(sp) => match validate(sp, v, cur.iter().find(|(a, _)| a == k).map(|(_, v)| v)) { Ok(x) => updates.push((k.clone(), x)), Err(e) => errors.push(format!("{}: {}.", gs(sp, "label"), e)) },
            }
        }
    }
    if let Some(J::Obj(m)) = body.get("other") {
        for (k, v) in m {
            let okk = !k.is_empty() && k.len() <= 80 && k.chars().all(|c| c.is_ascii_alphanumeric() || "._-".contains(c));
            let val = v.as_str().unwrap_or("").to_string();
            if !okk || specs.iter().any(|f| gs(f, "key") == k) { errors.push(format!("Bad setting name: {}", k)); }
            else if val.contains('\n') || val.contains('\r') || val.len() > 1000 { errors.push(format!("{} must be one short line.", k)); }
            else { updates.push((k.clone(), val)); }
        }
    }
    let mut jvm_new: Option<(i64, i64, Vec<String>)> = None;
    if let Some(jv @ J::Obj(_)) = body.get("jvm") {
        let g = |k: &str| num(jv, k).map(|x| x as i64).unwrap_or(0);
        let (xms, xmx) = (g("xms_mb"), g("xmx_mb"));
        if xmx > 0 {
            if xms < 256 || xmx < 512 { errors.push("RAM is too small, use at least 512 MB.".into()); }
            if xms > xmx { errors.push("Minimum RAM can't be bigger than maximum RAM.".into()); }
            if let Some(t) = sys_ram_mb() { if xmx > t { errors.push(format!("This computer only has {:.1} GB of RAM.", t as f64 / 1024.0)); } }
        } else { errors.push("RAM values must be numbers.".into()); }
        let mut clean = vec![];
        if let Some(J::Arr(a)) = jv.get("extra") {
            for l in a {
                let l = l.as_str().unwrap_or("").trim().to_string();
                if l.is_empty() { continue; }
                if !(l.starts_with('-') || l.starts_with('@')) || l.len() > 300 || l.chars().any(|c| (c as u32) < 32) { errors.push(format!("Java option not accepted: {}", l.chars().take(60).collect::<String>())); }
                else if l.starts_with("-Xms") || l.starts_with("-Xmx") { errors.push("Set memory with the sliders, not in the extra options.".into()); }
                else { clean.push(l); }
            }
        }
        jvm_new = Some((xms, xmx, clean));
    }
    if let Some(e) = errors.first() { return (false, e.clone()); }
    if !updates.is_empty() { if let Err(e) = write_props(dir, &updates) { return (false, format!("Couldn't save: {}", e)); } }
    if let Some((xms, xmx, extra)) = jvm_new {
        let p = dir.join("user_jvm_args.txt");
        let comments: Vec<String> = fs::read_to_string(&p).map(|t| t.lines().filter(|l| l.trim().starts_with('#')).map(|l| l.trim_end().to_string()).collect())
            .unwrap_or_else(|_| vec!["# Written by the Create Server Panel. One Java option per line.".into()]);
        let _ = fs::copy(&p, dir.join("user_jvm_args.txt.panel-bak"));
        let arg = |f: &str, mb: i64| if mb % 1024 == 0 { format!("-{}{}G", f, mb / 1024) } else { format!("-{}{}M", f, mb) };
        let mut lines = comments; lines.push(arg("Xms", xms)); lines.push(arg("Xmx", xmx)); lines.extend(extra);
        let tmp = dir.join("user_jvm_args.txt.panel-tmp");
        if let Err(e) = fs::write(&tmp, lines.join("\n") + "\n").and_then(|_| fs::rename(&tmp, &p)) { return (false, format!("Couldn't save: {}", e)); }
    }
    (true, "Saved.".into())
}
