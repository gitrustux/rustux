// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! sshd: Rustica's SSH server (network step D5, docs/NETWORK.md, man
//! sshd). The SSH protocol is sunset's (vendored with the server pieces it
//! lacked); this program listens, authenticates (passwords through
//! sshd-session, which knows /etc/shadow as login does; Ed25519 public
//! keys from ~/.ssh/authorized_keys), and runs each session's shell or
//! command as the user through sshd-session, on a pseudo-terminal when the
//! client asks for one.
//!
//! One thread a connection; on it a loop over poll(): the socket, and a
//! wake-up pipe the session's reader threads write to when the programs
//! print. Configuration: /etc/ssh/sshd_config (Port, ListenAddress,
//! PermitRootLogin, PasswordAuthentication, PubkeyAuthentication,
//! MaxAuthTries). The host key, made at the first start:
//! /etc/ssh/ssh_host_ed25519_key (and .pub).

mod sys;

use sha2::Digest;
use std::collections::VecDeque;
use std::io::{ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::os::fd::AsRawFd;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;
use sunset::{ChanData, ChanHandle, Event, Runner, ServEvent, SignKey};

const SESSION: &str = "/bin/sshd-session";

fn log(s: &str) {
    println!("sshd: {s}");
    let _ = std::io::stdout().flush();
}

// ---- configuration ----

#[derive(Clone)]
struct Config {
    ports: Vec<u16>,
    listen: Vec<String>,
    root_login: RootLogin,
    password: bool,
    pubkey: bool,
    max_tries: u32,
}

#[derive(Clone, Copy, PartialEq)]
enum RootLogin {
    Yes,
    No,
    KeysOnly, // prohibit-password
}

fn config() -> Config {
    let mut c = Config { ports: vec![], listen: vec![], root_login: RootLogin::KeysOnly, password: true, pubkey: true, max_tries: 6 };
    let text = std::fs::read_to_string("/etc/ssh/sshd_config").unwrap_or_default();
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        let mut w = line.split_whitespace();
        let (Some(k), Some(v)) = (w.next(), w.next()) else { continue };
        let yes = matches!(v.to_ascii_lowercase().as_str(), "yes" | "true" | "on");
        match k.to_ascii_lowercase().as_str() {
            "port" => {
                if let Ok(p) = v.parse() {
                    c.ports.push(p)
                }
            }
            "listenaddress" => c.listen.push(v.to_string()),
            "permitrootlogin" => {
                c.root_login = match v {
                    "yes" => RootLogin::Yes,
                    "no" => RootLogin::No,
                    _ => RootLogin::KeysOnly,
                }
            }
            "passwordauthentication" => c.password = yes,
            "pubkeyauthentication" => c.pubkey = yes,
            "maxauthtries" => c.max_tries = v.parse().unwrap_or(6),
            _ => log(&format!("/etc/ssh/sshd_config: {k} is not known; ignored")),
        }
    }
    if c.ports.is_empty() {
        c.ports.push(22);
    }
    c
}

// ---- the host key ----

fn b64(data: &[u8]) -> String {
    const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::new();
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            if i <= c.len() {
                s.push(T[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                s.push('=');
            }
        }
    }
    s
}

