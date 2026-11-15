// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Sockets for programs (network step D2, <rustux/netsock.h>,
//! docs/NETWORK.md): each one a connection to /svc/netsock, its control
//! channel. TCP connections carry their bytes on a kernel socket pair (the
//! data socket), copied to and from smoltcp's TCP socket here; UDP
//! datagrams are messages on the control channel.
//!
//! smoltcp's sockets belong to one interface each, so a listening or UDP
//! socket bound to 0.0.0.0 has one smoltcp socket on every interface
//! (several for a listener: its backlog), lo among them; a connection has
//! one, on the interface its route takes (lo for this computer's own
//! addresses).

use crate::{log, now, rx, Netd};
use smoltcp::iface::SocketHandle;
use smoltcp::socket::{tcp, udp};
use smoltcp::time::{Duration, Instant};
use smoltcp::wire::{IpAddress, IpEndpoint, IpListenEndpoint, Ipv4Address, Ipv6Address};

const OPEN: u32 = 1;
const BIND: u32 = 2;
const LISTEN: u32 = 3;
const CONNECT: u32 = 4;
const ACCEPTED: u32 = 5;
const SENDTO: u32 = 6;
const DGRAM: u32 = 7;
const ERROR: u32 = 8;
const SETOPT: u32 = 9;
const OPT_NODELAY: u32 = 1;
const OPT_KEEPALIVE: u32 = 2;
const OPT_REUSEADDR: u32 = 3;
const MSG: usize = 56; // sizeof(rx_netsock_msg_t)
const AF_INET: u16 = 2;
const AF_INET6: u16 = 10;
const MAX_DGRAM: usize = 65000;
const TCP_BUF: usize = 64 * 1024;
const CONNECT_TIMEOUT: u64 = 30; // seconds

// Linux's errno numbers
const EACCES: i32 = 13;
const EINVAL: i32 = 22;
const EOPNOTSUPP: i32 = 95;
const EAFNOSUPPORT: i32 = 97;
const EADDRINUSE: i32 = 98;
const EADDRNOTAVAIL: i32 = 99;
const ENETDOWN: i32 = 100;
const ENETUNREACH: i32 = 101;
const ECONNRESET: i32 = 104;
const EISCONN: i32 = 106;
const ETIMEDOUT: i32 = 110;
const ECONNREFUSED: i32 = 111;
const EALREADY: i32 = 114;

#[derive(Clone, Copy, PartialEq, Debug)]
enum State {
    New,        // waiting for OPEN
    Open,       // OPEN done, not bound
    Bound,
    Listening,
    Connecting,
    Connected,
}

pub struct Sock {
    pub id: u64,
    pub ctl: rx::Handle, // 0 once the program has closed it
    stream: bool,
    v6: bool, // an AF_INET6 socket (IPv4 peers show as ::ffff:a.b.c.d in the library)
    state: State,
    local: Option<(IpAddress, u16)>, // UNSPECIFIED: any address
    peer: Option<(IpAddress, u16)>,
    backlog: usize,
    reuse: bool,
    nodelay: bool,
    keepalive: bool,
    // A TCP connection
    link: u32,
    tcp: Option<SocketHandle>,
    pub data: rx::Handle,
    data_full: bool,     // the program's side of the data socket is full
    to_prog_done: bool,  // the peer's FIN passed on (or nowhere to pass data)
    from_prog_done: bool, // the program wrote its last (we sent FIN)
    deadline: Option<Instant>,
    // Listening and UDP sockets: smoltcp sockets per interface
    per_link: Vec<(u32, SocketHandle)>,
    remove: bool,
}

fn new_sock(id: u64, ctl: rx::Handle) -> Sock {
    Sock {
        id,
        ctl,
        stream: true,
        v6: false,
        state: State::New,
        local: None,
        peer: None,
        backlog: 4,
        reuse: false,
        nodelay: false,
        keepalive: false,
        link: 0,
        tcp: None,
        data: 0,
        data_full: false,
        to_prog_done: false,
        from_prog_done: false,
        deadline: None,
        per_link: Vec::new(),
        remove: false,
    }
}

impl Sock {
    /// What to wait for on the data socket, if anything
    pub fn data_signals(&self, n: &Netd) -> Option<u32> {
        if self.data == 0 || self.state != State::Connected {
            return None;
        }
        let mut sig = 0;
        if self.data_full && !self.to_prog_done {
            sig |= rx::WRITABLE | rx::PEER_CLOSED;
        }
        if !self.from_prog_done {
            if let (Some(i), Some(h)) = (n.link_index(self.link), self.tcp) {
                if n.links[i].sockets.get::<tcp::Socket>(h).can_send() {
                    sig |= rx::CHANNEL_READABLE | rx::PEER_CLOSED;
                }
            }
        }
        (sig != 0).then_some(sig)
    }
}

