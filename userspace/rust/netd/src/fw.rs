// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! The firewall (network step D5, docs/NETWORK.md, man fwctl, man
//! firewall.conf): netd looks at each frame a card gives it, and each one
//! it sends, before smoltcp and the wire. Rules decide about new
//! conversations: a TCP connection's first packet (SYN without ACK), a UDP
//! datagram that is not an answer, an ICMP echo request. The rest of a
//! conversation passes: TCP packets with ACK, UDP to a port this computer
//! sent from in the last minutes, ICMP answers and errors, IPv6's
//! neighbour discovery. lo is never filtered.
//!
//! Rules, first match wins, then the direction's default:
//!   allow|deny in|out tcp|udp|icmp|any [PORT[-PORT]] [from|to ADDRESS[/PREFIX]]
//!   default in|out allow|deny

use smoltcp::wire::{IpAddress, IpCidr};
use std::collections::HashMap;
use std::str::FromStr;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Proto {
    Tcp,
    Udp,
    Icmp,
    Any,
}

#[derive(Clone, Debug)]
pub struct Rule {
    pub allow: bool,
    pub inbound: bool,
    pub proto: Proto,
    pub ports: Option<(u16, u16)>,
    pub addr: Option<IpCidr>, // the other side's
    pub hits: u64,
}

pub struct Firewall {
    pub rules: Vec<Rule>,
    pub default_in: bool,  // allow?
    pub default_out: bool, // allow?
    pub enabled: bool,
    pub dropped_in: u64,
    pub dropped_out: u64,
    // Local UDP ports that sent lately (their answers may come in): seconds
    udp_out: HashMap<u16, u64>,
}

pub const CONF: &str = "/etc/firewall.conf";

impl Rule {
    pub fn text(&self) -> String {
        let mut s = format!(
            "{} {} {}",
            if self.allow { "allow" } else { "deny" },
            if self.inbound { "in" } else { "out" },
            match self.proto {
                Proto::Tcp => "tcp",
                Proto::Udp => "udp",
                Proto::Icmp => "icmp",
                Proto::Any => "any",
            }
        );
        if let Some((a, b)) = self.ports {
            if a == b {
                s.push_str(&format!(" {a}"));
            } else {
                s.push_str(&format!(" {a}-{b}"));
            }
        }
        if let Some(c) = self.addr {
            s.push_str(&format!(" {} {}", if self.inbound { "from" } else { "to" }, c));
        }
        s
    }
}

/// A rule from words: "allow in tcp 22", "deny tcp 8080" (in), ...
pub fn parse_rule(w: &[&str]) -> Result<Rule, String> {
    let mut it = w.iter().copied().peekable();
    let allow = match it.next() {
        Some("allow") => true,
        Some("deny") => false,
        _ => return Err("a rule starts with allow or deny".into()),
    };
    let mut inbound = true;
    if let Some(&d) = it.peek() {
        if d == "in" || d == "out" {
            inbound = d == "in";
            it.next();
        }
    }
    let proto = match it.next() {
        Some("tcp") => Proto::Tcp,
        Some("udp") => Proto::Udp,
        Some("icmp") | Some("icmp6") | Some("ping") => Proto::Icmp,
        Some("any") | Some("all") => Proto::Any,
        Some(p) => return Err(format!("{p}: tcp, udp, icmp or any")),
        None => return Err("which protocol? (tcp, udp, icmp, any)".into()),
    };
    let mut r = Rule { allow, inbound, proto, ports: None, addr: None, hits: 0 };
    while let Some(x) = it.next() {
        match x {
            "from" | "to" => {
                let a = it.next().ok_or("an address after from/to")?;
                let c = IpCidr::from_str(a)
                    .or_else(|_| IpAddress::from_str(a).map(|ip| IpCidr::new(ip, if matches!(ip, IpAddress::Ipv4(_)) { 32 } else { 128 })))
                    .map_err(|_| format!("{a}: not an address"))?;
                r.addr = Some(c);
            }
            p => {
                if !matches!(proto, Proto::Tcp | Proto::Udp) {
                    return Err("ports are for tcp and udp".into());
                }
                let (a, b) = match p.split_once('-') {
                    Some((a, b)) => (a, b),
                    None => (p, p),
                };
                let a: u16 = a.parse().map_err(|_| format!("{p}: not a port"))?;
                let b: u16 = b.parse().map_err(|_| format!("{p}: not a port"))?;
                if a > b || a == 0 {
                    return Err(format!("{p}: not a port range"));
                }
                r.ports = Some((a, b));
            }
        }
    }
    Ok(r)
}

