#!/bin/bash
# The apps from outside FoxDen test (FoxDen step FD7c, docs/FOXDEN.md
# "Apps from outside FoxDen", docs/TESTS.md). Boots Rustux in QEMU with
# "desktop" and a FAT disk: a FoxDen stand-in (an app source signed by a
# key listed in /etc/rpg/store-keys) with org.test.store 1.0.0, a third
# party's source (another key) with org.test.store 1.0.1 and
# org.test.other, and package files. At the console:
#   1. rpg list --apps --available: From foxden, From source ...;
#   2. org.test.store comes from FoxDen (the third party's newer 1.0.1 of
#      that ID is not offered, nor its upgrade: it stays at 1.0.0);
#   3. a package file installs with the warning (Origin file); a package
#      file of org.test.store is refused: it does not replace the FoxDen
#      one; org.test.other from the third party: the warning;
#   4. carol (not an administrator) is refused a package file; root: rpg
#      config outside anyone; carol installs one, no password; one asking
#      for files:home still needs an administrator; carol cannot change
#      the setting; rpg list --apps --file shows a file's app (From file);
#   5. developer mode: carol's app folder's sandbox (appctl view --dir:
#      the folder read only, not her home); an app run from a folder
#      (appctl run --dir): typed keys reach it, it is not installed;
#   6. FoxDen opens a package file (foxden FILE.rpg: not from FoxDen);
#   7. the console works.
# $SHOTS=DIR: a screendump of FoxDen's page for the package file there.
# Exit status 0 if all is well.
#
#   scripts/run-sideload-test.sh [KERNEL.efi]
#
# Needs qemu-system-x86_64, OVMF, mtools, dosfstools, python3, openssl and
# cargo (rpg for this computer).
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
Q=""
trap 'kill $Q 2>/dev/null; rm -rf "$W"' EXIT
HOSTT=$(rustc -vV | sed -n 's/^host: //p')
(cd userspace/rust/rpg && cargo build -q --release --target "$HOSTT" --target-dir "$W/cargo") || { echo "rpg did not build for this computer"; exit 2; }
export RPG="$W/cargo/$HOSTT/release/rpg"
D=$W/disk
mkdir -p "$D"
for k in store third; do
    openssl genpkey -algorithm ed25519 -out "$W/$k.key" 2>/dev/null
    openssl pkey -in "$W/$k.key" -pubout -out "$D/$k.pem"
