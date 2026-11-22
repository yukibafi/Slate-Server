// Everything beyond the basics: console filters, activity, graph, mod browser, datapacks, files,
// automation settings. Same rule as main.rs: the engine does the work, this just shows it.
use crate::*;
use std::io::Write;

#[derive(Default)]
pub struct Extra {
    pub sel: HashSet<String>,
    pub sort: usize,
    pub range: i64,
    pub f_path: String,
    pub f_edit_path: String,
    pub f_edit_mtime: String,
    pub f_edit_crlf: bool,
    pub auto: Option<J>,
    pub modupd: Option<J>,
    pub auto_open: HashSet<String>,
    pub icons: HashMap<String, String>,
    pub br_icons: HashMap<String, String>,
    pub tools_path: String,
    pub pk_vers: Vec<String>,
    pub cmp: Vec<(String, String, String)>,
    pub cats: Vec<(String, String)>,
    pub cats_src: String,
    pub lp_mcs: Vec<String>,
    pub lp_builds: Vec<String>,
    pub lp_cur: String,
    pub lp_loader: String,
    pub lp_name: String,
    pub fp_now: Option<String>,
    pub f_items: Vec<J>,
    pub modids: HashMap<String, String>,
    pub f_filter: String,
    pub con_seen: usize,
}

// ---------------------------------------------------------------- small helpers
pub fn enc(t: &str) -> String {
    let mut o = String::new();
    for b in t.bytes() { if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) { o.push(b as char); } else { o.push_str(&format!("%{:02X}", b)); } }
    o
}
fn home() -> std::path::PathBuf { std::path::PathBuf::from(std::env::var("HOME").or_else(|_| std::env::var("USERPROFILE")).unwrap_or_else(|_| ".".into())) }
fn now_s() -> i64 { std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0) }
pub fn ago(secs: i64) -> String {
    if secs < 90 { "just now".into() } else if secs < 3600 * 2 { format!("{} min ago", secs / 60) } else if secs < 86400 * 2 { format!("{} h ago", secs / 3600) } else { format!("{} d ago", secs / 86400) }
}
fn human(nn: i64) -> String { if nn >= 1_000_000 { format!("{:.1}M", nn as f64 / 1e6) } else if nn >= 1000 { format!("{:.1}K", nn as f64 / 1e3) } else { nn.to_string() } }
fn fsize(b: i64) -> String { if b >= 1 << 30 { format!("{:.1} GB", b as f64 / (1u64 << 30) as f64) } else if b >= 1 << 20 { format!("{:.1} MB", b as f64 / (1u64 << 20) as f64) } else if b >= 1024 { format!("{} KB", b / 1024) } else { format!("{} B", b) } }
fn num(j: &J) -> Option<f64> { if let J::Num(x) = j { Some(*x) } else { None } }
fn sid(st: &Shared) -> String { st.lock().unwrap().sel.id.clone() }
fn flag(j: &J, k: &str) -> bool { matches!(j.get(k), Some(J::Bool(true))) }

pub fn copy_clip(text: &str) {
    #[cfg(target_os = "macos")] let (cmd, args): (&str, Vec<&str>) = ("pbcopy", vec![]);
    #[cfg(windows)] let (cmd, args): (&str, Vec<&str>) = ("clip", vec![]);
    #[cfg(all(unix, not(target_os = "macos")))] let (cmd, args): (&str, Vec<&str>) = ("xclip", vec!["-selection", "clipboard"]);
    if let Ok(mut ch) = std::process::Command::new(cmd).args(args).stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn() {
        if let Some(mut i) = ch.stdin.take() { let _ = i.write_all(text.as_bytes()); }
        let _ = ch.wait();
    }
}

/// Native "choose file(s)" window using what the OS already has (no extra libraries).
pub fn pick_files() -> Vec<String> {
    #[cfg(target_os = "macos")]
    let out = std::process::Command::new("osascript").arg("-e").arg("set fs to choose file with multiple selections allowed\nset o to \"\"\nrepeat with f in fs\nset o to o & POSIX path of f & \"\\n\"\nend repeat\nreturn o").output();
    #[cfg(windows)]
    let out = std::process::Command::new("powershell").args(["-NoProfile", "-Command", "Add-Type -AssemblyName System.Windows.Forms; $d = New-Object System.Windows.Forms.OpenFileDialog; $d.Multiselect = $true; if ($d.ShowDialog() -eq 'OK') { $d.FileNames -join \"`n\" }"]).output();
    #[cfg(all(unix, not(target_os = "macos")))]
    let out = std::process::Command::new("zenity").args(["--file-selection", "--multiple", "--separator=\n"]).output();
    match out { Ok(o) => String::from_utf8_lossy(&o.stdout).lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect(), Err(_) => vec![] }
}

pub fn post_raw(c: &Client, path: &str, body: &[u8], headers: &[(String, String)]) -> Result<J, String> {
    match c.raw_h("POST", path, Some(body), headers) {
        Ok((code, data)) => {
            let j = json::parse(&String::from_utf8_lossy(&data)).unwrap_or(J::Null);
            if code >= 400 { let m = gs(&j, "msg"); Err(if m.is_empty() { gs(&j, "error") } else { m }) } else { Ok(j) }
        }
        Err(e) => Err(e.to_string()),
    }
}
pub fn file_name(p: &str) -> String { std::path::Path::new(p).file_name().map(|x| x.to_string_lossy().to_string()).unwrap_or_default() }

// ---------------------------------------------------------------- console + activity
pub fn kind_of(l: &str) -> i32 {
    if l.contains("[panel]") { 4 }
    else if l.starts_with("> ") { 5 }
    else if l.contains("ERROR") || l.contains("FATAL") || l.contains("Exception") { 3 }
    else if l.contains("WARN") { 2 }
    else if l.contains("]: <") { 1 }
    else { 0 }
}
/// Lines that pass the current console filter and search.
fn shown(a: &App, lines: &[String]) -> Vec<(String, i32)> {
    let f = a.get_con_filter().to_string(); let q = a.get_con_search().to_string().to_lowercase();
    lines.iter().filter_map(|l| { let k = kind_of(l);
        let ok = match f.as_str() { "chat" => k == 1, "warn" => k == 2, "error" => k == 3, "panel" => k == 4, "cmd" => k == 5, _ => true };
        if !ok || (!q.is_empty() && !l.to_lowercase().contains(&q)) { None } else { Some((l.clone(), k)) } }).collect()
}

fn toast(ui: &Weak<App>, t: &str) {
    let t = t.to_string(); with_ui(ui, move |a| a.set_con_toast(SharedString::from(t)));
    let u = ui.clone(); thread::spawn(move || { thread::sleep(Duration::from_millis(1800)); with_ui(&u, |a| a.set_con_toast(SharedString::new())); });
}

pub fn fp_fetch(ui: &Weak<App>, st: &Shared) -> String {
    let (Some(c), id) = (client(st), st.lock().unwrap().sel.id.clone()) else { return String::new() };
    let fp = c.get(&sc(&c, &id, "/api/am/fingerprint")).map(|j| gs(&j, "fingerprint")).unwrap_or_default();
    st.lock().unwrap().x.fp_now = if fp.is_empty() { None } else { Some(fp.clone()) };
    let f2 = fp.clone(); with_ui(ui, move |a| a.set_fp(SharedString::from(f2)));
    fp
}

pub fn set_console(a: &App, lines: &[String]) {
    let out: Vec<ConLine> = shown(a, lines).into_iter().map(|(t, k)| ConLine { text: t.as_str().into(), kind: k }).collect();
    a.set_con_count(SharedString::from(format!("{} lines", out.len())));
    a.set_con(ModelRc::new(VecModel::from(out)));
}

