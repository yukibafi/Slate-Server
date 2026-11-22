//! Modpack picker (Modrinth), add-existing-folder, keep-Mac-awake (lid) and quit.
use crate::*;

fn jb(j: &J, k: &str) -> bool { matches!(j.get(k), Some(J::Bool(true))) }
fn pk_msg(u: &Weak<App>, m: &str) { let m = m.to_string(); with_ui(u, move |a| a.set_pk_msg(SharedString::from(m))); }
fn big(n: i64) -> String { if n >= 1_000_000 { format!("{:.1}M", n as f64 / 1e6) } else if n >= 1000 { format!("{}K", n / 1000) } else { n.to_string() } }

pub fn lid_refresh(ui: &Weak<App>, st: &Shared) {
    let Some(c) = client(st) else { return };
    let key = c.get("/api/hub/key").ok();
    let lid = c.get("/api/lid/status").ok();
    with_ui(ui, move |a| {
        if let Some(k) = key { a.set_remote_url(SharedString::from(gs(&k, "url"))); }
        if let Some(l) = lid { a.set_lid_supported(jb(&l, "supported")); a.set_lid_enabled(jb(&l, "enabled")); a.set_lid_helper(jb(&l, "helper")); }
    });
}

pub fn wire(app: &App, ui: &Weak<App>, st: &Shared) {
    // ---- modpack search / choose / go
    { let (u, s_) = (ui.clone(), st.clone()); app.on_pk_search(move |q| {
        let (q, u, s_) = (q.to_string(), u.clone(), s_.clone());
        bg(move || { let Some(c) = client(&s_) else { return };
            pk_msg(&u, "Searching...");
            let Ok(j) = c.get(&format!("/api/hub/packs/search?q={}&sort=downloads", extra::enc(&q))) else { return pk_msg(&u, "Couldn't reach Modrinth.") };
            if !jb(&j, "ok") { return pk_msg(&u, &gs(&j, "error")); }
            let rows: Vec<(String, String, String, String, String, String)> = arr(&j, "results").iter().map(|r| (gs(r, "id"), gs(r, "name"), gs(r, "summary"), gs(r, "author"), big(gn(r, "downloads")), gs(r, "mc"))).collect();
            let n_ = rows.len();
            with_ui(&u, move |a| { a.set_pk_results(ModelRc::new(VecModel::from(rows.into_iter().map(|r| PackRow { id: r.0.into(), name: r.1.into(), summary: r.2.into(), author: r.3.into(), downloads: r.4.into(), mc: r.5.into() }).collect::<Vec<_>>()))); a.set_pk_msg(SharedString::from(if n_ == 0 { "No modpacks found." } else { "" })); });
        });
    }); }
    { let (u, s_) = (ui.clone(), st.clone()); app.on_pk_choose(move |id, name| {
        let (id, name, u, s_) = (id.to_string(), name.to_string(), u.clone(), s_.clone());
        bg(move || { let Some(c) = client(&s_) else { return };
            pk_msg(&u, "Loading versions...");
            // versions come from the hub for a new server, or the server itself when adding to one
            let sid = s_.lock().unwrap().sel.id.clone(); let mode = u.upgrade().map(|a| a.get_pk_mode().to_string()).unwrap_or_default();
            let path = if mode == "apply" && !sid.is_empty() { format!("/api/s/{sid}/api/packs/versions?id={id}") } else { format!("/api/hub/packs/versions?id={id}") };
            let Ok(j) = c.get(&path) else { return pk_msg(&u, "Couldn't load versions.") };
            if !jb(&j, "ok") { return pk_msg(&u, &gs(&j, "error")); }
            let vs: Vec<(String, String)> = arr(&j, "versions").iter().filter(|v| jb(v, "supported")).map(|v| { let mc: Vec<String> = arr(v, "mc").iter().filter_map(|x| x.as_str().map(|t| t.to_string())).collect(); (gs(v, "id"), format!("{}  (Minecraft {}, {})", gs(v, "version"), mc.join("/"), gs(v, "type"))) }).collect();
            if vs.is_empty() { return pk_msg(&u, "That pack has no versions this server can use."); }
            { let mut g = s_.lock().unwrap(); g.x.pk_vers = vs.iter().map(|x| x.0.clone()).collect(); }
            let labels: Vec<SharedString> = vs.into_iter().map(|x| SharedString::from(x.1)).collect();
            with_ui(&u, move |a| { a.set_pk_sel_name(SharedString::from(name.clone())); a.set_pk_versions(ModelRc::new(VecModel::from(labels))); a.set_pk_ver_index(0); if a.get_pk_name().is_empty() { a.set_pk_name(SharedString::from(name)); } a.set_pk_msg(SharedString::new()); });
        });
    }); }
    { let (u, s_) = (ui.clone(), st.clone()); app.on_pk_go(move || {
        let Some(a) = u.upgrade() else { return };
        let (mode, idx, name, ram) = (a.get_pk_mode().to_string(), a.get_pk_ver_index().max(0) as usize, a.get_pk_name().to_string(), a.get_pk_ram().to_string());
        let vid = s_.lock().unwrap().x.pk_vers.get(idx).cloned().unwrap_or_default(); if vid.is_empty() { return; }
        a.set_pk_busy(true); let (u, s_) = (u.clone(), s_.clone());
        bg(move || { let Some(c) = client(&s_) else { return };
            let fin = |u: &Weak<App>, m: String, ok: bool, to: &'static str| with_ui(u, move |a| { a.set_pk_busy(false); a.set_pk_msg(SharedString::from(m)); if ok { a.set_page(SharedString::from(to)); } });
            if mode == "new" {
                if name.trim().is_empty() { return fin(&u, "Give the server a name.".into(), false, "hub"); }
                pk_msg(&u, "Starting...");
                let r = c.post("/api/hub/create", &obj(vec![("name", s(&name)), ("pack", s(&vid)), ("ram", n(ram.trim().parse::<i64>().unwrap_or(8))), ("eula", J::Bool(true))]));
                if !ok_of(&r) { return fin(&u, msg_of(&r), false, "hub"); }
                let job = match &r { Ok(j) => gs(j, "job"), _ => String::new() };
                let (ok, m) = tools::wait_job(&c, &u, &job);
                with_ui(&u, move |a| a.set_note(SharedString::new()));
                fin(&u, m, ok, "hub");
            } else {
                let sid = s_.lock().unwrap().sel.id.clone();
                let r = c.post(&sc(&c, &sid, "/api/packs/apply"), &obj(vec![("version", s(&vid))]));
                if !ok_of(&r) { return fin(&u, msg_of(&r), false, "server"); }
                for _ in 0..1800 { thread::sleep(Duration::from_secs(2)); let Ok(j) = c.get(&sc(&c, &sid, "/api/packs/status")) else { continue };
                    if gs(&j, "phase") != "running" { let ok = jb(&j, "ok"); let m = gs(&j, "msg"); return fin(&u, if m.is_empty() { "Done.".into() } else { m }, ok, "server"); }
                    pk_msg(&u, &format!("{} {}%", gs(&j, "step"), gn(&j, "pct"))); }
                fin(&u, "Still working in the background.".into(), false, "server");
            }
        });
    }); }
    // ---- add existing folder
    { let (u, s_) = (ui.clone(), st.clone()); app.on_add_existing(move |dir, name| {
        let (dir, name, u, s_) = (dir.trim().to_string(), name.trim().to_string(), u.clone(), s_.clone());
        if dir.is_empty() { with_ui(&u, |a| a.set_job_text(SharedString::from("Enter the folder path."))); return; }
        bg(move || { let Some(c) = client(&s_) else { return };
            let r = c.post("/api/hub/add", &obj(vec![("dir", s(&dir)), ("name", s(&name))])); let ok = ok_of(&r); let m = msg_of(&r);
            with_ui(&u, move |a| { a.set_job_text(SharedString::from(m.clone())); if ok { a.set_page(SharedString::from("hub")); a.set_note(SharedString::from(m)); } });
        });
    }); }
    // ---- lid / quit
    { let (u, s_) = (ui.clone(), st.clone()); app.on_lid_refresh(move || { let (u, s_) = (u.clone(), s_.clone()); bg(move || lid_refresh(&u, &s_)); }); }
    { let (u, s_) = (ui.clone(), st.clone()); app.on_lid_setup(move || { let (u, s_) = (u.clone(), s_.clone()); bg(move || { let Some(c) = client(&s_) else { return }; with_ui(&u, |a| a.set_lid_msg(SharedString::from("Waiting for your Mac password..."))); let r = c.post("/api/lid/setup", &obj(vec![])); let m = msg_of(&r); with_ui(&u, move |a| a.set_lid_msg(SharedString::from(m))); lid_refresh(&u, &s_); }); }); }
    { let (u, s_) = (ui.clone(), st.clone()); app.on_lid_set(move |on| { let (u, s_) = (u.clone(), s_.clone()); bg(move || { let Some(c) = client(&s_) else { return }; let r = c.post("/api/lid/set", &obj(vec![("enabled", J::Bool(on))])); let m = msg_of(&r); with_ui(&u, move |a| a.set_lid_msg(SharedString::from(m))); lid_refresh(&u, &s_); }); }); }
    { let s_ = st.clone(); app.on_quit_engine(move || { let s_ = s_.clone(); bg(move || { if let Some(c) = client(&s_) { let _ = c.post("/api/quit", &obj(vec![])); } thread::sleep(Duration::from_millis(600)); let _ = slint::invoke_from_event_loop(|| { let _ = slint::quit_event_loop(); }); }); }); }
}

