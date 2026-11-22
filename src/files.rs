//! File browser, datapacks and storage tools (port of the Python panel's fs_* functions).
use crate::config::read_props;
use crate::http::{Req, Resp};
use crate::json::{self, n, obj, s, J};
use crate::{sys, App};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::UNIX_EPOCH;

const EDIT_MAX: u64 = 1024 * 1024;
const TAIL: u64 = 256 * 1024;

type R<T> = Result<T, FsErr>;
enum FsErr { Msg(String), Conflict(String) }
fn bad<T>(m: &str) -> R<T> { Err(FsErr::Msg(m.into())) }
impl From<std::io::Error> for FsErr { fn from(e: std::io::Error) -> Self { FsErr::Msg(format!("That didn't work: {}", e)) } }

fn root(app: &App) -> PathBuf { app.dir.canonicalize().unwrap_or_else(|_| app.dir.clone()) }
fn parts(rel: &str) -> R<Vec<String>> {
    let rel = rel.replace('\\', "/");
    if rel.contains('\0') { return bad("That path isn't valid."); }
    let p: Vec<String> = rel.split('/').filter(|x| !x.is_empty() && *x != ".").map(|x| x.to_string()).collect();
    if p.iter().any(|x| x == "..") { return bad("That path isn't valid."); }
    Ok(p)
}
/// A path inside the server folder; symlinks are followed and refused if they lead outside.
fn resolve(app: &App, rel: &str) -> R<PathBuf> {
    let r = root(app);
    let mut p = r.clone();
    for x in parts(rel)? { p.push(x); }
    let p = p.canonicalize().unwrap_or(p);
    if p != r && !p.starts_with(&r) { return bad("That path is outside the server folder."); }
    Ok(p)
}
/// Like resolve, but a symlink at the very end stays itself.
fn resolve_link(app: &App, rel: &str) -> R<PathBuf> {
    let mut ps = parts(rel)?;
    let last = match ps.pop() { Some(l) => l, None => return bad("That's the server folder itself.") };
    Ok(resolve(app, &ps.join("/"))?.join(last))
}
fn rel_of(app: &App, p: &Path) -> String {
    p.strip_prefix(root(app)).map(|x| x.to_string_lossy().replace('\\', "/")).unwrap_or_default()
}
fn name_ok(name: &str) -> R<String> {
    let n_ = name.trim();
    if n_.is_empty() || n_ == "." || n_ == ".." || n_.chars().count() > 200 || n_.contains('/') || n_.contains('\\') || n_.contains('\0') { return bad("That name isn't allowed. Don't use slashes."); }
    Ok(n_.to_string())
}
fn world(app: &App) -> PathBuf {
    let name = read_props(&app.dir).into_iter().find(|(k, _)| k == "level-name").map(|x| x.1).filter(|v| !v.is_empty()).unwrap_or_else(|| "world".into());
    root(app).join(name)
}
fn in_world(app: &App, p: &Path) -> bool { let w = world(app); let w = w.canonicalize().unwrap_or(w); p == w || p.starts_with(&w) }
fn guard_write(app: &App, p: &Path) -> R<()> {
    if app.mc.running() && in_world(app, p) { return bad("The server is running. Stop it before changing world files."); }
    Ok(())
}
fn mtime_ns(m: &fs::Metadata) -> u128 { m.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_nanos()).unwrap_or(0) }
fn mtime_s(m: &fs::Metadata) -> i64 { (mtime_ns(m) / 1_000_000_000) as i64 }

