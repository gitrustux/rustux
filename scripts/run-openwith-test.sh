#!/bin/bash
# The "open with" test (FoxDen step FD6b, docs/FOXDEN.md "Opening files",
# docs/TESTS.md): the Text Editor and the Image Viewer are apps that come
# with the system. Boots Rustux in QEMU with "desktop", then:
#   1. packagesd installed both at its start (from /usr/share/rpg/apps),
#      noted in /var/lib/rpg/preinstalled; /bin/editor and /bin/viewer are
#      gone;
#   2. appctl view ID FILE: the file in the view, rw; a folder, a missing
#      file and a path with .. refused;
#   3. appctl open: the editor gets ~/Documents/a.txt alone, changes it
#      and saves it in place; the viewer gets one of two pictures and sees
#      only that one;
#   4. a new text: Save asks with the file chooser (the name it suggests,
#      Untitled.txt), saving again rewrites it (shorter: no old bytes
#      left); Open (Ctrl+O) through the chooser reads a.txt;
#   5. an app an administrator removed is not installed again when the
#      installer service starts again;
#   6. the console works.
# $SHOTS=DIR: screendumps of the two apps there (the apps' screenshots).
# Exit status 0 if all is well.
#
#   scripts/run-openwith-test.sh [KERNEL.efi]
#
# Needs qemu-system-x86_64, OVMF, mtools, dosfstools and python3.
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

rm -f "$W/mon" "$W/qmp" "$W/console.log"
qemu-system-x86_64 -machine q35 -m 512M -smp 2 -bios "$OVMF" -drive file="$W/boot.img",format=raw \
    -device qemu-xhci,id=x -device usb-tablet,bus=x.0 \
    -display none -debugcon file:"$W/console.log" -no-reboot -monitor unix:"$W/mon",server,nowait \
    -qmp unix:"$W/qmp",server,nowait >/dev/null 2>&1 &
Q=$!
SHOTS=${SHOTS:-} python3 - "$W" <<'PY'
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

def qmp_click(x, y):
    qmp_events([{'type': 'abs', 'data': {'axis': 'x', 'value': math.ceil(x * 32767 / 1279)}},
                {'type': 'abs', 'data': {'axis': 'y', 'value': math.ceil(y * 32767 / 799)}}])
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

NAMES = {' ': 'spc', '\n': 'ret', '-': 'minus', '/': 'slash', '.': 'dot', ';': 'semicolon', "'": 'apostrophe',
         '|': 'shift-backslash', '&': 'shift-7', '$': 'shift-4', '(': 'shift-9', ')': 'shift-0', ',': 'comma',
         '"': 'shift-apostrophe', '@': 'shift-2', ':': 'shift-semicolon', '>': 'shift-dot', '*': 'shift-8',
         '+': 'shift-equal', '=': 'equal', '_': 'shift-minus'}

def type_(text):
    for ch in text:
        if ch.isupper():
            mon('sendkey shift-' + ch.lower())
        else:
            mon('sendkey ' + (ch if ch.isalnum() else NAMES[ch]))

def key(k):
    mon('sendkey ' + k)

def console(cmd, t=3, until=None):
    """A command at the console (Ctrl+Alt+F1), then back to graphics; its output"""
    key('ctrl-alt-f1')
    time.sleep(1)
    n = len(log())
    type_(cmd + '\n')
    if until:
        wait(until, t, n)
    else:
        time.sleep(t)
    key('ctrl-alt-f7')
    time.sleep(1.5)
    return log()[n:]

if not wait(r'rustux.*:.*/root.* > ', 120):
    print('no shell prompt')
    print(log()[-3000:])
    sys.exit(1)
def screen():
    f = '%s/s.ppm' % W_DIR
    try:
        os.remove(f)
    except OSError:
        pass
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

def wait_pixel(x, y, want, t, tol=8):
    end = time.time() + t
    while time.time() < end:
        c = screen()(x, y)
        if all(abs(((c >> s) & 255) - ((want >> s) & 255)) <= tol for s in (0, 8, 16)):
            return True
        time.sleep(1)
    return False

# The desktop takes the keyboard when it comes up: wait for it (the
# panel's Applications button)
check(wait_pixel(64, 782, 0xFF5A1F, 90), 'the desktop starts at boot')
time.sleep(3)
E, I = 'org.rustica.texteditor', 'org.rustica.imageviewer'
NAMES['^'] = 'shift-6'
NAMES['#'] = 'shift-3'
NAMES['?'] = 'shift-slash'
NAMES['['] = 'bracket_left'
NAMES[']'] = 'bracket_right'

def since(n, pat, t):
    return wait(pat, t, n) is not None

def sh(cmd, mark, t=60):
    n = len(log())
    console(cmd + '; echo ' + mark, t, r'(?<!echo )' + mark + r'\r?\n')
    return log()[n:]

def shot(name):
    d = os.environ.get('SHOTS')
    if d:
        mon('screendump %s/%s.ppm' % (d, name))
        time.sleep(1.5)

# 1. Installed at the start
out = ''
for _ in range(20):
    out = sh('rpg list --apps; cat /var/lib/rpg/preinstalled; ls /bin/editor /bin/viewer', 'S1-DONE', 15)
    if re.search(r'%s +1\.\d+\.\d+' % re.escape(I), out):
        break
    time.sleep(3)