// ---- messages ----

struct Msg {
    op: u32,
    status: i32,
    a: u32,
    b: u32,
    addr: Option<(IpAddress, u16)>,
    addr2: Option<(IpAddress, u16)>,
}

fn put_addr(out: &mut [u8], a: Option<(IpAddress, u16)>) {
    match a {
        Some((IpAddress::Ipv4(ip), port)) => {
            out[..2].copy_from_slice(&AF_INET.to_le_bytes());
            out[2..4].copy_from_slice(&port.to_le_bytes());
            out[4..8].copy_from_slice(&ip.octets());
        }
        Some((IpAddress::Ipv6(ip), port)) => {
            out[..2].copy_from_slice(&AF_INET6.to_le_bytes());
            out[2..4].copy_from_slice(&port.to_le_bytes());
            out[4..20].copy_from_slice(&ip.octets());
        }
        None => {}
    }
}

fn encode(m: &Msg, data: &[u8]) -> Vec<u8> {
    let mut b = vec![0u8; MSG + data.len()];
    b[0..4].copy_from_slice(&m.op.to_le_bytes());
    b[4..8].copy_from_slice(&m.status.to_le_bytes());
    b[8..12].copy_from_slice(&m.a.to_le_bytes());
    b[12..16].copy_from_slice(&m.b.to_le_bytes());
    put_addr(&mut b[16..36], m.addr);
    put_addr(&mut b[36..56], m.addr2);
    b[MSG..].copy_from_slice(data);
    b
}

fn get_addr(b: &[u8]) -> Result<(IpAddress, u16), i32> {
    let fam = u16::from_le_bytes([b[0], b[1]]);
    let port = u16::from_le_bytes([b[2], b[3]]);
    match fam {
        AF_INET => Ok((IpAddress::Ipv4(Ipv4Address::new(b[4], b[5], b[6], b[7])), port)),
        AF_INET6 => {
            let mut o = [0u8; 16];
            o.copy_from_slice(&b[4..20]);
            Ok((IpAddress::Ipv6(Ipv6Address::from_octets(o)), port))
        }
        _ => Err(EAFNOSUPPORT),
    }
}

/// "any address" of the socket's family
fn any(v6: bool) -> IpAddress {
    if v6 {
        IpAddress::Ipv6(Ipv6Address::UNSPECIFIED)
    } else {
        IpAddress::Ipv4(Ipv4Address::UNSPECIFIED)
    }
}

fn send(ch: rx::Handle, m: Msg, data: &[u8], handles: &[rx::Handle]) -> bool {
    ch != 0 && rx::channel_write(ch, &encode(&m, data), handles) == rx::OK
}

fn answer(ch: rx::Handle, op: u32, status: i32, addr: Option<(IpAddress, u16)>) {
    send(ch, Msg { op, status: -status.abs(), a: 0, b: 0, addr, addr2: None }, &[], &[]);
}

// ---- ports ----

fn port_used(n: &Netd, stream: bool, addr: IpAddress, port: u16, except: u64, reuse: bool) -> bool {
    n.socks.iter().any(|s| {
        if s.id == except || s.stream != stream || s.remove {
            return false;
        }
        let Some((a, p)) = s.local else { return false };
        if p != port {
            return false;
        }
        // A connection from connect() holds its port too; with SO_REUSEADDR
        // only listeners and bound sockets count (Linux's rule, near enough)
        if reuse && s.stream && matches!(s.state, State::Connected | State::Connecting) {
            return false;
        }
        a.is_unspecified() || addr.is_unspecified() || a == addr
    })
}

fn ephemeral(n: &mut Netd, stream: bool, addr: IpAddress) -> Option<u16> {
    for _ in 0..16384 {
        let p = n.next_port;
        n.next_port = if n.next_port >= 65535 { 49152 } else { n.next_port + 1 };
        if !port_used(n, stream, addr, p, 0, false) {
            return Some(p);
        }
    }
    None
}

// ---- smoltcp sockets ----

fn tcp_socket() -> tcp::Socket<'static> {
    tcp::Socket::new(tcp::SocketBuffer::new(vec![0; TCP_BUF]), tcp::SocketBuffer::new(vec![0; TCP_BUF]))
}

