#!/bin/bash
# The desktop session test (DESKTOP.md step F6, docs/TESTS.md). Boots
# Rustux in QEMU with "desktop" on the kernel command line (the graphical
# target) and a USB tablet, and checks:
#   1. the desktop starts at boot: root's session (/etc/autologin) with the
#      panel at the bottom of the wallpaper;
#   2. Applications opens the launcher and closes it again; the Super key
#      alone opens it too; Enter starts the first application (its window
#      appears, the panel lists it);
#   3. a notification from the console (notify), shown top right;
#   4. Log out ends the session (its programs are killed) and the
#      graphical login comes; a wrong password is refused; alice (made at
#      the console) logs in; her panel runs as alice;
#   5. Lock: the lock screen; a wrong password is refused, hers unlocks;
#   6. the console works throughout (Ctrl+Alt+F1).
# Exit status 0 if all is well.
#
#   scripts/run-desktop-test.sh [KERNEL.efi]
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

# 1. The desktop at boot
check(wait_pixel(64, 782, ACCENT, 90), 'the desktop starts at boot: the panel (Applications)')
px = screen()
check(not near(px(640, 300), 0x101012, 2), 'the wallpaper (%06x)' % px(640, 300))

# 2. The launcher
click(64, 782)
check(wait_pixel(150, 48, 0x1C1B20, 20), 'Applications opens the launcher (a focused window; %06x)' % screen()(150, 48))
click(64, 782)
time.sleep(3)
check(not near(screen()(150, 48), 0x1C1B20, 2), 'Applications again closes the launcher (%06x)' % screen()(150, 48))
key('meta_l')
check(wait_pixel(150, 48, 0x1C1B20, 20), 'the Super key alone opens the launcher (%06x)' % screen()(150, 48))
key('ret')
time.sleep(4)
px = screen()
check(not near(px(150, 48), 0x1C1B20, 2) or not near(px(150, 100), 0x101012, 30), 'Enter starts an application and closes the launcher')

# 3. A notification from the console
key('ctrl-alt-f1')
time.sleep(1)
n = len(log())
type_('notify Hello from the console; useradd -m alice; echo alice:secret | chpasswd\n')
time.sleep(3)
key('ctrl-alt-f7')
time.sleep(1)
px = screen()
check(near(px(SW - 16 - 320 + 200, 30), TOAST, 4), 'the notification, top right')

# 4. Log out, the login, alice
click(1187, 782)
check(wait_pixel(SW // 2, 20, 0x101012, 15, 2), 'Log out: the graphical login, full screen')
time.sleep(2)
type_('alice')
key('tab')
type_('wrong')
key('ret')
time.sleep(3)
type_('secret')
key('ret')
check(wait_pixel(64, 782, ACCENT, 30), 'alice logs in: her panel')
time.sleep(2)

# 5. Lock
click(64, 782)
time.sleep(3)
key('ret')          # an application in her session, to be killed at logout
time.sleep(3)
click(1103, 782)
time.sleep(2)
px = screen()
check(px(SW // 2, SH // 2 - 30) == LOCKBOX, 'Lock: the lock screen')
type_('nope')
key('ret')
time.sleep(3)
px = screen()
check(px(SW // 2, SH // 2 - 30) == LOCKBOX, 'a wrong password: still locked')
type_('secret')
key('ret')
time.sleep(2)
px = screen()
check(px(SW // 2, SH // 2 - 30) != LOCKBOX and near(px(64, 782), ACCENT, 8), 'her password unlocks')

# Log out again: her programs go
click(1187, 782)
time.sleep(4)

# 6. The console, and what happened
key('ctrl-alt-f1')
time.sleep(1)
n = len(log())
type_('echo console works; ps; logs | grep -E "desktop: |launcher: started|panel: launcher|super: |notification|lock screen|unlocked"\n')
wait(r'console works', 10, n)
time.sleep(5)
out = log()[n:]
# The end of her session can come later on a busy machine: ask again
for _ in range(4):
    if 'desktop: alice logged out' in out:
        break
    time.sleep(5)
    m = len(log())
    type_('logs | grep "logged out"\n')
    wait(r'logged out', 10, m)
    time.sleep(2)
    out = log()[n:]
check('\nconsole works' in out, 'the console works')
check(re.search(r'desktop: root logged in \(/etc/autologin\)', out) is not None, 'root\'s session from /etc/autologin')
check(re.search(r'launcher: started /bin/wintest Animation -a', out) is not None, 'the launcher started "Animation"')
check('panel: launcher closed' in out and 'super: launcher' in out, 'the log: the launcher closed, the Super key seen')
check(re.search(r'compositor: notification "Hello from the console"', out) is not None, 'the notification was shown')
check(re.search(r'desktop: root logged out', out) is not None, 'root logged out')
check(re.search(r'desktop: a failed login for alice', out) is not None, 'a wrong password at the login is refused')
check(re.search(r'desktop: alice logged in', out) is not None, 'alice logged in')
check(re.search(r'compositor: wrong password at the lock screen', out) is not None and re.search(r'compositor: unlocked', out) is not None,
      'the lock screen refused a wrong password and took hers')
check(re.search(r'desktop: alice logged out', out) is not None, 'alice logged out')
ps_out = out.split('console works', 1)[-1].split(' rustica ', 1)[0]
check(' wintest' not in ps_out and re.search(r'alice +\d+ +\d+ +\d+ +\d+ +[\d:.]+ panel', ps_out) is None,
      'no program of either session is left (ps)')

if fails:
    print('run-desktop-test: %d checks failed' % fails)
    print(log()[-5000:])
    sys.exit(1)
PY
    r=$?
    kill $Q 2>/dev/null; wait $Q 2>/dev/null
    return $r
}
boot desktop -device qemu-xhci,id=x -device usb-tablet,bus=x.0 || { echo "run-desktop-test: some checks failed"; exit 1; }
echo "run-desktop-test: all checks passed"