check(re.search(r'%s +1\.\d+\.\d+ +Text Editor' % re.escape(E), out) is not None and re.search(r'%s +1\.\d+\.\d+ +Image Viewer' % re.escape(I), out) is not None, 'the installer service installed the Text Editor and the Image Viewer at its start')
check(re.search(r'\n%s\r?\n' % re.escape(E), out) is not None and re.search(r'\n%s\r?\n' % re.escape(I), out) is not None, 'both noted in /var/lib/rpg/preinstalled')
check('/bin/editor' in out and '/bin/viewer' in out and out.count('No such file') >= 2, 'the system has no /bin/editor or /bin/viewer any more')
out = sh('logs -t packagesd | grep "comes with the system"', 'S1B', 10)
check('installed %s, which comes with the system' % E in out, 'the installer service logs it')

# 2. appctl view with files
out = sh('mkdir -p /root/Documents /root/Pictures; echo "first line" > /root/Documents/a.txt; '
         'appctl view %s /root/Documents/a.txt | grep Documents' % E, 'S2-DONE', 10)
check('rw /root/Documents/a.txt' in out, 'appctl view ID FILE: the file is in the view, read-write')
out = sh('appctl view %s /root/Documents; echo CODE-$?; appctl view %s /root/Documents/../Documents/a.txt; echo CODE-$?; appctl view %s /root/nothing.txt; echo CODE-$?' % (E, E, E), 'S2B', 10)
check('not a file' in out and 'without . or ..' in out and 'No such file' in out and len(re.findall(r'(?<!echo )CODE-1', out)) == 3, 'a folder, a path with .. and a missing file are refused')

# 3. appctl open: the editor, in place
n = len(log())
console('appctl open %s /root/Documents/a.txt & sleep 1; echo S3-STARTED' % E, 6, r'(?<!echo )S3-STARTED\r?\n')
check(since(n, r'appctl: %s gets /root/Documents/a\.txt \(rw\)' % re.escape(E), 10) and since(n, r'editor: open /root/Documents/a\.txt', 15), 'appctl open: the editor starts with the file')
time.sleep(4)
type_('edited ')
time.sleep(1)
shot('editor')
key('ctrl-s')
check(since(n, r'editor: saved /root/Documents/a\.txt \(\d+ bytes\)', 15), 'it saves the file in place')
time.sleep(1)
key('ctrl-q')
check(since(n, r'appctl: %s ended \(0\)' % re.escape(E), 15), 'it quits')
out = sh('cat /root/Documents/a.txt', 'S3B', 5)
check('edited first line' in out, 'the file has the change')

# the viewer: one picture of two
out = sh('cp /usr/share/rpg/apps/%s/icon.png /root/Pictures/one.png; cp /usr/share/rustica/photo.jpg /root/Pictures/two.jpg' % I, 'S3C', 5)
n = len(log())
console('appctl open %s /root/Pictures/two.jpg & sleep 1; echo S3-VIEW' % I, 6, r'(?<!echo )S3-VIEW\r?\n')
check(since(n, r'viewer: showing /root/Pictures/two\.jpg \(320x180\), 1 of 1 it can see', 20), 'the viewer shows the picture (a JPEG), and sees no other in its folder')
time.sleep(3)
shot('viewer')
key('esc')
check(since(n, r'appctl: %s ended \(0\)' % re.escape(I), 15), 'the viewer quits')

# 4. A new text: Save as and Open through the chooser
n = len(log())
console('appctl run %s & sleep 1; echo S4-STARTED' % E, 6, r'(?<!echo )S4-STARTED\r?\n')
check(since(n, r'editor: open \(new\)', 15), 'the editor starts with a new file')
time.sleep(4)
type_('fresh text, longer')
key('ctrl-s')
time.sleep(5)
key('ret')
check(since(n, r'appctl: %s: the user chose /root/Documents/Untitled\.txt \(save\)' % re.escape(E), 15) and since(n, r'editor: saved Untitled\.txt \(chosen\) \(18 bytes\)', 15), 'Save asks with the chooser, the name it suggests in Documents')
time.sleep(1)
for _ in range(8):
    key('backspace')
key('ctrl-s')
check(since(n, r'editor: saved Untitled\.txt \(chosen\) \(10 bytes\)', 15), 'saving again writes to the same file')
time.sleep(1)
key('ctrl-o')
time.sleep(5)
type_('a.txt\n')
check(since(n, r'editor: open a\.txt \(chosen, \d+ bytes\)', 15), 'Open (Ctrl+O) through the chooser reads a.txt')
time.sleep(1)
key('ctrl-q')
check(since(n, r'appctl: %s ended \(0\)' % re.escape(E), 15), 'the editor quits')
out = sh('echo "[$(cat /root/Documents/Untitled.txt)]"', 'S4B', 5)
check(re.search(r'(?<!")\[fresh text\]', out) is not None, 'Untitled.txt has the shorter text alone')

# 5. Removed stays removed
out = sh('rpg remove %s; svc restart packagesd; sleep 8; rpg list --apps | grep -c %s; ls /apps' % (I, I), 'S5-DONE', 40)
check(re.search(r'\n0\r?\n', out) is not None, 'an app removed is not installed again when the installer service starts')

# 6. The console
out = console('echo console works', 2)
check('\nconsole works' in out, 'the console works')

if fails:
    print('run-openwith-test: %d checks failed' % fails)
    print(log()[-8000:])
    sys.exit(1)
PY
r=$?
kill $Q 2>/dev/null; wait $Q 2>/dev/null
[ $r -eq 0 ] && echo "run-openwith-test: all checks passed" || echo "run-openwith-test: some checks failed"
exit $r
