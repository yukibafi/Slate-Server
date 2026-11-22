//! A small backtracking regex engine (std only) covering the Python `re` features the panel uses:
//! literals, . classes \d \w \s \b, groups (capturing, (?:), named (?P<n>)), alternation, * + ? {n,m} (greedy and lazy),
//! ^ $, lookahead (?= (?!, lookbehind (?<= (?<!, and a leading (?i). Operates on chars; offsets are char indexes.
use std::collections::HashMap;

#[derive(Debug, Clone)]
enum N {
    Char(char), Any, Set(Vec<Item>, bool), Start, End, WordB(bool),
    Group(Box<N>, Option<usize>), Cat(Vec<N>), Alt(Vec<N>),
    Rep(Box<N>, usize, usize, bool), Look { ahead: bool, neg: bool, n: Box<N> },
}
#[derive(Debug, Clone)]
enum Item { Range(char, char), Digit(bool), Word(bool), Space(bool) }

#[derive(Debug, Clone)]
pub struct Regex { root: N, ngroups: usize, names: HashMap<String, usize>, icase: bool, multiline: bool }
pub type Caps = Vec<Option<(usize, usize)>>;
#[derive(Debug, Clone)]
pub struct Match { pub caps: Caps, chars: Vec<char> }
impl Match {
    pub fn start(&self) -> usize { self.caps[0].unwrap().0 }
    pub fn end(&self) -> usize { self.caps[0].unwrap().1 }
    pub fn get(&self, i: usize) -> Option<String> { self.caps.get(i).and_then(|c| *c).map(|(a, b)| self.chars[a..b].iter().collect()) }
    pub fn whole(&self) -> String { self.get(0).unwrap_or_default() }
}

