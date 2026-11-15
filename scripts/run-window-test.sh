#!/bin/bash
# The window test (DESKTOP.md step F4, docs/TESTS.md). Boots Rustux in QEMU
# with a USB tablet (exact pointer positions), starts the compositor and
# test clients (wintest) and checks:
#   1. two windows from separate processes overlap as they should
#      (pixels of the screendump: content, title bars, the desktop);
#   2. click to focus and raise; keys reach only the focused window;
#   3. dragging the title bar moves a window, dragging its corner
#      resizes it (the client gets the new size and redraws);
#   4. maximise, minimise, Alt+Tab, F11 (full screen), the close button;
#   5. the clipboard between two windows;
#   6. a killed client loses its window; a client that ignores a close
#      request loses it after 5 s; an animating client is paced by frames;
#      a Rust client (rustica-window) gets its click;
#   7. killing the compositor gives the screen back to the text console.
# Exit status 0 if all is well.
#
#   scripts/run-window-test.sh [KERNEL.efi]
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
         '"': 'shift-apostrophe'}

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

ONE, TWO = 0xFF8080, 0x80FF80
BAYER = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]]

def wallpaper(x, y, W=1280, H=800):
    # compositor.c's make_wallpaper(), the same integer arithmetic
    rad = H * 10 // 14
    r2 = rad * rad
    t = y * 256 // H
    br, bg, bb = 0x1E * 256 + (0x2B - 0x1E) * t, 0x1F * 256 + (0x2D - 0x1F) * t, 0x29 * 256 + (0x3C - 0x29) * t
    dx, dy = x - W // 4, y - 3 * H // 4
    d2 = dx * dx + dy * dy
    g = 41 * (r2 - d2) // r2 if d2 < r2 else 0
    g = g * max(r2 - d2, 0) // r2
    r = br + ((0xBD * 256 - br) * g) // 256
    gg = bg + ((0x93 * 256 - bg) * g) // 256
    b = bb + ((0xF9 * 256 - bb) * g) // 256
    d = BAYER[y & 3][x & 3] * 16 + 8
    return ((r + d) >> 8) << 16 | ((gg + d) >> 8) << 8 | ((b + d) >> 8)

TITLE, TITLE_OFF = 0x44475A, 0x2E303E

def px_is(px, x, y, want, what):
    got = px(x, y)
    check(got == want, '%s (%d,%d: %06x)' % (what, x, y, got))

if not wait(r'rustux.*:.*/root.* > ', 120):
    print('no shell prompt')
    print(log()[-3000:])
    sys.exit(1)

n0 = len(log())
type_('compositor -v & sleep 1; wintest one -c ff8080 -p 100,100 -k & sleep 1; wintest two -c 80ff80 -p 300,200 -s 400x300 -k -t "from two" &\n')
check(wait(r'compositor: (\d+)x(\d+), /svc/window', 20, n0) is not None, 'the compositor starts')
check(wait(r'wintest two: window 400x300', 20, n0) is not None and wait(r'wintest one: window 320x200', 1, n0) is not None,
      'two windows from separate processes')
time.sleep(1)

px = screen()
px_is(px, 150, 150, ONE, 'window one\'s content')
px_is(px, 350, 250, TWO, 'two overlaps one (two is on top)')
px_is(px, 650, 450, TWO, 'window two\'s content')
px_is(px, 50, 50, wallpaper(50, 50), 'the desktop (the wallpaper)')
px_is(px, 500, 188, TITLE, 'two\'s title bar (focused)')
px_is(px, 250, 88, TITLE_OFF, 'one\'s title bar (not focused)')

n = len(log())
click(150, 150)
px = screen()
px_is(px, 350, 250, ONE, 'a click raises one above two')
check(wait(r'wintest one: focus in', 5, n) is not None and wait(r'wintest two: focus out', 1, n) is not None, 'and gives it the focus')
type_('ab')
check(wait(r'wintest one: key 0x05 text "b"', 5, n) is not None, 'keys reach the focused window')
check(re.search(r'wintest two: key', log()[n:]) is None, 'and only it')

n = len(log())
drag(250, 88, 450, 388)
check(wait(r'compositor: window "one" moved to 300,400', 5, n) is not None, 'dragging the title bar moves one to 300,400')
px = screen()
px_is(px, 350, 450, ONE, 'one\'s content is there')
px_is(px, 150, 150, wallpaper(150, 150), 'and gone from where it was')

n = len(log())
drag(700, 500, 800, 560)
check(wait(r'wintest two: configure 500x360', 5, n) is not None, 'dragging two\'s corner: the client is asked for 500x360')
check(wait(r'compositor: window "two" resized to 500x360', 5, n) is not None, 'two is resized')
time.sleep(1)
px = screen()
px_is(px, 790, 550, TWO, 'two\'s new size on the screen')