fn udp_socket() -> udp::Socket<'static> {
    udp::Socket::new(
        udp::PacketBuffer::new(vec![udp::PacketMetadata::EMPTY; 64], vec![0; 128 * 1024]),
        udp::PacketBuffer::new(vec![udp::PacketMetadata::EMPTY; 64], vec![0; 128 * 1024]),
    )
}

fn listen_endpoint(local: (IpAddress, u16)) -> IpListenEndpoint {
    IpListenEndpoint { addr: if local.0.is_unspecified() { None } else { Some(local.0) }, port: local.1 }
}

/// Does interface i serve a socket bound to `addr`?
fn link_serves(n: &Netd, i: usize, addr: IpAddress) -> bool {
    let l = &n.links[i];
    match addr {
        _ if addr.is_unspecified() => true,
        _ if l.is_lo() => n.is_local(addr),
        IpAddress::Ipv4(a) => l.v4().iter().any(|c| c.address() == a),
        IpAddress::Ipv6(a) => l.v6().iter().any(|c| c.address() == a),
    }
}

// A listener's smoltcp sockets on interface i (its backlog)
fn add_listeners(n: &mut Netd, si: usize, i: usize) {
    let s = &n.socks[si];
    let Some(local) = s.local else { return };
    let count = s.backlog.clamp(1, 8);
    let (id, nodelay) = (n.links[i].id, s.nodelay);
    let have = s.per_link.iter().filter(|(l, _)| *l == id).count();
    for _ in have..count {
        let mut t = tcp_socket();
        t.set_nagle_enabled(!nodelay);
        if t.listen(listen_endpoint(local)).is_err() {
            return;
        }
        let h = n.links[i].sockets.add(t);
        n.socks[si].per_link.push((id, h));
    }
}

fn add_udp(n: &mut Netd, si: usize, i: usize) {
    let Some(local) = n.socks[si].local else { return };
    let id = n.links[i].id;
    if n.socks[si].per_link.iter().any(|(l, _)| *l == id) {
        return;
    }
    let mut u = udp_socket();
    if u.bind(listen_endpoint(local)).is_err() {
        return;
    }
    let h = n.links[i].sockets.add(u);
    n.socks[si].per_link.push((id, h));
}

/// A new interface: listeners and UDP sockets bound to any address (or to
/// one it has) get their smoltcp sockets on it
pub fn link_added(n: &mut Netd, id: u32) {
    let Some(i) = n.link_index(id) else { return };
    for si in 0..n.socks.len() {
        let s = &n.socks[si];
        let Some((addr, _)) = s.local else { continue };
        if !link_serves(n, i, addr) {
            continue;
        }
        match (s.stream, s.state) {
            (true, State::Listening) => add_listeners(n, si, i),
            (false, State::Bound | State::Connected) => add_udp(n, si, i),
            _ => {}
        }
    }
}

/// An interface gone: its sockets with it; its connections fail
pub fn link_removed(n: &mut Netd, id: u32) {
    for s in n.socks.iter_mut() {
        s.per_link.retain(|(l, _)| *l != id);
        if s.tcp.is_some() && s.link == id {
            s.tcp = None;
            if s.state == State::Connecting {
                answer(s.ctl, CONNECT, ENETDOWN, None);
            } else {
                send(s.ctl, Msg { op: ERROR, status: -ENETDOWN, a: 0, b: 0, addr: None, addr2: None }, &[], &[]);
            }
            s.remove = true;
        }
    }
    let gone: Vec<u64> = n.socks.iter().filter(|s| s.remove).map(|s| s.id).collect();
    for g in gone {
        drop_sock(n, g);
    }
}

fn index(n: &Netd, id: u64) -> Option<usize> {
    n.socks.iter().position(|s| s.id == id)
}

/// Forget a socket: its smoltcp sockets, its handles
fn drop_sock(n: &mut Netd, id: u64) {
    let Some(si) = index(n, id) else { return };
    let s = n.socks.remove(si);
    for (l, h) in &s.per_link {
        if let Some(i) = n.link_index(*l) {
            n.links[i].sockets.remove(*h);
        }
    }
    if let Some(h) = s.tcp {
        if let Some(i) = n.link_index(s.link) {
            n.links[i].sockets.remove(h);
        }
    }
    if s.ctl != 0 {
        rx::close(s.ctl);
    }
    if s.data != 0 {
        rx::close(s.data);
    }
}

pub fn new_connection(n: &mut Netd, ctl: rx::Handle) {
    let id = n.next_sock;
    n.next_sock += 1;
    n.socks.push(new_sock(id, ctl));
}

// ---- requests ----

