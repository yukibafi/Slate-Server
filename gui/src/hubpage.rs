//! Hub Backups page: world safety net + move-to-another-computer (full backup / restore).
use crate::*;

static PREV: std::sync::Mutex<(String, String)> = std::sync::Mutex::new((String::new(), String::new()));
fn jb(j: &J, k: &str) -> bool { matches!(j.get(k), Some(J::Bool(true))) }
fn when(t: i64) -> String { let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0); extra::ago((now - t).max(0)) }

pub fn refresh(ui: &Weak<App>, st: &Shared) {
    let Some(c) = client(st) else { return };
    let Ok(j) = c.get("/api/hub/backups") else { return };
    let cfg = j.get("cfg").cloned().unwrap_or(J::Null);
    let rows: Vec<(String, String, String, String, String, String)> = arr(&j, "items").iter().map(|b| (gs(b, "rel"), gs(b, "name"), gs(b, "server"), gs(b, "kind"), mb(gn(b, "size")), when(gn(b, "mtime")))).collect();
    let (busy, msg) = (jb(&j, "busy"), gs(&j, "msg"));
    let free = match j.get("free") { Some(J::Num(x)) => format!("{} free", mb(*x as i64)), _ => String::new() };
    let (auto, hours, keep) = (gn(&cfg, "auto") != 0, gn(&cfg, "hours"), gn(&cfg, "keep"));
    let bundles: Vec<(String, String, String)> = c.get("/api/hub/migrate/find").map(|b| arr(&b, "bundles").iter().map(|x| (gs(x, "path"), gs(x, "name"), mb(gn(x, "size")))).collect()).unwrap_or_default();
    let last = c.get("/api/hub/migrate/last").map(|l| if jb(&l, "exists") { gs(&l, "path") } else { String::new() }).unwrap_or_default();
    with_ui(ui, move |a| {
        a.set_hb_rows(ModelRc::new(VecModel::from(rows.into_iter().map(|r| HBRow { rel: r.0.into(), name: r.1.into(), server: r.2.into(), kind: r.3.into(), size: r.4.into(), ago: r.5.into() }).collect::<Vec<_>>())));
        a.set_hb_busy(busy); if !msg.is_empty() { a.set_hb_msg(SharedString::from(msg)); } a.set_hb_free(SharedString::from(free));
        // don't overwrite what's being typed: only fill the boxes while they still hold the defaults / last values
        a.set_hb_auto(auto);
        let mut p = PREV.lock().unwrap(); let (h, k) = (hours.to_string(), keep.to_string());
        if a.get_hb_hours().as_str() == p.0 || p.0.is_empty() { a.set_hb_hours(SharedString::from(h.clone())); }
        if a.get_hb_keep().as_str() == p.1 || p.1.is_empty() { a.set_hb_keep(SharedString::from(k.clone())); }
        *p = (h, k);
        a.set_bundles(ModelRc::new(VecModel::from(bundles.into_iter().map(|b| BundleRow { path: b.0.into(), name: b.1.into(), size: b.2.into() }).collect::<Vec<_>>())));
        a.set_mig_last(SharedString::from(last));
    });
}

fn save_file(u: &Weak<App>, c: &create_panel::client::Client, path: &str, name: &str) {
    say(u, "Downloading...");
    match c.raw("GET", path, None) {
        Ok((200, body)) => { let dest = extra::downloads_dir().join(name); match std::fs::write(&dest, body) { Ok(_) => say(u, &format!("Saved to {}", dest.display())), Err(e) => say(u, &format!("Couldn't save it: {e}")) } }
        _ => say(u, "Couldn't download that."),
    }
}
fn hb_msg(u: &Weak<App>, m: &str) { let m = m.to_string(); with_ui(u, move |a| a.set_hb_msg(SharedString::from(m))); }
fn mig_msg(u: &Weak<App>, m: &str) { let m = m.to_string(); with_ui(u, move |a| a.set_mig_msg(SharedString::from(m))); }

