//! Tiny JSON writer + parser (std only). Phase 1 only needs flat objects from the UI
//! like {"cmd":"list"}; phase 2 swaps this for serde_json.
use std::collections::BTreeMap;
use std::fmt::Write;

#[derive(Clone, Debug)]
pub enum J {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<J>),
    Obj(Vec<(String, J)>),
}

pub fn s(v: &str) -> J { J::Str(v.to_string()) }
pub fn n(v: i64) -> J { J::Num(v as f64) }
pub fn obj(v: Vec<(&str, J)>) -> J { J::Obj(v.into_iter().map(|(k, v)| (k.to_string(), v)).collect()) }

impl J {
    pub fn to_string(&self) -> String {
        let mut o = String::new();
        self.write(&mut o);
        o
    }
    fn write(&self, o: &mut String) {
        match self {
            J::Null => o.push_str("null"),
            J::Bool(b) => o.push_str(if *b { "true" } else { "false" }),
            J::Num(x) => {
                if x.fract() == 0.0 && x.abs() < 1e15 { let _ = write!(o, "{}", *x as i64); } else { let _ = write!(o, "{}", x); }
            }
            J::Str(t) => esc(t, o),
            J::Arr(a) => {
                o.push('[');
                for (i, v) in a.iter().enumerate() { if i > 0 { o.push(','); } v.write(o); }
                o.push(']');
            }
            J::Obj(m) => {
                o.push('{');
                for (i, (k, v)) in m.iter().enumerate() { if i > 0 { o.push(','); } esc(k, o); o.push(':'); v.write(o); }
                o.push('}');
            }
        }
    }
}

fn esc(t: &str, o: &mut String) {
    o.push('"');
    for c in t.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\t' => o.push_str("\\t"),
            c if (c as u32) < 0x20 => { let _ = write!(o, "\\u{:04x}", c as u32); }
            c => o.push(c),
        }
    }
    o.push('"');
}

/// Parses a JSON value. Returns None on malformed input.
pub fn parse(src: &str) -> Option<J> {
    let b: Vec<char> = src.chars().collect();
    let mut i = 0;
    let v = val(&b, &mut i)?;
    ws(&b, &mut i);
    if i == b.len() { Some(v) } else { None }
}

fn ws(b: &[char], i: &mut usize) { while *i < b.len() && b[*i].is_whitespace() { *i += 1; } }

fn val(b: &[char], i: &mut usize) -> Option<J> {
    ws(b, i);
    match *b.get(*i)? {
        '{' => {
            *i += 1;
            let mut m = Vec::new();
            ws(b, i);
            if b.get(*i) == Some(&'}') { *i += 1; return Some(J::Obj(m)); }
            loop {
                ws(b, i);
                let k = match val(b, i)? { J::Str(k) => k, _ => return None };
                ws(b, i);
                if b.get(*i) != Some(&':') { return None; }
                *i += 1;
                let v = val(b, i)?;
                m.push((k, v));
                ws(b, i);
                match b.get(*i)? { ',' => *i += 1, '}' => { *i += 1; return Some(J::Obj(m)); } _ => return None }
            }
        }
        '[' => {
            *i += 1;
            let mut a = Vec::new();
            ws(b, i);
            if b.get(*i) == Some(&']') { *i += 1; return Some(J::Arr(a)); }
            loop {
                a.push(val(b, i)?);
                ws(b, i);
                match b.get(*i)? { ',' => *i += 1, ']' => { *i += 1; return Some(J::Arr(a)); } _ => return None }
            }
        }
        '"' => {
            *i += 1;
            let mut o = String::new();
            loop {
                let c = *b.get(*i)?; *i += 1;
                match c {
                    '"' => return Some(J::Str(o)),
                    '\\' => {
                        let e = *b.get(*i)?; *i += 1;
                        match e {
                            'n' => o.push('\n'), 'r' => o.push('\r'), 't' => o.push('\t'), 'b' => o.push('\u{8}'), 'f' => o.push('\u{c}'),
                            'u' => {
                                let h: String = b.get(*i..*i + 4)?.iter().collect(); *i += 4;
                                o.push(char::from_u32(u32::from_str_radix(&h, 16).ok()?).unwrap_or('\u{fffd}'));
                            }
                            c => o.push(c),
                        }
                    }
                    c => o.push(c),
                }
            }
        }
        't' if b[*i..].starts_with(&['t', 'r', 'u', 'e']) => { *i += 4; Some(J::Bool(true)) }
        'f' if b[*i..].starts_with(&['f', 'a', 'l', 's', 'e']) => { *i += 5; Some(J::Bool(false)) }
        'n' if b[*i..].starts_with(&['n', 'u', 'l', 'l']) => { *i += 4; Some(J::Null) }
        _ => {
            let st = *i;
            while *i < b.len() && (b[*i].is_ascii_digit() || "+-.eE".contains(b[*i])) { *i += 1; }
            let t: String = b[st..*i].iter().collect();
            t.parse::<f64>().ok().map(J::Num)
        }
    }
}

impl J {
    pub fn get(&self, k: &str) -> Option<&J> {
        if let J::Obj(m) = self { m.iter().find(|(a, _)| a == k).map(|(_, v)| v) } else { None }
    }
    pub fn as_str(&self) -> Option<&str> { if let J::Str(t) = self { Some(t) } else { None } }
}

#[allow(dead_code)]
pub fn map(j: &J) -> BTreeMap<String, J> {
    if let J::Obj(m) = j { m.iter().cloned().collect() } else { BTreeMap::new() }
}
