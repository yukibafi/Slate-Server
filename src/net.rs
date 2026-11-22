//! LAN address detection, firewall/netmask checks, public IP.
use crate::sys;
use std::net::UdpSocket;
use std::sync::Mutex;

const SKIP: &[&str] = &["lo", "utun", "awdl", "llw", "bridge", "vmenet", "vmnet", "docker", "vboxnet", "gif", "stf", "anpi", "ap", "ipsec", "veth", "br-", "tailscale", "zt"];
pub const NETMASK_HELP: &str = "This Mac's network connection has a netmask of 255.255.255.255 (/32). That makes it think no other device is on its network, so phones and PCs can't connect even though the panel is running. Fix: System Settings > Network > Wi-Fi > Details > TCP/IP > click 'Renew DHCP Lease'. If it stays /32, set 'Configure IPv4' to Manually, keep the same IP, set Subnet mask to 255.255.252.0 (or whatever your PC's ipconfig shows) and Router to 192.168.4.1.";

pub fn private(ip: &str) -> bool {
    let p: Vec<i64> = ip.split('.').take(2).filter_map(|x| x.parse().ok()).collect();
    p.len() == 2 && (p[0] == 10 || (p[0] == 172 && (16..=31).contains(&p[1])) || (p[0] == 192 && p[1] == 168))
}
fn ifconfig() -> String { if cfg!(windows) { String::new() } else { sys::run("ifconfig", &[], None).map(|x| String::from_utf8_lossy(&x.1).to_string()).unwrap_or_default() } }

/// (interface, ip, netmask-hex) for every active real connection.
fn iface_ips() -> Vec<(String, String, String)> {
    let mut v = vec![]; let (mut name, mut active) = (String::new(), true);
    for l in ifconfig().lines() {
        if !l.is_empty() && !l.starts_with(char::is_whitespace) { name = l.split(':').next().unwrap_or("").to_string(); active = true; }
        else if l.contains("status:") && l.contains("inactive") { active = false; }
        let t = l.trim_start();
        if l.starts_with(char::is_whitespace) { if let Some(r) = t.strip_prefix("inet ") {
            let mut it = r.split_whitespace(); let ip = it.next().unwrap_or("").to_string();
            let mask = if it.next() == Some("netmask") { it.next().unwrap_or("").to_string() } else { String::new() };
            if active && !name.is_empty() && !SKIP.iter().any(|s| name.starts_with(s)) { v.push((name.clone(), ip, mask)); }
        } }
    }
    v
}
pub fn bad_netmask() -> Vec<(String, String)> { iface_ips().into_iter().filter(|(_, ip, m)| m.eq_ignore_ascii_case("0xffffffff") && private(ip)).map(|(n, ip, _)| (n, ip)).collect() }

static CACHE: Mutex<(f64, Vec<String>)> = Mutex::new((0.0, Vec::new()));
pub fn lan_ips() -> Vec<String> {
    { let c = CACHE.lock().unwrap(); if sys::now() - c.0 < 10.0 && !c.1.is_empty() { return c.1.clone(); } }
    let mut ips: Vec<String> = vec![];
    let mut add = |ip: &str| { let ip = ip.trim(); if !ip.is_empty() && !ip.starts_with("127.") && !ip.starts_with("169.254.") && !ip.starts_with("0.") && !ips.iter().any(|x| x == ip) { ips.push(ip.to_string()); } };
    let mut pairs = iface_ips();
    pairs.sort_by_key(|p| (if p.0 == "en0" { 0 } else if p.0.starts_with("en") { 1 } else { 2 }, p.0.clone()));
    let routed = ["8.8.8.8:80", "10.255.255.255:80"].iter().find_map(|t| { let s = UdpSocket::bind("0.0.0.0:0").ok()?; s.connect(t).ok()?; Some(s.local_addr().ok()?.ip().to_string()) });
    if let Some(r) = &routed { if private(r) { add(r); } }
    for p in &pairs { if private(&p.1) { add(&p.1); } }
    let empty = { let _ = &add; false };
    let _ = empty;
    drop(add);
    if ips.is_empty() { for p in &pairs { let ip = p.1.trim(); if !ip.starts_with("127.") && !ip.starts_with("169.254.") && !ip.starts_with("0.") && !ips.contains(&ip.to_string()) { ips.push(ip.to_string()); } } if let Some(r) = &routed { if !ips.contains(r) { ips.push(r.clone()); } } }
    *CACHE.lock().unwrap() = (sys::now(), ips.clone());
    ips
}

