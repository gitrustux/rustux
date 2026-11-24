#!/bin/bash
# The toolkit test (DESKTOP.md step F5, docs/TESTS.md). Boots Rustux in
# QEMU with a USB tablet, starts the compositor and uidemo (the
# rustica-ui toolkit's demo), and checks:
#   1. the form with the keyboard: typing, Ctrl+A/C/V through the
#      clipboard, Tab between fields, Space on the checkbox, Enter on
#      Submit, the dialog closed with Enter; Reset with the mouse;
#   2. menus with F10 and arrows, and with the mouse;
#   3. the text editor field (click, Ctrl+A, typing);
#   4. the 1000-item list (click, End, Home, Page Down) and the tree
#      (open with Right, move with Down);
#   5. a PNG and a JPEG drawn, anti-aliased text (screendump pixels);
#   6. File > Quit and its dialog end the program; at 2x the window is
#      twice the size.
# Exit status 0 if all is well.
#
#   scripts/run-ui-test.sh [KERNEL.efi]
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
         '"': 'shift-apostrophe', '@': 'shift-2'}

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

if not wait(r'rustux.*:.*/root.* > ', 120):
    print('no shell prompt')
    print(log()[-3000:])
    sys.exit(1)

n0 = len(log())
type_('compositor -v & sleep 1; uidemo 100,60 &\n')
check(wait(r'uidemo: started', 30, n0) is not None and wait(r'compositor: window "Rustica UI demo" 760x520', 30, n0) is not None,
      'uidemo opens its 760x520 window')
time.sleep(5)

# 1. The form (the name field has the focus)
n = len(log())
type_('Ada Lovelace')
key('ctrl-a'); key('ctrl-c'); key('tab'); key('ctrl-v')
type_('@x')
key('tab'); key('spc')
check(wait(r'uidemo: subscribe true', 10, n) is not None, 'Tab to the checkbox, Space ticks it')
key('tab'); key('ret')
check(wait(r'uidemo: submit name="Ada Lovelace" email="Ada Lovelace@x" subscribe=true', 10, n) is not None,
      'typed, copied and pasted, submitted with Enter')
time.sleep(1)
px = screen()
check(near(px(150, 520), 0x070708, 6) and px(480, 300) == 0x1C1B20, 'a dialog over the dimmed window (%06x, %06x)' % (px(150, 520), px(480, 300)))
key('ret')
check(wait(r'uidemo: dialog Submitted: OK', 10, n) is not None, 'Enter closes the dialog')
click(238, 415)
check(wait(r'uidemo: reset', 10, n) is not None, 'the Reset button with the mouse')

# 2. Menus
n = len(log())
key('f10'); key('right'); key('down'); key('ret')
check(wait(r'uidemo: menu Edit > Fill example', 10, n) is not None, 'F10, Right, Down, Enter: Edit > Fill example')
click(234, 75)
click(240, 172)
check(wait(r'uidemo: menu View > Editor', 10, n) is not None, 'the mouse: View > Editor')

# 3. The editor
n = len(log())
click(500, 400)
key('ctrl-a')
type_('Hello')
check(wait(r'uidemo: editor "Hello"', 10, n) is not None, 'the text editor field: select all, type')

# 4. The list and the tree
n = len(log())
click(207, 118)
click(156, 196)
check(wait(r'uidemo: selected Item 0001', 10, n) is not None, 'a click selects in the list')
key('end')
check(wait(r'uidemo: selected Item 1000', 10, n) is not None, 'End: the last of 1000')
key('home'); key('pgdn')
check(wait(r'uidemo: selected Item 0012', 10, n) is not None, 'Home, Page Down')
click(576, 196)
check(wait(r'uidemo: tree Rustica$', 10, n) is not None or wait(r'uidemo: tree Rustica\n', 1, n) is not None, 'a click selects in the tree')
key('right'); key('down')
check(wait(r'uidemo: tree Rustica/bin', 10, n) is not None, 'Right opens a node, Down moves into it')
key('right'); key('down')
check(wait(r'uidemo: tree Rustica/bin/compositor', 10, n) is not None, 'and deeper')

# 5. Images and text
click(350, 118)
time.sleep(1)
px = screen()
check(near(px(360, 222), 0xBD93F9, 12), 'the PNG logo (%06x)' % px(360, 222))
check(near(px(369, 381), 0xFFB86C, 40), 'the JPEG photo (%06x)' % px(369, 381))
colours = set(px(x, y) for x in range(110, 400) for y in range(185, 215))
check(len(colours) > 12, 'anti-aliased text (%d colours in a line of text)' % len(colours))

# 6. Quit, then 2x
n = len(log())
click(130, 75)
click(136, 172)
time.sleep(1)
key('ret')
check(wait(r'uidemo: dialog Quit: Quit', 10, n) is not None and wait(r'uidemo: closed', 10, n) is not None,
      'File > Quit and the dialog end uidemo')
check(wait(r'compositor: window "Rustica UI demo" removed', 10, n) is not None, 'its window is gone')
mon('sendkey ctrl-alt-f1')
time.sleep(1)
n = len(log())
type_('uidemo --scale 2 0,0 &\n')
check(wait(r'compositor: window "Rustica UI demo" 1520x1040', 30, n) is not None, '--scale 2: a 1520x1040 window')

if fails:
    print('run-ui-test: %d checks failed' % fails)
    print(log()[-4000:])
    sys.exit(1)
PY
    r=$?
    kill $Q 2>/dev/null; wait $Q 2>/dev/null
    return $r
}
boot ui -device qemu-xhci,id=x -device usb-tablet,bus=x.0 || { echo "run-ui-test: some checks failed"; exit 1; }
echo "run-ui-test: all checks passed"
