// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! netd: Rustica's network service (NETWORK.md, docs/NETWORK.md). The
//! TCP/IP stack (smoltcp) for every interface: network card drivers send
//! and receive Ethernet frames through /svc/netdev (<rustux/netdev.h>);
//! programs reach the network through /svc/net.
//!
//! Step D1: interfaces (Ethernet, ARP, IPv4, ICMP: it answers pings),
//! static addresses and routes from /etc/network.conf, and a text control
//! protocol on /svc/net for `ip` and `ping`: one request a message, the
//! answer a message, its last line "ok" or "error: ...".
//!
//!   links | addrs | routes                    (anyone)
//!   addr add|del CIDR dev IF                  (root)
//!   route add|del default via GATEWAY         (root)
//!   link set IF up|down                       (root)
//!   ping ADDRESS SEQ TIMEOUT_MS SIZE          -> "reply FROM SEQ BYTES MICROSECONDS" or "timeout"

mod rx;

use smoltcp::iface::{Config, Interface, SocketHandle, SocketSet};
use smoltcp::phy::{ChecksumCapabilities, Device, DeviceCapabilities, Medium, RxToken, TxToken};
use smoltcp::socket::icmp;
use smoltcp::time::Instant;
use smoltcp::wire::{EthernetAddress, HardwareAddress, Icmpv4Packet, Icmpv4Repr, IpAddress, IpCidr, Ipv4Address, Ipv4Cidr};
use std::collections::VecDeque;
use std::io::Write;
use std::str::FromStr;

const NETDEV_HELLO: u32 = 1;
const NETDEV_FRAME: u32 = 2;
const NETDEV_LINK: u32 = 3;
const PING_IDENT: u16 = 0x5258;

fn log(s: &str) {
    println!("netd: {s}");
    let _ = std::io::stdout().flush();
}

fn mac_str(m: &EthernetAddress) -> String {
    let b = m.0;
    format!("{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}", b[0], b[1], b[2], b[3], b[4], b[5])
}

fn now() -> Instant {
    Instant::from_micros((rx::monotonic() / 1000) as i64)
}

// ---- frames to and from a driver ----

struct ChanDev {
    ch: rx::Handle,
    rxq: VecDeque<Vec<u8>>,
    rx_frames: u64,
    tx_frames: u64,
    rx_bytes: u64,
    tx_bytes: u64,
    up: bool,
}

struct RxT(Vec<u8>);
struct TxT<'a> {
    dev: &'a mut ChanDev,
}

impl RxToken for RxT {
    fn consume<R, F: FnOnce(&[u8]) -> R>(self, f: F) -> R {
        f(&self.0)
    }
}

impl<'a> TxToken for TxT<'a> {
    fn consume<R, F: FnOnce(&mut [u8]) -> R>(self, len: usize, f: F) -> R {
        let mut buf = vec![0u8; 8 + len];
        buf[..4].copy_from_slice(&NETDEV_FRAME.to_le_bytes());
        let r = f(&mut buf[8..]);
        if self.dev.up {
            rx::channel_write(self.dev.ch, &buf, &[]);
            self.dev.tx_frames += 1;
            self.dev.tx_bytes += len as u64;
        }
        r
    }
}

impl Device for ChanDev {
    type RxToken<'a> = RxT;
    type TxToken<'a> = TxT<'a>;

    fn receive(&mut self, _t: Instant) -> Option<(RxT, TxT<'_>)> {
        let f = self.rxq.pop_front()?;
        Some((RxT(f), TxT { dev: self }))
    }

    fn transmit(&mut self, _t: Instant) -> Option<TxT<'_>> {
        Some(TxT { dev: self })
    }

    fn capabilities(&self) -> DeviceCapabilities {
        let mut c = DeviceCapabilities::default();
        c.medium = Medium::Ethernet;
        c.max_transmission_unit = 1514;
        c.max_burst_size = Some(32);
        c
    }
}

// ---- interfaces ----

