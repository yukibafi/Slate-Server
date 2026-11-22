use create_panel::rx::Regex;
use std::io::BufRead;
fn main() {
    // input lines: pattern<TAB>text ; output: JSON-ish "null" or [whole, g1, g2...] with None as null
    for line in std::io::stdin().lock().lines().flatten() {
        let (p, t) = line.split_once('\t').unwrap();
        let r = Regex::new(p);
        match r.find(t) {
            None => println!("null"),
            Some(m) => { let n = m.caps.len(); let v: Vec<String> = (0..n).map(|i| match m.get(i) { Some(s) => format!("{:?}", s), None => "null".into() }).collect(); println!("[{}]", v.join(",")); }
        }
    }
}