n = len(log())
click(768, 188)   # two's maximise button
check(wait(r'wintest two: configure 1278x774', 5, n) is not None and wait(r'wintest two: state 0x1', 5, n) is not None,
      'the maximise button: 1278x774, maximised')
time.sleep(1)
px = screen()
px_is(px, 640, 780, TWO, 'two fills the screen below its title bar')
n = len(log())
click(1247, 9)    # its maximise button again (restore)
check(wait(r'wintest two: configure 500x360', 5, n) is not None and wait(r'wintest two: state 0x0', 5, n) is not None,
      'again: restored to 500x360')
time.sleep(1)

n = len(log())
click(748, 188)   # two's minimise button
check(wait(r'wintest two: state 0x4', 5, n) is not None, 'the minimise button')
check(wait(r'wintest one: focus in', 5, n) is not None, 'one gets the focus')
time.sleep(0.5)
px = screen()
px_is(px, 750, 300, wallpaper(750, 300), 'two is gone from the screen')
n = len(log())
mon('sendkey alt-tab')
check(wait(r'wintest two: state 0x0', 5, n) is not None and wait(r'wintest two: focus in', 5, n) is not None,
      'Alt+Tab brings two back with the focus')
time.sleep(0.5)
px = screen()
px_is(px, 750, 300, TWO, 'two is on the screen again')

n = len(log())
mon('sendkey f11')
check(wait(r'wintest two: configure 1280x800', 5, n) is not None and wait(r'wintest two: state 0x2', 5, n) is not None,
      'F11: two full screen')
time.sleep(1)
px = screen()
px_is(px, 2, 2, TWO, 'two covers the whole screen')
n = len(log())
mon('sendkey f11')
check(wait(r'wintest two: configure 500x360', 5, n) is not None, 'F11 again: back to 500x360')
time.sleep(1)

n = len(log())
click(350, 580)   # one's content, below two
type_('c')
check(wait(r'wintest one: copied "copied text"', 5, n) is not None, 'one copies to the clipboard')
click(750, 300)
type_('v')
check(wait(r'wintest two: pasted "copied text" \(11 bytes\)', 5, n) is not None, 'two pastes it')

# To the console, to type commands
n = len(log())
mon('sendkey ctrl-alt-f1')
time.sleep(1)
type_("kill $(ps | grep ' wintest' | head -1 | cut -c1-6); wintest three -n -p 900,100 & wintest anim -a -p 900,450 -s 200x100 & rustica-window 500,580 &\n")
check(wait(r'compositor: window "one" removed \(its client closed it or ended\)', 10, n) is not None, 'one killed: its window is removed')
check(wait(r'wintest anim: window', 10, n) is not None and wait(r'wintest three: window', 10, n) is not None, 'two more windows')
mon('sendkey ctrl-alt-f7')
time.sleep(1)
px = screen()
px_is(px, 150, 450, wallpaper(150, 450), 'one\'s window is gone from the screen')
check(wait(r'wintest anim: 200 frames', 30, n) is not None, 'an animating client is paced by frames (200 so far)')
n = len(log())
click(600, 630)
check(wait(r'rustica-window: buttons 0x1 at 100,50', 5, n) is not None, 'a window from Rust (the rustica-window crate) gets its click')

n = len(log())
click(1200 - 2 + 8, 100 - 24 + 4 + 8)   # three's close button (content 900,100 320x200: button at 1200,80)
check(wait(r'wintest three: close request \(ignored\)', 5, n) is not None, 'three ignores the close button')
check(wait(r'compositor: window "three" removed \(it did not close when asked\)', 10, n) is not None,
      'and loses its window after 5 s')

n = len(log())
click(400, 300)   # two
mon('sendkey alt-f4')
check(wait(r'wintest two: closed', 5, n) is not None and wait(r'compositor: window "two" removed', 5, n) is not None,
      'Alt+F4 closes two')

n = len(log())
mon('sendkey ctrl-alt-f1')
time.sleep(1)
type_("kill $(ps | grep ' compositor' | cut -c1-6)\n")
check(wait(r'wintest anim: the compositor is gone', 10, n) is not None, 'the compositor killed: its clients notice')
time.sleep(1)
mon('sendkey ctrl-alt-f7')
time.sleep(1)
px = screen()
check(px(SW - 1, 0) != wallpaper(SW - 1, 0) and px(50, 50) != wallpaper(50, 50), 'the text console stays on the screen')
m = wait(r'compositor: (\d+)x(\d+)', 1, n0)
if fails:
    print('run-window-test: %d checks failed' % fails)
    print(log()[-4000:])
    sys.exit(1)
PY
    r=$?
    kill $Q 2>/dev/null; wait $Q 2>/dev/null
    return $r
}
boot windows -device qemu-xhci,id=x -device usb-tablet,bus=x.0 || { echo "run-window-test: some checks failed"; exit 1; }
echo "run-window-test: all checks passed"
