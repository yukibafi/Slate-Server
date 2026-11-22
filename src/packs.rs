//! Modpack export (zip / Modrinth / CurseForge / Prism), compare-with-launcher, and adding Modrinth modpacks.
use crate::browse::{av, cf_fingerprint, cf_json, cf_key, enc, file_hash, iv, jo, loaders_browse, mods_dir, mr, qs, reply, sa, so, sv};
use crate::http::{Req, Resp};
use crate::json::{self, n, obj, s, J};
use crate::mods::{self, parse_filename};
use crate::{dl, envinfo, jobs, sys, App};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;

const FORMATS: &[&str] = &["zip", "mrpack", "curseforge", "prism"];
const FOLDERS: &[&str] = &["config", "defaultconfigs", "kubejs", "scripts", "global_packs", "openloader", "packmenu", "fancymenu_data", "paxi", "resourcepacks", "shaderpacks"];
const PACK_HOSTS: &[&str] = &["cdn.modrinth.com", "github.com", "raw.githubusercontent.com", "gitlab.com"];
const KEEP: &[&str] = &["server.properties", "ops.json", "whitelist.json", "banned-players.json", "banned-ips.json", "eula.txt", "user_jvm_args.txt", "start.sh", "usercache.json"];
static PACKS: Mutex<Vec<(String, PathBuf, String, &'static str)>> = Mutex::new(Vec::new());
static CMP: Mutex<Vec<String>> = Mutex::new(Vec::new());

fn dest_name(f: &str) -> String { if f.to_lowercase().ends_with(".disabled") { f[..f.len() - 9].to_string() } else { f.to_string() } }
fn pack_mods(app: &App) -> Vec<J> { mods::list_mods(app).into_iter().filter(|m| (matches!(m.get("enabled"), Some(J::Bool(true))) || matches!(m.get("client_only"), Some(J::Bool(true)))) && !matches!(m.get("server_only"), Some(J::Bool(true)))).collect() }
fn staging(app: &App) -> PathBuf { let d = app.dir.join(".panel-staging"); let _ = fs::create_dir_all(&d); d }
fn link_or_copy(a: &Path, b: &Path) -> std::io::Result<()> { if let Some(p) = b.parent() { fs::create_dir_all(p)?; } if fs::hard_link(a, b).is_ok() { return Ok(()); } fs::copy(a, b).map(|_| ()) }
fn copy_configs(app: &App, into: &Path) {
    fn walk(root: &Path, base: &Path, into: &Path) {
        let Ok(rd) = fs::read_dir(root) else { return };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() { walk(&p, base, into); continue; }
            let nm = e.file_name().to_string_lossy().to_string();
            if nm.ends_with(".log") || nm.ends_with(".lock") { continue; }
            if let Ok(rel) = p.strip_prefix(base) { let _ = link_or_copy(&p, &into.join(rel)); }
        }
    }
    for f in FOLDERS { let r = app.dir.join(f); if r.is_dir() { walk(&r, &app.dir, into); } }
}
fn zip_dir(dir: &Path, out: &Path, store: bool) -> Result<(), String> {
    let _ = fs::remove_file(out);
    let o = out.to_string_lossy().to_string();
    let mut a = vec!["-q", "-r", "-y"]; if store { a.push("-0"); } a.push(&o); a.push(".");
    match sys::run("zip", &a, Some(dir)) { Some((0, _)) => Ok(()), _ => Err("Couldn't build the zip (is the zip tool installed?).".into()) }
}
fn newest_dir(d: &Path) -> Option<String> {
    let mut v: Vec<String> = fs::read_dir(d).ok()?.flatten().map(|e| e.file_name().to_string_lossy().to_string()).filter(|n| n.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false)).collect();
    v.sort_by(|a, b| envinfo::vkey(a).cmp(&envinfo::vkey(b))); v.pop()
}
fn pack_env(app: &App) -> Result<(String, String, String), String> {
    let (loader, mc, _, ver) = envinfo::env_cached(&app.dir);
    let loader = loader.filter(|l| ["neoforge", "forge", "fabric", "quilt"].contains(&l.as_str())).ok_or("This server doesn't use NeoForge, Forge, Fabric or Quilt, so there's no loader to put in a pack. Use the plain zip instead.")?;
    let mc = mc.ok_or("I can't tell which Minecraft version this server is. Set it in Settings first.")?;
    let mut ver = ver.filter(|v| !v.is_empty());
    if ver.is_none() && (loader == "fabric" || loader == "quilt") {
        let sub: &[&str] = if loader == "fabric" { &["net", "fabricmc", "fabric-loader"] } else { &["org", "quiltmc", "quilt-loader"] };
        let mut p = app.dir.join("libraries"); for x in sub { p = p.join(x); }
        ver = newest_dir(&p);
        if ver.is_none() {
            let url = if loader == "fabric" { "https://meta.fabricmc.net/v2/versions/loader" } else { "https://meta.quiltmc.org/v3/versions/loader" };
            if let Ok(J::Arr(l)) = dl::get_json(url, &["meta.fabricmc.net", "meta.quiltmc.org"]) {
                let st: Vec<&J> = l.iter().filter(|x| !matches!(x.get("stable"), Some(J::Bool(false))) && !sv(x, "version").contains('-')).collect();
                ver = st.first().copied().or(l.first()).map(|x| sv(x, "version")).filter(|v| !v.is_empty());
            }
        }
    }
    let ver = ver.ok_or_else(|| format!("I can't tell which {} version this server uses, so I can't write the pack. Use the plain zip instead.", loader))?;
    Ok((loader, mc, ver))
}
fn plain_info(app: &App, mods: &[J], include_config: bool) -> String {
    let loader = envinfo::env_cached(&app.dir).0;
    let mut l = vec!["Modpack exported from the Create Server Panel".to_string(), String::new()];
    if let Some(x) = loader { l.push(format!("Needs: {x}")); }
    l.push(format!("Mods: {}", mods.len())); l.push(String::new());
    l.push("How to use: unzip this into your Minecraft folder (or your launcher instance's folder).".into());
    l.push(format!("It adds a mods/ folder{}.", if include_config { " plus config/ and kubejs/ if they exist" } else { "" }));
    l.push("Use the same loader and Minecraft version as above, then join the server.".into()); l.push(String::new()); l.push("Mod list:".into());
    for m in mods { l.push(format!("  - {} {} ({}){}", sv(m, "name"), sv(m, "version"), sv(m, "file"), if matches!(m.get("client_only"), Some(J::Bool(true))) { "  [client-side only]" } else { "" })); }
    l.join("\n") + "\n"
}
fn build_pack(app: &App, fmt: &str, include_config: bool) -> Result<J, String> {
    let fmt = if FORMATS.contains(&fmt) { fmt } else { "zip" };
    let mods = pack_mods(app);
    if mods.is_empty() { return Err("There are no mods to put in the pack.".into()); }
    let name = { let t = mods::pretty_name(&app.dir.file_name().map(|x| x.to_string_lossy().to_string()).unwrap_or_else(|| "server".into())); if t.is_empty() { "Server".to_string() } else { t } };
    let safe = { let t: String = name.chars().map(|c| if c.is_ascii_alphanumeric() || "._-".contains(c) { c } else { '-' }).collect(); let t = t.trim_matches('-').to_string(); if t.is_empty() { "server".into() } else { t } };
    let id = jobs::new_id();
    let stg = staging(app); let lay = stg.join(format!("layout-{id}")); let tmp = stg.join(format!("modpack-{id}.zip"));
    let _ = fs::remove_dir_all(&lay); fs::create_dir_all(&lay).map_err(|e| e.to_string())?;
    let (mut notes, mut linked) = (Vec::<String>::new(), 0usize);
    let mut ctype = "application/zip";
    let (filename, summary);
    let src = |m: &J| mods_dir(app).join(sv(m, "file"));
    let res = (|| -> Result<(), String> {
        if fmt == "zip" {
            for m in &mods { link_or_copy(&src(m), &lay.join("mods").join(dest_name(&sv(m, "file")))).map_err(|e| e.to_string())?; }
            if include_config { copy_configs(app, &lay); }
            fs::write(lay.join("modpack-info.txt"), plain_info(app, &mods, include_config)).map_err(|e| e.to_string())?;
            return zip_dir(&lay, &tmp, true);
        }
        let (loader, mc, lver) = pack_env(app)?;
        let lkey = match loader.as_str() { "neoforge" => "neoforge", "forge" => "forge", "fabric" => "fabric-loader", _ => "quilt-loader" };
        match fmt {
            "mrpack" => {
                let mut by_hash: Vec<(String, &J)> = vec![]; for m in &mods { if let Some(h) = sys::sha1_file(&src(m)) { by_hash.push((h, m)); } }
                let mut found: HashMap<String, J> = HashMap::new();
                let hs: Vec<String> = by_hash.iter().map(|x| x.0.clone()).collect();
                let mut err: Option<String> = None;
                for ch in hs.chunks(100) {
                    match mr("POST", "/version_files", Some(&obj(vec![("hashes", J::Arr(ch.iter().map(|x| s(x)).collect())), ("algorithm", s("sha1"))]))) {
                        Ok(J::Obj(m)) => for (h, ver) in m { for f in av(&ver, "files") { if f.get("hashes").map(|x| sv(x, "sha1")) == Some(h.clone()) && !sv(&f, "url").is_empty() { found.insert(h.clone(), f); break; } } },
                        Ok(_) => {}, Err(e) => { err = Some(e); break; }
                    }
                }
                if let Some(e) = err { found.clear(); notes.push(format!("Couldn't reach Modrinth ({e}), so every mod is bundled inside the file instead of linked.")); }
                let mut files = vec![]; let mut bundled = vec![];
                for (h, m) in &by_hash {
                    if let Some(f) = found.get(h) {
                        files.push(obj(vec![("path", s(&format!("mods/{}", dest_name(&sv(m, "file"))))), ("hashes", obj(vec![("sha1", s(h)), ("sha512", s(&sys::sha_file(&src(m), 512).unwrap_or_default()))])),
                            ("env", obj(vec![("client", s("required")), ("server", s(if matches!(m.get("client_only"), Some(J::Bool(true))) { "unsupported" } else { "required" }))])),
                            ("downloads", J::Arr(vec![s(&sv(f, "url"))])), ("fileSize", n(fs::metadata(src(m)).map(|x| x.len() as i64).unwrap_or(0)))]));
                    } else { bundled.push(*m); }
                }
                let idx = obj(vec![("formatVersion", n(1)), ("game", s("minecraft")), ("versionId", s("1.0.0")), ("name", s(&name)), ("summary", s("Exported from the Create Server Panel")), ("files", J::Arr(files.clone())), ("dependencies", obj(vec![("minecraft", s(&mc)), (lkey, s(&lver))]))]);
                fs::write(lay.join("modrinth.index.json"), idx.to_string()).map_err(|e| e.to_string())?;
                for m in &bundled { link_or_copy(&src(m), &lay.join("overrides/mods").join(dest_name(&sv(m, "file")))).map_err(|e| e.to_string())?; }
                if include_config { copy_configs(app, &lay.join("overrides")); }
                linked = files.len(); ctype = "application/x-modrinth-modpack+zip";
                zip_dir(&lay, &tmp, false)?;
                return Ok(());
            }
            "curseforge" => {
                let mut matched: HashMap<String, (i64, i64)> = HashMap::new();
                if cf_key().is_some() {
                    let mut fps: HashMap<i64, &J> = HashMap::new(); for m in &mods { if let Some(f) = cf_fingerprint(&src(m)) { fps.insert(f as i64, m); } }
                    let keys: Vec<i64> = fps.keys().cloned().collect();
                    for ch in keys.chunks(100) {
                        match cf_json("POST", &format!("/fingerprints/{}", 432), Some(&obj(vec![("fingerprints", J::Arr(ch.iter().map(|k| n(*k)).collect()))]))) {
                            Ok(j) => for em in j.get("data").map(|d| av(d, "exactMatches")).unwrap_or_default() { let f = em.get("file").cloned().unwrap_or(J::Null); if let Some(m) = fps.get(&iv(&f, "fileFingerprint")) { if iv(&em, "id") != 0 && iv(&f, "id") != 0 { matched.insert(sv(m, "file"), (iv(&em, "id"), iv(&f, "id"))); } } },
                            Err(e) => { matched.clear(); notes.push(format!("CurseForge lookup failed ({e}), so the mods are bundled inside the pack instead of linked.")); break; }
                        }
                    }
                } else { notes.push("No CurseForge API key is saved (Settings), so the mods are bundled inside the pack instead of linked. It still imports fine.".into()); }
                let files: Vec<J> = mods.iter().filter_map(|m| matched.get(&sv(m, "file"))).map(|(p, f)| obj(vec![("projectID", n(*p)), ("fileID", n(*f)), ("required", J::Bool(true))])).collect();
                let manifest = obj(vec![("minecraft", obj(vec![("version", s(&mc)), ("modLoaders", J::Arr(vec![obj(vec![("id", s(&format!("{loader}-{lver}"))), ("primary", J::Bool(true))])]))])), ("manifestType", s("minecraftModpack")), ("manifestVersion", n(1)), ("name", s(&name)), ("version", s("1.0.0")), ("author", s("Create Server Panel")), ("files", J::Arr(files.clone())), ("overrides", s("overrides"))]);
                fs::write(lay.join("manifest.json"), manifest.to_string()).map_err(|e| e.to_string())?;
                for m in &mods { if !matched.contains_key(&sv(m, "file")) { link_or_copy(&src(m), &lay.join("overrides/mods").join(dest_name(&sv(m, "file")))).map_err(|e| e.to_string())?; } }
                if include_config { copy_configs(app, &lay.join("overrides")); }
                linked = files.len();
                zip_dir(&lay, &tmp, false)?;
                return Ok(());
            }
            _ => {
                let uid = match loader.as_str() { "neoforge" => "net.neoforged", "forge" => "net.minecraftforge", "fabric" => "net.fabricmc.fabric-loader", _ => "org.quiltmc.quilt-loader" };
                let ram = crate::misc::max_ram_mb(&app.dir).unwrap_or(6144).clamp(4096, 8192);
                fs::write(lay.join("instance.cfg"), format!("InstanceType=OneSix\nname={name}\niconKey=default\nOverrideMemory=true\nMinMemAlloc=2048\nMaxMemAlloc={ram}\n")).map_err(|e| e.to_string())?;
                fs::write(lay.join("mmc-pack.json"), obj(vec![("components", J::Arr(vec![obj(vec![("uid", s("net.minecraft")), ("version", s(&mc)), ("important", J::Bool(true))]), obj(vec![("uid", s(uid)), ("version", s(&lver))])])), ("formatVersion", n(1))]).to_string()).map_err(|e| e.to_string())?;
                for m in &mods { link_or_copy(&src(m), &lay.join(".minecraft/mods").join(dest_name(&sv(m, "file")))).map_err(|e| e.to_string())?; }
                if include_config { copy_configs(app, &lay.join(".minecraft")); }
                zip_dir(&lay, &tmp, true)?;
                return Ok(());
            }
        }
    })();
    let _ = fs::remove_dir_all(&lay);
    if let Err(e) = res { let _ = fs::remove_file(&tmp); return Err(e); }
    let lname = match envinfo::env_cached(&app.dir).0.as_deref() { Some("neoforge") => "NeoForge", Some("forge") => "Forge", Some("fabric") => "Fabric", Some("quilt") => "Quilt", _ => "" };
    match fmt {
        "zip" => { filename = format!("{safe}-modpack.zip"); summary = format!("Plain zip with {} mods.", mods.len()); }
        "mrpack" => { filename = format!("{safe}.mrpack"); summary = format!("Modrinth pack: {} linked, {} bundled inside.", linked, mods.len() - linked); }
        "curseforge" => { filename = format!("{safe}-curseforge.zip"); summary = format!("CurseForge pack: {} linked, {} bundled inside.", linked, mods.len() - linked); }
        _ => { let (_, mc, lv) = pack_env(app).unwrap_or_else(|_| (String::new(), String::new(), String::new())); filename = format!("{safe}-prism.zip"); summary = format!("Prism instance with {} mods ({lname} {lv}, Minecraft {mc}).", mods.len()); }
    }
    { let mut g = PACKS.lock().unwrap(); g.retain(|p| p.1.is_file()); g.push((id.clone(), tmp.clone(), filename.clone(), ctype)); }
    Ok(obj(vec![("ok", J::Bool(true)), ("id", s(&id)), ("filename", s(&filename)), ("size", n(fs::metadata(&tmp).map(|m| m.len() as i64).unwrap_or(0))), ("mods", n(mods.len() as i64)), ("linked", n(linked as i64)), ("notes", J::Arr(notes.iter().map(|x| s(x)).collect())), ("summary", s(&summary))]))
}