fn trash_target(app: &App, p: &Path, stamp: &str) -> R<PathBuf> {
    let mut dest = root(app).join("panel-trash").join(stamp).join(rel_of(app, p));
    if let Some(par) = dest.parent() { fs::create_dir_all(par)?; }
    let base = dest.clone(); let mut i = 1;
    while dest.symlink_metadata().is_ok() { dest = PathBuf::from(format!("{}.{}", base.display(), i)); i += 1; }
    Ok(dest)
}
fn move_path(from: &Path, to: &Path) -> std::io::Result<()> {
    if fs::rename(from, to).is_ok() { return Ok(()); }
    if from.is_dir() { copy_dir(from, to)?; fs::remove_dir_all(from) } else { fs::copy(from, to)?; fs::remove_file(from) }
}
fn copy_dir(a: &Path, b: &Path) -> std::io::Result<()> {
    fs::create_dir_all(b)?;
    for e in fs::read_dir(a)? { let e = e?; let t = b.join(e.file_name()); if e.path().is_dir() { copy_dir(&e.path(), &t)?; } else { fs::copy(e.path(), &t)?; } }
    Ok(())
}

fn list(app: &App, rel: &str) -> R<J> {
    let d = resolve(app, rel)?;
    if !d.is_dir() { return bad("That isn't a folder."); }
    let rd = fs::read_dir(&d).map_err(|e| FsErr::Msg(format!("Couldn't open that folder: {}", e)))?;
    let names: Vec<_> = rd.flatten().collect();
    let mut items: Vec<(bool, String, J)> = vec![];
    for e in names.iter().take(5000) {
        let fp = e.path();
        let lst = match fp.symlink_metadata() { Ok(m) => m, Err(_) => continue };
        let link = lst.file_type().is_symlink();
        let st = if link { fs::metadata(&fp).unwrap_or(lst.clone()) } else { lst };
        let isdir = fp.is_dir();
        let nm = e.file_name().to_string_lossy().to_string();
        items.push((isdir, nm.to_lowercase(), obj(vec![("name", s(&nm)), ("path", s(&rel_of(app, &fp))), ("dir", J::Bool(isdir)), ("link", J::Bool(link)), ("size", n(if isdir { 0 } else { st.len() as i64 })), ("mtime", n(mtime_s(&st)))])));
    }
    items.sort_by(|a, b| (!a.0, &a.1).cmp(&(!b.0, &b.1)));
    let level = read_props(&app.dir).into_iter().find(|(k, _)| k == "level-name").map(|x| x.1).filter(|v| !v.is_empty()).unwrap_or_else(|| "world".into());
    Ok(obj(vec![("ok", J::Bool(true)), ("path", s(&rel_of(app, &d))), ("items", J::Arr(items.into_iter().map(|x| x.2).collect())), ("truncated", J::Bool(names.len() > 5000)), ("free", J::Null), ("world", s(&level))]))
}

fn read(app: &App, rel: &str) -> R<J> {
    let p = resolve(app, rel)?;
    if !p.is_file() { return bad("That file isn't there."); }
    let md = fs::metadata(&p)?;
    let all = fs::read(&p).map_err(|e| FsErr::Msg(format!("Couldn't read that file: {}", e)))?;
    if all.iter().take(8192).any(|&b| b == 0) { return bad("That looks like a binary file, so it can't be edited here. Use Download."); }
    let truncated = md.len() > EDIT_MAX;
    let data: &[u8] = if truncated { &all[all.len().saturating_sub(TAIL as usize)..] } else { &all };
    let text = match std::str::from_utf8(data) { Ok(t) => t.to_string(), Err(_) if truncated => String::from_utf8_lossy(data).to_string(), Err(_) => return bad("That file isn't plain UTF-8 text, so it can't be edited here. Use Download.") };
    let crlf = text.contains("\r\n");
    let text = if crlf { text.replace("\r\n", "\n") } else { text };
    Ok(obj(vec![("ok", J::Bool(true)), ("path", s(&rel_of(app, &p))), ("name", s(&p.file_name().unwrap_or_default().to_string_lossy())), ("content", s(&text)), ("size", n(md.len() as i64)),
        ("mtime", s(&mtime_ns(&md).to_string())), ("crlf", J::Bool(crlf)), ("truncated", J::Bool(truncated)), ("readonly", J::Bool(truncated)), ("shown", n(data.len() as i64)), ("in_world", J::Bool(in_world(app, &p)))]))
}

