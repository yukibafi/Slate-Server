//! Loader version picker: choose the Minecraft version and (Fabric/Forge) the exact loader build.
use crate::*;

fn jb(j: &J, k: &str) -> bool { matches!(j.get(k), Some(J::Bool(true))) }
fn vkey(v: &str) -> Vec<u32> { v.split('.').map(|x| x.parse().unwrap_or(0)).collect() }
fn lp_msg(u: &Weak<App>, m: &str) { let m = m.to_string(); with_ui(u, move |a| a.set_lp_msg(SharedString::from(m))); }
fn strs(v: &[String]) -> ModelRc<SharedString> { ModelRc::new(VecModel::from(v.iter().map(|x| SharedString::from(x.as_str())).collect::<Vec<_>>())) }

fn note_for(cur: &str, pick: &str) -> String {
    if cur.is_empty() || pick == cur { return "Same Minecraft version: this just installs the chosen build.".into(); }
    if vkey(pick) > vkey(cur) { format!("You are moving up from Minecraft {cur}. Your mods must support {pick}, and a world saved on the newer version can't be opened on the older one again.") }
    else { format!("That is older than Minecraft {cur}. A world played on a newer version can break on an older one. Only do this with a backup.") }
}

fn load_builds(u: &Weak<App>, st: &Shared, mc: String) {
    let (u, st) = (u.clone(), st.clone());
    bg(move || {
        let (Some(c), id, ld) = (client(&st), st.lock().unwrap().sel.id.clone(), st.lock().unwrap().x.lp_loader.clone()) else { return };
        if ld != "fabric" && ld != "forge" { return; }
        let r = c.get(&sc(&c, &id, &format!("/api/loader/builds?mc={}", extra::enc(&mc)))).ok();
        let mut vals = vec![String::new()]; let mut labels = vec!["Recommended / newest stable".to_string()];
        if let Some(j) = r { for b in arr(&j, "builds") { let v = gs(&b, "v"); if v.is_empty() { continue; } labels.push(format!("{v}{}", if jb(&b, "stable") { "" } else { "  (unstable)" })); vals.push(v); } }
        st.lock().unwrap().x.lp_builds = vals;
        with_ui(&u, move |a| { a.set_lp_builds(strs(&labels)); a.set_lp_build_index(0); });
    });
}

pub fn open(ui: &Weak<App>, st: &Shared) {
    let (ui, st) = (ui.clone(), st.clone());
    bg(move || {
        let (Some(c), id) = (client(&st), st.lock().unwrap().sel.id.clone()) else { return };
        if id.is_empty() { return; }
        say(&ui, "Loading versions...");
        let Ok(j) = c.get(&sc(&c, &id, "/api/loader/info")) else { return say(&ui, "Couldn't reach the server panel."); };
        if !jb(&j, "supported") { return say(&ui, "This server type is updated from the Overview page."); }
        let mcs: Vec<String> = arr(&j, "mcs").iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect();
        if mcs.is_empty() { let e = gs(&j, "error"); return say(&ui, if e.is_empty() { "Couldn't get the version list. Check your internet." } else { &e }); }
        let (cur, ld, name) = (gs(&j, "mc"), gs(&j, "loader"), gs(&j, "name"));
        let idx = mcs.iter().position(|x| *x == cur).unwrap_or(0);
        let labels: Vec<String> = mcs.iter().enumerate().map(|(i, v)| format!("{v}{}", if *v == cur { "  (installed)" } else if i == 0 { "  (newest)" } else { "" })).collect();
        { let mut g = st.lock().unwrap(); g.x.lp_mcs = mcs.clone(); g.x.lp_cur = cur.clone(); g.x.lp_loader = ld.clone(); g.x.lp_name = name.clone(); g.x.lp_builds = vec![String::new()]; }
        let has = ld == "fabric" || ld == "forge"; let running = jb(&j, "running");
        let first = mcs.get(idx).cloned().unwrap_or_default(); let nt = note_for(&cur, &first);
        with_ui(&ui, move |a| {
            a.set_lp_name(SharedString::from(name)); a.set_lp_mcs(strs(&labels)); a.set_lp_mc_index(idx as i32);
            a.set_lp_builds(strs(&["Recommended / newest stable".to_string()])); a.set_lp_build_index(0); a.set_lp_has_builds(has);
            a.set_lp_note(SharedString::from(if running { "The server is running. Stop it first.".to_string() } else { nt })); a.set_lp_msg(SharedString::new()); a.set_lp_busy(false);
            a.set_note(SharedString::new()); a.set_page(SharedString::from("loaderpick"));
        });
        load_builds(&ui, &st, first);
    });
}

pub fn wire(app: &App, ui: &Weak<App>, st: &Shared) {
    { let (u, s_) = (ui.clone(), st.clone()); app.on_lp_mc_changed(move |i| {
        let (mc, cur) = { let g = s_.lock().unwrap(); (g.x.lp_mcs.get(i.max(0) as usize).cloned().unwrap_or_default(), g.x.lp_cur.clone()) };
        if mc.is_empty() { return; }
        let nt = note_for(&cur, &mc); with_ui(&u, move |a| a.set_lp_note(SharedString::from(nt)));
        load_builds(&u, &s_, mc);
    }); }
    { let (u, s_) = (ui.clone(), st.clone()); app.on_lp_go(move || {
        let Some(a) = u.upgrade() else { return };
        let (mi, bi, backup) = (a.get_lp_mc_index().max(0) as usize, a.get_lp_build_index().max(0) as usize, a.get_lp_backup());
        let (mc, build) = { let g = s_.lock().unwrap(); (g.x.lp_mcs.get(mi).cloned().unwrap_or_default(), g.x.lp_builds.get(bi).cloned().unwrap_or_default()) };
        if mc.is_empty() { return; }
        a.set_lp_busy(true); let (u, s_) = (u.clone(), s_.clone());
        bg(move || {
            let (Some(c), id) = (client(&s_), s_.lock().unwrap().sel.id.clone()) else { return };
            let fin = |u: &Weak<App>, m: String| with_ui(u, move |a| { a.set_lp_busy(false); a.set_lp_msg(SharedString::from(m)); });
            lp_msg(&u, "Starting...");
            let r = c.post(&sc(&c, &id, "/api/loader/update"), &obj(vec![("mc", s(&mc)), ("version", s(&build)), ("backup", J::Bool(backup))]));
            if !ok_of(&r) { return fin(&u, msg_of(&r)); }
            for _ in 0..1800 {
                thread::sleep(Duration::from_secs(2));
                let Ok(j) = c.get(&sc(&c, &id, "/api/loader/status")) else { continue };
                let m = gs(&j, "msg");
                if gs(&j, "phase") != "running" {
                    let res = j.get("result").cloned().unwrap_or(J::Null);
                    let text = if jb(&res, "ok") { format!("Updated: {} -> {}", gs(&res, "old"), gs(&res, "new")) } else if gs(&res, "msg").is_empty() { m } else { gs(&res, "msg") };
                    return fin(&u, text);
                }
                lp_msg(&u, &m);
            }
            fin(&u, "Still working in the background. Check the Console.".into());
        });
    }); }
}