// ---- compare with launcher
fn norm(x: &str) -> String { x.chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>().to_lowercase() }
fn compare(app: &App, b: &J) -> Resp {
    let mut mine: Vec<(String, String)> = vec![];
    for x in av(b, "mods").iter().take(2000) { let h = sv(x, "sha1"); if h.len() == 40 && h.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()) { if !mine.iter().any(|m| m.0 == h) { mine.push((h, sv(x, "name").chars().take(200).collect())); } } }
    if av(b, "mods").is_empty() { return reply(false, "No mods were sent."); }
    if mine.is_empty() { return reply(false, "I couldn't read any mod files."); }
    let (loader, mc, label, _) = envinfo::env_cached(&app.dir);
    let all = mods::list_mods(app);
    let srv: Vec<&J> = all.iter().filter(|m| matches!(m.get("enabled"), Some(J::Bool(true))) || matches!(m.get("client_only"), Some(J::Bool(true)))).collect();
    let srv_on: Vec<&J> = srv.iter().filter(|m| matches!(m.get("enabled"), Some(J::Bool(true)))).cloned().collect();
    let mut srv_hash: HashMap<String, &J> = HashMap::new(); let mut hash_of: HashMap<String, String> = HashMap::new();
    for m in &srv { let h = file_hash(&mods_dir(app).join(sv(m, "file"))); if !h.is_empty() { srv_hash.insert(h.clone(), m); hash_of.insert(sv(m, "file"), h); } }
    let mut known: HashMap<String, J> = HashMap::new(); let mut latest: HashMap<String, J> = HashMap::new(); let mut projects: HashMap<String, J> = HashMap::new();
    let r = (|| -> Result<(), String> {
        let mut hs: Vec<String> = mine.iter().map(|x| x.0.clone()).chain(srv_hash.keys().cloned()).collect(); hs.sort(); hs.dedup();
        for ch in hs.chunks(100) { if let J::Obj(m) = mr("POST", "/version_files", Some(&obj(vec![("hashes", J::Arr(ch.iter().map(|x| s(x)).collect())), ("algorithm", s("sha1"))])))? { for (k, v) in m { known.insert(k, v); } } }
        let want: Vec<String> = mine.iter().map(|x| x.0.clone()).filter(|h| known.contains_key(h)).collect();
        if loader.is_some() || mc.is_some() {
            for ch in want.chunks(100) {
                let mut q = vec![("hashes", J::Arr(ch.iter().map(|x| s(x)).collect())), ("algorithm", s("sha1"))];
                if let Some(l) = &loader { q.push(("loaders", J::Arr(loaders_browse(l).iter().map(|x| s(x)).collect()))); }
                if let Some(m) = &mc { q.push(("game_versions", J::Arr(vec![s(m)]))); }
                if let J::Obj(m) = mr("POST", "/version_files/update", Some(&obj(q)))? { for (k, v) in m { latest.insert(k, v); } }
            }
        }
        let mut pids: Vec<String> = want.iter().filter_map(|h| known.get(h).and_then(|k| so(k, "project_id"))).collect(); pids.sort(); pids.dedup();
        for ch in pids.chunks(80) { if let J::Arr(a) = mr("GET", &format!("/projects?ids={}", enc(&J::Arr(ch.iter().map(|x| s(x)).collect()).to_string())), None)? { for p in a { projects.insert(sv(&p, "id"), p); } } }
        Ok(())
    })();
    if let Err(e) = r { return reply(false, &e); }
    let pid_of_srv = |m: &J| -> Option<String> { hash_of.get(&sv(m, "file")).and_then(|h| known.get(h)).and_then(|k| so(k, "project_id")).or_else(|| if sv(m, "source") == "modrinth" { so(m, "project_id") } else { None }) };
    let srv_pids: HashSet<String> = srv.iter().filter_map(|m| pid_of_srv(m)).collect();
    let name_set = |m: &J| -> HashSet<String> { [sv(m, "modid"), sv(m, "name"), parse_filename(&sv(m, "file"), None).0].iter().map(|x| norm(x)).filter(|x| !x.is_empty()).collect() };
    let mut srv_names = HashSet::new(); for m in &srv { srv_names.extend(name_set(m)); }
    let my_pids: HashSet<String> = mine.iter().filter_map(|(h, _)| known.get(h).and_then(|k| so(k, "project_id"))).collect();
    let my_names: HashSet<String> = mine.iter().map(|(_, f)| norm(&parse_filename(f, None).0)).filter(|x| !x.is_empty()).collect();
    let (mut mine_rows, mut missing, mut seen) = (vec![], vec![], HashSet::new());
    for (h, fname) in &mine {
        let k = known.get(h); let pid = k.and_then(|k| so(k, "project_id")); let pj = pid.as_ref().and_then(|p| projects.get(p)).cloned().unwrap_or(J::Null);
        let slug = so(&pj, "slug");
        let mut row = vec![("name", s(&so(&pj, "title").unwrap_or_else(|| fname.clone()))), ("icon", pj.get("icon_url").cloned().unwrap_or(J::Null)), ("file", s(fname)), ("id", jo(pid.clone())), ("page", slug.map(|x| s(&format!("https://modrinth.com/mod/{x}"))).unwrap_or(J::Null))];
        if let Some(p) = &pid { if !seen.insert(p.clone()) { continue; } }
        let status = if srv_hash.contains_key(h) || pid.as_ref().map(|p| srv_pids.contains(p)).unwrap_or(false) || (k.is_none() && srv_names.contains(&norm(&parse_filename(fname, None).0))) { "both" }
            else if k.is_none() { "unknown" } else if sv(&pj, "server_side") == "unsupported" { "client" } else if (loader.is_some() || mc.is_some()) && !latest.contains_key(h) { "unavailable" } else { "only" };
        row.push(("status", s(status)));
        let r = obj(row); if status == "only" { missing.push(r.clone()); } mine_rows.push(r);
    }
    let (mut srv_rows, mut need) = (vec![], vec![]);
    for m in &srv_on {
        let h = hash_of.get(&sv(m, "file")).cloned().unwrap_or_default(); let pid = pid_of_srv(m);
        let have = mine.iter().any(|x| x.0 == h) || pid.as_ref().map(|p| my_pids.contains(p)).unwrap_or(false) || name_set(m).iter().any(|x| my_names.contains(x));
        let status = if have { "both" } else if matches!(m.get("server_only"), Some(J::Bool(true))) { "srvonly" } else { need.push(sv(m, "file")); "only" };
        srv_rows.push(obj(vec![("name", s(&sv(m, "name"))), ("version", s(&sv(m, "version"))), ("icon", m.get("icon").cloned().unwrap_or(J::Null)), ("file", s(&sv(m, "file"))), ("status", s(status))]));
    }
    *CMP.lock().unwrap() = need.clone();
    let key = |r: &J| (sv(r, "status") == "both", sv(r, "name").to_lowercase());
    srv_rows.sort_by_key(|r| key(r)); mine_rows.sort_by_key(|r| key(r));
    Resp::json(200, obj(vec![("ok", J::Bool(true)), ("msg", s("Compared.")), ("env", s(&label.unwrap_or_default())), ("mine", J::Arr(mine_rows)), ("server", J::Arr(srv_rows)), ("missing", J::Arr(missing)), ("need_n", n(need.len() as i64))]).to_string())
}

