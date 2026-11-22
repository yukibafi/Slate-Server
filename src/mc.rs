//! Runs the Minecraft server (start.sh) and keeps its console, state and player list.
use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

const MAX_LINES: usize = 5000;

pub struct Inner {
    pub state: &'static str, // stopped | starting | online | stopping
    child: Option<Child>,
    stdin: Option<ChildStdin>,
    pub lines: VecDeque<String>,
    pub total: usize,
    pub players: Vec<String>,
    started: Option<Instant>,
    restart_pending: bool,
    pub saved: bool,
    pub user_stop: bool,
    pub exit_code: Option<i32>,
    crash_times: Vec<Instant>,
    pub started_unix: u64,
    gen: u64, // bumps on every start so old watcher threads can tell they are stale
}

#[derive(Clone)]
pub struct Mc {
    pub dir: PathBuf,
    pub inner: Arc<Mutex<Inner>>,
}

pub struct Snapshot {
    pub state: &'static str,
    pub restarting: bool,
    pub players: Vec<String>,
    pub mem_mb: Option<u64>,
    pub uptime: Option<u64>,
    pub lines: Vec<String>,
    pub total: usize,
}

impl Inner { pub fn restart_pending_pub(&self) -> bool { self.restart_pending } }

impl Mc {
    pub fn new(dir: PathBuf) -> Mc {
        Mc { dir, inner: Arc::new(Mutex::new(Inner {
            state: "stopped", child: None, stdin: None, lines: VecDeque::new(), total: 0,
            players: Vec::new(), started: None, restart_pending: false, saved: false, user_stop: false, exit_code: None, crash_times: Vec::new(), started_unix: 0, gen: 0,
        })) }
    }

    pub fn log(&self, line: &str) {
        let mut g = self.inner.lock().unwrap();
        push_line(&mut g, line.to_string());
    }

    pub fn has_start(&self) -> bool {
        self.dir.join(if cfg!(windows) { "start.bat" } else { "start.sh" }).is_file()
    }

    pub fn running(&self) -> bool {
        let g = self.inner.lock().unwrap();
        g.state != "stopped"
    }

    pub fn start(&self) -> (bool, String) {
        {
            let g = self.inner.lock().unwrap();
            if g.state != "stopped" { return (false, "The server is already running.".into()); }
        }
        if !self.has_start() {
            return (false, format!("Couldn't find start.sh in {}. Run the setup script first, or launch the panel with SERVER_DIR set.", self.dir.display()));
        }
        let mut cmd = if cfg!(windows) {
            let mut c = Command::new("cmd"); c.args(["/C", "start.bat"]); c
        } else {
            let mut c = Command::new("bash"); c.arg("./start.sh"); c
        };
        #[cfg(unix)]
        { use std::os::unix::process::CommandExt; cmd.process_group(0); }
        cmd.current_dir(&self.dir).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = match cmd.spawn() { Ok(c) => c, Err(e) => return (false, format!("Couldn't start it: {e}")) };
        let out = child.stdout.take();
        let err = child.stderr.take();
        let stdin = child.stdin.take();
        let gen;
        {
            let mut g = self.inner.lock().unwrap();
            g.gen += 1;
            gen = g.gen;
            g.state = "starting";
            g.child = Some(child);
            g.stdin = stdin;
            g.players.clear();
            g.started = Some(Instant::now());
            g.started_unix = crate::sys::now() as u64;
            g.restart_pending = false;
            g.user_stop = false;
            g.exit_code = None;
            push_line(&mut g, "[panel] Starting server...".into());
        }
        if let Some(o) = out { self.reader(o, gen); }
        if let Some(e) = err { self.reader(e, gen); }
        self.watcher(gen);
        (true, "Starting...".into())
    }

