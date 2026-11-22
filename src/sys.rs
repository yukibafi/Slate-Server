//! Shared system helpers (std only). HTTPS goes through the system `curl`; archives through
//! `zip`/`unzip` (macOS, Linux) or `tar` (Windows 10+, bsdtar reads and writes zip).
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

pub fn now() -> f64 { SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0) }
pub fn home() -> String { std::env::var("HOME").or_else(|_| std::env::var("USERPROFILE")).unwrap_or_else(|_| ".".into()) }

/// Run a program, return (exit code, stdout). None if it can't be started.
pub fn run(prog: &str, args: &[&str], cwd: Option<&Path>) -> Option<(i32, Vec<u8>)> {
    let mut c = Command::new(prog);
    c.args(args).stdin(Stdio::null()).stderr(Stdio::null());
    if let Some(d) = cwd { c.current_dir(d); }
    let o = c.output().ok()?;
    Some((o.status.code().unwrap_or(-1), o.stdout))
}
pub fn have(prog: &str) -> bool {
    let probe = if cfg!(windows) { "where" } else { "which" };
    run(probe, &[prog], None).map(|(c, _)| c == 0).unwrap_or(false)
}

/// GET/POST over http(s) with curl. Returns (status, body). Status 0 = couldn't connect.
pub fn http(method: &str, url: &str, headers: &[(&str, &str)], body: Option<&[u8]>, timeout: u32) -> (u16, Vec<u8>) {
    let t = timeout.to_string();
    let mut args: Vec<String> = vec!["-sS".into(), "-L".into(), "--max-time".into(), t, "-X".into(), method.into(), "-w".into(), "\n%{http_code}".into()];
    for (k, v) in headers { args.push("-H".into()); args.push(format!("{}: {}", k, v)); }
    if body.is_some() { args.push("--data-binary".into()); args.push("@-".into()); }
    args.push(url.into());
    let mut c = Command::new("curl");
    c.args(&args).stdin(if body.is_some() { Stdio::piped() } else { Stdio::null() }).stdout(Stdio::piped()).stderr(Stdio::null());
    let mut ch = match c.spawn() { Ok(c) => c, Err(_) => return (0, vec![]) };
    if let (Some(b), Some(mut si)) = (body, ch.stdin.take()) { let _ = si.write_all(b); }
    let o = match ch.wait_with_output() { Ok(o) => o, Err(_) => return (0, vec![]) };
    let mut out = o.stdout;
    let nl = match out.iter().rposition(|&b| b == b'\n') { Some(i) => i, None => return (0, vec![]) };
    let code: u16 = String::from_utf8_lossy(&out[nl + 1..]).trim().parse().unwrap_or(0);
    out.truncate(nl);
    (code, out)
}
pub fn get(url: &str, timeout: u32) -> (u16, Vec<u8>) { http("GET", url, &[("User-Agent", "create-panel-rs/0.1")], None, timeout) }

/// Download to a file (via a .part temp), returns Err(message) on failure.
pub fn download(url: &str, dest: &Path, timeout: u32) -> Result<(), String> {
    let tmp = dest.with_extension("part");
    let r = run("curl", &["-sS", "-L", "-f", "--max-time", &timeout.to_string(), "-A", "create-panel-rs/0.1", "-o", &tmp.to_string_lossy(), url], None);
    match r { Some((0, _)) => fs::rename(&tmp, dest).map_err(|e| e.to_string()), _ => { let _ = fs::remove_file(&tmp); Err("download failed".into()) } }
}

