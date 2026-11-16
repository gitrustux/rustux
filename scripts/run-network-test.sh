#!/bin/bash
# The network test (NETWORK.md step D1, docs/TESTS.md). Boots Rustux in
# QEMU with two virtio-net cards and autologin as root:
#   eth0  a transitional virtio-net (1af4:1000) on QEMU's user-mode network
#         (-netdev user): 10.0.2.15 from /etc/network.conf, the gateway
#         10.0.2.2 answers pings;
#   eth1  a modern-only one (1af4:1041) on a "wire" to this script
#         (-netdev dgram: each Ethernet frame a UDP datagram), where a peer
#         at 10.0.3.1 written in Python answers ARP and pings, and pings
#         the guest itself.
# It checks netd's log, `ip addr`/`ip route`/`ip link`, `ping` to the
# gateway, to a missing address (no answer, exit status 1) and to the
# loopback address, adding and removing addresses and the default route,
# that only root may change them, the peer's pings of the guest, and a
# restart of netd (the drivers connect again; /etc/network.conf again).
# Sockets (step D2): `nettest local` (Rust's std::net inside the guest:
# TCP on 127.0.0.1 and the guest's own address, 300000 bytes with a
# half-close, refused, in use, time-outs, non-blocking connect, UDP,
# localhost); the guest's nc and nettest reach TCP and UDP servers this
# script runs on the host (10.0.2.2 from the guest); the host reaches the
# guest's nc -l and nettest serve through QEMU's port forwarding
# (hostfwd); ss lists a listener; nobody may not listen on port 80.
# DHCP, DNS and the web (step D3): eth0's address, route and DNS server
# from QEMU's DHCP server (/etc/resolv.conf written by netd), the host name
# in eth1's DHCP requests; a DNS server this script runs on the host's
# port 53 (A, CNAME, PTR, NXDOMAIN, an answer too long for UDP): host,
# nslookup, the search domain, ping and Rust by name; curl over HTTP
# (redirects, -I, -f, -d, -o, -w) and HTTPS with a test CA the script
# makes (refused without --cacert, accepted with it, -k). The DNS checks
# are skipped if port 53 cannot be had on the host.
# Real network cards and IPv6 (step D4): a third card, an Intel e1000e
# (eth2), on a second user-mode network (10.0.5.0/24, IPv6 fd05::/64):
# its DHCP lease, ping and curl through it; IPv6 on eth0 and eth2:
# link-local and SLAAC addresses, the router's default route (proto ra),
# ping -6 to QEMU's router on both networks and to ::1, an address added
# by hand, and nc over IPv6 (nettest local's IPv6 checks run above).
# SSH and the firewall (step D5): the image's firewall (new connections
# in denied but SSH's and pings; the tests above open 8080 and 8081 with
# fwctl), `fwctl deny tcp 8080` keeps the host's connection from nc -l
# 8080 and `fwctl delete 1` lets it in again; from the host (OpenSSH's
# ssh, through a forwarded port), a key login runs a command (its output
# and exit status), a session on a pseudo-terminal, a user's password
# login (useradd, chpasswd), root's password refused (prohibit-password),
# a wrong key refused, and 1 MB each way. The SSH checks are skipped
# without ssh and ssh-keygen on the host.
# Exit status 0 if all is well.
#
#   scripts/run-network-test.sh [KERNEL.efi]
#
# Needs qemu-system-x86_64 (7.2 or later, for dgram), OVMF, mtools, python3,
# openssl; ssh and ssh-keygen (OpenSSH's client) for the SSH checks.
set -u
cd "$(dirname "$0")/.."
EFI=${1:-target/x86_64-unknown-uefi/release/rustux.efi}
[ -f "$EFI" ] || { echo "no $EFI (build it first)"; exit 2; }
OVMF=""
for p in /usr/share/ovmf/OVMF.fd /usr/share/qemu/OVMF.fd /usr/share/edk2-ovmf/x64/OVMF_CODE.fd /usr/share/OVMF/OVMF_CODE.fd; do
    [ -f "$p" ] && OVMF=$p && break
done
[ -n "$OVMF" ] || { echo "OVMF firmware not found"; exit 2; }
export PATH=$PATH:/sbin:/usr/sbin
for t in qemu-system-x86_64 mformat mmd mcopy python3 openssl; do
    command -v $t >/dev/null || { echo "$t is needed"; exit 2; }
done
W=$(mktemp -d)
trap 'rm -rf "$W"' EXIT

dd if=/dev/zero of="$W/boot.img" bs=1M count=64 status=none
mformat -i "$W/boot.img" -F ::
mmd -i "$W/boot.img" ::/EFI ::/EFI/BOOT
mcopy -i "$W/boot.img" "$EFI" ::/EFI/BOOT/BOOTX64.EFI
echo "autologin=root" > "$W/cmdline.txt"
# A test CA and a certificate for www.test.rustica from it (curl's HTTPS)
openssl req -x509 -newkey rsa:2048 -nodes -days 2 -subj "/CN=Rustica test CA" -keyout "$W/ca.key" -out "$W/ca.pem" \
    -addext basicConstraints=critical,CA:TRUE -addext keyUsage=critical,keyCertSign,cRLSign 2>/dev/null