fn act_of(l: &str) -> Option<(i32, ActRow)> {
    let time = l.get(1..6).filter(|_| l.starts_with('[')).unwrap_or("").to_string();
    let msg = l.split_once("]: ").map(|x| x.1).unwrap_or(l);
    let row = |k: i32, t: String| Some((k, ActRow { text: t.into(), kind: if k == 1 { 1 } else if k == 2 { 2 } else { 3 }, time: time.clone().into() }));
    if let Some(i) = l.find("]: <") { let rest = &l[i + 4..]; if let Some(j) = rest.find("> ") { return row(1, format!("{}: {}", &rest[..j], &rest[j + 2..])); } }
    if let Some(n) = msg.strip_suffix(" joined the game") { return row(2, format!("{n} joined")); }
    if let Some(n) = msg.strip_suffix(" left the game") { return row(2, format!("{n} left")); }
    if msg.contains(" has made the advancement ") || msg.contains(" has completed the challenge ") || msg.contains(" has reached the goal ") { return row(4, msg.to_string()); }
    const D: [&str; 14] = [" was slain by ", " was shot by ", " fell from ", " drowned", " burned to death", " blew up", " hit the ground too hard", " starved to death", " suffocated", " was killed", " tried to swim in lava", " was pricked", " was blown up", " died"];
    if !msg.starts_with('<') && !msg.contains(": ") && D.iter().any(|d| msg.contains(d)) { return row(3, msg.to_string()); }
    None
}
pub fn set_activity(a: &App, lines: &[String]) {
    let f = a.get_act_filter().to_string();
    let mut out: Vec<ActRow> = vec![];
    for l in lines.iter().rev().take(900) {
        if let Some((k, r)) = act_of(l) {
            let ok = match f.as_str() { "chat" => k == 1, "join" => k == 2, "death" => k == 3, _ => true };
            if ok { out.push(r); if out.len() >= 150 { break; } }
        }
    }
    out.reverse();
    a.set_act(ModelRc::new(VecModel::from(out)));
}

// ---------------------------------------------------------------- graph + overview extras
fn range_of(label: &str) -> (i64, &'static str) {
    match label { "1h" => (3600, "Last hour"), "24h" => (86400, "Last 24 hours"), "3d" => (259200, "Last 3 days"), "7d" => (604800, "Last 7 days"), _ => (21600, "Last 6 hours") }
}
pub fn load_history(ui: &Weak<App>, st: &Shared) {
    let (Some(c), id) = (client(st), sid(st)) else { return };
    if id.is_empty() { return; }
    let range = { let g = st.lock().unwrap(); if g.x.range == 0 { 21600 } else { g.x.range } };
    let Ok(j) = c.get(&sc(&c, &id, &format!("/api/history2?range={range}"))) else { return };
    let pts: Vec<(f64, Option<f64>, f64)> = arr(&j, "points").iter().filter_map(|p| if let J::Arr(a) = p { Some((num(a.first()?)?, a.get(1).and_then(num), a.get(2).and_then(num).unwrap_or(0.0))) } else { None }).collect();
    let (mut mem, mut ply, mut note) = (String::new(), String::new(), String::new());
    if pts.len() >= 2 {
        let t0 = pts.first().unwrap().0; let t1 = pts.last().unwrap().0;
        let span = (t1 - t0).max(1.0);
        let maxm = pts.iter().filter_map(|p| p.1).fold(1.0f64, f64::max) * 1.15;
        let maxp = pts.iter().map(|p| p.2).fold(2.0f64, f64::max);
        let mut first = true;
        for p in &pts { if let Some(m) = p.1 { mem.push_str(&format!("{} {:.1} {:.1} ", if first { "M" } else { "L" }, (p.0 - t0) / span * 1000.0, 100.0 - m / maxm * 100.0)); first = false; } }
        let mut first = true;
        for p in &pts { ply.push_str(&format!("{} {:.1} {:.1} ", if first { "M" } else { "L" }, (p.0 - t0) / span * 1000.0, 100.0 - p.2 / maxp * 100.0)); first = false; }
    } else { note = "Collecting data... (a point every 30 seconds, kept for 7 days)".into(); }
    with_ui(ui, move |a| { a.set_g_mem(SharedString::from(mem)); a.set_g_ply(SharedString::from(ply)); a.set_g_note(SharedString::from(note)); });
}
pub fn load_public_ip(ui: &Weak<App>, st: &Shared) {
    let (Some(c), id) = (client(st), sid(st)) else { return };
    if id.is_empty() { return; }
    let t = match c.get(&sc(&c, &id, "/api/publicip")) { Ok(j) => { let ip = gs(&j, "ip"); if ip.is_empty() { "Not available".to_string() } else { format!("{}:{}", ip, gs(&c.get(&sc(&c, &id, "/api/status?since=2000000000")).unwrap_or(J::Null), "port")) } } Err(_) => "Not available".to_string() };
    with_ui(ui, move |a| a.set_ov_public(SharedString::from(t)));
}
pub fn load_extras(ui: &Weak<App>, st: &Shared) { load_history(ui, st); load_public_ip(ui, st); }

fn nf_load(ui: &Weak<App>, st: &Shared) {
    let (Some(c), id) = (client(st), sid(st)) else { return };
    match c.get(&sc(&c, &id, "/api/neoforge/versions")) {
        Ok(j) => {
            let cur = gs(&j, "current");
            let list: Vec<SharedString> = arr(&j, "versions").iter().map(|i| { let mut t = format!("{}  ({})", gs(i, "v"), gs(i, "type")); if flag(i, "installed") { t.push_str("  - installed"); } SharedString::from(t) }).collect();
            with_ui(ui, move |a| { a.set_nf_current(SharedString::from(cur)); a.set_nf_versions(ModelRc::new(VecModel::from(list))); a.set_nf_msg(SharedString::new()); });
        }
        Err(e) => { let m = e.to_string(); with_ui(ui, move |a| a.set_nf_msg(SharedString::from(m))); }
    }
}

pub fn split_ids(t: &str) -> Vec<String> { let mut v: Vec<String> = t.split(|c: char| c == ',' || c == '\n' || c == ';').map(|x| x.trim().to_lowercase()).filter(|x| !x.is_empty()).collect(); v.sort(); v.dedup(); v }

// ---------------------------------------------------------------- mods list
fn size_val(t: &str) -> f64 {
    let mut it = t.split_whitespace(); let v: f64 = it.next().and_then(|x| x.parse().ok()).unwrap_or(0.0);
    v * match it.next() { Some("GB") => 1e9, Some("MB") => 1e6, Some("KB") => 1e3, _ => 1.0 }
}
fn filtered(rows: &[ModRow], q: &str, chip: &str) -> Vec<ModRow> {
    let q = q.trim().to_lowercase();
    rows.iter().filter(|r| {
        let hit = q.is_empty() || r.name.to_lowercase().contains(&q) || r.file.to_lowercase().contains(&q);
        let c = match chip { "disabled" => !r.enabled, "client" => r.badge.contains("Client"), "update" => !r.update.is_empty(), _ => true };
        hit && c
    }).cloned().collect()
}
pub fn render_mods(a: &App, st: &Shared) {
    let (mut rows, sel, sort) = { let g = st.lock().unwrap(); (g.mods.clone(), g.x.sel.clone(), g.x.sort) };
    for r in rows.iter_mut() { r.sel = sel.contains(r.file.as_str()); }
    let total: f64 = rows.iter().map(|r| size_val(r.size.as_str())).sum();
    let dis = rows.iter().filter(|r| !r.enabled).count();
    let cli = rows.iter().filter(|r| r.badge.contains("Client")).count();
    let upd = rows.iter().filter(|r| !r.update.is_empty()).count();
    let mut list = filtered(&rows, a.get_mod_search().as_str(), a.get_mod_chip().as_str());
    match sort { 1 => list.sort_by(|x, y| size_val(y.size.as_str()).partial_cmp(&size_val(x.size.as_str())).unwrap_or(std::cmp::Ordering::Equal)), 2 => list.sort_by_key(|r| (r.enabled, r.name.to_lowercase())), _ => list.sort_by_key(|r| r.name.to_lowercase()) }
    a.set_c_all(rows.len() as i32); a.set_c_dis(dis as i32); a.set_c_cli(cli as i32); a.set_c_upd(upd as i32);
    a.set_mods_sub(SharedString::from(format!("{} on the server · {} disabled · {} total", rows.len() - dis, dis, fsize(total as i64))));
    a.set_mod_sort(SharedString::from(["Name", "Size", "Status"][sort % 3]));
    a.set_sel_count(rows.iter().filter(|r| r.sel).count() as i32);
    a.set_mod_count_text(SharedString::from(format!("{} mods", list.len())));
    a.set_ov_mods(SharedString::from(rows.len().to_string()));
    a.set_mods(ModelRc::new(VecModel::from(list)));
    crate::icons::apply(a, st);
}