// ---------------------------------------------------------------- first-run setup guide
fn wz_msg(u: &Weak<App>, m: &str) { let m = m.to_string(); with_ui(u, move |a| a.set_wz_msg(SharedString::from(m))); }

pub fn wizard_load(ui: &Weak<App>, st: &Shared, open: bool) {
    let Some(c) = client(st) else { return };
    let Ok(j) = c.get("/api/hub/setup") else { return };
    let first = jb(&j, "first_run");
    if open == false && !first { return; }
    let majors: Vec<String> = arr(&j, "java").iter().filter_map(|x| if let J::Num(f) = x { Some(format!("{}", *f as i64)) } else { None }).collect();
    let info = if majors.is_empty() { "No Java found yet. Download one below (the panel keeps its own copy; nothing is installed system-wide).".to_string() } else { format!("Java found: {}. You can download more if a server needs a different one.", majors.join(", ")) };
    let (mac, brew) = (jb(&j, "mac"), jb(&j, "brew"));
    let has21 = majors.iter().any(|m| m == "21");
    let bundles: Vec<(String, String, String)> = c.get("/api/hub/migrate/find").map(|b| arr(&b, "bundles").iter().map(|x| (gs(x, "path"), gs(x, "name"), mb(gn(x, "size")))).collect()).unwrap_or_default();
    with_ui(ui, move |a| {
        a.set_wz_info(SharedString::from(info)); a.set_wz_mac(mac); a.set_wz_brew(brew); a.set_wz_j21(!has21); a.set_wz_j17(false); a.set_wz_busy(false); a.set_wz_msg(SharedString::new());
        a.set_bundles(ModelRc::new(VecModel::from(bundles.into_iter().map(|b| BundleRow { path: b.0.into(), name: b.1.into(), size: b.2.into() }).collect::<Vec<_>>())));
        if a.get_page().as_str() == "hub" || open { a.set_page(SharedString::from("wizard")); }
    });
}

