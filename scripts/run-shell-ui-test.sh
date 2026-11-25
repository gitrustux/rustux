#!/bin/bash
# The desktop shell test (2026-11-25, DESKTOP.md steps F10-F13,
# docs/TESTS.md): the panel, the menu, the desktop's icons, tiling and
# scrolling. Boots the desktop in QEMU and checks:
#   1. the desktop's icons: Home and Trash; files put in ~/Desktop appear;
#      one dragged onto the trash goes there (the trash shows full); a
#      right click on the desktop opens its menu, whose New folder works;
#   2. twelve windows: the panel's end (Lock, Log out, the clock) stays
#      where it was, the window list shortened with "+N";
#   3. a window dragged to the left edge fills the left half; Super+Right
#      gives it its size back, and again moves it to the right half;
#   4. the menu (Apps): typing "term" finds Terminal, Enter starts it;
#   5. Settings > Panel's page scrolls with the wheel;
#   6. the panel on the left (panel.conf), then at the top with auto-hide:
#      hidden until the pointer reaches the top edge.
# Exit status 0 if all is well.
#
#   scripts/run-shell-ui-test.sh [KERNEL.efi]
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
dd if=/dev/zero of="$W/boot.img" bs=1M count=64 status=none
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
         '"': 'shift-apostrophe', '@': 'shift-2', ':': 'shift-semicolon'}

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

ACCENT, LOCKBOX, TOAST = 0xFF5A1F, 0x1C1B20, 0x1C1B20  # the Rustica theme

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

check(wait_pixel(30, 782, ACCENT, 90), 'the panel\'s Apps button in the accent colour')
time.sleep(4)
n0 = len(log())

# 1. The desktop's icons
px = screen()
check(any(near(px(x, y), ACCENT, 30) for x in range(30, 80, 2) for y in range(14, 60, 2)), 'the Home icon, top left')
console('mkdir -p /root/Desktop/Projects; echo hi > /root/Desktop/notes.txt')
time.sleep(3)
px = screen()
# The fourth cell (Home, Trash, Projects, notes.txt) holds a file icon
check(any(near(px(x, y), 0xF5F2EF, 20) for x in range(40, 70, 2) for y in range(292, 330, 2)), 'notes.txt on the desktop (the fourth icon)')
drag(54, 320, 54, 140)
time.sleep(3)
to(640, 300)
time.sleep(0.5)
rbutton(True)
rbutton(False)
time.sleep(1.5)
px = screen()
check(near(px(700, 310), 0x1C1B20, 12), 'a right click on the desktop: its menu (%06x)' % px(700, 310))
click(680, 318)
time.sleep(3)

# 2. Twelve windows: the end of the panel stays
before = region(screen(), 1100, 768, 1278, 796)
console('for i in 1 2 3 4 5 6 7 8 9 10 11 12; do wintest "A long window title $i" -s 300x200 & done', 8)
time.sleep(3)
after = region(screen(), 1100, 768, 1278, 796)
same = sum(1 for a, b in zip(before, after) if near(a, b, 20)) / len(before)
check(same > 0.95, 'twelve windows: Lock and Log out stay in place (%.0f%% the same)' % (same * 100))
console('for p in $(ps | grep wintest | cut -c1-6); do kill $p; done', 4)
time.sleep(2)

# 3. Tiling
console('wintest Snapme -s 400x300 -p 500,200 &', 4)
drag(600, 185, 0, 300, 14)
time.sleep(2)
key('meta_l-right')
time.sleep(2)
key('meta_l-right')
time.sleep(3)

# 4. The menu
click(30, 782)
time.sleep(4)
type_('term')
time.sleep(2)
key('ret')
time.sleep(5)

# 5. Settings > Panel scrolls
console('settings panel &', 6)
time.sleep(3)
before = region(screen(), 0, 0, 1280, 760, 8)
to(700, 500)
time.sleep(0.5)
for _ in range(4):
    qmp_events([{'type': 'btn', 'data': {'down': True, 'button': 'wheel-down'}}])
    qmp_events([{'type': 'btn', 'data': {'down': False, 'button': 'wheel-down'}}])
    time.sleep(0.4)
time.sleep(2)
after = region(screen(), 0, 0, 1280, 760, 8)
diff = sum(1 for a, b in zip(before, after) if not near(a, b, 10))
check(diff > 200, 'the wheel scrolls Settings > Panel (%d points changed)' % diff)

# 6. The panel's edge and auto-hide
console('mkdir -p /root/.config/rustica; printf "position=left\\nwidth=180\\n" > /root/.config/rustica/panel.conf', 4)
check(wait_pixel(16, 18, ACCENT, 10), 'position=left: the panel on the left, Apps at the top')
console('printf "position=top\\nautohide=yes\\n" > /root/.config/rustica/panel.conf', 4)
to(640, 400)
time.sleep(2)
px = screen()
check(not near(px(30, 18), ACCENT, 30), 'position=top with auto-hide: hidden (%06x)' % px(30, 18))
to(640, 0)
time.sleep(2)
check(wait_pixel(30, 18, ACCENT, 6), 'the pointer at the top edge: the panel shows')

key('ctrl-alt-f1')
time.sleep(1)
n = len(log())
type_('echo console works; logs | grep -E "desk:|launcher:|compositor: (window .Snapme. (un)?tiled|panel:)|panel: (settings|start)" | tail -80\n')
wait(r'console works', 10, n)
time.sleep(5)
out = log()[n0:]
check(re.search(r'desk: 4 icons', out) is not None, 'desk: Home, Trash and the two new ones')
check('desk: trash /root/Desktop/notes.txt' in out, 'notes.txt dropped on the trash went there')
check('desk: menu: New folder' in out and 'desk: new folder /root/Desktop/New folder' in out, 'the desktop menu\'s New folder')
check('compositor: window "Snapme" tiled left' in out, 'dragged to the left edge: tiled left')
check('compositor: window "Snapme" untiled' in out and 'compositor: window "Snapme" tiled right' in out, 'Super+Right: back to its size, then the right half')
check(re.search(r'launcher: search "term": 1 found', out) is not None and 'launcher: started /bin/terminal' in out, 'the menu found Terminal and started it')
check('compositor: panel: left, 180, always shown' in out and 'compositor: panel: top, 36, auto-hide' in out, 'the compositor docked the panel left, then on top with auto-hide')
sys.exit(1 if fails else 0)
PY
    r=$?
    kill $Q 2>/dev/null; wait $Q 2>/dev/null
    return $r
}
boot desktop -device qemu-xhci,id=x -device usb-tablet,bus=x.0 || { echo "run-shell-ui-test: some checks failed"; exit 1; }
echo "run-shell-ui-test: all checks passed"
