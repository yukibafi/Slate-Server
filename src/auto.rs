//! Scheduled tasks, idle auto-stop, welcome message, Discord alerts and player play-time stats.
use crate::http::{Req, Resp};
use crate::json::{self, n, obj, s, J};
use crate::{sys, App};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use std::sync::atomic::{AtomicBool, Ordering};

const KINDS: &[&str] = &["restart", "backup", "command", "broadcast", "stop", "start"];
const WELCOME: &str = "Welcome to {server}, {player}!";

#[derive(Clone)]
pub struct Task { id: String, on: bool, kind: String, mode: String, time: String, every: i64, days: Vec<i64>, text: String, warn: bool }
#[derive(Clone)]
pub struct Cfg { tasks: Vec<Task>, idle_on: bool, idle_min: i64, welcome_on: bool, welcome_text: String, url: String, d_start: bool, d_stop: bool, d_crash: bool, d_join: bool, d_leave: bool, ttl: i64 }

struct St { cfg: Option<Cfg>, mt: Option<std::time::SystemTime>, fired: HashMap<String, f64>, base: f64, prev_players: Vec<String>, prev_state: Option<String>, empty_since: Option<f64>, restarting: bool, play: Option<J>, sess: HashMap<String, f64>, dirty: bool }
static ST: Mutex<Option<St>> = Mutex::new(None);
fn with<R>(f: impl FnOnce(&mut St) -> R) -> R {
    let mut g = ST.lock().unwrap();
    let st = g.get_or_insert_with(|| St { cfg: None, mt: None, fired: HashMap::new(), base: sys::now(), prev_players: vec![], prev_state: None, empty_since: None, restarting: false, play: None, sess: HashMap::new(), dirty: false });
    f(st)
}

fn auto_file(app: &App) -> PathBuf { crate::envinfo::settings_path(&app.dir).parent().map(|p| p.to_path_buf()).unwrap_or_else(|| app.dir.clone()).join(".panel-automation.json") }
fn play_file(app: &App) -> PathBuf { auto_file(app).with_file_name(".panel-players.json") }

fn webhook_ok(u: &str) -> bool {
    if std::env::var("PANEL_ALLOW_ANY_WEBHOOK").is_ok() && u.starts_with("http://127.0.0.1") { return true; }
    let rest = match u.strip_prefix("https://discord.com/api/webhooks/").or_else(|| u.strip_prefix("https://discordapp.com/api/webhooks/")) { Some(r) => r, None => return false };
    let rest = rest.strip_suffix('/').unwrap_or(rest);
    let mut p = rest.splitn(2, '/');
    let (id, tok) = (p.next().unwrap_or(""), p.next().unwrap_or(""));
    !id.is_empty() && id.chars().all(|c| c.is_ascii_digit()) && !tok.is_empty() && tok.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}
fn gs(j: &J, k: &str) -> String { match j.get(k) { Some(J::Str(t)) => t.clone(), Some(J::Num(x)) => format!("{}", *x as i64), _ => String::new() } }
fn gb(j: &J, k: &str, def: bool) -> bool { match j.get(k) { Some(J::Bool(b)) => *b, Some(J::Num(x)) => *x != 0.0, Some(J::Str(t)) => !t.is_empty(), Some(J::Null) => false, _ => def } }
fn gi(j: &J, k: &str, def: i64) -> i64 { match j.get(k) { Some(J::Num(x)) => *x as i64, Some(J::Str(t)) => t.trim().parse().unwrap_or(def), _ => def } }
fn oneline(t: &str, max: usize) -> String { let mut o = String::new(); let mut prev_nl = false; for c in t.chars() { if c == '\r' || c == '\n' { if !prev_nl { o.push(' '); } prev_nl = true; } else { o.push(c); prev_nl = false; } } o.chars().take(max).collect() }
fn time_ok(t: &str) -> bool {
    let Some((h, m)) = t.split_once(':') else { return false };
    h.len() >= 1 && h.len() <= 2 && h.chars().all(|c| c.is_ascii_digit()) && h.parse::<u32>().map(|v| v < 24).unwrap_or(false) && m.len() == 2 && m.chars().all(|c| c.is_ascii_digit()) && m.parse::<u32>().map(|v| v < 60).unwrap_or(false)
}
fn rand_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::hash::{BuildHasher, Hasher};
    static C: AtomicU64 = AtomicU64::new(1);
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u64(C.fetch_add(1, Ordering::SeqCst)); h.write_u64((sys::now() * 1e6) as u64);
    format!("{:06x}", h.finish() & 0xffffff)
}