fn unb64(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let (mut acc, mut bits) = (0u32, 0);
    for ch in s.bytes() {
        let v = match ch {
            b'A'..=b'Z' => ch - b'A',
            b'a'..=b'z' => ch - b'a' + 26,
            b'0'..=b'9' => ch - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' => break,
            _ => return None,
        } as u32;
        acc = acc << 6 | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Some(out)
}

/// The SSH wire form of an Ed25519 public key ("ssh-ed25519", the 32 bytes)
fn ed25519_blob(key: &[u8; 32]) -> Vec<u8> {
    let mut b = Vec::new();
    b.extend_from_slice(&11u32.to_be_bytes());
    b.extend_from_slice(b"ssh-ed25519");
    b.extend_from_slice(&32u32.to_be_bytes());
    b.extend_from_slice(key);
    b
}

fn fingerprint(key: &[u8; 32]) -> String {
    let h = sha2::Sha256::digest(ed25519_blob(key));
    format!("SHA256:{}", b64(&h).trim_end_matches('='))
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn host_key() -> Result<SignKey, String> {
    let path = "/etc/ssh/ssh_host_ed25519_key";
    let seed: [u8; 32] = match std::fs::read_to_string(path) {
        Ok(t) => {
            // "rustica-ed25519-seed HEX" (Rustux's own form; not OpenSSH's)
            let h = t.split_whitespace().nth(1).ok_or(format!("{path}: not a host key"))?;
            let mut s = [0u8; 32];
            if h.len() != 64 {
                return Err(format!("{path}: not a host key"));
            }
            for i in 0..32 {
                s[i] = u8::from_str_radix(&h[2 * i..2 * i + 2], 16).map_err(|_| format!("{path}: not a host key"))?;
            }
            s
        }
        Err(_) => {
            let mut s = [0u8; 32];
            sys::random(&mut s);
            let _ = std::fs::create_dir_all("/etc/ssh");
            std::fs::write(path, format!("rustica-ed25519-seed {}\n", hex(&s))).map_err(|e| format!("{path}: {e}"))?;
            sys::chmod(path, 0o600);
            let k = ed25519_dalek::SigningKey::from_bytes(&s);
            let pk = k.verifying_key().to_bytes();
            let host = std::fs::read_to_string("/etc/hostname").unwrap_or_else(|_| "rustica".into());
            let _ = std::fs::write(format!("{path}.pub"), format!("ssh-ed25519 {} root@{}\n", b64(&ed25519_blob(&pk)), host.trim()));
            log(&format!("made the host key {path}"));
            s
        }
    };
    let k = ed25519_dalek::SigningKey::from_bytes(&seed);
    log(&format!("host key {}", fingerprint(&k.verifying_key().to_bytes())));
    Ok(SignKey::Ed25519(k))
}

// ---- users ----

struct User {
    home: String,
    uid: u32,
}

fn user(name: &str) -> Option<User> {
    let text = std::fs::read_to_string("/etc/passwd").ok()?;
    for line in text.lines() {
        let f: Vec<&str> = line.split(':').collect();
        if f.len() >= 7 && f[0] == name {
            return Some(User { home: f[5].to_string(), uid: f[2].parse().ok()? });
        }
    }
    None
}

/// Is this Ed25519 key in the user's ~/.ssh/authorized_keys?
fn key_authorized(u: &User, key: &[u8; 32]) -> bool {
    let want = ed25519_blob(key);
    let text = std::fs::read_to_string(format!("{}/.ssh/authorized_keys", u.home)).unwrap_or_default();
    text.lines().any(|line| {
        let mut w = line.split_whitespace();
        // Options before the type ("from=...") are not supported: such a
        // line is skipped
        w.next() == Some("ssh-ed25519") && w.next().and_then(unb64).as_deref() == Some(&want[..])
    })
}

fn password_ok(name: &str, pass: &str) -> bool {
    let Ok(mut c) = Command::new(SESSION).arg("--check").arg(name).stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null()).spawn()
    else {
        return false;
    };
    if let Some(mut i) = c.stdin.take() {
        let _ = i.write_all(pass.as_bytes());
        let _ = i.write_all(b"\n");
    }
    c.wait().map(|s| s.code() == Some(0)).unwrap_or(false)
}

// ---- sessions ----

enum Out {
    Data(Vec<u8>, bool), // stderr?
    Eof,                 // one reader is done
    Exit(i32),
}

struct Session {
    chan: ChanHandle,
    term: Option<(String, u32, u32)>,
    pty: Option<sys::Pty>,
    child: Option<Child>,
    stdin: Option<std::process::ChildStdin>,
    started: bool,
    rx: Option<mpsc::Receiver<Out>>,
    pending: VecDeque<(Vec<u8>, bool)>,
    readers: u32,
    exit: Option<i32>,
    ended: bool,
}

struct Conn {
    user: Option<String>,
    peer: SocketAddr,
    local: SocketAddr,
    failures: u32,
    sessions: Vec<Session>,
    wake_w: i32,
    cfg: Arc<Config>,
}

fn reader(mut r: impl Read + Send + 'static, err: bool, tx: mpsc::Sender<Out>, wake: i32) {
    std::thread::spawn(move || {
        let mut buf = vec![0u8; 16384];
        loop {
            match r.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if tx.send(Out::Data(buf[..n].to_vec(), err)).is_err() {
                        return;
                    }
                    sys::wake(wake);
                }
            }
        }
        let _ = tx.send(Out::Eof);
        sys::wake(wake);
    });
}

