#!/bin/bash
# The leak test (2026-11-26; CHECKLIST.md "Fourth round": "appctl: Bad
# file descriptor" and a freeze after opening and closing many windows on
# the desktop PC). Boots the desktop in QEMU; from the console, 40 windows
# (wintest) opened and killed and 10 sandboxed apps (the Image Viewer)
# started by appctl and ended; fsd's connections (df -i /: inodes) must be
# the same afterwards (10 at most more), with no "Bad file descriptor".
# Slow under emulation (about 10 s a window): its own test, not part of
# the shell-UI test. Exit status 0 if all is well.
#
#   scripts/run-leak-test.sh [KERNEL.efi]
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
echo desktop > "$W/cmdline.txt"
mcopy -i "$W/boot.img" "$W/cmdline.txt" ::/EFI/BOOT/cmdline.txt
boot() { # name, extra QEMU arguments
    rm -f "$W/mon" "$W/qmp" "$W/console.log"
    qemu-system-x86_64 -machine q35 -m 512M -smp 2 -bios "$OVMF" -drive file="$W/boot.img",format=raw \
        -display none -debugcon file:"$W/console.log" -no-reboot -monitor unix:"$W/mon",server,nowait \
        -qmp unix:"$W/qmp",server,nowait "${@:2}" >/dev/null 2>&1 &
    Q=$!
    python3 - "$W" "$1" <<'PY'
import json, math, os, re, socket, sys, time
W_DIR = sys.argv[1]
LOG, MON = W_DIR + '/console.log', W_DIR + '/mon'
fails = 0

def check(ok, what):
    global fails
    print(('  ok   ' if ok else '  FAIL ') + what)
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
    c.sendall((cmd + '\n').encode())
    time.sleep(0.15)
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
    # The compositor maps 0..32767 onto 0..SW-1: the smallest value that lands on x
    qmp_events([{'type': 'abs', 'data': {'axis': 'x', 'value': math.ceil(x * 32767 / (SW - 1))}},
                {'type': 'abs', 'data': {'axis': 'y', 'value': math.ceil(y * 32767 / (SH - 1))}}])

def button(down):
    qmp_events([{'type': 'btn', 'data': {'down': down, 'button': 'left'}}])

def click(x, y):
    to(x, y)
    button(True)
    button(False)
    time.sleep(0.3)

def drag(x0, y0, x1, y1):
    to(x0, y0)
    button(True)
    for i in range(1, 5):
        to(x0 + (x1 - x0) * i // 4, y0 + (y1 - y0) * i // 4)
    button(False)
    time.sleep(0.5)

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
         '|': 'shift-backslash', '&': 'shift-7', '$': 'shift-4', '(': 'shift-9', ')': 'shift-0', ',': 'comma',
         '"': 'shift-apostrophe', '@': 'shift-2', ':': 'shift-semicolon',
         '!': 'shift-1', '=': 'equal', '+': 'shift-equal'}

def type_(text):
    for ch in text:
        if ch.isupper():
            mon('sendkey shift-' + ch.lower())
        else:
            mon('sendkey ' + (ch if ch.isalnum() else NAMES[ch]))

shot = [0]
def screen():
    shot[0] += 1
    f = '%s/s%d.ppm' % (W_DIR, shot[0])
    mon('screendump ' + f)
    for _ in range(50):
        if os.path.exists(f) and os.path.getsize(f) > 0:
            break
        time.sleep(0.1)
    time.sleep(0.5)
    d = open(f, 'rb').read()
    head = d.split(b'\n', 3)
    w, h = map(int, head[1].split())
    data = head[3]
    return lambda x, y: int.from_bytes(data[3 * (y * w + x):3 * (y * w + x) + 3], 'big')

def near(c, want, tol=40):
    return all(abs(((c >> s) & 255) - ((want >> s) & 255)) <= tol for s in (0, 8, 16))

def key(k):
    mon('sendkey ' + k)

ACCENT, LOCKBOX, TOAST = 0xFF6500, 0x202024, 0x202024  # the Rustica theme (ACCENT: the fox)

def wait_pixel(x, y, want, t, tol=8):
    end = time.time() + t
    while time.time() < end:
        if near(screen()(x, y), want, tol):
            return True
        time.sleep(1)
    return False

if not wait(r'rustux.*:.*/root.* > ', 120):
    print('no shell prompt')
    print(log()[-3000:])
    sys.exit(1)

def rbutton(down):
    qmp_events([{'type': 'btn', 'data': {'down': down, 'button': 'right'}}])

NAMES.update({'>': 'shift-dot', '\\': 'backslash', '=': 'equal', '?': 'shift-slash'})

def lbutton(down):
    qmp_events([{'type': 'btn', 'data': {'down': down, 'button': 'left'}}])

def drag(x0, y0, x1, y1, steps=12):
    to(x0, y0)
    time.sleep(0.3)
    lbutton(True)
    time.sleep(0.3)
    for i in range(1, steps + 1):
        to(x0 + (x1 - x0) * i // steps, y0 + (y1 - y0) * i // steps)
        time.sleep(0.12)
    time.sleep(0.5)
    lbutton(False)
    time.sleep(1.5)

def console(cmd, wait_s=3):
    key('ctrl-alt-f1')
    time.sleep(1)
    type_(cmd + '\n')
    time.sleep(wait_s)
    key('ctrl-alt-f7')
    time.sleep(3)

def region(px, x0, y0, x1, y1, step=2):
    return [px(x, y) for y in range(y0, y1, step) for x in range(x0, x1, step)]

check(wait_pixel(25, 787, ACCENT, 90), 'the panel\'s fox button')
time.sleep(4)
n0 = len(log())

# 7. Opening and closing windows many times leaks nothing (2026-11-26,
#    the "appctl: Bad file descriptor" and the freeze on the desktop PC):
#    40 windows opened and killed, 10 sandboxed apps started by appctl
#    and ended (appctl returns once the app runs: the viewer is killed); fsd's connections (df -i /: inodes) the same after
n_leak = len(log())
console('echo LEAK0 $(df -i / | tail -1); for i in $(seq 40); do wintest L$i -s 160x100 & p=$!; sleep 0.2; kill $p; done; '
        'for i in $(seq 10); do appctl run org.rustica.imageviewer & sleep 3; for v in $(ps | grep viewer | cut -c1-6); do kill $v; done; done; sleep 10; '
        'for v in $(ps | grep viewer | cut -c1-6); do kill $v; done; sleep 10; echo LEFT $(ps | grep -c -E "viewer|chooser|appctl"); '
        'echo LEAK1 $(df -i / | tail -1); echo END-$((40+2))', 5)
# (the end marker is the shell's sum: the typed command, echoed, does
# not contain it) Progress every 30 s: windows opened so far (a loop that slows down as
# it goes is a leak too)
t_start = time.time()
while time.time() - t_start < 1500 and 'END-42' not in log()[n_leak:]:
    time.sleep(30)
    o = log()[n_leak:]
    print('  %4d s: %d windows, %d apps' % (time.time() - t_start, o.count('window 160x100'), o.count('appctl: started')), flush=True)
out = log()[n_leak:]
m0 = re.search(r'LEAK0 \S+\s+(\d+)\s+(\d+)', out)
m1 = re.search(r'LEAK1 \S+\s+(\d+)\s+(\d+)', out)
if m0 and m1:
    used0, used1 = int(m0.group(2)), int(m1.group(2))
    check(used1 - used0 <= 10, 'windows opened and closed 40 times, appctl 10 times: fsd connections %d -> %d' % (used0, used1))
else:
    check(False, 'the leak test printed its counts')
    print(out[-1500:])
m2 = re.search(r'LEFT (\d+)', out)
check(m2 is not None and m2.group(1) == '0', 'the apps ended, and with them appctl and their file choosers (%s left)' % (m2.group(1) if m2 else '?'))
check('Bad file descriptor' not in out, 'no "Bad file descriptor" while doing it')


sys.exit(1 if fails else 0)
PY
    r=$?
    kill $Q 2>/dev/null; wait $Q 2>/dev/null
    return $r
}
boot desktop -device qemu-xhci,id=x -device usb-tablet,bus=x.0 || { echo "run-leak-test: some checks failed"; exit 1; }
echo "run-leak-test: all checks passed"