pub fn clean(o: &J) -> Cfg {
    let mut c = Cfg { tasks: vec![], idle_on: false, idle_min: 15, welcome_on: false, welcome_text: WELCOME.into(), url: String::new(), d_start: true, d_stop: true, d_crash: true, d_join: true, d_leave: false, ttl: 60 };
    if !matches!(o, J::Obj(_)) { return c; }
    if let Some(J::Arr(ts)) = o.get("tasks") {
        for t in ts.iter().take(40) {
            let kind = gs(t, "kind");
            if !matches!(t, J::Obj(_)) || !KINDS.contains(&kind.as_str()) { continue; }
            let mode = if gs(t, "mode") == "every" { "every" } else { "daily" };
            let mut tm = if t.get("time").is_some() { gs(t, "time") } else { "04:00".into() };
            if !time_ok(&tm) { tm = "04:00".into(); }
            let ev = gi(t, "every", 120).clamp(1, 10080);
            let days: Vec<i64> = if mode == "daily" { match t.get("days") { Some(J::Arr(a)) => a.iter().filter_map(|d| if let J::Num(x) = d { Some(*x as i64) } else { None }).filter(|d| (0..=6).contains(d)).collect(), _ => vec![] } } else { vec![] };
            let raw = if t.get("id").map(|x| !matches!(x, J::Null) && gs(t, "id") != "").unwrap_or(false) { gs(t, "id") } else { rand_id() };
            let mut id: String = raw.chars().filter(|c| c.is_ascii_alphanumeric()).take(12).collect();
            if id.is_empty() { id = rand_id(); }
            c.tasks.push(Task { id, on: gb(t, "on", true), kind, mode: mode.into(), time: tm, every: ev, days, text: oneline(&gs(t, "text"), 200), warn: gb(t, "warn", true) });
        }
    }
    let i = o.get("idle").cloned().unwrap_or(J::Null);
    c.idle_min = gi(&i, "minutes", 15).clamp(2, 1440); c.idle_on = gb(&i, "on", false);
    let w = o.get("welcome").cloned().unwrap_or(J::Null);
    c.welcome_on = gb(&w, "on", false);
    c.welcome_text = oneline(&if w.get("text").is_some() { gs(&w, "text") } else { WELCOME.into() }, 200);
    let d = o.get("discord").cloned().unwrap_or(J::Null);
    let url = gs(&d, "url").trim().to_string();
    c.url = if webhook_ok(&url) { url } else { String::new() };
    c.d_start = gb(&d, "start", true); c.d_stop = gb(&d, "stop", true); c.d_crash = gb(&d, "crash", true); c.d_join = gb(&d, "join", true); c.d_leave = gb(&d, "leave", false);
    c.ttl = gi(&d, "ttl", 60).clamp(0, 86400);
    c
}
pub fn to_json(c: &Cfg) -> J {
    obj(vec![
        ("tasks", J::Arr(c.tasks.iter().map(|t| obj(vec![("id", s(&t.id)), ("on", J::Bool(t.on)), ("kind", s(&t.kind)), ("mode", s(&t.mode)), ("time", s(&t.time)), ("every", n(t.every)), ("days", J::Arr(t.days.iter().map(|d| n(*d)).collect())), ("text", s(&t.text)), ("warn", J::Bool(t.warn))])).collect())),
        ("idle", obj(vec![("on", J::Bool(c.idle_on)), ("minutes", n(c.idle_min))])),
        ("welcome", obj(vec![("on", J::Bool(c.welcome_on)), ("text", s(&c.welcome_text))])),
        ("discord", obj(vec![("url", s(&c.url)), ("start", J::Bool(c.d_start)), ("stop", J::Bool(c.d_stop)), ("crash", J::Bool(c.d_crash)), ("join", J::Bool(c.d_join)), ("leave", J::Bool(c.d_leave)), ("ttl", n(c.ttl))])),
    ])
}
pub fn cfg(app: &App) -> Cfg {
    let f = auto_file(app);
    let mt = fs::metadata(&f).and_then(|m| m.modified()).ok();
    with(|st| {
        if mt != st.mt { st.mt = mt; if mt.is_some() { st.cfg = None; } }
        if st.cfg.is_none() { st.cfg = Some(clean(&fs::read_to_string(&f).ok().and_then(|t| json::parse(&t)).unwrap_or(J::Null))); }
        st.cfg.clone().unwrap()
    })
}
pub fn save(app: &App, o: &J) -> (bool, String) {
    let mut c = clean(o);
    if o.get("discord").map(|d| gs(d, "url")) == Some("keep".into()) { c.url = cfg(app).url; }
    let f = auto_file(app);
    let tmp = f.with_extension("json.tmp");
    if let Err(e) = fs::write(&tmp, to_json(&c).to_string()).and_then(|_| fs::rename(&tmp, &f)) { return (false, format!("Couldn't save: {e}")); }
    let mt = fs::metadata(&f).and_then(|m| m.modified()).ok();
    with(|st| { st.cfg = Some(c); st.mt = mt; });
    (true, "Saved.".into())
}