openssl req -newkey rsa:2048 -nodes -subj "/CN=www.test.rustica" -keyout "$W/www.key" -out "$W/www.csr" 2>/dev/null
printf 'subjectAltName=DNS:www.test.rustica\nbasicConstraints=CA:FALSE\nextendedKeyUsage=serverAuth\n' > "$W/ext.cnf"
openssl x509 -req -in "$W/www.csr" -CA "$W/ca.pem" -CAkey "$W/ca.key" -CAcreateserial -days 2 -extfile "$W/ext.cnf" -out "$W/www.pem" 2>/dev/null
[ -s "$W/www.pem" ] || { echo "openssl could not make the test certificates"; exit 2; }
mcopy -i "$W/boot.img" "$W/cmdline.txt" ::/EFI/BOOT/cmdline.txt

python3 - "$W" "$OVMF" <<'PY'
import os, re, socket, ssl, struct, subprocess, sys, threading, time
import http.server
W, OVMF = sys.argv[1], sys.argv[2]
def free_port(kind=socket.SOCK_STREAM):
    t = socket.socket(socket.AF_INET, kind); t.bind(('127.0.0.1', 0)); p = t.getsockname()[1]; t.close(); return p
FWD_NC, FWD_RUST, FWD_SSH = free_port(), free_port(), free_port()   # host ports forwarded to the guest's 8080, 8081, 22
LOG, MON = W + '/console.log', W + '/mon'

# The wire: QEMU sends eth1's frames to UDP port B and listens on port A
wire = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
wire.bind(('127.0.0.1', 0))
B = wire.getsockname()[1]
s2 = socket.socket(socket.AF_INET, socket.SOCK_DGRAM); s2.bind(('127.0.0.1', 0)); A = s2.getsockname()[1]; s2.close()