pub fn ctl_ready(n: &mut Netd, id: u64, buf: &mut [u8]) {
    loop {
        let Some(si) = index(n, id) else { return };
        let ctl = n.socks[si].ctl;
        if ctl == 0 {
            return;
        }
        let mut who = rx::Sender::default();
        match rx::channel_read(ctl, buf, Some(&mut who)) {
            Ok((len, h)) => {
                if len < MSG {
                    if let Some(h) = h {
                        rx::close(h);
                    }
                    continue;
                }
                let m = Msg {
                    op: u32::from_le_bytes(buf[0..4].try_into().unwrap()),
                    status: 0,
                    a: u32::from_le_bytes(buf[8..12].try_into().unwrap()),
                    b: u32::from_le_bytes(buf[12..16].try_into().unwrap()),
                    addr: get_addr(&buf[16..36]).ok(),
                    addr2: None,
                };
                let fam_ok = get_addr(&buf[16..36]).is_ok();
                let data = buf[MSG..len].to_vec();
                request(n, si, &who, m, fam_ok, data, h);
            }
            Err(rx::ERR_SHOULD_WAIT) => return,
            Err(_) => {
                // The program closed the socket
                program_closed(n, si);
                return;
            }
        }
    }
}

fn program_closed(n: &mut Netd, si: usize) {
    let li = n.link_index(n.socks[si].link);
    let s = &mut n.socks[si];
    rx::close(s.ctl);
    s.ctl = 0;
    match s.state {
        // A connection finishes sending what the program wrote, then closes
        State::Connected => {}
        State::Connecting => {
            if let (Some(i), Some(h)) = (li, s.tcp) {
                n.links[i].sockets.get_mut::<tcp::Socket>(h).abort();
            }
            s.remove = true;
        }
        _ => s.remove = true,
    }
    if n.socks[si].remove {
        let id = n.socks[si].id;
        drop_sock(n, id);
    }
}

fn request(n: &mut Netd, si: usize, who: &rx::Sender, m: Msg, fam_ok: bool, data: Vec<u8>, h: Option<rx::Handle>) {
    let ctl = n.socks[si].ctl;
    let state = n.socks[si].state;
    let stream = n.socks[si].stream;
    match m.op {
        OPEN => {
            if state != State::New {
                return answer(ctl, OPEN, EINVAL, None);
            }
            let s = &mut n.socks[si];
            match m.a {
                1 => s.stream = true,
                2 => s.stream = false,
                _ => return answer(ctl, OPEN, EINVAL, None),
            }
            s.v6 = m.b == AF_INET6 as u32;
            s.state = State::Open;
            answer(ctl, OPEN, 0, None);
        }
        BIND => {
            let Some((addr, port)) = m.addr.filter(|_| fam_ok) else { return answer(ctl, BIND, EAFNOSUPPORT, None) };
            if state != State::Open {
                return answer(ctl, BIND, EINVAL, None);
            }
            match bind(n, si, who, addr, port) {
                Ok(l) => answer(ctl, BIND, 0, Some(l)),
                Err(e) => answer(ctl, BIND, e, None),
            }
        }
        LISTEN => {
            if !stream || !matches!(state, State::Open | State::Bound | State::Listening) {
                return answer(ctl, LISTEN, if stream { EINVAL } else { EOPNOTSUPP }, None);
            }
            if state == State::Open {
                if let Err(e) = bind(n, si, who, any(n.socks[si].v6), 0) {
                    return answer(ctl, LISTEN, e, None);
                }
            }
            n.socks[si].backlog = (m.a as usize).clamp(1, 8);
            n.socks[si].state = State::Listening;
            for i in 0..n.links.len() {
                let addr = n.socks[si].local.unwrap().0;
                if link_serves(n, i, addr) {
                    add_listeners(n, si, i);
                }
            }
            let l = n.socks[si].local.unwrap();
            log_sock(n, si, &format!("listening on {}:{}", l.0, l.1));
            answer(ctl, LISTEN, 0, None);
        }
        CONNECT => {
            let Some((addr, port)) = m.addr.filter(|_| fam_ok) else {
                if let Some(h) = h {
                    rx::close(h);
                }
                return answer(ctl, CONNECT, EAFNOSUPPORT, None);
            };
            if stream {
                let Some(data) = h else { return answer(ctl, CONNECT, EINVAL, None) };
                match state {
                    State::Connected | State::Listening => {
                        rx::close(data);
                        return answer(ctl, CONNECT, if state == State::Connected { EISCONN } else { EINVAL }, None);
                    }
                    State::Connecting => {
                        rx::close(data);
                        return answer(ctl, CONNECT, EALREADY, None);
                    }
                    _ => {}
                }
                if let Err(e) = connect_tcp(n, si, who, addr, port, data) {
                    rx::close(data);
                    // Back to what it was: a refused connect may be tried again
                    let s = &mut n.socks[si];
                    s.data = 0;
                    answer(ctl, CONNECT, e, None);
                }
            } else {
                if let Some(h) = h {
                    rx::close(h);
                }
                if addr.is_unspecified() && port == 0 {
                    // AF_UNSPEC-like: disconnect
                    n.socks[si].peer = None;
                    return answer(ctl, CONNECT, 0, n.socks[si].local);
                }
                if n.route(addr).is_none() {
                    return answer(ctl, CONNECT, ENETUNREACH, None);
                }
                if n.socks[si].local.is_none() {
                    if let Err(e) = bind(n, si, who, any(n.socks[si].v6), 0) {
                        return answer(ctl, CONNECT, e, None);
                    }
                }
                let s = &mut n.socks[si];
                s.peer = Some((addr, port));
                s.state = State::Connected;
                let local = s.local;
                answer(ctl, CONNECT, 0, local.map(|(a, p)| (if a.is_unspecified() { n.route(addr).unwrap().1 } else { a }, p)));
            }
        }
        SENDTO => {
            if let Some(h) = h {
                rx::close(h);
            }
            if stream || data.len() > MAX_DGRAM {
                return;
            }
            let dst = match m.addr.filter(|(_, p)| *p != 0) {
                Some(d) => d,
                None => match n.socks[si].peer {
                    Some(p) => p,
                    None => return,
                },
            };
            if n.socks[si].local.is_none() && bind(n, si, who, any(n.socks[si].v6), 0).is_err() {
                return;
            }
            send_udp(n, si, dst, &data);
        }
        SETOPT => {
            if let Some(h) = h {
                rx::close(h);
            }
            let on = m.b != 0;
            let li = n.link_index(n.socks[si].link);
            let s = &mut n.socks[si];
            match m.a {
                OPT_NODELAY => s.nodelay = on,
                OPT_KEEPALIVE => s.keepalive = on,
                OPT_REUSEADDR => s.reuse = on,
                _ => {}
            }
            if let (Some(i), Some(h)) = (li, s.tcp) {
                let t = n.links[i].sockets.get_mut::<tcp::Socket>(h);
                t.set_nagle_enabled(!s.nodelay);
                t.set_keep_alive(s.keepalive.then(|| Duration::from_secs(75)));
            }
        }
        _ => {
            if let Some(h) = h {
                rx::close(h);
            }
        }
    }
}

