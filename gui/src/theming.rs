//! Custom colour theme: any accent colour (hex or sliders) plus a surface style. The look is derived from
//! the accent, so every screen follows it. Saved on the engine so every computer shows the same theme.
use crate::*;

type Rgb = (u8, u8, u8);

fn col(c: Rgb) -> Color { Color::from_rgb_u8(c.0, c.1, c.2) }

pub fn parse_hex(t: &str) -> Option<Rgb> {
    let t = t.trim().trim_start_matches('#');
    let t: String = if t.len() == 3 { t.chars().flat_map(|c| [c, c]).collect() } else { t.to_string() };
    if t.len() != 6 || !t.chars().all(|c| c.is_ascii_hexdigit()) { return None; }
    let v = u32::from_str_radix(&t, 16).ok()?;
    Some(((v >> 16) as u8, (v >> 8) as u8, v as u8))
}

fn cap(n: &str) -> String { let mut c = n.chars(); match c.next() { Some(f) => f.to_uppercase().collect::<String>() + c.as_str(), None => String::new() } }

fn hex_of(c: Rgb) -> String { format!("#{:02X}{:02X}{:02X}", c.0, c.1, c.2) }

pub fn hsv_to_rgb(h: f32, s: f32, v: f32) -> Rgb {
    let h = (h.clamp(0.0, 1.0) * 6.0) % 6.0; let i = h.floor(); let f = h - i;
    let (p, q, t) = (v * (1.0 - s), v * (1.0 - s * f), v * (1.0 - s * (1.0 - f)));
    let (r, g, b) = match i as i32 { 0 => (v, t, p), 1 => (q, v, p), 2 => (p, v, t), 3 => (p, q, v), 4 => (t, p, v), _ => (v, p, q) };
    ((r * 255.0).round() as u8, (g * 255.0).round() as u8, (b * 255.0).round() as u8)
}

pub fn rgb_to_hsv(c: Rgb) -> (f32, f32, f32) {
    let (r, g, b) = (c.0 as f32 / 255.0, c.1 as f32 / 255.0, c.2 as f32 / 255.0);
    let mx = r.max(g).max(b); let mn = r.min(g).min(b); let d = mx - mn;
    let h = if d == 0.0 { 0.0 } else if mx == r { ((g - b) / d).rem_euclid(6.0) } else if mx == g { (b - r) / d + 2.0 } else { (r - g) / d + 4.0 } / 6.0;
    (h, if mx == 0.0 { 0.0 } else { d / mx }, mx)
}

fn mix(a: Rgb, b: Rgb, t: f32) -> Rgb { let m = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8; (m(a.0, b.0), m(a.1, b.1), m(a.2, b.2)) }

fn luminance(c: Rgb) -> f32 { let f = |v: u8| { let x = v as f32 / 255.0; if x <= 0.03928 { x / 12.92 } else { ((x + 0.055) / 1.055).powf(2.4) } }; 0.2126 * f(c.0) + 0.7152 * f(c.1) + 0.0722 * f(c.2) }

type Six = (Rgb, Rgb, Rgb, Rgb, Rgb, Rgb);

/// window, sidebar, card, raised, hairline, stronger hairline for a surface style and accent
fn surface_colors(surface: &str, accent: Rgb) -> Six {
    match surface {
        "black" => ((0x00, 0x00, 0x00), (0x00, 0x00, 0x00), (0x0c, 0x0c, 0x0e), (0x16, 0x16, 0x18), (0x22, 0x22, 0x26), (0x32, 0x32, 0x38)),
        "graphite" => ((0x0d, 0x0f, 0x12), (0x0a, 0x0c, 0x0f), (0x14, 0x17, 0x1c), (0x1b, 0x1f, 0x26), (0x23, 0x27, 0x2d), (0x34, 0x3a, 0x43)),
        _ => (mix((0x0c, 0x0f, 0x14), accent, 0.04), mix((0x09, 0x0b, 0x0f), accent, 0.04), mix((0x12, 0x16, 0x1c), accent, 0.06), mix((0x19, 0x1e, 0x26), accent, 0.08), mix((0x22, 0x27, 0x2f), accent, 0.12), mix((0x33, 0x3a, 0x45), accent, 0.14)),
    }
}

