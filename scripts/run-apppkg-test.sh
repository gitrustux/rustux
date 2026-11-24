#!/bin/bash
# The app packages test (FoxDen step FD1, FOXDEN.md, docs/TESTS.md).
# Boots Rustux in QEMU with "desktop" on the kernel command line and a
# second disk (FAT, APPDISK) with an app package, org.rustica.example,
# built on the host with the host's rpg (its program is wintest). Then:
#   1. at the console: rpg install puts it into /apps/org.rustica.example/
#      and nowhere else; rpg list --apps shows it; rpg verify is clean;
#   2. Applications (the launcher) lists it beside the system's and starts
#      it (appctl run: in its sandbox, step FD2): its window comes;
#   3. rpg remove --purge: its folder and the user's data for it are gone,
#      rpg list --apps and the launcher have only the two apps that come with the system;
#   4. the console works throughout.
# Exit status 0 if all is well.
#
#   scripts/run-apppkg-test.sh [KERNEL.efi]
#
# Needs qemu-system-x86_64, OVMF, mtools, dosfstools, python3, cargo.
set -u
cd "$(dirname "$0")/.."
EFI=${1:-target/x86_64-unknown-uefi/release/rustux.efi}
[ -f "$EFI" ] || { echo "no $EFI (build it first)"; exit 2; }
OVMF=""
for p in /usr/share/ovmf/OVMF.fd /usr/share/qemu/OVMF.fd /usr/share/edk2-ovmf/x64/OVMF_CODE.fd /usr/share/OVMF/OVMF_CODE.fd; do
    [ -f "$p" ] && OVMF=$p && break
done
[ -n "$OVMF" ] || { echo "OVMF firmware not found"; exit 2; }
for t in qemu-system-x86_64 mformat mmd mcopy mkfs.fat python3 cargo; do
    command -v $t >/dev/null || { echo "$t is needed"; exit 2; }
done
W=$(mktemp -d)
trap 'kill $Q 2>/dev/null; rm -rf "$W"' EXIT
HOSTT=$(rustc -vV | sed -n 's/^host: //p')
(cd userspace/rust/rpg && cargo build -q --release --target "$HOSTT" --target-dir "$W/cargo") || { echo "rpg did not build for this computer"; exit 2; }
RPG="$W/cargo/$HOSTT/release/rpg"

# The app: wintest as its program, a manifest
A=$W/app
mkdir -p "$A/RPG" "$A/bin" "$W/disk"
cp userspace/bin/wintest "$A/bin/example"; chmod 755 "$A/bin/example"
printf 'Package: org.rustica.example\nVersion: 1.0\nArchitecture: x86_64\nKind: app\nMaintainer: The Rustux tests\nDescription: an example app\n' > "$A/RPG/control"
printf 'Id: org.rustica.example\nName: Example\nSummary: an example app (a window)\nExec: bin/example Example -a\nCategories: Utility\nPermissions: network\nPlatform: 1\n' > "$A/manifest"
"$RPG" build "$A" -o "$W/disk/example.rpg" > /dev/null || { echo "rpg build of the app failed"; exit 2; }
truncate -s 16M "$W/app.fat"; mkfs.fat -n APPDISK "$W/app.fat" >/dev/null
mcopy -i "$W/app.fat" "$W/disk/example.rpg" ::

dd if=/dev/zero of="$W/boot.img" bs=1M count=64 status=none
mformat -i "$W/boot.img" -F ::
mmd -i "$W/boot.img" ::/EFI ::/EFI/BOOT
mcopy -i "$W/boot.img" "$EFI" ::/EFI/BOOT/BOOTX64.EFI
echo desktop > "$W/cmdline.txt"
mcopy -i "$W/boot.img" "$W/cmdline.txt" ::/EFI/BOOT/cmdline.txt

rm -f "$W/mon" "$W/qmp" "$W/console.log"
qemu-system-x86_64 -machine q35 -m 512M -smp 2 -bios "$OVMF" -drive file="$W/boot.img",format=raw \
    -drive file="$W/app.fat",if=none,id=r,format=raw -device virtio-blk-pci,drive=r \
    -device qemu-xhci,id=x -device usb-tablet,bus=x.0 \
    -display none -debugcon file:"$W/console.log" -no-reboot -monitor unix:"$W/mon",server,nowait \
    -qmp unix:"$W/qmp",server,nowait >/dev/null 2>&1 &
Q=$!
python3 - "$W" <<'PY'
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

# 1. Installing it
n0 = len(log())
out = console('mount LABEL=APPDISK /mnt && rpg install /mnt/example.rpg; find /apps; rpg list --apps; rpg verify org.rustica.example; mkdir -p /root/.local/share/apps/org.rustica.example; echo APP-DONE', 30, r'APP-DONE\r?\n')
check('org.rustica.example 1.0: installed (2 files)' in out, 'rpg install installs the app')
check('/apps/org.rustica.example/bin/example' in out and '/apps/org.rustica.example/manifest' in out, 'its files are in /apps/org.rustica.example/')
check(re.search(r'org\.rustica\.example +1\.0 +Example: an example app \(a window\)', out) is not None, 'rpg list --apps shows it')
check('org.rustica.example: 2 files as installed' in out, 'rpg verify: its files as installed')

# 2. Applications: Animation, Example, Files...: the second
qmp_click(64, 782)
time.sleep(4)
key('down'); time.sleep(0.6)
key('ret'); time.sleep(5)
out = console('logs | grep -E "launcher: |wintest Example"; ps | grep example', 4)
check('launcher: 8 applications, 3 apps' in out, 'the launcher lists the app beside the system\'s')
check('launcher: started /bin/appctl run org.rustica.example' in out, 'the launcher starts it (appctl run, in its sandbox)')
check(re.search(r'wintest Example: (configure|focus|frame|size)', out) is not None or re.search(r'\bexample\b', out.split('ps | grep example')[-1]) is not None, 'its window comes (the program runs)')

# 3. Removing it, with the user's data
out = console('rpg remove --purge org.rustica.example; echo LEFT $(ls /apps | wc -l) $(ls /root/.local/share/apps | wc -l); rpg list --apps; echo RM-DONE', 15, r'RM-DONE\r?\n')
check('org.rustica.example: removed /root/.local/share/apps/org.rustica.example' in out, 'the user\'s data for it goes with --purge')
check('org.rustica.example 1.0: removed' in out, 'rpg remove --purge removes it')
check('org.rustica.example ' not in out.split('\nLEFT')[-1] and re.search(r'\nLEFT +2 +0\r?\n', out) is not None, 'nothing of it is left in /apps (the two apps that come with the system) or the home')
n = len(log())
qmp_click(64, 782)
time.sleep(4)
key('esc'); time.sleep(1)
out = console('logs | grep "launcher: "', 4)
check('launcher: 8 applications, 2 apps' in out, 'the launcher lists only the apps that come with the system')

# 4. The console
out = console('echo console works', 2)
check('\nconsole works' in out, 'the console works')

if fails:
    print('run-apppkg-test: %d checks failed' % fails)
    print(log()[-6000:])
    sys.exit(1)
PY
r=$?
kill $Q 2>/dev/null; wait $Q 2>/dev/null
[ $r -eq 0 ] && echo "run-apppkg-test: all checks passed" || echo "run-apppkg-test: some checks failed"
exit $r