fn bind(n: &mut Netd, si: usize, who: &rx::Sender, addr: IpAddress, port: u16) -> Result<(IpAddress, u16), i32> {
    if !n.socks[si].v6 && matches!(addr, IpAddress::Ipv6(_)) {
        return Err(EAFNOSUPPORT);
    }
    if !addr.is_unspecified() && !n.is_local(addr) {
        return Err(EADDRNOTAVAIL);
    }
    let stream = n.socks[si].stream;
    let id = n.socks[si].id;
    let reuse = n.socks[si].reuse;
    let port = if port == 0 {
        ephemeral(n, stream, addr).ok_or(EADDRINUSE)?
    } else {
        if port < 1024 && who.euid != 0 {
            return Err(EACCES);
        }
        if port_used(n, stream, addr, port, id, reuse) {
            return Err(EADDRINUSE);
        }
        port
    };
    let s = &mut n.socks[si];
    s.local = Some((addr, port));
    s.state = State::Bound;
    if !stream {
        for i in 0..n.links.len() {
            if link_serves(n, i, addr) {
                add_udp(n, si, i);
            }
        }
    }
    Ok((addr, port))
}

fn connect_tcp(n: &mut Netd, si: usize, who: &rx::Sender, addr: IpAddress, port: u16, data: rx::Handle) -> Result<(), i32> {
    if port == 0 || addr.is_unspecified() {
        return Err(EADDRNOTAVAIL);
    }
    if !n.socks[si].v6 && matches!(addr, IpAddress::Ipv6(_)) {
        return Err(EAFNOSUPPORT);
    }
    let (i, mut src) = n.route(addr).ok_or(ENETUNREACH)?;
    if !n.links[i].usable() {
        return Err(ENETUNREACH);
    }
    let local = match n.socks[si].local {
        Some(l) => l,
        None => bind(n, si, who, any(n.socks[si].v6), 0)?,
    };
    if !local.0.is_unspecified() {
        src = local.0;
    }
    let s = &n.socks[si];
    let mut t = tcp_socket();
    t.set_nagle_enabled(!s.nodelay);
    t.set_keep_alive(s.keepalive.then(|| Duration::from_secs(75)));
    let l = &mut n.links[i];
    t.connect(l.iface.context(), IpEndpoint::new(addr, port), IpListenEndpoint { addr: Some(src), port: local.1 })
        .map_err(|_| EADDRNOTAVAIL)?;
    let h = l.sockets.add(t);
    let link = l.id;
    let s = &mut n.socks[si];
    s.local = Some((src, local.1));
    s.peer = Some((addr, port));
    s.tcp = Some(h);
    s.link = link;
    s.data = data;
    s.state = State::Connecting;
    s.deadline = Some(now() + Duration::from_secs(CONNECT_TIMEOUT));
    Ok(())
}