/// What a packet is, for the rules
struct Pkt {
    proto: Proto,
    src: IpAddress,
    dst: IpAddress,
    sport: u16,
    dport: u16,
    new: bool, // the start of a conversation (or a request)
}

fn parse(frame: &[u8], ethernet: bool) -> Option<Pkt> {
    let ip = if ethernet {
        if frame.len() < 14 {
            return None;
        }
        let mut et = u16::from_be_bytes([frame[12], frame[13]]);
        let mut off = 14;
        if et == 0x8100 && frame.len() >= 18 {
            et = u16::from_be_bytes([frame[16], frame[17]]);
            off = 18;
        }
        if et != 0x0800 && et != 0x86dd {
            return None; // ARP and the rest: not the firewall's
        }
        &frame[off..]
    } else {
        frame
    };
    if ip.is_empty() {
        return None;
    }
    let (proto_no, src, dst, l4) = match ip[0] >> 4 {
        4 if ip.len() >= 20 => {
            let ihl = ((ip[0] & 15) as usize) * 4;
            // A fragment after the first: no ports to look at; let it go
            // with its first
            if u16::from_be_bytes([ip[6], ip[7]]) & 0x1fff != 0 {
                return None;
            }
            (
                ip[9],
                IpAddress::v4(ip[12], ip[13], ip[14], ip[15]),
                IpAddress::v4(ip[16], ip[17], ip[18], ip[19]),
                ip.get(ihl..)?,
            )
        }
        6 if ip.len() >= 40 => {
            let mut s = [0u8; 16];
            let mut d = [0u8; 16];
            s.copy_from_slice(&ip[8..24]);
            d.copy_from_slice(&ip[24..40]);
            // Extension headers are not followed: the next header as is
            (ip[6], IpAddress::Ipv6(s.into()), IpAddress::Ipv6(d.into()), &ip[40..])
        }
        _ => return None,
    };
    match proto_no {
        6 if l4.len() >= 14 => {
            let flags = l4[13];
            let syn = flags & 0x02 != 0;
            let ack = flags & 0x10 != 0;
            Some(Pkt {
                proto: Proto::Tcp,
                src,
                dst,
                sport: u16::from_be_bytes([l4[0], l4[1]]),
                dport: u16::from_be_bytes([l4[2], l4[3]]),
                new: syn && !ack,
            })
        }
        17 if l4.len() >= 8 => Some(Pkt {
            proto: Proto::Udp,
            src,
            dst,
            sport: u16::from_be_bytes([l4[0], l4[1]]),
            dport: u16::from_be_bytes([l4[2], l4[3]]),
            new: true, // decided with udp_out
        }),
        // ICMP echo request (8), ICMPv6 echo request (128): requests; the
        // rest (answers, errors, neighbour discovery) pass
        1 if !l4.is_empty() => Some(Pkt { proto: Proto::Icmp, src, dst, sport: 0, dport: 0, new: l4[0] == 8 }),
        58 if !l4.is_empty() => Some(Pkt { proto: Proto::Icmp, src, dst, sport: 0, dport: 0, new: l4[0] == 128 }),
        _ => Some(Pkt { proto: Proto::Any, src, dst, sport: 0, dport: 0, new: true }),
    }
}

impl Firewall {
    pub fn new() -> Firewall {
        let mut f = Firewall {
            rules: Vec::new(),
            default_in: true,
            default_out: true,
            enabled: false,
            dropped_in: 0,
            dropped_out: 0,
            udp_out: HashMap::new(),
        };
        f.load();
        f
    }

