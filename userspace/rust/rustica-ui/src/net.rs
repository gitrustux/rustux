// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! The network's state for the desktop (network step D6): the panel's
//! "Net" and Settings > Network ask netd's control protocol (/svc/net,
//! docs/NETWORK.md) through the C library's rx_net_control.

extern "C" {
    fn rx_net_control(req: *const u8, out: *mut u8, cap: usize) -> i32;
}

/// One request; its answer without the final "ok", or Err with netd's
/// error ("" when netd cannot be reached)
pub fn control(req: &str) -> Result<String, String> {
    let mut r = req.as_bytes().to_vec();
    r.push(0);
    let mut buf = vec![0u8; 65536];
    let n = unsafe { rx_net_control(r.as_ptr(), buf.as_mut_ptr(), buf.len()) };
    if n < 0 {
        return Err(String::new());
    }
    buf.truncate(n as usize);
    let text = String::from_utf8_lossy(&buf).to_string();
    if let Some(e) = text.strip_prefix("error: ") {
        return Err(e.trim().to_string());
    }
    Ok(text.lines().filter(|l| *l != "ok").map(|l| format!("{l}\n")).collect())
}

#[derive(Clone, Debug, Default)]
pub struct Iface {
    pub name: String,
    pub driver: String,
    pub mac: String,
    /// "up", "no-carrier" or "down"
    pub state: String,
    /// IPv4 addresses with their prefix, and the seconds left on a DHCP
    /// lease
    pub v4: Vec<(String, Option<u64>)>,
    /// IPv6 addresses (link-local ones too), with their prefix
    pub v6: Vec<String>,
    pub gateway: Option<String>,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
}

impl Iface {
    /// The first IPv4 address, without its prefix
    pub fn address(&self) -> Option<&str> {
        self.v4.first().map(|(a, _)| a.split('/').next().unwrap_or(a))
    }
}

/// The cards (not lo), in name order; None when netd is not running
pub fn interfaces() -> Option<Vec<Iface>> {
    let links = control("links").ok()?;
    let addrs = control("addrs").unwrap_or_default();
    let routes = control("routes").unwrap_or_default();
    let mut v = Vec::new();
    // NAME DRIVER MAC STATE mtu N rx F tx F RXBYTES TXBYTES
    for l in links.lines() {
        let w: Vec<&str> = l.split_whitespace().collect();
        if w.len() < 4 || w[0] == "lo" {
            continue;
        }
        let mut i = Iface {
            name: w[0].to_string(),
            driver: w[1].replace('_', " "),
            mac: w[2].to_string(),
            state: w[3].to_string(),
            rx_bytes: w.get(10).and_then(|x| x.parse().ok()).unwrap_or(0),
            tx_bytes: w.get(11).and_then(|x| x.parse().ok()).unwrap_or(0),
            ..Default::default()
        };
        // NAME ADDRESS/PREFIX [dynamic SECONDS|forever]
        for a in addrs.lines() {
            let f: Vec<&str> = a.split_whitespace().collect();
            if f.len() < 2 || f[0] != i.name {
                continue;
            }
            if f[1].contains(':') {
                i.v6.push(f[1].to_string());
            } else {
                i.v4.push((f[1].to_string(), if f.get(2) == Some(&"dynamic") { f.get(3).and_then(|s| s.parse().ok()) } else { None }));
            }
        }
        for r in routes.lines() {
            let f: Vec<&str> = r.split_whitespace().collect();
            if f.len() >= 5 && f[0] == "default" && f[4] == i.name && !f[2].contains(':') {
                i.gateway = Some(f[2].to_string());
            }
        }
        v.push(i);
    }
    Some(v)
}

/// The DNS servers in use (what /etc/resolv.conf says)
pub fn dns_servers() -> Vec<String> {
    std::fs::read_to_string("/etc/resolv.conf")
        .unwrap_or_default()
        .lines()
        .filter_map(|l| l.strip_prefix("nameserver").map(|s| s.trim().to_string()))
        .filter(|s| !s.is_empty())
        .collect()
}

/// The panel's text: "eth0 10.0.2.15", "no cable", "connecting", "off"
pub fn summary(ifs: Option<&[Iface]>) -> String {
    let Some(ifs) = ifs else { return "off".into() };
    if let Some(i) = ifs.iter().find(|i| i.state == "up" && i.address().is_some()) {
        return format!("{} {}", i.name, i.address().unwrap_or(""));
    }
    if ifs.iter().any(|i| i.state == "up") {
        return "connecting".into();
    }
    if ifs.iter().any(|i| i.state == "no-carrier") {
        return "no cable".into();
    }
    if ifs.is_empty() {
        "none".into()
    } else {
        "down".into()
    }
}
