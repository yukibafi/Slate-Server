//! Mod / browse result icons. Downloaded through the engine (it fetches from Modrinth / CurseForge), cached on disk,
//! and shown through two parallel models (images + "has image") so no Image is ever held off the UI thread.
use crate::{client, with_ui, App, Shared};
use slint::{Image, Model, ModelRc, VecModel, Weak};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

fn dir() -> PathBuf { let d = std::env::temp_dir().join("create-panel-icons"); let _ = std::fs::create_dir_all(&d); d }
fn stem(url: &str) -> String { let mut h = DefaultHasher::new(); url.hash(&mut h); format!("{:x}", h.finish()) }
/// Slint picks the decoder from the file extension, so the file must be saved with the right one.
fn ext_of(b: &[u8]) -> Option<&'static str> {
    if b.starts_with(&[0x89, b'P', b'N', b'G']) { Some("png") } else if b.starts_with(&[0xff, 0xd8]) { Some("jpg") } else if b.starts_with(b"GIF8") { Some("gif") }
    else if b.len() > 12 && &b[0..4] == b"RIFF" && &b[8..12] == b"WEBP" { Some("webp") } else if b.starts_with(b"BM") { Some("bmp") }
    else if String::from_utf8_lossy(&b[..b.len().min(300)]).contains("<svg") { Some("svg") } else { None }
}
fn cached(url: &str) -> Option<PathBuf> {
    let st = stem(url);
    for e in ["png", "jpg", "gif", "webp", "bmp", "svg"] { let p = dir().join(format!("{st}.{e}")); if std::fs::metadata(&p).map(|m| m.len() > 16).unwrap_or(false) { return Some(p); } }
    None
}

fn build(urls: Vec<Option<String>>) -> (ModelRc<Image>, ModelRc<bool>) {
    let (mut imgs, mut has) = (vec![], vec![]);
    for u in urls { match u.and_then(|u| cached(&u)).and_then(|p| Image::load_from_path(&p).ok()) { Some(i) => { imgs.push(i); has.push(true); } None => { imgs.push(Image::default()); has.push(false); } } }
    (ModelRc::new(VecModel::from(imgs)), ModelRc::new(VecModel::from(has)))
}
/// UI thread: refresh the icon models for the rows currently shown.
pub fn apply(a: &App, st: &Shared) {
    let (mi, bi) = { let g = st.lock().unwrap(); (g.x.icons.clone(), g.x.br_icons.clone()) };
    let m = a.get_mods(); let urls: Vec<Option<String>> = (0..m.row_count()).map(|i| m.row_data(i).and_then(|r| mi.get(r.file.as_str()).cloned())).collect();
    let (i1, h1) = build(urls); a.set_mod_icons(i1); a.set_mod_has(h1);
    let b = a.get_br(); let urls: Vec<Option<String>> = (0..b.row_count()).map(|i| b.row_data(i).and_then(|r| bi.get(r.id.as_str()).cloned())).collect();
    let (i2, h2) = build(urls); a.set_br_icons(i2); a.set_br_has(h2);
}
/// Download any icons not on disk yet (4 at a time), re-applying as they land.
pub fn fetch(ui: &Weak<App>, st: &Shared, urls: Vec<String>) {
    let Some(c) = client(st) else { return };
    let id = st.lock().unwrap().sel.id.clone(); if id.is_empty() { return; }
    let mut todo: Vec<String> = urls.into_iter().filter(|u| u.starts_with("https://") && cached(u).is_none()).collect(); todo.sort(); todo.dedup();
    if todo.is_empty() { return; }
    let queue = Arc::new(Mutex::new(todo));
    for _ in 0..4 {
        let (q, c, ui, st, id) = (queue.clone(), c.clone(), ui.clone(), st.clone(), id.clone());
        std::thread::spawn(move || loop {
            let Some(url) = q.lock().unwrap().pop() else { break };
            let Ok((200, body)) = c.raw("GET", &format!("/api/s/{id}/api/img?u={}", crate::extra::enc(&url)), None) else { continue };
            if body.len() < 16 { continue; }
            let Some(ext) = ext_of(&body) else { continue };
            let _ = std::fs::write(dir().join(format!("{}.{ext}", stem(&url))), body);
            let st2 = st.clone(); with_ui(&ui, move |a| apply(a, &st2));
        });
    }
}