// ---- add a Modrinth modpack to this server
pub fn safe_rel(name: &str) -> Option<Vec<String>> {
    let parts: Vec<String> = name.replace('\\', "/").split('/').filter(|p| !p.is_empty() && *p != ".").map(|p| p.chars().filter(|c| (*c as u32) >= 32).collect()).collect();
    if parts.is_empty() || parts.iter().any(|p| p == "..") { None } else { Some(parts) }
}
pub struct Pack { pub idx: J, pub mc: String, pub loader: String, pub lv: String, pub path: PathBuf, pub title: String }
fn pack_dl(url: &str, dest: &Path, sha512: Option<&str>, sha1: Option<&str>) -> Result<(), String> { dl::download(url, dest, PACK_HOSTS, false, sha1, sha512, 1024) }
pub fn pack_fetch(version_id: &str, d: &Path) -> Result<Pack, String> {
    let v = mr("GET", &format!("/version/{}", enc(version_id)), None)?;
    let files: Vec<J> = av(&v, "files").into_iter().filter(|f| sv(f, "filename").to_lowercase().ends_with(".mrpack")).collect();
    let pf = files.iter().find(|f| matches!(f.get("primary"), Some(J::Bool(true)))).or(files.first()).cloned().ok_or("That modpack version has no .mrpack file.")?;
    let path = d.join("pack.mrpack"); let hs = pf.get("hashes").cloned().unwrap_or(J::Null);
    pack_dl(&sv(&pf, "url"), &path, so(&hs, "sha512").as_deref(), so(&hs, "sha1").as_deref())?;
    let bad = || "That modpack file is damaged or isn't a Modrinth modpack.".to_string();
    let out = sys::run("unzip", &["-p", &path.to_string_lossy(), "modrinth.index.json"], None).filter(|r| r.0 == 0).ok_or_else(bad)?;
    let idx = json::parse(&String::from_utf8_lossy(&out.1)).ok_or_else(bad)?;
    let deps = idx.get("dependencies").cloned().unwrap_or(J::Null);
    let mc = so(&deps, "minecraft");
    let (mut loader, mut lv) = (None, None);
    for (key, ld) in [("neoforge", "neoforge"), ("forge", "forge"), ("fabric-loader", "fabric"), ("quilt-loader", "quilt")] { if let Some(x) = so(&deps, key) { loader = Some(ld.to_string()); lv = Some(x); break; } }
    let (Some(mc), Some(loader)) = (mc, loader) else { return Err("This modpack needs a loader I can't set up (it must be NeoForge, Forge, Fabric or Quilt).".into()) };
    let ok = { let p: Vec<&str> = mc.split('.').collect(); (p.len() == 2 || p.len() == 3) && p.iter().all(|x| !x.is_empty() && x.chars().all(|c| c.is_ascii_digit())) };
    if !ok { return Err(format!("This modpack is for Minecraft {mc}, which I can't set up.")); }
    Ok(Pack { title: so(&idx, "name").or_else(|| so(&v, "name")).unwrap_or_default(), idx, mc, loader, lv: lv.unwrap_or_default(), path })
}
pub fn pack_apply(job: &jobs::Job, d: &Path, pack: &Pack, existing: bool) -> Result<(), String> {
    let allf = av(&pack.idx, "files");
    let files: Vec<J> = allf.iter().filter(|f| f.get("env").map(|e| sv(e, "server")) != Some("unsupported".into())).cloned().collect();
    let root = fs::canonicalize(d).map_err(|e| e.to_string())?;
    let dest_of = |rel: &str| -> Result<PathBuf, String> {
        let parts = safe_rel(rel).ok_or("The modpack has a file with a strange path, stopped.")?;
        let mut p = root.clone(); for x in &parts { p.push(x); }
        if !p.starts_with(&root) { return Err("The modpack tried to write outside the server folder, stopped.".into()); }
        Ok(p)
    };
    let done = Arc::new(Mutex::new(0usize)); let errors = Arc::new(Mutex::new(Vec::<String>::new()));
    let total = files.len();
    let queue = Arc::new(Mutex::new(files.clone()));
    let mut hs = vec![];
    for _ in 0..4 {
        let (queue, done, errors, job) = (queue.clone(), done.clone(), errors.clone(), job.clone());
        let jobs_dest: Vec<(String, PathBuf)> = files.iter().map(|f| (sv(f, "path"), dest_of(&sv(f, "path")).unwrap_or_default())).collect();
        hs.push(thread::spawn(move || loop {
            let f = { let mut q = queue.lock().unwrap(); if q.is_empty() { return; } q.remove(0) };
            let r = (|| -> Result<(), String> {
                let hsx = f.get("hashes").cloned().unwrap_or(J::Null);
                let url = av(&f, "downloads").iter().find_map(|u| u.as_str().map(|x| x.to_string())).ok_or_else(|| format!("No download address for {}.", sv(&f, "path")))?;
                let dest = jobs_dest.iter().find(|x| x.0 == sv(&f, "path")).map(|x| x.1.clone()).filter(|p| !p.as_os_str().is_empty()).ok_or("The modpack has a file with a strange path, stopped.")?;
                if let Some(p) = dest.parent() { let _ = fs::create_dir_all(p); }
                pack_dl(&url, &dest, so(&hsx, "sha512").as_deref(), so(&hsx, "sha1").as_deref())
            })();
            match r { Ok(()) => { let mut dn = done.lock().unwrap(); *dn += 1; jobs::step(&job, &format!("Downloading mods... {} of {}", *dn, total), 72 + (20 * *dn / total.max(1)) as i64); } Err(e) => errors.lock().unwrap().push(e) }
        }));
    }
    for h in hs { let _ = h.join(); }
    { let e = errors.lock().unwrap(); if !e.is_empty() { return Err(format!("{} of the pack's files failed to download. {}", e.len(), e[0])); } }
    jobs::step(job, "Unpacking the pack's config files...", 93);
    let ex = d.join(".pack-unzip"); let _ = fs::remove_dir_all(&ex); fs::create_dir_all(&ex).map_err(|e| e.to_string())?;
    if sys::run("unzip", &["-q", "-o", &pack.path.to_string_lossy(), "overrides/*", "server-overrides/*", "-d", &ex.to_string_lossy()], None).is_none() { return Err("Couldn't unpack the modpack (is unzip installed?).".into()); }
    let mut kept = 0;
    for prefix in ["overrides", "server-overrides"] {
        let base = ex.join(prefix);
        fn walk(dir: &Path, out: &mut Vec<PathBuf>) { if let Ok(rd) = fs::read_dir(dir) { for e in rd.flatten() { let p = e.path(); if p.is_dir() { walk(&p, out); } else { out.push(p); } } } }
        let mut all = vec![]; walk(&base, &mut all);
        for p in all {
            let rel = p.strip_prefix(&base).unwrap().to_string_lossy().to_string();
            let dest = dest_of(&rel)?;
            if existing && (dest.exists() || KEEP.contains(&dest.file_name().map(|x| x.to_string_lossy().to_string()).unwrap_or_default().as_str())) { kept += 1; continue; }
            if let Some(par) = dest.parent() { let _ = fs::create_dir_all(par); }
            fs::copy(&p, &dest).map_err(|e| e.to_string())?;
        }
    }
    let _ = fs::remove_dir_all(&ex); let _ = fs::remove_file(&pack.path);
    let skipped = allf.len() - files.len();
    jobs::log(job, &format!("{} mods installed{}.", files.len(), if skipped > 0 { format!(", {skipped} client-only ones left out") } else { String::new() }));
    if kept > 0 { jobs::log(job, &format!("{kept} of the pack's config files were left alone because you already have them.")); }
    Ok(())
}
static PACKJOB: Mutex<Option<jobs::Job>> = Mutex::new(None);
fn base_name(f: &str) -> String { let t = if f.to_lowercase().ends_with(".jar") { &f[..f.len() - 4] } else { f }; let mut end = t.len(); let b = t.as_bytes(); for i in 0..b.len() { if matches!(b[i], b'-' | b'_' | b'+' | b' ') { let r = &b[i + 1..]; let r = if r.first() == Some(&b'v') { &r[1..] } else { r }; if r.first().map(|c| c.is_ascii_digit()).unwrap_or(false) { end = i; break; } } } t[..end].to_lowercase() }
fn list_jars(app: &App) -> Vec<String> { fs::read_dir(mods_dir(app)).map(|rd| rd.flatten().map(|e| e.file_name().to_string_lossy().to_string()).filter(|f| f.to_lowercase().ends_with(".jar")).collect()).unwrap_or_default() }
fn pack_add_worker(app: Arc<App>, job: jobs::Job, version_id: String, loader: String, mc: String) {
    let r = (|| -> Result<(String, Vec<String>), String> {
        let pack = pack_fetch(&version_id, &app.dir)?;
        if pack.loader != loader || pack.mc != mc {
            let _ = fs::remove_file(&pack.path);
            return Err(format!("This pack is for Minecraft {} on {}, but this server runs Minecraft {mc} on {loader}. A server's Minecraft version and loader can't be swapped, so make a new server from this pack on the homepage (Modpacks button).", pack.mc, pack.loader));
        }
        let before: HashSet<String> = list_jars(&app).into_iter().collect();
        jobs::log(&job, &format!("Modpack: {}", pack.title));
        pack_apply(&job, &app.dir, &pack, true)?;
        crate::misc::mods_touched();
        let mut now: HashMap<String, Vec<String>> = HashMap::new(); for f in list_jars(&app) { now.entry(base_name(&f)).or_default().push(f); }
        let mut warn = vec![];
        for (b, mut fns) in now { if fns.len() > 1 && fns.iter().any(|f| before.contains(f)) && fns.iter().any(|f| !before.contains(f)) { fns.sort(); warn.push(format!("Two copies of {b}: {}. Delete the older one in the Mods tab or the server may not start.", fns.join(" and "))); } }
        warn.sort(); warn.truncate(10);
        Ok((pack.title, warn))
    })();
    match r {
        Ok((title, warn)) => { jobs::set(&job, "pct", n(100)); jobs::set(&job, "step", s("Done")); jobs::set(&job, "warnings", J::Arr(warn.iter().map(|x| s(x)).collect())); jobs::finish(&job, true, &format!("{title} was added. Restart the server to load it.")); }
        Err(e) => jobs::finish(&job, false, &e),
    }
}
fn job_status() -> J {
    let g = PACKJOB.lock().unwrap();
    let Some(j) = g.as_ref() else { return obj(vec![("phase", s("idle")), ("step", s("")), ("pct", n(0)), ("ok", J::Null), ("msg", s("")), ("lines", J::Arr(vec![])), ("warnings", J::Arr(vec![]))]) };
    let v = jobs::get_obj(j).unwrap_or(J::Null);
    let done = matches!(v.get("done"), Some(J::Bool(true)));
    obj(vec![("phase", s(if done { "done" } else { "running" })), ("step", v.get("step").cloned().unwrap_or(J::Null)), ("pct", v.get("pct").cloned().unwrap_or(J::Null)), ("ok", v.get("ok").cloned().unwrap_or(J::Null)), ("msg", v.get("msg").cloned().unwrap_or(J::Null)), ("lines", v.get("lines").cloned().unwrap_or(J::Arr(vec![]))), ("warnings", v.get("warnings").cloned().unwrap_or(J::Arr(vec![])))])
}