// ---------------------------------------------------------------- hashes
pub fn sha1(data: &[u8]) -> [u8; 20] {
    let mut h: [u32; 5] = [0x67452301, 0xEFCDAB89, 0x98BADCFE, 0x10325476, 0xC3D2E1F0];
    let mut m = data.to_vec();
    let bits = (data.len() as u64) * 8;
    m.push(0x80);
    while m.len() % 64 != 56 { m.push(0); }
    m.extend_from_slice(&bits.to_be_bytes());
    for chunk in m.chunks(64) {
        let mut w = [0u32; 80];
        for i in 0..16 { w[i] = u32::from_be_bytes([chunk[4 * i], chunk[4 * i + 1], chunk[4 * i + 2], chunk[4 * i + 3]]); }
        for i in 16..80 { w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1); }
        let (mut a, mut b, mut c, mut d, mut e) = (h[0], h[1], h[2], h[3], h[4]);
        for i in 0..80 {
            let (f, k) = match i { 0..=19 => ((b & c) | (!b & d), 0x5A827999u32), 20..=39 => (b ^ c ^ d, 0x6ED9EBA1), 40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1BBCDC), _ => (b ^ c ^ d, 0xCA62C1D6) };
            let t = a.rotate_left(5).wrapping_add(f).wrapping_add(e).wrapping_add(k).wrapping_add(w[i]);
            e = d; d = c; c = b.rotate_left(30); b = a; a = t;
        }
        h[0] = h[0].wrapping_add(a); h[1] = h[1].wrapping_add(b); h[2] = h[2].wrapping_add(c); h[3] = h[3].wrapping_add(d); h[4] = h[4].wrapping_add(e);
    }
    let mut o = [0u8; 20];
    for i in 0..5 { o[4 * i..4 * i + 4].copy_from_slice(&h[i].to_be_bytes()); }
    o
}
pub fn hex(b: &[u8]) -> String { b.iter().map(|x| format!("{:02x}", x)).collect() }
pub fn sha1_file(p: &Path) -> Option<String> { fs::read(p).ok().map(|d| hex(&sha1(&d))) }
/// sha512 / sha256 via the system tool (shasum / sha*sum / certutil).
pub fn sha_file(p: &Path, bits: u32) -> Option<String> {
    let ps = p.to_string_lossy().to_string();
    let b = bits.to_string();
    if let Some((0, o)) = run("shasum", &["-a", &b, &ps], None) { return String::from_utf8_lossy(&o).split_whitespace().next().map(|s| s.to_lowercase()); }
    if let Some((0, o)) = run(&format!("sha{}sum", bits), &[&ps], None) { return String::from_utf8_lossy(&o).split_whitespace().next().map(|s| s.to_lowercase()); }
    if let Some((0, o)) = run("certutil", &["-hashfile", &ps, &format!("SHA{}", bits)], None) { return String::from_utf8_lossy(&o).lines().nth(1).map(|s| s.replace(' ', "").to_lowercase()); }
    None
}

