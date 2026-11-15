// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! nettest: Rust's std::net through the C library's sockets and netd
//! (network step D2). Each check prints "ok NAME" or "FAIL NAME: why".
//!
//!   nettest local                  TCP and UDP on 127.0.0.1 and on this
//!                                  computer's address: listener, threads,
//!                                  half-close, timeouts, non-blocking
//!   nettest connect HOST:PORT MSG  send MSG, print what comes back until the end
//!   nettest serve PORT             accept one connection: read a line,
//!                                  answer "rust got: LINE", close
//!   nettest udp HOST:PORT MSG      send MSG as a datagram, print the answer

use std::io::{BufRead, BufReader, ErrorKind, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream, ToSocketAddrs, UdpSocket};
use std::time::{Duration, Instant};

fn check(name: &str, r: Result<(), String>) -> bool {
    match r {
        Ok(()) => {
            println!("ok {name}");
            true
        }
        Err(e) => {
            println!("FAIL {name}: {e}");
            false
        }
    }
}

fn s<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

fn echo_pair(addr: &str) -> Result<(), String> {
    let l = TcpListener::bind(addr).map_err(s)?;
    let at = l.local_addr().map_err(s)?;
    if at.port() == 0 {
        return Err("no port".into());
    }
    let t = std::thread::spawn(move || -> Result<(), String> {
        let (mut c, peer) = l.accept().map_err(s)?;
        let mut all = Vec::new();
        c.read_to_end(&mut all).map_err(s)?; // ends at the client's half-close
        c.write_all(format!("{} bytes from {}", all.len(), peer.ip()).as_bytes()).map_err(s)?;
        Ok(())
    });
    let mut c = TcpStream::connect(at).map_err(s)?;
    c.set_nodelay(true).map_err(s)?;
    let big = vec![b'x'; 300_000]; // more than every buffer on the way
    c.write_all(&big).map_err(s)?;
    c.shutdown(Shutdown::Write).map_err(s)?;
    let mut answer = String::new();
    c.read_to_string(&mut answer).map_err(s)?;
    t.join().map_err(|_| "thread")??;
    let want = format!("300000 bytes from {}", c.local_addr().map_err(s)?.ip());
    if answer != want {
        return Err(format!("answer {answer:?}, wanted {want:?}"));
    }
    Ok(())
}

fn refused() -> Result<(), String> {
    match TcpStream::connect("127.0.0.1:1") {
        Err(e) if e.kind() == ErrorKind::ConnectionRefused => Ok(()),
        Err(e) => Err(format!("wrong error: {e}")),
        Ok(_) => Err("it connected".into()),
    }
}

fn in_use() -> Result<(), String> {
    let a = TcpListener::bind("127.0.0.1:0").map_err(s)?;
    let port = a.local_addr().map_err(s)?.port();
    match TcpListener::bind(("127.0.0.1", port)) {
        Err(e) if e.kind() == ErrorKind::AddrInUse => Ok(()),
        Err(e) => Err(format!("wrong error: {e}")),
        Ok(_) => Err("bound twice".into()),
    }
}

fn timeouts() -> Result<(), String> {
    let l = TcpListener::bind("127.0.0.1:0").map_err(s)?;
    let mut c = TcpStream::connect(l.local_addr().map_err(s)?).map_err(s)?;
    let _keep = l.accept().map_err(s)?;
    c.set_read_timeout(Some(Duration::from_millis(300))).map_err(s)?;
    let t = Instant::now();
    let mut b = [0u8; 8];
    match c.read(&mut b) {
        Err(e) if e.kind() == ErrorKind::WouldBlock || e.kind() == ErrorKind::TimedOut => {}
        other => return Err(format!("read gave {other:?}")),
    }
    let ms = t.elapsed().as_millis();
    if !(250..2000).contains(&ms) {
        return Err(format!("waited {ms} ms"));
    }
    // Non-blocking: nothing there yet
    c.set_nonblocking(true).map_err(s)?;
    match c.read(&mut b) {
        Err(e) if e.kind() == ErrorKind::WouldBlock => Ok(()),
        other => Err(format!("non-blocking read gave {other:?}")),
    }
}

fn connect_timeout() -> Result<(), String> {
    // Non-blocking connect, poll, SO_ERROR: what connect_timeout does
    let l = TcpListener::bind("127.0.0.1:0").map_err(s)?;
    let c = TcpStream::connect_timeout(&l.local_addr().map_err(s)?, Duration::from_secs(5)).map_err(s)?;
    let (_a, peer) = l.accept().map_err(s)?;
    if peer != c.local_addr().map_err(s)? {
        return Err(format!("peer {peer} is not {}", c.local_addr().map_err(s)?));
    }
    if c.peer_addr().map_err(s)? != l.local_addr().map_err(s)? {
        return Err("peer_addr".into());
    }
    Ok(())
}