fn save(app: &App, rel: &str, content: Option<&str>, mtime: &str, crlf: bool, force: bool) -> R<String> {
    let content = match content { Some(c) => c, None => return bad("Nothing to save.") };
    let p = resolve(app, rel)?;
    if !p.is_file() { return bad("That file isn't there any more."); }
    guard_write(app, &p)?;
    let md = fs::metadata(&p)?;
    if !force && !mtime.is_empty() && mtime_ns(&md).to_string() != mtime { return Err(FsErr::Conflict("That file changed on the Mac since you opened it.".into())); }
    let mut data = content.replace("\r\n", "\n");
    if crlf { data = data.replace('\n', "\r\n"); }
    if data.len() as u64 > EDIT_MAX * 2 { return bad("That's too big to save from the browser."); }
    let t = trash_target(app, &p, &sys::stamp())?;
    fs::copy(&p, &t)?;
    let tmp = PathBuf::from(format!("{}.panel-tmp", p.display()));
    fs::write(&tmp, data.as_bytes())?;
    let _ = fs::set_permissions(&tmp, md.permissions());
    fs::rename(&tmp, &p)?;
    Ok(mtime_ns(&fs::metadata(&p)?).to_string())
}

fn new_item(app: &App, dirrel: &str, name: &str, dir: bool) -> R<String> {
    let d = resolve(app, dirrel)?;
    if !d.is_dir() { return bad("That folder isn't there."); }
    let name = name_ok(name)?;
    let target = d.join(&name);
    guard_write(app, &target)?;
    if target.symlink_metadata().is_ok() { return bad(&format!("{} already exists.", name)); }
    if dir { fs::create_dir(&target)?; } else { fs::OpenOptions::new().write(true).create_new(true).open(&target)?; }
    Ok(rel_of(app, &target))
}

fn rename(app: &App, rel: &str, newname: &str) -> R<String> {
    let p = resolve_link(app, rel)?;
    if p.symlink_metadata().is_err() { return bad("That isn't there any more."); }
    let newname = name_ok(newname)?;
    guard_write(app, &p)?;
    let target = p.parent().unwrap_or(Path::new("")).join(&newname);
    if target.symlink_metadata().is_ok() && target.to_string_lossy().to_lowercase() != p.to_string_lossy().to_lowercase() { return bad(&format!("{} already exists.", newname)); }
    fs::rename(&p, &target)?;
    Ok(rel_of(app, &target))
}

fn delete(app: &App, rels: &[String]) -> (Vec<String>, Vec<String>) {
    let stamp = sys::stamp();
    let trash = root(app).join("panel-trash");
    let (mut done, mut errs) = (vec![], vec![]);
    for rel in rels.iter().take(500) {
        let r: R<()> = (|| {
            let p = resolve_link(app, rel)?;
            if p.symlink_metadata().is_err() { return bad("not there any more"); }
            guard_write(app, &p)?;
            if p == trash || p.starts_with(&trash) {
                if p.is_dir() && !p.symlink_metadata()?.file_type().is_symlink() { fs::remove_dir_all(&p)?; } else { fs::remove_file(&p)?; }
            } else { let t = trash_target(app, &p, &stamp)?; move_path(&p, &t)?; }
            Ok(())
        })();
        match r { Ok(()) => done.push(rel.clone()), Err(FsErr::Msg(m)) | Err(FsErr::Conflict(m)) => errs.push(format!("{}: {}", rel, m)) }
    }
    (done, errs)
}

fn err_json(code: u16, m: &str) -> Resp { Resp::json(code, obj(vec![("ok", J::Bool(false)), ("error", s(m)), ("msg", s(m))]).to_string()) }
fn out(r: R<J>) -> Resp { match r { Ok(j) => Resp::json(200, j.to_string()), Err(FsErr::Msg(m)) | Err(FsErr::Conflict(m)) => err_json(400, &m) } }
fn reply(ok: bool, msg: &str, extra: Vec<(&str, J)>) -> Resp {
    let mut v = vec![("ok", J::Bool(ok)), ("msg", s(msg))]; v.extend(extra);
    Resp::json(200, obj(v).to_string())
}