impl Conn {
    fn session(&mut self, num: u32) -> Option<&mut Session> {
        self.sessions.iter_mut().find(|s| s.chan.num().0 == num)
    }

    /// The shell, or a command, for a session
    fn start(&mut self, num: u32, cmd: Option<String>) -> bool {
        let user = self.user.clone().unwrap_or_default();
        let (peer, local, wake) = (self.peer, self.local, self.wake_w);
        let Some(s) = self.session(num) else { return false };
        if s.started {
            return false;
        }
        let mut c = Command::new(SESSION);
        c.arg(&user);
        if let Some(cmd) = &cmd {
            c.arg(cmd);
        }
        c.env_clear();
        c.env("SSH_CONNECTION", format!("{} {} {} {}", peer.ip(), peer.port(), local.ip(), local.port()));
        c.env("SSH_CLIENT", format!("{} {} {}", peer.ip(), peer.port(), local.port()));
        let (tx, rx) = mpsc::channel();
        if let Some((term, cols, rows)) = s.term.clone() {
            c.env("TERM", if term.is_empty() { "xterm".into() } else { term });
            let Ok(p) = sys::Pty::open() else { return false };
            // 0 (a client without a terminal): 80x24
            p.set_size(if cols == 0 { 80 } else { cols as usize }, if rows == 0 { 24 } else { rows as usize });
            let child = match p.spawn(&mut c) {
                Ok(ch) => ch,
                Err(e) => {
                    log(&format!("cannot start a session for {user}: {e}"));
                    return false;
                }
            };
            if let Ok(r) = p.reader() {
                reader(r, false, tx.clone(), wake);
                s.readers = 1;
            }
            s.pty = Some(p);
            s.child = Some(child);
        } else {
            c.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
            let mut child = match c.spawn() {
                Ok(ch) => ch,
                Err(e) => {
                    log(&format!("cannot start a session for {user}: {e}"));
                    return false;
                }
            };
            s.stdin = child.stdin.take();
            if let Some(o) = child.stdout.take() {
                reader(o, false, tx.clone(), wake);
                s.readers += 1;
            }
            if let Some(e) = child.stderr.take() {
                reader(e, true, tx.clone(), wake);
                s.readers += 1;
            }
            s.child = Some(child);
        }
        // Its exit code, when it ends
        if let Some(mut child) = s.child.take() {
            let tx2 = tx.clone();
            std::thread::spawn(move || {
                let code = child.wait().ok().and_then(|st| st.code()).unwrap_or(255);
                let _ = tx2.send(Out::Exit(code));
                sys::wake(wake);
            });
        }
        s.rx = Some(rx);
        s.started = true;
        true
    }
}