fn udp_local() -> Result<(), String> {
    let a = UdpSocket::bind("127.0.0.1:0").map_err(s)?;
    let b = UdpSocket::bind("127.0.0.1:0").map_err(s)?;
    a.send_to(b"ping", b.local_addr().map_err(s)?).map_err(s)?;
    let mut buf = [0u8; 64];
    b.set_read_timeout(Some(Duration::from_secs(3))).map_err(s)?;
    let (n, from) = b.recv_from(&mut buf).map_err(s)?;
    if &buf[..n] != b"ping" || from != a.local_addr().map_err(s)? {
        return Err(format!("got {:?} from {from}", &buf[..n]));
    }
    b.connect(from).map_err(s)?;
    b.send(b"pong").map_err(s)?;
    a.set_read_timeout(Some(Duration::from_secs(3))).map_err(s)?;
    let n = a.recv(&mut buf).map_err(s)?;
    if &buf[..n] != b"pong" {
        return Err(format!("got {:?}", &buf[..n]));
    }
    Ok(())
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let arg = |i: usize| args.get(i).cloned().unwrap_or_default();
    match arg(1).as_str() {
        "local" => {
            let mut ok = true;
            ok &= check("tcp 127.0.0.1: 300000 bytes, half-close, answer", echo_pair("127.0.0.1:0"));
            // This computer's own address goes through lo too
            let me = std::env::var("NETTEST_ADDR").unwrap_or_else(|_| "10.0.2.15".into());
            ok &= check(&format!("tcp {me} (this computer)"), echo_pair(&format!("{me}:0")));
            ok &= check("tcp 0.0.0.0 listener reached at 127.0.0.1", (|| {
                let l = TcpListener::bind("0.0.0.0:0").map_err(s)?;
                let port = l.local_addr().map_err(s)?.port();
                let mut c = TcpStream::connect(("127.0.0.1", port)).map_err(s)?;
                let (mut a, _) = l.accept().map_err(s)?;
                c.write_all(b"hi").map_err(s)?;
                let mut b = [0u8; 2];
                a.read_exact(&mut b).map_err(s)?;
                (&b == b"hi").then_some(()).ok_or("data".into())
            })());
            ok &= check("connection refused", refused());
            ok &= check("address in use", in_use());
            ok &= check("read time-out, non-blocking", timeouts());
            ok &= check("connect_timeout, addresses", connect_timeout());
            ok &= check("udp 127.0.0.1 send_to, recv_from, connect", udp_local());
            ok &= check("tcp [::1] (IPv6): 300000 bytes, half-close, answer", echo_pair("[::1]:0"));
            ok &= check("udp [::1] (IPv6)", (|| {
                let a = UdpSocket::bind("[::1]:0").map_err(s)?;
                let b = UdpSocket::bind("[::1]:0").map_err(s)?;
                a.send_to(b"six", b.local_addr().map_err(s)?).map_err(s)?;
                let mut buf = [0u8; 16];
                b.set_read_timeout(Some(Duration::from_secs(3))).map_err(s)?;
                let (n, from) = b.recv_from(&mut buf).map_err(s)?;
                (&buf[..n] == b"six" && from == a.local_addr().map_err(s)?).then_some(()).ok_or(format!("got {:?} from {from}", &buf[..n]))
            })());
            ok &= check("[::] listener: IPv4 and IPv6 clients", (|| {
                let l = TcpListener::bind("[::]:0").map_err(s)?;
                let port = l.local_addr().map_err(s)?.port();
                let _c6 = TcpStream::connect(("::1", port)).map_err(s)?;
                let (_a, p6) = l.accept().map_err(s)?;
                let _c4 = TcpStream::connect(("127.0.0.1", port)).map_err(s)?;
                let (_b, p4) = l.accept().map_err(s)?;
                let want4 = "::ffff:127.0.0.1";
                (p6.ip().to_string() == "::1" && p4.ip().to_string() == want4).then_some(()).ok_or(format!("peers {p6} and {p4}"))
            })());
            ok &= check("localhost:80 resolves", (|| {
                let v: Vec<SocketAddr> = "localhost:80".to_socket_addrs().map_err(s)?.collect();
                (v.first().map(|a| a.to_string()) == Some("127.0.0.1:80".into())).then_some(()).ok_or(format!("{v:?}"))
            })());
            println!("nettest local: {}", if ok { "all passed" } else { "failures" });
            std::process::exit(if ok { 0 } else { 1 });
        }
        "connect" => {
            let mut c = TcpStream::connect(arg(2).as_str()).unwrap_or_else(|e| {
                eprintln!("nettest: connect {}: {e}", arg(2));
                std::process::exit(1)
            });
            c.write_all(arg(3).as_bytes()).unwrap();
            c.write_all(b"\n").unwrap();
            c.shutdown(Shutdown::Write).unwrap();
            let mut all = String::new();
            c.read_to_string(&mut all).unwrap();
            print!("{all}");
        }
        "serve" => {
            let l = TcpListener::bind(("0.0.0.0", arg(2).parse::<u16>().unwrap_or(8081))).unwrap_or_else(|e| {
                eprintln!("nettest: bind: {e}");
                std::process::exit(1)
            });
            println!("nettest: listening on {}", l.local_addr().unwrap());
            let (c, peer) = l.accept().unwrap();
            let mut line = String::new();
            BufReader::new(&c).read_line(&mut line).unwrap();
            (&c).write_all(format!("rust got: {}", line).as_bytes()).unwrap();
            println!("nettest: served {peer}");
        }
        "udp" => {
            let u = UdpSocket::bind("0.0.0.0:0").unwrap();
            u.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            u.send_to(arg(3).as_bytes(), arg(2).as_str()).unwrap();
            let mut buf = [0u8; 1500];
            match u.recv_from(&mut buf) {
                Ok((n, from)) => println!("{} from {from}", String::from_utf8_lossy(&buf[..n])),
                Err(e) => {
                    eprintln!("nettest: no answer: {e}");
                    std::process::exit(1)
                }
            }
        }
        _ => {
            eprintln!("usage: nettest local | connect HOST:PORT MSG | serve PORT | udp HOST:PORT MSG");
            std::process::exit(2);
        }
    }
}
