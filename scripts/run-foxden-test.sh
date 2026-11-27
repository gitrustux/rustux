#!/bin/bash
# The FoxDen test (FoxDen step FD5, docs/FOXDEN.md, docs/TESTS.md). An apps
# source made on the host (the Calculator and the System Monitor of
# userspace/apps, with their icons and screenshots, and org.test.homeapp
# asking for files:home; signed with a test apps key) on a second disk.
# Boots Rustux in QEMU with "desktop", then:
#   1. FoxDen lists the source's apps (3, none installed; and the two that
#      come with the system); its page for
#      the Calculator; Install (Enter) installs it, Applications lists it,
#      Open (Enter) starts it sandboxed;
#   2. Remove (Tab, Enter, Enter in the question) removes it;
#   3. carol (not an administrator) in FoxDen: the files:home app needs an
#      administrator; FoxDen's sheet takes erin's name and password and it
#      installs; on Installed she turns its network permission off
#      (appctl then gives it files:home only);
#   4. its pictures are cached in ~/.cache/foxden; the console works.
# Exit status 0 if all is well.
#
#   scripts/run-foxden-test.sh [KERNEL.efi]
#
# Needs qemu-system-x86_64, OVMF, mtools, dosfstools, python3, openssl,
# cargo, and the apps' programs in userspace/apps/bin.
set -u
cd "$(dirname "$0")/.."
EFI=${1:-target/x86_64-unknown-uefi/release/rustux.efi}
[ -f "$EFI" ] || { echo "no $EFI (build it first)"; exit 2; }
OVMF=""
for p in /usr/share/ovmf/OVMF.fd /usr/share/qemu/OVMF.fd /usr/share/edk2-ovmf/x64/OVMF_CODE.fd /usr/share/OVMF/OVMF_CODE.fd; do
    [ -f "$p" ] && OVMF=$p && break
done
[ -n "$OVMF" ] || { echo "OVMF firmware not found"; exit 2; }
for t in qemu-system-x86_64 mformat mmd mcopy mkfs.fat python3 cargo openssl; do
    command -v $t >/dev/null || { echo "$t is needed"; exit 2; }
done
W=$(mktemp -d)
trap 'kill $Q 2>/dev/null; rm -rf "$W"' EXIT
HOSTT=$(rustc -vV | sed -n 's/^host: //p')
(cd userspace/rust/rpg && cargo build -q --release --target "$HOSTT" --target-dir "$W/cargo") || { echo "rpg did not build for this computer"; exit 2; }
RPG="$W/cargo/$HOSTT/release/rpg"

