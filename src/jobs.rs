//! Background jobs (Java install, server create, restore...) the app polls via /api/hub/job?id=.
use crate::json::{n, s, J};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

pub type Job = Arc<Mutex<Vec<(String, J)>>>;
static JOBS: Mutex<Option<HashMap<String, Job>>> = Mutex::new(None);

pub fn new_id() -> String {
    use std::hash::{BuildHasher, Hasher};
    use std::sync::atomic::{AtomicU64, Ordering};
    static C: AtomicU64 = AtomicU64::new(1);
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u64(C.fetch_add(1, Ordering::SeqCst)); h.write_u64((crate::sys::now() * 1e6) as u64);
    format!("{:010x}", h.finish() & 0xff_ffff_ffff)
}
pub fn create(name: &str, register: bool) -> (String, Job) {
    let id = new_id();
    let j: Job = Arc::new(Mutex::new(vec![
        ("id".into(), s(&id)), ("step".into(), s("Starting...")), ("pct".into(), n(1)), ("done".into(), J::Bool(false)), ("ok".into(), J::Null),
        ("msg".into(), s("")), ("lines".into(), J::Arr(vec![])), ("name".into(), s(name)),
    ]));
    if register { JOBS.lock().unwrap().get_or_insert_with(HashMap::new).insert(id.clone(), j.clone()); }
    (id, j)
}
pub fn get(id: &str) -> Option<J> { JOBS.lock().unwrap().as_ref()?.get(id).map(|j| J::Obj(j.lock().unwrap().clone())) }
pub fn set(j: &Job, k: &str, v: J) { let mut g = j.lock().unwrap(); if let Some(e) = g.iter_mut().find(|(a, _)| a == k) { e.1 = v; } else { g.push((k.to_string(), v)); } }
pub fn step(j: &Job, text: &str, pct: i64) { set(j, "step", s(text)); set(j, "pct", n(pct)); }
pub fn log(j: &Job, line: &str) {
    let l = line.trim_end(); if l.is_empty() { return; }
    let mut g = j.lock().unwrap();
    let e = g.iter_mut().find(|(a, _)| a == "lines").unwrap();
    if let J::Arr(v) = &mut e.1 { v.push(s(l)); let len = v.len(); if len > 40 { v.drain(..len - 40); } }
}
pub fn finish(j: &Job, ok: bool, msg: &str) { set(j, "done", J::Bool(true)); set(j, "ok", J::Bool(ok)); set(j, "msg", s(msg)); }
pub fn get_str(j: &Job, k: &str) -> String { j.lock().unwrap().iter().find(|(a, _)| a == k).and_then(|(_, v)| v.as_str().map(|x| x.to_string())).unwrap_or_default() }
pub fn is_ok(j: &Job) -> bool { matches!(j.lock().unwrap().iter().find(|(a, _)| a == "ok").map(|x| &x.1), Some(J::Bool(true))) }
pub fn get_obj(j: &Job) -> Option<J> { Some(J::Obj(j.lock().unwrap().clone())) }
