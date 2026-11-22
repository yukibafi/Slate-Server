//! Player heads (from skins) and play-time stats.
use crate::{arr, client, gn, gs, sc, with_ui, App, PStat, PlayerRow, Shared};
use slint::{Image, Model, ModelRc, SharedString, VecModel, Weak};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

static TRIED: Mutex<Option<HashMap<String, u64>>> = Mutex::new(None);
fn now() -> u64 { SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) }
fn dir() -> PathBuf { let d = std::env::temp_dir().join("create-panel-skins"); let _ = std::fs::create_dir_all(&d); d }
fn file_for(name: &str) -> PathBuf { dir().join(format!("{}.png", name.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '_').collect::<String>())) }
pub fn cached(name: &str) -> Option<PathBuf> {
    let p = file_for(name);
    let m = std::fs::metadata(&p).ok()?;
    if m.len() < 100 { return None; }
    Some(p)
}
/// Download missing skins in the background, then fill in the heads on every list.
pub fn prefetch(ui: &Weak<App>, st: &Shared, names: Vec<String>) {
    let missing: Vec<String> = {
        let mut g = TRIED.lock().unwrap(); let m = g.get_or_insert_with(HashMap::new); let t = now();
        let mut out = vec![];
        for n in names { if n.is_empty() || cached(&n).is_some() { continue; } if m.get(&n).map(|x| t - x <= 600).unwrap_or(false) { continue; } m.insert(n.clone(), t); out.push(n); }
        out
    };
    if missing.is_empty() { return; }
    let (ui, st) = (ui.clone(), st.clone());
    std::thread::spawn(move || {
        let (Some(c), id) = (client(&st), st.lock().unwrap().sel.id.clone()) else { return };
        if id.is_empty() { return; }
        let mut any = false;
        for n in missing {
            if let Ok((200, body)) = c.raw("GET", &sc(&c, &id, &format!("/api/skin?name={}", crate::extra::enc(&n))), None) {
                if body.len() > 100 && body.starts_with(&[0x89, b'P', b'N', b'G']) { let _ = std::fs::write(file_for(&n), body); any = true; }
            }
        }
        if any { with_ui(&ui, apply_heads); }
    });
}
fn load(name: &str) -> Option<Image> { cached(name).and_then(|p| Image::load_from_path(&p).ok()) }
pub fn row(name: &str) -> PlayerRow {
    match load(name) { Some(i) => PlayerRow { name: name.into(), note: SharedString::new(), head: i, has_head: true }, None => PlayerRow { name: name.into(), note: SharedString::new(), head: Image::default(), has_head: false } }
}
fn fix_rows(m: ModelRc<PlayerRow>) {
    for i in 0..m.row_count() {
        let Some(mut r) = m.row_data(i) else { continue };
        if r.has_head { continue; }
        if let Some(img) = load(r.name.as_str()) { r.head = img; r.has_head = true; m.set_row_data(i, r); }
    }
}
pub fn apply_heads(a: &App) {
    fix_rows(a.get_online()); fix_rows(a.get_wl()); fix_rows(a.get_ops()); fix_rows(a.get_banned());
    let m = a.get_pstats();
    for i in 0..m.row_count() {
        let Some(mut r) = m.row_data(i) else { continue };
        if r.has_head { continue; }
        if let Some(img) = load(r.name.as_str()) { r.head = img; r.has_head = true; m.set_row_data(i, r); }
    }
}
fn dur(sec: i64) -> String { if sec >= 3600 { format!("{}h {}m", sec / 3600, (sec % 3600) / 60) } else if sec >= 60 { format!("{}m", sec / 60) } else { format!("{}s", sec.max(0)) } }
pub fn load_stats(ui: &Weak<App>, st: &Shared) {
    let (Some(c), id) = (client(st), st.lock().unwrap().sel.id.clone()) else { return };
    if id.is_empty() { return; }
    let Ok(j) = c.get(&sc(&c, &id, "/api/players/stats")) else { return };
    let t = now() as i64;
    let rows: Vec<(String, String, String, String, String, bool)> = arr(&j, "players").iter().map(|p| {
        let last = gn(p, "last"); let first = gn(p, "first");
        (gs(p, "name"), dur(gn(p, "seconds")), gn(p, "joins").to_string(), if last > 0 { crate::extra::ago(t - last) } else { "-".into() }, if first > 0 { crate::extra::ago(t - first) } else { "-".into() }, matches!(p.get("online"), Some(crate::J::Bool(true))))
    }).collect();
    let names: Vec<String> = rows.iter().map(|r| r.0.clone()).collect();
    let total = rows.len();
    let lists = c.get(&sc(&c, &id, "/api/lists")).ok();
    let has = |k: &str, n: &str| lists.as_ref().map(|j| arr(j, k).iter().any(|x| x.as_str().map(|t| t.eq_ignore_ascii_case(n)).unwrap_or(false))).unwrap_or(false);
    let tags: Vec<String> = names.iter().map(|n| { let mut v: Vec<&str> = vec![]; if has("ops", n) { v.push("Op"); } if has("whitelist", n) { v.push("Whitelisted"); } if has("banned", n) { v.push("Banned"); } v.join("  ·  ") }).collect();
    let online_n = rows.iter().filter(|r| r.5).count();
    with_ui(ui, move |a| {
        let v: Vec<PStat> = rows.into_iter().zip(tags.into_iter()).map(|((n, pt, jn, last, first, on), tg)| { let img = load(&n); PStat { has_head: img.is_some(), head: img.unwrap_or_default(), name: n.into(), playtime: pt.into(), joins: jn.into(), last: last.into(), first: first.into(), online: on, tags: tg.into() } }).collect();
        let _ = online_n;
        a.set_pstats_note(SharedString::from(format!("{total} player{} seen", if total == 1 { "" } else { "s" })));
        a.set_pstats(ModelRc::new(VecModel::from(v)));
    });
    prefetch(ui, st, names);
}