fn handle(mut sock: TcpStream, key: Arc<SignKey>, cfg: Arc<Config>) {
    let peer = sock.peer_addr().unwrap_or_else(|_| "0.0.0.0:0".parse().unwrap());
    let local = sock.local_addr().unwrap_or_else(|_| "0.0.0.0:0".parse().unwrap());
    let _ = sock.set_nodelay(true);
    let _ = sock.set_nonblocking(true);
    let (wake_r, wake_w) = sys::pipe();
    let mut inbuf = vec![0u8; 40000];
    let mut outbuf = vec![0u8; 40000];
    let mut runner = Runner::new_server(&mut inbuf, &mut outbuf);
    let mut conn = Conn { user: None, peer, local, failures: 0, sessions: vec![], wake_w, cfg };
    let mut netin = vec![0u8; 65536];
    let (mut in_start, mut in_end) = (0usize, 0usize);
    let mut chanbuf = vec![0u8; 65536];
    let mut closed = false;
    let mut why = String::from("the connection closed");
    'conn: loop {
        // What came over the network, into sunset
        if !closed && in_start == in_end {
            match sock.read(&mut netin) {
                Ok(0) => {
                    closed = true;
                    runner.close_input();
                }
                Ok(n) => {
                    in_start = 0;
                    in_end = n;
                }
                Err(e) if e.kind() == ErrorKind::WouldBlock => {}
                Err(_) => {
                    closed = true;
                    runner.close_input();
                }
            }
        }
        if in_start < in_end {
            match runner.input(&netin[in_start..in_end]) {
                Ok(n) => {
                    log::debug!("input {} of {}", n, in_end - in_start);
                    in_start += n
                }
                Err(e) => {
                    why = format!("protocol error: {e:?}");
                    break 'conn;
                }
            }
        }
        // sunset's events
        loop {
            let ev = match runner.progress() {
                Ok(e) => e,
                Err(e) => {
                    why = format!("protocol error: {e:?}");
                    break 'conn;
                }
            };
            log::debug!("event {:?}", match &ev { Event::Serv(e) => format!("{e:?}"), Event::Progressed => "Progressed".into(), Event::None => "None".into(), Event::Cli(_) => "cli".into() });
            match ev {
                Event::Serv(ServEvent::Hostkeys(h)) => {
                    let _ = h.hostkeys(&[&*key]);
                }
                Event::Serv(ServEvent::FirstAuth(mut a)) => {
                    // Never "none": ask for a password or a key
                    let name = a.username().unwrap_or("").to_string();
                    let root = name == "root";
                    let pw = conn.cfg.password && !(root && conn.cfg.root_login != RootLogin::Yes);
                    let pk = conn.cfg.pubkey && !(root && conn.cfg.root_login == RootLogin::No);
                    let _ = a.set_auth_methods(pw, pk);
                    let _ = a.reject();
                }
                Event::Serv(ServEvent::PasswordAuth(a)) => {
                    let name = a.username().unwrap_or("").to_string();
                    let pass = a.password().unwrap_or("").to_string();
                    let root = name == "root";
                    let ok = conn.cfg.password && !(root && conn.cfg.root_login != RootLogin::Yes) && user(&name).is_some() && password_ok(&name, &pass);
                    if ok {
                        log(&format!("accepted password for {name} from {peer}"));
                        conn.user = Some(name);
                        let _ = a.allow();
                    } else {
                        conn.failures += 1;
                        log(&format!("failed password for {name} from {peer}"));
                        let _ = a.reject();
                        std::thread::sleep(Duration::from_millis(500));
                    }
                }
                Event::Serv(ServEvent::PubkeyAuth(a)) => {
                    let name = a.username().unwrap_or("").to_string();
                    let root = name == "root";
                    let key = match a.pubkey() {
                        Ok(sunset::PubKey::Ed25519(k)) => Some(k.key.0),
                        _ => None,
                    };
                    let ok = conn.cfg.pubkey
                        && !(root && conn.cfg.root_login == RootLogin::No)
                        && key.is_some_and(|k| user(&name).is_some_and(|u| key_authorized(&u, &k)));
                    if ok {
                        conn.user = Some(name.clone());
                        let _ = a.allow();
                    } else {
                        let _ = a.reject();
                    }
                }
                Event::Serv(ServEvent::Authenticated) => {
                    let u = conn.user.clone().unwrap_or_default();
                    log(&format!("{u} logged in from {peer} (uid {})", user(&u).map(|x| x.uid).unwrap_or(0)));
                }
                Event::Serv(ServEvent::OpenSession(o)) => {
                    if conn.user.is_none() {
                        drop(o);
                        continue;
                    }
                    match o.accept() {
                        Ok(chan) => conn.sessions.push(Session {
                            chan,
                            term: None,
                            pty: None,
                            child: None,
                            stdin: None,
                            started: false,
                            rx: None,
                            pending: VecDeque::new(),
                            readers: 0,
                            exit: None,
                            ended: false,
                        }),
                        Err(_) => {}
                    }
                }
                Event::Serv(ServEvent::SessionPty(p)) => {
                    let num = p.channel().0;
                    let info = p.pty().ok().map(|(t, c, r)| (t.to_string(), c, r));
                    if let (Some(s), Some(info)) = (conn.session(num), info) {
                        s.term = Some(info);
                        let _ = p.succeed();
                    } else {
                        let _ = p.fail();
                    }
                }
                Event::Serv(ServEvent::SessionEnv(e)) => {
                    let _ = e.fail(); // not taken (as OpenSSH's AcceptEnv by default)
                }
                Event::Serv(ServEvent::SessionShell(r)) => {
                    let num = r.channel().0;
                    if conn.start(num, None) {
                        let _ = r.succeed();
                    } else {
                        let _ = r.fail();
                    }
                }
                Event::Serv(ServEvent::SessionExec(r)) => {
                    let num = r.channel().0;
                    let cmd = r.command().unwrap_or("").to_string();
                    if conn.start(num, Some(cmd)) {
                        let _ = r.succeed();
                    } else {
                        let _ = r.fail();
                    }
                }
                Event::Serv(ServEvent::SessionSubsystem(r)) => {
                    // sftp and the like: not yet
                    let _ = r.fail();
                }
                Event::Serv(ServEvent::Defunct) => {
                    why = "the session ended".into();
                    break 'conn;
                }
                Event::Serv(ServEvent::PollAgain) | Event::Progressed => continue,
                Event::Cli(_) => {}
                Event::None => break,
            }
        }
        if conn.failures >= conn.cfg.max_tries {
            why = format!("too many authentication failures from {peer}");
            break 'conn;
        }
        // The client's bytes, to the programs
        while let Some((num, dt, _len)) = runner.read_channel_ready() {
            let Some(idx) = conn.sessions.iter().position(|s| s.chan.num() == num) else {
                break;
            };
            let chan = &conn.sessions[idx].chan;
            match runner.read_channel(chan, dt, &mut chanbuf) {
                Ok(n) if n > 0 => {
                    let s = &mut conn.sessions[idx];
                    if let Some(p) = &s.pty {
                        p.write(&chanbuf[..n]);
                    } else if let Some(i) = &mut s.stdin {
                        if i.write_all(&chanbuf[..n]).is_err() {
                            s.stdin = None;
                        }
                    }
                }
                Ok(_) => break,
                Err(_) => break,
            }
        }
        for s in conn.sessions.iter_mut() {
            // The client's EOF: the program's input ends
            if runner.is_channel_eof(&s.chan) && s.stdin.is_some() {
                s.stdin = None;
            }
            if let (Some(p), Some((c, r))) = (&s.pty, runner.take_window_change(&s.chan)) {
                p.set_size(if c == 0 { 80 } else { c as usize }, if r == 0 { 24 } else { r as usize });
            }
            // The programs' output, to the client
            if let Some(rx) = &s.rx {
                while let Ok(o) = rx.try_recv() {
                    match o {
                        Out::Data(d, e) => s.pending.push_back((d, e)),
                        Out::Eof => s.readers = s.readers.saturating_sub(1),
                        Out::Exit(c) => s.exit = Some(c),
                    }
                }
            }
            while let Some((d, e)) = s.pending.front_mut() {
                let dt = if *e { ChanData::Stderr } else { ChanData::Normal };
                match runner.write_channel(&s.chan, dt, d) {
                    Ok(0) => break, // the client's window is full
                    Ok(n) => {
                        d.drain(..n);
                        if d.is_empty() {
                            s.pending.pop_front();
                        }
                    }
                    Err(_) => {
                        s.pending.clear();
                        break;
                    }
                }
            }
            // The end: the program has exited and its output is all sent
            // (a pty's reader may wait for programs it left behind: not
            // after the exit)
            let output_done = s.readers == 0 || (s.pty.is_some() && s.exit.is_some());
            if !s.ended && s.started && s.exit.is_some() && output_done && s.pending.is_empty() {
                let _ = runner.send_exit(&s.chan, s.exit.map(|c| c as u32));
                s.ended = true;
                s.pty = None;
            }
        }
        // Sessions the client has closed
        let mut k = 0;
        while k < conn.sessions.len() {
            if runner.is_channel_closed(&conn.sessions[k].chan) {
                let s = conn.sessions.remove(k);
                if let Some(mut c) = s.child {
                    let _ = c.kill();
                }
                let _ = runner.channel_done(s.chan);
            } else {
                k += 1;
            }
        }
        // Out over the network
        loop {
            let b = runner.output_buf();
            if b.is_empty() {
                break;
            }
            match sock.write(b) {
                Ok(n) => runner.consume_output(n),
                Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                Err(_) => {
                    closed = true;
                    break;
                }
            }
        }
        if closed && in_start == in_end {
            break 'conn;
        }
        // Wait for the network or the programs
        let want_out = runner.is_output_pending();
        let busy = runner.read_channel_ready().is_some() || in_start < in_end;
        if !busy {
            sys::wait(sock.as_raw_fd(), want_out, wake_r, 1000);
            sys::drain(wake_r);
        }
    }
    for s in conn.sessions.iter_mut() {
        if let Some(mut c) = s.child.take() {
            let _ = c.kill();
        }
    }
    sys::close_pipe(wake_r);
    sys::close_pipe(wake_w);
    let u = conn.user.unwrap_or_else(|| "(not logged in)".into());
    log(&format!("{u} from {peer}: {why}"));
}