pub fn wire_wizard(app: &App, ui: &Weak<App>, st: &Shared) {
    { let (u, s_) = (ui.clone(), st.clone()); app.on_wz_open(move || { let (u, s_) = (u.clone(), s_.clone()); bg(move || wizard_load(&u, &s_, true)); }); }
    { let (u, s_) = (ui.clone(), st.clone()); app.on_wz_java(move || {
        let Some(a) = u.upgrade() else { return };
        let mut vs: Vec<J> = vec![]; if a.get_wz_j21() { vs.push(n(21)); } if a.get_wz_j17() { vs.push(n(17)); }
        if vs.is_empty() { a.set_wz_msg(SharedString::from("Tick at least one Java version.")); return; }
        a.set_wz_busy(true); let (u, s_) = (u.clone(), s_.clone());
        bg(move || { let Some(c) = client(&s_) else { return };
            wz_msg(&u, "Starting...");
            let r = c.post("/api/hub/java/install", &obj(vec![("versions", J::Arr(vs))]));
            if !ok_of(&r) { with_ui(&u, |a| a.set_wz_busy(false)); return wz_msg(&u, &msg_of(&r)); }
            let job = match &r { Ok(j) => gs(j, "job"), _ => String::new() };
            let (_ok, m) = tools::wait_job(&c, &u, &job);
            with_ui(&u, |a| a.set_note(SharedString::new()));
            wz_msg(&u, &m); wizard_load(&u, &s_, true);
        });
    }); }
    { let (u, s_) = (ui.clone(), st.clone()); app.on_wz_brew_install(move || { let (u, s_) = (u.clone(), s_.clone());
        bg(move || { let Some(c) = client(&s_) else { return }; let r = c.post("/api/hub/brew/install", &obj(vec![])); wz_msg(&u, &msg_of(&r));
            for _ in 0..400 { thread::sleep(Duration::from_secs(3)); if c.get("/api/hub/setup").map(|j| jb(&j, "brew")).unwrap_or(false) { wizard_load(&u, &s_, true); wz_msg(&u, "Homebrew is installed."); return; } } });
    }); }
    { let (u, s_) = (ui.clone(), st.clone()); app.on_wz_finish(move || { let (u, s_) = (u.clone(), s_.clone());
        with_ui(&u, |a| a.set_page(SharedString::from("hub")));
        bg(move || { if let Some(c) = client(&s_) { let _ = c.post("/api/hub/setup/done", &obj(vec![])); } });
    }); }
}

