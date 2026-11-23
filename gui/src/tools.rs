//! Tools tab: server software, Java, memory advice, disk use, network check, AutoModpack, world tools, world import.
use crate::*;
use create_panel::client::Client;
use std::time::Duration;

struct Row { kind: i32, key: String, label: String, help: String, value: String }
fn r(kind: i32, key: &str, label: &str, help: &str, value: &str) -> Row { Row { kind, key: key.into(), label: label.into(), help: help.into(), value: value.into() } }
fn h(label: &str, help: &str) -> Row { r(10, "", label, help, "") }
fn info(label: &str, value: &str) -> Row { r(7, "", label, "", value) }
fn infoh(label: &str, help: &str, value: &str) -> Row { r(7, "", label, help, value) }
fn btn(key: &str, label: &str, help: &str, b: &str) -> Row { r(8, key, label, help, b) }
fn to_field(x: Row) -> Field { Field { key: x.key.into(), label: x.label.into(), help: x.help.into(), kind: x.kind, value: x.value.into(), options: ModelRc::default(), count: 0, open: false } }
fn jb(j: &J, k: &str) -> bool { matches!(j.get(k), Some(J::Bool(true))) }

fn build(c: &Client, id: &str, path: &str, cmp: &[(String, String, String)]) -> Vec<Row> {
    let get = |p: &str| c.get(&sc(c, id, p)).ok();
    let mut v: Vec<Row> = vec![];
    // ---- server software
    v.push(h("Server software", "What the server runs on."));
    match get("/api/loader/info") {
        Some(j) if jb(&j, "supported") => {
            let mc = gs(&j, "mc");
            v.push(info("Running", &format!("{} {}", gs(&j, "name"), gs(&j, "label"))));
            v.push(btn("loader", &format!("Update {} to the newest version", gs(&j, "name")), &format!("Keeps Minecraft {mc}. Makes a backup first. The server must be stopped."), "Update"));
            v.push(btn("loaderpick", "Choose a specific version", "Pick the Minecraft version and the exact loader build.", "Choose..."));
        }
        Some(j) => { let l = gs(&j, "label"); v.push(info("Running", if l.is_empty() { "NeoForge (or another type)" } else { &l })); v.push(infoh("Updating", "NeoForge is updated from the Overview page (Update NeoForge).", "see Overview")); }
        None => v.push(info("Running", "unknown")),
    }
    // ---- java
    v.push(h("Java", "The Java the server starts with."));
    if let Some(j) = get("/api/java/info") {
        let need = gn(&j, "need"); let cur = j.get("current").cloned().unwrap_or(J::Null);
        v.push(info("Minecraft needs", &if need > 0 { format!("Java {need} or newer ({})", gs(&j, "mc")) } else { "unknown".into() }));
        v.push(info("Using now", &if matches!(cur, J::Obj(_)) { format!("Java {}", gn(&cur, "major")) } else { "the system Java".into() }));
        if need > 0 { v.push(btn(&format!("java:{need}"), &format!("Download Java {need} and use it for this server"), "Stored privately next to the panel. Nothing is installed system-wide.", "Download")); }
        for it in arr(&j, "installed") { let p = gs(&it, "path"); v.push(btn(&format!("usej|{p}"), &format!("Java {} {}{}", gn(&it, "major"), gs(&it, "kind"), if jb(&it, "private") { " (downloaded by the panel)" } else { "" }), &p, "Use this")); }
    }
    // ---- storage
    v.push(h("Disk use", "Where this server's space goes."));
    if let Some(j) = get("/api/storage") {
        for it in arr(&j, "items") { if let J::Arr(a) = it { let nm = a.first().and_then(|x| x.as_str()).unwrap_or(""); let sz = match a.get(1) { Some(J::Num(x)) => *x as i64, _ => 0 }; v.push(info(nm, &mb(sz))); } }
        v.push(info("Total", &mb(gn(&j, "total"))));
        v.push(btn("clean:logs", "Delete old logs", "Keeps the current log and anything from the last 7 days.", "Clean"));
        v.push(btn("clean:crash", "Delete crash reports", "Safe to remove once you've read them.", "Clean"));
    }
    // ---- network
    v.push(h("Network check", "Can friends on your network reach this server?"));
    if let Some(j) = get("/api/netcheck") {
        let ips: Vec<String> = arr(&j, "ips").iter().filter_map(|x| x.as_str().map(|t| t.to_string())).collect();
        v.push(info("Addresses on your network", &if ips.is_empty() { "none found".into() } else { ips.join(", ") }));
        if let Some(fw) = j.get("fw").filter(|f| matches!(f, J::Obj(_))) {
            v.push(info("Mac firewall", if jb(fw, "ok") { "Fine, Java is allowed in" } else { "May be blocking Java" }));
            if !jb(fw, "ok") { v.push(btn("fw", "Let Java through the firewall", "Asks for your Mac password once.", "Fix it")); }
        }
        if jb(&j, "netmask_bad") { v.push(infoh("Network warning", &gs(&j, "netmask_help"), "check it")); }
    }
    // ---- automodpack
    v.push(h("AutoModpack", "Sends the right mods to players when they join."));
    match get("/api/am/status") {
        Some(j) if jb(&j, "installed") => {
            v.push(info("Mods in the player pack", &gn(&j, "sent").to_string()));
            let l = j.get("last").cloned().unwrap_or(J::Null);
            if !gs(&l, "msg").is_empty() { v.push(info("Last sync", &gs(&l, "msg"))); }
            v.push(btn("am", "Sync the pack now", "Pushes your current mods to what players download.", "Sync"));
        }
        _ => v.push(info("Status", "Not installed on this server (find it in Mods > Browse)")),
    }
    // ---- world tools
    v.push(h("World tools", "Needs the server running."));
    for (k, l, hp) in [("day", "Set time to day", ""), ("noon", "Set time to noon", ""), ("night", "Set time to night", ""), ("midnight", "Set time to midnight", ""), ("clear", "Clear weather", ""), ("rain", "Make it rain", ""), ("thunder", "Make a thunderstorm", ""),
        ("peaceful", "Difficulty: peaceful", ""), ("easy", "Difficulty: easy", ""), ("normal", "Difficulty: normal", ""), ("hard", "Difficulty: hard", ""), ("clear_items", "Remove all dropped items", "Cleans up lag from item piles."), ("save", "Save the world now", ""),
        ("wl_on", "Whitelist on", ""), ("wl_off", "Whitelist off", ""), ("wl_reload", "Reload the whitelist", "")] { v.push(btn(&format!("w:{k}"), l, hp, "Run")); }
    // ---- scheduled mod updates
    v.push(h("Scheduled mod updates", "Set the schedule on Settings > Automation & updates."));
    v.push(btn("mu-now", "Install the waiting updates now", "Installs what the last check found, safely (with a backup and a rollback if it fails).", "Install"));
    // ---- compare with the launcher
    v.push(h("Compare with your launcher", "Check your own game's mods against the server's, so players aren't missing anything."));
    v.push(btn("cmp", "Choose your launcher's mods folder", "Prism, CurseForge, Modrinth App... pick the folder with the .jar files.", "Choose..."));
    for (k, a, b) in cmp { v.push(if k == "h" { h(a, b) } else { info(a, b) }); }
    // ---- import a world
    v.push(h("Import a world", "Replaces this server's world. The old one is kept beside it. The server must be stopped."));
    v.push(r(0, "imp-path", "Folder or .zip on the computer running the engine", "For big worlds. Paste the full path.", path));
    v.push(btn("imp-go", "Import from that path", "", "Import"));
    v.push(btn("imp-up", "Upload a .zip from this computer", "Up to 1 GB.", "Choose..."));
    v
}