    fn reader<R: std::io::Read + Send + 'static>(&self, r: R, gen: u64) {
        let me = self.clone();
        thread::spawn(move || {
            for line in BufReader::new(r).split(b'\n') {
                let Ok(raw) = line else { break };
                let l = String::from_utf8_lossy(&raw).trim_end_matches('\r').to_string();
                let mut g = me.inner.lock().unwrap();
                if g.gen != gen { break; }
                track(&mut g, &l);
                push_line(&mut g, l);
            }
        });
    }

    fn watcher(&self, gen: u64) {
        let me = self.clone();
        thread::spawn(move || loop {
            thread::sleep(Duration::from_millis(300));
            let mut g = me.inner.lock().unwrap();
            if g.gen != gen { return; }
            let status = match g.child.as_mut() { Some(c) => match c.try_wait() { Ok(Some(st)) => Some(st.code()), _ => None }, None => Some(None) };
            let Some(code) = status else { continue };
            let code = code.unwrap_or(-1);
            g.child = None; g.stdin = None; g.saved = true; g.state = "stopped"; g.players.clear(); g.started = None; g.exit_code = Some(code);
            push_line(&mut g, format!("[panel] Server stopped (exit code {code})."));
            let mut delay = 0u64;
            if g.restart_pending {
                g.restart_pending = false; delay = 2;
                push_line(&mut g, "[panel] Restarting...".into());
            } else if !g.user_stop && code != 0 && crate::envinfo::load_settings(&me.dir).auto_restart {
                g.crash_times.retain(|t| t.elapsed().as_secs() < 600);
                if g.crash_times.len() < 3 {
                    g.crash_times.push(Instant::now()); delay = 6;
                    push_line(&mut g, "[panel] The server crashed. Auto-restarting in 6s...".into());
                } else {
                    push_line(&mut g, "[panel] The server crashed 3 times in 10 minutes. Not restarting again, check the log above.".into());
                }
            }
            drop(g);
            if delay > 0 { thread::sleep(Duration::from_secs(delay)); let (ok, m) = me.start(); if !ok { me.log(&format!("[panel] {m}")); } }
            return;
        });
    }

    pub fn send(&self, cmd: &str) -> bool {
        let mut g = self.inner.lock().unwrap();
        match g.stdin.as_mut() {
            Some(s) => s.write_all(cmd.as_bytes()).and_then(|_| s.write_all(b"\n")).and_then(|_| s.flush()).is_ok(),
            None => false,
        }
    }

    pub fn stop(&self) -> (bool, String) {
        {
            let mut g = self.inner.lock().unwrap();
            if g.state == "stopped" { return (false, "The server isn't running.".into()); }
            g.state = "stopping";
            g.user_stop = true;
            push_line(&mut g, "[panel] Stopping server (saving world)...".into());
        }
        self.send("stop");
        // If it hasn't exited after 90 s, force it so a stuck server can't block the panel.
        let me = self.clone();
        let gen = self.inner.lock().unwrap().gen;
        thread::spawn(move || {
            for _ in 0..300 { thread::sleep(Duration::from_millis(300)); if me.inner.lock().unwrap().gen != gen || !me.running() { return; } }
            let _ = me.kill();
        });
        (true, "Stopping...".into())
    }

    pub fn restart(&self) -> (bool, String) {
        if !self.running() { return self.start(); }
        self.inner.lock().unwrap().restart_pending = true;
        let r = self.stop();
        if r.0 { self.inner.lock().unwrap().user_stop = false; (true, "Restarting...".into()) } else { r }
    }

    pub fn kill(&self) -> (bool, String) {
        let pid;
        {
            let mut g = self.inner.lock().unwrap();
            let Some(c) = g.child.as_ref() else { return (false, "The server isn't running.".into()) };
            pid = c.id();
            g.user_stop = true; g.restart_pending = false;
            push_line(&mut g, "[panel] Force stopping server. The last few minutes may not be saved.".into());
        }
        kill_group(pid, false);
        let me = self.clone();
        thread::spawn(move || {
            thread::sleep(Duration::from_secs(8));
            let still = me.inner.lock().unwrap().child.as_ref().map(|c| c.id()) == Some(pid);
            if still { kill_group(pid, true); }
        });
        (true, "Force stopping...".into())
    }

    pub fn started_unix(&self) -> u64 { self.inner.lock().unwrap().started_unix }
    pub fn clear_saved(&self) { self.inner.lock().unwrap().saved = false; }
    pub fn wait_saved(&self, secs: u64) -> bool {
        let end = Instant::now() + Duration::from_secs(secs);
        while Instant::now() < end { if self.inner.lock().unwrap().saved { return true; } thread::sleep(Duration::from_millis(200)); }
        false
    }

    /// Waits (up to `secs`) for the server to be fully stopped.
    pub fn wait_stopped(&self, secs: u64) -> bool {
        let end = Instant::now() + Duration::from_secs(secs);
        while Instant::now() < end { if !self.running() { return true; } thread::sleep(Duration::from_millis(250)); }
        !self.running()
    }

    pub fn snapshot(&self, since: usize) -> Snapshot {
        let (state, restarting, players, uptime, lines, total, pid);
        {
            let g = self.inner.lock().unwrap();
            let first = g.total - g.lines.len();
            let since = since.max(first).min(g.total);
            state = g.state;
            restarting = g.restart_pending;
            players = g.players.clone();
            uptime = if state == "online" { g.started.map(|s| s.elapsed().as_secs()) } else { None };
            lines = g.lines.iter().skip(since - first).cloned().collect::<Vec<_>>();
            total = g.total;
            pid = g.child.as_ref().map(|c| c.id());
        }
        let mem_mb = pid.and_then(tree_rss_mb);
        Snapshot { state, restarting, players, mem_mb, uptime, lines, total }
    }
}