pub fn wire(app: &App, ui: &Weak<App>, st: &Shared) {
    { let (u, s_) = (ui.clone(), st.clone()); app.on_hb_refresh(move || { let (u, s_) = (u.clone(), s_.clone()); bg(move || refresh(&u, &s_)); }); }
    { let (u, s_) = (ui.clone(), st.clone()); app.on_hb_run(move || { let (u, s_) = (u.clone(), s_.clone()); bg(move || { let Some(c) = client(&s_) else { return }; let r = c.post("/api/hub/backups/run", &obj(vec![])); hb_msg(&u, &msg_of(&r)); refresh(&u, &s_); }); }); }
    { let (u, s_) = (ui.clone(), st.clone()); app.on_hb_config(move |k, v| {
        let (k, v, u, s_) = (k.to_string(), v.to_string(), u.clone(), s_.clone());
        bg(move || { let Some(c) = client(&s_) else { return };
            let n_ = v.trim().parse::<i64>().unwrap_or(-1); if n_ < 0 { hb_msg(&u, "Enter a number."); return; }
            let key = match k.as_str() { "auto" => "auto", "hours" => "hours", "keep" => "keep", _ => return };
            let r = c.post("/api/hub/backups/config", &obj(vec![(key, n(n_))])); hb_msg(&u, &msg_of(&r)); });
    }); }
    { let (u, s_) = (ui.clone(), st.clone()); app.on_hb_download(move |rel| { let (rel, u, s_) = (rel.to_string(), u.clone(), s_.clone()); bg(move || { let Some(c) = client(&s_) else { return }; let name = rel.rsplit('/').next().unwrap_or("backup.zip").to_string(); save_file(&u, &c, &format!("/api/hub/backups/download?rel={}", extra::enc(&rel)), &name); }); }); }
    { let (u, s_) = (ui.clone(), st.clone()); app.on_hb_restore(move |rel| { let (rel, u, s_) = (rel.to_string(), u.clone(), s_.clone()); bg(move || { let Some(c) = client(&s_) else { return }; let r = c.post("/api/hub/backups/restore", &obj(vec![("rel", s(&rel))])); hb_msg(&u, &msg_of(&r)); }); }); }
    { let (u, s_) = (ui.clone(), st.clone()); app.on_hb_delete(move |rel| { let (rel, u, s_) = (rel.to_string(), u.clone(), s_.clone()); bg(move || { let Some(c) = client(&s_) else { return }; let r = c.post("/api/hub/backups/delete", &obj(vec![("rel", s(&rel))])); hb_msg(&u, &msg_of(&r)); refresh(&u, &s_); }); }); }
    { let (u, s_) = (ui.clone(), st.clone()); app.on_mig_backup(move || {
        let (stop, wb) = u.upgrade().map(|a| (a.get_mig_stop(), a.get_mig_backups())).unwrap_or((true, true)); let (u, s_) = (u.clone(), s_.clone());
        bg(move || { let Some(c) = client(&s_) else { return };
            mig_msg(&u, "Starting...");
            let r = c.post("/api/hub/migrate/backup", &obj(vec![("stop", J::Bool(stop)), ("backups", J::Bool(wb))]));
            if !ok_of(&r) { return mig_msg(&u, &msg_of(&r)); }
            let job = match &r { Ok(j) => gs(j, "job"), _ => String::new() };
            for _ in 0..3600 { thread::sleep(Duration::from_secs(2)); let Ok(j) = c.get(&format!("/api/hub/job?id={job}")) else { continue };
                if jb(&j, "done") { mig_msg(&u, &gs(&j, "msg")); break; } mig_msg(&u, &format!("{} {}%", gs(&j, "step"), gn(&j, "pct"))); }
            refresh(&u, &s_); });
    }); }
    { let (u, s_) = (ui.clone(), st.clone()); app.on_mig_restore(move |p| {
        let (p, u, s_) = (p.trim().to_string(), u.clone(), s_.clone());
        bg(move || { let Some(c) = client(&s_) else { return };
            if p.is_empty() { return mig_msg(&u, "Pick a backup or paste its path."); }
            mig_msg(&u, "Starting...");
            let r = c.post("/api/hub/migrate/restore", &obj(vec![("path", s(&p))]));
            if !ok_of(&r) { return mig_msg(&u, &msg_of(&r)); }
            let job = match &r { Ok(j) => gs(j, "job"), _ => String::new() };
            for _ in 0..3600 { thread::sleep(Duration::from_secs(2)); let Ok(j) = c.get(&format!("/api/hub/job?id={job}")) else { continue };
                if jb(&j, "done") { mig_msg(&u, &gs(&j, "msg")); break; } mig_msg(&u, &format!("{} {}%", gs(&j, "step"), gn(&j, "pct"))); }
            refresh(&u, &s_); });
    }); }
    { let (u, s_) = (ui.clone(), st.clone()); app.on_mig_download(move || { let (u, s_) = (u.clone(), s_.clone()); bg(move || { let Some(c) = client(&s_) else { return }; let name = c.get("/api/hub/migrate/last").map(|l| gs(&l, "path")).unwrap_or_default(); let name = name.rsplit(['/', '\\']).next().unwrap_or("Server Hub Backup.zip").to_string(); save_file(&u, &c, "/api/hub/migrate/download", &name); }); }); }
}
