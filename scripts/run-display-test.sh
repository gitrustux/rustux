#!/bin/bash
# The display test (DESKTOP.md step F2, docs/TESTS.md). Boots Rustux in
# QEMU and, at the shell:
#   1. fbtest --check: the display service's snapshot of the screen is
#      what fbtest drew;
#   2. fbtest --pattern in the background and screenshot: the screenshot's
#      pixels (CRC-32) are QEMU's screendump's, and the screendump is the
#      test pattern computed here (so the pixel layout is right);
#   3. Ctrl+Alt+F1 shows the text console, Ctrl+Alt+F7 the pattern again,
#      q (a key passed to fbtest) ends it and the console is back;
#   4. fbtest -n 200 animates 200 frames, all shown, and reports the times;
#   5. the display service killed: the console shows, devmgr starts the
#      service again and fbtest --check passes with it.
# Exit status 0 if all is well.
#
#   scripts/run-display-test.sh [KERNEL.efi]
#
# Needs qemu-system-x86_64, OVMF, mtools, python3.
set -u
cd "$(dirname "$0")/.."
EFI=${1:-target/x86_64-unknown-uefi/release/rustux.efi}
[ -f "$EFI" ] || { echo "no $EFI (build it first)"; exit 2; }
OVMF=""
for p in /usr/share/ovmf/OVMF.fd /usr/share/qemu/OVMF.fd /usr/share/edk2-ovmf/x64/OVMF_CODE.fd /usr/share/OVMF/OVMF_CODE.fd; do
    [ -f "$p" ] && OVMF=$p && break
done
[ -n "$OVMF" ] || { echo "OVMF firmware not found"; exit 2; }
for t in qemu-system-x86_64 mformat mmd mcopy python3; do
    command -v $t >/dev/null || { echo "$t is needed"; exit 2; }
done
W=$(mktemp -d)
trap 'kill $Q 2>/dev/null; rm -rf "$W"' EXIT
dd if=/dev/zero of="$W/boot.img" bs=1M count=128 status=none
mformat -i "$W/boot.img" -F ::
mmd -i "$W/boot.img" ::/EFI ::/EFI/BOOT
mcopy -i "$W/boot.img" "$EFI" ::/EFI/BOOT/BOOTX64.EFI
qemu-system-x86_64 -machine q35 -m 512M -smp 2 -bios "$OVMF" -drive file="$W/boot.img",format=raw \
    -display none -debugcon file:"$W/console.log" -no-reboot -monitor unix:"$W/mon",server,nowait \
    >/dev/null 2>&1 &
Q=$!

python3 - "$W" <<'PY'
import os, re, socket, sys, time, zlib
W = sys.argv[1]
LOG, MON = W + '/console.log', W + '/mon'
fails = 0

def check(ok, what):
    global fails
    print(('  ok   ' if ok else '  FAIL ') + what)
    if not ok:
        fails += 1

def mon(cmd):
    for _ in range(50):
        try:
            c = socket.socket(socket.AF_UNIX)
            c.connect(MON)
            break
        except OSError:
            time.sleep(0.1)
    c.sendall((cmd + '\n').encode())
    time.sleep(0.2)
    c.close()

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
        time.sleep(0.2)
    return None

NAMES = {' ': 'spc', '\n': 'ret', '-': 'minus', '/': 'slash', '.': 'dot', ';': 'semicolon', "'": 'apostrophe',
         '|': 'shift-backslash', '&': 'shift-7', '$': 'shift-4', '(': 'shift-9', ')': 'shift-0'}

def type_(text):
    for ch in text:
        mon('sendkey ' + (ch if ch.isalnum() and not ch.isupper() else NAMES[ch]))

def dump(name):
    f = W + '/' + name
    mon('screendump ' + f)
    for _ in range(50):
        if os.path.exists(f) and os.path.getsize(f) > 0:
            break
        time.sleep(0.1)
    time.sleep(0.5)
    d = open(f, 'rb').read()
    head = d.split(b'\n', 3)
    w, h = map(int, head[1].split())
    return w, h, head[3]

