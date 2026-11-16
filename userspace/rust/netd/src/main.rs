// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! netd: Rustica's network service (NETWORK.md, docs/NETWORK.md). The
//! TCP/IP stack (smoltcp) for every interface: network card drivers send
//! and receive Ethernet frames through /svc/netdev (<rustux/netdev.h>);
//! programs reach the network through /svc/net (control: `ip`, `ping`)
//! and /svc/netsock (a connection per socket, <rustux/netsock.h>, sock.rs).
//!
//! /svc/net is a text protocol: one request a message, the answer a
//! message, its last line "ok" or "error: ...".
//!
//!   links | addrs | routes                    (anyone)
//!   addr add|del CIDR dev IF                  (root)
//!   route add|del default via GATEWAY         (root)
//!   link set IF up|down                       (root)
//!   ping ADDRESS SEQ TIMEOUT_MS SIZE          -> "reply FROM SEQ BYTES MICROSECONDS" or "timeout"
//!   sockets                                   (anyone: what `ss` shows)
//!   dns                                       (anyone: the DNS servers DHCP gave)

mod fw;
mod rx;
mod sock;

use smoltcp::iface::{Config, Interface, SocketHandle, SocketSet};
use smoltcp::phy::{ChecksumCapabilities, Device, DeviceCapabilities, Medium, RxToken, TxToken};
use smoltcp::socket::{dhcpv4, icmp};
use smoltcp::time::Instant;
use smoltcp::wire::{
    EthernetAddress, HardwareAddress, Icmpv4Packet, Icmpv4Repr, Icmpv6Packet, Icmpv6Repr, IpAddress, IpCidr, Ipv4Address, Ipv4Cidr,
    Ipv6Address, Ipv6Cidr,
};
use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::io::Write;
use std::str::FromStr;

const NETDEV_HELLO: u32 = 1;
const NETDEV_FRAME: u32 = 2;
const NETDEV_LINK: u32 = 3;
const PING_IDENT: u16 = 0x5258;
const LO_ID: u32 = 0;

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

// ---- frames to and from a driver (or round again, on lo) ----

pub struct ChanDev {
    ch: rx::Handle, // 0 for lo
    lo: bool,
    fw: Rc<RefCell<fw::Firewall>>,
    rxq: VecDeque<Vec<u8>>,
    rx_frames: u64,
    tx_frames: u64,
    rx_bytes: u64,
    tx_bytes: u64,
    up: bool,
}

pub struct RxT(Vec<u8>);
pub struct TxT<'a> {
    dev: &'a mut ChanDev,
}

impl RxToken for RxT {
    fn consume<R, F: FnOnce(&[u8]) -> R>(self, f: F) -> R {
        f(&self.0)
    }
}

impl<'a> TxToken for TxT<'a> {
    fn consume<R, F: FnOnce(&mut [u8]) -> R>(self, len: usize, f: F) -> R {
        if self.dev.lo {
            let mut buf = vec![0u8; len];
            let r = f(&mut buf);
            if self.dev.up {
                self.dev.tx_frames += 1;
                self.dev.tx_bytes += len as u64;
                self.dev.rx_frames += 1;
                self.dev.rx_bytes += len as u64;
                self.dev.rxq.push_back(buf);
            }
            return r;
        }
        let mut buf = vec![0u8; 8 + len];
        buf[..4].copy_from_slice(&NETDEV_FRAME.to_le_bytes());
        let r = f(&mut buf[8..]);
        if self.dev.up && self.dev.fw.borrow_mut().allow_out(&buf[8..], rx::monotonic() / 1_000_000_000) {
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
        if self.lo {
            c.medium = Medium::Ip;
            c.max_transmission_unit = 65535;
        } else {
            c.medium = Medium::Ethernet;
            c.max_transmission_unit = 1514;
        }
        c.max_burst_size = Some(32);
        c
    }
}

// ---- interfaces ----

pub struct Link {
    pub id: u32,
    pub name: String,
    driver: String,
    mac: EthernetAddress,
    carrier: bool, // the driver's link
    admin_up: bool,
    pub dev: ChanDev,
    pub iface: Interface,
    pub sockets: SocketSet<'static>,
    icmp: SocketHandle,
    gateway: Option<Ipv4Address>,
    // DHCP (step D3): its socket, the address and DNS servers it gave,
    // when the lease began and how long it is
    dhcp: Option<SocketHandle>,
    dhcp_addr: Option<Ipv4Cidr>,
    dhcp_router: bool,
    pub dns: Vec<Ipv4Address>,
    lease: Option<(Instant, u32)>,
    // IPv6 (step D4): addresses given with `ip`, and an IPv6 default
    // route set by hand (SLAAC's come from router advertisements)
    static6: Vec<Ipv6Cidr>,
    gateway6: Option<Ipv6Address>,
}

impl Link {
    pub fn is_lo(&self) -> bool {
        self.id == LO_ID
    }

    /// Its IPv4 addresses (for lo: 127.0.0.1 only, not the copies of the
    /// others' it carries)
    pub fn v4(&self) -> Vec<Ipv4Cidr> {
        if self.is_lo() {
            return vec![Ipv4Cidr::new(Ipv4Address::new(127, 0, 0, 1), 8)];
        }
        self.iface
            .ip_addrs()
            .iter()
            .filter_map(|c| if let IpCidr::Ipv4(c) = c { Some(*c) } else { None })
            .collect()
    }

    /// Its IPv6 addresses (lo: ::1)
    pub fn v6(&self) -> Vec<Ipv6Cidr> {
        if self.is_lo() {
            return vec![Ipv6Cidr::new(Ipv6Address::LOCALHOST, 128)];
        }
        self.iface
            .ip_addrs()
            .iter()
            .filter_map(|c| if let IpCidr::Ipv6(c) = c { Some(*c) } else { None })
            .collect()
    }