struct Link {
    name: String,
    driver: String,
    mac: EthernetAddress,
    carrier: bool, // the driver's link
    admin_up: bool,
    dev: ChanDev,
    iface: Interface,
    sockets: SocketSet<'static>,
    icmp: SocketHandle,
    gateway: Option<Ipv4Address>,
}

/// /etc/network.conf: "NAME static ADDRESS/PREFIX [GATEWAY]" or "NAME dhcp"
fn configure(link: &mut Link) {
    let text = std::fs::read_to_string("/etc/network.conf").unwrap_or_default();
    for line in text.lines() {
        let w: Vec<&str> = line.split('#').next().unwrap_or("").split_whitespace().collect();
        if w.len() < 2 || w[0] != link.name {
            continue;
        }
        match w[1] {
            "static" => {
                if let Some(c) = w.get(2).and_then(|s| Ipv4Cidr::from_str(s).ok()) {
                    link.iface.update_ip_addrs(|a| {
                        let _ = a.push(IpCidr::Ipv4(c));
                    });
                    log(&format!("{}: address {}", link.name, c));
                }
                if let Some(g) = w.get(3).and_then(|s| Ipv4Address::from_str(s).ok()) {
                    if link.iface.routes_mut().add_default_ipv4_route(g).is_ok() {
                        link.gateway = Some(g);
                        log(&format!("{}: default route via {}", link.name, g));
                    }
                }
            }
            "dhcp" => log(&format!("{}: DHCP comes with step D3; no address", link.name)),
            other => log(&format!("{}: unknown method {other} in /etc/network.conf", link.name)),
        }
    }
}

fn new_link(name: String, ch: rx::Handle, hello: &[u8]) -> Link {
    let mut mac = [0u8; 6];
    mac.copy_from_slice(&hello[..6]);
    let link_up = u16::from_le_bytes([hello[6], hello[7]]) != 0;
    let driver = String::from_utf8_lossy(&hello[8..32]).trim_end_matches('\0').to_string();
    let mut dev = ChanDev { ch, rxq: VecDeque::new(), rx_frames: 0, tx_frames: 0, rx_bytes: 0, tx_bytes: 0, up: true };
    let mut config = Config::new(HardwareAddress::Ethernet(EthernetAddress(mac)));
    config.random_seed = rx::monotonic() ^ (u64::from_le_bytes([mac[0], mac[1], mac[2], mac[3], mac[4], mac[5], 0, 0]));
    let iface = Interface::new(config, &mut dev, now());
    let mut sockets = SocketSet::new(vec![]);
    let icmp_s = icmp::Socket::new(
        icmp::PacketBuffer::new(vec![icmp::PacketMetadata::EMPTY; 16], vec![0; 16 * 1100]),
        icmp::PacketBuffer::new(vec![icmp::PacketMetadata::EMPTY; 16], vec![0; 16 * 1100]),
    );
    let icmp = sockets.add(icmp_s);
    sockets.get_mut::<icmp::Socket>(icmp).bind(icmp::Endpoint::Ident(PING_IDENT)).ok();
    let mut l = Link {
        name,
        driver,
        mac: EthernetAddress(mac),
        carrier: link_up,
        admin_up: true,
        dev,
        iface,
        sockets,
        icmp,
        gateway: None,
    };
    configure(&mut l);
    l
}

// ---- pings in flight ----

struct Ping {
    client: rx::Handle,
    link: usize,
    seq: u16,       // ours, on the wire
    client_seq: u32,
    sent: Instant,
    deadline: Instant,
    bytes: usize,
}

struct Netd {
    links: Vec<Link>,
    pending_drivers: Vec<rx::Handle>,
    clients: Vec<rx::Handle>,
    pings: Vec<Ping>,
    next_seq: u16,
    next_eth: usize,
}

fn reply(ch: rx::Handle, text: &str) {
    rx::channel_write(ch, text.as_bytes(), &[]);
}