pub fn load(ui: &Weak<App>, st: &Shared) {
    let (Some(c), id) = (client(st), st.lock().unwrap().sel.id.clone()) else { return };
    if id.is_empty() { return; }
    let (path, cmp) = { let g = st.lock().unwrap(); (g.x.tools_path.clone(), g.x.cmp.clone()) };
    let rows = build(&c, &id, &path, &cmp);
    load_mem(ui, &c, &id);
    with_ui(ui, move |a| { let fl: Vec<crate::Field> = rows.into_iter().map(to_field).collect(); a.set_tools_h(crate::rows_h(&fl)); a.set_tools(ModelRc::new(VecModel::from(fl))); });
}

fn load_mem(ui: &Weak<App>, c: &Client, id: &str) {
    let gbs = |mb: i64| { let g = mb as f64 / 1024.0; if (g - g.round()).abs() < 0.05 { format!("{}", g.round() as i64) } else { format!("{:.1}", g) } };
    let Ok(j) = c.get(&sc(c, id, "/api/recommend")) else { with_ui(ui, |a| a.set_mem_ok(false)); return };
    let (mac, rec, jvm) = (j.get("mac").cloned().unwrap_or(J::Null), j.get("recommend").cloned().unwrap_or(J::Null), j.get("jvm").cloned().unwrap_or(J::Null));
    if !matches!(rec, J::Obj(_)) { with_ui(ui, |a| a.set_mem_ok(false)); return; }
    let rg = match rec.get("max_gb") { Some(J::Num(x)) => *x, _ => 0.0 };
    let total = gn(&mac, "total_mb"); let free = gn(&mac, "avail_mb"); let cores = gn(&mac, "cores");
    let mut chip = { let c = gs(&mac, "chip"); if c.is_empty() { let m = gs(&mac, "model"); if m.is_empty() { "This computer".to_string() } else { m } } else { c } };
    if total > 0 { chip.push_str(&format!("  ·  {} GB RAM", gbs(total))); }
    if cores > 0 { chip.push_str(&format!("  ·  {cores} cores")); }
    if free > 0 { chip.push_str(&format!("  ·  about {} GB free now", gbs(free))); }
    let xmx = gn(&jvm, "xmx_mb");
    let cur = if xmx > 0 { format!("set to {} GB right now", gbs(xmx)) } else { "no memory limit set yet".to_string() };
    let why: Vec<String> = arr(&rec, "reasons").iter().filter_map(|x| x.as_str().map(|t| t.to_string())).collect();
    let frac = if total > 0 { ((rg * 1024.0) / total as f64).clamp(0.0, 1.0) as f32 } else { 0.0 };
    let curfrac = if total > 0 && xmx > 0 { (xmx as f64 / total as f64).clamp(0.0, 1.0) as f32 } else { 0.0 };
    let rec_txt = format!("Recommended: {} GB", if rg == rg.trunc() { format!("{}", rg as i64) } else { format!("{rg}") });
    let mods = format!("{} mods, {} MB of jars", gn(&rec, "mod_count"), gn(&rec, "mod_mb"));
    with_ui(ui, move |a| { a.set_mem_ok(true); a.set_mem_chip(SharedString::from(chip)); a.set_mem_rec(SharedString::from(rec_txt)); a.set_mem_cur(SharedString::from(cur)); a.set_mem_why(SharedString::from(why.join("  "))); a.set_mem_frac(frac); a.set_mem_curfrac(curfrac); a.set_mem_mods(SharedString::from(mods)); });
}

