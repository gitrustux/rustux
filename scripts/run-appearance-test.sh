#!/bin/bash
# The appearance test (2026-11-24, docs/TESTS.md): themes, wallpapers and
# the live session's login. Boots the desktop in QEMU and checks:
#   1. the Rustica theme and the fox wallpaper by default;
#   2. a right click on the wallpaper opens the desktop's menu; its first
#      choice opens Settings > Appearance (through the panel);
#   3. there, Dracula: the windows' title bars and the panel change at
#      once; a picture of the user's, its path typed in the picture
#      field, becomes the wallpaper (as QOI); `settings --wallpaper FILE`
#      (Files > Set as wallpaper) does the same;
#   4. `theme` at the console: the desktop back to Rustica, and the
#      console's own colours to Rustica (OSC sequences), kept for root in
#      /etc/console-theme;
#   5. Log out on the live system: the login offers "Continue as root",
#      which needs no password.
# Exit status 0 if all is well.
#
#   scripts/run-appearance-test.sh [KERNEL.efi]
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

# 1. Rustica and the fox
check(wait_pixel(64, 782, ACCENT, 90), 'Rustica: the panel\'s Applications in fiery orange')
px = screen()
check(near(px(1275, 795), 0x1C1B20, 8), 'Rustica: the panel in smoky charcoal (%06x)' % px(1275, 795))
reds = sum(1 for y in range(100, 700, 20) for x in range(300, 1200, 20) if (px(x, y) >> 16) > 0xB0 and (px(x, y) >> 8 & 255) < 0x80)
check(reds > 100, 'the fox wallpaper (%d red points)' % reds)

# a picture for later, at the console
key('ctrl-alt-f1')
time.sleep(1)
n0 = len(log())
type_('mkdir -p /root/Pictures; screenshot /root/Pictures/shot.png\n')
time.sleep(4)
key('ctrl-alt-f7')
time.sleep(2)

# 2. The desktop's menu
to(900, 200)
rbutton(True)
rbutton(False)
time.sleep(1)
px = screen()
check(near(px(1000, 204), 0x1C1B20, 6) and near(px(1000, 240), 0x1C1B20, 6), 'a right click on the wallpaper: the menu (%06x)' % px(1000, 204))
to(960, 218)
time.sleep(0.5)
px = screen()
check(near(px(1050, 218), 0x4A2414, 10), 'the item under the pointer is highlighted (%06x)' % px(1050, 218))
button(True)
button(False)
time.sleep(5)

# 3. Dracula, then a picture of ours
key('tab'); key('tab'); key('ret')
time.sleep(3)
check(wait_pixel(64, 782, 0xBD93F9, 10), 'Dracula: the panel changes at once')
px = screen()
check(any(near(px(x, y), 0x44475A, 2) for x in range(60, 700, 4) for y in range(30, 80, 2)), 'Dracula: the Settings window\'s title bar')
key('tab'); key('tab'); key('tab')
type_('/root/Pictures/shot.png\n')
time.sleep(6)

# 4. theme at the console
key('ctrl-alt-f1')
time.sleep(1)
n = len(log())
type_('settings --wallpaper /root/Pictures/shot.png && echo set-from-files; theme --desktop rustica; theme --console rustica; cat /etc/console-theme; theme\n')
time.sleep(3)
px = screen()
check(near(px(1270, 790), 0x101012, 4), 'theme --console rustica: the console\'s background (%06x)' % px(1270, 790))
key('ctrl-alt-f7')
time.sleep(3)
check(wait_pixel(64, 782, ACCENT, 10), 'theme --desktop rustica: the panel orange again')
px = screen()
reds = sum(1 for y in range(100, 700, 20) for x in range(300, 1200, 20) if (px(x, y) >> 16) > 0xB0 and (px(x, y) >> 8 & 255) < 0x80)
check(reds < 50, 'the wallpaper is the picture now, not the fox (%d red points)' % reds)

# 5. Log out on the live system
click(1187, 782)
check(wait_pixel(SW // 2, 20, 0x101012, 15, 2), 'Log out: the login')
time.sleep(2)
key('shift-tab')
key('ret')
check(wait_pixel(64, 782, ACCENT, 30), 'Continue as root: back in, no password')

key('ctrl-alt-f1')
time.sleep(1)
n = len(log())
type_('echo console works; logs | grep -E "settings: (started|theme|wallpaper)|appearance:|desktop menu|desktop: root|wallpaper /root"; cat /etc/console-theme\n')
wait(r'console works', 10, n)
time.sleep(5)
out = log()[n0:]
check(re.search(r'compositor: desktop menu: Change wallpaper', out) is not None, 'the menu\'s choice went to the panel')
check(re.search(r'panel: desktop menu, Settings > Appearance', out) is not None and 'settings: started' in out, 'the panel opened Settings > Appearance')
check(re.search(r'settings: theme dracula', out) is not None and re.search(r'compositor: appearance: theme dracula', out) is not None, 'Settings set Dracula; the compositor followed')
check(len(re.findall(r'settings: wallpaper /root/\.config/rustica/wallpaper-\d+\.qoi', out)) >= 2, 'the picture became ~/.config/rustica/wallpaper-*.qoi, from the field and from settings --wallpaper')
check('set-from-files' in out, 'settings --wallpaper succeeded')
check(re.search(r'compositor: wallpaper /root/\.config/rustica/wallpaper-\d+\.qoi \(1280x800\)', out) is not None, 'the compositor shows it (1280x800)')
check(re.search(r'theme: your desktop uses rustica', out) is not None and re.search(r'theme: the console uses rustica \(from now on\)', out) is not None, 'theme set both')
check(re.search(r'\nrustica\r?\n', out) is not None, '/etc/console-theme says rustica')
check(re.search(r'desktop: root logged in again \(/etc/autologin\)', out) is not None, 'the live session came back without a password')
sys.exit(1 if fails else 0)
PY
    r=$?
    kill $Q 2>/dev/null; wait $Q 2>/dev/null
    return $r
}
boot desktop -device qemu-xhci,id=x -device usb-tablet,bus=x.0 || { echo "run-appearance-test: some checks failed"; exit 1; }
echo "run-appearance-test: all checks passed"