struct P<'a> { c: &'a [char], i: usize, ng: usize, names: HashMap<String, usize> }
impl<'a> P<'a> {
    fn peek(&self) -> Option<char> { self.c.get(self.i).copied() }
    fn eat(&mut self, ch: char) -> bool { if self.peek() == Some(ch) { self.i += 1; true } else { false } }
    fn alt(&mut self) -> Result<N, String> {
        let mut alts = vec![self.cat()?];
        while self.eat('|') { alts.push(self.cat()?); }
        Ok(if alts.len() == 1 { alts.pop().unwrap() } else { N::Alt(alts) })
    }
    fn cat(&mut self) -> Result<N, String> {
        let mut v = vec![];
        while let Some(ch) = self.peek() { if ch == '|' || ch == ')' { break; } v.push(self.rep()?); }
        Ok(N::Cat(v))
    }
    fn rep(&mut self) -> Result<N, String> {
        let a = self.atom()?;
        let (min, max) = match self.peek() {
            Some('*') => { self.i += 1; (0, usize::MAX) }
            Some('+') => { self.i += 1; (1, usize::MAX) }
            Some('?') => { self.i += 1; (0, 1) }
            Some('{') => {
                let save = self.i; self.i += 1;
                let mut a_ = String::new(); while let Some(d) = self.peek() { if d.is_ascii_digit() { a_.push(d); self.i += 1; } else { break; } }
                let mut b_ = None;
                if self.eat(',') { let mut t = String::new(); while let Some(d) = self.peek() { if d.is_ascii_digit() { t.push(d); self.i += 1; } else { break; } } b_ = Some(t); }
                if !self.eat('}') || a_.is_empty() { self.i = save; return Ok(a); }
                let lo: usize = a_.parse().unwrap_or(0);
                let hi = match b_ { None => lo, Some(t) if t.is_empty() => usize::MAX, Some(t) => t.parse().unwrap_or(usize::MAX) };
                (lo, hi)
            }
            _ => return Ok(a),
        };
        let greedy = !self.eat('?');
        Ok(N::Rep(Box::new(a), min, max, greedy))
    }
    fn esc_item(&mut self, c: char) -> Result<Option<Item>, String> {
        Ok(match c { 'd' => Some(Item::Digit(true)), 'D' => Some(Item::Digit(false)), 'w' => Some(Item::Word(true)), 'W' => Some(Item::Word(false)), 's' => Some(Item::Space(true)), 'S' => Some(Item::Space(false)), _ => None })
    }
    fn esc_char(c: char) -> char { match c { 'n' => '\n', 't' => '\t', 'r' => '\r', x => x } }
    fn atom(&mut self) -> Result<N, String> {
        let ch = self.peek().ok_or("unexpected end")?;
        self.i += 1;
        match ch {
            '.' => Ok(N::Any), '^' => Ok(N::Start), '$' => Ok(N::End),
            '(' => {
                if self.eat('?') {
                    if self.eat(':') { let n = self.alt()?; self.eat(')'); return Ok(N::Group(Box::new(n), None)); }
                    if self.eat('=') { let n = self.alt()?; self.eat(')'); return Ok(N::Look { ahead: true, neg: false, n: Box::new(n) }); }
                    if self.eat('!') { let n = self.alt()?; self.eat(')'); return Ok(N::Look { ahead: true, neg: true, n: Box::new(n) }); }
                    if self.eat('<') {
                        if self.eat('=') { let n = self.alt()?; self.eat(')'); return Ok(N::Look { ahead: false, neg: false, n: Box::new(n) }); }
                        if self.eat('!') { let n = self.alt()?; self.eat(')'); return Ok(N::Look { ahead: false, neg: true, n: Box::new(n) }); }
                        return Err("bad (?<".into());
                    }
                    if self.eat('P') && self.eat('<') {
                        let mut name = String::new(); while let Some(c) = self.peek() { self.i += 1; if c == '>' { break; } name.push(c); }
                        self.ng += 1; let idx = self.ng; self.names.insert(name, idx);
                        let n = self.alt()?; self.eat(')'); return Ok(N::Group(Box::new(n), Some(idx)));
                    }
                    return Err("unsupported group".into());
                }
                self.ng += 1; let idx = self.ng;
                let n = self.alt()?; if !self.eat(')') { return Err("missing )".into()); }
                Ok(N::Group(Box::new(n), Some(idx)))
            }
            '[' => {
                let neg = self.eat('^'); let mut items = vec![]; let mut first = true;
                loop {
                    let c = self.peek().ok_or("unterminated [")?; self.i += 1;
                    if c == ']' && !first { break; }
                    first = false;
                    let lo = if c == '\\' { let e = self.peek().ok_or("bad escape")?; self.i += 1; if let Some(it) = self.esc_item(e)? { items.push(it); continue; } Self::esc_char(e) } else { c };
                    if self.peek() == Some('-') && self.c.get(self.i + 1).map(|x| *x != ']').unwrap_or(false) {
                        self.i += 1; let h = self.peek().unwrap(); self.i += 1;
                        let hi = if h == '\\' { let e = self.peek().ok_or("bad escape")?; self.i += 1; Self::esc_char(e) } else { h };
                        items.push(Item::Range(lo, hi));
                    } else { items.push(Item::Range(lo, lo)); }
                }
                Ok(N::Set(items, neg))
            }
            '\\' => {
                let e = self.peek().ok_or("bad escape")?; self.i += 1;
                if let Some(it) = self.esc_item(e)? { return Ok(N::Set(vec![it], false)); }
                if e == 'b' { return Ok(N::WordB(true)); } if e == 'B' { return Ok(N::WordB(false)); }
                Ok(N::Char(Self::esc_char(e)))
            }
            c => Ok(N::Char(c)),
        }
    }
}

fn is_word(c: char) -> bool { c.is_alphanumeric() || c == '_' }
fn lc(c: char) -> char { c.to_lowercase().next().unwrap_or(c) }