fn note(ui: &Weak<App>, t: &str) { let t = t.to_string(); with_ui(ui, move |a| { a.set_mods_note(SharedString::from(t.clone())); a.set_note(SharedString::from(t)); }); }

pub fn wait_job(c: &Client, ui: &Weak<App>, job: &str) -> (bool, String) {
    for _ in 0..1800 {
        thread::sleep(Duration::from_secs(2));
        let Ok(j) = c.get(&format!("/api/hub/job?id={job}")) else { continue };
        let pct = gn(&j, "pct"); let step = gs(&j, "step");
        if matches!(j.get("done"), Some(J::Bool(true))) { return (jb(&j, "ok"), gs(&j, "msg")); }
        note(ui, &format!("{step} {pct}%"));
    }
    (false, "Timed out waiting.".into())
}

pub fn act(ui: &Weak<App>, st: &Shared, key: String) {
    let (ui, st) = (ui.clone(), st.clone());
    bg(move || {
        let (Some(c), id) = (client(&st), st.lock().unwrap().sel.id.clone()) else { return };
        if id.is_empty() { return; }
        let post = |p: &str, b: J| c.post(&sc(&c, &id, p), &b);
        let done = |ui: &Weak<App>, st: &Shared, m: &str| { note(ui, m); load(ui, st); };
        if key == "loaderpick" { return loaderpick::open(&ui, &st); }
        if key == "loader" {
            let r = post("/api/loader/update", obj(vec![])); if !ok_of(&r) { return done(&ui, &st, &msg_of(&r)); }
            for _ in 0..900 { thread::sleep(Duration::from_secs(2)); let Ok(j) = c.get(&sc(&c, &id, "/api/loader/status")) else { continue }; let m = gs(&j, "msg"); if gs(&j, "phase") != "running" { return done(&ui, &st, &m); } note(&ui, &m); }
            done(&ui, &st, "Still working. Check back on the Console.");
        } else if let Some(v) = key.strip_prefix("java:") {
            let r = post("/api/java/install", obj(vec![("version", n(v.parse().unwrap_or(0))), ("use", J::Bool(true))])); if !ok_of(&r) { return done(&ui, &st, &msg_of(&r)); }
            for _ in 0..900 { thread::sleep(Duration::from_secs(2)); let Ok(j) = c.get(&sc(&c, &id, "/api/java/status")) else { continue }; if gs(&j, "phase") != "running" { return done(&ui, &st, &gs(&j, "msg")); } note(&ui, &format!("{} {}%", gs(&j, "step"), gn(&j, "pct"))); }
            done(&ui, &st, "Still downloading.");
        } else if let Some(p) = key.strip_prefix("usej|") {
            let r = post("/api/java/use", obj(vec![("path", s(p))])); done(&ui, &st, &msg_of(&r));
        } else if let Some(w) = key.strip_prefix("clean:") {
            let r = post("/api/storage/clean", obj(vec![("what", s(w))])); done(&ui, &st, &msg_of(&r));
        } else if key == "fw" { let r = post("/api/netfix", obj(vec![])); done(&ui, &st, &msg_of(&r)); }
        else if key == "cmp" {
            let Some(dir) = extra::pick_folder() else { return };
            note(&ui, "Reading your launcher's mods...");
            let mut mods = vec![];
            if let Ok(rd) = std::fs::read_dir(&dir) { for e in rd.flatten() { let nm = e.file_name().to_string_lossy().to_string(); if nm.to_lowercase().ends_with(".jar") || nm.to_lowercase().ends_with(".jar.disabled") { if let Some(h) = create_panel::sys::sha1_file(&e.path()) { mods.push(obj(vec![("sha1", s(&h)), ("name", s(&nm))])); } } } }
            if mods.is_empty() { return note(&ui, "I didn't find any .jar files in that folder."); }
            note(&ui, "Comparing with Modrinth (this can take a minute)...");
            let r = post("/api/mods/compare", obj(vec![("mods", J::Arr(mods))])); if !ok_of(&r) { return done(&ui, &st, &msg_of(&r)); }
            let j = r.unwrap_or(J::Null); let mut rows: Vec<(String, String, String)> = vec![];
            let missing: Vec<String> = arr(&j, "missing").iter().map(|x| gs(x, "name")).collect();
            let srv_need: Vec<String> = arr(&j, "server").iter().filter(|x| gs(x, "status") == "only").map(|x| gs(x, "name")).collect();
            rows.push(("h".into(), "Result".into(), format!("Server runs {}", gs(&j, "env"))));
            rows.push(("i".into(), format!("In your launcher but missing on the server ({})", missing.len()), if missing.is_empty() { "none".into() } else { missing.join(", ") }));
            rows.push(("i".into(), format!("On the server but missing in your launcher ({})", srv_need.len()), if srv_need.is_empty() { "none".into() } else { srv_need.join(", ") }));
            st.lock().unwrap().x.cmp = rows; done(&ui, &st, "Compared.");
        }
        else if key == "mu-now" { let r = post("/api/modupd/install-now", obj(vec![])); done(&ui, &st, &msg_of(&r)); }
        else if key == "am" { let r = post("/api/am/sync", obj(vec![])); done(&ui, &st, &msg_of(&r)); }
        else if let Some(w) = key.strip_prefix("w:") { let r = post("/api/world/do", obj(vec![("action", s(w))])); note(&ui, &msg_of(&r)); }
        else if key == "imp-go" || key == "imp-up" {
            let mut body = vec![("id", s(&id))];
            if key == "imp-go" {
                let p = st.lock().unwrap().x.tools_path.trim().to_string(); if p.is_empty() { return note(&ui, "Paste the folder or .zip path first."); }
                body.push(("path", s(&p)));
            } else {
                let Some(f) = extra::pick_files().into_iter().next() else { return };
                note(&ui, "Uploading the world...");
                let Ok(data) = std::fs::read(&f) else { return note(&ui, "Couldn't read that file."); };
                let hdr = vec![("x-filename".to_string(), extra::file_name(&f))];
                if let Err(e) = extra::post_raw(&c, &format!("/api/hub/world/upload?id={}", extra::enc(&id)), &data, &hdr) { return note(&ui, &e); }
            }
            let r = c.post("/api/hub/world/commit", &obj(body)); if !ok_of(&r) { return note(&ui, &msg_of(&r)); }
            let job = match &r { Ok(j) => gs(j, "job"), _ => String::new() };
            let (_, m) = wait_job(&c, &ui, &job); done(&ui, &st, &m);
        }
    });
}
pub fn edit(st: &Shared, key: &str, v: &str) { if key == "imp-path" { st.lock().unwrap().x.tools_path = v.to_string(); } }