fn srv_name() -> String { let n = std::env::var("PANEL_NAME").unwrap_or_default(); if n.is_empty() { "Server".into() } else { n } }

pub fn discord_send(app: &App, text: &str, force_url: Option<&str>, ttl_force: i64) -> (bool, String) {
    let c = cfg(app);
    let url = force_url.map(|x| x.to_string()).unwrap_or(c.url.clone());
    if url.is_empty() { return (false, "No webhook address saved.".into()); }
    let ttl = if force_url.is_some() { ttl_force } else { c.ttl };
    let t: String = text.chars().take(1900).collect();
    let name: String = srv_name().chars().take(80).collect();
    let go = move || -> (bool, String) {
        let body = obj(vec![("content", s(&t)), ("username", s(&name))]).to_string();
        let base = url.trim_end_matches('/').to_string();
        let (code, resp) = sys::http("POST", &format!("{base}?wait=true"), &[("Content-Type", "application/json"), ("User-Agent", "create-panel-rs/0.1")], Some(body.as_bytes()), 10);
        if code == 0 { return (false, "Couldn't reach Discord.".into()); }
        if !(200..300).contains(&code) { return (false, format!("Discord said HTTP {code}.")); }
        let mid = json::parse(&String::from_utf8_lossy(&resp)).and_then(|j| j.get("id").map(|x| match x { J::Str(t) => t.clone(), J::Num(v) => format!("{}", *v as i64), _ => String::new() })).unwrap_or_default();
        if !mid.is_empty() && ttl > 0 {
            thread::spawn(move || { thread::sleep(Duration::from_secs(ttl as u64)); let _ = sys::http("DELETE", &format!("{base}/messages/{mid}"), &[("User-Agent", "create-panel-rs/0.1")], None, 10); });
        }
        (true, "Sent.".into())
    };
    if force_url.is_some() { go() } else { thread::spawn(move || { go(); }); (true, "Sending...".into()) }
}

fn play_load(app: &App) { with(|st| { if st.play.is_none() { st.play = Some(fs::read_to_string(play_file(app)).ok().and_then(|t| json::parse(&t)).filter(|j| matches!(j, J::Obj(_))).unwrap_or(J::Obj(vec![]))); } }); }
fn play_touch(app: &App, name: &str, f: impl FnOnce(&mut Vec<(String, J)>)) {
    play_load(app);
    with(|st| {
        let J::Obj(m) = st.play.as_mut().unwrap() else { return };
        if !m.iter().any(|(k, _)| k == name) { m.push((name.to_string(), obj(vec![("first", n(sys::now() as i64)), ("last", n(0)), ("seconds", n(0)), ("joins", n(0))]))); }
        if let Some((_, J::Obj(e))) = m.iter_mut().find(|(k, _)| k == name) { f(e); }
        st.dirty = true;
    });
}
fn setn(e: &mut Vec<(String, J)>, k: &str, v: i64) { if let Some(x) = e.iter_mut().find(|(a, _)| a == k) { x.1 = n(v); } else { e.push((k.to_string(), n(v))); } }
fn getn(e: &[(String, J)], k: &str) -> i64 { e.iter().find(|(a, _)| a == k).map(|(_, v)| if let J::Num(x) = v { *x as i64 } else { 0 }).unwrap_or(0) }
fn play_join(app: &App, name: &str) { let now = sys::now(); play_touch(app, name, |e| { let j = getn(e, "joins") + 1; setn(e, "joins", j); setn(e, "last", now as i64); }); with(|st| { st.sess.insert(name.to_string(), now); }); }
fn play_leave(app: &App, name: &str) {
    let now = sys::now();
    let t0 = with(|st| st.sess.remove(name));
    play_touch(app, name, |e| { if let Some(t0) = t0 { let sec = getn(e, "seconds") + (now - t0) as i64; setn(e, "seconds", sec); } setn(e, "last", now as i64); });
}
fn play_flush(app: &App) {
    let (txt, dirty) = with(|st| (st.play.as_ref().map(|p| p.to_string()), st.dirty));
    if !dirty { return; }
    if let Some(t) = txt { let f = play_file(app); let tmp = f.with_extension("json.tmp"); if fs::write(&tmp, t).and_then(|_| fs::rename(&tmp, &f)).is_ok() { with(|st| st.dirty = false); } }
}