    /// /etc/firewall.conf (none: everything allowed)
    pub fn load(&mut self) -> Vec<String> {
        let mut errors = Vec::new();
        self.rules.clear();
        self.default_in = true;
        self.default_out = true;
        self.enabled = false;
        let Ok(text) = std::fs::read_to_string(CONF) else { return errors };
        self.enabled = true;
        for (n, line) in text.lines().enumerate() {
            let w: Vec<&str> = line.split('#').next().unwrap_or("").split_whitespace().collect();
            match w.as_slice() {
                [] => {}
                ["default", dir @ ("in" | "out"), act @ ("allow" | "deny")] => {
                    if *dir == "in" {
                        self.default_in = *act == "allow";
                    } else {
                        self.default_out = *act == "allow";
                    }
                }
                ["enabled", v] => self.enabled = *v != "no",
                _ => match parse_rule(&w) {
                    Ok(r) => self.rules.push(r),
                    Err(e) => errors.push(format!("{CONF}:{}: {e}", n + 1)),
                },
            }
        }
        errors
    }

    pub fn save(&self) -> std::io::Result<()> {
        let mut s = String::from("# The firewall (man firewall.conf); written by fwctl save\n");
        s.push_str(&format!("enabled {}\n", if self.enabled { "yes" } else { "no" }));
        s.push_str(&format!("default in {}\n", if self.default_in { "allow" } else { "deny" }));
        s.push_str(&format!("default out {}\n", if self.default_out { "allow" } else { "deny" }));
        for r in &self.rules {
            s.push_str(&r.text());
            s.push('\n');
        }
        std::fs::write(CONF, s)
    }

    fn decide(&mut self, p: &Pkt, inbound: bool, now_s: u64) -> bool {
        let (local_port, other) = if inbound { (p.dport, p.src) } else { (p.sport, p.dst) };
        // UDP: answers to what this computer sent pass
        if p.proto == Proto::Udp {
            if inbound {
                if let Some(t) = self.udp_out.get(&local_port) {
                    if now_s.saturating_sub(*t) < 180 {
                        return true;
                    }
                }
            }
        } else if !p.new {
            return true;
        }
        let port = p.dport; // the service's, either way
        for r in self.rules.iter_mut() {
            if r.inbound != inbound {
                continue;
            }
            if r.proto != Proto::Any && r.proto != p.proto {
                continue;
            }
            if let Some((a, b)) = r.ports {
                if port < a || port > b {
                    continue;
                }
            }
            if let Some(c) = r.addr {
                if !c.contains_addr(&other) {
                    continue;
                }
            }
            r.hits += 1;
            return r.allow;
        }
        if inbound {
            self.default_in
        } else {
            self.default_out
        }
    }

    /// A frame a card received: let it in?
    pub fn allow_in(&mut self, frame: &[u8], now_s: u64) -> bool {
        if !self.enabled {
            return true;
        }
        let Some(p) = parse(frame, true) else { return true };
        let ok = self.decide(&p, true, now_s);
        if !ok {
            self.dropped_in += 1;
        }
        ok
    }

    /// A frame to send: let it out?
    pub fn allow_out(&mut self, frame: &[u8], now_s: u64) -> bool {
        if !self.enabled {
            return true;
        }
        let Some(p) = parse(frame, true) else { return true };
        let ok = self.decide(&p, false, now_s);
        if ok && p.proto == Proto::Udp {
            self.udp_out.insert(p.sport, now_s);
            if self.udp_out.len() > 4096 {
                self.udp_out.retain(|_, t| now_s.saturating_sub(*t) < 180);
            }
        }
        if !ok {
            self.dropped_out += 1;
        }
        ok
    }

    pub fn list(&self) -> String {
        let mut s = format!(
            "enabled {}\ndefault in {} dropped {}\ndefault out {} dropped {}\n",
            if self.enabled { "yes" } else { "no" },
            if self.default_in { "allow" } else { "deny" },
            self.dropped_in,
            if self.default_out { "allow" } else { "deny" },
            self.dropped_out
        );
        for (i, r) in self.rules.iter().enumerate() {
            s.push_str(&format!("rule {} {} hits {}\n", i + 1, r.text(), r.hits));
        }
        s
    }
}
