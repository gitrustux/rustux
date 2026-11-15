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
# Exit status 0 if all is well.
#
#   scripts/run-network-test.sh [KERNEL.efi]
#
# Needs qemu-system-x86_64 (7.2 or later, for dgram), OVMF, mtools, python3.
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
for t in qemu-system-x86_64 mformat mmd mcopy python3; do
    command -v $t >/dev/null || { echo "$t is needed"; exit 2; }
done
W=$(mktemp -d)
trap 'rm -rf "$W"' EXIT

dd if=/dev/zero of="$W/boot.img" bs=1M count=64 status=none
mformat -i "$W/boot.img" -F ::
mmd -i "$W/boot.img" ::/EFI ::/EFI/BOOT
mcopy -i "$W/boot.img" "$EFI" ::/EFI/BOOT/BOOTX64.EFI
echo "autologin=root" > "$W/cmdline.txt"
mcopy -i "$W/boot.img" "$W/cmdline.txt" ::/EFI/BOOT/cmdline.txt

python3 - "$W" "$OVMF" <<'PY'
import os, re, socket, struct, subprocess, sys, threading, time
W, OVMF = sys.argv[1], sys.argv[2]
LOG, MON = W + '/console.log', W + '/mon'

# The wire: QEMU sends eth1's frames to UDP port B and listens on port A
wire = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
wire.bind(('127.0.0.1', 0))
B = wire.getsockname()[1]
s2 = socket.socket(socket.AF_INET, socket.SOCK_DGRAM); s2.bind(('127.0.0.1', 0)); A = s2.getsockname()[1]; s2.close()

qemu = subprocess.Popen(['qemu-system-x86_64', '-machine', 'q35', '-m', '512M', '-smp', '2', '-bios', OVMF,
    '-drive', f'file={W}/boot.img,format=raw', '-display', 'none', '-debugcon', f'file:{LOG}', '-no-reboot',
    '-monitor', f'unix:{MON},server,nowait',
    '-netdev', 'user,id=n0', '-device', 'virtio-net-pci,netdev=n0,mac=52:54:00:12:34:56',
    '-netdev', f'dgram,id=n1,local.type=inet,local.host=127.0.0.1,local.port={A},remote.type=inet,remote.host=127.0.0.1,remote.port={B}',
    '-device', 'virtio-net-pci,netdev=n1,disable-legacy=on,mac=52:54:00:ab:cd:ef'],
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
KEYS = {' ': 'spc', '\n': 'ret', '-': 'minus', '=': 'equal', ';': 'semicolon', "'": 'apostrophe', '\\': 'backslash', ',': 'comma', '.': 'dot', '/': 'slash'}
SHIFT = {'|': 'backslash', '>': 'dot', '<': 'comma', '"': 'apostrophe', '_': 'minus', ':': 'semicolon', '$': '4', '?': 'slash', '&': '7', '*': '8'}
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
    keys(f'{cmd}; echo "=DONE{DONE}= $?"\n')
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
    check('eth0: address 10.0.2.15/24' in out and 'eth0: default route via 10.0.2.2' in out, '/etc/network.conf applied', out)
    out = run('ip addr')
    check('inet 10.0.2.15/24 scope global eth0' in out and 'inet 127.0.0.1/8 scope host lo' in out
          and re.search(r'3: eth1: <BROADCAST,MULTICAST,UP,LOWER_UP> mtu 1500 state UP', out), 'ip addr', out)
    out = run('ip route; ip -br link')
    check('default via 10.0.2.2 dev eth0' in out and '10.0.2.0/24 dev eth0 src 10.0.2.15' in out
          and re.search(r'eth1 +UP +52:54:00:ab:cd:ef', out), 'ip route, ip -br link', out)
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
finally:
    qemu.kill(); qemu.wait()
print('network test:', 'all passed' if fails == 0 else f'{fails} failed')
sys.exit(1 if fails else 0)
PY