impl Netd {
    // A card's name: ethN, N its place among the network controllers in
    // PCI order (the driver names its slot: "virtio-net 00:03.0"), so the
    // names do not depend on which driver started first; else the next
    // name free after theirs
    fn name_for(&mut self, hello: &[u8]) -> String {
        let driver = String::from_utf8_lossy(&hello[8..32]).trim_end_matches('\0').to_string();
        let taken = |n: &str, links: &[Link]| links.iter().any(|l| l.name == n);
        let slots = rx::network_pci_slots();
        if let Some(slot) = driver.split_whitespace().last() {
            if let Some(k) = slots.iter().position(|s| s == slot) {
                let name = format!("eth{k}");
                if !taken(&name, &self.links) {
                    return name;
                }
            }
        }
        // After the PCI cards' names (USB adapters, later)
        self.next_eth = self.next_eth.max(slots.len());
        loop {
            let name = format!("eth{}", self.next_eth);
            self.next_eth += 1;
            if !taken(&name, &self.links) {
                return name;
            }
        }
    }

    fn link_by_name(&mut self, n: &str) -> Option<usize> {
        self.links.iter().position(|l| l.name == n)
    }

    // The interface for an address: the one whose subnet holds it, else
    // the one with a default route
    fn link_for(&self, a: Ipv4Address) -> Option<usize> {
        for (i, l) in self.links.iter().enumerate() {
            for c in l.iface.ip_addrs() {
                if let IpCidr::Ipv4(c) = c {
                    if c.contains_addr(&a) {
                        return Some(i);
                    }
                }
            }
        }
        self.links.iter().position(|l| l.gateway.is_some())
    }

    // The interfaces in name order (eth0, eth1, ..., eth10)
    fn sorted(&self) -> Vec<&Link> {
        let mut v: Vec<&Link> = self.links.iter().collect();
        v.sort_by_key(|l| (l.name.len(), l.name.clone()));
        v
    }