struct M<'a> { c: &'a [char], icase: bool, multi: bool }
impl<'a> M<'a> {
    fn item(&self, it: &Item, ch: char) -> bool {
        match it {
            Item::Range(a, b) => (*a <= ch && ch <= *b) || (self.icase && { let l = lc(ch); let u = ch.to_uppercase().next().unwrap_or(ch); (*a <= l && l <= *b) || (*a <= u && u <= *b) }),
            Item::Digit(p) => ch.is_ascii_digit() == *p, Item::Word(p) => is_word(ch) == *p, Item::Space(p) => ch.is_whitespace() == *p,
        }
    }
    fn m(&self, n: &N, i: usize, caps: &mut Caps, k: &mut dyn FnMut(usize, &mut Caps) -> bool) -> bool {
        match n {
            N::Char(c) => { if i < self.c.len() && (self.c[i] == *c || (self.icase && lc(self.c[i]) == lc(*c))) { k(i + 1, caps) } else { false } }
            N::Any => { if i < self.c.len() && self.c[i] != '\n' { k(i + 1, caps) } else { false } }
            N::Set(items, neg) => { if i < self.c.len() && (items.iter().any(|it| self.item(it, self.c[i])) != *neg) { k(i + 1, caps) } else { false } }
            N::Start => { if i == 0 || (self.multi && self.c[i - 1] == '\n') { k(i, caps) } else { false } }
            N::End => { if i == self.c.len() || (i + 1 == self.c.len() && self.c[i] == '\n') || (self.multi && self.c[i] == '\n') { k(i, caps) } else { false } }
            N::WordB(want) => {
                let a = i > 0 && is_word(self.c[i - 1]); let b = i < self.c.len() && is_word(self.c[i]);
                if (a != b) == *want { k(i, caps) } else { false }
            }
            N::Group(inner, idx) => {
                match idx {
                    None => self.m(inner, i, caps, k),
                    Some(g) => {
                        let g = *g; let old = caps[g];
                        let r = self.m(inner, i, caps, &mut |e, caps2: &mut Caps| { let prev = caps2[g]; caps2[g] = Some((i, e)); if k(e, caps2) { true } else { caps2[g] = prev; false } });
                        if !r { caps[g] = old; }
                        r
                    }
                }
            }
            N::Cat(v) => self.cat(v, 0, i, caps, k),
            N::Alt(v) => { for a in v { if self.m(a, i, caps, k) { return true; } } false }
            N::Rep(inner, min, max, greedy) => self.rep(inner, *min, *max, *greedy, 0, i, caps, k),
            N::Look { ahead, neg, n } => {
                let found = if *ahead { self.m(n, i, &mut caps.clone(), &mut |_, _| true) } else {
                    (0..=i).rev().any(|s| self.m(n, s, &mut caps.clone(), &mut |e, _| e == i))
                };
                if found != *neg { k(i, caps) } else { false }
            }
        }
    }
    fn cat(&self, v: &[N], idx: usize, i: usize, caps: &mut Caps, k: &mut dyn FnMut(usize, &mut Caps) -> bool) -> bool {
        if idx == v.len() { return k(i, caps); }
        self.m(&v[idx], i, caps, &mut |e, caps2: &mut Caps| self.cat(v, idx + 1, e, caps2, k))
    }
    fn rep(&self, inner: &N, min: usize, max: usize, greedy: bool, count: usize, i: usize, caps: &mut Caps, k: &mut dyn FnMut(usize, &mut Caps) -> bool) -> bool {
        let more = |this: &Self, caps: &mut Caps, k: &mut dyn FnMut(usize, &mut Caps) -> bool| -> bool {
            if count >= max { return false; }
            this.m(inner, i, caps, &mut |e, caps2: &mut Caps| { if e == i && count >= min { return false; } this.rep(inner, min, max, greedy, count + 1, e, caps2, k) })
        };
        if count < min { return more(self, caps, k); }
        if greedy { if more(self, caps, k) { return true; } k(i, caps) } else { if k(i, caps) { return true; } more(self, caps, k) }
    }
}