fn say(app: &App, text: &str) -> bool { app.mc.send(&format!("say {}", oneline(text, 100000))) }

pub fn run_task(app: &Arc<App>, t: &Task) -> (bool, String) {
    let running = app.mc.running();
    match t.kind.as_str() {
        "backup" => return crate::misc::backup_async(app),
        "start" => return if !running { app.mc.start() } else { (false, "Already running.".into()) },
        _ => {}
    }
    if !running { return (false, "The server isn't running.".into()); }
    match t.kind.as_str() {
        "broadcast" => if say(app, if t.text.is_empty() { "Hello!" } else { &t.text }) { (true, "Sent.".into()) } else { (false, "Couldn't send.".into()) },
        "command" => {
            let cmd = t.text.trim().trim_start_matches('/').to_string();
            if cmd.is_empty() { return (false, "No command set.".into()); }
            app.mc.log(&format!("> {cmd}  [scheduled]"));
            if app.mc.send(&cmd) { (true, "Sent.".into()) } else { (false, "Couldn't send.".into()) }
        }
        "stop" => app.mc.stop(),
        "restart" => {
            if with(|st| { let r = st.restarting; st.restarting = true; r }) { return (false, "A restart is already counting down.".into()); }
            let (a, warn) = (app.clone(), t.warn);
            thread::spawn(move || {
                if warn && !a.mc.snapshot(usize::MAX).players.is_empty() {
                    say(&a, "Server restarting in 5 minutes."); thread::sleep(Duration::from_secs(240));
                    say(&a, "Server restarting in 1 minute."); thread::sleep(Duration::from_secs(50));
                    say(&a, "Server restarting in 10 seconds."); thread::sleep(Duration::from_secs(10));
                }
                a.mc.log("[panel] Scheduled restart.");
                a.mc.restart();
                with(|st| st.restarting = false);
            });
            (true, "Restart scheduled.".into())
        }
        _ => (false, "Unknown task.".into()),
    }
}

fn due(t: &Task, local: f64, now: f64) -> bool {
    let (last, base) = with(|st| (st.fired.get(&t.id).copied().unwrap_or(0.0), st.base));
    if t.mode == "every" { return now - last.max(base) >= (t.every * 60) as f64; }
    let (hh, mm) = t.time.split_once(':').map(|(h, m)| (h.parse::<f64>().unwrap_or(4.0), m.parse::<f64>().unwrap_or(0.0))).unwrap_or((4.0, 0.0));
    let days = (local / 86400.0).floor() as i64;
    let page_day = (days + 4).rem_euclid(7); // 0 = Sunday
    if !t.days.is_empty() && !t.days.contains(&page_day) { return false; }
    let day_start_local = days as f64 * 86400.0;
    let target_local = day_start_local + hh * 3600.0 + mm * 60.0;
    let target_ts = target_local - (local - now);
    target_local <= local && now - target_ts < 120.0 && last < target_ts
}