// ---------------------------------------------------------------- datapacks
fn dp_dir(app: &App, create: bool) -> Option<PathBuf> {
    let w = world(app);
    if !w.is_dir() { return None; }
    let d = w.join("datapacks");
    if create { let _ = fs::create_dir_all(&d); }
    Some(d)
}
fn level_name(app: &App) -> String { read_props(&app.dir).into_iter().find(|(k, _)| k == "level-name").map(|x| x.1).filter(|v| !v.is_empty()).unwrap_or_else(|| "world".into()) }

fn dp_list(app: &App) -> J {
    let mut packs = vec![];
    if let Some(d) = dp_dir(app, false) {
        let mut names: Vec<String> = fs::read_dir(&d).map(|r| r.flatten().map(|e| e.file_name().to_string_lossy().to_string()).collect()).unwrap_or_default();
        names.sort_by_key(|x| x.to_lowercase());
        for fnm in names {
            if fnm.starts_with('.') { continue; }
            let fp = d.join(&fnm);
            if fp.is_file() && fnm.to_lowercase().ends_with(".zip") { packs.push(obj(vec![("name", s(&fnm)), ("kind", s("zip")), ("size", n(fs::metadata(&fp).map(|m| m.len() as i64).unwrap_or(0)))])); }
            else if fp.is_dir() && fp.join("pack.mcmeta").is_file() { packs.push(obj(vec![("name", s(&fnm)), ("kind", s("folder")), ("size", n(0))])); }
        }
    }
    obj(vec![("world", s(&level_name(app))), ("world_exists", J::Bool(dp_dir(app, false).is_some())), ("packs", J::Arr(packs)), ("running", J::Bool(app.mc.running())), ("mc", s(""))])
}

/// Copy a datapack zip into place; a zip wrapped in one extra folder gets re-wrapped so the game can read it.
fn dp_save_zip(src: &Path, dest: &Path) -> Option<String> {
    let names: Vec<String> = sys::zip_list(src).into_iter().filter(|n| !n.starts_with("__MACOSX") && !n.ends_with(".DS_Store")).collect();
    if names.iter().any(|n| n == "pack.mcmeta") { return fs::copy(src, dest).err().map(|e| e.to_string()); }
    let roots: std::collections::BTreeSet<&str> = names.iter().filter(|n| n.ends_with("/pack.mcmeta") && n.matches('/').count() == 1).map(|n| n.rsplit_once('/').unwrap().0).collect();
    if roots.len() != 1 { return Some("That zip has no pack.mcmeta file, so it isn't a datapack (or it's a resource pack or mod).".into()); }
    let root = roots.into_iter().next().unwrap().to_string();
    let tmp = std::env::temp_dir().join(format!("dp-{}-{}", std::process::id(), sys::now() as u64));
    if sys::unzip_to(src, &tmp).is_err() { return Some("That doesn't look like a valid .zip file.".into()); }
    let r = sys::zip_dir(&tmp.join(&root), dest, &[]);
    let _ = fs::remove_dir_all(&tmp);
    r.err()
}

fn dp_upload(app: &App, req: &Req) -> Resp {
    let raw = req.header("x-filename").map(|v| crate::http::url_decode(&v)).unwrap_or_default();
    let base = raw.rsplit(['/', '\\']).next().unwrap_or("").to_string();
    let name: String = base.chars().map(|c| if c.is_alphanumeric() || "_.+-[]() ".contains(c) { c } else { '_' }).collect::<String>().trim().to_string();
    if !name.to_lowercase().ends_with(".zip") || name.starts_with('.') { return err_json(400, "Datapacks are .zip files."); }
    if req.body.is_empty() { return err_json(400, "Empty file."); }
    if req.body.len() > 200 * 1024 * 1024 { return err_json(413, "That file is too big for a datapack (limit 200 MB)."); }
    let dp = match dp_dir(app, true) { Some(d) => d, None => return err_json(400, "This server has no world yet. Start it once so the world gets created, then add datapacks.") };
    let tmp = std::env::temp_dir().join(format!("dpup-{}-{}.zip", std::process::id(), sys::now() as u64));
    if fs::write(&tmp, &req.body).is_err() { return err_json(500, "Couldn't save it."); }
    if !req.body.starts_with(b"PK") { let _ = fs::remove_file(&tmp); return err_json(400, "That doesn't look like a valid .zip file."); }
    let dest = dp.join(&name);
    let res = dp_save_zip(&tmp, &dest);
    let _ = fs::remove_file(&tmp);
    if let Some(e) = res { let _ = fs::remove_file(&dest); return err_json(400, &e); }
    app.mc.log(&format!("[panel] Datapacks: added {}.", name));
    Resp::json(200, obj(vec![("ok", J::Bool(true)), ("msg", s(&format!("Added {}.", name))), ("running", J::Bool(app.mc.running()))]).to_string())
}

