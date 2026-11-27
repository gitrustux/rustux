#!/bin/bash
# The apps repository test (FoxDen step FD3, docs/FOXDEN.md,
# docs/TESTS.md). Builds test repositories on the host (the apps of
# userspace/apps, scripts/build-apps.sh; rpg index --apps; signed with a
# test apps key and a test archive key) on a second disk (FAT, APPSREPO),
# read as file:// sources. Boots Rustux in QEMU with "desktop", then at
# the console:
#   1. rpg update reads the apps source ("platform 1: 2 apps"), rpg search
#      finds the Calculator, rpg install shows what it asks for and
#      installs it, rpg list --apps lists it;
#   2. appctl runs it (sandboxed): typed keys reach it, 12+30*2 = 72; the
#      System Monitor installs and starts;
#   3. refused: an apps index signed with the archive key, an apps index
#      that lists a system package, an app in a system repository, Apps.gz
#      changed after signing;
#   4. a newer version in the apps source: rpg upgrade brings it;
#   5. rpg remove; the console works throughout.
# Exit status 0 if all is well.
#
#   scripts/run-appsrepo-test.sh [KERNEL.efi]
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
openssl genpkey -algorithm ed25519 -out "$W/archive.key" 2>/dev/null
openssl pkey -in "$W/apps.key" -pubout -out "$D/apps.pem"
openssl pkey -in "$W/archive.key" -pubout -out "$D/archive.pem"
put() { # repo file.rpg
    rel=$("$RPG" pool-path "$2"); mkdir -p "$1/$(dirname "$rel")"; cp "$2" "$1/$rel"
}
sign() { # repo dists-dir key
    openssl pkeyutl -sign -inkey "$3" -rawin -in "$1/$2/Release" -out "$1/$2/Release.sig"
}
scripts/build-apps.sh "$W/apps1" org.rustica.calculator org.rustica.sysmon > /dev/null || { echo "the apps did not build"; exit 2; }
# good: both apps, signed with the apps key
for f in "$W"/apps1/*.rpg; do put "$D/good" "$f"; done
"$RPG" index --apps "$D/good" > /dev/null && sign "$D/good" platform/1 "$W/apps.key"
# good2: the Calculator 1.1.0
cp -r userspace/apps "$W/src2"
sed -i 's/^Version: .*/Version: 1.1.0/' "$W/src2/org.rustica.calculator/RPG/control"
APPS_SRC=$W/src2 scripts/build-apps.sh "$W/apps2" org.rustica.calculator > /dev/null
for f in "$W"/apps2/*.rpg "$W"/apps1/org.rustica.sysmon_*.rpg; do put "$D/good2" "$f"; done
"$RPG" index --apps "$D/good2" > /dev/null && sign "$D/good2" platform/1 "$W/apps.key"
# syskey: the same apps, signed with the archive key
cp -r "$D/good" "$D/syskey" && sign "$D/syskey" platform/1 "$W/archive.key"
# withsys: an apps index listing a system package too, signed with the apps key
cp -r "$D/good" "$D/withsys"
python3 - "$D/withsys/platform/1" <<'PYI'
import gzip, hashlib, sys
d = sys.argv[1]
t = gzip.decompress(open(d + '/x86_64/Apps.gz', 'rb').read()).decode()
t += '\nPackage: hello\nVersion: 1.0\nArchitecture: x86_64\nDescription: a system package\nFilename: pool/h/hello/hello_1.0_x86_64.rpg\nSize: 10\nSHA256: ' + '0' * 64 + '\n'
gz = gzip.compress(t.encode())
open(d + '/x86_64/Apps.gz', 'wb').write(gz)
rel = open(d + '/Release').read().splitlines()
rel = [(' %s %d x86_64/Apps.gz' % (hashlib.sha256(gz).hexdigest(), len(gz))) if l.endswith(' x86_64/Apps.gz') else l for l in rel]
open(d + '/Release', 'w').write('\n'.join(rel) + '\n')
PYI
sign "$D/withsys" platform/1 "$W/apps.key"
# tampered: Apps.gz changed after signing
cp -r "$D/good" "$D/tampered"
printf 'x' >> "$D/tampered/platform/1/x86_64/Apps.gz"
# sysrepo: a system repository (the archive key) with an app in it
mkdir -p "$W/sneaky/RPG" "$W/sneaky/bin"
cp userspace/apps/bin/calculator "$W/sneaky/bin/sneaky"
printf 'Package: org.test.sneaky\nVersion: 1.0\nArchitecture: x86_64\nKind: app\nDescription: an app in a system repository\n' > "$W/sneaky/RPG/control"
printf 'Id: org.test.sneaky\nName: Sneaky\nSummary: should not install\nExec: bin/sneaky\nPlatform: 1\n' > "$W/sneaky/manifest"
"$RPG" build "$W/sneaky" -o "$W/sneaky.rpg" > /dev/null
mkdir -p "$D/sysrepo/pool/main/o/org.test.sneaky" && cp "$W/sneaky.rpg" "$D/sysrepo/pool/main/o/org.test.sneaky/org.test.sneaky_1.0_x86_64.rpg"
"$RPG" index "$D/sysrepo" > /dev/null && sign "$D/sysrepo" dists/redfox "$W/archive.key"
truncate -s 32M "$W/repo.fat"; mkfs.fat -n APPSREPO "$W/repo.fat" >/dev/null
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
C = 'org.rustica.calculator'
V = re.search(r'^Version: *(\S+)', open('userspace/apps/org.rustica.calculator/RPG/control').read(), re.M).group(1)   # the Calculator's version in userspace/apps

# 1. The apps source
out = console('mount LABEL=APPSREPO /mnt && rpg key add --apps /mnt/apps.pem apps-test && rpg key add /mnt/archive.pem archive-test; rpg source remove 1; rpg source remove 1; rpg source add apps file:///mnt/good platform 1; rpg update; rpg search calculator; rpg install -y %s; rpg list --apps; echo S1-DONE' % C, 40, r'S1-DONE\r?\n')
check('file:///mnt/good platform 1: 2 apps' in out, 'rpg update reads the apps source (2 apps)')
check('org.rustica.calculator %s - Calculator: add, subtract, multiply and divide (app)' % V in out, 'rpg search finds the Calculator')
check('asks for: nothing (its own folder and data, a window)' in out, 'rpg install shows what it asks for')
check('org.rustica.calculator %s: installed' % V in out and re.search(r'org\.rustica\.calculator +%s +Calculator' % re.escape(V), out) is not None, 'rpg install installs it; rpg list --apps lists it')

# 2. Running them (from the console: what they print shows there); keys
# typed into the Calculator (it has the focus)
n2 = len(log())
out = console('rpg install -y org.rustica.sysmon; RUSTICA_WINDOW_AT=300,150 appctl run %s & RUSTICA_WINDOW_AT=700,150 appctl run org.rustica.sysmon & sleep 1; echo S2-DONE' % C, 25, r'S2-DONE\r?\n')
time.sleep(4)
qmp_click(450, 190)
type_('12+30*2\n')
time.sleep(2)
out = log()[n2:]
check('appctl: started %s (sandboxed)' % C in out, 'appctl runs the Calculator sandboxed')
check('calculator: 12+30*2 = 72' in out, 'typed keys reach it: 12+30*2 = 72')
check(re.search(r'sysmon: \d+ CPUs, \d+ MiB of \d+ MiB memory in use', out) is not None, 'the System Monitor installs and starts')

# 3. What is refused
out = console('rpg source remove 1; rpg source add apps file:///mnt/syskey platform 1; rpg update; rpg source remove 1; rpg source add apps file:///mnt/withsys platform 1; rpg update; rpg source remove 1; rpg source add apps file:///mnt/tampered platform 1; rpg update; rpg source remove 1; rpg source add file:///mnt/sysrepo redfox main; rpg update; rpg install -y org.test.sneaky; ls /apps; echo S3-DONE', 40, r'S3-DONE\r?\n')
check('syskey platform 1: REFUSED: the signature of Release is not by a trusted apps key' in out, 'an apps index signed with the archive key is refused')
check('withsys platform 1: REFUSED: ' in out and 'hello: a system package in an app source: refused' in out, 'an apps index listing a system package is refused')
check('tampered platform 1: REFUSED: ' in out and "differs from Release's" in out, 'Apps.gz changed after signing is refused')
check('an app in a system repository: refused' in out and 'org.test.sneaky' not in out.split('ls /apps')[-1].split('S3-DONE')[0], 'an app in a system repository is refused')

# 4. A newer version from the apps source
out = console('rpg source remove 1; rpg source add apps file:///mnt/good2 platform 1; rpg update; rpg upgrade -y; rpg list --apps; echo S4-DONE', 40, r'S4-DONE\r?\n')
check('org.rustica.calculator %s -> 1.1.0' % V in out and re.search(r'org\.rustica\.calculator +1\.1\.0', out) is not None, 'rpg upgrade brings the Calculator 1.1.0')

# 5. Removing it
out = console('rpg remove %s; ls /apps; echo S5-DONE' % C, 15, r'S5-DONE\r?\n')
check('%s 1.1.0: removed' % C in out and C not in out.split('ls /apps')[-1].split('S5-DONE')[0], 'rpg remove removes it')
out = console('echo console works', 2)
check('\nconsole works' in out, 'the console works')

if fails:
    print('run-appsrepo-test: %d checks failed' % fails)
    print(log()[-6000:])
    sys.exit(1)
PY
r=$?
kill $Q 2>/dev/null; wait $Q 2>/dev/null
[ $r -eq 0 ] && echo "run-appsrepo-test: all checks passed" || echo "run-appsrepo-test: some checks failed"
exit $r