// ---------------------------------------------------------------- datapacks
pub fn load_dp(ui: &Weak<App>, st: &Shared) {
    let (Some(c), id) = (client(st), sid(st)) else { return };
    if let Ok(j) = c.get(&sc(&c, &id, "/api/dp/list")) {
        let rows: Vec<DpRow> = arr(&j, "packs").iter().map(|p| DpRow { name: gs(p, "name").into(), size: if gn(p, "size") > 0 { fsize(gn(p, "size")).into() } else { SharedString::new() }, kind: gs(p, "kind").into() }).collect();
        let note = if flag(&j, "world_exists") { format!("In the world \"{}\". Datapacks load when the world loads, or press Reload.", gs(&j, "world")) } else { "No world yet. Start the server once so it creates one.".to_string() };
        with_ui(ui, move |a| { a.set_dp(ModelRc::new(VecModel::from(rows))); a.set_dp_note(SharedString::from(note)); });
    }
}

// ---------------------------------------------------------------- files
fn when(t: i64) -> String { let d = now_s() - t; if d < 0 { "now".into() } else { ago(d) } }
fn f_kind_ok(f: &J, filter: &str) -> bool {
    if filter.is_empty() || filter == "all" { return true; }
    let dir = flag(f, "dir");
    if filter == "dir" { return dir; }
    if dir { return false; }
    let n = gs(f, "name").to_lowercase();
    let ext = n.rsplit_once('.').map(|x| x.1.to_string()).unwrap_or_default();
    match filter {
        "config" => ["toml", "json", "json5", "properties", "cfg", "conf", "yml", "yaml", "ini", "txt", "snbt"].contains(&ext.as_str()),
        "logs" => ["log", "gz"].contains(&ext.as_str()) && (n.contains("log") || ext == "log"),
        "mods" => ["jar", "disabled"].contains(&ext.as_str()) || n.ends_with(".jar.disabled"),
        "archives" => ["zip", "7z", "rar", "tar", "gz", "tgz", "mrpack"].contains(&ext.as_str()),
        "images" => ["png", "jpg", "jpeg", "gif", "webp", "bmp", "svg"].contains(&ext.as_str()),
        _ => true,
    }
}
pub fn files_render(ui: &Weak<App>, st: &Shared) {
    let (items, filter, p) = { let g = st.lock().unwrap(); (g.x.f_items.clone(), g.x.f_filter.clone(), g.x.f_path.clone()) };
    let total = items.len();
    let rows: Vec<FileRow> = items.iter().filter(|f| f_kind_ok(f, &filter)).map(|f| { let dir = flag(f, "dir"); FileRow { name: gs(f, "name").into(), path: gs(f, "path").into(), dir, size: if dir { SharedString::new() } else { fsize(gn(f, "size")).into() }, mtime: when(gn(f, "mtime")).into() } }).collect();
    let shown = rows.len();
    let mut crumbs = vec![Crumb { name: "Server".into(), path: SharedString::new() }];
    let mut acc = String::new();
    for seg in p.split('/').filter(|x| !x.is_empty()) { if !acc.is_empty() { acc.push('/'); } acc.push_str(seg); crumbs.push(Crumb { name: seg.into(), path: acc.clone().into() }); }
    let note = if shown == total { format!("{total} items") } else { format!("{shown} of {total} items") };
    with_ui(ui, move |a| { a.set_files(ModelRc::new(VecModel::from(rows))); a.set_crumbs(ModelRc::new(VecModel::from(crumbs))); a.set_f_path(SharedString::from(p)); a.set_f_count(SharedString::from(note)); });
}
pub fn files_load(ui: &Weak<App>, st: &Shared, path: &str) {
    let (Some(c), id) = (client(st), sid(st)) else { return };
    match c.get(&sc(&c, &id, &format!("/api/files/list?path={}", enc(path)))) {
        Ok(j) => {
            { let mut g = st.lock().unwrap(); g.x.f_path = gs(&j, "path"); g.x.f_items = arr(&j, "items").to_vec(); }
            files_render(ui, st);
        }
        Err(e) => { let m = e.to_string(); with_ui(ui, move |a| a.set_f_msg(SharedString::from(m))); }
    }
}
fn file_msg(ui: &Weak<App>, t: &str) { let t = t.to_string(); with_ui(ui, move |a| a.set_f_msg(SharedString::from(t))); }

// ---------------------------------------------------------------- automation
fn get_path<'a>(j: &'a J, path: &str) -> Option<&'a J> {
    let mut cur = j;
    for seg in path.split('.') { cur = match cur { J::Obj(m) => m.iter().find(|(k, _)| k == seg).map(|x| &x.1)?, J::Arr(a) => a.get(seg.parse::<usize>().ok()?)?, _ => return None }; }
    Some(cur)
}
fn set_path(j: &mut J, path: &str, v: String) {
    let segs: Vec<&str> = path.split('.').collect();
    fn go(cur: &mut J, segs: &[&str], v: &str) {
        let Some((first, rest)) = segs.split_first() else { return };
        let slot: Option<&mut J> = match cur { J::Obj(m) => m.iter_mut().find(|(k, _)| k == first).map(|x| &mut x.1), J::Arr(a) => first.parse::<usize>().ok().and_then(|i| a.get_mut(i)), _ => None };
        let Some(slot) = slot else { return };
        if rest.is_empty() {
            *slot = match slot { J::Bool(_) => J::Bool(v == "true"), J::Num(old) => J::Num(v.trim().parse::<f64>().unwrap_or(*old)), _ => J::Str(v.to_string()) };
        } else { go(slot, rest, v); }
    }
    go(j, &segs, &v);
}
fn fld(key: &str, label: &str, help: &str, kind: i32, value: String, options: &[&str]) -> Field {
    Field { key: key.into(), label: label.into(), help: help.into(), kind, value: value.into(), options: ModelRc::new(VecModel::from(options.iter().map(|o| SharedString::from(*o)).collect::<Vec<_>>())), count: 0, open: false }
}
fn head(name: &str, count: usize, open: bool) -> Field { Field { key: SharedString::new(), label: name.into(), help: SharedString::new(), kind: 9, value: SharedString::new(), options: ModelRc::default(), count: count as i32, open } }
fn pv(j: &J, p: &str) -> String { crate::jstr(get_path(j, p)) }
fn kind_label(k: &str) -> &'static str { match k { "backup" => "Back up the world", "restart" => "Restart the server", "broadcast" => "Announce a message", "command" => "Run a command", "start" => "Start the server", "stop" => "Stop the server", _ => "Task" } }
pub fn auto_rows(cfg: &J, mu: &J, open: &HashSet<String>) -> Vec<Field> {
    let mut out = vec![];
    let tasks = arr(cfg, "tasks");
    let o = open.contains("Scheduled tasks");
    out.push(head("Scheduled tasks", tasks.len(), o));
    if o {
        for (i, t) in tasks.iter().enumerate() {
            let k = gs(t, "kind"); let mode = gs(t, "mode");
            let when = if mode == "every" { format!("every {} min", gn(t, "every")) } else { format!("daily at {}", gs(t, "time")) };
            out.push(fld(&format!("tasks.{i}.on"), &format!("{} · {}", kind_label(&k), when), "", 5, pv(cfg, &format!("tasks.{i}.on")), &[]));
            out.push(fld(&format!("tasks.{i}.mode"), "   Repeat", "Daily at a set time, or every so often", 2, mode.clone(), &["daily", "every"]));
            if mode == "every" { out.push(fld(&format!("tasks.{i}.every"), "   Every (minutes)", "", 0, pv(cfg, &format!("tasks.{i}.every")), &[])); }
            else { out.push(fld(&format!("tasks.{i}.time"), "   Time (24 hour, HH:MM)", "", 0, pv(cfg, &format!("tasks.{i}.time")), &[])); }
            if k == "broadcast" || k == "command" { out.push(fld(&format!("tasks.{i}.text"), if k == "broadcast" { "   Message" } else { "   Command" }, "", 0, pv(cfg, &format!("tasks.{i}.text")), &[])); }
        }
        for (k, l) in [("backup", "Add: back up"), ("restart", "Add: restart"), ("broadcast", "Add: announcement"), ("command", "Add: command"), ("start", "Add: start"), ("stop", "Add: stop")] { out.push(fld(&format!("add:{k}"), l, "", 6, String::new(), &[])); }
    }
    let o = open.contains("Idle shutdown");
    out.push(head("Idle shutdown", 2, o));
    if o { out.push(fld("idle.on", "Stop the server when nobody is on", "Saves power and memory on your Mac", 1, pv(cfg, "idle.on"), &[])); out.push(fld("idle.minutes", "After this many empty minutes", "", 0, pv(cfg, "idle.minutes"), &[])); }
    let o = open.contains("Welcome message");
    out.push(head("Welcome message", 2, o));
    if o { out.push(fld("welcome.on", "Greet players when they join", "", 1, pv(cfg, "welcome.on"), &[])); out.push(fld("welcome.text", "Message", "", 0, pv(cfg, "welcome.text"), &[])); }
    let o = open.contains("Discord alerts");
    out.push(head("Discord alerts", 7, o));
    if o {
        out.push(fld("discord.url", "Webhook address", "Leave it as 'keep' to keep the one already saved", 0, pv(cfg, "discord.url"), &[]));
        for (k, l) in [("start", "Server started"), ("stop", "Server stopped"), ("crash", "Server crashed"), ("join", "Player joined"), ("leave", "Player left")] { out.push(fld(&format!("discord.{k}"), l, "", 1, pv(cfg, &format!("discord.{k}")), &[])); }
        out.push(fld("discord-test", "Send a test message", "", 6, String::new(), &[]));
    }
    let o = open.contains("Mod updates");
    out.push(head("Mod updates", 3, o));
    if o {
        out.push(fld("modupd.mode", "Check for mod updates", "always, on boot, every morning, daily at a time, or only when you press the button", 2, pv(mu, "mode"), &["always", "boot", "morning", "daily", "manual"]));
        out.push(fld("modupd.time", "Time (for daily)", "24 hour, HH:MM", 0, pv(mu, "time"), &[]));
        out.push(fld("modupd.skip", "Mods to leave alone", "Mod ids that never get update offers, separated by commas. Select mods on the Mods tab and press Leave alone to add them.", 0, pv(mu, "skip"), &[]));
        out.push(fld("modupd.install", "Install updates automatically", "Warns players, saves, stops, installs, then restarts", 1, pv(mu, "install"), &[]));
    }
    out
}
pub fn render_auto(a: &App, st: &Shared) {
    let (cfg, mu, open) = { let g = st.lock().unwrap(); (g.x.auto.clone(), g.x.modupd.clone(), g.x.auto_open.clone()) };
    if let (Some(c), Some(m)) = (cfg, mu) { a.set_afields(ModelRc::new(VecModel::from(auto_rows(&c, &m, &open)))); }
}
pub fn load_auto(ui: &Weak<App>, st: &Shared) {
    let (Some(c), id) = (client(st), sid(st)) else { return };
    let cfg = c.get(&sc(&c, &id, "/api/auto")).ok().and_then(|j| j.get("cfg").cloned());
    let mu = c.get(&sc(&c, &id, "/api/modupd")).ok().map(|mut j| { let t = arr(&j, "skip").iter().filter_map(|x| x.as_str().map(|y| y.to_string())).collect::<Vec<_>>().join(", "); if let J::Obj(m) = &mut j { m.retain(|(k, _)| k != "skip"); m.push(("skip".into(), s(&t))); } j });
    if let (Some(cfg), Some(mu)) = (cfg, mu) {
        { let mut g = st.lock().unwrap(); g.x.auto = Some(cfg); g.x.modupd = Some(mu); if g.x.auto_open.is_empty() { g.x.auto_open.insert("Scheduled tasks".into()); } }
        let s2 = st.clone();
        with_ui(ui, move |a| { a.set_auto_dirty(false); a.set_auto_msg(SharedString::new()); render_auto(a, &s2); });
    } else { with_ui(ui, |a| a.set_auto_msg(SharedString::from("Couldn't load the automation settings."))); }
}
fn new_task(kind: &str) -> J {
    let id: String = format!("t{:x}", now_s() % 0xfffffff);
    obj(vec![("id", s(&id)), ("on", J::Bool(true)), ("kind", s(kind)), ("mode", s("daily")), ("time", s("04:00")), ("every", n(120)), ("days", J::Arr(vec![])), ("text", s("")), ("warn", J::Bool(true))])
}