impl Regex {
    pub fn new(pat: &str) -> Regex { Self::try_new(pat).unwrap_or_else(|e| panic!("bad regex {:?}: {}", pat, e)) }
    pub fn try_new(pat: &str) -> Result<Regex, String> {
        let (mut icase, mut multiline) = (false, false);
        let mut p = pat;
        loop {
            if let Some(r) = p.strip_prefix("(?i)") { icase = true; p = r; } else if let Some(r) = p.strip_prefix("(?m)") { multiline = true; p = r; } else { break; }
        }
        let chars: Vec<char> = p.chars().collect();
        let mut ps = P { c: &chars, i: 0, ng: 0, names: HashMap::new() };
        let root = ps.alt()?;
        if ps.i != chars.len() { return Err(format!("unexpected {:?}", chars[ps.i])); }
        Ok(Regex { root, ngroups: ps.ng, names: ps.names, icase, multiline })
    }
    fn at(&self, chars: &[char], start: usize) -> Option<Caps> {
        let m = M { c: chars, icase: self.icase, multi: self.multiline };
        let mut caps: Caps = vec![None; self.ngroups + 1];
        let mut end = None;
        if m.m(&self.root, start, &mut caps, &mut |e, _| { end = Some(e); true }) {
            // re-run to capture groups consistently: the closure above keeps caps from the successful path
            caps[0] = Some((start, end.unwrap()));
            return Some(caps);
        }
        None
    }
    /// First match at or after `from` (char index).
    pub fn find_from(&self, text: &str, from: usize) -> Option<Match> {
        let chars: Vec<char> = text.chars().collect();
        for s in from..=chars.len() { if let Some(caps) = self.at(&chars, s) { return Some(Match { caps, chars }); } }
        None
    }
    pub fn find(&self, text: &str) -> Option<Match> { self.find_from(text, 0) }
    /// Python re.match: anchored at the start.
    pub fn match_start(&self, text: &str) -> Option<Match> { let chars: Vec<char> = text.chars().collect(); self.at(&chars, 0).map(|caps| Match { caps, chars }) }
    /// Python re.fullmatch-ish: used with ^...$ patterns via find. This is a helper for `^pat$` semantic.
    pub fn is_match(&self, text: &str) -> bool { self.find(text).is_some() }
    pub fn find_all(&self, text: &str) -> Vec<Match> {
        let chars: Vec<char> = text.chars().collect(); let mut out = vec![]; let mut s = 0;
        while s <= chars.len() {
            match self.at(&chars, s) { Some(caps) => { let (a, b) = caps[0].unwrap(); out.push(Match { caps, chars: chars.clone() }); s = if b == a { b + 1 } else { b }; } None => s += 1 }
        }
        out
    }
    pub fn replace_all(&self, text: &str, rep: &dyn Fn(&Match) -> String) -> String {
        let chars: Vec<char> = text.chars().collect(); let mut out = String::new(); let mut last = 0;
        for m in self.find_all(text) { let (a, b) = m.caps[0].unwrap(); out.extend(&chars[last..a]); out.push_str(&rep(&m)); last = b; }
        out.extend(&chars[last..]); out
    }
    pub fn group_index(&self, name: &str) -> Option<usize> { self.names.get(name).copied() }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn basics() {
        let r = Regex::new(r"(?i)(?<![A-Za-z0-9])mc[-_ ]?(1\.(?:[6-9]|1\d|2[0-2])(?:\.\d{1,2})?)(?!\d)");
        assert_eq!(r.find("create-MC1.20.1-x").unwrap().get(1).unwrap(), "1.20.1");
        assert!(r.find("abcmc1.20.1").is_none());
        let v = Regex::new(r"^[vV]?\d+(?:\.\d+)+(?:\.?[A-Za-z][A-Za-z0-9]*)?$");
        assert!(v.is_match("1.2.3") && v.is_match("v1.2.3b4") && !v.is_match("abc"));
        assert_eq!(Regex::new(r"a+?").find("aaa").unwrap().whole(), "a");
        assert_eq!(Regex::new(r"(?<=x)y").find("zxy").unwrap().start(), 2);
        assert_eq!(Regex::new(r"(?P<a>\d+)-(\d+)").find("ab 12-34").unwrap().get(2).unwrap(), "34");
        assert_eq!(Regex::new(r"[_]+").replace_all("a__b_c", &|_| " ".into()), "a b c");
    }
}
