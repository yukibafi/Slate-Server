//! One theme for everything. Saved in a single file on this computer, so the hub and every server
//! panel (each on its own port, which browsers treat as separate sites) always show the same look.
use crate::json::{self, obj, s, J};
use std::path::PathBuf;

const PALETTES: [&str; 21] = ["ice", "amber", "rose", "slate", "stone", "violet", "teal", "ember", "forest", "lime", "orange", "crimson", "pink", "indigo", "ocean", "cyan", "mint", "gold", "lavender", "sand", "custom"];
const SURFACES: [&str; 3] = ["graphite", "tinted", "black"];
const FONTS: [&str; 11] = ["inter", "manrope", "dmsans", "jakarta", "poppins", "nunito", "space", "jbmono", "sys", "serif", "mono"];

pub fn data_dir() -> PathBuf {
    if let Ok(d) = std::env::var("PANEL_DATA") { return PathBuf::from(d); }
    let home = std::env::var("HOME").or_else(|_| std::env::var("USERPROFILE")).unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join(".create-panel-rs")
}

fn file() -> PathBuf { data_dir().join("theme.json") }

fn valid_hex(v: &str) -> bool { v.len() == 7 && v.starts_with('#') && v[1..].chars().all(|c| c.is_ascii_hexdigit()) }

pub fn get() -> J {
    let txt = std::fs::read_to_string(file()).unwrap_or_default();
    let j = json::parse(&txt);
    let pick = |k: &str, ok: &[&str], d: &str| j.as_ref().and_then(|j| j.get(k)).and_then(|v| v.as_str()).filter(|v| ok.contains(v)).unwrap_or(d).to_string();
    let accent = j.as_ref().and_then(|j| j.get("accent")).and_then(|v| v.as_str()).filter(|v| valid_hex(v)).unwrap_or("").to_string();
    obj(vec![("palette", s(&pick("palette", &PALETTES, "ice"))), ("font", s(&pick("font", &FONTS, "inter"))), ("accent", s(&accent)), ("surface", s(&pick("surface", &SURFACES, "tinted")))])
}

/// Applies a partial update ({"palette":..}, {"font":..}, {"accent":"#rrggbb"}, {"surface":..}); unknown values are ignored.
pub fn set(body: &str) -> J {
    let cur = get();
    let inb = json::parse(body);
    let pick = |k: &str, ok: &[&str]| inb.as_ref().and_then(|j| j.get(k)).and_then(|v| v.as_str()).filter(|v| ok.contains(v)).map(|v| v.to_string())
        .or_else(|| cur.get(k).and_then(|v| v.as_str()).map(|v| v.to_string())).unwrap_or_default();
    let accent = inb.as_ref().and_then(|j| j.get("accent")).and_then(|v| v.as_str()).filter(|v| valid_hex(v)).map(|v| v.to_string())
        .or_else(|| cur.get("accent").and_then(|v| v.as_str()).map(|v| v.to_string())).unwrap_or_default();
    let out = obj(vec![("palette", s(&pick("palette", &PALETTES))), ("font", s(&pick("font", &FONTS))), ("accent", s(&accent)), ("surface", s(&pick("surface", &SURFACES)))]);
    let _ = std::fs::create_dir_all(data_dir());
    let tmp = data_dir().join("theme.json.tmp");
    if std::fs::write(&tmp, out.to_string()).is_ok() { let _ = std::fs::rename(&tmp, file()); }
    out
}
