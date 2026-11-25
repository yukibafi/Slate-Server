// Create Panel desktop app. All logic lives in the engine (the `create-panel` binary, ../src); this
// file only shows its state and sends it commands. The Mac app uses the engine on this computer; the
// Windows app (same code) connects to the Mac's engine, so both always show the same servers.
#![cfg_attr(windows, windows_subsystem = "windows")] // never open a console window, in test builds either
slint::include_modules!();

use create_panel::client::Client;
use create_panel::json::{self, n, obj, s, J};
use slint::{Color, ComponentHandle, Model, ModelRc, SharedString, VecModel, Weak};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

mod extra;
mod people;
mod dc;
mod icons;
mod tools;
mod hubpage;
mod hubmisc;
mod loaderpick;
mod theming;
mod appupd;

fn port() -> u16 { std::env::var("PANEL_PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(8080) }
const LOADERS: [&str; 6] = ["neoforge", "forge", "fabric", "quilt", "paper", "vanilla"];
const PALETTES: [(&str, u32); 20] = [("ice", 0x7cc4ff), ("amber", 0xf0b24a), ("rose", 0xf27a98), ("slate", 0x9aa7b8), ("stone", 0xc2b8a8), ("violet", 0xa78bfa), ("teal", 0x2dd4bf), ("ember", 0xff7a45), ("forest", 0x6fcf7f), ("lime", 0xb5e04a), ("orange", 0xff9a52), ("crimson", 0xe5484d), ("pink", 0xf472b6), ("indigo", 0x8b8cf8), ("ocean", 0x3b82f6), ("cyan", 0x22d3ee), ("mint", 0x6ee7b7), ("gold", 0xf5c542), ("lavender", 0xc4b5fd), ("sand", 0xd9b99b)];

#[derive(Default)]
struct Sel { id: String, tab: String, since: usize, lines: Vec<String> }
#[derive(Default)]
struct St { client: Option<Client>, sel: Sel, gen: u64, mods: Vec<ModRow>, cards_all: Vec<SrvCard>, x: extra::Extra, fdata: Vec<(String, Vec<FD>)>, edits: HashMap<String, String>, open: HashSet<String> }
#[derive(Clone, Default)]
struct FD { key: String, label: String, help: String, kind: i32, value: String, options: Vec<String> }
type Shared = Arc<Mutex<St>>;

fn cfg_path() -> std::path::PathBuf {
    let home = std::env::var("HOME").or_else(|_| std::env::var("USERPROFILE")).unwrap_or_else(|_| ".".into());
    std::path::PathBuf::from(home).join(".create-panel-app.json")
}
fn load_cfg() -> J { std::fs::read_to_string(cfg_path()).ok().and_then(|t| json::parse(&t)).unwrap_or(J::Null) }
fn save_cfg(mode: &str, host: &str, key: &str) { let _ = std::fs::write(cfg_path(), obj(vec![("mode", s(mode)), ("host", s(host)), ("key", s(key))]).to_string()); }
fn gs(j: &J, k: &str) -> String { match j.get(k) { Some(J::Str(t)) => t.clone(), Some(J::Num(x)) => format!("{}", *x as i64), _ => String::new() } }
fn gn(j: &J, k: &str) -> i64 { match j.get(k) { Some(J::Num(x)) => *x as i64, _ => 0 } }
fn arr(j: &J, k: &str) -> Vec<J> { match j.get(k) { Some(J::Arr(a)) => a.clone(), _ => vec![] } }
fn msg_of(r: &Result<J, create_panel::client::Err>) -> String { match r { Ok(j) => { let m = gs(j, "msg"); if m.is_empty() { gs(j, "error") } else { m } } Err(e) => e.to_string() } }
fn ok_of(r: &Result<J, create_panel::client::Err>) -> bool { matches!(r, Ok(j) if matches!(j.get("ok"), Some(J::Bool(true)))) }

fn client(st: &Shared) -> Option<Client> { st.lock().unwrap().client.clone() }
fn sc(c: &Client, id: &str, rest: &str) -> String { let _ = c; format!("/api/s/{id}{rest}") }
fn mb(b: i64) -> String { if b >= 1 << 30 { format!("{:.1} GB", b as f64 / (1u64 << 30) as f64) } else { format!("{:.1} MB", b as f64 / (1u64 << 20) as f64) } }

fn with_ui<F: FnOnce(&App) + Send + 'static>(ui: &Weak<App>, f: F) { let _ = ui.upgrade_in_event_loop(move |a| f(&a)); }
fn say(ui: &Weak<App>, t: &str) { let t = t.to_string(); with_ui(ui, move |a| a.set_note(SharedString::from(t))); }

fn apply_palette(a: &App, name: &str) {
    let hex = PALETTES.iter().find(|p| p.0 == name).map(|p| p.1).unwrap_or(0x7cc4ff);
    let th = a.global::<Theme>();
    th.set_accent(Color::from_rgb_u8((hex >> 16) as u8, (hex >> 8) as u8, hex as u8));
    a.set_palette(SharedString::from(name));
}

// ---------------------------------------------------------------- engine
fn engine_dir_key() -> std::path::PathBuf { create_panel::theme::data_dir().join("key") }

fn read_key() -> String { std::fs::read_to_string(engine_dir_key()).unwrap_or_default().trim().to_string() }
fn ensure_local_engine() -> Result<(Client, String), String> {
    // The engine rejects requests without its key, so always probe with the saved key.
    let up = |k: &str| Client::new("127.0.0.1", port(), k).ping().is_ok();
    let key = read_key();
    if !key.is_empty() && up(&key) { return Ok((Client::new("127.0.0.1", port(), &key), key)); }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let dir = exe.parent().ok_or("no folder")?;
    let name = if cfg!(windows) { "create-panel.exe" } else { "create-panel" };
    let eng = dir.join(name);
    if !eng.is_file() { return Err(format!("I can't find the engine ({name}) next to this app.")); }
    let mut cmd = std::process::Command::new(eng);
    cmd.env("PANEL_BACKGROUND", "1").stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    #[cfg(unix)] { use std::os::unix::process::CommandExt; cmd.process_group(0); }
    #[cfg(windows)] { use std::os::windows::process::CommandExt; cmd.creation_flags(0x0000_0008 | 0x0800_0000); }
    cmd.spawn().map_err(|e| format!("Couldn't start the engine: {e}"))?;
    for _ in 0..60 { thread::sleep(Duration::from_millis(250)); let k = read_key(); if !k.is_empty() && up(&k) { return Ok((Client::new("127.0.0.1", port(), &k), k)); } }
    Err("The engine didn't start, or something else is using that port (the old panel uses 8080). Quit the old panel, or start this app with PANEL_PORT=8090.".into())
}

fn after_connect(ui: &Weak<App>, st: &Shared, c: Client, key: String, host_label: String) {
    let theme = c.get("/api/theme").ok();
    let ever = c.get("/api/ping").ok().map(|j| gs(&j, "version")).unwrap_or_default();
    let auto = c.get("/api/autostart").ok().map(|j| matches!(j.get("enabled"), Some(J::Bool(true)))).unwrap_or(false);
    { let mut g = st.lock().unwrap(); g.client = Some(c); g.gen += 1; g.sel = Sel::default(); }
    with_ui(ui, move |a| {
        a.set_connected(true); a.set_status(SharedString::new()); a.set_remote_key(SharedString::from(key)); a.set_where_text(SharedString::from(host_label));
        a.set_engine_autostart(auto);
        a.set_ver_text(SharedString::from(format!("App {}  ·  Engine {}", env!("CARGO_PKG_VERSION"), ever)));
        if let Some(t) = theme { theming::apply_saved(a, &t); }
        a.set_page(SharedString::from("hub"));
    });
}

fn connect_local(ui: Weak<App>, st: Shared) {
    thread::spawn(move || match ensure_local_engine() {
        Ok((c, key)) => { save_cfg("local", "", ""); after_connect(&ui, &st, c, key, "This computer".into()); }
        Err(e) => with_ui(&ui, move |a| a.set_status(SharedString::from(e))),
    });
}
fn connect_remote(ui: Weak<App>, st: Shared, host: String, key: String) {
    thread::spawn(move || {
        let host = host.trim().trim_start_matches("http://").trim_end_matches('/').to_string();
        let (h, p) = match host.rsplit_once(':') { Some((h, p)) if p.parse::<u16>().is_ok() => (h.to_string(), p.parse().unwrap()), _ => (host.clone(), port()) };
        let c = Client::new(&h, p, key.trim());
        match c.ping() {
            Err(e) => with_ui(&ui, move |a| a.set_status(SharedString::from(format!("Can't reach {host}. Is the Mac awake and on the same network? ({e})")))),
            Ok(_) => match c.get("/api/hub/state") {
                Err(e) => with_ui(&ui, move |a| a.set_status(SharedString::from(e.to_string()))),
                Ok(_) => { save_cfg("remote", &format!("{h}:{p}"), key.trim()); after_connect(&ui, &st, c, key.trim().to_string(), format!("Connected to {h}:{p}")); }
            },
        }
    });
}

// ---------------------------------------------------------------- polling
fn dur_text(sec: i64) -> String { if sec >= 86400 { format!("{}d {}h", sec / 86400, (sec % 86400) / 3600) } else if sec >= 3600 { format!("{}h {}m", sec / 3600, (sec % 3600) / 60) } else if sec >= 60 { format!("{}m", sec / 60) } else { format!("{sec}s") } }
fn card_of(e: &J, ip: &str) -> SrvCard {
    let state = gs(e, "state");
    let live = state == "online" || state == "starting";
    let players = arr(e, "players").len();
    let mem = gn(e, "mem_mb"); let up = gn(e, "uptime");
    let mut warns: Vec<String> = vec![];
    if matches!(e.get("exists"), Some(J::Bool(false))) { warns.push("Its folder is missing.".into()); }
    else if matches!(e.get("has_start"), Some(J::Bool(false))) && state != "online" { warns.push("No start.sh in its folder yet.".into()); }
    let clash: Vec<String> = arr(e, "clash").iter().filter_map(|x| x.as_str().map(|t| t.to_string())).collect();
    if !clash.is_empty() { warns.push(format!("Same Minecraft port as {}: run them one at a time or change a port.", clash.join(", "))); }
    if state == "panel_off" || state == "connecting" {
        let why = gs(e, "why");
        if !why.is_empty() { warns.push(why.to_string()); } else if state == "panel_off" { warns.push("Its panel is off. Start will turn it on.".into()); }
    }
    let sub = { let m = gs(e, "motd"); if m.is_empty() { gs(e, "name") } else { m } };
    SrvCard {
        id: gs(e, "id").into(), name: gs(e, "name").into(), state: state.into(), info: sub.into(), busy: false, tint: tint_of(&gs(e, "name")), letter: letter_of(&gs(e, "name")).into(),
        players: if live { players.to_string() } else { "-".into() }.into(),
        mem: if live && mem > 0 { format!("{:.1} GB", mem as f64 / 1024.0) } else { "-".into() }.into(),
        uptime: if live && up > 0 { dur_text(up) } else { "-".into() }.into(),
        join: format!("{}:{}", if ip.is_empty() { "this computer" } else { ip }, gn(e, "mc_port")).into(),
        warn1: warns.get(0).cloned().unwrap_or_default().into(), warn2: warns.get(1).cloned().unwrap_or_default().into(),
        warn1_bad: !warns.is_empty() && (warns[0].contains("missing") || warns[0].contains("start.sh")),
    }
}

/// Start/stop one server the same way the buttons do (turns its panel on first when needed).
fn run_action(c: &Client, id: &str, action: &str) -> Result<J, create_panel::client::Err> {
    if action == "kill" { return c.post(&sc(c, id, "/api/kill"), &obj(vec![])); }
    let mut r = c.post("/api/hub/action", &obj(vec![("id", s(id)), ("action", s(action))]));
    if action == "start" && msg_of(&r).contains("panel is stopped") {
        let _ = c.post("/api/hub/action", &obj(vec![("id", s(id)), ("action", s("panel-start"))]));
        for _ in 0..20 { thread::sleep(Duration::from_millis(500)); r = c.post("/api/hub/action", &obj(vec![("id", s(id)), ("action", s("start"))])); if !msg_of(&r).contains("Couldn't reach") && !msg_of(&r).contains("panel is stopped") { break; } }
    }
    r
}

fn apply_cards(a: &App, st: &Shared) {
    let all = st.lock().unwrap().cards_all.clone();
    let q = a.get_hub_search().to_string().trim().to_lowercase();
    let shown: Vec<SrvCard> = all.iter().filter(|c| q.is_empty() || c.name.to_lowercase().contains(&q) || c.info.to_lowercase().contains(&q)).cloned().collect();
    a.set_hub_total(all.len() as i32);
    let online = all.iter().filter(|c| c.state.as_str() == "online").count();
    let players: i32 = all.iter().filter_map(|c| c.players.parse::<i32>().ok()).sum();
    a.set_hub_sub(SharedString::from(format!("{} server{} · {} online · {} player{} right now", all.len(), if all.len() == 1 { "" } else { "s" }, online, players, if players == 1 { "" } else { "s" })));
    a.set_hub_online(SharedString::from(format!("{} of {}", online, all.len())));
    a.set_hub_online_sub(SharedString::from(if all.is_empty() { "Add your first server" } else if online == 0 { "Everything is off" } else if online == all.len() { "All running" } else { "Some are off" }));
    a.set_hub_players(SharedString::from(players.to_string()));
    a.set_cards(ModelRc::new(VecModel::from(shown)));
}

fn poll_loop(ui: Weak<App>, st: Shared) {
    let mut tick: u64 = 0;
    let mut wiz_checked = false;
    loop {
        thread::sleep(Duration::from_millis(1500));
        tick += 1;
        let Some(c) = client(&st) else { continue };
        match c.get("/api/hub/state") {
            Ok(j) => {
                let ip = j.get("hub").map(|h| arr(h, "ips")).and_then(|v| v.first().and_then(|x| x.as_str().map(|t| t.to_string()))).unwrap_or_default();
                let cards: Vec<SrvCard> = arr(&j, "servers").iter().map(|e| card_of(e, &ip)).collect();
                // who is on, and how much memory the running servers use (for the hub home tiles)
                let mut who: Vec<(String, String, Color, String)> = vec![];
                let mut mem_mb = 0i64;
                for e in arr(&j, "servers").iter() {
                    let live = matches!(gs(e, "state").as_str(), "online" | "starting");
                    if live { mem_mb += gn(e, "mem_mb"); }
                    let sname = gs(e, "name");
                    for p in arr(e, "players") { if let Some(n) = p.as_str() { who.push((n.to_string(), sname.clone(), tint_of(&sname), letter_of(n))); } }
                }
                let who_more = who.len().saturating_sub(6) as i32;
                who.truncate(6);
                let wmax = cards.iter().map(|c| (!c.warn1.is_empty()) as usize + (!c.warn2.is_empty()) as usize).max().unwrap_or(0);
                let ip = j.get("hub").map(|h| arr(h, "ips")).and_then(|v| v.first().and_then(|x| x.as_str().map(|t| t.to_string())));
                let host = j.get("hub").map(|h| gs(h, "host")).unwrap_or_default();
                let key = c.key.clone(); let local = c.host == "127.0.0.1";
                st.lock().unwrap().cards_all = cards;
                let st_c = st.clone();
                with_ui(&ui, move |a| {
                    apply_cards(a, &st_c);
                    a.set_hub_mem(SharedString::from(if mem_mb > 0 { format!("{:.1} GB", mem_mb as f64 / 1024.0) } else { "-".to_string() }));
                    a.set_hub_who_more(who_more);
                    a.set_hub_who(ModelRc::new(VecModel::from(who.into_iter().map(|w| HubWho { name: w.0.into(), server: w.1.into(), tint: w.2, letter: w.3.into() }).collect::<Vec<_>>())));
                    a.set_card_h(330.0 + 52.0 * wmax as f32);
                    if local { if let Some(ip) = ip { let alt = if host.is_empty() { String::new() } else { format!(" (or {host}:{}, which doesn't change)", port()) }; a.set_where_text(SharedString::from(format!("This computer · address for Windows: {ip}:{}{alt}", port()))); a.set_where_ip(SharedString::from(format!("{ip}:{}", port()))); a.set_where_alt(SharedString::from(if host.is_empty() { String::new() } else { format!("{host}:{}", port()) })); } } else { a.set_where_ip(SharedString::new()); a.set_where_alt(SharedString::new()); }
                    let _ = key;
                });
            }
            Err(e) => { let m = format!("Lost the connection: {e}"); with_ui(&ui, move |a| { if a.get_page().as_str() != "connect" { a.set_note(SharedString::from(m)); } }); continue; }
        }
        if !wiz_checked { wiz_checked = true; hubmisc::wizard_load(&ui, &st, false); }
        if tick % 20 == 1 { hubpage::last_backup(&ui, &c); }
        let (id, since) = { let g = st.lock().unwrap(); (g.sel.id.clone(), g.sel.since) };
        if id.is_empty() { continue; }
        if tick % 20 == 0 { extra::load_history(&ui, &st); people::load_stats(&ui, &st); extra::fp_fetch(&ui, &st); }
        if let Ok(j) = c.get(&sc(&c, &id, &format!("/api/status?since={since}"))) {
            let total = gn(&j, "total") as usize;
            let new: Vec<String> = arr(&j, "lines").iter().filter_map(|x| x.as_str().map(|t| t.to_string())).collect();
            let (lines, changed) = {
                let mut g = st.lock().unwrap();
                if g.sel.id != id { continue; }
                let ch = !new.is_empty() || total < g.sel.since;
                if total < g.sel.since { g.sel.lines.clear(); }
                g.sel.lines.extend(new);
                let len = g.sel.lines.len(); if len > 1500 { g.sel.lines.drain(..len - 1500); }
                g.sel.since = total;
                (g.sel.lines.clone(), ch)
            };
            let state = gs(&j, "state");
            let names: Vec<String> = arr(&j, "players").iter().filter_map(|x| x.as_str().map(|t| t.to_string()).or_else(|| { let n = gs(x, "name"); if n.is_empty() { None } else { Some(n) } })).collect();
            let pl = names.len();
            let ar = j.get("settings").map(|x| matches!(x.get("auto_restart"), Some(J::Bool(true)))).unwrap_or(false);
            let ip = gs(&j, "ip"); let port_s = gs(&j, "port");
            let mem = if gn(&j, "mem_mb") > 0 { format!("{:.1} of {:.0} GB", gn(&j, "mem_mb") as f64 / 1024.0, gn(&j, "max_mb") as f64 / 1024.0) } else { "-".into() };
            let frac = if gn(&j, "max_mb") > 0 { (gn(&j, "mem_mb") as f32 / gn(&j, "max_mb") as f32).clamp(0.0, 1.0) } else { 0.0 };
            let up = gn(&j, "uptime"); let uptime = if state == "online" && up > 0 { format!("{}h {}m", up / 3600, (up % 3600) / 60) } else { "-".into() };
            let maxp = { let v = gn(&j, "max_players"); if v > 0 { v } else { 20 } } as i32;
            let (tps_s, tps_lbl, tps_lvl) = match j.get("tps") { Some(J::Num(v)) => (format!("{:.1}", v), if *v >= 19.5 { "Smooth" } else if *v >= 17.0 { "Slightly slow" } else if *v >= 12.0 { "Laggy" } else { "Struggling" }.to_string(), if *v >= 19.0 { 0 } else if *v >= 15.0 { 1 } else { 2 }), _ => ("-".into(), if state == "online" { "Measuring..." } else { "Server is offline" }.to_string(), 3) };
            let tps_path = { let h: Vec<f64> = arr(&j, "tps_hist").iter().filter_map(|x| if let J::Num(v) = x { Some(*v) } else { None }).collect(); if h.len() < 2 { String::new() } else { let mut p = String::new(); for (i, v) in h.iter().enumerate() { p.push_str(&format!("{} {:.1} {:.1} ", if i == 0 { "M" } else { "L" }, i as f64 / (h.len() - 1) as f64 * 1000.0, 100.0 - (v / 20.0).clamp(0.0, 1.0) * 100.0)); } p } };
            let world = { let w = gn(&j, "world_mb"); if w <= 0 { "-".to_string() } else if w >= 1024 { format!("{:.1} GB", w as f64 / 1024.0) } else { format!("{w} MB") } };
            let free = { let f = gn(&j, "disk_free_mb"); if f <= 0 { String::new() } else if f >= 1024 { format!("{:.0} GB", f as f64 / 1024.0) } else { format!("{f} MB") } };
            people::prefetch(&ui, &st, names.clone());
            let (errs, fp_hint) = { let mut g = st.lock().unwrap(); let on = g.sel.tab == "console"; if g.x.con_seen > lines.len() || on { g.x.con_seen = lines.len(); }
                (lines[g.x.con_seen..].iter().filter(|l| extra::kind_of(l) == 3).count() as i32, changed && lines.iter().rev().take(20).any(|l| l.to_lowercase().contains("fingerprint"))) };
            if fp_hint { let (u2, s2) = (ui.clone(), st.clone()); thread::spawn(move || { thread::sleep(Duration::from_millis(800)); extra::fp_fetch(&u2, &s2); }); }
            with_ui(&ui, move |a| {
                a.set_con_errs(errs);
                a.set_server_state(SharedString::from(state));
                a.set_server_players(SharedString::from(if pl > 0 { format!("{pl} online") } else { String::new() }));
                a.set_auto_restart(ar);
                a.set_ov_ip(SharedString::from(ip)); a.set_ov_port(SharedString::from(port_s)); a.set_ov_mem(SharedString::from(mem)); a.set_ov_uptime(SharedString::from(uptime));
                a.set_online(ModelRc::new(VecModel::from(names.iter().map(|n| people::row(n)).collect::<Vec<_>>())));
                a.set_ov_mem_frac(frac);
                a.set_ov_maxp(maxp); a.set_ov_tps(SharedString::from(tps_s)); a.set_ov_tps_label(SharedString::from(tps_lbl)); a.set_ov_tps_lvl(tps_lvl); a.set_ov_tps_path(SharedString::from(tps_path)); a.set_ov_world(SharedString::from(world)); a.set_ov_free(SharedString::from(free));
                if changed { extra::set_console(a, &lines); extra::set_activity(a, &lines); }
            });
        }
    }
}

// ---------------------------------------------------------------- server pages
fn letter_of(name: &str) -> String { name.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_else(|| "?".into()) }
fn tint_of(name: &str) -> Color {
    const T: [u32; 8] = [0xd8c68a, 0x9bb86f, 0x7fc8a9, 0x7fb1d4, 0xb59be0, 0xe39aa8, 0xe3a97a, 0x8fd0d0];
    let h = name.bytes().fold(5381u32, |a, b| a.wrapping_mul(33).wrapping_add(b as u32));
    let c = T[(h % 8) as usize];
    Color::from_rgb_u8((c >> 16) as u8, (c >> 8) as u8, c as u8)
}
fn fmt_size(b: i64) -> String { if b >= 1 << 20 { format!("{:.1} MB", b as f64 / (1u64 << 20) as f64) } else { format!("{} KB", (b + 1023) / 1024) } }
fn mods_filtered(rows: &[ModRow], q: &str, chip: &str) -> Vec<ModRow> {
    let q = q.trim().to_lowercase();
    rows.iter().filter(|r| {
        let hit = q.is_empty() || r.name.to_lowercase().contains(&q) || r.file.to_lowercase().contains(&q);
        let c = match chip { "disabled" => !r.enabled, "client" => r.badge.contains("Client"), "update" => !r.update.is_empty(), _ => true };
        hit && c
    }).cloned().collect()
}
fn load_mods(ui: &Weak<App>, st: &Shared) {
    let (Some(c), id) = (client(st), st.lock().unwrap().sel.id.clone()) else { return };
    if id.is_empty() { return; }
    match c.get(&sc(&c, &id, "/api/mods")) {
        Ok(j) => {
            let rows: Vec<ModRow> = arr(&j, "mods").iter().map(|m| {
                let up = m.get("update").filter(|u| matches!(u, J::Obj(_))).map(|u| if matches!(u.get("manual"), Some(J::Bool(true))) { format!("update {} (manual)", gs(u, "to")) } else { format!("update to {}", gs(u, "to")) }).unwrap_or_default();
                let enabled = matches!(m.get("enabled"), Some(J::Bool(true)));
                let client_only = matches!(m.get("client_only"), Some(J::Bool(true)));
                let srv_only = matches!(m.get("server_only"), Some(J::Bool(true)));
                let badge = match (client_only, enabled) { (true, true) => "Client only · on", (true, false) => "Client only · off", (false, false) => "Disabled", _ => if srv_only { "Server only" } else { "" } };
                let name = gs(m, "name"); let name_s = name.clone(); let letter = name.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_else(|| "?".into());
                ModRow { file: gs(m, "file").into(), name: name.into(), version: gs(m, "version").into(), enabled, update: up.into(), badge: badge.into(), letter: letter.clone().into(), size: fmt_size(gn(m, "size")).into(), tint: tint_of(&name_s), sel: false }
            }).collect();
            let nf = gs(&j, "loader").to_lowercase().contains("neoforge");
            let ic: HashMap<String, String> = arr(&j, "mods").iter().filter_map(|m| { let u = gs(m, "icon"); if u.starts_with("https://") { Some((gs(m, "file"), u)) } else { None } }).collect();
            { let mut g = st.lock().unwrap(); g.x.icons = ic.clone(); g.x.modids = arr(&j, "mods").iter().map(|m| (gs(m, "file"), gs(m, "modid"))).collect(); g.mods = rows; let keep: HashSet<String> = g.mods.iter().map(|r| r.file.to_string()).collect(); g.x.sel.retain(|f| keep.contains(f)); }
            let st2 = st.clone();
            with_ui(ui, move |a| { a.set_ov_nf(nf); extra::render_mods(a, &st2); });
            icons::fetch(ui, st, ic.into_values().collect());
        }
        Err(e) => say(ui, &e.to_string()),
    }
}
fn load_backups(ui: &Weak<App>, st: &Shared) {
    let (Some(c), id) = (client(st), st.lock().unwrap().sel.id.clone()) else { return };
    if id.is_empty() { return; }
    if let Ok(j) = c.get(&sc(&c, &id, "/api/backups")) {
        let rows: Vec<BackupRow> = arr(&j, "backups").iter().map(|b| BackupRow { name: gs(b, "name").into(), size: mb(gn(b, "size")).into() }).collect();
        let note = format!("{} backups", rows.len());
        let newest = arr(&j, "backups").iter().map(|b| gn(b, "mtime")).max();
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
        let latest = newest.map(|t| extra::ago((now - t).max(0))).unwrap_or_else(|| "none yet".into());
        with_ui(ui, move |a| { a.set_backups(ModelRc::new(VecModel::from(rows))); a.set_mods_note(SharedString::from(note)); a.set_ov_backup(SharedString::from(latest)); });
    }
}
fn bg<F: FnOnce() + Send + 'static>(f: F) { thread::spawn(f); }

fn load_players(ui: &Weak<App>, st: &Shared) {
    let (Some(c), id) = (client(st), st.lock().unwrap().sel.id.clone()) else { return };
    if id.is_empty() { return; }
    let rows = |j: &J, k: &str| -> Vec<String> { arr(j, k).iter().filter_map(|x| x.as_str()).map(|n| n.to_string()).collect() };
    let (wl, ops, bn) = match c.get(&sc(&c, &id, "/api/lists")) { Ok(j) => (rows(&j, "whitelist"), rows(&j, "ops"), rows(&j, "banned")), Err(_) => return };
    let reasons: HashMap<String, String> = c.get(&sc(&c, &id, "/api/bans")).map(|j| arr(&j, "players").iter().map(|b| (gs(b, "name").to_lowercase(), gs(b, "reason"))).collect()).unwrap_or_default();
    let mut all: Vec<String> = wl.iter().chain(ops.iter()).chain(bn.iter()).cloned().collect(); all.sort(); all.dedup();
    with_ui(ui, move |a| { let m = |v: Vec<String>| ModelRc::new(VecModel::from(v.iter().map(|n| people::row(n)).collect::<Vec<_>>())); a.set_wl(m(wl)); a.set_ops(m(ops)); let mut br: Vec<PlayerRow> = bn.iter().map(|n| people::row(n)).collect(); for r in br.iter_mut() { if let Some(x) = reasons.get(&r.name.to_lowercase()) { r.note = SharedString::from(x.as_str()); } } a.set_banned(ModelRc::new(VecModel::from(br))); });
    people::prefetch(ui, st, all);
    people::load_stats(ui, st);
}

fn jstr(j: Option<&J>) -> String { match j { Some(J::Str(t)) => t.clone(), Some(J::Num(x)) => if x.fract() == 0.0 { format!("{}", *x as i64) } else { x.to_string() }, Some(J::Bool(b)) => b.to_string(), _ => String::new() } }
fn fd_of(f: &J) -> FD {
    let kind = match gs(f, "type").as_str() { "bool" => 1, "select" => 2, _ => 0 };
    let options = match f.get("options") { Some(J::Arr(o)) => o.iter().map(|x| x.as_str().map(|t| t.to_string()).unwrap_or_else(|| jstr(x.get("value")))).collect(), _ => vec![] };
    FD { key: gs(f, "key"), label: { let l = gs(f, "label"); if l.is_empty() { gs(f, "key") } else { l } }, help: gs(f, "help"), kind, value: jstr(f.get("value")), options }
}
fn load_settings(ui: &Weak<App>, st: &Shared) {
    let (Some(c), id) = (client(st), st.lock().unwrap().sel.id.clone()) else { return };
    if id.is_empty() { return; }
    let Ok(j) = c.get(&sc(&c, &id, "/api/config")) else { return };
    let mut groups: Vec<(String, Vec<FD>)> = arr(&j, "groups").iter().map(|g| (gs(g, "name"), arr(g, "fields").iter().map(fd_of).collect())).collect();
    let other: Vec<FD> = arr(&j, "other").iter().filter(|f| !matches!(f.get("secret"), Some(J::Bool(true)))).map(|f| FD { key: gs(f, "key"), label: gs(f, "key"), help: String::new(), kind: 0, value: jstr(f.get("value")), options: vec![] }).collect();
    if !other.is_empty() { groups.push(("Other".into(), other)); }
    { let mut g = st.lock().unwrap(); g.fdata = groups; g.edits.clear(); if g.open.is_empty() { if let Some(f) = g.fdata.first() { let n = f.0.clone(); g.open.insert(n); } } }
    let st = st.clone();
    with_ui(ui, move |a| { a.set_settings_dirty(false); render_fields(a, &st); });
}
fn render_fields(a: &App, st: &Shared) {
    let q = a.get_settings_search().to_string().trim().to_lowercase();
    let mut g = st.lock().unwrap();
    // one category open at a time (the left list picks it); a search shows every match
    if !g.fdata.iter().any(|x| g.open.contains(&x.0)) { if let Some(f) = g.fdata.first() { let n = f.0.clone(); g.open.clear(); g.open.insert(n); } }
    let cur = g.fdata.iter().find(|x| g.open.contains(&x.0)).map(|x| x.0.clone()).unwrap_or_default();
    let mut out: Vec<Field> = vec![];
    let mut nav: Vec<Field> = vec![];
    for (name, fs_) in &g.fdata {
        nav.push(Field { key: SharedString::new(), label: name.as_str().into(), help: SharedString::new(), kind: 9, value: SharedString::new(), options: ModelRc::default(), count: fs_.len() as i32, open: *name == cur });
        let shown: Vec<&FD> = fs_.iter().filter(|f| q.is_empty() || f.key.to_lowercase().contains(&q) || f.label.to_lowercase().contains(&q) || f.help.to_lowercase().contains(&q)).collect();
        if shown.is_empty() { continue; }
        let open = !q.is_empty() || *name == cur;
        if !open { continue; }
        if !q.is_empty() { out.push(Field { key: SharedString::new(), label: name.as_str().into(), help: SharedString::new(), kind: 10, value: SharedString::new(), options: ModelRc::default(), count: shown.len() as i32, open: true }); }
        for f in shown {
            let v = g.edits.get(&f.key).cloned().unwrap_or_else(|| f.value.clone());
            out.push(Field { key: f.key.as_str().into(), label: f.label.as_str().into(), help: f.help.as_str().into(), kind: f.kind, value: v.into(), options: ModelRc::new(VecModel::from(f.options.iter().map(|o| SharedString::from(o.as_str())).collect::<Vec<_>>())), count: 0, open: false });
        }
    }
    let title = if q.is_empty() { cur.clone() } else { "Search results".to_string() };
    let count = g.fdata.iter().find(|x| x.0 == cur).map(|x| x.1.len()).unwrap_or(0);
    drop(g);
    a.set_group_title(SharedString::from(title));
    a.set_group_count(count as i32);
    a.set_groups(ModelRc::new(VecModel::from(nav)));
    a.set_fields_h(rows_h(&out));
    a.set_fields(ModelRc::new(VecModel::from(out)));
}
/// Total height of a FieldList (row heights depend on the row kind).
pub fn rows_h(v: &[Field]) -> f32 { v.iter().map(|f| match f.kind { 9 | 6 => 58.0, 10 => 62.0, _ => 68.0 }).sum::<f32>() + 8.0 }
fn load_overview(ui: &Weak<App>, st: &Shared) { load_mods(ui, st); load_backups(ui, st); }

fn wire(app: &App, st: Shared) {
    let ui = app.as_weak();
    { let (u, s_) = (ui.clone(), st.clone()); app.on_connect_local(move || { u.upgrade().map(|a| a.set_status(SharedString::from("Starting..."))); connect_local(u.clone(), s_.clone()) }); }
    { let (u, s_) = (ui.clone(), st.clone()); app.on_connect_remote(move |h, k| { u.upgrade().map(|a| a.set_status(SharedString::from("Connecting..."))); connect_remote(u.clone(), s_.clone(), h.to_string(), k.to_string()) }); }
    { let (u, s_) = (ui.clone(), st.clone()); app.on_disconnect(move || { let mut g = s_.lock().unwrap(); g.client = None; g.sel = Sel::default(); drop(g); save_cfg("", "", ""); u.upgrade().map(|a| { a.set_connected(false); a.set_page(SharedString::from("connect")); }); }); }
    { let (u, s_) = (ui.clone(), st.clone()); app.on_hub_refilter(move || { if let Some(a) = u.upgrade() { apply_cards(&a, &s_); } }); }
    { let (u, s_) = (ui.clone(), st.clone()); app.on_all_action(move |what| {
        let what = what.to_string(); let (u, s_) = (u.clone(), s_.clone());
        bg(move || {
            let Some(c) = client(&s_) else { return };
            let Ok(j) = c.get("/api/hub/state") else { return };
            let mut done = 0;
            for e in arr(&j, "servers") {
                let (id, state) = (gs(&e, "id"), gs(&e, "state"));
                let go = if what == "start" { state == "stopped" || state == "panel_off" } else { state == "online" || state == "starting" };
                if !go { continue; }
                let r = run_action(&c, &id, &what); if ok_of(&r) { done += 1; }
            }
            say(&u, &if done == 0 { format!("Nothing to {what}.") } else { String::new() });
        });
    }); }
    { let (u, s_) = (ui.clone(), st.clone()); app.on_server_action(move |id, action| {
        let (id, action) = (id.to_string(), action.to_string()); let (u, s_) = (u.clone(), s_.clone());
        bg(move || {
            let Some(c) = client(&s_) else { return };
            let r = run_action(&c, &id, &action);
            say(&u, &if ok_of(&r) { String::new() } else { msg_of(&r) });
        });
    }); }
    { let (u, s_) = (ui.clone(), st.clone()); app.on_open_server(move |id| {
        let id = id.to_string();
        let name = u.upgrade().map(|a| a.get_cards().iter().find(|c| c.id.as_str() == id).map(|c| c.name.to_string()).unwrap_or_default()).unwrap_or_default();
        { let mut g = s_.lock().unwrap(); g.sel = Sel { id: id.clone(), tab: "overview".into(), since: 0, lines: vec![] }; }
        if let Some(a) = u.upgrade() { a.set_server_id(SharedString::from(id)); a.set_fp(SharedString::new()); a.set_con_errs(0); a.set_con_select(false); a.set_server_letter(SharedString::from(letter_of(&name))); a.set_server_tint(tint_of(&name)); a.set_server_name(SharedString::from(name)); a.set_console_text(SharedString::new()); a.set_server_state(SharedString::from("stopped")); a.set_tab(SharedString::from("overview")); a.set_note(SharedString::new()); a.set_page(SharedString::from("server")); a.set_online(ModelRc::default()); a.set_mods(ModelRc::default()); a.set_ov_mods(SharedString::from("0")); a.set_ov_backup(SharedString::from("none yet")); a.set_mod_search(SharedString::new()); a.set_mod_chip(SharedString::from("all")); }
        { let mut g = s_.lock().unwrap(); g.fdata.clear(); g.edits.clear(); g.open.clear(); g.mods.clear(); g.x = extra::Extra::default(); }
        if let Some(a) = u.upgrade() { a.set_con(ModelRc::default()); a.set_act(ModelRc::default()); a.set_files(ModelRc::default()); a.set_br(ModelRc::default()); a.set_f_edit_open(false); a.set_mods_tab(SharedString::from("mods")); a.set_mods_view(SharedString::from("installed")); a.set_set_tab(SharedString::from("server")); a.set_nf_open(false); a.set_ov_public(SharedString::from("Looking up...")); a.set_g_mem(SharedString::new()); a.set_g_ply(SharedString::new()); a.set_g_range(SharedString::from("6h")); a.set_g_title(SharedString::from("Last 6 hours")); a.set_mods_msg(SharedString::new()); a.set_f_msg(SharedString::new()); a.set_cmds_open(false); a.set_more_open(false); a.set_ov_nf(false); a.set_sel_count(0); }
        let (u2, s2) = (u.clone(), s_.clone()); bg(move || { load_overview(&u2, &s2); extra::load_extras(&u2, &s2); });
    }); }
    { let (u, s_) = (ui.clone(), st.clone()); app.on_close_server(move || { s_.lock().unwrap().sel = Sel::default(); if let Some(a) = u.upgrade() { a.set_note(SharedString::new()); a.set_page(SharedString::from("hub")); } }); }
    { let (u, s_) = (ui.clone(), st.clone()); app.on_send_cmd(move |t| {
        let t = t.trim().to_string(); if t.is_empty() { return; } let (u, s_) = (u.clone(), s_.clone());
        bg(move || { let (Some(c), id) = (client(&s_), s_.lock().unwrap().sel.id.clone()) else { return }; let r = c.post(&sc(&c, &id, "/api/command"), &obj(vec![("cmd", s(&t))])); if !ok_of(&r) { say(&u, &msg_of(&r)); } });
    }); }
    { let (u, s_) = (ui.clone(), st.clone()); app.on_show_tab(move |t| {
        let t = t.to_string(); s_.lock().unwrap().sel.tab = t.clone();
        if let Some(a) = u.upgrade() { a.set_tab(SharedString::from(t.clone())); a.set_note(SharedString::new()); }
        let (is_auto, is_dp) = u.upgrade().map(|a| (a.get_set_tab().as_str() == "auto", a.get_mods_tab().as_str() == "dp")).unwrap_or((false, false));
        let (u, s_) = (u.clone(), s_.clone());
        bg(move || match t.as_str() {
            "mods" => { load_mods(&u, &s_); if is_dp { extra::load_dp(&u, &s_); } }
            "files" => { let p = s_.lock().unwrap().x.f_path.clone(); extra::files_load(&u, &s_, &p); }
            "backups" => load_backups(&u, &s_), "players" => load_players(&u, &s_),
            "settings" => { if is_auto { extra::load_auto(&u, &s_); } else { load_settings(&u, &s_); } }
            "tools" => tools::load(&u, &s_),
            "overview" => { load_overview(&u, &s_); extra::load_extras(&u, &s_); }
            _ => {}
        });
    }); }
    { let (u, s_) = (ui.clone(), st.clone()); app.on_mod_toggle(move |f, on| {
        let (f, u, s_) = (f.to_string(), u.clone(), s_.clone());
        bg(move || { let (Some(c), id) = (client(&s_), s_.lock().unwrap().sel.id.clone()) else { return }; let r = c.post(&sc(&c, &id, "/api/mods/toggle"), &obj(vec![("files", J::Arr(vec![s(&f)])), ("enabled", J::Bool(on))])); say(&u, &msg_of(&r)); load_mods(&u, &s_); });
    }); }
    { let (u, s_) = (ui.clone(), st.clone()); app.on_mod_remove(move |f| {
        let (f, u, s_) = (f.to_string(), u.clone(), s_.clone());
        bg(move || { let (Some(c), id) = (client(&s_), s_.lock().unwrap().sel.id.clone()) else { return }; let r = c.post(&sc(&c, &id, "/api/mods/delete"), &obj(vec![("files", J::Arr(vec![s(&f)]))])); say(&u, &(msg_of(&r) + " (kept in mods-trash)")); load_mods(&u, &s_); });
    }); }
    { let (u, s_) = (ui.clone(), st.clone()); app.on_mods_check(move || {
        let (u, s_) = (u.clone(), s_.clone());
        bg(move || {
            let (Some(c), id) = (client(&s_), s_.lock().unwrap().sel.id.clone()) else { return };
            let r = c.post(&sc(&c, &id, "/api/mods/update/check"), &obj(vec![]));
            if !ok_of(&r) { say(&u, &msg_of(&r)); return; }
            say(&u, "Checking Modrinth for updates...");
            for _ in 0..240 { thread::sleep(Duration::from_secs(1)); match c.get(&sc(&c, &id, "/api/mods/update/status")) { Ok(j) if gs(&j, "phase") == "checking" => { let m = gs(&j, "msg"); say(&u, &m); } Ok(j) => { if gs(&j, "phase") == "error" { say(&u, &gs(&j, "msg")); } else { say(&u, ""); } break; } Err(_) => break } }
            load_mods(&u, &s_);
        });
    }); }
    { let (u, s_) = (ui.clone(), st.clone()); app.on_mods_update_all(move || {
        let (u, s_) = (u.clone(), s_.clone());
        bg(move || {
            let (Some(c), id) = (client(&s_), s_.lock().unwrap().sel.id.clone()) else { return };
            let files: Vec<J> = match c.get(&sc(&c, &id, "/api/mods")) { Ok(j) => arr(&j, "mods").iter().filter(|m| m.get("update").map(|x| matches!(x, J::Obj(_)) && !matches!(x.get("manual"), Some(J::Bool(true)))).unwrap_or(false)).map(|m| s(&gs(m, "file"))).collect(), Err(_) => vec![] };
            if files.is_empty() { say(&u, "Nothing to update. Press Check for updates first."); return; }
            let r = c.post(&sc(&c, &id, "/api/mods/update/apply"), &obj(vec![("files", J::Arr(files))]));
            if !ok_of(&r) { say(&u, &msg_of(&r)); return; }
            for _ in 0..600 { thread::sleep(Duration::from_secs(1)); if let Ok(j) = c.get(&sc(&c, &id, "/api/mods/update/status")) { if gs(&j, "phase") == "installing" { say(&u, &gs(&j, "msg")); continue; } let res = j.get("result").cloned().unwrap_or(J::Null); say(&u, &format!("Updated {}, {} skipped. Old files are in mods-trash. Restart the server to use them.", arr(&res, "ok").len(), arr(&res, "failed").len())); break; } }
            load_mods(&u, &s_);
        });
    }); }
    { let (u, s_) = (ui.clone(), st.clone()); app.on_backup_now(move || {
        let (u, s_) = (u.clone(), s_.clone());
        bg(move || { let (Some(c), id) = (client(&s_), s_.lock().unwrap().sel.id.clone()) else { return }; let r = c.post(&sc(&c, &id, "/api/backup"), &obj(vec![])); say(&u, &msg_of(&r)); for _ in 0..600 { thread::sleep(Duration::from_secs(2)); if let Ok(j) = c.get(&sc(&c, &id, "/api/status?since=2000000000")) { let b = j.get("backup").cloned().unwrap_or(J::Null); let (pct, m) = (gn(&b, "pct") as i32, gs(&b, "msg")); with_ui(&u, move |a| { a.set_bk_pct(pct); a.set_bk_msg(SharedString::from(m)); }); if !matches!(b.get("running"), Some(J::Bool(true))) { break; } } } with_ui(&u, |a| a.set_bk_pct(0)); load_backups(&u, &s_); say(&u, ""); });
    }); }
    { let (u, s_) = (ui.clone(), st.clone()); app.on_backup_delete(move |nm| {
        let (nm, u, s_) = (nm.to_string(), u.clone(), s_.clone());
        bg(move || { let (Some(c), id) = (client(&s_), s_.lock().unwrap().sel.id.clone()) else { return }; let r = c.post(&sc(&c, &id, "/api/backup/delete"), &obj(vec![("name", s(&nm))])); say(&u, &msg_of(&r)); load_backups(&u, &s_); });
    }); }
    { let (u, s_) = (ui.clone(), st.clone()); app.on_set_auto_restart(move |on| {
        let (u, s_) = (u.clone(), s_.clone());
        bg(move || { let (Some(c), id) = (client(&s_), s_.lock().unwrap().sel.id.clone()) else { return }; let r = c.post(&sc(&c, &id, "/api/settings"), &obj(vec![("auto_restart", J::Bool(on))])); if !ok_of(&r) { say(&u, &msg_of(&r)); } });
    }); }
    { let (u, s_) = (ui.clone(), st.clone()); app.on_rename_server(move |id, nm| {
        let (id, nm, u, s_) = (id.to_string(), nm.to_string(), u.clone(), s_.clone());
        bg(move || { let Some(c) = client(&s_) else { return }; let r = c.post("/api/hub/rename", &obj(vec![("id", s(&id)), ("name", s(&nm))])); let ok = ok_of(&r); say(&u, &msg_of(&r)); if ok { with_ui(&u, move |a| a.set_server_name(SharedString::from(nm))); } });
    }); }
    { let (u, s_) = (ui.clone(), st.clone()); app.on_remove_server(move |id| {
        let (id, u, s_) = (id.to_string(), u.clone(), s_.clone());
        bg(move || { let Some(c) = client(&s_) else { return }; let r = c.post("/api/hub/remove", &obj(vec![("id", s(&id)), ("delete_files", J::Bool(false))])); let ok = ok_of(&r); say(&u, &msg_of(&r)); if ok { s_.lock().unwrap().sel = Sel::default(); with_ui(&u, |a| a.set_page(SharedString::from("hub"))); } });
    }); }
    { let (u, s_) = (ui.clone(), st.clone()); app.on_create_server(move |name, li, mc, ram| {
        let (name, mc, ram, li, u, s_) = (name.to_string(), mc.to_string(), ram.to_string(), li as usize, u.clone(), s_.clone());
        if name.trim().is_empty() { say(&u, ""); with_ui(&u, |a| a.set_job_text(SharedString::from("Give the server a name."))); return; }
        with_ui(&u, |a| { a.set_job_busy(true); a.set_job_text(SharedString::from("Starting...")); });
        bg(move || {
            let Some(c) = client(&s_) else { return };
            let r = c.post("/api/hub/create", &obj(vec![("name", s(&name)), ("loader", s(LOADERS.get(li).copied().unwrap_or("neoforge"))), ("mc", s(mc.trim())), ("ram", n(ram.trim().parse::<i64>().unwrap_or(6))), ("eula", J::Bool(true))]));
            let job = match &r { Ok(j) if ok_of(&r) => gs(j, "job"), _ => { let m = msg_of(&r); with_ui(&u, move |a| { a.set_job_busy(false); a.set_job_text(SharedString::from(m)); }); return; } };
            loop {
                thread::sleep(Duration::from_secs(1));
                let Ok(j) = c.get(&format!("/api/hub/job?id={job}")) else { continue };
                let done = matches!(j.get("done"), Some(J::Bool(true))); let ok = matches!(j.get("ok"), Some(J::Bool(true)));
                let text = if done { gs(&j, "msg") } else { format!("{} ({}%)", gs(&j, "step"), gn(&j, "pct")) };
                with_ui(&u, move |a| { a.set_job_text(SharedString::from(text)); if done { a.set_job_busy(false); if ok { a.set_page(SharedString::from("hub")); a.set_note(SharedString::from("Server created. Press Start.")); } } });
                if done { break; }
            }
        });
    }); }
    { let (u, s_) = (ui.clone(), st.clone()); app.on_import_detect(move || {
        let (u, s_) = (u.clone(), s_.clone());
        bg(move || { let Some(c) = client(&s_) else { return }; let t = match c.get("/api/hub/import/detect") { Ok(j) => { let list = arr(&j, "servers"); if matches!(j.get("found"), Some(J::Bool(true))) { format!("Found {} server{} in the old panel: {}.{}", list.len(), if list.len() == 1 { "" } else { "s" }, list.iter().map(|x| gs(x, "name")).collect::<Vec<_>>().join(", "), if matches!(j.get("done"), Some(J::Bool(true))) { " (already imported)" } else { "" }) } else { "I couldn't find an old panel on this computer. (Open the app on the computer where the old panel lived.)".to_string() } } Err(e) => e.to_string() }; with_ui(&u, move |a| a.set_import_text(SharedString::from(t))); });
    }); }
    { let (u, s_) = (ui.clone(), st.clone()); app.on_import_run(move || {
        let (u, s_) = (u.clone(), s_.clone());
        bg(move || {
            let Some(c) = client(&s_) else { return };
            let r = c.post("/api/hub/import/run", &obj(vec![("confirm", J::Bool(true)), ("stop_old", J::Bool(true))]));
            if !ok_of(&r) { let m = msg_of(&r); with_ui(&u, move |a| a.set_import_text(SharedString::from(m))); return; }
            loop { thread::sleep(Duration::from_secs(1)); let Ok(j) = c.get("/api/hub/import/status") else { continue }; let st_ = gs(&j, "state"); let log: Vec<String> = arr(&j, "log").iter().filter_map(|x| x.as_str().map(|t| t.to_string())).collect(); let t = log.join("\n"); with_ui(&u, move |a| a.set_import_text(SharedString::from(t))); if st_ != "running" { break; } }
        });
    }); }
    { let (u, s_) = (ui.clone(), st.clone()); app.on_quick(move |what| {
        let what = what.to_string(); let (u, s_) = (u.clone(), s_.clone());
        bg(move || {
            let (Some(c), id) = (client(&s_), s_.lock().unwrap().sel.id.clone()) else { return };
            if what == "backup" { let r = c.post(&sc(&c, &id, "/api/backup"), &obj(vec![])); say(&u, &msg_of(&r)); for _ in 0..600 { thread::sleep(Duration::from_secs(2)); if let Ok(j) = c.get(&sc(&c, &id, "/api/status?since=2000000000")) { if !matches!(j.get("backup").and_then(|b| b.get("running")), Some(J::Bool(true))) { break; } } } load_backups(&u, &s_); say(&u, "Backup finished."); return; }
            let cmd = match what.as_str() { "save" => "save-all", "day" => "time set day", "night" => "time set night", "clear" => "weather clear", _ => return };
            let r = c.post(&sc(&c, &id, "/api/command"), &obj(vec![("cmd", s(cmd))]));
            say(&u, &if ok_of(&r) { format!("Sent: {cmd}") } else { msg_of(&r) });
        });
    }); }
    { let (u, s_) = (ui.clone(), st.clone()); app.on_broadcast(move |t| {
        let t = t.trim().to_string(); if t.is_empty() { return; } let (u, s_) = (u.clone(), s_.clone());
        bg(move || { let (Some(c), id) = (client(&s_), s_.lock().unwrap().sel.id.clone()) else { return }; let r = c.post(&sc(&c, &id, "/api/command"), &obj(vec![("cmd", s(&format!("say {t}")))])); say(&u, &if ok_of(&r) { "Sent.".into() } else { msg_of(&r) }); });
    }); }
    { let (u, s_) = (ui.clone(), st.clone()); app.on_player_do(move |what, name| {
        let (what, name) = (what.to_string(), name.trim().to_string()); if name.is_empty() { return; }
        let reason = u.upgrade().map(|a| a.get_ban_reason().to_string()).unwrap_or_default(); let (u, s_) = (u.clone(), s_.clone());
        bg(move || {
            let (Some(c), id) = (client(&s_), s_.lock().unwrap().sel.id.clone()) else { return };
            let r = match what.as_str() {
                "ban" => c.post(&sc(&c, &id, "/api/bans/ban"), &obj(vec![("name", s(&name)), ("reason", s(&reason))])),
                "pardon" => c.post(&sc(&c, &id, "/api/bans/unban"), &obj(vec![("name", s(&name))])),
                "op" => c.post(&sc(&c, &id, "/api/command"), &obj(vec![("cmd", s(&format!("op {name}")))])),
                "deop" => c.post(&sc(&c, &id, "/api/command"), &obj(vec![("cmd", s(&format!("deop {name}")))])),
                "kick" => c.post(&sc(&c, &id, "/api/command"), &obj(vec![("cmd", s(&format!("kick {name}")))])),
                "unwhitelist" => c.post(&sc(&c, &id, "/api/command"), &obj(vec![("cmd", s(&format!("whitelist remove {name}")))])),
                _ => c.post(&sc(&c, &id, "/api/command"), &obj(vec![("cmd", s(&format!("whitelist add {name}")))])),
            };
            say(&u, &if ok_of(&r) { format!("Done: {what} {name}") } else { msg_of(&r) });
            thread::sleep(Duration::from_millis(800)); load_players(&u, &s_);
        });
    }); }
    { let (u, s_) = (ui.clone(), st.clone()); app.on_field_edited(move |k, v| {
        { let mut g = s_.lock().unwrap(); let orig = g.fdata.iter().flat_map(|x| x.1.iter()).find(|f| f.key == k.as_str()).map(|f| f.value.clone()); if orig.as_deref() == Some(v.as_str()) { g.edits.remove(k.as_str()); } else { g.edits.insert(k.to_string(), v.to_string()); } let dirty = !g.edits.is_empty(); drop(g); if let Some(a) = u.upgrade() { a.set_settings_dirty(dirty); } }
    }); }
    { let (u, s_) = (ui.clone(), st.clone()); app.on_group_toggle(move |name| {
        { let mut g = s_.lock().unwrap(); g.open.clear(); g.open.insert(name.to_string()); }
        if let Some(a) = u.upgrade() { a.set_settings_search(SharedString::new()); render_fields(&a, &s_); }
    }); }
    { let (u, s_) = (ui.clone(), st.clone()); app.on_settings_refilter(move || { if let Some(a) = u.upgrade() { render_fields(&a, &s_); } }); }
    { let (u, s_) = (ui.clone(), st.clone()); app.on_settings_discard(move || { s_.lock().unwrap().edits.clear(); if let Some(a) = u.upgrade() { a.set_settings_dirty(false); render_fields(&a, &s_); } }); }
    { let (u, s_) = (ui.clone(), st.clone()); app.on_settings_save(move || {
        let (u, s_) = (u.clone(), s_.clone());
        let edits: Vec<(String, String)> = s_.lock().unwrap().edits.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        if edits.is_empty() { return; }
        bg(move || {
            let (Some(c), id) = (client(&s_), s_.lock().unwrap().sel.id.clone()) else { return };
            let props = J::Obj(edits.iter().map(|(k, v)| (k.clone(), s(v))).collect());
            let r = c.post(&sc(&c, &id, "/api/config"), &obj(vec![("props", props)]));
            say(&u, &if ok_of(&r) { "Saved. Restart the server for most changes to apply.".to_string() } else { msg_of(&r) });
            if ok_of(&r) { load_settings(&u, &s_); }
        });
    }); }
    extra::wire(app, &ui, &st);
    dc::wire(app, &ui, &st);
    hubpage::wire(app, &ui, &st);
    hubmisc::wire(app, &ui, &st);
    loaderpick::wire(app, &ui, &st);
    theming::wire(app, &ui, &st);
    hubmisc::wire_wizard(app, &ui, &st);
    hubmisc::wire_engine(app, &ui, &st);
    appupd::wire(app, &ui, &st);
    { let (u, s_) = (ui.clone(), st.clone()); app.on_backup_download(move |nm| {
        let (nm, u, s_) = (nm.to_string(), u.clone(), s_.clone());
        bg(move || { let (Some(c), id) = (client(&s_), s_.lock().unwrap().sel.id.clone()) else { return }; say(&u, "Downloading the backup...");
            match c.raw("GET", &sc(&c, &id, &format!("/api/backup/download?name={}", extra::enc(&nm))), None) { Ok((200, body)) => { let dest = extra::downloads_dir().join(&nm); match std::fs::write(&dest, body) { Ok(_) => say(&u, &format!("Saved to {}", dest.display())), Err(e) => say(&u, &format!("Couldn't save it: {e}")) } } _ => say(&u, "Couldn't download that backup.") } });
    }); }
    { let (u, s_) = (ui.clone(), st.clone()); app.on_tools_act(move |k| tools::act(&u, &s_, k.to_string())); }
    { let s_ = st.clone(); app.on_tools_edit(move |k, v| tools::edit(&s_, k.as_str(), v.as_str())); }
    { let (u, s_) = (ui.clone(), st.clone()); app.on_set_palette(move |p| {
        let p = p.to_string(); if let Some(a) = u.upgrade() { apply_palette(&a, &p); }
        let s_ = s_.clone(); bg(move || { if let Some(c) = client(&s_) { let _ = c.post("/api/theme", &obj(vec![("palette", s(&p))])); } });
    }); }
    { let (u, s_) = (ui.clone(), st.clone()); app.on_set_engine_autostart(move |on| {
        let (u, s_) = (u.clone(), s_.clone());
        bg(move || { let Some(c) = client(&s_) else { return }; let r = c.post("/api/autostart", &obj(vec![("enable", J::Bool(on))])); say(&u, &msg_of(&r)); });
    }); }
}

fn main() {
    // keeps the version marker in this binary so an engine update can tell which app build is installed
    std::hint::black_box(create_panel::update::MARKER);
    appupd::cleanup();
    let app = App::new().unwrap();
    let st: Shared = Arc::new(Mutex::new(St::default()));
    { let mk = |s: &[(&str, u32)]| ModelRc::new(VecModel::from(s.iter().map(|p| Pal { name: p.0.into(), color: Color::from_rgb_u8((p.1 >> 16) as u8, (p.1 >> 8) as u8, p.1 as u8) }).collect::<Vec<_>>())); app.set_pals(mk(&PALETTES[..10])); app.set_pals2(mk(&PALETTES[10..])); }
    wire(&app, st.clone());
    { let (u, s_) = (app.as_weak(), st.clone()); thread::spawn(move || poll_loop(u, s_)); }
    // reconnect to what was used last time
    let cfg = load_cfg();
    match gs(&cfg, "mode").as_str() {
        "local" => connect_local(app.as_weak(), st.clone()),
        "remote" => { app.set_remote_host(SharedString::from(gs(&cfg, "host"))); app.set_remote_key(SharedString::from(gs(&cfg, "key"))); connect_remote(app.as_weak(), st.clone(), gs(&cfg, "host"), gs(&cfg, "key")); }
        _ => {}
    }
    app.run().unwrap();
}
