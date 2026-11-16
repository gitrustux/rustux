# Networking

Rustica's network: card drivers in userspace, one network service (`netd`)
with the TCP/IP stack, `ip` and `ping` (`NETWORK.md` in the planning
documents, step D1), POSIX sockets for programs, C and Rust (`nc`,
`ss`, `std::net`; step D2), DHCP, DNS and `curl` over HTTPS (step D3),
Intel's e1000e cards with IPv6 beside IPv4 (step D4), the SSH server
and the firewall (step D5), and the desktop's network settings (step D6).

```
  ping, ip, ss ──/svc/net──────┐                    ┌── virtio-net 00:02.0 (eth0)
                               netd (smoltcp) ──/svc/netdev──┤
  socket() ────/svc/netsock────┘ interfaces, ARP,   └── e1000e 00:03.0 (eth1)
  (C library)   + data sockets   IPv4, IPv6, ICMP,
                                 TCP, UDP, routes,
                                 DHCP, SLAAC, lo
```

The kernel has no network code. A driver only moves Ethernet frames; netd
owns the interfaces, their addresses and routes, and every socket.

## Drivers: /svc/netdev

devmgr starts a network driver from `/etc/drivers` like any PCI driver
(`DRIVERS.md`):

```
1af4:1000   /bin/virtio-net     # transitional virtio-net (QEMU's default)
1af4:1041   /bin/virtio-net     # modern-only (disable-legacy=on)
8086:10d3   /bin/e1000e         # Intel 82574L (QEMU's -device e1000e)
8086:0d4f   /bin/e1000e         # Intel I219-V, and the other I217/I218/I219s
```

The driver connects to `/svc/netdev`, which netd publishes, and keeps
trying every half second until netd is there (and again whenever netd
goes away: `svc restart netd` loses nothing but the running changes). The
protocol is `<rustux/netdev.h>`: each message is `rx_netdev_msg_t { u32
op; u32 a; }` and its data.

| Op | Direction | `a` | Data |
|----|-----------|-----|------|
| `HELLO` 1 | driver → netd, first | MTU | `rx_netdev_hello_t`: MAC address, link (1 up), driver name and PCI slot (`"virtio-net 00:03.0"`) |
| `FRAME` 2 | both ways | — | one Ethernet frame without the FCS, at most `RX_NETDEV_MAX_FRAME` (1518) bytes |
| `LINK` 3 | driver → netd | 1 up, 0 down | — |