fn send_udp(n: &mut Netd, si: usize, dst: (IpAddress, u16), data: &[u8]) {
    let Some((i, src)) = n.route(dst.0) else { return };
    let local = n.socks[si].local.unwrap();
    let src = if local.0.is_unspecified() { src } else { local.0 };
    let id = n.links[i].id;
    if !n.socks[si].per_link.iter().any(|(l, _)| *l == id) {
        add_udp(n, si, i);
    }
    let Some(&(_, h)) = n.socks[si].per_link.iter().find(|(l, _)| *l == id) else { return };
    let u = n.links[i].sockets.get_mut::<udp::Socket>(h);
    let meta = udp::UdpMetadata {
        endpoint: IpEndpoint::new(dst.0, dst.1),
        local_address: Some(src),
        meta: Default::default(),
    };
    let _ = u.send_slice(data, meta); // full: dropped, as UDP may
}

pub fn data_ready(n: &mut Netd, id: u64, pending: u32) {
    if let Some(si) = index(n, id) {
        if pending & (rx::WRITABLE | rx::PEER_CLOSED) != 0 {
            n.socks[si].data_full = false;
        }
    }
}

fn log_sock(n: &Netd, si: usize, what: &str) {
    let _ = (n, si, what);
}

// ---- after the stack has run: connections, accepts, data, datagrams ----

/// Move what can move; true if something did
pub fn service(n: &mut Netd) -> bool {
    let mut busy = false;
    let t = now();
    let mut si = 0;
    while si < n.socks.len() {
        let s = &n.socks[si];
        match (s.stream, s.state) {
            (true, State::Listening) => busy |= accept(n, si),
            (true, State::Connecting) => connecting(n, si, t),
            (true, State::Connected) => busy |= pump(n, si),
            (false, State::Bound | State::Connected) => busy |= datagrams(n, si),
            _ => {}
        }
        if n.socks[si].remove {
            let id = n.socks[si].id;
            drop_sock(n, id);
            continue;
        }
        si += 1;
    }
    busy
}

pub fn next_deadline(n: &Netd) -> Option<Instant> {
    n.socks.iter().filter_map(|s| s.deadline).min()
}

fn accept(n: &mut Netd, si: usize) -> bool {
    let mut busy = false;
    let mut k = 0;
    while k < n.socks[si].per_link.len() {
        let (lid, h) = n.socks[si].per_link[k];
        let Some(i) = n.link_index(lid) else {
            k += 1;
            continue;
        };
        let t = n.links[i].sockets.get_mut::<tcp::Socket>(h);
        match t.state() {
            tcp::State::Listen | tcp::State::SynReceived => {
                k += 1;
                continue;
            }
            tcp::State::Closed => {
                // A handshake that failed: listen again
                let local = n.socks[si].local.unwrap();
                let _ = t.listen(listen_endpoint(local));
                k += 1;
                continue;
            }
            _ => {}
        }
        // Established (or already closing): a connection for the program
        let peer = t.remote_endpoint();
        let local = t.local_endpoint();
        let lst = n.socks[si].ctl;
        n.socks[si].per_link.remove(k);
        if !n.socks[si].v6 && matches!(peer, Some(IpEndpoint { addr: IpAddress::Ipv6(_), .. })) {
            // An IPv6 connection to an IPv4 socket's port (smoltcp listened
            // on both): refused
            n.links[i].sockets.get_mut::<tcp::Socket>(h).abort();
            add_listeners(n, si, i);
            continue;
        }
        let pair = rx::socket_create();
        let chans = rx::channel_create();
        let (Some((data_ours, data_theirs)), Some((ctl_ours, ctl_theirs))) = (pair, chans) else {
            n.links[i].sockets.get_mut::<tcp::Socket>(h).abort();
            continue;
        };
        let ep = |e: Option<IpEndpoint>| e.map(|e| (e.addr, e.port));
        let (peer, local) = (ep(peer), ep(local));
        let msg = Msg { op: ACCEPTED, status: 0, a: 0, b: 0, addr: peer, addr2: local };
        if !send(lst, msg, &[], &[data_theirs, ctl_theirs]) {
            // Nobody to take it (the program's queue is full): refuse it
            n.links[i].sockets.get_mut::<tcp::Socket>(h).abort();
            rx::close(data_ours);
            rx::close(ctl_ours);
            rx::close(data_theirs);
            rx::close(ctl_theirs);
        } else {
            let id = n.next_sock;
            n.next_sock += 1;
            let mut c = new_sock(id, ctl_ours);
            c.state = State::Connected;
            c.local = local;
            c.peer = peer;
            c.tcp = Some(h);
            c.link = lid;
            c.data = data_ours;
            c.nodelay = n.socks[si].nodelay;
            n.socks.push(c);
        }
        // Another listening socket in its place
        add_listeners(n, si, i);
        busy = true;
    }
    busy
}