command -v openssl >/dev/null || { echo "openssl is needed"; exit 2; }
export RPG
D=$W/disk
mkdir -p "$D"
openssl genpkey -algorithm ed25519 -out "$W/apps.key" 2>/dev/null
openssl pkey -in "$W/apps.key" -pubout -out "$D/apps.pem"
put() { rel=$("$RPG" pool-path "$2"); mkdir -p "$1/$(dirname "$rel")"; cp "$2" "$1/$rel"; }
scripts/build-apps.sh "$W/apps" org.rustica.calculator org.rustica.sysmon > /dev/null || { echo "the apps did not build"; exit 2; }
cp -r userspace/apps "$W/src"
cp -r "$W/src/org.rustica.calculator" "$W/src/org.test.homeapp"
sed -i 's/org\.rustica\.calculator/org.test.homeapp/; s/^Name: .*/Name: Home App/; s/^Summary: .*/Summary: needs the whole home/' "$W/src/org.test.homeapp/RPG/control" "$W/src/org.test.homeapp/manifest"
printf 'Permissions: files:home network\n' >> "$W/src/org.test.homeapp/manifest"
APPS_SRC=$W/src scripts/build-apps.sh "$W/apps" org.test.homeapp > /dev/null || { echo "homeapp did not build"; exit 2; }
for f in "$W"/apps/*.rpg; do put "$D/good" "$f"; done
"$RPG" index --apps "$D/good" > /dev/null
openssl pkeyutl -sign -inkey "$W/apps.key" -rawin -in "$D/good/platform/1/Release" -out "$D/good/platform/1/Release.sig"
truncate -s 32M "$W/repo.fat"; mkfs.fat -n FOXREPO "$W/repo.fat" >/dev/null
mcopy -s -i "$W/repo.fat" "$D"/* ::

dd if=/dev/zero of="$W/boot.img" bs=1M count=128 status=none
mformat -i "$W/boot.img" -F ::
mmd -i "$W/boot.img" ::/EFI ::/EFI/BOOT
mcopy -i "$W/boot.img" "$EFI" ::/EFI/BOOT/BOOTX64.EFI
echo desktop > "$W/cmdline.txt"
mcopy -i "$W/boot.img" "$W/cmdline.txt" ::/EFI/BOOT/cmdline.txt

rm -f "$W/mon" "$W/qmp" "$W/console.log"
qemu-system-x86_64 -machine q35 -m 512M -smp 2 -bios "$OVMF" -drive file="$W/boot.img",format=raw \
    -drive file="$W/repo.fat",if=none,id=r,format=raw -device virtio-blk-pci,drive=r \
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
# panel's fox button)
check(wait_pixel(25, 787, 0xFF6500, 90), 'the desktop starts at boot')
time.sleep(3)
C, H = 'org.rustica.calculator', 'org.test.homeapp'
NAMES['\\'] = 'backslash'

out = console('mount LABEL=FOXREPO /mnt && rpg key add --apps /mnt/apps.pem apps-test; rpg key list | grep apps-test | cut -d" " -f1 >> /etc/rpg/store-keys; rpg source remove 1; rpg source remove 1; rpg source add apps file:///mnt/good platform 1; rpg update; useradd -m carol; echo carol:pwc | chpasswd; useradd -m -G sudo erin; echo erin:pwe | chpasswd; echo S0-DONE', 40, r'(?<!echo )S0-DONE\r?\n')
check('file:///mnt/good platform 1: 3 apps' in out, 'the apps source is read (3 apps)')

def since(n, pat, t):
    return wait(pat, t, n) is not None

# 1. FoxDen, as root: the list, the Calculator's page, Install, Open
n = len(log())
console('RUSTICA_WINDOW_AT=60,40 foxden %s & sleep 1; echo S1-STARTED' % C, 8, r'(?<!echo )S1-STARTED\r?\n')
check(since(n, r'foxden: 5 apps \(2 installed, 0 to update\)', 20), 'FoxDen lists the source\'s apps (3, none installed) beside the two that come with the system')
check(since(n, r'foxden: page %s' % re.escape(C), 10), 'it opens the Calculator\'s page')
time.sleep(4)
key('ret')
check(since(n, r'foxden: install done %s' % re.escape(C), 60), 'Install installs it (rpg install, its output shown)')
time.sleep(2)
out = console('echo APPS: $(ls /apps); ls /root/.cache/foxden/%s; echo S1B' % C, 5, r'(?<!echo )S1B\r?\n')
check(C in (re.search(r'(?<!echo )APPS:([^\r\n]*)', out) or [None, ''])[1], 'it is in /apps')
check('icon.png' in out and 'screenshot.png' in out, 'its icon and screenshot are cached in ~/.cache/foxden')
n2 = len(log())
qmp_click(25, 782)
time.sleep(4)
key('esc'); time.sleep(1)
out = console('logs | grep "launcher: " | tail -1', 4)
check('launcher: 9 applications, 3 apps' in out, 'Applications lists it (and FoxDen)')
n3 = len(log())
key('ret')
check(since(n3, r'foxden: open %s' % re.escape(C), 10) and since(n3, r'appctl: started %s \(sandboxed\)' % re.escape(C), 15), 'Open starts it, sandboxed')
time.sleep(3)
key('ctrl-q'); time.sleep(2)

# 2. Remove, from a new FoxDen on its page (the focus on Open; Tab: Remove)
n = len(log())
console('RUSTICA_WINDOW_AT=60,40 foxden %s & sleep 1; echo S2-STARTED' % C, 8, r'(?<!echo )S2-STARTED\r?\n')
check(since(n, r'foxden: page %s' % re.escape(C), 15), 'its page again, installed')
time.sleep(4)
key('tab'); time.sleep(0.5)
key('ret'); time.sleep(1.5)
key('ret')
check(since(n, r'foxden: remove done %s' % re.escape(C), 40), 'Remove removes it (after the question)')
out = console('echo APPS: $(ls /apps)', 4)
check(C not in (re.search(r'(?<!echo )APPS:([^\r\n]*)', out) or [None, C])[1], 'it is gone from /apps')

# 3. carol: the files:home app, with an administrator's name and password
n = len(log())
console("su carol -c 'RUSTICA_WINDOW_AT=60,40 foxden %s' & sleep 1; echo S3-STARTED" % H, 8, r'(?<!echo )S3-STARTED\r?\n')
check(since(n, r'foxden: page %s' % re.escape(H), 15), 'carol\'s FoxDen opens the files:home app\'s page')
time.sleep(4)
key('ret')
check(since(n, r"foxden: rpg: %s asks for files:home \(the whole home folder\): an administrator's password is needed" % re.escape(H), 30), 'it needs an administrator: FoxDen asks')
time.sleep(2)
type_('erin'); key('tab'); type_('pwe'); key('ret')
check(since(n, r'foxden: install done %s' % re.escape(H), 60), 'with erin\'s name and password it installs')
out = console('logs -t packagesd | grep "allowed by"', 4)
check('install asked by carol, allowed by erin' in out, 'the installer service logs who allowed it')

# Installed: carol turns the network off for it (its second checkbox)
time.sleep(3)
qmp_click(380, 67)
time.sleep(3)
n = len(log())
qmp_click(85, 237)
check(since(n, r'foxden: permissions %s: files:home\r?\n' % re.escape(H), 10), 'Installed: carol turns its network permission off')
out = console("su carol -c 'appctl permissions %s'" % H, 4)
check(re.search(r'gets: files:home\r?\n\(narrowed', out) is not None, 'appctl gives it files:home only from then on')

# 4. The console
out = console('echo console works', 2)
check('\nconsole works' in out, 'the console works')

if fails:
    print('run-foxden-test: %d checks failed' % fails)
    print(log()[-8000:])
    sys.exit(1)
PY
r=$?
kill $Q 2>/dev/null; wait $Q 2>/dev/null
[ $r -eq 0 ] && echo "run-foxden-test: all checks passed" || echo "run-foxden-test: some checks failed"
exit $r
