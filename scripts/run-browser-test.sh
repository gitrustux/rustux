#!/bin/bash
# The web browser test (phase W, docs/BROWSER.md, docs/TESTS.md): NetSurf
# on the Rustica desktop in QEMU, against web servers on the host (QEMU's
# user networking reaches the host as 10.0.2.2). Checks:
#   0. NetSurf is an app (org.rustica.browser), installed as the system
#      starts, and the default browser (/bin/browser starts it);
#   1. a page over HTTP: NetSurf's window, the page's colours and its text
#      in the fonts (the heading and a box in colours of their own); the
#      pointer over a link is the hand;
#   2. a click on a link to a file downloads it into ~/Downloads, the same
#      bytes as on the host (sha256), and the status line says so;
#   3. typing a URL into the address bar (the keyboard's text) loads a page
#      over HTTPS, its certificate from a test CA the script makes (added to
#      the system's bundle from the console);
#   4. a local file opened by its path (as Files opens one);
#   5. libcurl's own checks (curltest): an unknown CA refused;
#   6. a page whose body scrolls (overflow on body) has one scrollbar, the
#      window's, not a second inside it.
# Exit status 0 if all is well.
#
#   scripts/run-browser-test.sh [KERNEL.efi]
#
# Needs qemu-system-x86_64, OVMF, mtools, python3, openssl.
set -u
cd "$(dirname "$0")/.."
EFI=${1:-target/x86_64-unknown-uefi/release/rustux.efi}
[ -f "$EFI" ] || { echo "no $EFI (build it first)"; exit 2; }
OVMF=""
for p in /usr/share/ovmf/OVMF.fd /usr/share/qemu/OVMF.fd /usr/share/edk2-ovmf/x64/OVMF_CODE.fd /usr/share/OVMF/OVMF_CODE.fd; do
    [ -f "$p" ] && OVMF=$p && break
done
[ -n "$OVMF" ] || { echo "OVMF firmware not found"; exit 2; }
for t in qemu-system-x86_64 mformat mmd mcopy python3 openssl; do
    command -v $t >/dev/null || { echo "$t is needed"; exit 2; }
done
W=$(mktemp -d)
trap 'kill $Q $SRV 2>/dev/null; rm -rf "$W"' EXIT
Q=""
SRV=""

# The boot medium (the kernel holds the system: about 70 MB)
dd if=/dev/zero of="$W/boot.img" bs=1M count=128 status=none
mformat -i "$W/boot.img" -F ::
mmd -i "$W/boot.img" ::/EFI ::/EFI/BOOT
mcopy -i "$W/boot.img" "$EFI" ::/EFI/BOOT/BOOTX64.EFI
echo desktop > "$W/cmdline.txt"
mcopy -i "$W/boot.img" "$W/cmdline.txt" ::/EFI/BOOT/cmdline.txt