fn connecting(n: &mut Netd, si: usize, now: Instant) {
    let s = &n.socks[si];
    let (Some(i), Some(h)) = (n.link_index(s.link), s.tcp) else { return };
    let t = n.links[i].sockets.get_mut::<tcp::Socket>(h);
    let st = t.state();
    let timed_out = s.deadline.is_some_and(|d| now >= d);
    if st == tcp::State::SynSent && !timed_out {
        return;
    }
    let ctl = s.ctl;
    if matches!(st, tcp::State::Established | tcp::State::CloseWait) {
        let s = &mut n.socks[si];
        s.state = State::Connected;
        s.deadline = None;
        let (local, peer) = (s.local, s.peer);
        send(ctl, Msg { op: CONNECT, status: 0, a: 0, b: 0, addr: local, addr2: peer }, &[], &[]);
        return;
    }
    // Refused (RST) or no answer
    t.abort();
    let err = if timed_out { ETIMEDOUT } else { ECONNREFUSED };
    n.links[i].sockets.remove(h);
    let s = &mut n.socks[si];
    s.tcp = None;
    s.deadline = None;
    rx::close(s.data);
    s.data = 0;
    s.state = State::Bound;
    answer(ctl, CONNECT, err, None);
}

// Bytes between the data socket and smoltcp's TCP socket
fn pump(n: &mut Netd, si: usize) -> bool {
    let (lid, h, data) = {
        let s = &n.socks[si];
        (s.link, s.tcp, s.data)
    };
    let (Some(i), Some(h)) = (n.link_index(lid), h) else {
        n.socks[si].remove = true;
        return false;
    };
    let mut busy = false;
    let s = &mut n.socks[si];
    let t = n.links[i].sockets.get_mut::<tcp::Socket>(h);
    // The peer's bytes, to the program
    while !s.to_prog_done && t.can_recv() {
        let r = t.recv(|b| match rx::socket_write(data, b) {
            Ok(k) => (k, Ok(k)),
            Err(e) => (0, Err(e)),
        });
        match r {
            Ok(Ok(k)) if k > 0 => busy = true,
            Ok(Err(rx::ERR_SHOULD_WAIT)) => {
                s.data_full = true;
                break;
            }
            Ok(Err(_)) => {
                // The program is gone: nowhere to put them
                s.to_prog_done = true;
                break;
            }
            _ => break,
        }
    }
    if s.to_prog_done && s.from_prog_done {
        // Discard what still comes, so the window stays open until the end
        while t.can_recv() {
            let _ = t.recv(|b| (b.len(), ()));
        }
    }
    // The peer's FIN: end-of-stream for the program
    if !s.to_prog_done && !t.may_recv() && !t.can_recv() && !matches!(t.state(), tcp::State::SynSent | tcp::State::SynReceived) {
        rx::socket_shutdown_write(data);
        s.to_prog_done = true;
        busy = true;
    }
    // The program's bytes, to the peer
    while !s.from_prog_done && t.can_send() {
        let r = t.send(|b| match rx::socket_read(data, b) {
            Ok(k) => (k, Ok(k)),
            Err(e) => (0, Err(e)),
        });
        match r {
            Ok(Ok(k)) if k > 0 => busy = true,
            Ok(Err(rx::ERR_SHOULD_WAIT)) => break,
            Ok(Err(_)) => {
                // It shut down writing, or closed the socket: FIN
                s.from_prog_done = true;
                t.close();
                busy = true;
                break;
            }
            _ => break,
        }
    }
    if s.from_prog_done && s.ctl == 0 && t.state() != tcp::State::Closed && t.state() != tcp::State::TimeWait {
        // Closed by the program: give the peer a minute to finish
        if s.deadline.is_none() {
            s.deadline = Some(now() + Duration::from_secs(60));
        }
        if s.deadline.is_some_and(|d| now() >= d) {
            t.abort();
        }
    }
    match t.state() {
        tcp::State::Closed | tcp::State::TimeWait => {
            if !s.from_prog_done && t.state() == tcp::State::Closed {
                // Reset by the peer (or timed out)
                send(s.ctl, Msg { op: ERROR, status: -ECONNRESET, a: 0, b: 0, addr: None, addr2: None }, &[], &[]);
                if !s.to_prog_done {
                    rx::socket_shutdown_write(data);
                    s.to_prog_done = true;
                }
                s.remove = true;
            } else if s.from_prog_done && (s.to_prog_done || s.ctl == 0) {
                s.remove = true;
            }
        }
        _ => {}
    }
    busy
}