    fn command(&mut self, ch: rx::Handle, who: &rx::Sender, text: &str) {
        let w: Vec<&str> = text.split_whitespace().collect();
        let root = who.euid == 0;
        let mut out = String::new();
        let res: Result<(), String> = (|| {
            match w.as_slice() {
                ["links"] => {
                    out.push_str("lo loopback 00:00:00:00:00:00 up mtu 65536 rx 0 tx 0 0 0\n");
                    for l in self.sorted() {
                        out.push_str(&format!(
                            "{} {} {} {} mtu 1500 rx {} tx {} {} {}\n",
                            l.name,
                            l.driver.replace(' ', "_"),
                            mac_str(&l.mac),
                            if !l.admin_up { "down" } else if l.carrier { "up" } else { "no-carrier" },
                            l.dev.rx_frames,
                            l.dev.tx_frames,
                            l.dev.rx_bytes,
                            l.dev.tx_bytes
                        ));
                    }
                }
                ["addrs"] => {
                    out.push_str("lo 127.0.0.1/8\n");
                    for l in self.sorted() {
                        for c in l.iface.ip_addrs() {
                            out.push_str(&format!("{} {}\n", l.name, c));
                        }
                    }
                }
                ["routes"] => {
                    for l in self.sorted() {
                        if let Some(g) = l.gateway {
                            out.push_str(&format!("default via {} dev {}\n", g, l.name));
                        }
                        for c in l.iface.ip_addrs() {
                            if let IpCidr::Ipv4(c) = c {
                                out.push_str(&format!("{} dev {} src {}\n", c.network(), l.name, c.address()));
                            }
                        }
                    }
                }
                ["addr", op @ ("add" | "del"), cidr, "dev", ifname] => {
                    if !root {
                        return Err("only root changes addresses".into());
                    }
                    let c = Ipv4Cidr::from_str(cidr).map_err(|_| format!("{cidr}: not an address/prefix"))?;
                    let i = self.link_by_name(ifname).ok_or(format!("{ifname}: no such interface"))?;
                    let l = &mut self.links[i];
                    let mut err = None;
                    l.iface.update_ip_addrs(|a| {
                        if *op == "add" {
                            if a.iter().any(|x| *x == IpCidr::Ipv4(c)) {
                                err = Some("the address is there already".to_string());
                            } else if a.push(IpCidr::Ipv4(c)).is_err() {
                                err = Some("too many addresses".to_string());
                            }
                        } else if let Some(p) = a.iter().position(|x| *x == IpCidr::Ipv4(c)) {
                            a.remove(p);
                        } else {
                            err = Some("no such address".to_string());
                        }
                    });
                    if let Some(e) = err {
                        return Err(e);
                    }
                    log(&format!("{}: address {} {}", l.name, if *op == "add" { "added" } else { "removed" }, c));
                }
                ["route", "add", "default", "via", gw] => {
                    if !root {
                        return Err("only root changes routes".into());
                    }
                    let g = Ipv4Address::from_str(gw).map_err(|_| format!("{gw}: not an address"))?;
                    let i = self.link_for(g).ok_or("no interface reaches it")?;
                    for l in self.links.iter_mut() {
                        if l.gateway.take().is_some() {
                            l.iface.routes_mut().remove_default_ipv4_route();
                        }
                    }
                    let l = &mut self.links[i];
                    l.iface.routes_mut().add_default_ipv4_route(g).map_err(|_| "too many routes".to_string())?;
                    l.gateway = Some(g);
                    log(&format!("{}: default route via {}", l.name, g));
                }
                ["route", "del", "default", ..] => {
                    if !root {
                        return Err("only root changes routes".into());
                    }
                    let mut any = false;
                    for l in self.links.iter_mut() {
                        if l.gateway.take().is_some() {
                            l.iface.routes_mut().remove_default_ipv4_route();
                            any = true;
                        }
                    }
                    if !any {
                        return Err("there is no default route".into());
                    }
                }
                ["link", "set", ifname, state @ ("up" | "down")] => {
                    if !root {
                        return Err("only root changes interfaces".into());
                    }
                    let i = self.link_by_name(ifname).ok_or(format!("{ifname}: no such interface"))?;
                    let l = &mut self.links[i];
                    l.admin_up = *state == "up";
                    l.dev.up = l.admin_up;
                    log(&format!("{}: {}", l.name, state));
                }
                ["ping", addr, seq, timeout, size] => {
                    let a = Ipv4Address::from_str(addr).map_err(|_| format!("{addr}: not an IPv4 address"))?;
                    let cseq: u32 = seq.parse().map_err(|_| "bad sequence number")?;
                    let tmo: u64 = timeout.parse().map_err(|_| "bad time-out")?;
                    let size: usize = size.parse::<usize>().map_err(|_| "bad size")?.min(1000);
                    // Loopback and our own addresses answer at once
                    let local = a.octets()[0] == 127
                        || self.links.iter().any(|l| l.iface.ip_addrs().iter().any(|c| c.address() == IpAddress::Ipv4(a)));
                    if local {
                        reply(ch, &format!("reply {} {} {} {}\n", a, cseq, size, 5));
                        return Ok(());
                    }
                    let i = self.link_for(a).ok_or("network unreachable")?;
                    let t = now();
                    let seq_no = self.next_seq;
                    self.next_seq = self.next_seq.wrapping_add(1);
                    let l = &mut self.links[i];
                    if !l.admin_up || !l.carrier {
                        return Err(format!("{}: the link is down", l.name));
                    }
                    let mut data = vec![0u8; size];
                    for (k, b) in data.iter_mut().enumerate() {
                        *b = k as u8;
                    }
                    let repr = Icmpv4Repr::EchoRequest { ident: PING_IDENT, seq_no, data: &data };
                    let s = l.sockets.get_mut::<icmp::Socket>(l.icmp);
                    let buf = s.send(repr.buffer_len(), IpAddress::Ipv4(a)).map_err(|_| "too many pings at once")?;
                    let mut pkt = Icmpv4Packet::new_unchecked(buf);
                    repr.emit(&mut pkt, &ChecksumCapabilities::default());
                    self.pings.push(Ping {
                        client: ch,
                        link: i,
                        seq: seq_no,
                        client_seq: cseq,
                        sent: t,
                        deadline: t + smoltcp::time::Duration::from_millis(tmo),
                        bytes: size,
                    });
                    return Ok(()); // answered when the reply comes, or the time is up
                }
                _ => return Err(format!("unknown request: {}", text.trim())),
            }
            out.push_str("ok\n");
            reply(ch, &out);
            Ok(())
        })();
        if let Err(e) = res {
            reply(ch, &format!("error: {e}\n"));
        }
    }