fn tick(app: &Arc<App>) {
    let c = cfg(app);
    let now = sys::now();
    let local = now + sys::tz_offset() as f64;
    let sn = app.mc.snapshot(usize::MAX);
    let st_now = sn.state.to_string();
    let mut pl = sn.players.clone(); pl.sort();
    let (prev_pl, prev_st) = with(|st| (st.prev_players.clone(), st.prev_state.clone()));
    for p in pl.iter().filter(|p| !prev_pl.contains(p)) {
        play_join(app, p);
        if c.welcome_on {
            let msg = c.welcome_text.replace("{player}", p).replace("{server}", &srv_name());
            app.mc.send(&format!("tellraw {p} {}", obj(vec![("text", s(&msg)), ("color", s("aqua"))]).to_string()));
        }
        if !c.url.is_empty() && c.d_join { discord_send(app, &format!("{p} joined. {} online.", pl.len()), None, 60); }
    }
    for p in prev_pl.iter().filter(|p| !pl.contains(p)) {
        play_leave(app, p);
        if !c.url.is_empty() && c.d_leave && st_now != "stopped" { discord_send(app, &format!("{p} left. {} online.", pl.len()), None, 60); }
    }
    if st_now == "stopped" { let sess: Vec<String> = with(|st| st.sess.keys().cloned().collect()); let any = !sess.is_empty(); for p in sess { play_leave(app, &p); } if any { play_flush(app); } }
    if let Some(ps) = &prev_st { if *ps != st_now && !c.url.is_empty() {
        if st_now == "online" && c.d_start { discord_send(app, &format!("✅ {} is online.", srv_name()), None, 60); }
        else if st_now == "stopped" {
            let (us, ec, rp) = { let g = app.mc.inner.lock().unwrap(); (g.user_stop, g.exit_code, g.restart_pending_pub()) };
            let crashed = !us && ec.map(|x| x != 0).unwrap_or(false) && !rp;
            if crashed && c.d_crash { discord_send(app, &format!("⚠️ {} crashed (exit code {}).", srv_name(), ec.unwrap_or(0)), None, 60); }
            else if !crashed && c.d_stop { discord_send(app, &format!("⛔ {} stopped.", srv_name()), None, 60); }
        }
    } }
    with(|st| { st.prev_players = pl.clone(); st.prev_state = Some(st_now.clone()); });
    if c.idle_on && st_now == "online" {
        if !pl.is_empty() { with(|st| st.empty_since = None); }
        else {
            let es = with(|st| { if st.empty_since.is_none() { st.empty_since = Some(now); } st.empty_since.unwrap() });
            if now - es >= (c.idle_min * 60) as f64 {
                app.mc.log(&format!("[panel] Nobody has been online for {} minutes. Stopping to save resources.", c.idle_min));
                with(|st| st.empty_since = None);
                app.mc.stop();
            }
        }
    } else { with(|st| st.empty_since = None); }
    for t in &c.tasks {
        if t.on && due(t, local, now) { with(|st| { st.fired.insert(t.id.clone(), now); }); run_task(app, t); }
    }
    if with(|st| st.dirty) && (now as i64) % 60 < 5 { play_flush(app); }
}