done
put() { rel=$("$RPG" pool-path "$2"); mkdir -p "$1/$(dirname "$rel")"; cp "$2" "$1/$rel"; }
sign() { openssl pkeyutl -sign -inkey "$3" -rawin -in "$1/platform/1/Release" -out "$1/platform/1/Release.sig"; }
# Apps made of the Calculator's folder: ID NAME VERSION [PERMISSIONS]
mkapp() {
    s=$W/src/$1
    rm -rf "$s"; mkdir -p "$W/src"; [ -d "$W/src/bin" ] || cp -r userspace/apps/bin "$W/src/bin"; cp -r userspace/apps/org.rustica.calculator "$s"
    sed -i "s/org\.rustica\.calculator/$1/; s/^Name: .*/Name: $2/" "$s/RPG/control" "$s/manifest"
    sed -i "s/^Version: .*/Version: $3/" "$s/RPG/control"
    [ -n "${4:-}" ] && printf 'Permissions: %s\n' "$4" >> "$s/manifest"
    APPS_SRC=$W/src scripts/build-apps.sh "$W/out" "$1" > /dev/null || { echo "$1 did not build"; exit 2; }
}
mkapp org.test.store "Store App" 1.0.0
put "$D/store" "$W/out/org.test.store_1.0.0_x86_64.rpg"
"$RPG" index --apps "$D/store" > /dev/null && sign "$D/store" x "$W/store.key"
mkapp org.test.store "Store App" 1.0.1
mkapp org.test.other "Other App" 1.0.0
put "$D/third" "$W/out/org.test.store_1.0.1_x86_64.rpg"
put "$D/third" "$W/out/org.test.other_1.0.0_x86_64.rpg"
"$RPG" index --apps "$D/third" > /dev/null && sign "$D/third" x "$W/third.key"
mkapp org.test.store "Store App" 1.0.2
cp "$W/out/org.test.store_1.0.2_x86_64.rpg" "$D/store-file.rpg"
mkapp org.test.side "Side App" 1.0.0
cp "$W/out/org.test.side_1.0.0_x86_64.rpg" "$D/side.rpg"
mkapp org.test.carol "Carol App" 1.0.0
cp "$W/out/org.test.carol_1.0.0_x86_64.rpg" "$D/carol.rpg"
mkapp org.test.wide "Wide App" 1.0.0 files:home
cp "$W/out/org.test.wide_1.0.0_x86_64.rpg" "$D/wide.rpg"
mkapp org.test.shown "Shown App" 1.0.0
cp "$W/out/org.test.shown_1.0.0_x86_64.rpg" "$D/shown.rpg"
# A developer's folder: the Calculator's, as org.test.dev (manifest, program)
mkdir -p "$D/dev/bin"
sed 's/org\.rustica\.calculator/org.test.dev/; s/^Name: .*/Name: Dev App/' userspace/apps/org.rustica.calculator/manifest > "$D/dev/manifest"
cp userspace/apps/bin/calculator "$D/dev/bin/calculator"
truncate -s 48M "$W/repo.fat"; mkfs.fat -n SIDEREPO "$W/repo.fat" >/dev/null
mcopy -s -i "$W/repo.fat" "$D"/* ::

dd if=/dev/zero of="$W/boot.img" bs=1M count=64 status=none
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
# panel's Applications button)
check(wait_pixel(64, 782, 0xFF5A1F, 90), 'the desktop starts at boot')
time.sleep(3)
S, O = 'org.test.store', 'org.test.other'
NAMES['?'] = 'shift-slash'
NAMES['^'] = 'shift-6'
NAMES['~'] = 'shift-grave_accent'

def sh(cmd, mark, t=60):
    n = len(log())
    console(cmd + '; echo ' + mark, t, r'(?<!echo )' + mark + r'\r?\n')
    return log()[n:]

# Setup: the store stand-in's key in /etc/rpg/store-keys
out = sh("mount LABEL=SIDEREPO /mnt && rpg key add --apps /mnt/store.pem store-test && rpg key add --apps /mnt/third.pem third-test; "
         "rpg key list | grep store-test | cut -d' ' -f1 >> /etc/rpg/store-keys; rpg source remove 1; rpg source remove 1; "
         "rpg source add apps file:///mnt/store platform 1; rpg source add apps file:///mnt/third platform 1; "
         "useradd -m carol; echo carol:pwc | chpasswd; svc restart packagesd; sleep 2; rpg update", 'SETUP-DONE', 120)
check(re.search(r'file:///mnt/third platform 1: 2 apps', out) is not None, 'setup: the two app sources')

# 1. Where they are from
out = sh('rpg list --apps --available | grep -E "^(Package|From):"', 'S1-DONE', 20)
check(re.search(r'Package: %s\r?\nFrom: foxden' % re.escape(O), out) is None and re.search(r'Package: %s\r?\nFrom: source file:///mnt/third' % re.escape(O), out) is not None, 'rpg list --apps --available: the third party\'s app is From its source')
check(re.search(r'Package: %s\r?\nFrom: foxden' % re.escape(S), out) is not None and len(re.findall(r'Package: %s\r?\n' % re.escape(S), out)) == 1, 'org.test.store is FoxDen\'s: the third party\'s newer one of that ID is not offered')

# 2. From FoxDen; the third party's newer is not its upgrade
out = sh('rpg install -y %s' % S, 'S2-DONE', 60)
check('not from FoxDen' not in out, 'it installs from FoxDen without the warning')
out2 = sh('rpg list --apps --available | grep -A30 "^Package: %s" | grep Installed-From; rpg upgrade -y; rpg list --apps' % S, 'S2B-DONE', 60)
check(re.search(r'Installed-From: foxden', out2) is not None, 'it is installed from FoxDen (Installed-From: foxden)')
check(re.search(r'%s +1\.0\.0' % re.escape(S), out2.split('upgrade -y')[-1]) is not None and re.search(r'%s +1\.0\.1' % re.escape(S), out2.split('upgrade -y')[-1]) is None, 'rpg upgrade does not take the third party\'s newer version')

# 3. Package files and the third party
out = sh('rpg install /mnt/side.rpg; rpg list --apps --available | grep -A30 "^Package: org.test.side" | grep Installed-From', 'S3A', 60)
check('org.test.side 1.0.0 is not from FoxDen (a package file): nobody has checked it' in out and 'Installed-From: file' in out, 'a package file installs with the warning (Installed-From: file)')
out = sh('rpg install /mnt/store-file.rpg; echo CODE-$?; rpg list --apps', 'S3B', 60)
check('installed from FoxDen; a package file does not replace it' in out and re.search(r'%s +1\.0\.0' % re.escape(S), out) is not None, 'a package file of the FoxDen app is refused')
out = sh('rpg install -y %s' % O, 'S3C', 60)
check('%s 1.0.0 is not from FoxDen (the app source file:///mnt/third): nobody has checked it' % O in out, 'the third party\'s app installs with the warning')

# 4. Who may install package files
out = sh("echo | su carol -c 'rpg install /mnt/carol.rpg'; echo APPS: $(ls /apps)", 'S4A', 30)
check("installing a package file (not from FoxDen): an administrator's password is needed" in out and 'org.test.carol' not in out.split('APPS:')[-1], 'carol is refused a package file')
out = sh('rpg config outside anyone; rpg config; cat /etc/rpg/install.conf', 'S4B', 20)
check('outside anyone' in out, 'root: rpg config outside anyone')
out = sh("echo | su carol -c 'rpg install /mnt/carol.rpg'; echo APPS: $(ls /apps)", 'S4C', 60)
check('org.test.carol' in out.split('APPS:')[-1] and 'password' not in out, 'carol installs a package file, no password')
out = sh("echo | su carol -c 'rpg install /mnt/wide.rpg'; echo APPS: $(ls /apps)", 'S4D', 30)
check("org.test.wide asks for files:home (the whole home folder): an administrator's password is needed" in out and 'org.test.wide' not in out.split('APPS:')[-1], 'one asking for files:home still needs an administrator')
out = sh("echo | su carol -c 'rpg config outside admin'; rpg config", 'S4E', 30)
check("an administrator's password is needed" in out and re.search(r'\noutside anyone\r?\n', out) is not None, 'carol cannot change the setting')
out = sh("su carol -c 'rpg list --apps --file /mnt/shown.rpg' | grep -E '^(Package|From|App-Name):'", 'S4F', 20)
check('Package: org.test.shown' in out and 'From: file' in out, 'rpg list --apps --file: a package file\'s app')

# 5. Developer mode: carol's view (no administrator); run in root's
# session (the desktop's)
out = sh("su carol -c 'mkdir -p ~/dev && cp -r /mnt/dev ~/dev/app && chmod +x ~/dev/app/bin/calculator && appctl view --dir ~/dev/app'", 'S5A', 30)
check('developer mode: org.test.dev from /home/carol/dev/app' in out and 'ro /home/carol/dev/app' in out and re.search(r'\nrw /home/carol\r?\n', out) is None, 'carol: appctl view --dir: the folder read only, not the home')
n = len(log())
console("mkdir -p /root/dev && cp -r /mnt/dev /root/dev/app && chmod +x /root/dev/app/bin/calculator; RUSTICA_WINDOW_AT=300,150 appctl run --dir /root/dev/app & sleep 1; echo S5B", 10, r'(?<!echo )S5B\r?\n')
time.sleep(6)
type_('6*7\n')
check(wait(r'calculator: 6\*7 = 42', 15, n) is not None, 'appctl run --dir: it runs in its sandbox (6*7 = 42)')
key('esc')
time.sleep(1)
out = sh('rpg list --apps | grep -c org.test.dev', 'S5C', 10)
check(re.search(r'\n0\r?\nS5C', out) is not None, 'it is not installed')

# 6. FoxDen opens a package file
n = len(log())
console("foxden /mnt/shown.rpg & sleep 1; echo S6", 6, r'(?<!echo )S6\r?\n')
check(wait(r'foxden: package file /mnt/shown\.rpg: org\.test\.shown 1\.0\.0, not from FoxDen', 30, n) is not None, 'FoxDen shows a package file\'s app, not from FoxDen')
time.sleep(4)
if os.environ.get('SHOTS'):
    mon('screendump %s/foxden-file.ppm' % os.environ['SHOTS'])
    time.sleep(1.5)
key('ctrl-q')

# 7. The console
out = console('echo console works', 2)
check('\nconsole works' in out, 'the console works')

if fails:
    print('run-sideload-test: %d checks failed' % fails)
    print(log()[-8000:])
    sys.exit(1)
PY
r=$?
kill $Q 2>/dev/null; wait $Q 2>/dev/null
[ $r -eq 0 ] && echo "run-sideload-test: all checks passed" || echo "run-sideload-test: some checks failed"
exit $r