static DP_RELOAD: Mutex<f64> = Mutex::new(0.0);

// ---------------------------------------------------------------- storage
fn dir_size(p: &Path) -> i64 {
    let mut t = 0i64;
    if let Ok(rd) = fs::read_dir(p) { for e in rd.flatten() { if let Ok(m) = e.path().symlink_metadata() { if m.is_dir() { t += dir_size(&e.path()); } else if m.is_file() { t += m.len() as i64; } } } }
    t
}
fn storage(app: &App) -> J {
    let r = root(app);
    let mut items: Vec<J> = vec![];
    let worlds: Vec<PathBuf> = fs::read_dir(&r).map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.is_dir() && p.join("level.dat").is_file()).collect()).unwrap_or_default();
    items.push(J::Arr(vec![s(&format!("World{}", if worlds.len() != 1 { "s" } else { "" })), n(worlds.iter().map(|w| dir_size(w)).sum())]));
    for (label, rel) in [("Mods", "mods"), ("Config", "config"), ("Backups made by this server", "panel-backups"), ("Logs", "logs"), ("Crash reports", "crash-reports"), ("Libraries and game files", "libraries"), ("Panel trash (deleted files)", "panel-trash")] {
        let p = r.join(rel);
        if p.is_dir() { items.push(J::Arr(vec![s(label), n(dir_size(&p))])); }
    }
    obj(vec![("items", J::Arr(items)), ("total", n(dir_size(&r))), ("free", J::Null)])
}
fn storage_clean(app: &App, what: &str) -> (bool, String) {
    let cutoff = std::time::SystemTime::now() - std::time::Duration::from_secs(7 * 86400);
    let r = root(app);
    let targets: Vec<PathBuf> = match what {
        "logs" => fs::read_dir(r.join("logs")).map(|rd| rd.flatten().map(|e| e.path()).filter(|p| { let f = p.file_name().unwrap_or_default().to_string_lossy().to_string(); f != "latest.log" && (f.ends_with(".log.gz") || f.ends_with(".log")) }).collect()).unwrap_or_default(),
        "crash" => fs::read_dir(r.join("crash-reports")).map(|rd| rd.flatten().map(|e| e.path()).collect()).unwrap_or_default(),
        _ => return (false, "Unknown clean-up.".into()),
    };
    let (mut freed, mut cnt) = (0u64, 0);
    for p in targets { if let Ok(m) = fs::metadata(&p) { if m.is_file() && m.modified().map(|t| t < cutoff).unwrap_or(false) && fs::remove_file(&p).is_ok() { freed += m.len(); cnt += 1; } } }
    (true, format!("Removed {} old file{} ({:.1} MB).", cnt, if cnt == 1 { "" } else { "s" }, freed as f64 / 1048576.0))
}