// ---------------------------------------------------------------- update the engine from a file
fn eng_msg(u: &Weak<App>, m: &str) { let m = m.to_string(); with_ui(u, move |a| a.set_eng_msg(SharedString::from(m))); }

pub fn wire_engine(app: &App, ui: &Weak<App>, st: &Shared) {
    let (u, s_) = (ui.clone(), st.clone());
    app.on_engine_upgrade(move || {
        let (u, s_) = (u.clone(), s_.clone());
        bg(move || {
            let Some(c) = client(&s_) else { return };
            let Some(path) = extra::pick_files().into_iter().next() else { return };
            let bytes = match std::fs::read(&path) { Ok(b) => b, Err(e) => return eng_msg(&u, &format!("Couldn't read that file: {e}")) };
            eng_msg(&u, "Checking the file...");
            let chk = match extra::post_raw(&c, "/api/panel/upgrade?check=1", &bytes, &[]) { Ok(j) => j, Err(e) => return eng_msg(&u, &e) };
            let want = gs(&chk, "version");
            eng_msg(&u, &format!("Installing {want} (the engine restarts, any running server is saved and stopped first)..."));
            if let Err(e) = extra::post_raw(&c, "/api/panel/upgrade", &bytes, &[]) { return eng_msg(&u, &e); }
            for _ in 0..60 {
                thread::sleep(Duration::from_secs(2));
                if let Ok(j) = c.get("/api/ping") { if gs(&j, "version") == want { return eng_msg(&u, &format!("Updated to {want}. The app window updates itself a few seconds later: close this app and open it again to use the new version.")); } }
            }
            eng_msg(&u, "The engine didn't answer in time. If it rolled back, the old version is still running.");
        });
    });
}