qemu = subprocess.Popen(['qemu-system-x86_64', '-machine', 'q35', '-m', '512M', '-smp', '2', '-bios', OVMF,
    '-drive', f'file={W}/boot.img,format=raw', '-display', 'none', '-debugcon', f'file:{LOG}', '-no-reboot',
    '-monitor', f'unix:{MON},server,nowait',
    '-netdev', f'user,id=n0,hostfwd=tcp:127.0.0.1:{FWD_NC}-:8080,hostfwd=tcp:127.0.0.1:{FWD_RUST}-:8081,hostfwd=tcp:127.0.0.1:{FWD_SSH}-:22', '-device', 'virtio-net-pci,netdev=n0,mac=52:54:00:12:34:56',
    '-netdev', f'dgram,id=n1,local.type=inet,local.host=127.0.0.1,local.port={A},remote.type=inet,remote.host=127.0.0.1,remote.port={B}',
    '-device', 'virtio-net-pci,netdev=n1,disable-legacy=on,mac=52:54:00:ab:cd:ef',
    '-netdev', 'user,id=n2,net=10.0.5.0/24,ipv6-prefix=fd05::,ipv6-prefixlen=64', '-device', 'e1000e,netdev=n2,mac=52:54:00:00:00:e1'],
    stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
for _ in range(50):
    if os.path.exists(MON): break
    time.sleep(0.1)

def log():
    try: return open(LOG, 'rb').read().decode('latin1')
    except OSError: return ''
def wait(pat, t, start=0):
    for _ in range(int(t * 10)):
        if re.search(pat, log()[start:]): return True
        time.sleep(0.1)
    return False
KEYS = {' ': 'spc', '\n': 'ret', '-': 'minus', '=': 'equal', ';': 'semicolon', "'": 'apostrophe', '\\': 'backslash', ',': 'comma', '.': 'dot', '/': 'slash', '[': 'bracket_left', ']': 'bracket_right', '`': 'grave_accent'}
SHIFT = {'|': 'backslash', '>': 'dot', '<': 'comma', '"': 'apostrophe', '_': 'minus', ':': 'semicolon', '$': '4', '?': 'slash', '&': '7', '*': '8',
         '%': '5', '{': 'bracket_left', '}': 'bracket_right', '#': '3', '@': '2', '!': '1', '(': '9', ')': '0', '^': '6', '~': 'grave_accent', '+': 'equal'}
def keys(text):
    c = socket.socket(socket.AF_UNIX); c.connect(MON)
    for ch in text:
        k = ('shift-' + ch.lower()) if ch.isupper() else ch if ch.isalnum() else KEYS.get(ch) or ('shift-' + SHIFT[ch])
        c.sendall(f'sendkey {k}\n'.encode()); time.sleep(0.02)
    time.sleep(0.2); c.close()
DONE = 0
def run(cmd, t=15):
    """A command at the shell; its output (up to the marker it echoes)"""
    global DONE
    DONE += 1
    st = len(log())
    sep = ' ' if cmd.rstrip().endswith('&') else '; '
    keys(f'{cmd}{sep}echo "=DONE{DONE}= $?"\n')
    wait(rf'=DONE{DONE}= \d+', t, st)
    out = log()[st:]
    return out

fails = 0
def check(ok, what, out=''):
    global fails
    print(('ok    ' if ok else 'FAIL  ') + what)
    if not ok:
        fails += 1
        if out: print('      ' + out.strip().replace('\n', '\n      ')[-1500:])

# ---- the peer at 10.0.3.1 ----
PMAC = bytes.fromhex('020000000001')
PIP = bytes([10, 0, 3, 1])
GIP = bytes([10, 0, 3, 15])
guest_mac = None
replies = {}     # our echo requests answered: seq -> payload
dhcp_names = []  # host names (option 12) in eth1's DHCP requests
echo_answered = 0
def csum(b):
    if len(b) % 2: b += b'\0'
    s = sum(struct.unpack('!%dH' % (len(b) // 2), b))
    while s >> 16: s = (s & 0xffff) + (s >> 16)
    return (~s) & 0xffff
def ip_packet(dst, proto, payload):
    h = struct.pack('!BBHHHBBH4s4s', 0x45, 0, 20 + len(payload), 0x1234, 0, 64, proto, 0, PIP, dst)
    h = h[:10] + struct.pack('!H', csum(h)) + h[12:]
    return h + payload
def send(dst_mac, ethertype, payload):
    wire.sendto(dst_mac + PMAC + struct.pack('!H', ethertype) + payload, ('127.0.0.1', A))
def arp(op, tmac, tip):
    return struct.pack('!HHBBH6s4s6s4s', 1, 0x0800, 6, 4, op, PMAC, PIP, tmac, tip)
def peer():
    global guest_mac, echo_answered
    while True:
        try: f, _ = wire.recvfrom(4096)
        except OSError: return
        if len(f) < 14: continue
        et = struct.unpack('!H', f[12:14])[0]
        p = f[14:]
        if et == 0x0806 and len(p) >= 28:
            op, smac, sip, tip = struct.unpack('!H', p[6:8])[0], p[8:14], p[14:18], p[24:28]
            if op == 1 and tip == PIP:
                send(smac, 0x0806, arp(2, smac, sip))
            elif op == 2 and sip == GIP:
                guest_mac = smac
        elif et == 0x0800 and len(p) >= 28 and p[9] == 17 and p[(p[0] & 15) * 4 + 2:(p[0] & 15) * 4 + 4] == b'\x00\x43':
            # A DHCP request from eth1: its options (after the 240-byte header)
            d = p[(p[0] & 15) * 4 + 8:]
            o = 240
            while o + 1 < len(d) and d[o] != 255:
                if d[o] == 0: o += 1; continue
                if d[o] == 12: dhcp_names.append(d[o + 2:o + 2 + d[o + 1]].decode('latin1'))
                o += 2 + d[o + 1]
        elif et == 0x0800 and len(p) >= 28 and p[9] == 1:
            ihl = (p[0] & 15) * 4
            src, dst, icmp = p[12:16], p[16:20], p[ihl:]
            if dst != PIP: continue
            if icmp[0] == 8:   # echo request: answer it
                r = b'\0\0\0\0' + icmp[4:]
                r = r[:2] + struct.pack('!H', csum(r)) + r[4:]
                send(f[6:12], 0x0800, ip_packet(src, 1, r))
                echo_answered += 1
            elif icmp[0] == 0 and csum(icmp) == 0:   # echo reply to us
                seq = struct.unpack('!H', icmp[6:8])[0]
                replies[seq] = icmp[8:]
threading.Thread(target=peer, daemon=True).start()

try:
    if not wait(r'rustux.*> ', 120):
        check(False, 'boot to the root shell', log()[-2000:]); raise SystemExit
    time.sleep(2)
    out = run('logs | grep netd:')
    check('netd: started' in out and re.search(r'eth0: 52:54:00:12:34:56 \(virtio-net [0-9a-f:.]+\), link up', out)
          and re.search(r'eth1: 52:54:00:ab:cd:ef \(virtio-net', out), 'netd took both cards (eth0, eth1)', out)
    check('eth0: DHCP lease 10.0.2.15/24, router 10.0.2.2, DNS 10.0.2.3' in out, 'eth0 configured by DHCP (/etc/network.conf: eth0 dhcp)', out)
    out = run('ip addr')
    check('inet 10.0.2.15/24 scope global dynamic eth0' in out and re.search(r'valid_lft \d+sec', out) and 'inet 127.0.0.1/8 scope host lo' in out
          and re.search(r'3: eth1: <BROADCAST,MULTICAST,UP,LOWER_UP> mtu 1500 state UP', out), 'ip addr', out)
    out = run('ip route; ip -br link')
    check('default via 10.0.2.2 dev eth0' in out and '10.0.2.0/24 dev eth0 src 10.0.2.15' in out
          and re.search(r'eth1 +UP +52:54:00:ab:cd:ef', out), 'ip route, ip -br link', out)
    # netcfg (step D6): the configuration and state; a change and back
    out = run('netcfg')
    check('eth0: dhcp, up (virtio-net' in out and re.search(r'address  10\.0\.2\.15/24 \(DHCP, \d+ s left\)', out)
          and 'gateway  10.0.2.2' in out and 'DNS: 10.0.2.3 (DHCP on eth0)' in out, 'netcfg shows the cards', out)
    out = run('netcfg eth1 static 10.0.4.9/24; ip -br a show eth1; netcfg eth1 bogus; netcfg eth1 static 10.0.4.9; echo st=$?')
    check('netcfg: eth1 static 10.0.4.9/24' in out and '10.0.4.9/24' in out.split('ip -br a show eth1', 1)[-1] and 'usage:' in out
          and 'not an IPv4 address with its prefix' in out and 'st=1' in out, 'netcfg eth1 static: applied at once; bad ones refused', out)
    out = run('netcfg eth1 off; ip -br a show eth1; grep eth1 /etc/network.conf; sudo -u nobody netcfg eth1 dhcp; echo st=$?; netcfg eth1 dhcp')
    check('10.0.4.9' not in out.split('ip -br a show eth1', 1)[-1] and '\neth1 off' in out and 'needs root' in out and 'st=1' in out,
          'netcfg eth1 off; not for nobody', out)
    out = run('ping -c 3 10.0.2.2')
    check(re.search(r'64 bytes from 10.0.2.2: icmp_seq=3 time=[\d.]+ ms', out) and '3 packets transmitted, 3 received, 0% packet loss' in out
          and '=DONE' in out and re.search(r'=DONE\d+= 0', out), 'ping 10.0.2.2 (the gateway): 3 replies', out)
    out = run('ping -c 2 -W 0.5 10.0.2.99')
    check('2 packets transmitted, 0 received, 100% packet loss' in out and re.search(r'=DONE\d+= 1', out), 'ping 10.0.2.99: no answer, status 1', out)
    out = run('ping -c 1 -s 1000 localhost')
    check('1008 bytes from 127.0.0.1: icmp_seq=1' in out, 'ping localhost', out)
    out = run('sudo -u nobody ip addr add 10.0.3.15/24 dev eth1')
    check('only root changes addresses' in out and re.search(r'=DONE\d+= 2', out), 'only root changes addresses', out)
    out = run('ip addr add 10.0.3.15/24 dev eth1; ip -br addr show eth1')
    check(re.search(r'eth1 +UP +10.0.3.15/24', out), 'ip addr add', out)

    # The peer pings the guest
    deadline = time.time() + 10
    while guest_mac is None and time.time() < deadline:
        send(b'\xff' * 6, 0x0806, arp(1, b'\0' * 6, GIP)); time.sleep(0.5)
    check(guest_mac == bytes.fromhex('5254 00ab cdef'.replace(' ', '')), 'the guest answers ARP for 10.0.3.15')
    if guest_mac:
        for seq in range(1, 4):
            data = bytes(range(seq, seq + 48))
            icmp = struct.pack('!BBHHH', 8, 0, 0, 0x4242, seq) + data
            icmp = icmp[:2] + struct.pack('!H', csum(icmp)) + icmp[4:]
            send(guest_mac, 0x0800, ip_packet(GIP, 1, icmp))
            time.sleep(0.3)
        time.sleep(1)
        check(all(replies.get(s) == bytes(range(s, s + 48)) for s in range(1, 4)), 'the guest answers the peer\'s 3 pings', str(replies))
    out = run('ping -c 2 10.0.3.1')
    check('2 packets transmitted, 2 received' in out and echo_answered >= 2, 'the guest pings the peer (10.0.3.1)', out)

    out = run('ip route del default; ping -c 1 1.1.1.1')
    check('network unreachable' in out and re.search(r'=DONE\d+= 2', out), 'without a default route 1.1.1.1 is unreachable', out)
    out = run('ip route add default via 10.0.2.2; ip addr del 10.0.3.15/24 dev eth1; ip route; ip -br a show eth1')
    check('default via 10.0.2.2 dev eth0' in out and '10.0.3' not in out.split('ip -br a show eth1')[-1], 'route add, addr del', out)
    out = run('ip addr add 10.0.2.15/33 dev eth0')
    check('not an address/prefix' in out, 'a bad prefix is refused', out)
    out = run('ip link set eth1 down; ip -br link show eth1; ip link set eth1 up')
    check(re.search(r'eth1 +DOWN', out), 'ip link set down/up', out)
    out = run('svc restart netd; sleep 3; ip -br addr; ping -c 1 10.0.2.2', 25)
    check(re.search(r'eth0 +UP +10.0.2.15/24', out) and re.search(r'eth1 +UP', out) and '1 received' in out,
          'netd restarted: the drivers come back, the address too', out)

    # ---- sockets (step D2) ----
    out = run('nettest local', 60)
    check('nettest local: all passed' in out, 'Rust std::net in the guest (nettest local)', out)
    # Servers on the host: TCP answers "host got " + all it received; UDP echoes
    tsrv = socket.socket(); tsrv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    tsrv.bind(('127.0.0.1', 0)); tsrv.listen(4); HT = tsrv.getsockname()[1]
    usrv = socket.socket(socket.AF_INET, socket.SOCK_DGRAM); usrv.bind(('127.0.0.1', 0)); HU = usrv.getsockname()[1]
    def tcp_server():
        while True:
            try: c, _ = tsrv.accept()
            except OSError: return
            d = b''
            c.settimeout(10)
            try:
                while True:
                    x = c.recv(65536)
                    if not x: break
                    d += x
                c.sendall(b'host got ' + d)
            except OSError: pass
            c.close()
    def udp_server():
        while True:
            try: d, a = usrv.recvfrom(65536)
            except OSError: return
            usrv.sendto(b'echo: ' + d, a)
    threading.Thread(target=tcp_server, daemon=True).start()
    threading.Thread(target=udp_server, daemon=True).start()
    out = run(f'echo hello host | nc 10.0.2.2 {HT}')
    check('host got hello host' in out, 'nc in the guest reaches a TCP server on the host', out)
    out = run(f'nettest connect 10.0.2.2:{HT} from-rust')
    check('host got from-rust' in out, 'Rust TcpStream reaches the host', out)
    out = run(f'nettest udp 10.0.2.2:{HU} ping-udp')
    check(f'echo: ping-udp from 10.0.2.2:{HU}' in out, 'Rust UdpSocket reaches the host and gets the answer', out)
    out = run(f'echo hi-udp | nc -u -w 2 10.0.2.2 {HU}')
    check('echo: hi-udp' in out, 'nc -u to the host', out)
    # The host reaches the guest (hostfwd)
    # The image's firewall lets SSH in and pings, nothing else new: open
    # the two ports the host connects to (step D5 checks the rest)
    out = run('fwctl allow tcp 8080; fwctl allow tcp 8081; fwctl')
    check('Firewall: on' in out and 'New connections in: denied by default' in out and 'allow in tcp 22' in out
          and 'allow in tcp 8081' in out, 'the firewall is on; fwctl opens 8080 and 8081', out)
    out = run('nc -l 8080 < /etc/hostname > /tmp/nc-in.txt &', 3)
    out = run('ss -tln')
    check(re.search(r'tcp +LISTEN +0 +0 +0.0.0.0:8080', out), 'ss shows nc listening on 8080', out)
    got = b''
    try:
        c = socket.create_connection(('127.0.0.1', FWD_NC), timeout=10)
        c.sendall(b'from the host\n'); c.shutdown(socket.SHUT_WR)
        while True:
            x = c.recv(4096)
            if not x: break
            got += x
        c.close()
    except OSError as e:
        got = repr(e).encode()
    time.sleep(1)
    out = run('cat /tmp/nc-in.txt')
    check('from the host' in out and b'rustica' in got, 'the host reaches nc -l in the guest, both ways', out + ' / host got ' + got.decode('latin1'))
    out = run('nettest serve 8081 &', 3)
    got = b''
    try:
        c = socket.create_connection(('127.0.0.1', FWD_RUST), timeout=10)
        c.sendall(b'a line from the host\n')
        while True:
            x = c.recv(4096)
            if not x: break
            got += x
        c.close()
    except OSError as e:
        got = repr(e).encode()
    check(got == b'rust got: a line from the host\n', 'the host reaches a Rust TcpListener in the guest', got.decode('latin1'))
    out = run('sudo -u nobody nc -l 80')
    check('Permission denied' in out, 'nobody may not listen on port 80', out)
    tsrv.close(); usrv.close()

    # ---- DHCP, DNS and the web (step D3) ----
    out = run('cat /etc/resolv.conf; ip route')
    check('# Written by netd' in out and 'nameserver 10.0.2.3' in out and 'default via 10.0.2.2 dev eth0' in out,
          'netd wrote /etc/resolv.conf from DHCP', out)
    check('rustica' in dhcp_names, "eth1's DHCP requests carry the host name (option 12)", str(dhcp_names[:3]))
    # A DNS server on the host's port 53 (the guest's 10.0.2.2:53)
    ZONE = {'www.test.rustica': [('A', '10.0.2.2')], 'alias.test.rustica': [('CNAME', 'www.test.rustica'), ('A', '10.0.2.2')],
            'big.test.rustica': [('A', '10.9.0.%d' % i) for i in range(1, 41)],
            '2.2.0.10.in-addr.arpa': [('PTR', 'www.test.rustica')]}
    def enc(name):
        return b''.join(bytes([len(x)]) + x.encode() for x in name.split('.') if x) + b'\0'
    def dns_answer(q, tcp):
        qid, qd = q[:2], q[12:]
        labels, o = [], 0
        while qd[o]:
            labels.append(qd[o + 1:o + 1 + qd[o]].decode()); o += 1 + qd[o]
        name = '.'.join(labels).lower(); qtype = struct.unpack('!H', qd[o + 1:o + 3])[0]
        question = qd[:o + 5]
        recs = ZONE.get(name)
        if recs is None:
            return qid + b'\x81\x83' + struct.pack('!HHHH', 1, 0, 0, 0) + question  # NXDOMAIN
        want = {1: 'A', 5: 'CNAME', 12: 'PTR'}.get(qtype)
        rr = [r for r in recs if r[0] == want or (r[0] == 'CNAME' and want == 'A')]
        if name == 'big.test.rustica' and not tcp:
            return qid + b'\x83\x80' + struct.pack('!HHHH', 1, 0, 0, 0) + question  # truncated: ask over TCP
        body = b''
        owner = enc(name)
        for t, v in rr:
            if t == 'A': data = socket.inet_aton(v); tt = 1; own = owner if recs[0][0] != 'CNAME' else enc(recs[0][1])
            else: data = enc(v); tt = 5 if t == 'CNAME' else 12; own = owner
            body += own + struct.pack('!HHIH', tt, 1, 300, len(data)) + data
        return qid + b'\x81\x80' + struct.pack('!HHHH', 1, len(rr), 0, 0) + question + body
    dns_ok = True
    try:
        du = socket.socket(socket.AF_INET, socket.SOCK_DGRAM); du.bind(('127.0.0.1', 53))
        dt = socket.socket(); dt.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1); dt.bind(('127.0.0.1', 53)); dt.listen(4)
    except OSError as e:
        dns_ok = False
        print(f'skip  DNS checks: port 53 on the host: {e}')
    if dns_ok:
        tcp_queries = []
        def dns_udp():
            while True:
                try: q, a = du.recvfrom(512)
                except OSError: return
                try: du.sendto(dns_answer(q, False), a)
                except Exception: pass
        def dns_tcp():
            while True:
                try: c, _ = dt.accept()
                except OSError: return
                try:
                    n = struct.unpack('!H', c.recv(2))[0]; q = c.recv(n)
                    a = dns_answer(q, True); tcp_queries.append(q); c.sendall(struct.pack('!H', len(a)) + a)
                except Exception: pass
                c.close()
        threading.Thread(target=dns_udp, daemon=True).start()
        threading.Thread(target=dns_tcp, daemon=True).start()
        # The administrator's own resolv.conf: netd leaves it alone
        run('printf "nameserver 10.0.2.2\\nsearch test.rustica\\n" > /etc/resolv.conf')
        out = run('host www.test.rustica; host alias.test.rustica')
        check('www.test.rustica has address 10.0.2.2' in out and 'alias.test.rustica is an alias for www.test.rustica.' in out,
              'host: A and CNAME', out)
        out = run('host www')
        check('www.test.rustica has address 10.0.2.2' in out, 'the search domain (host www)', out)
        out = run('host big.test.rustica | grep -c "has address"')
        check(re.search(r'\n40\r?\n', out) and tcp_queries, 'an answer too long for UDP comes over TCP', out)
        out = run('host nothere.test.rustica')
        check('Host nothere.test.rustica not found: 3(NXDOMAIN)' in out and re.search(r'=DONE\d+= 1', out), 'host: NXDOMAIN', out)
        out = run('host 10.0.2.2; nslookup www.test.rustica')
        check('2.2.0.10.in-addr.arpa domain name pointer www.test.rustica.' in out and 'Address: 10.0.2.2' in out
              and 'Server:\t\t10.0.2.2' in out, 'host ADDRESS (PTR), nslookup', out)
        out = run('ping -c 1 www.test.rustica')
        check('PING www.test.rustica (10.0.2.2)' in out and '1 received' in out, 'ping by name', out)
    # Web servers on the host: HTTP and HTTPS
    PAGE = b'<h1>Rustica test page</h1>\n'
    class H(http.server.BaseHTTPRequestHandler):
        def log_message(self, *a): pass
        def do_GET(self):
            if self.path == '/redirect':
                self.send_response(302); self.send_header('Location', '/'); self.send_header('Content-Length', '0'); self.end_headers(); return
            if self.path == '/ca.pem': body, ct = open(W + '/ca.pem', 'rb').read(), 'application/x-pem-file'
            elif self.path in ('/', '/index.html'): body, ct = PAGE, 'text/html'
            else:
                self.send_response(404); self.send_header('Content-Length', '9'); self.end_headers(); self.wfile.write(b'not here\n'); return
            self.send_response(200); self.send_header('Content-Type', ct); self.send_header('Content-Length', str(len(body))); self.end_headers()
            self.wfile.write(body)
        def do_HEAD(self):
            self.send_response(200); self.send_header('Content-Type', 'text/html'); self.send_header('Content-Length', str(len(PAGE))); self.end_headers()
        def do_POST(self):
            n = int(self.headers.get('Content-Length', 0)); d = self.rfile.read(n)
            body = b'posted ' + d + b'\n'
            self.send_response(200); self.send_header('Content-Length', str(len(body))); self.end_headers(); self.wfile.write(body)
    web = http.server.ThreadingHTTPServer(('127.0.0.1', 0), H); HP = web.server_address[1]
    webs = http.server.ThreadingHTTPServer(('127.0.0.1', 0), H); HS = webs.server_address[1]
    ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER); ctx.load_cert_chain(W + '/www.pem', W + '/www.key')
    webs.socket = ctx.wrap_socket(webs.socket, server_side=True)
    threading.Thread(target=web.serve_forever, daemon=True).start()
    threading.Thread(target=webs.serve_forever, daemon=True).start()
    if not dns_ok:
        run('echo "10.0.2.2 www.test.rustica" >> /etc/hosts')
    out = run(f'curl -s http://www.test.rustica:{HP}/')
    check('<h1>Rustica test page</h1>' in out, 'curl http:// by name', out)
    out = run(f'curl -s -o /tmp/ca.pem -w "%{{http_code}} %{{size_download}}\\n" http://10.0.2.2:{HP}/ca.pem; head -1 /tmp/ca.pem')
    check(re.search(r'200 \d+', out) and '-----BEGIN CERTIFICATE-----' in out, 'curl -o, -w', out)
    out = run(f'curl -sS https://www.test.rustica:{HS}/')
    check('SSL certificate problem' in out and re.search(r'=DONE\d+= 60', out), 'curl https:// refuses an unknown CA (60)', out)
    out = run(f'curl -sS --cacert /tmp/ca.pem https://www.test.rustica:{HS}/')
    check('<h1>Rustica test page</h1>' in out and re.search(r'=DONE\d+= 0', out), 'curl https:// with --cacert (TLS, the name checked)', out)
    out = run(f'curl -sk https://10.0.2.2:{HS}/')
    check('<h1>Rustica test page</h1>' in out, 'curl -k', out)
    out = run(f'curl -s -o /tmp/r.txt -w "%{{http_code}}\\n" http://10.0.2.2:{HP}/redirect; curl -sL http://10.0.2.2:{HP}/redirect')
    check('302' in out and '<h1>Rustica test page</h1>' in out, 'curl: a redirect, -L', out)
    out = run(f'curl -sI http://10.0.2.2:{HP}/')
    check(re.search(r'HTTP/1\.[01] 200 OK', out) and 'content-type: text/html' in out.lower(), 'curl -I', out)
    out = run(f'curl -sf http://10.0.2.2:{HP}/nothere')
    check(re.search(r'=DONE\d+= 22', out) and 'not here' not in out, 'curl -f: HTTP 404 is exit status 22', out)
    out = run(f'curl -s -d "a=1&b=2" http://10.0.2.2:{HP}/echo')
    check('posted a=1&b=2' in out, 'curl -d (POST)', out)
    out = run('curl -sS http://nothere.test.rustica/' if dns_ok else 'curl -sS http://nothere.invalid/')
    check(re.search(r'=DONE\d+= 6', out), 'curl: a name that does not resolve (6)', out)
    if dns_ok:
        out = run(f'nettest connect www.test.rustica:{HP} x')
        check('HTTP/1.' in out, 'Rust TcpStream by name (DNS)', out)

    # ---- an Intel card and IPv6 (step D4) ----
    out = run('logs | grep netd: | grep eth2')
    check(re.search(r'eth2: 52:54:00:00:00:e1 \(e1000e [0-9a-f:.]+\), link up', out) and 'eth2: DHCP lease 10.0.5.15/24, router 10.0.5.2' in out,
          'eth2 is the e1000e, with its DHCP lease', out)
    out = run('ping -c 2 10.0.5.2')
    check('2 received' in out, 'ping through the e1000e (10.0.5.2)', out)
    out = run(f'curl -s http://10.0.5.2:{HP}/')
    check('<h1>Rustica test page</h1>' in out, 'curl through the e1000e', out)
    out = run('ip -6 addr show eth0; ip -6 route')
    check('inet6 fe80::5054:ff:fe12:3456/64 scope link' in out and 'inet6 fec0::5054:ff:fe12:3456/64 scope global dynamic' in out
          and 'default via fe80::2 dev eth0 proto ra' in out and 'fd05::/64 dev eth2' in out, 'IPv6: link-local and SLAAC addresses, the router (ip -6)', out)
    out = run('ping -6 -c 2 fec0::2; ping -c 2 fd05::2; ping -c 1 ::1')
    check(re.search(r'PING fec0::2\(fec0::2\).*?[12] received', out, re.S) and re.search(r'PING fd05::2\(fd05::2\).*?[12] received', out, re.S)
          and re.search(r'PING ::1\(::1\).*?1 received', out, re.S), 'ping -6: both routers (eth0, eth2), ::1', out)
    out = run('ip addr add fd99::1/64 dev eth0; ip -6 -br a show eth0; ping -c 1 fd99::1; ip addr del fd99::1/64 dev eth0')
    check('fd99::1/64' in out and '1 received' in out, 'an IPv6 address added by hand (and reached through lo)', out)
    out = run('nc -6 -l 9106 > /tmp/nc6.txt &', 2)
    out = run('echo over ipv6 | nc -6 ::1 9106; sleep 1; cat /tmp/nc6.txt')
    check(out.count('over ipv6') >= 2, 'nc -6 over ::1', out)
    web.shutdown(); webs.shutdown()

    # ---- the firewall and SSH (step D5) ----
    def host_send(port, data, wait=6):
        got = b''
        try:
            c = socket.create_connection(('127.0.0.1', port), timeout=wait)
            c.sendall(data); c.shutdown(socket.SHUT_WR)
            while True:
                x = c.recv(4096)
                if not x: break
                got += x
            c.close()
        except OSError:
            pass
        return got
    out = run('fwctl deny tcp 8080; nc -l 8080 > /tmp/fw.txt &', 3)
    host_send(FWD_NC, b'through the firewall?\n')
    time.sleep(1)
    out = run('cat /tmp/fw.txt; fwctl')
    check('through the firewall?' not in out and re.search(r'1  deny in tcp 8080 +[1-9]\d* hits', out), 'fwctl deny tcp 8080: the host cannot reach nc -l 8080', out)
    out = run('fwctl delete 1', 2)
    host_send(FWD_NC, b'through the firewall again\n')
    time.sleep(1)
    out = run('cat /tmp/fw.txt')
    check('through the firewall again' in out, 'fwctl delete 1: it can again', out)
    import shutil
    if not (shutil.which('ssh') and shutil.which('ssh-keygen')):
        print('skip  SSH checks: no ssh and ssh-keygen on the host')
    else:
        subprocess.run(['ssh-keygen', '-q', '-t', 'ed25519', '-N', '', '-f', W + '/id'], check=True)
        subprocess.run(['ssh-keygen', '-q', '-t', 'ed25519', '-N', '', '-f', W + '/other'], check=True)
        pub = open(W + '/id.pub').read().strip()
        with open(W + '/askpass', 'w') as f:
            f.write('#!/bin/sh\necho ssh-secret-1\n')
        os.chmod(W + '/askpass', 0o755)
        out = run(f'mkdir -p /root/.ssh; echo "{pub}" > /root/.ssh/authorized_keys; useradd -m tester; echo tester:ssh-secret-1 | chpasswd; logs | grep sshd:', 5)
        check(re.search(r'host key SHA256:\S+', out) and 'listening on [::]:22' in out, 'sshd is running (its host key made at the first start)', out)
        O = ['-p', str(FWD_SSH), '-o', 'StrictHostKeyChecking=no', '-o', 'UserKnownHostsFile=/dev/null', '-o', 'LogLevel=ERROR']
        K = ['ssh', '-i', W + '/id', '-o', 'BatchMode=yes'] + O
        def ssh(args, inp=None, env=None):
            try:
                r = subprocess.run(args, input=inp, capture_output=True, timeout=60, env=env)
                return r.returncode, r.stdout, r.stderr
            except subprocess.TimeoutExpired:
                return 'timeout', b'', b''
        c, o, e = ssh(K + ['root@127.0.0.1', 'echo hello from $(hostname); id -u; exit 3'])
        check(c == 3 and b'hello from rustica' in o and b'\n0\n' in o, 'ssh root@guest COMMAND (a key): its output and exit status', f'{c} {o!r} {e!r}')
        c, o, e = ssh(K[:1] + ['-tt'] + K[1:] + ['root@127.0.0.1'], b'echo on a terminal: $TERM\nexit 5\n')
        # The interactive shell (its prompt and the echo of what was typed): a pseudo-terminal
        check(c == 5 and b'on a terminal: ' in o and b'rustux' in o and b'> exit 5' in o, 'ssh -tt: a shell on a pseudo-terminal', f'{c} {o[-400:]!r} {e!r}')
        env = dict(os.environ, SSH_ASKPASS=W + '/askpass', SSH_ASKPASS_REQUIRE='force', DISPLAY=':0')
        c, o, e = ssh(['ssh', '-o', 'PubkeyAuthentication=no'] + O + ['tester@127.0.0.1', 'id -un; pwd'], env=env)
        check(c == 0 and b'tester\n/home/tester' in o, 'a password login (tester)', f'{c} {o!r} {e!r}')
        c, o, e = ssh(['ssh', '-o', 'PubkeyAuthentication=no', '-o', 'NumberOfPasswordPrompts=1'] + O + ['root@127.0.0.1', 'id'], env=env)
        check(c == 255 and b'Permission denied' in e, "root's password refused (PermitRootLogin prohibit-password)", f'{c} {e!r}')
        c, o, e = ssh(['ssh', '-i', W + '/other', '-o', 'BatchMode=yes'] + O + ['root@127.0.0.1', 'id'])
        check(c == 255 and b'Permission denied' in e, 'a key not in authorized_keys refused', f'{c} {e!r}')
        data = os.urandom(1000000)
        c, o, e = ssh(K + ['root@127.0.0.1', 'cat > /tmp/up.bin; wc -c < /tmp/up.bin'], data)
        c2, o2, e2 = ssh(K + ['root@127.0.0.1', 'cat /tmp/up.bin'])
        check(c == 0 and b'1000000' in o and o2 == data, '1 MB up and down over ssh', f'{c} {o!r} {c2} {len(o2)}')
        out = run('logs | grep sshd: | tail -12')
        check('accepted password for tester' in out and 'root logged in from' in out, 'sshd logs the logins', out)
finally:
    qemu.kill(); qemu.wait()
print('network test:', 'all passed' if fails == 0 else f'{fails} failed')
sys.exit(1 if fails else 0)
PY