    // Echo replies and pings whose time is up
    fn pings(&mut self) {
        let t = now();
        for (li, l) in self.links.iter_mut().enumerate() {
            let s = l.sockets.get_mut::<icmp::Socket>(l.icmp);
            while s.can_recv() {
                let Ok((payload, from)) = s.recv() else { break };
                let Ok(pkt) = Icmpv4Packet::new_checked(payload) else { continue };
                let Ok(Icmpv4Repr::EchoReply { ident, seq_no, data }) = Icmpv4Repr::parse(&pkt, &ChecksumCapabilities::default()) else {
                    continue;
                };
                if ident != PING_IDENT {
                    continue;
                }
                if let Some(p) = self.pings.iter().position(|p| p.link == li && p.seq == seq_no) {
                    let p = self.pings.remove(p);
                    let us = (t - p.sent).total_micros();
                    reply(p.client, &format!("reply {} {} {} {}\n", from, p.client_seq, data.len(), us));
                    let _ = p.bytes;
                }
            }
        }
        self.pings.retain(|p| {
            if t >= p.deadline {
                reply(p.client, "timeout\n");
                false
            } else {
                true
            }
        });
    }

    fn driver_message(&mut self, i: usize, buf: &[u8]) {
        if buf.len() < 8 {
            return;
        }
        let op = u32::from_le_bytes(buf[..4].try_into().unwrap());
        let a = u32::from_le_bytes(buf[4..8].try_into().unwrap());
        let l = &mut self.links[i];
        match op {
            NETDEV_FRAME => {
                if l.admin_up && l.dev.rxq.len() < 256 {
                    l.dev.rx_frames += 1;
                    l.dev.rx_bytes += (buf.len() - 8) as u64;
                    l.dev.rxq.push_back(buf[8..].to_vec());
                }
            }
            NETDEV_LINK => {
                l.carrier = a != 0;
                log(&format!("{}: link {}", l.name, if l.carrier { "up" } else { "down" }));
            }
            _ => {}
        }
    }
}

