//! Discord bot page: talks to the engine's /api/hub/discord routes.
use crate::{arr, bg, client, gs, gn, msg_of, ok_of, sc, with_ui, App, Shared};
use slint::{ModelRc, VecModel};
use create_panel::json::{obj, s, J};
use slint::{ComponentHandle, SharedString, Weak};
use std::sync::Mutex;

fn flag(j: &J, k: &str) -> bool { matches!(j.get(k), Some(J::Bool(true))) }
static PREV_ALLOWED: Mutex<String> = Mutex::new(String::new());

fn uptime(sec: i64) -> String { if sec >= 3600 { format!("{}h {}m", sec / 3600, (sec % 3600) / 60) } else if sec >= 60 { format!("{}m", sec / 60) } else { format!("{sec}s") } }

pub fn refresh(ui: &Weak<App>, st: &Shared) {
    let Some(c) = client(st) else { return };
    let Ok(j) = c.get("/api/hub/discord") else { return };
    with_ui(ui, move |a| {
        a.set_dc_installed(flag(&j, "installed")); a.set_dc_installing(flag(&j, "installing")); a.set_dc_install_msg(gs(&j, "install_msg").into());
        a.set_dc_token_set(flag(&j, "token_set")); a.set_dc_invite(gs(&j, "invite").into());
        a.set_dc_running(flag(&j, "running")); a.set_dc_uptime(uptime(gn(&j, "uptime")).into()); a.set_dc_note(gs(&j, "note").into());
        a.set_dc_autostart(flag(&j, "autostart")); a.set_dc_modlist(flag(&j, "modlist")); a.set_dc_welcome(flag(&j, "welcome")); a.set_dc_fingerprint(flag(&j, "fingerprint")); a.set_dc_startcmd(flag(&j, "startcmd"));
        a.set_dc_log(gs(&j, "log").into());
        let new = gs(&j, "allowed"); let mut p = PREV_ALLOWED.lock().unwrap();
        if a.get_dc_allowed().as_str() == p.as_str() { a.set_dc_allowed(new.clone().into()); }
        *p = new;
    });
}

fn open_url(u: &str) {
    if !u.starts_with("https://") { return; }
    #[cfg(target_os = "macos")] let _ = std::process::Command::new("open").arg(u).spawn();
    #[cfg(windows)] let _ = std::process::Command::new("cmd").args(["/C", "start", "", u]).spawn();
    #[cfg(all(unix, not(target_os = "macos")))] let _ = std::process::Command::new("xdg-open").arg(u).spawn();
}

pub fn wire(app: &App, ui: &Weak<App>, st: &Shared) {
    let post = move |ui: Weak<App>, st: Shared, path: &'static str, body: J| {
        bg(move || {
            let Some(c) = client(&st) else { return };
            let r = c.post(path, &body); let m = msg_of(&r); let ok = ok_of(&r);
            with_ui(&ui, move |a| a.set_dc_msg(SharedString::from(if ok && m == "Saved." { String::new() } else { m })));
            refresh(&ui, &st);
        });
    };
    { let (u, s_) = (ui.clone(), st.clone()); app.on_dc_refresh(move || { let (u, s_) = (u.clone(), s_.clone()); bg(move || refresh(&u, &s_)); }); }
    { let (u, s_) = (ui.clone(), st.clone()); app.on_dc_install(move || post(u.clone(), s_.clone(), "/api/hub/discord/install", obj(vec![]))); }
    { let (u, s_) = (ui.clone(), st.clone()); app.on_dc_save_token(move |t| {
        let t = t.to_string();
        with_ui(&u, |a| a.set_dc_token(SharedString::new()));
        post(u.clone(), s_.clone(), "/api/hub/discord/token", obj(vec![("token", s(&t))]));
    }); }
    { let (u, s_) = (ui.clone(), st.clone()); app.on_dc_action(move |act| post(u.clone(), s_.clone(), "/api/hub/discord/action", obj(vec![("action", s(act.as_str()))]))); }
    { let (u, s_) = (ui.clone(), st.clone()); app.on_dc_config(move |k, on| {
        let key: &str = match k.as_str() { "autostart" => "autostart", "modlist" => "modlist", "welcome" => "welcome", "fingerprint" => "fingerprint", "startcmd" => "startcmd", _ => return };
        post(u.clone(), s_.clone(), "/api/hub/discord/config", obj(vec![(key, J::Bool(on))]));
    }); }
    { let (u, s_) = (ui.clone(), st.clone()); app.on_dc_save_allowed(move |t| post(u.clone(), s_.clone(), "/api/hub/discord/config", obj(vec![("allowed", s(t.as_str()))]))); }
    app.on_dc_copy(move |t| crate::extra::copy_clip(t.as_str()));
    app.on_dc_open(move |u| open_url(u.as_str()));

    wire_alerts(app, ui, st);
}

// ---------------------------------------------------------------- per-server Discord alerts (webhook), edited from the hub
static AL_IDS: Mutex<Vec<String>> = Mutex::new(Vec::new());
const TTLS: [i64; 5] = [30, 60, 300, 3600, 0];
fn al_msg(u: &Weak<App>, m: &str) { let m = m.to_string(); with_ui(u, move |a| a.set_al_msg(SharedString::from(m))); }
fn al_sel(idx: i32) -> Option<String> { AL_IDS.lock().unwrap().get(idx.max(0) as usize).cloned() }