// ---------------------------------------------------------------- wiring
pub fn wire(app: &App, ui: &Weak<App>, st: &Shared) {
    { app.on_copy_text(move |t| { let t = t.to_string(); bg(move || copy_clip(&t)); }); }
    // ---- overview
    { let (u, s_) = (ui.clone(), st.clone()); app.on_nf_load(move || { let (u, s_) = (u.clone(), s_.clone()); bg(move || nf_load(&u, &s_)); }); }
    { let (u, s_) = (ui.clone(), st.clone()); app.on_nf_install(move |v| {
        let v = v.split_whitespace().next().unwrap_or("").to_string(); let (u, s_) = (u.clone(), s_.clone());
        bg(move || {
            let (Some(c), id) = (client(&s_), sid(&s_)) else { return };
            let r = c.post(&sc(&c, &id, "/api/neoforge/update"), &obj(vec![("version", s(&v)), ("backup", J::Bool(true))]));
            if !ok_of(&r) { let m = msg_of(&r); with_ui(&u, move |a| a.set_nf_msg(SharedString::from(m))); return; }
            for _ in 0..900 {
                thread::sleep(Duration::from_secs(2));
                let Ok(j) = c.get(&sc(&c, &id, "/api/neoforge/status")) else { continue };
                let m = gs(&j, "msg"); let ph = gs(&j, "phase");
                with_ui(&u, move |a| a.set_nf_msg(SharedString::from(m)));
                if ph != "running" { break; }
            }
            nf_load(&u, &s_);
        });
    }); }
    { let (u, s_) = (ui.clone(), st.clone()); app.on_g_set(move |l| {
        let (secs, title) = range_of(l.as_str()); s_.lock().unwrap().x.range = secs;
        if let Some(a) = u.upgrade() { a.set_g_range(l.clone()); a.set_g_title(SharedString::from(title)); }
        let (u, s_) = (u.clone(), s_.clone()); bg(move || load_history(&u, &s_));
    }); }
    { let (u, s_) = (ui.clone(), st.clone()); app.on_act_refilter(move || { if let Some(a) = u.upgrade() { let l = s_.lock().unwrap().sel.lines.clone(); set_activity(&a, &l); } }); }
    { let (u, s_) = (ui.clone(), st.clone()); app.on_act_say(move |t| {
        let t = t.trim().to_string(); if t.is_empty() { return; } let (u, s_) = (u.clone(), s_.clone());
        bg(move || { let (Some(c), id) = (client(&s_), sid(&s_)) else { return }; let r = c.post(&sc(&c, &id, "/api/command"), &obj(vec![("cmd", s(&format!("say {t}")))])); say(&u, &if ok_of(&r) { String::new() } else { msg_of(&r) }); });
    }); }
    // ---- console
    { let (u, s_) = (ui.clone(), st.clone()); app.on_con_refilter(move || { if let Some(a) = u.upgrade() { let l = s_.lock().unwrap().sel.lines.clone(); set_console(&a, &l); } }); }
    { let (u, s_) = (ui.clone(), st.clone()); app.on_con_clear(move || { s_.lock().unwrap().sel.lines.clear(); if let Some(a) = u.upgrade() { set_console(&a, &[]); set_activity(&a, &[]); } }); }
    { let s_ = st.clone(); app.on_con_copy(move || { let t = s_.lock().unwrap().sel.lines.join("\n"); bg(move || copy_clip(&t)); }); }
    { let u = ui.clone(); app.on_con_copy_line(move |t| { let t = t.to_string(); let u = u.clone(); bg(move || { copy_clip(&t); toast(&u, "Line copied"); }); }); }
    { let (u, s_) = (ui.clone(), st.clone()); app.on_con_snapshot(move || {
        let Some(a) = u.upgrade() else { return }; let l = s_.lock().unwrap().sel.lines.clone();
        let tail: Vec<String> = shown(&a, &l).into_iter().map(|x| x.0).rev().take(800).collect::<Vec<_>>().into_iter().rev().collect();
        a.set_con_text(SharedString::from(tail.join("\n")));
    }); }
    { let (u, s_) = (ui.clone(), st.clone()); app.on_con_copy(move || {
        let Some(a) = u.upgrade() else { return }; let l = s_.lock().unwrap().sel.lines.clone();
        let t = shown(&a, &l).into_iter().map(|x| x.0).collect::<Vec<_>>().join("\n"); let u = u.clone();
        bg(move || { copy_clip(&t); toast(&u, "Copied what is shown"); });
    }); }
    { let (u, s_) = (ui.clone(), st.clone()); app.on_con_save(move || {
        let Some(a) = u.upgrade() else { return }; let l = s_.lock().unwrap().sel.lines.clone(); let t = shown(&a, &l).into_iter().map(|x| x.0).collect::<Vec<_>>().join("\n"); let u = u.clone();
        bg(move || { let name = format!("server-log-{}.txt", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)); let p = downloads_dir().join(&name);
            match std::fs::write(&p, t) { Ok(_) => toast(&u, &format!("Saved to Downloads/{name}")), Err(e) => toast(&u, &format!("Couldn't save: {e}")) } });
    }); }
    { let s_ = st.clone(); app.on_fp_copy(move || { let Some(fp) = s_.lock().unwrap().x.fp_now.clone() else { return }; bg(move || copy_clip(&fp)); }); }
    { let (u, s_) = (ui.clone(), st.clone()); app.on_fp_get(move || {
        let Some(a) = u.upgrade() else { return }; if a.get_server_state().as_str() != "online" { toast(&u, "Start the server first."); return; }
        let (u, s_) = (u.clone(), s_.clone());
        bg(move || { let (Some(c), id) = (client(&s_), s_.lock().unwrap().sel.id.clone()) else { return };
            let _ = c.post(&sc(&c, &id, "/api/command"), &obj(vec![("cmd", s("automodpack host fingerprint"))])); toast(&u, "Asked the server...");
            for _ in 0..8 { thread::sleep(Duration::from_secs(1)); if !fp_fetch(&u, &s_).is_empty() { return; } }
            toast(&u, "No fingerprint printed. Does this server run AutoModpack?"); });
    }); }
    // ---- mods list
    { let (u, s_) = (ui.clone(), st.clone()); app.on_mods_sort(move || { { let mut g = s_.lock().unwrap(); g.x.sort = (g.x.sort + 1) % 3; } if let Some(a) = u.upgrade() { render_mods(&a, &s_); } }); }
    { let (u, s_) = (ui.clone(), st.clone()); app.on_mod_select(move |f, on| { { let mut g = s_.lock().unwrap(); if on { g.x.sel.insert(f.to_string()); } else { g.x.sel.remove(f.as_str()); } } if let Some(a) = u.upgrade() { render_mods(&a, &s_); } }); }
    { let (u, s_) = (ui.clone(), st.clone()); app.on_mods_select_all(move |on| {
        let Some(a) = u.upgrade() else { return };
        let rows = s_.lock().unwrap().mods.clone();
        let shown = filtered(&rows, a.get_mod_search().as_str(), a.get_mod_chip().as_str());
        { let mut g = s_.lock().unwrap(); if on { for r in shown { g.x.sel.insert(r.file.to_string()); } } else { g.x.sel.clear(); } }
        render_mods(&a, &s_);
    }); }
    { let (u, s_) = (ui.clone(), st.clone()); app.on_mods_refilter(move || { if let Some(a) = u.upgrade() { render_mods(&a, &s_); } }); }
    { let (u, s_) = (ui.clone(), st.clone()); app.on_bulk(move |what| {
        let what = what.to_string(); let (u, s_) = (u.clone(), s_.clone());
        let files: Vec<J> = s_.lock().unwrap().x.sel.iter().map(|f| s(f)).collect();
        if files.is_empty() { return; }
        bg(move || {
            let (Some(c), id) = (client(&s_), sid(&s_)) else { return };
            let r = match what.as_str() {
                "leave" => {
                    let (ids, mut cur): (Vec<String>, Vec<String>) = { let g = s_.lock().unwrap(); (files.iter().filter_map(|f| f.as_str().and_then(|f| g.x.modids.get(f)).cloned()).filter(|x| !x.is_empty()).collect(), g.x.modupd.as_ref().map(|m| split_ids(&pv(m, "skip"))).unwrap_or_default()) };
                    if ids.is_empty() { with_ui(&u, |a| a.set_mods_msg(SharedString::from("Couldn't read an id for those mods."))); return; }
                    for i in ids.iter() { let l = i.to_lowercase(); if !cur.contains(&l) { cur.push(l); } }
                    let r = c.post(&sc(&c, &id, "/api/modupd"), &obj(vec![("skip", J::Arr(cur.iter().map(|x| s(x)).collect()))]));
                    if ok_of(&r) { Ok(obj(vec![("ok", J::Bool(true)), ("msg", s(&format!("Leaving {} mod{} alone. They won't be offered updates. Edit the list under Settings > Automation > Mod updates.", ids.len(), if ids.len() == 1 { "" } else { "s" })))])) } else { r }
                }
                "remove" => c.post(&sc(&c, &id, "/api/mods/delete"), &obj(vec![("files", J::Arr(files))])),
                "srvonly" | "inpack" => c.post(&sc(&c, &id, "/api/mods/flag"), &obj(vec![("files", J::Arr(files)), ("server_only", J::Bool(what == "srvonly"))])),
                w => c.post(&sc(&c, &id, "/api/mods/toggle"), &obj(vec![("files", J::Arr(files)), ("enabled", J::Bool(w == "enable"))])),
            };
            s_.lock().unwrap().x.sel.clear();
            let m = msg_of(&r); with_ui(&u, move |a| a.set_mods_msg(SharedString::from(m)));
            load_mods(&u, &s_);
        });
    }); }
    { let (u, s_) = (ui.clone(), st.clone()); app.on_mods_set_tab(move |t| {
        if let Some(a) = u.upgrade() { a.set_mods_tab(t.clone()); }
        if t.as_str() == "dp" { let (u, s_) = (u.clone(), s_.clone()); bg(move || load_dp(&u, &s_)); }
    }); }
    { let (u, _s) = (ui.clone(), st.clone()); app.on_mods_set_view(move |v| {
        if let Some(a) = u.upgrade() { a.set_mods_view(v.clone()); if v.as_str() == "browse" && a.get_br_note().is_empty() { a.invoke_br_search(); } }
    }); }
    { let (u, s_) = (ui.clone(), st.clone()); app.on_mods_add(move || {
        let (u, s_) = (u.clone(), s_.clone());
        bg(move || {
            let paths = pick_files();
            if paths.is_empty() { return; }
            let (Some(c), id) = (client(&s_), sid(&s_)) else { return };
            let mut items: Vec<J> = vec![]; let mut notes: Vec<String> = vec![];
            for p in &paths {
                let name = file_name(p);
                let Ok(bytes) = std::fs::read(p) else { notes.push(format!("{name}: couldn't read it")); continue };
                with_ui(&u, { let n_ = name.clone(); move |a| a.set_mods_msg(SharedString::from(format!("Uploading {n_}..."))) });
                match post_raw(&c, &sc(&c, &id, "/api/mods/stage"), &bytes, &[("x-filename".into(), enc(&name))]) {
                    Ok(j) => {
                        let conflicts = arr(&j, "conflicts");
                        let action = if conflicts.iter().any(|x| flag(x, "same_file")) || !conflicts.is_empty() { "replace" } else { "add" };
                        let rep: Vec<J> = conflicts.iter().map(|x| s(&gs(x, "file"))).collect();
                        items.push(obj(vec![("id", s(&gs(&j, "id"))), ("action", s(action)), ("replace_files", J::Arr(rep))]));
                    }
                    Err(e) => notes.push(format!("{name}: {e}")),
                }
            }
            let mut msg = String::new();
            if !items.is_empty() {
                match c.post(&sc(&c, &id, "/api/mods/commit"), &obj(vec![("items", J::Arr(items))])) {
                    Ok(j) => { let r = j.get("result").cloned().unwrap_or(J::Null); msg = format!("Added {}, replaced {}.", arr(&r, "added").len(), arr(&r, "replaced").len()); for k in ["skipped", "errors"] { for x in arr(&r, k) { if let Some(t) = x.as_str() { notes.push(t.to_string()); } } } }
                    Err(e) => notes.push(e.to_string()),
                }
            }
            if !notes.is_empty() { msg = format!("{} {}", msg, notes.join("; ")); }
            with_ui(&u, move |a| a.set_mods_msg(SharedString::from(msg.trim().to_string())));
            load_mods(&u, &s_);
        });
    }); }
    { let (u, s_) = (ui.clone(), st.clone()); app.on_mods_copy_list(move || {
        let mut rows = s_.lock().unwrap().mods.clone(); rows.sort_by_key(|r| r.name.to_lowercase());
        let mut t = String::new(); let mut n_ = 0;
        for r in rows.iter().filter(|r| r.enabled) { n_ += 1; t.push_str(r.name.as_str()); if !r.version.is_empty() { t.push(' '); t.push_str(r.version.as_str()); } t.push('\n'); }
        copy_clip(&t);
        let m = format!("Copied {n_} mod names to the clipboard.");
        with_ui(&u, move |a| a.set_mods_msg(SharedString::from(m)));
    }); }
    { let (u, s_) = (ui.clone(), st.clone()); app.on_mods_am_sync(move || {
        let (u, s_) = (u.clone(), s_.clone());
        bg(move || { let (Some(c), id) = (client(&s_), sid(&s_)) else { return }; let r = c.post(&sc(&c, &id, "/api/am/sync"), &obj(vec![])); let m = msg_of(&r); with_ui(&u, move |a| a.set_mods_msg(SharedString::from(m))); });
    }); }
    { let (u, s_) = (ui.clone(), st.clone()); app.on_mods_export(move |fmt| {
        let fmt = fmt.to_string(); let (u, s_) = (u.clone(), s_.clone());
        bg(move || {
            let (Some(c), id) = (client(&s_), sid(&s_)) else { return };
            with_ui(&u, |a| a.set_mods_msg(SharedString::from("Building the pack...")));
            let j = match c.get(&sc(&c, &id, &format!("/api/modpack/build?format={}", enc(&fmt)))) { Ok(j) => j, Err(e) => { let m = e.to_string(); with_ui(&u, move |a| a.set_mods_msg(SharedString::from(m))); return; } };
            let (pid, fname) = (gs(&j, "id"), gs(&j, "filename"));
            let msg = match c.raw("GET", &sc(&c, &id, &format!("/api/modpack/file?id={}", enc(&pid))), None) {
                Ok((200, bytes)) => { let dir = home().join("Downloads"); let _ = std::fs::create_dir_all(&dir); let dest = dir.join(&fname); match std::fs::write(&dest, bytes) { Ok(_) => format!("Saved {} to your Downloads folder. {}", fname, gs(&j, "summary")), Err(e) => format!("Couldn't save it: {e}") } }
                _ => "That download expired. Try again.".to_string(),
            };
            with_ui(&u, move |a| a.set_mods_msg(SharedString::from(msg)));
        });
    }); }
    // ---- browse
    { let (u, s_) = (ui.clone(), st.clone()); app.on_br_search(move || {
        let Some(a) = u.upgrade() else { return };
        let (q, src, sort) = (a.get_br_q().to_string(), a.get_br_source().to_string(), a.get_br_sort().to_string());
        let cat_idx = a.get_br_cat_index().max(0) as usize;
        a.set_br_note(SharedString::from("Searching..."));
        let (u, s_) = (u.clone(), s_.clone());
        bg(move || {
            let (Some(c), id) = (client(&s_), sid(&s_)) else { return };
            let mut cat_idx = cat_idx;
            if s_.lock().unwrap().x.cats_src != src {
                let cats: Vec<(String, String)> = c.get(&sc(&c, &id, &format!("/api/browse/categories?source={}", enc(&src)))).map(|j| arr(&j, "categories").iter().map(|x| (gs(x, "id"), gs(x, "name"))).collect()).unwrap_or_default();
                let names: Vec<SharedString> = std::iter::once(SharedString::from("All categories")).chain(cats.iter().map(|x| SharedString::from(x.1.as_str()))).collect();
                { let mut g = s_.lock().unwrap(); g.x.cats = cats; g.x.cats_src = src.clone(); }
                cat_idx = 0;
                with_ui(&u, move |a| { a.set_br_cats(ModelRc::new(VecModel::from(names))); a.set_br_cat_index(0); });
            }
            let category = if cat_idx == 0 { String::new() } else { s_.lock().unwrap().x.cats.get(cat_idx - 1).map(|x| x.0.clone()).unwrap_or_default() };
            match c.get(&sc(&c, &id, &format!("/api/browse/search?source={}&q={}&sort={}&category={}&limit=30", enc(&src), enc(&q), enc(&sort), enc(&category)))) {
                Ok(j) => {
                    let icons: Vec<(String, String)> = arr(&j, "results").iter().filter_map(|r| { let u = gs(r, "icon"); if u.starts_with("https://") { Some((gs(r, "id"), u)) } else { None } }).collect();
                    let rows: Vec<BrowseRow> = arr(&j, "results").iter().map(|r| {
                        let nm = gs(r, "name");
                        let inst = r.get("installed").filter(|x| matches!(x, J::Obj(_))).map(|x| gs(x, "version")).unwrap_or_default();
                        BrowseRow { id: gs(r, "id").into(), name: nm.as_str().into(), author: gs(r, "author").into(), summary: gs(r, "summary").into(), downloads: human(gn(r, "downloads")).into(), installed: inst.into(), compat: !matches!(r.get("compat"), Some(J::Bool(false))), manual: flag(r, "manual"), tint: tint_of(&nm), letter: letter_of(&nm).into() }
                    }).collect();
                    let env = j.get("env").map(|e| gs(e, "label")).unwrap_or_default();
                    let note = if rows.is_empty() { "Nothing found.".to_string() } else if env.is_empty() { String::new() } else { format!("Showing mods that work with {env}.") };
                    let s3 = s_.clone();
                    with_ui(&u, move |a| { a.set_br(ModelRc::new(VecModel::from(rows))); a.set_br_note(SharedString::from(note)); a.set_br_need_key(false); crate::icons::apply(a, &s3); });
                    { s_.lock().unwrap().x.br_icons = icons.iter().cloned().collect(); }
                    crate::icons::fetch(&u, &s_, icons.into_iter().map(|x| x.1).collect());
                }
                Err(e) => { let m = e.to_string(); let need = src == "curseforge" && (m.contains("key") || m.contains("Key")); with_ui(&u, move |a| { a.set_br_note(SharedString::from(m)); a.set_br_need_key(need); }); }
            }
        });
    }); }
    { let (u, s_) = (ui.clone(), st.clone()); app.on_br_install(move |pid| {
        let pid = pid.to_string(); let (u, s_) = (u.clone(), s_.clone());
        let src = u.upgrade().map(|a| a.get_br_source().to_string()).unwrap_or_else(|| "modrinth".into());
        with_ui(&u, |a| { a.set_br_busy(true); a.set_br_note(SharedString::from("Finding the right version...")); });
        bg(move || {
            let done = |u: &Weak<App>, m: String| with_ui(u, move |a| { a.set_br_busy(false); a.set_br_note(SharedString::from(m)); });
            let (Some(c), id) = (client(&s_), sid(&s_)) else { return };
            let vs = match c.get(&sc(&c, &id, &format!("/api/browse/versions?source={}&id={}", enc(&src), enc(&pid)))) { Ok(j) => arr(&j, "versions"), Err(e) => { done(&u, e.to_string()); return } };
            let Some(v) = vs.iter().find(|v| flag(v, "compatible") && flag(v, "can_download")).or_else(|| vs.iter().find(|v| flag(v, "can_download"))) else { done(&u, "There's no downloadable version for this server.".into()); return };
            let r = c.post(&sc(&c, &id, "/api/browse/install"), &obj(vec![("source", s(&src)), ("project_id", s(&pid)), ("version_id", s(&gs(v, "id"))), ("with_deps", J::Bool(true))]));
            if !ok_of(&r) { done(&u, msg_of(&r)); return; }
            let mut last = String::new();
            for _ in 0..600 {
                thread::sleep(Duration::from_secs(1));
                let Ok(j) = c.get(&sc(&c, &id, "/api/browse/install/status")) else { continue };
                last = gs(&j, "msg");
                let m = last.clone(); with_ui(&u, move |a| a.set_br_note(SharedString::from(m)));
                if gs(&j, "phase") != "running" { break; }
            }
            done(&u, if last.is_empty() { "Installed.".into() } else { last });
            load_mods(&u, &s_);
            with_ui(&u, |a| a.invoke_br_search());
        });
    }); }
    { let (u, s_) = (ui.clone(), st.clone()); app.on_cf_save_key(move |k| {
        let k = k.trim().to_string(); let (u, s_) = (u.clone(), s_.clone());
        bg(move || { let (Some(c), id) = (client(&s_), sid(&s_)) else { return }; let r = c.post(&sc(&c, &id, "/api/cf/key"), &obj(vec![("key", s(&k))])); let m = msg_of(&r); let ok = ok_of(&r); with_ui(&u, move |a| { a.set_br_note(SharedString::from(m)); if ok { a.set_br_need_key(false); a.invoke_br_search(); } }); });
    }); }
    // ---- datapacks
    { let (u, s_) = (ui.clone(), st.clone()); app.on_dp_add(move || {
        let (u, s_) = (u.clone(), s_.clone());
        bg(move || {
            let paths = pick_files(); if paths.is_empty() { return; }
            let (Some(c), id) = (client(&s_), sid(&s_)) else { return };
            let mut notes = vec![];
            for p in &paths { let name = file_name(p); match std::fs::read(p) { Ok(b) => { if let Err(e) = post_raw(&c, &sc(&c, &id, "/api/dp/upload"), &b, &[("x-filename".into(), enc(&name))]) { notes.push(format!("{name}: {e}")); } } Err(_) => notes.push(format!("{name}: couldn't read it")) } }
            let m = if notes.is_empty() { "Added.".to_string() } else { notes.join("; ") };
            with_ui(&u, move |a| a.set_dp_note(SharedString::from(m)));
            load_dp(&u, &s_);
        });
    }); }
    { let (u, s_) = (ui.clone(), st.clone()); app.on_dp_remove(move |nm| {
        let (nm, u, s_) = (nm.to_string(), u.clone(), s_.clone());
        bg(move || { let (Some(c), id) = (client(&s_), sid(&s_)) else { return }; let r = c.post(&sc(&c, &id, "/api/dp/delete"), &obj(vec![("name", s(&nm))])); let m = msg_of(&r); with_ui(&u, move |a| a.set_dp_note(SharedString::from(m))); load_dp(&u, &s_); });
    }); }
    { let (u, s_) = (ui.clone(), st.clone()); app.on_dp_reload(move || {
        let (u, s_) = (u.clone(), s_.clone());
        bg(move || { let (Some(c), id) = (client(&s_), sid(&s_)) else { return }; let r = c.post(&sc(&c, &id, "/api/dp/reload"), &obj(vec![])); let m = msg_of(&r); with_ui(&u, move |a| a.set_dp_note(SharedString::from(m))); });
    }); }
    // ---- files
    { let (u, s_) = (ui.clone(), st.clone()); app.on_f_open(move |path, dir| {
        let (path, u, s_) = (path.to_string(), u.clone(), s_.clone());
        bg(move || {
            if path == ".." { let cur = s_.lock().unwrap().x.f_path.clone(); let up = cur.rsplit_once('/').map(|x| x.0.to_string()).unwrap_or_default(); files_load(&u, &s_, &up); return; }
            if dir { files_load(&u, &s_, &path); return; }
            let (Some(c), id) = (client(&s_), sid(&s_)) else { return };
            match c.get(&sc(&c, &id, &format!("/api/files/read?path={}", enc(&path)))) {
                Ok(j) if flag(&j, "ok") => {
                    { let mut g = s_.lock().unwrap(); g.x.f_edit_path = path.clone(); g.x.f_edit_mtime = gs(&j, "mtime"); g.x.f_edit_crlf = flag(&j, "crlf"); }
                    let (name, text, ro) = (gs(&j, "name"), gs(&j, "content"), flag(&j, "readonly"));
                    with_ui(&u, move |a| { a.set_f_edit_name(SharedString::from(name)); a.set_f_edit_text(SharedString::from(text)); a.set_f_edit_dirty(false); a.set_f_msg(SharedString::from(if ro { "Too big to edit here, showing the start only." } else { "" })); a.set_f_edit_open(true); });
                }
                Ok(j) => file_msg(&u, &{ let m = gs(&j, "msg"); if m.is_empty() { gs(&j, "error") } else { m } }),
                Err(e) => file_msg(&u, &e.to_string()),
            }
        });
    }); }
    { let (u, s_) = (ui.clone(), st.clone()); app.on_f_filter(move |f| { s_.lock().unwrap().x.f_filter = f.to_string(); let (u, s_) = (u.clone(), s_.clone()); bg(move || files_render(&u, &s_)); }); }
    { let (u, s_) = (ui.clone(), st.clone()); app.on_f_go(move |p| { let (p, u, s_) = (p.to_string(), u.clone(), s_.clone()); bg(move || files_load(&u, &s_, &p)); }); }
    { let (u, s_) = (ui.clone(), st.clone()); app.on_f_refresh(move || { let (u, s_) = (u.clone(), s_.clone()); bg(move || { let p = s_.lock().unwrap().x.f_path.clone(); files_load(&u, &s_, &p); }); }); }
    { let (u, s_) = (ui.clone(), st.clone()); app.on_f_upload(move || {
        let (u, s_) = (u.clone(), s_.clone());
        bg(move || {
            let paths = pick_files(); if paths.is_empty() { return; }
            let (Some(c), id) = (client(&s_), sid(&s_)) else { return };
            let dir = s_.lock().unwrap().x.f_path.clone(); let mut notes = vec![];
            for p in &paths { let name = file_name(p); match std::fs::read(p) { Ok(b) => { file_msg(&u, &format!("Uploading {name}...")); if let Err(e) = post_raw(&c, &sc(&c, &id, "/api/files/upload"), &b, &[("x-path".into(), enc(&dir)), ("x-filename".into(), enc(&name)), ("x-overwrite".into(), "1".into())]) { notes.push(format!("{name}: {e}")); } } Err(_) => notes.push(format!("{name}: couldn't read it")) } }
            file_msg(&u, &if notes.is_empty() { "Uploaded.".to_string() } else { notes.join("; ") });
            files_load(&u, &s_, &dir);
        });
    }); }
    { let (u, s_) = (ui.clone(), st.clone()); app.on_f_new(move |name, dir| {
        let (name, u, s_) = (name.trim().to_string(), u.clone(), s_.clone()); if name.is_empty() { return; }
        bg(move || { let (Some(c), id) = (client(&s_), sid(&s_)) else { return }; let cur = s_.lock().unwrap().x.f_path.clone(); let r = c.post(&sc(&c, &id, "/api/files/new"), &obj(vec![("dir", s(&cur)), ("name", s(&name)), ("kind", s(if dir { "dir" } else { "file" }))])); file_msg(&u, &if ok_of(&r) { String::new() } else { msg_of(&r) }); files_load(&u, &s_, &cur); });
    }); }
    { let (u, s_) = (ui.clone(), st.clone()); app.on_f_rename(move |path, name| {
        let (path, name, u, s_) = (path.to_string(), name.trim().to_string(), u.clone(), s_.clone()); if name.is_empty() { return; }
        bg(move || { let (Some(c), id) = (client(&s_), sid(&s_)) else { return }; let cur = s_.lock().unwrap().x.f_path.clone(); let r = c.post(&sc(&c, &id, "/api/files/rename"), &obj(vec![("path", s(&path)), ("name", s(&name))])); file_msg(&u, &if ok_of(&r) { String::new() } else { msg_of(&r) }); files_load(&u, &s_, &cur); });
    }); }
    { let (u, s_) = (ui.clone(), st.clone()); app.on_f_delete(move |path| {
        let (path, u, s_) = (path.to_string(), u.clone(), s_.clone());
        bg(move || { let (Some(c), id) = (client(&s_), sid(&s_)) else { return }; let cur = s_.lock().unwrap().x.f_path.clone(); let r = c.post(&sc(&c, &id, "/api/files/delete"), &obj(vec![("paths", J::Arr(vec![s(&path)]))])); file_msg(&u, &msg_of(&r)); files_load(&u, &s_, &cur); });
    }); }
    { let (u, s_) = (ui.clone(), st.clone()); app.on_f_download(move |path| {
        let (path, u, s_) = (path.to_string(), u.clone(), s_.clone());
        bg(move || {
            let (Some(c), id) = (client(&s_), sid(&s_)) else { return };
            file_msg(&u, "Downloading...");
            match c.raw("GET", &sc(&c, &id, &format!("/api/files/download?path={}", enc(&path))), None) {
                Ok((200, bytes)) => {
                    let mut name = file_name(&path); if bytes.starts_with(b"PK") && !name.to_lowercase().ends_with(".zip") && !name.contains('.') { name.push_str(".zip"); }
                    let dir = home().join("Downloads"); let _ = std::fs::create_dir_all(&dir);
                    let mut dest = dir.join(&name); let mut k = 1; while dest.exists() { dest = dir.join(format!("{k}-{name}")); k += 1; }
                    file_msg(&u, &match std::fs::write(&dest, bytes) { Ok(_) => format!("Saved to your Downloads folder as {}.", dest.file_name().map(|x| x.to_string_lossy().to_string()).unwrap_or_default()), Err(e) => format!("Couldn't save it: {e}") });
                }
                _ => file_msg(&u, "Couldn't download that."),
            }
        });
    }); }
    { let (u, s_) = (ui.clone(), st.clone()); app.on_f_save(move || {
        let Some(a) = u.upgrade() else { return };
        let text = a.get_f_edit_text().to_string();
        let (path, mt, crlf) = { let g = s_.lock().unwrap(); (g.x.f_edit_path.clone(), g.x.f_edit_mtime.clone(), g.x.f_edit_crlf) };
        let (u, s_) = (u.clone(), s_.clone());
        bg(move || {
            let (Some(c), id) = (client(&s_), sid(&s_)) else { return };
            let r = c.post(&sc(&c, &id, "/api/files/save"), &obj(vec![("path", s(&path)), ("content", s(&text)), ("mtime", s(&mt)), ("crlf", J::Bool(crlf))]));
            if ok_of(&r) { if let Ok(j) = &r { s_.lock().unwrap().x.f_edit_mtime = gs(j, "mtime"); } with_ui(&u, |a| { a.set_f_edit_dirty(false); a.set_f_msg(SharedString::from("Saved. Restart the server for config changes to apply.")); }); }
            else { let m = msg_of(&r); with_ui(&u, move |a| a.set_f_msg(SharedString::from(m))); }
        });
    }); }
    { let (u, s_) = (ui.clone(), st.clone()); app.on_f_close(move || { if let Some(a) = u.upgrade() { a.set_f_edit_open(false); a.set_f_msg(SharedString::new()); } let (u, s_) = (u.clone(), s_.clone()); bg(move || { let p = s_.lock().unwrap().x.f_path.clone(); files_load(&u, &s_, &p); }); }); }
    { let u = ui.clone(); app.on_f_edited(move |_| { if let Some(a) = u.upgrade() { a.set_f_edit_dirty(true); } }); }
    // ---- settings
    { let (u, s_) = (ui.clone(), st.clone()); app.on_set_set_tab(move |t| {
        if let Some(a) = u.upgrade() { a.set_set_tab(t.clone()); }
        let (u, s_) = (u.clone(), s_.clone());
        bg(move || if t.as_str() == "auto" { load_auto(&u, &s_) } else { load_settings(&u, &s_) });
    }); }
    { let (u, s_) = (ui.clone(), st.clone()); app.on_groups_all(move |open| {
        { let mut g = s_.lock().unwrap(); if open { let names: Vec<String> = g.fdata.iter().map(|x| x.0.clone()).collect(); for n_ in names { g.open.insert(n_); } } else { g.open.clear(); } }
        if let Some(a) = u.upgrade() { render_fields(&a, &s_); }
    }); }
    { let (u, s_) = (ui.clone(), st.clone()); app.on_auto_edited(move |k, v| {
        { let mut g = s_.lock().unwrap();
          if k.starts_with("modupd.") { if let Some(m) = g.x.modupd.as_mut() { set_path(m, &k["modupd.".len()..], v.to_string()); } }
          else if let Some(c) = g.x.auto.as_mut() { set_path(c, k.as_str(), v.to_string()); } }
        if let Some(a) = u.upgrade() { a.set_auto_dirty(true); render_auto(&a, &s_); }
    }); }
    { let (u, s_) = (ui.clone(), st.clone()); app.on_auto_group(move |name| {
        { let mut g = s_.lock().unwrap(); if !g.x.auto_open.remove(name.as_str()) { g.x.auto_open.insert(name.to_string()); } }
        if let Some(a) = u.upgrade() { render_auto(&a, &s_); }
    }); }
    { let (u, s_) = (ui.clone(), st.clone()); app.on_auto_act(move |act, key| {
        let (act, key, u, s_) = (act.to_string(), key.to_string(), u.clone(), s_.clone());
        // tasks come from the saved copy on the engine, so "run" needs a saved task
        if act == "btn" && key.starts_with("add:") {
            { let mut g = s_.lock().unwrap(); if let Some(J::Obj(m)) = g.x.auto.as_mut() { if let Some((_, J::Arr(ts))) = m.iter_mut().find(|(k, _)| k == "tasks") { ts.push(new_task(&key["add:".len()..])); } } }
            if let Some(a) = u.upgrade() { a.set_auto_dirty(true); render_auto(&a, &s_); }
            return;
        }
        if act == "del" {
            let idx: Option<usize> = key.split('.').nth(1).and_then(|x| x.parse().ok());
            { let mut g = s_.lock().unwrap(); if let (Some(i), Some(J::Obj(m))) = (idx, g.x.auto.as_mut()) { if let Some((_, J::Arr(ts))) = m.iter_mut().find(|(k, _)| k == "tasks") { if i < ts.len() { ts.remove(i); } } } }
            if let Some(a) = u.upgrade() { a.set_auto_dirty(true); render_auto(&a, &s_); }
            return;
        }
        bg(move || {
            let (Some(c), id) = (client(&s_), sid(&s_)) else { return };
            let r = if act == "run" {
                let tid = { let g = s_.lock().unwrap(); let idx: Option<usize> = key.split('.').nth(1).and_then(|x| x.parse().ok()); idx.and_then(|i| g.x.auto.as_ref().and_then(|c| arr(c, "tasks").get(i).map(|t| gs(t, "id")))).unwrap_or_default() };
                c.post(&sc(&c, &id, "/api/auto/run"), &obj(vec![("id", s(&tid))]))
            } else if key == "discord-test" {
                let url = { let g = s_.lock().unwrap(); g.x.auto.as_ref().map(|c| pv(c, "discord.url")).unwrap_or_default() };
                c.post(&sc(&c, &id, "/api/auto/discord-test"), &obj(vec![("url", s(&url))]))
            } else { return };
            let m = msg_of(&r); with_ui(&u, move |a| a.set_auto_msg(SharedString::from(m)));
        });
    }); }
    { let (u, s_) = (ui.clone(), st.clone()); app.on_auto_discard(move || { let (u, s_) = (u.clone(), s_.clone()); bg(move || load_auto(&u, &s_)); }); }
    { let (u, s_) = (ui.clone(), st.clone()); app.on_auto_save(move || {
        let (u, s_) = (u.clone(), s_.clone());
        bg(move || {
            let (Some(c), id) = (client(&s_), sid(&s_)) else { return };
            let (cfg, mu) = { let g = s_.lock().unwrap(); (g.x.auto.clone(), g.x.modupd.clone()) };
            let (Some(cfg), Some(mu)) = (cfg, mu) else { return };
            let r1 = c.post(&sc(&c, &id, "/api/auto/save"), &cfg);
            let body = obj(vec![("mode", s(&pv(&mu, "mode"))), ("time", s(&pv(&mu, "time"))), ("install", J::Bool(pv(&mu, "install") == "true")), ("skip", J::Arr(split_ids(&pv(&mu, "skip")).iter().map(|x| s(x)).collect()))]);
            let r2 = c.post(&sc(&c, &id, "/api/modupd"), &body);
            let ok = ok_of(&r1) && ok_of(&r2);
            let m = if ok { "Saved.".to_string() } else if !ok_of(&r1) { msg_of(&r1) } else { msg_of(&r2) };
            with_ui(&u, move |a| a.set_auto_msg(SharedString::from(m)));
            if ok { load_auto(&u, &s_); }
        });
    }); }
}
pub fn downloads_dir() -> std::path::PathBuf { let d = home().join("Downloads"); let _ = std::fs::create_dir_all(&d); d }

pub fn pick_folder() -> Option<String> {
    #[cfg(target_os = "macos")]
    let out = std::process::Command::new("osascript").arg("-e").arg("POSIX path of (choose folder)").output();
    #[cfg(windows)]
    let out = std::process::Command::new("powershell").args(["-NoProfile", "-Command", "Add-Type -AssemblyName System.Windows.Forms; $d = New-Object System.Windows.Forms.FolderBrowserDialog; if ($d.ShowDialog() -eq 'OK') { $d.SelectedPath }"]).output();
    #[cfg(all(unix, not(target_os = "macos")))]
    let out = std::process::Command::new("zenity").args(["--file-selection", "--directory"]).output();
    out.ok().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).filter(|s| !s.is_empty())
}