    pub fn link_local(&self) -> Option<Ipv6Address> {
        self.v6().iter().map(|c| c.address()).find(|a| a.is_unicast_link_local())
    }

    /// The IPv6 default route: SLAAC's (from a router advertisement) or ours
    pub fn default6(&mut self) -> Option<(Ipv6Address, bool)> {
        if let Some(g) = self.gateway6 {
            return Some((g, false));
        }
        let mut r = None;
        self.iface.routes_mut().update(|v| {
            for x in v.iter() {
                if let (IpCidr::Ipv6(c), IpAddress::Ipv6(g)) = (x.cidr, x.via_router) {
                    if c.prefix_len() == 0 {
                        r = Some((g, true));
                    }
                }
            }
        });
        r
    }

    /// Forget the addresses, routes and DNS servers configure() and DHCP
    /// gave (the link-local address and SLAAC's stay), before configure()
    /// again (the request "reconfigure", step D6)
    fn unconfigure(&mut self) {
        if let Some(h) = self.dhcp.take() {
            self.sockets.remove(h);
        }
        self.dhcp_addr = None;
        self.dhcp_router = false;
        self.lease = None;
        self.dns.clear();
        if self.gateway.take().is_some() {
            self.iface.routes_mut().remove_default_ipv4_route();
        }
        if self.gateway6.take().is_some() {
            self.iface.routes_mut().remove_default_ipv6_route();
        }
        let static6 = std::mem::take(&mut self.static6);
        self.iface.update_ip_addrs(|a| a.retain(|c| !matches!(c, IpCidr::Ipv4(_)) && !matches!(c, IpCidr::Ipv6(x) if static6.contains(x))));
    }

    pub fn usable(&self) -> bool {
        self.admin_up && (self.carrier || self.is_lo())
    }
}

/// /etc/network.conf: "NAME static ADDRESS/PREFIX [GATEWAY]", "NAME dhcp"
/// or "NAME off"; an interface without a line gets DHCP
fn configure(link: &mut Link) {
    let text = std::fs::read_to_string("/etc/network.conf").unwrap_or_default();
    let mut found = false;
    for line in text.lines() {
        let w: Vec<&str> = line.split('#').next().unwrap_or("").split_whitespace().collect();
        if w.len() < 2 || w[0] != link.name {
            continue;
        }
        found |= w[1] != "static6";
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
            "static6" => {
                if let Some(c) = w.get(2).and_then(|s| Ipv6Cidr::from_str(s).ok()) {
                    link.iface.update_ip_addrs(|a| {
                        let _ = a.push(IpCidr::Ipv6(c));
                    });
                    link.static6.push(c);
                    log(&format!("{}: address {}", link.name, c));
                }
                if let Some(g) = w.get(3).and_then(|s| Ipv6Address::from_str(s).ok()) {
                    if link.iface.routes_mut().add_default_ipv6_route(g).is_ok() {
                        link.gateway6 = Some(g);
                    }
                }
                // IPv4 still gets DHCP unless the card has a line for it too
            }
            "dhcp" => start_dhcp(link),
            "off" | "manual" => log(&format!("{}: not configured (/etc/network.conf)", link.name)),
            other => log(&format!("{}: unknown method {other} in /etc/network.conf", link.name)),
        }
    }
    if !found {
        start_dhcp(link);
    }
}

/// This computer's name, for DHCP's host name option (12)
fn hostname() -> String {
    std::fs::read_to_string("/etc/hostname").ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).unwrap_or_else(|| "rustica".into())
}

fn start_dhcp(link: &mut Link) {
    if link.dhcp.is_some() {
        return;
    }
    let mut d = dhcpv4::Socket::new();
    // smoltcp keeps the options by reference for the socket's life
    let name: &'static [u8] = Box::leak(hostname().into_bytes().into_boxed_slice());
    let opts: &'static [smoltcp::wire::DhcpOption<'static>] = Box::leak(Box::new([smoltcp::wire::DhcpOption { kind: 12, data: name }]));
    d.set_outgoing_options(opts);
    // Keep the server's packet, for the lease time
    d.set_receive_packet_buffer(Box::leak(vec![0u8; 1500].into_boxed_slice()));
    link.dhcp = Some(link.sockets.add(d));
    log(&format!("{}: DHCP", link.name));
}

fn icmp_socket(sockets: &mut SocketSet<'static>) -> SocketHandle {
    let s = icmp::Socket::new(
        icmp::PacketBuffer::new(vec![icmp::PacketMetadata::EMPTY; 16], vec![0; 16 * 1100]),
        icmp::PacketBuffer::new(vec![icmp::PacketMetadata::EMPTY; 16], vec![0; 16 * 1100]),
    );
    let h = sockets.add(s);
    sockets.get_mut::<icmp::Socket>(h).bind(icmp::Endpoint::Ident(PING_IDENT)).ok();
    h
}