// ---------------------------------------------------------------- routes
pub fn handle(app: &Arc<App>, req: &Req) -> Option<Resp> {
    let body = || json::parse(&String::from_utf8_lossy(&req.body)).unwrap_or(J::Null);
    let g = |b: &J, k: &str| b.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
    match (req.method.as_str(), req.path.as_str()) {
        ("GET", "/api/files/list") => Some(out(list(app, &req.q("path").unwrap_or_default()))),
        ("GET", "/api/files/read") => Some(out(read(app, &req.q("path").unwrap_or_default()))),
        ("GET", "/api/files/download") => {
            let p = match resolve(app, &req.q("path").unwrap_or_default()) { Ok(p) => p, Err(FsErr::Msg(m)) | Err(FsErr::Conflict(m)) => return Some(err_json(400, &m)) };
            if p.is_file() { return Some(match fs::read(&p) { Ok(b) => Resp { code: 200, ctype: "application/octet-stream".into(), body: b }, Err(e) => err_json(500, &e.to_string()) }); }
            if p.is_dir() {
                let tmp = std::env::temp_dir().join(format!("dir-{}-{}.zip", std::process::id(), sys::now() as u64));
                let name = p.file_name().map(|x| x.to_string_lossy().to_string()).unwrap_or_else(|| "server".into());
                let res = sys::zip_folder(p.parent().unwrap_or(Path::new(".")), &name, &tmp, &["*/session.lock", "*/.panel-staging/*"]);
                let data = res.ok().and_then(|_| fs::read(&tmp).ok());
                let _ = fs::remove_file(&tmp);
                return Some(match data { Some(b) => Resp { code: 200, ctype: "application/zip".into(), body: b }, None => err_json(500, "Couldn't zip that folder.") });
            }
            Some(err_json(404, "Not found."))
        }
        ("POST", "/api/files/upload") => {
            let hv = |k: &str| req.header(k).map(|v| crate::http::url_decode(&v)).unwrap_or_default();
            let r: R<(PathBuf, String)> = (|| {
                let dd = resolve(app, &hv("x-path"))?;
                let name = name_ok(&hv("x-filename"))?;
                if !dd.is_dir() { return bad("That folder isn't there."); }
                let t = dd.join(&name);
                guard_write(app, &t)?;
                Ok((t, name))
            })();
            let (target, name) = match r { Ok(x) => x, Err(FsErr::Msg(m)) | Err(FsErr::Conflict(m)) => return Some(err_json(400, &m)) };
            if req.body.is_empty() { return Some(err_json(400, "Empty file (drop single files, not folders).")); }
            if target.symlink_metadata().is_ok() && req.header("x-overwrite").as_deref() != Some("1") {
                return Some(Resp::json(409, obj(vec![("ok", J::Bool(false)), ("exists", J::Bool(true)), ("error", s(&format!("{} already exists.", name))), ("msg", s(&format!("{} already exists.", name)))]).to_string()));
            }
            if target.is_dir() { return Some(err_json(400, &format!("{} is a folder.", name))); }
            let stg = root(app).join(".panel-staging");
            let _ = fs::create_dir_all(&stg);
            let tmp = stg.join(format!("up-{}-{}", std::process::id(), (sys::now() * 1000.0) as u64));
            let res: std::io::Result<()> = (|| {
                fs::write(&tmp, &req.body)?;
                if target.symlink_metadata().is_ok() { if let Ok(t) = trash_target(app, &target, &sys::stamp()) { let _ = fs::copy(&target, t); } }
                fs::rename(&tmp, &target)
            })();
            if let Err(e) = res { let _ = fs::remove_file(&tmp); return Some(err_json(500, &format!("Couldn't save the upload: {}", e))); }
            app.mc.log(&format!("[panel] Files: uploaded {}.", rel_of(app, &target)));
            Some(reply(true, "Uploaded.", vec![]))
        }
        ("POST", "/api/files/save") => {
            let b = body();
            let content = b.get("content").and_then(|v| v.as_str());
            let r = save(app, &g(&b, "path"), content, &g(&b, "mtime"), matches!(b.get("crlf"), Some(J::Bool(true))), matches!(b.get("force"), Some(J::Bool(true))));
            Some(match r {
                Ok(mt) => { app.mc.log(&format!("[panel] Files: saved {}.", g(&b, "path"))); reply(true, "Saved.", vec![("mtime", s(&mt))]) }
                Err(FsErr::Conflict(m)) => reply(false, &m, vec![("conflict", J::Bool(true))]),
                Err(FsErr::Msg(m)) => reply(false, &m, vec![]),
            })
        }
        ("POST", "/api/files/new") => {
            let b = body();
            Some(match new_item(app, &g(&b, "dir"), &g(&b, "name"), g(&b, "kind") == "dir") { Ok(p) => reply(true, "Created.", vec![("path", s(&p))]), Err(FsErr::Msg(m)) | Err(FsErr::Conflict(m)) => reply(false, &m, vec![]) })
        }
        ("POST", "/api/files/rename") => {
            let b = body();
            Some(match rename(app, &g(&b, "path"), &g(&b, "name")) {
                Ok(p) => { app.mc.log(&format!("[panel] Files: renamed {} to {}.", g(&b, "path"), p)); reply(true, "Renamed.", vec![("path", s(&p))]) }
                Err(FsErr::Msg(m)) | Err(FsErr::Conflict(m)) => reply(false, &m, vec![]),
            })
        }
        ("POST", "/api/files/delete") => {
            let b = body();
            let rels: Vec<String> = match b.get("paths") { Some(J::Arr(a)) => a.iter().filter_map(|x| x.as_str().map(|v| v.to_string())).collect(), _ => vec![] };
            if rels.is_empty() { return Some(reply(false, "Nothing selected.", vec![])); }
            let (done, errs) = delete(app, &rels);
            if !done.is_empty() { app.mc.log(&format!("[panel] Files: deleted {} (moved to panel-trash).{}", done.iter().take(6).cloned().collect::<Vec<_>>().join(", "), if done.len() > 6 { " and more" } else { "" })); }
            let mut msg = format!("Deleted {} item{}.", done.len(), if done.len() == 1 { "" } else { "s" });
            if !errs.is_empty() { msg += &format!(" Couldn't delete: {}", errs.iter().take(3).cloned().collect::<Vec<_>>().join("; ")); }
            Some(reply(!done.is_empty(), &msg, vec![("errors", J::Arr(errs.iter().map(|e| s(e)).collect()))]))
        }
        ("GET", "/api/dp/list") => Some(Resp::json(200, dp_list(app).to_string())),
        ("POST", "/api/dp/upload") => Some(dp_upload(app, req)),
        ("POST", "/api/dp/delete") => {
            let b = body(); let name = g(&b, "name");
            let d = dp_dir(app, false);
            if d.is_none() || name.is_empty() || name.contains('/') || name.contains('\\') || name.starts_with('.') { return Some(reply(false, "I can't remove that.", vec![])); }
            let fp = d.unwrap().join(&name);
            if fp.is_dir() { let _ = fs::remove_dir_all(&fp); } else if fp.is_file() { let _ = fs::remove_file(&fp); } else { return Some(reply(false, "That datapack isn't there any more.", vec![])); }
            app.mc.log(&format!("[panel] Datapacks: removed {}.", name));
            Some(reply(true, "Removed. Reload or restart to stop it running.", vec![]))
        }
        ("POST", "/api/dp/reload") => {
            if !app.mc.running() { return Some(reply(false, "The server isn't running. Datapacks load when it starts.", vec![])); }
            let mut last = DP_RELOAD.lock().unwrap();
            let wait = 90.0 - (sys::now() - *last);
            if wait > 0.0 { return Some(reply(false, &format!("A reload was just sent. Big modpacks freeze for a while while it runs. Wait about {} seconds before trying again.", wait as i64 + 1), vec![])); }
            *last = sys::now();
            app.mc.log("> reload"); app.mc.send("reload");
            Some(reply(true, "Reload sent. On a big modpack the server can freeze for 10 to 30 seconds. Wait for it to finish, don't press it again.", vec![]))
        }
        ("GET", "/api/storage") => Some(Resp::json(200, storage(app).to_string())),
        ("POST", "/api/storage/clean") => { let b = body(); let (ok, m) = storage_clean(app, &g(&b, "what")); Some(reply(ok, &m, vec![])) }
        _ => None,
    }
}
pub fn start(_app: &Arc<App>) {}