// `sshd -d`: sunset's own log too (protocol debugging), on stdout
struct DebugLog;

impl log::Log for DebugLog {
    fn enabled(&self, m: &log::Metadata) -> bool {
        m.level() <= log::Level::Debug
    }
    fn log(&self, r: &log::Record) {
        if self.enabled(r.metadata()) {
            println!("sshd: [{}] {}", r.level(), r.args());
            let _ = std::io::stdout().flush();
        }
    }
    fn flush(&self) {}
}

static DEBUG_LOG: DebugLog = DebugLog;

/// Accept connections on one address; listen again whenever netd has
/// restarted (the old socket answers ENETDOWN then)
fn listen(addr: String, key: Arc<SignKey>, cfg: Arc<Config>) {
    let mut said = false;
    loop {
        let l = match TcpListener::bind(&addr) {
            Ok(l) => {
                log(&format!("listening on {addr}"));
                said = false;
                l
            }
            Err(e) => {
                if !said {
                    log(&format!("cannot listen on {addr}: {e}; trying again"));
                    said = true;
                }
                std::thread::sleep(Duration::from_secs(1));
                continue;
            }
        };
        for c in l.incoming() {
            match c {
                Ok(sock) => {
                    let (key, cfg) = (key.clone(), cfg.clone());
                    std::thread::spawn(move || handle(sock, key, cfg));
                }
                Err(e) if e.raw_os_error() == Some(100) => {
                    log("the network service restarted: listening again");
                    std::thread::sleep(Duration::from_millis(500));
                    break;
                }
                Err(e) => {
                    log(&format!("accept: {e}"));
                    std::thread::sleep(Duration::from_millis(100));
                }
            }
        }
    }
}

fn main() {
    if std::env::args().any(|a| a == "-d") {
        let _ = log::set_logger(&DEBUG_LOG);
        log::set_max_level(log::LevelFilter::Debug);
    }
    let cfg = Arc::new(config());
    let key = match host_key() {
        Ok(k) => Arc::new(k),
        Err(e) => {
            eprintln!("sshd: {e}");
            std::process::exit(1);
        }
    };
    // [::] takes IPv4 connections too (as ::ffff:a.b.c.d)
    let addrs: Vec<String> = if cfg.listen.is_empty() { vec!["::".into()] } else { cfg.listen.clone() };
    let mut threads = Vec::new();
    for a in &addrs {
        for p in &cfg.ports {
            let s = if a.contains(':') { format!("[{a}]:{p}") } else { format!("{a}:{p}") };
            let (key, cfg) = (key.clone(), cfg.clone());
            threads.push(std::thread::spawn(move || listen(s, key, cfg)));
        }
    }
    for t in threads {
        let _ = t.join();
    }
}