pub fn handle(app: &Arc<App>, req: &Req) -> Option<Resp> {
    let body = || json::parse(&String::from_utf8_lossy(&req.body)).unwrap_or(J::Null);
    let reply = |ok: bool, m: &str| Resp::json(200, obj(vec![("ok", J::Bool(ok)), ("msg", s(m))]).to_string());
    Some(match (req.method.as_str(), req.path.as_str()) {
        ("GET", "/api/auto") => {
            let mut j = to_json(&cfg(app));
            if let J::Obj(m) = &mut j { if let Some((_, J::Obj(d))) = m.iter_mut().find(|(k, _)| k == "discord") {
                let has = d.iter().any(|(k, v)| k == "url" && matches!(v, J::Str(t) if !t.is_empty()));
                for (k, v) in d.iter_mut() { if k == "url" { *v = s(if has { "keep" } else { "" }); } }
                d.push(("has".into(), J::Bool(has)));
            } }
            let es = with(|st| st.empty_since);
            Resp::json(200, obj(vec![("cfg", j), ("state", s(app.mc.snapshot(usize::MAX).state)), ("empty_since", es.map(|x| J::Num(x)).unwrap_or(J::Null)), ("now", J::Num(sys::now()))]).to_string())
        }
        ("POST", "/api/auto/save") => { let (ok, m) = save(app, &body()); reply(ok, &m) }
        ("POST", "/api/auto/discord-test") => {
            let b = body(); let mut url = gs(&b, "url").trim().to_string();
            if url == "keep" || url.is_empty() { url = cfg(app).url; }
            if !webhook_ok(&url) { return Some(reply(false, "That doesn't look like a Discord webhook address.")); }
            let (ok, m) = discord_send(app, &format!("✅ Test message from {}. Discord alerts are working.", srv_name()), Some(&url), 60);
            reply(ok, &m)
        }
        ("POST", "/api/auto/run") => {
            let id = gs(&body(), "id");
            match cfg(app).tasks.iter().find(|t| t.id == id) { Some(t) => { let (ok, m) = run_task(app, t); reply(ok, &m) } None => reply(false, "Save the task first.") }
        }
        ("GET", "/api/players/stats") => {
            play_load(app);
            let now = sys::now();
            let mut out: Vec<(i64, J)> = with(|st| {
                let J::Obj(m) = st.play.as_ref().unwrap() else { return vec![] };
                m.iter().filter_map(|(name, v)| { let J::Obj(e) = v else { return None }; let sec = getn(e, "seconds") + st.sess.get(name).map(|t0| (now - t0) as i64).unwrap_or(0);
                    Some((sec, obj(vec![("name", s(name)), ("seconds", n(sec)), ("last", n(getn(e, "last"))), ("first", n(getn(e, "first"))), ("joins", n(getn(e, "joins"))), ("online", J::Bool(st.sess.contains_key(name)))]))) }).collect()
            });
            out.sort_by(|a, b| b.0.cmp(&a.0));
            Resp::json(200, obj(vec![("players", J::Arr(out.into_iter().take(200).map(|x| x.1).collect()))]).to_string())
        }
        ("GET", "/api/history" | "/api/history2") => return history_route(req),
        _ => return None,
    })
}

pub fn start(app: &Arc<App>) {
    history_start(app);
    let a = app.clone();
    thread::spawn(move || { thread::sleep(Duration::from_secs(8)); loop { tick(&a); thread::sleep(Duration::from_secs(5)); } });
}