// ---- firewall (macOS)
const FW: &str = "/usr/libexec/ApplicationFirewall/socketfilterfw";
fn fw(args: &[&str]) -> String { sys::run(FW, args, None).map(|x| String::from_utf8_lossy(&x.1).trim().to_string()).unwrap_or_default() }
pub fn fw_paths() -> Vec<String> { std::env::current_exe().ok().and_then(|p| std::fs::canonicalize(p).ok()).map(|p| vec![p.to_string_lossy().to_string()]).unwrap_or_default() }
pub fn fw_status() -> Option<crate::json::J> {
    use crate::json::{obj, s, J};
    if !cfg!(target_os = "macos") || !std::path::Path::new(FW).exists() { return None; }
    let paths = fw_paths();
    let (g, b, st) = (fw(&["--getglobalstate"]), fw(&["--getblockall"]), fw(&["--getstealthmode"]));
    let on = g.to_lowercase().contains("enabled"); let blockall = b.to_lowercase().contains("enabled");
    let apps: Vec<(String, String)> = paths.iter().map(|p| (p.clone(), fw(&["--getappblocked", p]))).collect();
    let permitted = apps.iter().any(|a| a.1.to_lowercase().contains("permitted"));
    Some(obj(vec![("on", J::Bool(on)), ("global", s(&g)), ("blockall", s(&b)), ("blockall_on", J::Bool(blockall)), ("stealth", s(&st)), ("paths", J::Arr(paths.iter().map(|p| s(p)).collect())),
        ("apps", J::Obj(apps.into_iter().map(|(k, v)| (k, s(&v))).collect())), ("ok", J::Bool(!on || (permitted && !blockall)))]))
}
pub fn fw_fix() -> (bool, String) {
    let Some(st) = fw_status() else { return (false, "Not a Mac.".into()) };
    let _ = st;
    let cmds: Vec<String> = fw_paths().iter().map(|p| format!("'{FW}' --add '{p}'; '{FW}' --unblockapp '{p}'")).collect();
    let script = format!("do shell script \"{}\" with administrator privileges", cmds.join(" ; ").replace('\\', "\\\\").replace('"', "\\\""));
    match sys::run("osascript", &["-e", &script], None) { Some((0, _)) => {}, _ => return (false, "The password box was cancelled or failed.".into()) }
    let ok = matches!(fw_status().and_then(|j| j.get("ok").cloned()), Some(crate::json::J::Bool(true)));
    (ok, if ok { "Firewall updated.".into() } else { "Changed it, but macOS still reports it blocked.".into() })
}

// ---- public IP
struct Pub { ip: Option<String>, state: &'static str, next: f64, busy: bool }
static PUB: Mutex<Pub> = Mutex::new(Pub { ip: None, state: "pending", next: 0.0, busy: false });
pub fn public_ip() -> (Option<String>, &'static str) {
    let go = { let mut p = PUB.lock().unwrap(); if sys::now() >= p.next && !p.busy { p.busy = true; true } else { false } };
    if go { std::thread::spawn(|| {
        let urls = std::env::var("PANEL_PUBIP_URLS").map(|v| v.split(',').map(|x| x.to_string()).collect()).unwrap_or_else(|_| vec!["https://api.ipify.org".into(), "https://ipv4.icanhazip.com".into(), "https://checkip.amazonaws.com".into()]);
        let mut ip = None;
        for u in urls { if let (200, b) = sys::get(&u, 5) { let t = String::from_utf8_lossy(&b).trim().to_string(); let parts: Vec<&str> = t.split('.').collect(); if parts.len() == 4 && parts.iter().all(|x| !x.is_empty() && x.len() <= 3 && x.parse::<u16>().map(|v| v < 256).unwrap_or(false)) { ip = Some(t); break; } } }
        let mut p = PUB.lock().unwrap();
        match ip { Some(i) => { p.ip = Some(i); p.state = "ok"; p.next = sys::now() + 600.0; } None => { p.state = if p.ip.is_some() { "ok" } else { "fail" }; p.next = sys::now() + 60.0; } }
        p.busy = false;
    }); }
    let p = PUB.lock().unwrap(); (p.ip.clone(), p.state)
}