A frame per message is simple and fast enough for now (a shared ring
between driver and netd can come later without changing netd's stack). A
driver drops frames it cannot queue; TCP sends them again.

### virtio-net

`/bin/virtio-net BDF` (`c-progs/virtio-net.c`): virtio 1.0 over PCI
(modern registers; the transitional device's are the same), features
`VERSION_1`, `MAC` and `STATUS` only: no checksum or segmentation
offloads, netd computes everything. A receive queue and a transmit queue
of 64 buffers of 2 KiB (the 12-byte virtio-net header, then the frame).
Interrupts by MSI-X vector 0 on a thread that drains the receive queue
(and polling every millisecond if there is no MSI-X); the link status is
read every second. The main thread reads netd's channel and transmits.

### e1000e (step D4)

`/bin/e1000e BDF` (`c-progs/e1000e.c`): Intel's PCI Express gigabit
cards with the e1000e register set: the 82574L that QEMU emulates
(`-device e1000e`), and the I217, I218 and I219 built into laptop and
desktop chipsets (their IDs are in `/etc/drivers`). A reset, the MAC
address from the receive-address registers (or the NVM through `EERD`),
legacy descriptor rings of 256 for each direction with 2 KiB buffers,
broadcast and every multicast address taken (IPv6's neighbour discovery
needs them), the FCS stripped by the card and added on sending. MSI on a
thread that drains the receive ring (polling without it); the link
(`STATUS.LU`) on the link-change interrupt and every second. **Tested in
QEMU only:** on the I219 of a laptop the PHY may first need waking from
its low-power state (ULP) and the LANPHYPC toggle that Linux's e1000e
does; that is for the laptop session.

## netd

`/bin/netd` (`rust/netd`, Rust, on [smoltcp](https://github.com/smoltcp-rs/smoltcp)
0.14, `/etc/svc/netd.svc`, target multi-user, `ready=/svc/net`):

- **Interfaces**: one smoltcp interface per card, named `eth0`, `eth1`,
  ... by the card's place among the network controllers in PCI order (it
  asks devmgr for the list; `lspci`), so the names do not depend on which
  driver started first. A card without a PCI slot gets the next name after
  those. `lo` is the loopback interface (a smoltcp interface whose frames
  go round again): 127.0.0.0/8, and a copy (/32) of every other
  interface's address, so traffic to this computer's own addresses stays
  inside, as on Linux.
- **Addresses and routes**: from `/etc/network.conf` when the card
  appears (below), changed with `ip`. Several addresses per interface (16
  at most), one default route.
- **IPv4, ARP, ICMP**: netd answers ARP and echo requests (pings) on every
  interface, and sends pings for `ping`.
- **IPv6** (step D4): each card gets its link-local address (fe80::/64
  from the MAC address, EUI-64) and, from routers' advertisements
  (smoltcp's SLAAC), global addresses and an IPv6 default route; lo has
  ::1. Neighbour discovery, ICMPv6 echo both ways (`ping -6`), TCP and UDP
  sockets. `/etc/network.conf`'s `NAME static6 ADDRESS/PREFIX [GATEWAY]`
  and `ip addr add fd00::1/64 dev eth0` add addresses by hand. No DHCPv6.
- **Several interfaces**: each has its own addresses and routes; a
  destination in an interface's subnet goes out there, others through the
  first interface with a default route (DHCP may give each one; `ip
  route` lists them all, the first one counts).
- **TCP and UDP sockets** for programs (below).
- One thread: it waits (`wait_many`) on the three listeners, the drivers'
  channels, the clients' and the sockets', until a frame, a request, a
  socket's bytes or smoltcp's next timer.

### /etc/network.conf

```
# NAME dhcp | NAME static ADDRESS/PREFIX [GATEWAY] | NAME off
# dns ADDRESS...   search DOMAIN...
eth0 dhcp
```

A card without a line gets DHCP; the image's file says `eth0 dhcp`.
`netcfg` changes it and applies it at once (step D6); after editing it by
hand, `sudo netcfg apply` (netd's `reconfigure` request: the card's
addresses, routes and DHCP are set again from the file, other cards and
open connections are left alone) or `sudo svc restart netd`.

### DHCP (step D3)

smoltcp's DHCPv4 client, one per interface configured `dhcp`: the
address, the default route (unless one is set already) and up to three
DNS servers, renewed and rebound as the lease runs out (and taken away if
it is lost). The requests carry the host name (`/etc/hostname`, option
12), so a router can list the computer by name. `ip addr` shows the
address as `dynamic` with its `valid_lft`, `ip route` the route as `proto
dhcp`; netd logs each lease (`logs -u netd`).

netd writes `/etc/resolv.conf` from the `dns` and `search` lines of
`network.conf` and DHCP's servers, with a first line `# Written by
netd`; a file without that line is the administrator's own and is left
alone (`man resolv.conf`).

### DNS (step D3)

The resolver is the C library's (`libc/src/resolv.c`, `<resolv.h>`), as
on Unix: `getaddrinfo`, `getnameinfo` and `gethostbyname` look in
`/etc/hosts` first, then ask the servers of `/etc/resolv.conf` over UDP
(each in turn, `timeout`, `attempts`), and over TCP when the answer is
truncated; short names get the `search` domains (`ndots`). A and AAAA
records (IPv4 addresses first); CNAMEs followed by the server. `res_query`,
`res_search` and `dn_expand` are there for programs that read DNS
messages themselves, such as `host`. Rust's `std::net` and everything on
it resolve through `getaddrinfo`.

### The control protocol: /svc/net

Text, one request per message, the answer one message whose last line is
`ok` or `error: ...` (`c-progs/netclient.h` is the client):

| Request | Answer lines | Who |
|---------|--------------|-----|
| `links` | `NAME DRIVER MAC up\|down\|no-carrier mtu N rx PACKETS tx PACKETS RXBYTES TXBYTES` (spaces in DRIVER as `_`) | anyone |
| `addrs` | `NAME ADDRESS/PREFIX` | anyone |
| `routes` | `default via GW dev NAME`, `NET/PREFIX dev NAME src ADDRESS` | anyone |
| `addr add\|del ADDRESS/PREFIX dev NAME` | — | root |
| `route add default via GW`, `route del default` | — | root |
| `link set NAME up\|down` | — | root |
| `ping ADDRESS SEQ TIMEOUT_MS SIZE` | `reply FROM SEQ BYTES MICROSECONDS` or `timeout` (no `ok`) | anyone |

netd reads who sent a request from the channel (the sender's credentials,
`rx_channel_read_creds`): changes need an effective uid of 0. Anyone may
ping, as Linux's unprivileged ICMP sockets allow.

## Sockets (step D2)

The C library's `socket`, `bind`, `listen`, `accept`/`accept4`,
`connect`, `send`/`recv`/`sendto`/`recvfrom`/`sendmsg`/`recvmsg` (no
ancillary data), `read`/`write`, `shutdown`, `getsockname`,
`getpeername`, `setsockopt`/`getsockopt`, `poll` and `select`,
non-blocking sockets (`SOCK_NONBLOCK`, `O_NONBLOCK`, `FIONBIO`,
`MSG_DONTWAIT`) and non-blocking `connect` (`EINPROGRESS`, then `poll`
for `POLLOUT` and `SO_ERROR`), `SO_RCVTIMEO`/`SO_SNDTIMEO`, `MSG_PEEK`,
`MSG_WAITALL`; `inet_pton`/`inet_ntop` (IPv4 and IPv6), `inet_aton`,
`htons` and friends; `getaddrinfo`, `getnameinfo`, `gethostbyname`,
`getservbyname` for numeric addresses, `localhost`, `/etc/hosts` and the
usual service names (names on the network need DNS, step D3).
`socketpair(AF_UNIX, SOCK_STREAM)` gives two connected pipe-like fds (a
kernel socket pair). Layouts and numbers are Linux's (musl's), so Rust's
`std::net` (`TcpStream`, `TcpListener`, `UdpSocket`, `connect_timeout`,
`to_socket_addrs`) works unchanged: `nettest` checks it. IPv6 too since
step D4: an `AF_INET6` socket is dual-stack, as Linux's by default (one
bound to `::` takes IPv4 connections too, their addresses shown as
`::ffff:a.b.c.d`); an `AF_INET` socket is IPv4 only. `IPV6_V6ONLY` is
accepted but has no effect yet.

**How a socket works.** `socket()` connects to `/svc/netsock`: that
channel is the socket's control channel to netd (`<rustux/netsock.h>`).
`bind`, `listen` and `connect` are requests on it. A TCP connection's
bytes travel on a kernel socket pair besides, the *data socket*: the
program's fd holds one end, netd the other, and copies between it and
smoltcp's TCP socket. The kernel buffers 64 KiB each way, which is the
flow control (a slow reader stops a fast sender at both ends), and
`read`, `write` and `poll` on the fd are the kernel's, as for a pipe. A
half-close is `RX_SOCKET_SHUTDOWN_WRITE` on the data socket (a kernel
addition of this step, `SYSCALL.md`): `shutdown(SHUT_WR)` makes netd send
a FIN, and the peer's FIN reaches the program as end-of-stream while it
can still write. A listening socket's new connections arrive as
`ACCEPTED` messages on its control channel, each with the new data
socket and control channel; `accept` takes one (so the backlog is
netd's: up to 8 handshakes at once per interface, plus the connections
waiting to be accepted). UDP datagrams are messages on the control
channel both ways (up to 65000 bytes). A reset reaches the program as
`ECONNRESET` on its next `read` or `write`.

**Ports.** `bind` to port 0 (or none) takes a free port from 49152 up;
ports below 1024 are root's (`EACCES`), checked by the credentials of
the message. `EADDRINUSE` when a port is taken; with `SO_REUSEADDR` only
listeners and bound sockets count. A socket bound to 0.0.0.0 is served
on every interface (lo too), one bound to an address on its interface
and lo.

| Socket option | |
|---------------|-|
| `SO_REUSEADDR`, `SO_KEEPALIVE` (75 s), `TCP_NODELAY` | passed to netd |
| `SO_RCVTIMEO`, `SO_SNDTIMEO` | in the library |
| `SO_ERROR`, `SO_TYPE`, `SO_ACCEPTCONN`, `SO_RCVBUF`/`SO_SNDBUF` (64 KiB) | read |
| `SO_BROADCAST`, `SO_LINGER`, `SO_REUSEPORT`, `IP_TTL`, ... | accepted, no effect |

Not yet: broadcast and multicast, raw sockets, Unix-domain
sockets with names, passing sockets to other programs (spawned programs
get fds 0-2 only, as before).

## The firewall (step D5)

In netd (`rust/netd/src/fw.rs`): each frame a card gives netd, and each
one netd sends, is looked at before smoltcp and the wire (lo is never
filtered). Rules decide about **new conversations** only: a TCP
connection's first packet (SYN without ACK), a UDP datagram that is not
an answer, an ICMP or ICMPv6 echo request. The rest passes: TCP packets
with ACK (connections already made, and the answers to this computer's
own), UDP datagrams to a local port this computer sent from in the last
3 minutes (DNS and DHCP answers among them), ICMP answers and errors,
IPv6's neighbour discovery and router advertisements, ARP. So it is
stateful enough for a computer's own firewall, without a connection
table; a forged non-SYN TCP packet gets through to smoltcp, which drops
or resets it as for no connection.

```
allow|deny in|out tcp|udp|icmp|any [PORT[-PORT]] [from|to ADDRESS[/PREFIX]]
default in|out allow|deny
```

First match wins, then the direction's default. `/etc/firewall.conf` at
netd's start (`man firewall.conf`); `fwctl` shows and changes the
running rules (`fw ...` requests on `/svc/net`, root's) and `fwctl save`
writes them back. The image's rules deny new connections in but SSH's
and pings, and allow everything out:

```
enabled yes
default in deny
default out allow
allow in tcp 22
allow in icmp
```

`fwctl allow tcp 8080` puts a rule first (it wins over the others);
`fwctl` lists the rules with their hits and the packets dropped.

## SSH (step D5)

`sshd` (Rust, `rust/sshd`, the service `sshd`): the SSH protocol is the
[sunset](https://github.com/mkj/sunset) crate's (0BSD, by Dropbear's
author), vendored in `rust/vendor/sunset` with what a server needed: the
exit status, EOF and close of a session, the pty request's terminal and
size, window changes, and a window and packets larger than its embedded
defaults. curve25519-sha256 or mlkem768x25519 key exchange, an Ed25519
host key, chacha20-poly1305 or aes256-ctr; OpenSSH's clients connect.

- **Listening:** `[::]:22` (IPv4 too), `/etc/ssh/sshd_config` for the
  port, addresses and the rest (`man sshd_config`); it listens again when
  netd restarts.
- **The host key:** `/etc/ssh/ssh_host_ed25519_key` (Rustux's own form:
  the 32-byte seed in hexadecimal, mode 600) and `.pub` in OpenSSH's,
  made at the first start; its fingerprint is logged.
- **Logging in:** passwords through `sshd-session --check` (C, the same
  user database code as `login`: `/etc/shadow`); keys from
  `~/.ssh/authorized_keys` (Ed25519 only; options before the key type
  are not supported). root with a key only by default
  (`PermitRootLogin prohibit-password`); accounts with no password or a
  locked one cannot log in with a password. 6 failures close the
  connection.
- **Sessions:** `sshd-session USER [COMMAND]` runs the user's shell (or
  `shell -c COMMAND`) acting as the user (`rx_spawn_as`), in the home
  directory, with the login's environment, `TERM` when there is a
  terminal, `SSH_CONNECTION` and `SSH_CLIENT`; on a pseudo-terminal
  (step F7's) when the client asks for one, else on pipes (stdout and
  stderr apart). The exit status goes back to the client.
- **One thread per connection**, a loop over `poll()` on the socket and a
  pipe that the session's reader threads write to when the programs print.

Not yet: scp and sftp (subsystems), port and agent forwarding, RSA and
ECDSA user keys, environment from the client (`AcceptEnv`).

## The desktop (step D6)

- **The panel** shows `Net: eth0 10.0.2.15` (the first card that is up
  with an address), `connecting` (up, no address yet), `no cable`, `down`
  or `off` (netd not running), asked again every 3 seconds; a click opens
  Settings > Network.
- **Settings > Network**: the cards (state, address, gateway), the one
  selected with its driver, MAC address, traffic, DHCP lease and IPv6
  addresses; Automatic (DHCP), Fixed address (address/prefix and gateway,
  starting from what DHCP gave) or Off, then Apply; DNS servers of your
  own, tried before DHCP's. The changes are `netcfg` commands, as root
  (another user types their password: `sudo -S`).
- Programs that are not C tools ask netd with the C library's
  `rx_net_control(request, out, size)` (`<rustux/netsock.h>`; the
  toolkit's `rustica_ui::net` for Rust).

## Commands

- `ip` (`man ip`): `ip link`, `ip addr`, `ip route` in iproute2's form;
  `-s` counters, `-br` one line per interface; `addr add|del`, `route
  add|del default`, `link set up|down` as root.
- `ping` (`man ping`): `-c`, `-i`, `-W`, `-w`, `-s`, `-q`; the summary
  with packet loss and min/avg/max/mdev; exit status 0, 1 (no reply) or 2
  (an error such as no route).
- `nc` (`man nc`, OpenBSD netcat's form): `nc HOST PORT` connects, `nc
  -l PORT` listens (`-k` again and again); `-u` UDP, `-v`, `-z` (only
  connect), `-w`, `-s`, `-p`, `-N`. Standard input goes out, what comes
  in goes to standard output; at the end of the input nc half-closes.
- `ss` (`man ss`): netd's sockets, `-t`, `-u`, `-l`, `-a`, in iproute2's
  columns (`sockets` on `/svc/net`).
- `host` and `nslookup` (`man host`): A, AAAA, MX by default, `-t
  TYPE`, `-v`, a server to ask; addresses backwards (PTR).
- `curl` (`man curl`, Rust: ureq on rustls with ring, Mozilla's root
  certificates built in through webpki-roots): HTTP and HTTPS, `-o -O -L
  -I -i -f -s -S -v -k -H -d -X -A -u -m -w --cacert --connect-timeout`,
  gzip; curl's exit statuses. The clock must be right for certificates
  (the UEFI clock, `timedatectl`).
- `ping` takes names too, and IPv6 addresses (`-6`, `-4`; `ping6`).
- `ip -6 addr`, `ip -6 route`, `ip -6 route add default via ADDRESS`;
  `nc -6`, `host -t AAAA`.
- `netcfg` (`man netcfg`): `/etc/network.conf` changed and applied:
  `netcfg eth0 dhcp`, `netcfg eth0 static 192.168.1.20/24 192.168.1.1`,
  `netcfg eth0 off`, `netcfg dns 1.1.1.1`, `netcfg search lan`, `netcfg
  apply`; `netcfg` alone shows each card's configuration and state.
- `fwctl` (`man fwctl`): the firewall's rules.
- `sshd` (`man sshd`), `sshd-session` (sshd's helper).
- `nettest` (Rust, `rust/nettest`): `nettest local` checks `std::net`
  inside the computer; `connect`, `serve`, `udp` for tests across the
  network.

```
$ ip -br a
lo               UP             127.0.0.1/8
eth0             UP             10.0.2.15/24
$ ping -c 2 10.0.2.2
PING 10.0.2.2 (10.0.2.2) 56(84) bytes of data.
64 bytes from 10.0.2.2: icmp_seq=1 time=1.445 ms
64 bytes from 10.0.2.2: icmp_seq=2 time=0.957 ms

--- 10.0.2.2 ping statistics ---
2 packets transmitted, 2 received, 0% packet loss, time 1004ms
rtt min/avg/max/mdev = 0.957/1.201/1.445/0.244 ms
```

## In QEMU

```bash
qemu-system-x86_64 ... -netdev user,id=n0 -device virtio-net-pci,netdev=n0
```

QEMU's user-mode network has the gateway at 10.0.2.2 (it answers pings),
DNS at 10.0.2.3 and gives 10.0.2.15 by DHCP; for IPv6 it advertises
fec0::/64 from fe80::2 (fec0::2 answers pings). `-device e1000e` instead
of virtio-net-pci gives the Intel card. Pings beyond
it reach the outside world only if the host lets QEMU use unprivileged
ICMP sockets (Linux's `net.ipv4.ping_group_range`). The host is 10.0.2.2 from the guest: a server listening
on the host's 127.0.0.1 is reached there (`nc 10.0.2.2 PORT`). The host
reaches the guest only through forwarded ports:

```bash
-netdev user,id=n0,hostfwd=tcp:127.0.0.1:2222-:22,hostfwd=tcp:127.0.0.1:8080-:8080
```

The network test reaches the guest at the frame level on a second card
too, a `-netdev dgram` "wire" to a peer written in Python
(`docs/TESTS.md`, network test).

Proxmox and KVM use the same virtio-net device (or an "Intel E1000E"
card); the laptop's Intel I219 has the e1000e driver, untested there
yet.