fn push_line(g: &mut Inner, l: String) {
    g.lines.push_back(l);
    g.total += 1;
    while g.lines.len() > MAX_LINES { g.lines.pop_front(); }
}

/// Reads state changes out of the server's own log lines.
fn track(g: &mut Inner, l: &str) {
    if l.contains("Saved the game") || l.contains("All dimensions are saved") || l.contains("Saved the world") { g.saved = true; }
    if g.state == "starting" && l.contains("]: Done (") && l.contains("For help") { g.state = "online"; g.started = Some(Instant::now()); }
    if let Some(i) = l.find("]: ") {
        let rest = &l[i + 3..];
        if let Some(n) = rest.strip_suffix(" joined the game") { if valid_name(n) && !g.players.iter().any(|p| p == n) { g.players.push(n.to_string()); } }
        if let Some(n) = rest.strip_suffix(" left the game") { g.players.retain(|p| p != n); }
    }
}

fn valid_name(n: &str) -> bool { !n.is_empty() && n.len() <= 16 && n.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.') }

/// Memory of the whole process tree in MB, via `ps` (Mac and Linux). None on Windows or if ps is missing.
fn tree_rss_mb(root: u32) -> Option<u64> {
    if cfg!(windows) { return None; }
    let out = Command::new("ps").args(["-A", "-o", "pid=,ppid=,rss="]).output().ok()?;
    let txt = String::from_utf8_lossy(&out.stdout);
    let rows: Vec<(u32, u32, u64)> = txt.lines().filter_map(|l| {
        let mut p = l.split_whitespace();
        Some((p.next()?.parse().ok()?, p.next()?.parse().ok()?, p.next()?.parse().ok()?))
    }).collect();
    let mut set = vec![root];
    let mut kb = 0u64;
    let mut i = 0;
    while i < set.len() {
        let cur = set[i];
        for (pid, ppid, rss) in &rows {
            if *pid == cur { kb += rss; }
            if *ppid == cur && !set.contains(pid) { set.push(*pid); }
        }
        i += 1;
    }
    Some(kb / 1024)
}

/// Signal the server's whole process group (start.sh and the Java it launched).
fn kill_group(pid: u32, hard: bool) {
    if cfg!(windows) {
        let _ = Command::new("taskkill").args(["/PID", &pid.to_string(), "/T", "/F"]).output();
    } else {
        let sig = if hard { "-KILL" } else { "-TERM" };
        let _ = Command::new("kill").args([sig, "--", &format!("-{pid}")]).output();
    }
}
