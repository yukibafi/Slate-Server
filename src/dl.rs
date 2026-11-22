//! Allow-listed HTTPS fetching for loader/mod/pack services. `PANEL_HOSTMAP=host=http://127.0.0.1:PORT,...` redirects a host (for tests).
use crate::json::{self, J};
use crate::sys;
use std::fs;
use std::path::Path;

pub const UA: &str = "CreatePanel/1.0 (+https://github.com/)";
pub const BROWSER_UA: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Safari/605.1.15";
pub const LOADER_HOSTS: &[&str] = &["meta.fabricmc.net", "maven.fabricmc.net", "maven.neoforged.net", "maven.minecraftforge.net", "files.minecraftforge.net", "meta.quiltmc.org", "maven.quiltmc.org",
    "fill.papermc.io", "fill-data.papermc.io", "api.purpurmc.org", "piston-meta.mojang.com", "piston-data.mojang.com", "launchermeta.mojang.com", "launcher.mojang.com"];

pub fn host_of(url: &str) -> String { url.split("://").nth(1).unwrap_or("").split(['/', '?', '#']).next().unwrap_or("").split('@').last().unwrap_or("").split(':').next().unwrap_or("").to_lowercase() }
pub fn allowed(url: &str, hosts: &[&str]) -> bool { url.starts_with("https://") && hosts.contains(&host_of(url).as_str()) }
pub fn map_url(url: &str) -> String {
    if let Ok(m) = std::env::var("PANEL_HOSTMAP") {
        let h = host_of(url);
        for pair in m.split(',') { if let Some((k, base)) = pair.split_once('=') { if k.trim() == h {
            let rest = &url[url.find("://").map(|i| i + 3).unwrap_or(0)..]; let path = rest.find('/').map(|i| &rest[i..]).unwrap_or("");
            return format!("{}{}", base.trim_end_matches('/'), path);
        } } }
    }
    url.to_string()
}
fn is_zip(p: &Path) -> bool { fs::read(p).map(|d| d.len() >= 4 && &d[..2] == b"PK" && matches!((d[2], d[3]), (3, 4) | (5, 6)) || d.windows(4).rev().take(70000).any(|w| w == b"PK\x05\x06")).unwrap_or(false) }

pub fn get_bytes(url: &str, hosts: &[&str], timeout: u32) -> Result<Vec<u8>, String> {
    if !allowed(url, hosts) { return Err("Unexpected download address, skipped.".into()); }
    let h = host_of(url);
    let (code, b) = sys::http("GET", &map_url(url), &[("User-Agent", UA), ("Accept", "application/json")], None, timeout);
    match code { 0 => Err(format!("Couldn't reach {h}. Check the internet connection.")), 200..=299 => Ok(b), c => Err(format!("{h} said HTTP {c} for that.")) }
}
pub fn get_json(url: &str, hosts: &[&str]) -> Result<J, String> {
    let b = get_bytes(url, hosts, 30)?;
    json::parse(&String::from_utf8_lossy(&b)).ok_or_else(|| format!("Got an unreadable answer from {}.", host_of(url)))
}

/// Download `url` to `dest`. `need_zip` rejects anything that isn't a jar/zip. Optional checksums.
pub fn download(url: &str, dest: &Path, hosts: &[&str], need_zip: bool, sha1: Option<&str>, sha512: Option<&str>, max_mb: u64) -> Result<(), String> {
    let url = url.replace(' ', "%20");
    if !allowed(&url, hosts) { return Err("Unexpected download address, skipped.".into()); }
    let h = host_of(&url);
    let tmp = dest.with_extension("part");
    let mut code = 0;
    for ua in [UA, BROWSER_UA] {
        let _ = fs::remove_file(&tmp);
        let r = sys::run("curl", &["-sS", "-L", "--max-time", "900", "-A", ua, "--max-filesize", &(max_mb * 1048576).to_string(), "-o", &tmp.to_string_lossy(), "-w", "%{http_code}", &map_url(&url)], None);
        code = r.map(|(_, o)| String::from_utf8_lossy(&o).trim().parse::<u16>().unwrap_or(0)).unwrap_or(0);
        if code != 403 { break; }
    }
    if code == 0 { let _ = fs::remove_file(&tmp); return Err("Download failed. Check the internet connection.".into()); }
    if !(200..300).contains(&code) { let _ = fs::remove_file(&tmp); return Err(format!("{h} said HTTP {code} for that download.")); }
    let len = fs::metadata(&tmp).map(|m| m.len()).unwrap_or(0);
    if len > max_mb * 1048576 { let _ = fs::remove_file(&tmp); return Err("That download is far bigger than expected, stopped.".into()); }
    if let Some(w) = sha512 { if sys::sha_file(&tmp, 512).map(|x| x != w.to_lowercase()).unwrap_or(true) { let _ = fs::remove_file(&tmp); return Err("Downloaded file didn't match its checksum.".into()); } }
    else if let Some(w) = sha1 { if sys::sha1_file(&tmp).map(|x| x != w.to_lowercase()).unwrap_or(true) { let _ = fs::remove_file(&tmp); return Err("Downloaded file didn't match its checksum.".into()); } }
    if need_zip && (len < 1024 || !is_zip(&tmp)) { let _ = fs::remove_file(&tmp); return Err("What came down wasn't a valid jar file.".into()); }
    fs::rename(&tmp, dest).map_err(|e| e.to_string())
}