# The web: a page, a file to download, the test CA; and over HTTPS a
# second page, its certificate for 10.0.2.2 from that CA
mkdir -p "$W/www"
cat > "$W/www/index.html" <<'EOF'
<!doctype html><html><head><title>Rustica browser test</title>
<style>body{font-family:sans-serif;background:#fff;margin:20px} h1{color:#3456cd}
.box{background:#12ab34;width:240px;height:80px}</style></head>
<body><h1>Rustica browser test</h1><p>Plain, <b>bold</b>, <i>italic</i>, <code>mono</code>.</p>
<div class="box"></div><p style="font-size:28px"><a href="big.zip">Download the file</a></p></body></html>
EOF
cat > "$W/www/secure.html" <<'EOF'
<!doctype html><html><head><title>Secure</title></head>
<body style="background:#ab12cd"><h1 style="color:#fff">Over HTTPS</h1></body></html>
EOF
head -c 2500000 /dev/urandom > "$W/www/big.zip"
python3 -c "print('<html><head><style>body{overflow-y:scroll;background:#ab12cd;margin:0}</style></head><body>' + ''.join('<p>Line %d</p>' % i for i in range(200)) + '</body></html>')" > "$W/www/scroll.html"
# A heavy page, as a news portal's: 160 pictures from 40 host names, over
# connections kept open (HTTP/1.1), then another site (heavy.html's port
# comes from the server below, so it is written there)
python3 -c "
import struct, zlib
def png(c):
    raw = b''.join(b'\x00' + bytes(c) * 16 for _ in range(16))
    ch = lambda t, d: struct.pack('>I', len(d)) + t + d + struct.pack('>I', zlib.crc32(t + d))
    return b'\x89PNG\r\n\x1a\n' + ch(b'IHDR', struct.pack('>IIBBBBB', 16, 16, 8, 2, 0, 0, 0)) + ch(b'IDAT', zlib.compress(raw)) + ch(b'IEND', b'')
open('$W/www/img.png', 'wb').write(png((200, 120, 40)))
"
echo '<html><body style="background:#2468ac;margin:0"><h1>After</h1></body></html>' > "$W/www/after.html"
SUM=$(sha256sum < "$W/www/big.zip" | cut -d' ' -f1)
openssl req -x509 -newkey rsa:2048 -nodes -days 2 -subj "/CN=Rustica browser test CA" \
    -keyout "$W/ca.key" -out "$W/www/ca.pem" 2>/dev/null
openssl req -newkey rsa:2048 -nodes -subj "/CN=10.0.2.2" -keyout "$W/srv.key" -out "$W/srv.csr" 2>/dev/null
printf 'subjectAltName=IP:10.0.2.2\nbasicConstraints=CA:FALSE\n' > "$W/ext.cnf"
openssl x509 -req -in "$W/srv.csr" -CA "$W/www/ca.pem" -CAkey "$W/ca.key" -CAcreateserial -days 2 \
    -extfile "$W/ext.cnf" -out "$W/srv.pem" 2>/dev/null
python3 - "$W" > "$W/ports" 2>"$W/www.log" <<'PY' &
import functools, http.server, ssl, sys, threading
W = sys.argv[1]
H = functools.partial(http.server.SimpleHTTPRequestHandler, directory=W + '/www')
web = http.server.ThreadingHTTPServer(('127.0.0.1', 0), H)
webs = http.server.ThreadingHTTPServer(('127.0.0.1', 0), H)
class K(http.server.SimpleHTTPRequestHandler):
    protocol_version = 'HTTP/1.1'   # connections kept open, as real sites do
    def log_message(self, *a): pass
webk = http.server.ThreadingHTTPServer(('127.0.0.1', 0), functools.partial(K, directory=W + '/www'))
hk = webk.server_address[1]
import os
NH = int(os.environ.get('HEAVY_HOSTS', '40'))
open(W + '/www/heavy.html', 'w').write('<html><body style="margin:0">' + ''.join(
    '<img src="http://h%d.rx.test:%d/img.png?%d" width=16 height=16>' % (i % NH, hk, i) for i in range(max(160, NH))) + '</body></html>')
threading.Thread(target=webk.serve_forever, daemon=True).start()
# A DNS server for *.rx.test (the guest asks 10.0.2.2:53, QEMU passes it
# here): A 10.0.2.2, no AAAA, anything else not found
import socket as so, struct
def dns():
    d = so.socket(so.AF_INET, so.SOCK_DGRAM)
    try:
        d.bind(('127.0.0.1', 53))
    except OSError as e:
        sys.stderr.write('no DNS server: %s\n' % e)
        return
    while True:
        q, a = d.recvfrom(512)
        i, labels = 12, []
        while q[i]:
            labels.append(q[i + 1:i + 1 + q[i]].decode()); i += 1 + q[i]
        qtype = struct.unpack('>H', q[i + 1:i + 3])[0]
        qend = i + 5
        name = '.'.join(labels).lower()
        ok = name.endswith('.rx.test')
        an = 1 if ok and qtype == 1 else 0
        flags = 0x8180 if ok else 0x8183
        r = q[:2] + struct.pack('>HHHHH', flags, 1, an, 0, 0) + q[12:qend]
        if an:
            r += b'\xc0\x0c' + struct.pack('>HHIH', 1, 1, 60, 4) + bytes([10, 0, 2, 2])
        d.sendto(r, a)
threading.Thread(target=dns, daemon=True).start()
ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
ctx.load_cert_chain(W + '/srv.pem', W + '/srv.key')
webs.socket = ctx.wrap_socket(webs.socket, server_side=True)
print(web.server_address[1], webs.server_address[1], hk, flush=True)
threading.Thread(target=webs.serve_forever, daemon=True).start()
web.serve_forever()
PY
SRV=$!
for _ in $(seq 50); do [ -s "$W/ports" ] && break; sleep 0.1; done
read -r HP HS HK < "$W/ports"

qemu-system-x86_64 -machine q35 -m 1536M -smp 2 -bios "$OVMF" -drive file="$W/boot.img",format=raw,if=none,id=u \
    -device qemu-xhci -device usb-storage,drive=u -device usb-tablet \
    -nic user,model=virtio-net-pci \
    -display none -debugcon file:"$W/console.log" -no-reboot -monitor unix:"$W/mon",server,nowait \
    -qmp unix:"$W/qmp",server,nowait >/dev/null 2>&1 &
Q=$!
python3 - "$W" "$HP" "$HS" "$SUM" "$HK" <<'PY'
import json, math, re, socket, sys, time
W_DIR, HP, HS, SUM, HK = sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4], sys.argv[5]
LOG, MON = W_DIR + '/console.log', W_DIR + '/mon'
fails = 0

def check(ok, what):
    global fails
    print(('  ok   ' if ok else '  FAIL ') + what, flush=True)
    if not ok:
        fails += 1

def connect(path):
    for _ in range(100):
        try:
            c = socket.socket(socket.AF_UNIX)
            c.connect(path)
            return c
        except OSError:
            time.sleep(0.1)
    raise SystemExit('no QEMU socket ' + path)

def mon(cmd):
    c = connect(MON)
    c.recv(4096)
    c.sendall((cmd + '\n').encode())
    time.sleep(0.2)
    c.close()

qmp = connect(W_DIR + '/qmp').makefile('rw')
qmp.readline()
qmp.write(json.dumps({'execute': 'qmp_capabilities'}) + '\n')
qmp.flush()
qmp.readline()

def qmp_events(events):
    qmp.write(json.dumps({'execute': 'input-send-event', 'arguments': {'events': events}}) + '\n')
    qmp.flush()
    qmp.readline()
    time.sleep(0.15)

SW, SH = 1280, 800

def to(x, y):
    qmp_events([{'type': 'abs', 'data': {'axis': 'x', 'value': math.ceil(x * 32767 / (SW - 1))}},
                {'type': 'abs', 'data': {'axis': 'y', 'value': math.ceil(y * 32767 / (SH - 1))}}])

def click(x, y):
    to(x, y)
    qmp_events([{'type': 'btn', 'data': {'down': True, 'button': 'left'}}])
    qmp_events([{'type': 'btn', 'data': {'down': False, 'button': 'left'}}])
    time.sleep(0.3)

def log():
    try:
        return open(LOG, 'rb').read().decode('latin1')
    except OSError:
        return ''

def wait(pat, t, start=0):
    end = time.time() + t
    while time.time() < end:
        m = re.search(pat, log()[start:])
        if m:
            return m
        time.sleep(0.3)
    return None

NAMES = {' ': 'spc', '\n': 'ret', '-': 'minus', '/': 'slash', '.': 'dot', ';': 'semicolon', ':': 'shift-semicolon',
         '&': 'shift-7', '>': 'shift-dot', '<': 'shift-comma', '|': 'shift-backslash', '_': 'shift-minus', '=': 'equal',
         '"': 'shift-apostrophe', "'": 'apostrophe', '#': 'shift-3', '$': 'shift-4', '(': 'shift-9', ')': 'shift-0',
         ',': 'comma', '*': 'shift-8', '+': 'shift-equal'}

def type_(text):
    for ch in text:
        mon('sendkey ' + ('shift-' + ch.lower() if ch.isupper() else ch if ch.isalnum() else NAMES[ch]))

def console(cmd, settle=2):
    """A command at the console (Ctrl+Alt+F1); back to the desktop after it
    has echoed (the guest takes keys slower than QEMU sends them).
    Returns where its output starts in the log."""
    mon('sendkey ctrl-alt-f1')
    time.sleep(1.5)
    n = len(log())
    type_(cmd + '\n')
    wait(re.escape(cmd[-14:]), 120, n)
    time.sleep(settle)
    return n

def desktop():
    mon('sendkey ctrl-alt-f7')
    time.sleep(1.5)

shot = [0]
def screen():
    shot[0] += 1
    f = '%s/s%d.ppm' % (W_DIR, shot[0])
    mon('screendump ' + f)
    for _ in range(50):
        try:
            d = open(f, 'rb').read()
            if len(d) > SW * SH * 3:
                break
        except OSError:
            pass
        time.sleep(0.2)
    head = d.split(b'\n', 3)
    w, h = map(int, head[1].split())
    return w, h, head[3]

def count(colour, box=(0, 0, SW, SH), tol=6):
    w, h, data = screen()
    r0, g0, b0 = colour >> 16, (colour >> 8) & 255, colour & 255
    n = 0
    for y in range(box[1], box[3], 2):
        row = data[3 * y * w:3 * (y + 1) * w]
        for x in range(box[0], box[2], 2):
            r, g, b = row[3 * x], row[3 * x + 1], row[3 * x + 2]
            if abs(r - r0) <= tol and abs(g - g0) <= tol and abs(b - b0) <= tol:
                n += 1
    return n * 4

def wait_colour(colour, least, t, box=(0, 0, SW, SH)):
    end = time.time() + t
    n = 0
    while time.time() < end:
        n = count(colour, box)
        if n >= least:
            return n
        time.sleep(2)
    return n

def find(colour, box, tol=12):
    """The first pixel of a colour in a box (reading order), or None"""
    w, h, data = screen()
    r0, g0, b0 = colour >> 16, (colour >> 8) & 255, colour & 255
    for y in range(box[1], box[3]):
        for x in range(box[0], box[2]):
            p = 3 * (y * w + x)
            if abs(data[p] - r0) <= tol and abs(data[p + 1] - g0) <= tol and abs(data[p + 2] - b0) <= tol:
                return x, y
    return None

if not wait(r'rustux.*:.*/root.* > ', 150):
    print('no shell prompt')
    print(log()[-3000:])
    sys.exit(1)
# The desktop's panel (the fox button) before anything else
end = time.time() + 120
while time.time() < end and count(0xFF6500, (0, SH - 30, 50, SH)) < 40:
    time.sleep(2)
time.sleep(10)

# 0. The browser is an app, installed as the system starts
n = console('for i in $(seq 120); do test -f /apps/org.rustica.browser/manifest && break; sleep 1; done; ls /apps; browser --which; echo APP-$((40+2))', 3)
wait(r'APP-42', 150, n)
out = log()[n:]
check('org.rustica.browser' in out.split('browser --which')[-1] and re.search(r'org\.rustica\.browser\s', out) is not None,
      'NetSurf is the app org.rustica.browser, installed at start, and the default browser')

# 5 first (the console only): libcurl refuses an unknown CA; then the test
# CA into the system's bundle
n = console('curltest https://10.0.2.2:%s/secure.html' % HS, 6)
check(wait(r'curltest: error 60', 30, n) is not None, 'libcurl refuses a certificate from an unknown CA')
n = console('curltest -o /tmp/ca.pem http://10.0.2.2:%s/ca.pem && cat /tmp/ca.pem >> /etc/ssl/certs/ca-certificates.crt && echo CA-$((40+2))' % HP, 6)
check(wait(r'CA-42', 30, n) is not None, 'the test CA added to /etc/ssl/certs/ca-certificates.crt')

# 1. A page over HTTP
console('browser http://10.0.2.2:%s/ > /tmp/ns.log 2>&1 &' % HP)
desktop()
green = wait_colour(0x12AB34, 9000, 90)
check(green >= 9000, 'NetSurf shows the page over HTTP: its box (%d pixels of #12ab34)' % green)
blue = count(0x3456CD)
check(blue >= 300, 'the heading in its colour and font (%d pixels of #3456cd)' % blue)
check(count(0x000000, (40, 100, 700, 220)) >= 200, 'the text (black pixels under the heading)')

# 2. A click on the link downloads the file
link = find(0x0000EE, (40, 200, 900, 600), 40) or find(0x0000FF, (40, 200, 900, 600), 40)
check(link is not None, 'the link is drawn in its colour (%s)' % (link,))
if link:
    # The pointer over it: the hand (its hot spot at the finger's tip; a
    # row of its palm: black, white, white at -5, -4, +3 of the pointer)
    px, py = link[0] + 20, link[1] + 6
    to(px, py)
    time.sleep(2)
    w, h, data = screen()
    def pix(x, y):
        p = 3 * (y * w + x)
        return data[p] << 16 | data[p + 1] << 8 | data[p + 2]
    a, b, c = pix(px - 5, py + 9), pix(px - 4, py + 9), pix(px + 3, py + 9)
    check(a == 0x000000 and b == 0xFFFFFF and c == 0xFFFFFF, 'over the link the pointer is the hand (%06x %06x %06x)' % (a, b, c))
    click(link[0] + 20, link[1] + 6)
    time.sleep(15)
n = console('sha256sum /root/Downloads/big.zip; ls /root/Downloads', 5)
check(wait(SUM, 40, n) is not None, 'the link downloaded big.zip into ~/Downloads, the same bytes (sha256)')
check(re.search(r'big\.zip\.part', log()[n:]) is None, 'no .part file left')
desktop()

# 3. A URL typed into the address bar, over HTTPS (the test CA)
click(640, 78)
mon('sendkey ctrl-u')
time.sleep(0.5)
type_('https://10.0.2.2:%s/secure.html\n' % HS)
purple = wait_colour(0xAB12CD, 50000, 90)
check(purple >= 50000, 'a URL typed into the address bar loads a page over HTTPS (%d pixels of #ab12cd)' % purple)
mon('sendkey alt-f4')
time.sleep(3)

# 4. A local file by its path
console('echo "<body style=background:#0a7f8c><h1>Local</h1>" > /root/t.html; browser /root/t.html > /tmp/ns2.log 2>&1 &')
desktop()
teal = wait_colour(0x0A7F8C, 50000, 90)
check(teal >= 50000, 'a local file opened by its path (%d pixels of #0a7f8c)' % teal)
mon('sendkey alt-f4')
time.sleep(2)

# 6. One scrollbar for a page whose body scrolls: from the page's colour
# to the window's orange border, no more than the window's own scrollbar
console('browser http://10.0.2.2:%s/scroll.html > /tmp/ns3.log 2>&1 &' % HP)
desktop()
wait_colour(0xAB12CD, 50000, 90)
w, h, data = screen()
gaps = []
for y in range(300, 600, 50):
    row = [(data[3 * (y * w + x)] << 16) | (data[3 * (y * w + x) + 1] << 8) | data[3 * (y * w + x) + 2] for x in range(w)]
    last = max([x for x in range(w) if row[x] == 0xAB12CD] or [0])
    border = next((x for x in range(last + 1, w) if abs((row[x] >> 16) - 0xFF) < 12 and abs(((row[x] >> 8) & 255) - 0x65) < 30 and (row[x] & 255) < 40), None)
    if last and border:
        gaps.append(border - last - 1)
check(gaps and max(gaps) <= 22, 'a page whose body scrolls has one scrollbar, the window\'s (%s px from the page to the border)' % gaps)
mon('sendkey alt-f4')
time.sleep(2)

# 7. A heavy page, as a news portal's (160 pictures from 40 host names,
# each looked up by DNS, connections kept open), then another site in the
# same window, then Back: they must still load (2026-11-27: after a heavy
# site the browser could load nothing more)
console('cp /etc/resolv.conf /tmp/resolv.keep; echo "nameserver 10.0.2.2" > /etc/resolv.conf')
console('browser http://h0.rx.test:%s/heavy.html > /tmp/ns4.log 2>&1 &' % HK)
desktop()
orange = wait_colour(0xC87828, 30000, 120)
check(orange >= 30000, 'a heavy page: 160 pictures from 40 host names, each looked up (%d pixels of them)' % orange)
time.sleep(5)
import os
def snap(name):
    if os.environ.get('SHOTS'):
        mon('screendump %s/%s.ppm' % (os.environ['SHOTS'], name)); time.sleep(1)
snap('heavy1')
# The address bar: the first row with a long run of white (wherever the
# window was placed)
w, h, data = screen()
bar = None
for y in range(40, 500):
    run = best = 0
    for x in range(0, w, 2):
        p = 3 * (y * w + x)
        run = run + 2 if data[p] == 255 and data[p + 1] == 255 and data[p + 2] == 255 else 0
        best = max(best, run)
    if best > 500:
        bar = y
        break
check(bar is not None, 'the browser window\'s address bar found (row %s)' % bar)
if bar is not None:
    left = next(x for x in range(w) if data[3 * ((bar + 8) * w + x)] == 255 and data[3 * ((bar + 8) * w + x) + 1] == 255)
    click(left + 300, bar + 10)
    mon('sendkey ctrl-u')
    time.sleep(0.5)
    type_('http://after.rx.test:%s/after.html\n' % HK)
    blue = wait_colour(0x2468AC, 50000, 60)
    snap('heavy2')
    check(blue >= 50000, 'after it, another site (looked up too) loads in the same window (%d pixels of #2468ac)' % blue)
    # Back: the toolbar's first button, left of the address bar
    click(left - 120, bar + 12)
    back = wait_colour(0xC87828, 30000, 60)
    snap('heavy3')
    check(back >= 30000, 'Back shows the heavy page again (%d pixels of its pictures)' % back)
    n = console('ps | grep -E "PID|netsurf|netd"; free', 3)
    print(log()[n:][-1500:])
    if blue < 50000 or back < 30000:
        n = console('tail -25 /tmp/ns4.log', 3)
        print(log()[n:][-3000:])
mon('sendkey alt-f4')
time.sleep(2)
console('cp /tmp/resolv.keep /etc/resolv.conf')

n = console('cat /tmp/ns.log /tmp/ns2.log | head -20; echo END-$((40+2))', 3)
wait(r'END-42', 30, n)
out = log()[n:]
check(not re.search(r'FAULT|panic|Segmentation', out), 'nothing went wrong in NetSurf\'s output')
if fails:
    print(log()[-3000:])
sys.exit(1 if fails else 0)
PY
r=$?
[ $r = 0 ] && echo "run-browser-test: all checks passed" || echo "run-browser-test: some checks failed"
exit $r