fn new_link(id: u32, name: String, ch: rx::Handle, hello: &[u8], fw: &Rc<RefCell<fw::Firewall>>) -> Link {
    let mut mac = [0u8; 6];
    mac.copy_from_slice(&hello[..6]);
    let link_up = u16::from_le_bytes([hello[6], hello[7]]) != 0;
    let driver = String::from_utf8_lossy(&hello[8..32]).trim_end_matches('\0').to_string();
    let mut dev = ChanDev { ch, lo: false, fw: fw.clone(), rxq: VecDeque::new(), rx_frames: 0, tx_frames: 0, rx_bytes: 0, tx_bytes: 0, up: true };
    let mut config = Config::new(HardwareAddress::Ethernet(EthernetAddress(mac)));
    config.random_seed = rx::monotonic() ^ (u64::from_le_bytes([mac[0], mac[1], mac[2], mac[3], mac[4], mac[5], 0, 0]));
    // SLAAC: addresses and the default route from router advertisements
    config.slaac = true;
    let mut iface = Interface::new(config, &mut dev, now());
    // The link-local address, fe80::/64 with the EUI-64 of the MAC address
    let eui = [mac[0] ^ 2, mac[1], mac[2], 0xff, 0xfe, mac[3], mac[4], mac[5]];
    let ll = Ipv6Address::new(
        0xfe80,
        0,
        0,
        0,
        u16::from_be_bytes([eui[0], eui[1]]),
        u16::from_be_bytes([eui[2], eui[3]]),
        u16::from_be_bytes([eui[4], eui[5]]),
        u16::from_be_bytes([eui[6], eui[7]]),
    );
    iface.update_ip_addrs(|a| {
        let _ = a.push(IpCidr::Ipv6(Ipv6Cidr::new(ll, 64)));
    });
    let mut sockets = SocketSet::new(vec![]);
    let icmp = icmp_socket(&mut sockets);
    let mut l = Link {
        id,
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
        dhcp: None,
        dhcp_addr: None,
        dhcp_router: false,
        dns: Vec::new(),
        lease: None,
        static6: Vec::new(),
        gateway6: None,
    };
    configure(&mut l);
    l
}

fn new_lo(fw: &Rc<RefCell<fw::Firewall>>) -> Link {
    let mut dev = ChanDev { ch: 0, lo: true, fw: fw.clone(), rxq: VecDeque::new(), rx_frames: 0, tx_frames: 0, rx_bytes: 0, tx_bytes: 0, up: true };
    let mut config = Config::new(HardwareAddress::Ip);
    config.random_seed = rx::monotonic();
    let iface = Interface::new(config, &mut dev, now());
    let mut sockets = SocketSet::new(vec![]);
    let icmp = icmp_socket(&mut sockets);
    Link {
        id: LO_ID,
        name: "lo".into(),
        driver: "loopback".into(),
        mac: EthernetAddress([0; 6]),
        carrier: true,
        admin_up: true,
        dev,
        iface,
        sockets,
        icmp,
        gateway: None,
        dhcp: None,
        dhcp_addr: None,
        dhcp_router: false,
        dns: Vec::new(),
        lease: None,
        static6: Vec::new(),
        gateway6: None,
    }
}

// ---- pings in flight ----

struct Ping {
    client: rx::Handle,
    link: u32,
    seq: u16, // ours, on the wire
    client_seq: u32,
    sent: Instant,
    deadline: Instant,
}

pub struct Netd {
    pub links: Vec<Link>,
    next_link: u32,
    pending_drivers: Vec<rx::Handle>,
    clients: Vec<rx::Handle>,
    pings: Vec<Ping>,
    next_seq: u16,
    next_eth: usize,
    pub socks: Vec<sock::Sock>,
    pub next_sock: u64,
    pub next_port: u16,
    fw: Rc<RefCell<fw::Firewall>>,
}

fn reply(ch: rx::Handle, text: &str) {
    rx::channel_write(ch, text.as_bytes(), &[]);
}

impl Netd {
    fn link_by_name(&self, n: &str) -> Option<usize> {
        self.links.iter().position(|l| l.name == n)
    }

    pub fn link_index(&self, id: u32) -> Option<usize> {
        self.links.iter().position(|l| l.id == id)
    }

    /// Is `a` this computer's (127/8, ::1 or an interface's address)?
    pub fn is_local4(&self, a: Ipv4Address) -> bool {
        a.octets()[0] == 127 || self.links.iter().any(|l| !l.is_lo() && l.v4().iter().any(|c| c.address() == a))
    }

    pub fn is_local(&self, a: IpAddress) -> bool {
        match a {
            IpAddress::Ipv4(a) => self.is_local4(a),
            IpAddress::Ipv6(a) => a.is_loopback() || self.links.iter().any(|l| !l.is_lo() && l.v6().iter().any(|c| c.address() == a)),
        }
    }

    // The interface (not lo) for an address: the one whose subnet holds
    // it, else the one with a default route
    fn link_for(&self, a: Ipv4Address) -> Option<usize> {
        for (i, l) in self.links.iter().enumerate() {
            if l.is_lo() {
                continue;
            }
            if l.v4().iter().any(|c| c.contains_addr(&a)) {
                return Some(i);
            }
        }
        self.links.iter().position(|l| l.gateway.is_some())
    }

    /// Where a packet to `dst` goes: the interface and the source address
    pub fn route(&mut self, dst: IpAddress) -> Option<(usize, IpAddress)> {
        match dst {
            IpAddress::Ipv4(d) => self.route4(d).map(|(i, s)| (i, IpAddress::Ipv4(s))),
            IpAddress::Ipv6(d) => self.route6(d).map(|(i, s)| (i, IpAddress::Ipv6(s))),
        }
    }

    // IPv6: this computer's addresses through lo; link-local ones on the
    // first card up; else the card whose prefix holds it, else the one
    // with an IPv6 default route; the source: a global address of that
    // card (link-local for link-local destinations)
    fn route6(&mut self, dst: Ipv6Address) -> Option<(usize, Ipv6Address)> {
        if self.is_local(IpAddress::Ipv6(dst)) {
            let lo = self.link_index(LO_ID)?;
            return Some((lo, if dst.is_loopback() { Ipv6Address::LOCALHOST } else { dst }));
        }
        if dst.is_unicast_link_local() || dst.is_multicast() {
            let i = self.links.iter().position(|l| !l.is_lo() && l.usable() && l.link_local().is_some())?;
            return Some((i, self.links[i].link_local()?));
        }
        let global = |l: &Link| l.v6().iter().map(|c| c.address()).find(|a| !a.is_unicast_link_local());
        let mut pick = None;
        for (i, l) in self.links.iter().enumerate() {
            if !l.is_lo() && l.v6().iter().any(|c| !c.address().is_unicast_link_local() && c.contains_addr(&dst)) {
                pick = Some(i);
                break;
            }
        }
        if pick.is_none() {
            for i in 0..self.links.len() {
                if !self.links[i].is_lo() && self.links[i].default6().is_some() && global(&self.links[i]).is_some() {
                    pick = Some(i);
                    break;
                }
            }
        }
        let i = pick?;
        let src = self.links[i].v6().iter().map(|c| c.address()).find(|a| !a.is_unicast_link_local() && Ipv6Cidr::new(*a, 64).contains_addr(&dst)).or(global(&self.links[i]))?;
        Some((i, src))
    }