fn al_load(ui: &Weak<App>, st: &Shared, idx: i32) {
    let Some(c) = client(st) else { return };
    let mut idx = idx;
    if idx < 0 {
        let Ok(j) = c.get("/api/hub/state") else { return };
        let list: Vec<(String, String)> = arr(&j, "servers").iter().map(|e| (gs(e, "id"), gs(e, "name"))).collect();
        *AL_IDS.lock().unwrap() = list.iter().map(|x| x.0.clone()).collect();
        let names: Vec<SharedString> = list.iter().map(|x| SharedString::from(x.1.as_str())).collect();
        with_ui(ui, move |a| { a.set_al_servers(ModelRc::new(VecModel::from(names))); a.set_al_index(0); a.set_al_msg(SharedString::new()); });
        idx = 0;
    }
    let Some(id) = al_sel(idx) else { return };
    let Ok(j) = c.get(&sc(&c, &id, "/api/auto")) else { return al_msg(ui, "Couldn't load that server (is its panel on?)."); };
    let d = j.get("cfg").and_then(|x| x.get("discord")).cloned().unwrap_or(J::Null);
    let has = flag(&d, "has"); let ttl = gn(&d, "ttl");
    let ti = TTLS.iter().position(|t| *t == ttl).unwrap_or(1) as i32;
    with_ui(ui, move |a| {
        a.set_al_hint(SharedString::from(if has { "set" } else { "" })); a.set_al_url(SharedString::new());
        a.set_al_start(flag(&d, "start")); a.set_al_stop(flag(&d, "stop")); a.set_al_crash(flag(&d, "crash")); a.set_al_join(flag(&d, "join")); a.set_al_leave(flag(&d, "leave"));
        a.set_al_ttl_index(ti); a.set_al_msg(SharedString::new());
    });
}

fn set_key(o: &mut J, k: &str, v: J) { if let J::Obj(m) = o { if let Some(e) = m.iter_mut().find(|(x, _)| x == k) { e.1 = v; } else { m.push((k.to_string(), v)); } } }

fn al_save(ui: &Weak<App>, st: &Shared, idx: i32, edits: Vec<(&'static str, J)>) {
    let (ui, st) = (ui.clone(), st.clone());
    bg(move || {
        let (Some(c), Some(id)) = (client(&st), al_sel(idx)) else { return };
        let Ok(j) = c.get(&sc(&c, &id, "/api/auto")) else { return al_msg(&ui, "Couldn't load that server."); };
        let mut cfg = j.get("cfg").cloned().unwrap_or(J::Null);
        let mut d = cfg.get("discord").cloned().unwrap_or(J::Null);
        for (k, v) in edits { set_key(&mut d, k, v); }
        set_key(&mut cfg, "discord", d);
        let r = c.post(&sc(&c, &id, "/api/auto/save"), &cfg);
        al_msg(&ui, &if ok_of(&r) { String::new() } else { msg_of(&r) });
        al_load(&ui, &st, idx);
    });
}

fn wire_alerts(app: &App, ui: &Weak<App>, st: &Shared) {
    { let (u, s_) = (ui.clone(), st.clone()); app.on_al_load(move |i| { let (u, s_) = (u.clone(), s_.clone()); bg(move || al_load(&u, &s_, i)); }); }
    { let (u, s_) = (ui.clone(), st.clone()); app.on_al_save_url(move |url| {
        let url = url.trim().to_string(); let Some(a) = u.upgrade() else { return };
        if url.is_empty() { al_msg(&u, "Paste the webhook link first."); return; }
        al_save(&u, &s_, a.get_al_index(), vec![("url", s(&url))]);
    }); }
    { let (u, s_) = (ui.clone(), st.clone()); app.on_al_remove(move || { let Some(a) = u.upgrade() else { return }; al_save(&u, &s_, a.get_al_index(), vec![("url", s(""))]); }); }
    { let (u, s_) = (ui.clone(), st.clone()); app.on_al_flag(move |k, on| {
        let key: &'static str = match k.as_str() { "start" => "start", "stop" => "stop", "crash" => "crash", "join" => "join", "leave" => "leave", _ => return };
        let Some(a) = u.upgrade() else { return }; al_save(&u, &s_, a.get_al_index(), vec![(key, J::Bool(on))]);
    }); }
    { let (u, s_) = (ui.clone(), st.clone()); app.on_al_ttl(move |i| { let Some(a) = u.upgrade() else { return }; al_save(&u, &s_, a.get_al_index(), vec![("ttl", crate::n(*TTLS.get(i.max(0) as usize).unwrap_or(&60)))]); }); }
    { let (u, s_) = (ui.clone(), st.clone()); app.on_al_test(move || {
        let Some(a) = u.upgrade() else { return }; let idx = a.get_al_index(); let (u, s_) = (u.clone(), s_.clone());
        bg(move || { let (Some(c), Some(id)) = (client(&s_), al_sel(idx)) else { return }; al_msg(&u, "Sending a test message...");
            let r = c.post(&sc(&c, &id, "/api/auto/discord-test"), &obj(vec![("url", s("keep"))])); al_msg(&u, &msg_of(&r)); });
    }); }
}