// ---- 7-day history sampler (/api/history, /api/history2)
static HIST: Mutex<Vec<(i64, Option<i64>, i64)>> = Mutex::new(Vec::new());
const HMAX: usize = 7 * 24 * 120;
fn hist_file(app: &App) -> PathBuf { app.dir.join(".panel-history.json") }
fn hist_load(app: &App) {
    if let Some(J::Obj(m)) = fs::read_to_string(hist_file(app)).ok().and_then(|t| json::parse(&t)) {
        let cut = sys::now() as i64 - 7 * 86400;
        if let Some((_, J::Arr(pts))) = m.iter().find(|(k, _)| k == "points") {
            let mut v: Vec<(i64, Option<i64>, i64)> = pts.iter().filter_map(|p| { let J::Arr(a) = p else { return None }; if a.len() != 3 { return None; }
                let f = |x: &J| if let J::Num(v) = x { Some(*v as i64) } else { None };
                Some((f(&a[0])?, f(&a[1]), f(&a[2])?)) }).filter(|p| p.0 >= cut).collect();
            if v.len() > HMAX { v.drain(..v.len() - HMAX); }
            *HIST.lock().unwrap() = v;
        }
    }
}
fn hist_save(app: &App) {
    let snap = HIST.lock().unwrap().clone();
    let pts = J::Arr(snap.iter().map(|(t, m, p)| J::Arr(vec![n(*t), m.map(n).unwrap_or(J::Null), n(*p)])).collect());
    let f = hist_file(app); let tmp = f.with_extension("json.tmp");
    let _ = fs::write(&tmp, obj(vec![("points", pts)]).to_string()).and_then(|_| fs::rename(&tmp, &f));
}
pub fn history_start(app: &Arc<App>) {
    tps_start(app);
    let a = app.clone();
    thread::spawn(move || {
        hist_load(&a); thread::sleep(Duration::from_secs(10));
        let mut i = 0u32;
        loop {
            let sn = a.mc.snapshot(usize::MAX);
            let running = sn.state != "stopped";
            { let mut h = HIST.lock().unwrap(); h.push((sys::now() as i64, if running { sn.mem_mb.map(|x| x as i64) } else { None }, sn.players.len() as i64)); let l = h.len(); if l > HMAX { h.drain(..l - HMAX); } }
            i += 1; if i % 10 == 0 { hist_save(&a); }
            thread::sleep(Duration::from_secs(30));
        }
    });
}
pub fn history_route(req: &Req) -> Option<Resp> {
    match req.path.as_str() {
        "/api/history" => {
            let h = HIST.lock().unwrap();
            let skip = h.len().saturating_sub(720);
            let pts = J::Arr(h.iter().skip(skip).map(|(t, m, p)| J::Arr(vec![n(*t), m.map(n).unwrap_or(J::Null), n(*p)])).collect());
            Some(Resp::json(200, obj(vec![("ok", J::Bool(true)), ("points", pts)]).to_string()))
        }
        "/api/history2" => {
            let rng = req.q("range").and_then(|v| v.parse::<i64>().ok()).unwrap_or(21600).clamp(600, 7 * 86400);
            let cut = sys::now() as i64 - rng;
            let pts: Vec<(i64, Option<i64>, i64)> = HIST.lock().unwrap().iter().filter(|p| p.0 >= cut).cloned().collect();
            let step = (rng / 300).max(30);
            let mut out: Vec<J> = vec![];
            if step <= 30 { out = pts.iter().map(|(t, m, p)| J::Arr(vec![n(*t), m.map(n).unwrap_or(J::Null), n(*p)])).collect(); }
            else {
                let mut b: Option<(i64, i64, i64, i64)> = None;
                let emit = |b: (i64, i64, i64, i64), out: &mut Vec<J>| out.push(J::Arr(vec![n(b.0 * step + step / 2), if b.2 > 0 { n(b.1 / b.2) } else { J::Null }, n(b.3)]));
                for (t, m, p) in pts {
                    let k = t / step;
                    if b.map(|x| x.0 != k).unwrap_or(true) { if let Some(x) = b { emit(x, &mut out); } b = Some((k, 0, 0, 0)); }
                    let x = b.as_mut().unwrap();
                    if let Some(m) = m { x.1 += m; x.2 += 1; }
                    x.3 = x.3.max(p);
                }
                if let Some(x) = b { emit(x, &mut out); }
            }
            Some(Resp::json(200, obj(vec![("ok", J::Bool(true)), ("points", J::Arr(out)), ("step", n(step)), ("range", n(rng)), ("now", n(sys::now() as i64))]).to_string()))
        }
        _ => None,
    }
}

// ---- TPS sampler, world size and free disk (shown on the Overview tiles)
static TPS_ARMED: AtomicBool = AtomicBool::new(false);
static TPS_FOUND: Mutex<Option<f64>> = Mutex::new(None);
static TPS_HIST: Mutex<Vec<f64>> = Mutex::new(Vec::new());
static WORLD: Mutex<(i64, i64)> = Mutex::new((-1, -1)); // (world MB, free disk MB)