// ---------------------------------------------------------------- zip
/// Zip the *contents* of `src` into `out`. `exclude` are glob-ish patterns (zip -x syntax, relative paths).
pub fn zip_dir(src: &Path, out: &Path, exclude: &[&str]) -> Result<(), String> {
    let _ = fs::remove_file(out);
    let outs = out.to_string_lossy().to_string();
    if have("zip") {
        let mut a: Vec<String> = vec!["-r".into(), "-q".into(), outs.clone(), ".".into()];
        if !exclude.is_empty() { a.push("-x".into()); for e in exclude { a.push((*e).into()); } }
        let ar: Vec<&str> = a.iter().map(|s| s.as_str()).collect();
        return match run("zip", &ar, Some(src)) { Some((c, _)) if c == 0 || c == 12 => Ok(()), _ => Err("zip failed".into()) };
    }
    let mut a: Vec<String> = vec!["-a".into(), "-c".into(), "-f".into(), outs];
    for e in exclude { a.push("--exclude".into()); a.push((*e).into()); }
    a.push(".".into());
    let ar: Vec<&str> = a.iter().map(|s| s.as_str()).collect();
    match run("tar", &ar, Some(src)) { Some((0, _)) => Ok(()), _ => Err("zip failed".into()) }
}
pub fn unzip_to(zip: &Path, dest: &Path) -> Result<(), String> {
    let _ = fs::create_dir_all(dest);
    let (z, d) = (zip.to_string_lossy().to_string(), dest.to_string_lossy().to_string());
    if have("unzip") { return match run("unzip", &["-q", "-o", &z, "-d", &d], None) { Some((c, _)) if c == 0 || c == 1 => Ok(()), _ => Err("unzip failed".into()) }; }
    match run("tar", &["-x", "-f", &z, "-C", &d], None) { Some((0, _)) => Ok(()), _ => Err("unzip failed".into()) }
}
/// Read one file out of a zip/jar.
pub fn zip_read(zip: &Path, entry: &str) -> Option<Vec<u8>> {
    let z = zip.to_string_lossy().to_string();
    if have("unzip") { return match run("unzip", &["-p", &z, entry], None) { Some((0, o)) => Some(o), _ => None }; }
    match run("tar", &["-x", "-O", "-f", &z, entry], None) { Some((0, o)) => Some(o), _ => None }
}
/// Names of all entries in a zip/jar.
pub fn zip_list(zip: &Path) -> Vec<String> {
    let z = zip.to_string_lossy().to_string();
    if have("unzip") { if let Some((0, o)) = run("unzip", &["-Z1", &z], None) { return String::from_utf8_lossy(&o).lines().map(|s| s.to_string()).collect(); } }
    match run("tar", &["-t", "-f", &z], None) { Some((0, o)) => String::from_utf8_lossy(&o).lines().map(|s| s.to_string()).collect(), _ => vec![] }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn sha1_abc() { assert_eq!(hex(&sha1(b"abc")), "a9993e364706816aba3e25717850c26c9cd0d89d"); assert_eq!(hex(&sha1(&vec![b'a'; 1000])), "291e9a6c66994949b57ba5e650361e98fc36b1ba"); }
    #[test] fn zip_roundtrip() {
        let d = std::path::PathBuf::from("/tmp/t1/d"); let z = std::path::PathBuf::from("/tmp/t1/o.zip");
        zip_dir(&d, &z, &["sub/*"]).unwrap();
        let l = zip_list(&z); assert!(l.iter().any(|x| x.ends_with("a.txt"))); assert!(!l.iter().any(|x| x.contains("b.txt")));
        assert_eq!(zip_read(&z, "a.txt").unwrap(), b"hello\n");
        unzip_to(&z, std::path::Path::new("/tmp/t1/out")).unwrap(); assert!(std::path::Path::new("/tmp/t1/out/a.txt").is_file());
    }
    #[test] fn curl_local() {
        let (c, _) = get("http://127.0.0.1:9/", 3); assert_eq!(c, 0);
    }
}

/// UTC timestamp like 20261110-142501 (used for trash folders).
pub fn tz_offset() -> i64 {
    use std::sync::OnceLock;
    static O: OnceLock<i64> = OnceLock::new();
    *O.get_or_init(|| {
        let o = std::process::Command::new("date").arg("+%z").output().ok()
            .and_then(|o| String::from_utf8(o.stdout).ok()).unwrap_or_default();
        let o = o.trim();
        if o.len() == 5 && (o.starts_with('+') || o.starts_with('-')) {
            let h: i64 = o[1..3].parse().unwrap_or(0); let m: i64 = o[3..5].parse().unwrap_or(0);
            let v = h * 3600 + m * 60; if o.starts_with('-') { -v } else { v }
        } else { 0 }
    })
}

/// Local-time stamp YYYYMMDD-HHMMSS (matches the Python panel's folder names).
pub fn stamp() -> String {
    let t = now() as i64 + tz_offset();
    let (days, secs) = (t.div_euclid(86400), t.rem_euclid(86400));
    // civil-from-days (Howard Hinnant)
    let z = days + 719468; let era = z.div_euclid(146097); let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365; let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1; let m = if mp < 10 { mp + 3 } else { mp - 9 }; let y = if m <= 2 { y + 1 } else { y };
    format!("{:04}{:02}{:02}-{:02}{:02}{:02}", y, m, d, secs / 3600, (secs % 3600) / 60, secs % 60)
}