fn main() {
    let (Some(svc), Some(netdev)) = (rx::publish("/svc/net"), rx::publish("/svc/netdev")) else {
        eprintln!("netd: cannot publish /svc/net and /svc/netdev");
        std::process::exit(3);
    };
    log("started (/svc/net, /svc/netdev)");
    let mut n = Netd { links: Vec::new(), pending_drivers: Vec::new(), clients: Vec::new(), pings: Vec::new(), next_seq: 1, next_eth: 0 };
    let mut buf = vec![0u8; 65536];
    loop {
        let t = now();
        for l in n.links.iter_mut() {
            l.iface.poll(t, &mut l.dev, &mut l.sockets);
        }
        n.pings();
        // How long until something is due
        let mut deadline = rx::TIME_INFINITE;
        let mono = rx::monotonic();
        for l in n.links.iter_mut() {
            if let Some(d) = l.iface.poll_delay(t, &l.sockets) {
                deadline = deadline.min(mono + d.total_micros() * 1000);
            }
            if !l.dev.rxq.is_empty() {
                deadline = mono;
            }
        }
        for p in &n.pings {
            deadline = deadline.min(mono + ((p.deadline - t).total_micros().max(0) * 1000));
        }
        // Wait for a frame, a request or a connection
        let mut items = vec![
            rx::WaitItem { handle: svc, waitfor: rx::CHANNEL_READABLE, pending: 0 },
            rx::WaitItem { handle: netdev, waitfor: rx::CHANNEL_READABLE, pending: 0 },
        ];
        for l in &n.links {
            items.push(rx::WaitItem { handle: l.dev.ch, waitfor: rx::CHANNEL_READABLE | rx::PEER_CLOSED, pending: 0 });
        }
        for &d in &n.pending_drivers {
            items.push(rx::WaitItem { handle: d, waitfor: rx::CHANNEL_READABLE | rx::PEER_CLOSED, pending: 0 });
        }
        for &c in &n.clients {
            items.push(rx::WaitItem { handle: c, waitfor: rx::CHANNEL_READABLE | rx::PEER_CLOSED, pending: 0 });
        }
        rx::wait_many(&mut items, deadline);
        let (nl, nd) = (n.links.len(), n.pending_drivers.len());
        // Frames from the drivers (a few at a time each, so one busy card
        // does not starve the rest)
        let mut gone_links = Vec::new();
        for i in 0..nl {
            if items[2 + i].pending == 0 {
                continue;
            }
            let ch = n.links[i].dev.ch;
            let mut k = 0;
            loop {
                match rx::channel_read(ch, &mut buf, None) {
                    Ok((len, h)) => {
                        if let Some(h) = h {
                            rx::close(h);
                        }
                        n.driver_message(i, &buf[..len]);
                        k += 1;
                        if k == 64 {
                            break;
                        }
                    }
                    Err(rx::ERR_SHOULD_WAIT) => break,
                    Err(_) => {
                        gone_links.push(i);
                        break;
                    }
                }
            }
        }
        // Drivers saying who they are
        let mut keep = Vec::new();
        for (j, &d) in n.pending_drivers.clone().iter().enumerate() {
            if items[2 + nl + j].pending == 0 {
                keep.push(d);
                continue;
            }
            match rx::channel_read(d, &mut buf, None) {
                Ok((len, _)) if len >= 8 + 32 && u32::from_le_bytes(buf[..4].try_into().unwrap()) == NETDEV_HELLO => {
                    let name = n.name_for(&buf[8..len]);
                    let l = new_link(name, d, &buf[8..len]);
                    log(&format!("{}: {} ({}), link {}", l.name, mac_str(&l.mac), l.driver, if l.carrier { "up" } else { "down" }));
                    n.links.push(l);
                }
                Err(rx::ERR_SHOULD_WAIT) => keep.push(d),
                Ok(_) => keep.push(d),
                Err(_) => rx::close(d),
            }
        }
        n.pending_drivers = keep;
        // Requests
        let mut gone_clients = Vec::new();
        for (j, &c) in n.clients.clone().iter().enumerate() {
            if items[2 + nl + nd + j].pending == 0 {
                continue;
            }
            let mut who = rx::Sender::default();
            loop {
                match rx::channel_read(c, &mut buf, Some(&mut who)) {
                    Ok((len, h)) => {
                        if let Some(h) = h {
                            rx::close(h);
                        }
                        let text = String::from_utf8_lossy(&buf[..len]).into_owned();
                        n.command(c, &who, &text);
                    }
                    Err(rx::ERR_SHOULD_WAIT) => break,
                    Err(_) => {
                        gone_clients.push(c);
                        break;
                    }
                }
            }
        }
        for c in gone_clients {
            n.clients.retain(|&x| x != c);
            n.pings.retain(|p| p.client != c);
            rx::close(c);
        }
        for i in gone_links.into_iter().rev() {
            let l = n.links.remove(i);
            log(&format!("{}: the driver is gone", l.name));
            n.pings.retain(|p| p.link != i);
            for p in n.pings.iter_mut() {
                if p.link > i {
                    p.link -= 1;
                }
            }
            rx::close(l.dev.ch);
        }
        if items[0].pending != 0 {
            n.clients.extend(rx::accept_all(svc));
        }
        if items[1].pending != 0 {
            n.pending_drivers.extend(rx::accept_all(netdev));
        }
    }
}