pub fn handle(app: &Arc<App>, req: &Req) -> Option<Resp> {
    let body = || json::parse(&String::from_utf8_lossy(&req.body)).unwrap_or(J::Null);
    let jerr = |m: &str| Resp::json(200, obj(vec![("ok", J::Bool(false)), ("error", s(m)), ("msg", s(m))]).to_string());
    Some(match (req.method.as_str(), req.path.as_str()) {
        ("GET", "/api/modpack/build") => match build_pack(app, req.q("format").unwrap_or("zip"), req.q("configs") == Some("1")) { Ok(j) => Resp::json(200, j.to_string()), Err(e) => Resp::json(502, obj(vec![("ok", J::Bool(false)), ("error", s(&e)), ("msg", s(&e))]).to_string()) },
        ("GET", "/api/modpack/file") => {
            let id = req.q("id").unwrap_or("").to_string();
            let ent = { let mut g = PACKS.lock().unwrap(); g.iter().position(|p| p.0 == id).map(|i| g.remove(i)) };
            match ent.and_then(|e| fs::read(&e.1).ok().map(|b| { let _ = fs::remove_file(&e.1); (b, e.3) })) { Some((b, ct)) => Resp { code: 200, ctype: ct, body: b }, None => Resp::json(404, obj(vec![("ok", J::Bool(false)), ("error", s("That download expired. Build it again.")), ("msg", s("That download expired. Build it again."))]).to_string()) }
        }
        ("GET", "/api/modpack.zip") => {
            if !mods_dir(app).is_dir() { return Some(Resp::json(404, obj(vec![("ok", J::Bool(false)), ("error", s("There's no mods folder yet."))]).to_string())); }
            match build_pack(app, "zip", req.q("configs") == Some("1")).ok().and_then(|j| { let id = sv(&j, "id"); let e = PACKS.lock().unwrap().iter().position(|p| p.0 == id).map(|i| PACKS.lock().unwrap().remove(i))?; fs::read(&e.1).ok() }) { Some(b) => Resp { code: 200, ctype: "application/zip", body: b }, None => Resp::json(500, obj(vec![("ok", J::Bool(false)), ("error", s("Couldn't build the modpack."))]).to_string()) }
        }
        ("POST", "/api/mods/compare") => compare(app, &body()),
        ("GET", "/api/mods/compare/zip") => {
            let files: Vec<String> = CMP.lock().unwrap().iter().filter(|f| mods_dir(app).join(f).is_file()).cloned().collect();
            if files.is_empty() { return Some(jerr("Nothing to download. Run the comparison again.")); }
            let lay = staging(app).join(format!("cmp-{}", jobs::new_id())); let _ = fs::create_dir_all(&lay);
            for f in &files { let _ = link_or_copy(&mods_dir(app).join(f), &lay.join(dest_name(f))); }
            let out = staging(app).join(format!("cmp-{}.zip", jobs::new_id()));
            let r = zip_dir(&lay, &out, true).ok().and_then(|_| fs::read(&out).ok());
            let _ = fs::remove_dir_all(&lay); let _ = fs::remove_file(&out);
            match r { Some(b) => Resp { code: 200, ctype: "application/zip", body: b }, None => jerr("Couldn't build the zip.") }
        }
        ("GET", "/api/packs/search") => {
            let g = |k: &str, d: &str| req.q(k).filter(|x| !x.is_empty()).unwrap_or(d).to_string();
            let sort = g("sort", "relevance"); let sort = if ["relevance", "downloads", "updated", "newest", "follows"].contains(&sort.as_str()) { sort } else { "relevance".into() };
            let facets = J::Arr(vec![J::Arr(vec![s("project_type:modpack")]), J::Arr(vec![s("server_side:required"), s("server_side:optional")])]).to_string();
            let q = qs(&[("query", g("q", "").chars().take(100).collect()), ("index", sort), ("offset", g("offset", "0").parse::<i64>().unwrap_or(0).max(0).to_string()), ("limit", "20".into()), ("facets", facets)]);
            match mr("GET", &format!("/search?{q}"), None) {
                Err(e) => jerr(&e),
                Ok(d) => {
                    let res: Vec<J> = av(&d, "hits").iter().map(|x| { let vs = sa(x, "versions"); obj(vec![("id", x.get("project_id").cloned().unwrap_or(J::Null)), ("slug", x.get("slug").cloned().unwrap_or(J::Null)), ("name", x.get("title").cloned().unwrap_or(J::Null)), ("summary", x.get("description").cloned().unwrap_or(J::Null)), ("icon", x.get("icon_url").cloned().unwrap_or(J::Null)), ("author", x.get("author").cloned().unwrap_or(J::Null)), ("downloads", x.get("downloads").cloned().unwrap_or(J::Null)), ("mc", vs.last().map(|v| s(v)).unwrap_or(J::Null)), ("loaders", J::Arr(sa(x, "categories").iter().filter(|c| ["forge", "fabric", "quilt", "neoforge"].contains(&c.as_str())).map(|c| s(c)).collect())), ("url", s(&format!("https://modrinth.com/modpack/{}", sv(x, "slug"))))]) }).collect();
                    Resp::json(200, obj(vec![("ok", J::Bool(true)), ("results", J::Arr(res)), ("total", n(iv(&d, "total_hits")))]).to_string())
                }
            }
        }
        ("GET", "/api/packs/versions") => {
            let id = req.q("id").unwrap_or("");
            if id.len() < 6 || id.len() > 12 || !id.chars().all(|c| c.is_ascii_alphanumeric()) { return Some(jerr("Bad pack id.")); }
            match mr("GET", &format!("/project/{}/version", enc(id)), None) {
                Err(e) => jerr(&e),
                Ok(J::Arr(vs)) => {
                    let out: Vec<J> = vs.iter().take(40).filter(|v| av(v, "files").iter().any(|f| sv(f, "filename").to_lowercase().ends_with(".mrpack"))).map(|v| { let ld: Vec<J> = sa(v, "loaders").iter().filter(|x| ["forge", "fabric", "quilt", "neoforge"].contains(&x.as_str())).map(|x| s(x)).collect(); let gv = sa(v, "game_versions"); let tail: Vec<J> = gv[gv.len().saturating_sub(3)..].iter().map(|x| s(x)).collect();
                        obj(vec![("id", v.get("id").cloned().unwrap_or(J::Null)), ("name", v.get("name").cloned().unwrap_or(J::Null)), ("version", v.get("version_number").cloned().unwrap_or(J::Null)), ("type", s(&{ let t = sv(v, "version_type"); if t.is_empty() { "release".into() } else { t } })), ("mc", J::Arr(tail)), ("supported", J::Bool(!ld.is_empty())), ("loaders", J::Arr(ld)), ("date", v.get("date_published").cloned().unwrap_or(J::Null)), ("downloads", v.get("downloads").cloned().unwrap_or(J::Null))]) }).collect();
                    let (l, m, lb, _) = envinfo::env_cached(&app.dir);
                    Resp::json(200, obj(vec![("ok", J::Bool(true)), ("versions", J::Arr(out)), ("env", obj(vec![("loader", jo(l)), ("mc", jo(m)), ("label", jo(lb))]))]).to_string())
                }
                Ok(_) => jerr("Unexpected reply from Modrinth."),
            }
        }
        ("GET", "/api/packs/status") => Resp::json(200, job_status().to_string()),
        ("POST", "/api/packs/apply") => {
            let vid = sv(&body(), "version");
            if vid.len() < 6 || vid.len() > 12 || !vid.chars().all(|c| c.is_ascii_alphanumeric()) { return Some(reply(false, "Pick a pack version first.")); }
            let (Some(l), Some(m)) = (envinfo::env_cached(&app.dir).0, envinfo::env_cached(&app.dir).1) else { return Some(reply(false, "I can't tell what this server runs yet. Start it once, or install its loader first.")) };
            let mut g = PACKJOB.lock().unwrap();
            if let Some(j) = g.as_ref() { if !matches!(jobs::get_obj(j).and_then(|v| v.get("done").cloned()), Some(J::Bool(true))) { return Some(reply(false, "A modpack is already being added.")); } }
            let (_, job) = jobs::create("modpack", false); jobs::step(&job, "Starting...", 2); *g = Some(job.clone());
            let a = app.clone(); thread::spawn(move || pack_add_worker(a, job, vid, l, m));
            reply(true, "Adding the modpack...")
        }
        _ => return None,
    })
}