BARS = [0xF8F8F2, 0xF1FA8C, 0x8BE9FD, 0x50FA7B, 0xFF79C6, 0xFF5555, 0xBD93F9, 0x282A36]

def pattern(w, h):
    # fbtest.c's pattern()
    rows = []
    s, x0, y0 = h // 4, w // 2 - h // 8, h // 2 - h // 8
    for y in range(h):
        row = bytearray(3 * w)
        for x in range(w):
            if y >= h - h // 8:
                v = x * 255 // (w - 1)
                p = v << 16 | v << 8 | v
            else:
                p = BARS[x * 8 // w]
            if y < 2 or y >= h - 2 or x < 2 or x >= w - 2:
                p = 0xFFFFFF
            if x0 <= x < x0 + s and y0 <= y < y0 + s:
                lx, ly = x - x0, y - y0
                p = 0 if (lx + ly) & 1 else 0xFF0000 >> ((lx // 16) % 3 * 8)
            row[3 * x:3 * x + 3] = bytes((p >> 16, (p >> 8) & 0xFF, p & 0xFF))
        rows.append(bytes(row))
    return b''.join(rows)

if not wait(r'rustux.*:.*/root.* > ', 120):
    print('no shell prompt')
    sys.exit(1)

n = len(log())
type_('fbtest --check\n')
m = wait(r'fbtest: (snapshot matches|.*differs|no .*|the graphics.*)', 60, n)
check(m is not None and 'matches' in m.group(0), 'fbtest --check: ' + (m.group(0) if m else 'no answer'))

n = len(log())
type_('fbtest --pattern 30 & sleep 3; screenshot /tmp/p.png\n')
m = wait(r'screenshot: (\d+)x(\d+) to \S+, pixels crc32 ([0-9a-f]+)', 40, n)
check(m is not None, 'screenshot of the pattern: ' + (m.group(0) if m else 'no answer'))
w, h, a = dump('a.ppm')
check(m is not None and int(m.group(3), 16) == zlib.crc32(a) and (int(m.group(1)), int(m.group(2))) == (w, h),
      'the screenshot has the pixels of QEMU\'s screendump (%dx%d)' % (w, h))
check(a == pattern(w, h), 'the screendump is the test pattern')

mon('sendkey ctrl-alt-f1')
time.sleep(1.5)
_, _, b = dump('b.ppm')
check(b != a and b[3 * (w - 1):3 * w] != b'\xff\xff\xff', 'Ctrl+Alt+F1 shows the text console')
mon('sendkey ctrl-alt-f7')
time.sleep(1.5)
_, _, c = dump('c.ppm')
check(c == a, 'Ctrl+Alt+F7 shows the pattern again')
mon('sendkey q')
time.sleep(2)
_, _, d = dump('d.ppm')
check(d == b, 'q ends fbtest and the console is back as it was')

n = len(log())
type_('fbtest -n 200\n')
m = wait(r'fbtest: (\d+)x(\d+), (\d+) frames in \d+ ms \((\d+) frames/s\), (\d+) shown', 120, n)
t = wait(r'fbtest: per frame .*', 10, n)
check(m is not None and m.group(3) == '200' and m.group(5) == '200',
      'fbtest -n 200: ' + (m.group(0) if m else 'no answer'))
if t:
    print('       ' + t.group(0).strip())

n = len(log())
type_("fbtest --pattern 30 & sleep 2; kill $(ps | grep ' display' | cut -c1-6); sleep 2; fbtest --check\n")
m = wait(r'fbtest: (snapshot matches|.*differs|no .*|the graphics.*)', 60, n)
check(wait(r'devmgr: display ended \(code -1024\); restarting it', 1, n) is not None, 'devmgr restarts the display service')
check(m is not None and 'matches' in m.group(0), 'fbtest --check with the new service: ' + (m.group(0) if m else 'no answer'))
time.sleep(1)
_, _, e = dump('e.ppm')
check(e != a and e[3 * (w - 1):3 * w] != b'\xff\xff\xff', 'the text console is on the screen')

if fails:
    print('run-display-test: %d checks failed' % fails)
    print(log()[-3000:])
    sys.exit(1)
print('run-display-test: all checks passed')
PY