/// Called for every server log line. While a TPS probe is in flight, the probe's own output is
/// swallowed (so it never clutters the console) and the number is remembered.
pub fn tps_capture(line: &str) -> bool {
    if !TPS_ARMED.load(Ordering::SeqCst) { return false; }
    let num = |t: &str| -> Option<f64> { let e = t.find(|c: char| !(c.is_ascii_digit() || c == '.')).unwrap_or(t.len()); t[..e].parse().ok() };
    if let Some(i) = line.find("Mean TPS: ") {
        if let Some(v) = num(&line[i + 10..]) { let mut f = TPS_FOUND.lock().unwrap(); if f.is_none() || line.contains("Overall") { *f = Some(v.min(20.0)); } }
        return true;
    }
    if let Some(i) = line.find("Average time per tick: ") {
        if let Some(ms) = num(&line[i + 23..]) { if ms > 0.0 { *TPS_FOUND.lock().unwrap() = Some((1000.0 / ms).min(20.0)); } }
        return true;
    }
    line.contains("Mean tick time:") || line.contains("Target tick rate") || line.contains("Percentiles") || line.contains("The game is running normally") || line.contains("Unknown or incomplete command") || line.contains("<--[HERE]")
}
fn dir_mb(p: &std::path::Path, depth: u32) -> u64 {
    if depth > 12 { return 0; }
    let mut t = 0u64;
    if let Ok(rd) = fs::read_dir(p) { for e in rd.flatten() { if let Ok(m) = e.metadata() { if m.is_dir() { t += dir_mb(&e.path(), depth + 1); } else { t += m.len(); } } } }
    t
}
fn free_mb(dir: &std::path::Path) -> i64 {
    #[cfg(unix)]
    { if let Ok(o) = std::process::Command::new("df").arg("-Pk").arg(dir).output() { let t = String::from_utf8_lossy(&o.stdout).to_string(); if let Some(l) = t.lines().nth(1) { if let Some(k) = l.split_whitespace().nth(3).and_then(|x| x.parse::<i64>().ok()) { return k / 1024; } } } }
    let _ = dir; -1
}
pub fn tps_start(app: &Arc<App>) {
    let a = app.clone();
    thread::spawn(move || {
        thread::sleep(Duration::from_secs(20));
        let mut misses = 0u32; let mut was_online = false; let mut tick = 0u32;
        loop {
            // world size + free disk, every ~3 minutes
            if tick % 3 == 0 {
                let ln = crate::config::read_props(&a.dir).into_iter().find(|(k, _)| k == "level-name").map(|x| x.1).filter(|v| !v.is_empty()).unwrap_or_else(|| "world".into());
                let mb = (dir_mb(&a.dir.join(&ln), 0) / 1_048_576) as i64;
                *WORLD.lock().unwrap() = (mb, free_mb(&a.dir));
            }
            tick += 1;
            let sn = a.mc.snapshot(0);
            let online = sn.state == "online";
            if online && !was_online { misses = 0; }
            was_online = online;
            if online && misses < 3 {
                let loader = crate::envinfo::env_cached(&a.dir).0.unwrap_or_default().to_lowercase();
                let cmd = if loader.contains("neoforge") { "neoforge tps" } else if loader.contains("forge") { "forge tps" } else { "tick query" };
                *TPS_FOUND.lock().unwrap() = None; TPS_ARMED.store(true, Ordering::SeqCst);
                a.mc.send(cmd);
                thread::sleep(Duration::from_secs(3));
                TPS_ARMED.store(false, Ordering::SeqCst);
                match TPS_FOUND.lock().unwrap().take() {
                    Some(v) => { misses = 0; let mut h = TPS_HIST.lock().unwrap(); h.push(v); let l = h.len(); if l > 40 { h.drain(..l - 40); } }
                    None => misses += 1,
                }
            }
            thread::sleep(Duration::from_secs(60));
        }
    });
}
pub fn live_stats() -> Vec<(&'static str, J)> {
    let h = TPS_HIST.lock().unwrap().clone();
    let (w, f) = *WORLD.lock().unwrap();
    vec![
        ("tps", h.last().map(|v| J::Num(((*v) * 10.0).round() / 10.0)).unwrap_or(J::Null)),
        ("tps_hist", J::Arr(h.iter().map(|v| J::Num((*v * 10.0).round() / 10.0)).collect())),
        ("world_mb", if w >= 0 { n(w) } else { J::Null }), ("disk_free_mb", if f >= 0 { n(f) } else { J::Null }),
    ]
}

#[cfg(test)]
mod tps_tests {
    use super::*;
    #[test]
    fn captures_and_swallows_probe_output() {
        assert!(!tps_capture("[12:00:00] [Server thread/INFO]: Mean TPS: 12.0")); // not armed: shown normally
        TPS_ARMED.store(true, Ordering::SeqCst); *TPS_FOUND.lock().unwrap() = None;
        assert!(tps_capture("[Server thread/INFO]: Dim minecraft:overworld: Mean tick time: 3.1 ms. Mean TPS: 20.000"));
        assert!(tps_capture("[Server thread/INFO]: Overall: Mean tick time: 60.0 ms. Mean TPS: 16.667"));
        assert_eq!(TPS_FOUND.lock().unwrap().map(|v| (v * 10.0).round() / 10.0), Some(16.7));
        *TPS_FOUND.lock().unwrap() = None;
        assert!(tps_capture("[Server thread/INFO]: Average time per tick: 100.0ms (Target: 50.0ms)"));
        assert_eq!(*TPS_FOUND.lock().unwrap(), Some(10.0));
        assert!(!tps_capture("[Server thread/INFO]: Alex joined the game"));
        TPS_ARMED.store(false, Ordering::SeqCst);
    }
}