fn datagrams(n: &mut Netd, si: usize) -> bool {
    let mut busy = false;
    let ctl = n.socks[si].ctl;
    let peer = n.socks[si].peer;
    for k in 0..n.socks[si].per_link.len() {
        let (lid, h) = n.socks[si].per_link[k];
        let Some(i) = n.link_index(lid) else { continue };
        let u = n.links[i].sockets.get_mut::<udp::Socket>(h);
        while u.can_recv() {
            let Ok((payload, meta)) = u.recv() else { break };
            let from = meta.endpoint.addr;
            if !n.socks[si].v6 && matches!(from, IpAddress::Ipv6(_)) {
                continue; // an IPv4 socket gets IPv4 datagrams only
            }
            // A connected UDP socket takes its peer's datagrams only
            if let Some((pa, pp)) = peer {
                if pa != from || pp != meta.endpoint.port {
                    continue;
                }
            }
            let to = meta.local_address.map(|a| (a, 0));
            send(ctl, Msg { op: DGRAM, status: 0, a: 0, b: 0, addr: Some((from, meta.endpoint.port)), addr2: to }, payload, &[]);
            busy = true;
        }
    }
    busy
}

/// The `sockets` request (`ss`): PROTO STATE LOCAL PEER QUEUES
pub fn list(n: &Netd) -> String {
    let mut out = String::new();
    let ep = |e: Option<(IpAddress, u16)>| match e {
        Some((IpAddress::Ipv6(a), p)) => format!("[{a}]:{p}"),
        Some((a, p)) => format!("{}:{}", if a.is_unspecified() { "0.0.0.0".to_string() } else { a.to_string() }, p),
        None => "*:*".into(),
    };
    for s in &n.socks {
        let proto = if s.stream { "tcp" } else { "udp" };
        let mut state = match s.state {
            State::New | State::Open => "UNCONN".to_string(),
            State::Bound => "UNCONN".to_string(),
            State::Listening => "LISTEN".to_string(),
            State::Connecting => "SYN-SENT".to_string(),
            State::Connected => if s.stream { "ESTAB".to_string() } else { "UNCONN".to_string() },
        };
        let (mut rq, mut sq) = (0, 0);
        if let (Some(i), Some(h)) = (n.link_index(s.link), s.tcp) {
            let t = n.links[i].sockets.get::<tcp::Socket>(h);
            rq = t.recv_queue();
            sq = t.send_queue();
            state = match t.state() {
                tcp::State::Established => "ESTAB",
                tcp::State::SynSent => "SYN-SENT",
                tcp::State::SynReceived => "SYN-RECV",
                tcp::State::FinWait1 => "FIN-WAIT-1",
                tcp::State::FinWait2 => "FIN-WAIT-2",
                tcp::State::CloseWait => "CLOSE-WAIT",
                tcp::State::Closing => "CLOSING",
                tcp::State::LastAck => "LAST-ACK",
                tcp::State::TimeWait => "TIME-WAIT",
                tcp::State::Closed => "CLOSED",
                tcp::State::Listen => "LISTEN",
            }
            .to_string();
        }
        if matches!(s.state, State::New | State::Open) && s.local.is_none() {
            continue;
        }
        let peer = if s.state == State::Listening || (!s.stream && s.peer.is_none()) { "0.0.0.0:*".to_string() } else { ep(s.peer) };
        out.push_str(&format!("{proto} {state} {rq} {sq} {} {peer}\n", ep(s.local)));
    }
    let _ = log;
    out
}