    pub fn route4(&self, dst: Ipv4Address) -> Option<(usize, Ipv4Address)> {
        if self.is_local4(dst) {
            let lo = self.link_index(LO_ID)?;
            let src = if dst.octets()[0] == 127 { Ipv4Address::new(127, 0, 0, 1) } else { dst };
            return Some((lo, src));
        }
        let i = self.link_for(dst)?;
        let v4 = self.links[i].v4();
        let src = v4.iter().find(|c| c.contains_addr(&dst)).or(v4.first())?.address();
        Some((i, src))
    }

    /// lo carries every interface's addresses too (/32), so traffic to
    /// them stays inside
    pub fn sync_lo(&mut self) {
        let mut all: Vec<Ipv4Address> = Vec::new();
        for l in &self.links {
            if !l.is_lo() {
                for c in l.v4() {
                    if !all.contains(&c.address()) {
                        all.push(c.address());
                    }
                }
            }
        }
        let mut all6: Vec<Ipv6Address> = Vec::new();
        for l in &self.links {
            if !l.is_lo() {
                for c in l.v6() {
                    if !c.address().is_unicast_link_local() && !all6.contains(&c.address()) {
                        all6.push(c.address());
                    }
                }
            }
        }
        let mut want: Vec<IpCidr> = vec![IpCidr::Ipv4(Ipv4Cidr::new(Ipv4Address::new(127, 0, 0, 1), 8)), IpCidr::Ipv6(Ipv6Cidr::new(Ipv6Address::LOCALHOST, 128))];
        want.extend(all.iter().map(|x| IpCidr::Ipv4(Ipv4Cidr::new(*x, 32))));
        want.extend(all6.iter().map(|x| IpCidr::Ipv6(Ipv6Cidr::new(*x, 128))));
        if let Some(i) = self.link_index(LO_ID) {
            if self.links[i].iface.ip_addrs() == &want[..want.len().min(16)] {
                return;
            }
            self.links[i].iface.update_ip_addrs(|a| {
                a.clear();
                for x in want.iter().take(16) {
                    let _ = a.push(*x);
                }
            });
        }
    }

    // The interfaces in name order (lo, eth0, eth1, ..., eth10)
    fn sorted(&self) -> Vec<&Link> {
        let mut v: Vec<&Link> = self.links.iter().collect();
        v.sort_by_key(|l| (!l.is_lo(), l.name.len(), l.name.clone()));
        v
    }

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