/// Colours of the window for an accent and a surface style.
pub fn apply(a: &App, accent: Rgb, surface: &str, sync_ctl: bool) {
    let th = a.global::<Theme>();
    th.set_accent(col(accent));
    th.set_on_accent(if luminance(accent) > 0.19 { Color::from_rgb_u8(0x08, 0x12, 0x1b) } else { Color::from_rgb_u8(0xff, 0xff, 0xff) });
    let (bg, rail, panel, panel2, line, line2) = surface_colors(surface, accent);
    th.set_bg(col(bg)); th.set_rail(col(rail)); th.set_panel(col(panel)); th.set_panel2(col(panel2)); th.set_line(col(line)); th.set_line2(col(line2));
    let ctl = a.global::<ThemeCtl>();
    ctl.set_surface(SharedString::from(surface));
    let (h, s, v) = if sync_ctl { rgb_to_hsv(accent) } else { (ctl.get_hue(), ctl.get_sat(), ctl.get_val()) };
    if sync_ctl { ctl.set_hue(h); ctl.set_sat(s); ctl.set_val(v); }
    ctl.set_hex(SharedString::from(hex_of(accent)));
    // the three surface preview cards follow the accent too
    let mut sv: Vec<Color> = vec![];
    for name in ["graphite", "tinted", "black"] { let c = surface_colors(name, accent); sv.extend([col(c.0), col(c.1), col(c.2), col(c.4)]); }
    ctl.set_surf(ModelRc::new(VecModel::from(sv)));
    ctl.set_sat_a(col(hsv_to_rgb(h, 0.0, v))); ctl.set_sat_b(col(hsv_to_rgb(h, 1.0, v))); ctl.set_val_b(col(hsv_to_rgb(h, s, 1.0)));
}

pub fn apply_saved(a: &App, theme: &J) {
    let acc = parse_hex(&gs(theme, "accent")).or_else(|| { let n = gs(theme, "palette"); PALETTES.iter().find(|p| p.0 == n).map(|p| ((p.1 >> 16) as u8, (p.1 >> 8) as u8, p.1 as u8)) }).unwrap_or((0x7c, 0xc4, 0xff));
    let sf = gs(theme, "surface"); apply(a, acc, if sf.is_empty() { "tinted" } else { &sf }, true);
}

fn save(st: &Shared, a: &App) {
    let ctl = a.global::<ThemeCtl>(); let (hex, sf) = (ctl.get_hex().to_string(), ctl.get_surface().to_string());
    let st = st.clone();
    bg(move || { if let Some(c) = client(&st) { let _ = c.post("/api/theme", &obj(vec![("accent", s(&hex)), ("surface", s(&sf)), ("palette", s("custom"))])); } });
}

pub fn wire(app: &App, ui: &Weak<App>, st: &Shared) {
    let ctl = app.global::<ThemeCtl>();
    ctl.set_quick(ModelRc::new(VecModel::from(PALETTES.iter().map(|p| Color::from_rgb_u8((p.1 >> 16) as u8, (p.1 >> 8) as u8, p.1 as u8)).collect::<Vec<_>>())));
    ctl.set_quick_names(ModelRc::new(VecModel::from(PALETTES.iter().map(|p| SharedString::from(cap(p.0))).collect::<Vec<_>>())));
    { let u = ui.clone(); ctl.on_changed(move || { let Some(a) = u.upgrade() else { return }; let c = a.global::<ThemeCtl>(); apply(&a, hsv_to_rgb(c.get_hue(), c.get_sat(), c.get_val()), &c.get_surface(), false); }); }
    { let (u, s_) = (ui.clone(), st.clone()); ctl.on_committed(move || { if let Some(a) = u.upgrade() { save(&s_, &a); } }); }
    { let (u, s_) = (ui.clone(), st.clone()); ctl.on_hex_entered(move |t| {
        let Some(a) = u.upgrade() else { return };
        match parse_hex(&t) { Some(c) => { let sf = a.global::<ThemeCtl>().get_surface(); apply(&a, c, &sf, true); save(&s_, &a); } None => { a.set_note(SharedString::from("That isn't a colour. Use something like #7CC4FF.")); } }
    }); }
    { let (u, s_) = (ui.clone(), st.clone()); ctl.on_pick_quick(move |i| {
        let Some(a) = u.upgrade() else { return }; let Some(p) = PALETTES.get(i.max(0) as usize) else { return };
        let sf = a.global::<ThemeCtl>().get_surface(); apply(&a, ((p.1 >> 16) as u8, (p.1 >> 8) as u8, p.1 as u8), &sf, true); save(&s_, &a);
    }); }
    { let (u, s_) = (ui.clone(), st.clone()); ctl.on_pick_surface(move |sf| {
        let Some(a) = u.upgrade() else { return }; let c = a.global::<ThemeCtl>();
        apply(&a, hsv_to_rgb(c.get_hue(), c.get_sat(), c.get_val()), &sf, false); save(&s_, &a);
    }); }
}