/// Zip a folder *including* its own name as the top entry (so unzipping gives back the folder).
pub fn zip_folder(parent: &Path, name: &str, out: &Path, exclude: &[&str]) -> Result<(), String> {
    let _ = fs::remove_file(out);
    let outs = out.to_string_lossy().to_string();
    if have("zip") {
        let mut a: Vec<String> = vec!["-r".into(), "-q".into(), "-y".into(), outs, name.into()];
        if !exclude.is_empty() { a.push("-x".into()); for e in exclude { a.push((*e).into()); } }
        let ar: Vec<&str> = a.iter().map(|s| s.as_str()).collect();
        return match run("zip", &ar, Some(parent)) { Some((c, _)) if c == 0 || c == 12 => Ok(()), _ => Err("zip failed".into()) };
    }
    let mut a: Vec<String> = vec!["-a".into(), "-c".into(), "-f".into(), outs];
    for e in exclude { a.push("--exclude".into()); a.push((*e).into()); }
    a.push(name.into());
    let ar: Vec<&str> = a.iter().map(|s| s.as_str()).collect();
    match run("tar", &ar, Some(parent)) { Some((0, _)) => Ok(()), _ => Err("zip failed".into()) }
}

/// Free bytes on the disk holding `p` (Mac/Linux via df; None elsewhere).
pub fn free_space(p: &Path) -> Option<u64> {
    if cfg!(windows) { return None; }
    let (_, out) = run("df", &["-k", &p.to_string_lossy()], None)?;
    let txt = String::from_utf8_lossy(&out).to_string();
    let l = txt.lines().last()?;
    let f: Vec<&str> = l.split_whitespace().collect();
    f.get(3)?.parse::<u64>().ok().map(|k| k * 1024)
}

/// Minimal glob: `*` matches within one path segment. Results sorted.
pub fn glob(pattern: &str) -> Vec<PathBuf> {
    let norm = pattern.replace('\\', "/");
    let (mut cur, rest): (Vec<PathBuf>, &str) = if let Some(r) = norm.strip_prefix('/') { (vec![PathBuf::from("/")], r) } else { (vec![PathBuf::new()], norm.as_str()) };
    for seg in rest.split('/') {
        let mut next = vec![];
        for base in &cur {
            if seg.contains('*') {
                let (pre, post) = seg.split_once('*').unwrap_or((seg, ""));
                let dir = if base.as_os_str().is_empty() { PathBuf::from(".") } else { base.clone() };
                if let Ok(rd) = fs::read_dir(&dir) {
                    let mut names: Vec<String> = rd.flatten().map(|e| e.file_name().to_string_lossy().to_string()).collect(); names.sort();
                    for nm in names { if nm.starts_with(pre) && nm.ends_with(post) && nm.len() >= pre.len() + post.len() && !nm.starts_with('.') { next.push(base.join(&nm)); } }
                }
            } else { let p = base.join(seg); if p.exists() { next.push(p); } }
        }
        cur = next;
    }
    cur
}

/// Full path of a program on PATH.
pub fn which(prog: &str) -> Option<String> {
    let path = std::env::var_os("PATH")?;
    let names: Vec<String> = if cfg!(windows) { vec![format!("{prog}.exe"), format!("{prog}.cmd"), prog.to_string()] } else { vec![prog.to_string()] };
    for d in std::env::split_paths(&path) { for nm in &names { let p = d.join(nm); if p.is_file() { return Some(p.to_string_lossy().to_string()); } } }
    None
}

/// Like `run` but stdout and stderr merged (what `java -version` needs).
pub fn run_both(prog: &str, args: &[&str]) -> Option<(i32, Vec<u8>)> {
    let o = Command::new(prog).args(args).stdin(Stdio::null()).output().ok()?;
    let mut b = o.stdout; b.extend(o.stderr);
    Some((o.status.code().unwrap_or(-1), b))
}