    fn command(&mut self, ch: rx::Handle, who: &rx::Sender, text: &str) {
        let w: Vec<&str> = text.split_whitespace().collect();
        let root = who.euid == 0;
        let mut out = String::new();
        let mut addrs_changed = false;
        let res: Result<(), String> = (|| {
            match w.as_slice() {
                ["links"] => {
                    for l in self.sorted() {
                        out.push_str(&format!(
                            "{} {} {} {} mtu {} rx {} tx {} {} {}\n",
                            l.name,
                            l.driver.replace(' ', "_"),
                            mac_str(&l.mac),
                            if !l.admin_up { "down" } else if l.carrier { "up" } else { "no-carrier" },
                            if l.is_lo() { 65536 } else { 1500 },
                            l.dev.rx_frames,
                            l.dev.tx_frames,
                            l.dev.rx_bytes,
                            l.dev.tx_bytes
                        ));
                    }
                }
                ["addrs"] => {
                    let t = now();
                    for l in self.sorted() {
                        for c in l.v4() {
                            if l.dhcp_addr == Some(c) {
                                let left = l.lease.map(|(at, secs)| (secs as i64 - (t - at).secs() as i64).max(0).to_string()).unwrap_or("forever".into());
                                out.push_str(&format!("{} {} dynamic {}\n", l.name, c, left));
                            } else {
                                out.push_str(&format!("{} {}\n", l.name, c));
                            }
                        }
                        for c in l.v6() {
                            if l.is_lo() || c.address().is_unicast_link_local() || l.static6.contains(&c) {
                                out.push_str(&format!("{} {}\n", l.name, c));
                            } else {
                                out.push_str(&format!("{} {} dynamic forever\n", l.name, c)); // SLAAC
                            }
                        }
                    }
                }
                ["routes"] => {
                    let mut d6: Vec<(String, Ipv6Address, bool)> = Vec::new();
                    for l in self.links.iter_mut() {
                        if let Some((g, ra)) = l.default6() {
                            d6.push((l.name.clone(), g, ra));
                        }
                    }
                    for l in self.sorted() {
                        if l.is_lo() {
                            continue;
                        }
                        if let Some(g) = l.gateway {
                            out.push_str(&format!("default via {} dev {}{}\n", g, l.name, if l.dhcp_router { " proto dhcp" } else { "" }));
                        }
                        for c in l.v4() {
                            out.push_str(&format!("{} dev {} src {}\n", c.network(), l.name, c.address()));
                        }
                        for (n, g, ra) in &d6 {
                            if *n == l.name {
                                out.push_str(&format!("default via {} dev {}{}\n", g, l.name, if *ra { " proto ra" } else { "" }));
                            }
                        }
                        for c in l.v6() {
                            let net = Ipv6Cidr::new(c.address(), c.prefix_len());
                            let mut b = net.address().octets();
                            let p = c.prefix_len() as usize;
                            for (k, x) in b.iter_mut().enumerate() {
                                let bits = (p.saturating_sub(k * 8)).min(8);
                                *x &= if bits == 0 { 0 } else { 0xffu8 << (8 - bits) };
                            }
                            out.push_str(&format!("{}/{} dev {} src {}\n", Ipv6Address::from_octets(b), p, l.name, c.address()));
                        }
                    }
                }
                ["sockets"] => out.push_str(&sock::list(self)),
                ["fw", "list"] => out.push_str(&self.fw.borrow().list()),
                ["fw", rest @ ..] => {
                    if !root {
                        return Err("only root changes the firewall".into());
                    }
                    let mut f = self.fw.borrow_mut();
                    match rest {
                        ["add", pos, rule @ ..] if pos.parse::<usize>().is_ok() => {
                            let r = fw::parse_rule(rule)?;
                            let at = pos.parse::<usize>().unwrap().clamp(1, f.rules.len() + 1) - 1;
                            log(&format!("firewall: rule {}: {}", at + 1, r.text()));
                            f.rules.insert(at, r);
                            f.enabled = true;
                        }
                        ["add", rule @ ..] => {
                            let r = fw::parse_rule(rule)?;
                            log(&format!("firewall: rule {}: {}", f.rules.len() + 1, r.text()));
                            f.rules.push(r);
                            f.enabled = true;
                        }
                        ["del", num] => {
                            let k: usize = num.parse().map_err(|_| format!("{num}: not a rule number"))?;
                            if k == 0 || k > f.rules.len() {
                                return Err(format!("there is no rule {k}"));
                            }
                            let r = f.rules.remove(k - 1);
                            log(&format!("firewall: removed {}", r.text()));
                        }
                        ["delrule", rule @ ..] => {
                            let r = fw::parse_rule(rule)?;
                            let k = f.rules.iter().position(|x| x.text() == r.text()).ok_or("no such rule")?;
                            f.rules.remove(k);
                            log(&format!("firewall: removed {}", r.text()));
                        }
                        ["default", dir @ ("in" | "out"), act @ ("allow" | "deny")] => {
                            if *dir == "in" {
                                f.default_in = *act == "allow";
                            } else {
                                f.default_out = *act == "allow";
                            }
                            log(&format!("firewall: default {dir} {act}"));
                        }
                        ["enable"] => {
                            f.enabled = true;
                            log("firewall: on");
                        }
                        ["disable"] => {
                            f.enabled = false;
                            log("firewall: off");
                        }
                        ["reload"] => {
                            let errs = f.load();
                            log(&format!("firewall: {} read again", fw::CONF));
                            if !errs.is_empty() {
                                return Err(errs.join("; "));
                            }
                        }
                        ["save"] => {
                            f.save().map_err(|e| format!("{}: {e}", fw::CONF))?;
                            log(&format!("firewall: saved to {}", fw::CONF));
                        }
                        _ => return Err(format!("unknown firewall request: {}", rest.join(" "))),
                    }
                }
                ["dns"] => {
                    for l in self.sorted() {
                        for d in &l.dns {
                            out.push_str(&format!("{} {}\n", l.name, d));
                        }
                    }
                }
                ["addr", op @ ("add" | "del"), cidr, "dev", ifname] => {
                    if !root {
                        return Err("only root changes addresses".into());
                    }
                    let c = IpCidr::from_str(cidr)
                        .ok()
                        .filter(|c| match c {
                            IpCidr::Ipv4(x) => x.prefix_len() <= 32,
                            IpCidr::Ipv6(x) => x.prefix_len() <= 128,
                        })
                        .ok_or(format!("{cidr}: not an address/prefix"))?;
                    let i = self.link_by_name(ifname).ok_or(format!("{ifname}: no such interface"))?;
                    if self.links[i].is_lo() {
                        return Err("lo has 127.0.0.1/8 and ::1 only".into());
                    }
                    let l = &mut self.links[i];
                    let mut err = None;
                    l.iface.update_ip_addrs(|a| {
                        if *op == "add" {
                            if a.iter().any(|x| *x == c) {
                                err = Some("the address is there already".to_string());
                            } else if a.push(c).is_err() {
                                err = Some("too many addresses".to_string());
                            }
                        } else if let Some(p) = a.iter().position(|x| *x == c) {
                            a.remove(p);
                        } else {
                            err = Some("no such address".to_string());
                        }
                    });
                    if let Some(e) = err {
                        return Err(e);
                    }
                    if let IpCidr::Ipv6(c6) = c {
                        if *op == "add" {
                            l.static6.push(c6);
                        } else {
                            l.static6.retain(|x| *x != c6);
                        }
                    }
                    addrs_changed = true;
                    log(&format!("{}: address {} {}", l.name, if *op == "add" { "added" } else { "removed" }, c));
                }
                ["route", "add", "default", "via", gw] => {
                    if !root {
                        return Err("only root changes routes".into());
                    }
                    let g = Ipv4Address::from_str(gw).map_err(|_| format!("{gw}: not an address"))?;
                    let i = self
                        .links
                        .iter()
                        .position(|l| !l.is_lo() && l.v4().iter().any(|c| c.contains_addr(&g)))
                        .ok_or("no interface reaches it")?;
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
                ["route6", "add", "default", "via", gw] => {
                    if !root {
                        return Err("only root changes routes".into());
                    }
                    let g = Ipv6Address::from_str(gw).map_err(|_| format!("{gw}: not an IPv6 address"))?;
                    let i = self
                        .links
                        .iter()
                        .position(|l| !l.is_lo() && (g.is_unicast_link_local() && l.link_local().is_some() || l.v6().iter().any(|c| c.contains_addr(&g))))
                        .ok_or("no interface reaches it")?;
                    for l in self.links.iter_mut() {
                        if l.gateway6.take().is_some() {
                            l.iface.routes_mut().remove_default_ipv6_route();
                        }
                    }
                    let l = &mut self.links[i];
                    l.iface.routes_mut().add_default_ipv6_route(g).map_err(|_| "too many routes".to_string())?;
                    l.gateway6 = Some(g);
                    log(&format!("{}: IPv6 default route via {}", l.name, g));
                }
                ["route6", "del", "default", ..] => {
                    if !root {
                        return Err("only root changes routes".into());
                    }
                    let mut any = false;
                    for l in self.links.iter_mut() {
                        if l.default6().is_some() {
                            l.gateway6 = None;
                            l.iface.routes_mut().remove_default_ipv6_route();
                            any = true;
                        }
                    }
                    if !any {
                        return Err("there is no IPv6 default route".into());
                    }
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
                // /etc/network.conf again, for one interface or all of
                // them (netcfg, Settings > Network; step D6)
                ["reconfigure", rest @ ..] => {
                    if !root {
                        return Err("only root changes interfaces".into());
                    }
                    let only = rest.first().copied();
                    if let Some(n) = only {
                        if self.link_by_name(n).is_none() {
                            return Err(format!("{n}: no such interface"));
                        }
                    }
                    for l in self.links.iter_mut() {
                        if l.is_lo() || only.is_some_and(|n| n != l.name) {
                            continue;
                        }
                        log(&format!("{}: configured again from /etc/network.conf", l.name));
                        l.unconfigure();
                        configure(l);
                    }
                    addrs_changed = true;
                    self.write_resolv();
                }
                // network.conf's dns and search lines again (netcfg dns)
                ["resolv"] => {
                    if !root {
                        return Err("only root changes the DNS servers".into());
                    }
                    self.write_resolv();
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
                    let a = IpAddress::from_str(addr).map_err(|_| format!("{addr}: not an address"))?;
                    let cseq: u32 = seq.parse().map_err(|_| "bad sequence number")?;
                    let tmo: u64 = timeout.parse().map_err(|_| "bad time-out")?;
                    let size: usize = size.parse::<usize>().map_err(|_| "bad size")?.min(1000);
                    let (i, src) = self.route(a).ok_or("network unreachable")?;
                    let t = now();
                    let seq_no = self.next_seq;
                    self.next_seq = self.next_seq.wrapping_add(1);
                    let l = &mut self.links[i];
                    if !l.usable() {
                        return Err(format!("{}: the link is down", l.name));
                    }
                    let mut data = vec![0u8; size];
                    for (k, b) in data.iter_mut().enumerate() {
                        *b = k as u8;
                    }
                    let s = l.sockets.get_mut::<icmp::Socket>(l.icmp);
                    match (a, src) {
                        (IpAddress::Ipv6(d6), IpAddress::Ipv6(s6)) => {
                            let repr = Icmpv6Repr::EchoRequest { ident: PING_IDENT, seq_no, data: &data };
                            let buf = s.send(repr.buffer_len(), a).map_err(|_| "too many pings at once")?;
                            let mut pkt = Icmpv6Packet::new_unchecked(buf);
                            repr.emit(&s6, &d6, &mut pkt, &ChecksumCapabilities::default());
                        }
                        _ => {
                            let repr = Icmpv4Repr::EchoRequest { ident: PING_IDENT, seq_no, data: &data };
                            let buf = s.send(repr.buffer_len(), a).map_err(|_| "too many pings at once")?;
                            let mut pkt = Icmpv4Packet::new_unchecked(buf);
                            repr.emit(&mut pkt, &ChecksumCapabilities::default());
                        }
                    }
                    self.pings.push(Ping {
                        client: ch,
                        link: l.id,
                        seq: seq_no,
                        client_seq: cseq,
                        sent: t,
                        deadline: t + smoltcp::time::Duration::from_millis(tmo),
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
        if addrs_changed {
            self.sync_lo();
        }
    }

    // Echo replies and pings whose time is up
    fn pings(&mut self) {
        let t = now();
        for l in self.links.iter_mut() {
            let s = l.sockets.get_mut::<icmp::Socket>(l.icmp);
            while s.can_recv() {
                let Ok((payload, from)) = s.recv() else { break };
                let (ident, seq_no, len) = match from {
                    IpAddress::Ipv4(_) => {
                        let Ok(pkt) = Icmpv4Packet::new_checked(payload) else { continue };
                        let Ok(Icmpv4Repr::EchoReply { ident, seq_no, data }) = Icmpv4Repr::parse(&pkt, &ChecksumCapabilities::default()) else {
                            continue;
                        };
                        (ident, seq_no, data.len())
                    }
                    IpAddress::Ipv6(f6) => {
                        let Ok(pkt) = Icmpv6Packet::new_checked(payload) else { continue };
                        let Ok(Icmpv6Repr::EchoReply { ident, seq_no, data }) =
                            Icmpv6Repr::parse(&f6, &Ipv6Address::UNSPECIFIED, &pkt, &ChecksumCapabilities::ignored())
                        else {
                            continue;
                        };
                        (ident, seq_no, data.len())
                    }
                };
                if ident != PING_IDENT {
                    continue;
                }
                if let Some(p) = self.pings.iter().position(|p| p.link == l.id && p.seq == seq_no) {
                    let p = self.pings.remove(p);
                    let us = (t - p.sent).total_micros();
                    reply(p.client, &format!("reply {} {} {} {}\n", from, p.client_seq, len, us));
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

    /// DHCP leases gained, renewed with a change, or lost; true if any
    fn dhcp_events(&mut self) -> bool {
        let mut changed = false;
        for l in self.links.iter_mut() {
            let Some(h) = l.dhcp else { continue };
            let ev = match l.sockets.get_mut::<dhcpv4::Socket>(h).poll() {
                None => continue,
                Some(dhcpv4::Event::Configured(c)) => {
                    let lease = c.packet.as_ref().and_then(|p| smoltcp::wire::DhcpRepr::parse(p).ok().and_then(|r| r.lease_duration));
                    Some((c.address, c.router, c.dns_servers.iter().copied().collect::<Vec<_>>(), lease))
                }
                Some(dhcpv4::Event::Deconfigured) => None,
            };
            changed = true;
            if ev.is_none() && l.dhcp_addr.is_some() {
                log(&format!("{}: DHCP lease lost", l.name));
            }
            // Out with the old lease
            if let Some(old) = l.dhcp_addr.take() {
                l.iface.update_ip_addrs(|a| a.retain(|x| *x != IpCidr::Ipv4(old)));
            }
            if l.dhcp_router {
                l.iface.routes_mut().remove_default_ipv4_route();
                l.gateway = None;
                l.dhcp_router = false;
            }
            l.dns.clear();
            l.lease = None;
            match ev {
                Some((addr, router, dns, lease)) => {
                    l.iface.update_ip_addrs(|a| {
                        if !a.contains(&IpCidr::Ipv4(addr)) {
                            let _ = a.push(IpCidr::Ipv4(addr));
                        }
                    });
                    l.dhcp_addr = Some(addr);
                    if let Some(r) = router {
                        if l.gateway.is_none() && l.iface.routes_mut().add_default_ipv4_route(r).is_ok() {
                            l.gateway = Some(r);
                            l.dhcp_router = true;
                        }
                    }
                    l.dns = dns;
                    l.lease = lease.map(|s| (now(), s));
                    log(&format!(
                        "{}: DHCP lease {}{}{}{}",
                        l.name,
                        addr,
                        router.map(|r| format!(", router {r}")).unwrap_or_default(),
                        if l.dns.is_empty() { String::new() } else { format!(", DNS {}", l.dns.iter().map(|d| d.to_string()).collect::<Vec<_>>().join(" ")) },
                        lease.map(|s| format!(", {s} s")).unwrap_or_default()
                    ));
                }
                None => {}
            }
        }
        if changed {
            self.sync_lo();
            self.write_resolv();
        }
        changed
    }

    /// /etc/resolv.conf from the DNS servers (network.conf's "dns" lines,
    /// then DHCP's), unless it is the administrator's own (no netd line)
    pub fn write_resolv(&self) {
        const MARK: &str = "# Written by netd";
        let path = "/etc/resolv.conf";
        if let Ok(old) = std::fs::read_to_string(path) {
            if !old.is_empty() && !old.starts_with(MARK) {
                return;
            }
        }
        let mut servers: Vec<Ipv4Address> = Vec::new();
        let mut search: Vec<String> = Vec::new();
        let conf = std::fs::read_to_string("/etc/network.conf").unwrap_or_default();
        for line in conf.lines() {
            let w: Vec<&str> = line.split('#').next().unwrap_or("").split_whitespace().collect();
            match w.as_slice() {
                ["dns", rest @ ..] => servers.extend(rest.iter().filter_map(|a| Ipv4Address::from_str(a).ok())),
                ["search", rest @ ..] => search.extend(rest.iter().map(|s| s.to_string())),
                _ => {}
            }
        }
        for l in self.sorted() {
            for d in &l.dns {
                if !servers.contains(d) {
                    servers.push(*d);
                }
            }
        }
        let mut text = format!("{MARK} (DHCP and /etc/network.conf); remove this line to keep your own\n");
        if !search.is_empty() {
            text.push_str(&format!("search {}\n", search.join(" ")));
        }
        for s in servers.iter().take(3) {
            text.push_str(&format!("nameserver {s}\n"));
        }
        if std::fs::read_to_string(path).ok().as_deref() != Some(text.as_str()) {
            let _ = std::fs::write(path, text);
        }
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
                if !self.fw.borrow_mut().allow_in(&buf[8..], rx::monotonic() / 1_000_000_000) {
                    return;
                }
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

    fn add_link(&mut self, d: rx::Handle, hello: &[u8]) {
        let name = self.name_for(hello);
        let id = self.next_link;
        self.next_link += 1;
        let l = new_link(id, name, d, hello, &self.fw);
        log(&format!("{}: {} ({}), link {}", l.name, mac_str(&l.mac), l.driver, if l.carrier { "up" } else { "down" }));
        self.links.push(l);
        self.sync_lo();
        sock::link_added(self, id);
    }

    fn remove_link(&mut self, id: u32) {
        let Some(i) = self.link_index(id) else { return };
        sock::link_removed(self, id);
        let l = self.links.remove(i);
        log(&format!("{}: the driver is gone", l.name));
        self.pings.retain(|p| p.link != id);
        rx::close(l.dev.ch);
        self.sync_lo();
    }
}

// What a wait item stands for
enum Waiting {
    Svc,
    Netdev,
    Netsock,
    Driver(u32),
    Pending(rx::Handle),
    Client(rx::Handle),
    SockCtl(u64),
    SockData(u64),
}

fn main() {
    let (Some(svc), Some(netdev), Some(netsock)) = (rx::publish("/svc/net"), rx::publish("/svc/netdev"), rx::publish("/svc/netsock")) else {
        eprintln!("netd: cannot publish /svc/net, /svc/netdev and /svc/netsock");
        std::process::exit(3);
    };
    log("started (/svc/net, /svc/netdev, /svc/netsock)");
    let firewall = Rc::new(RefCell::new(fw::Firewall::new()));
    {
        let f = firewall.borrow();
        if f.enabled {
            log(&format!(
                "firewall: {} rules, new connections in {} by default, out {}",
                f.rules.len(),
                if f.default_in { "allowed" } else { "denied" },
                if f.default_out { "allowed" } else { "denied" }
            ));
        }
    }
    let mut n = Netd {
        links: vec![new_lo(&firewall)],
        next_link: 1,
        pending_drivers: Vec::new(),
        clients: Vec::new(),
        pings: Vec::new(),
        next_seq: 1,
        next_eth: 0,
        socks: Vec::new(),
        next_sock: 1,
        next_port: 49152,
        fw: firewall.clone(),
    };
    n.sync_lo();
    n.write_resolv();
    let mut buf = vec![0u8; 65536];
    loop {
        // The stack, then the sockets, until neither has more to do now
        let mut rounds = 0;
        loop {
            let t = now();
            let mut busy = false;
            for l in n.links.iter_mut() {
                if l.iface.poll(t, &mut l.dev, &mut l.sockets) != smoltcp::iface::PollResult::None {
                    busy = true;
                }
            }
            n.pings();
            n.sync_lo();
            if n.dhcp_events() {
                busy = true;
            }
            if sock::service(&mut n) {
                busy = true;
            }
            rounds += 1;
            if !busy || rounds >= 16 {
                break;
            }
        }
        // How long until something is due
        let t = now();
        let mono = rx::monotonic();
        let mut deadline = rx::TIME_INFINITE;
        for l in n.links.iter_mut() {
            if let Some(d) = l.iface.poll_delay(t, &l.sockets) {
                deadline = deadline.min(mono + d.total_micros() * 1000);
            }
            if !l.dev.rxq.is_empty() {
                deadline = mono;
            }
        }
        for p in &n.pings {
            deadline = deadline.min(mono + ((p.deadline - t).total_micros().max(0) as u64 * 1000));
        }
        if let Some(d) = sock::next_deadline(&n) {
            deadline = deadline.min(mono + ((d - t).total_micros().max(0) as u64 * 1000));
        }
        // Wait for a frame, a request, a connection or a socket
        let mut items = Vec::new();
        let mut what = Vec::new();
        let add = |items: &mut Vec<rx::WaitItem>, what: &mut Vec<Waiting>, h: rx::Handle, sig: u32, w: Waiting| {
            items.push(rx::WaitItem { handle: h, waitfor: sig, pending: 0 });
            what.push(w);
        };
        add(&mut items, &mut what, svc, rx::CHANNEL_READABLE, Waiting::Svc);
        add(&mut items, &mut what, netdev, rx::CHANNEL_READABLE, Waiting::Netdev);
        add(&mut items, &mut what, netsock, rx::CHANNEL_READABLE, Waiting::Netsock);
        for l in &n.links {
            if l.dev.ch != 0 {
                add(&mut items, &mut what, l.dev.ch, rx::CHANNEL_READABLE | rx::PEER_CLOSED, Waiting::Driver(l.id));
            }
        }
        for &d in &n.pending_drivers {
            add(&mut items, &mut what, d, rx::CHANNEL_READABLE | rx::PEER_CLOSED, Waiting::Pending(d));
        }
        for &c in &n.clients {
            add(&mut items, &mut what, c, rx::CHANNEL_READABLE | rx::PEER_CLOSED, Waiting::Client(c));
        }
        for s in &n.socks {
            if s.ctl != 0 {
                add(&mut items, &mut what, s.ctl, rx::CHANNEL_READABLE | rx::PEER_CLOSED, Waiting::SockCtl(s.id));
            }
            if let Some(sig) = s.data_signals(&n) {
                add(&mut items, &mut what, s.data, sig, Waiting::SockData(s.id));
            }
        }
        rx::wait_many(&mut items, deadline);
        let mut gone_links = Vec::new();
        let mut gone_clients = Vec::new();
        for (k, w) in what.iter().enumerate() {
            let pending = items[k].pending;
            if pending == 0 {
                continue;
            }
            match *w {
                Waiting::Svc => n.clients.extend(rx::accept_all(svc)),
                Waiting::Netdev => n.pending_drivers.extend(rx::accept_all(netdev)),
                Waiting::Netsock => {
                    for c in rx::accept_all(netsock) {
                        sock::new_connection(&mut n, c);
                    }
                }
                Waiting::Driver(id) => {
                    let Some(i) = n.link_index(id) else { continue };
                    let ch = n.links[i].dev.ch;
                    // A few at a time each, so one busy card does not starve the rest
                    for _ in 0..64 {
                        match rx::channel_read(ch, &mut buf, None) {
                            Ok((len, h)) => {
                                if let Some(h) = h {
                                    rx::close(h);
                                }
                                n.driver_message(i, &buf[..len]);
                            }
                            Err(rx::ERR_SHOULD_WAIT) => break,
                            Err(_) => {
                                gone_links.push(id);
                                break;
                            }
                        }
                    }
                }
                Waiting::Pending(d) => match rx::channel_read(d, &mut buf, None) {
                    Ok((len, _)) if len >= 8 + 32 && u32::from_le_bytes(buf[..4].try_into().unwrap()) == NETDEV_HELLO => {
                        n.pending_drivers.retain(|&x| x != d);
                        let hello = buf[8..len].to_vec();
                        n.add_link(d, &hello);
                    }
                    Err(rx::ERR_SHOULD_WAIT) | Ok(_) => {}
                    Err(_) => {
                        n.pending_drivers.retain(|&x| x != d);
                        rx::close(d);
                    }
                },
                Waiting::Client(c) => {
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
                Waiting::SockCtl(id) => sock::ctl_ready(&mut n, id, &mut buf),
                Waiting::SockData(id) => sock::data_ready(&mut n, id, pending),
            }
        }
        for c in gone_clients {
            n.clients.retain(|&x| x != c);
            n.pings.retain(|p| p.client != c);
            rx::close(c);
        }
        for id in gone_links {
            n.remove_link(id);
        }
    }
}
