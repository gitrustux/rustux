#!/bin/bash
# The app updates and file chooser test (FoxDen step FD6, docs/FOXDEN.md,
# docs/TESTS.md). Two apps sources made on the host on a second disk: the
# first with the Calculator, the System Monitor and org.test.portal (an app
# around portaltest, with no files: permission); the second with the
# Calculator 1.1.0 (asking for nothing more) and the System Monitor 1.1.0
# (asking for network too). Boots Rustux in QEMU with "desktop", then:
#   1. the three installed from the first; the source switched to the
#      second; the installer service's check (updates check) upgrades the
#      Calculator at once and leaves the System Monitor waiting (it asks for
#      more): the status says so, the panel notifies both;
#   2. the user says yes (rpg install): the System Monitor 1.1.0; nothing
#      waits any more;
#   3. org.test.portal: ~/Documents/note.txt does not open by its path; the
#      file chooser (keys typed into it) gives it note.txt, which it reads,
#      and nothing beside it; it saves copy.txt through the chooser; a
#      question the user cancels gives it nothing;
#   4. the console works.
# Exit status 0 if all is well.
#
#   scripts/run-appupdates-test.sh [KERNEL.efi]
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
sign() { "$RPG" index --apps "$1" > /dev/null && openssl pkeyutl -sign -inkey "$W/apps.key" -rawin -in "$1/platform/1/Release" -out "$1/platform/1/Release.sig"; }
scripts/build-apps.sh "$W/apps1" org.rustica.calculator org.rustica.sysmon > /dev/null || { echo "the apps did not build"; exit 2; }
# org.test.portal: portaltest as an app, no permissions
mkdir -p "$W/src/org.test.portal/RPG"
cp userspace/bin/portaltest "$W/src/org.test.portal/portaltest-bin"
mkdir -p "$W/src/bin" && cp userspace/bin/portaltest "$W/src/bin/portaltest"
printf 'Package: org.test.portal\nVersion: 1.0\nArchitecture: x86_64\nKind: app\nDescription: the file chooser test\n' > "$W/src/org.test.portal/RPG/control"
printf 'Id: org.test.portal\nName: Portal Test\nSummary: opens a file the user picks\nExec: bin/portaltest\nPlatform: 1\n' > "$W/src/org.test.portal/manifest"
rm "$W/src/org.test.portal/portaltest-bin"
APPS_SRC=$W/src scripts/build-apps.sh "$W/apps1" org.test.portal > /dev/null || { echo "portal app did not build"; exit 2; }
for f in "$W"/apps1/*.rpg; do put "$D/one" "$f"; done
sign "$D/one"
# two: the Calculator 1.1.0 as it was, the System Monitor 1.1.0 with network
cp -r userspace/apps "$W/src2"
sed -i 's/^Version: .*/Version: 1.1.0/' "$W/src2/org.rustica.calculator/RPG/control" "$W/src2/org.rustica.sysmon/RPG/control"
sed -i 's/^Platform: 1/Permissions: network\nPlatform: 1/' "$W/src2/org.rustica.sysmon/manifest"
APPS_SRC=$W/src2 scripts/build-apps.sh "$W/apps2" org.rustica.calculator org.rustica.sysmon > /dev/null || { echo "the new versions did not build"; exit 2; }
for f in "$W"/apps2/*.rpg "$W"/apps1/org.test.portal_*.rpg; do put "$D/two" "$f"; done
sign "$D/two"
truncate -s 32M "$W/repo.fat"; mkfs.fat -n UPREPO "$W/repo.fat" >/dev/null
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
check(wait_pixel(64, 782, 0xBD93F9, 90), 'the desktop starts at boot')
time.sleep(3)
C, S, P = 'org.rustica.calculator', 'org.rustica.sysmon', 'org.test.portal'
NAMES['^'] = 'shift-6'
V = re.search(r'^Version: *(\S+)', open('userspace/apps/org.rustica.calculator/RPG/control').read(), re.M).group(1)

def since(n, pat, t):
    return wait(pat, t, n) is not None

def sh(cmd, mark, t=60):
    n = len(log())
    console(cmd + '; echo ' + mark, t, r'(?<!echo )' + mark + r'\r?\n')
    return log()[n:]

# 1. Installed from one; the check with two
out = sh('mount LABEL=UPREPO /mnt && rpg key add --apps /mnt/apps.pem apps-test; rpg key list | grep apps-test | cut -d" " -f1 >> /etc/rpg/store-keys; rpg source remove 1; rpg source remove 1; rpg source add apps file:///mnt/one platform 1; rpg update; rpg install -y %s %s %s; mkdir -p /root/Documents; echo "a note for portaltest" > /root/Documents/note.txt' % (C, S, P), 'S0-DONE', 90)
check('%s %s: installed' % (C, V) in out and '%s %s: installed' % (S, V) in out and '%s 1.0: installed' % P in out, 'the three apps installed')
out = sh('rpg source remove 1; rpg source add apps file:///mnt/two platform 1; updates check; rpg list --apps; updates status --raw | grep "^app"', 'S1-DONE', 120)
check(re.search(r'%s +1\.1\.0' % re.escape(C), out) is not None, 'the check upgrades the Calculator at once (it asks for nothing more)')
check(re.search(r'%s +%s ' % (re.escape(S), re.escape(V)), out) is not None, 'the System Monitor is not upgraded (1.1.0 asks for network)')
check('appwaits %s %s 1.1.0 network' % (S, V) in out, 'the status: it waits for the user')
check(re.search(r'appupdated \d+ %s %s 1\.1\.0' % (re.escape(C), re.escape(V)), out) is not None, 'the status: the Calculator was updated')
out = sh('logs -t packagesd | grep -E "app updated|waits for the user"', 'S1B', 10)
check('app updated: %s %s -> 1.1.0' % (C, V) in out and 'app update waits for the user: %s 1.1.0 asks for more (network)' % S in out, 'the installer service logs both')
# The panel looks every 30 seconds
n = len(log())
ok = False
for _ in range(9):
    time.sleep(6)
    out = console('logs | grep "panel: app update"', 4)
    if 'panel: app updated %s 1.1.0' % C in out and 'panel: app update waits %s 1.1.0' % S in out:
        ok = True
        break
check(ok, 'the panel notifies both (updated; asks for more)')

# 2. The user says yes
out = sh('rpg install -y %s; updates check; rpg list --apps; echo WAITS $(updates status --raw | grep -c "^appwaits")' % S, 'S2-DONE', 120)
check(re.search(r'%s +1\.1\.0' % re.escape(S), out) is not None and 'asks for: network' in out, 'with the user\'s yes, the System Monitor 1.1.0 (asks for network)')
check(re.search(r'(?<!echo )WAITS 0', out) is not None, 'nothing waits any more')
out = sh('rpg permissions %s revoke network; appctl permissions %s' % (S, S), 'S2B', 10)
check('%s: gets: nothing (from its next start)' % S in out and re.search(r'gets:\s*\r?\n\(narrowed', out) is not None, 'rpg permissions revokes it again (appctl agrees)')

# 3. The file chooser
n = len(log())
console('appctl run %s & sleep 1; echo S3-STARTED' % P, 8, r'(?<!echo )S3-STARTED\r?\n')
check(since(n, r'portaltest: path-refused ok', 15), 'the app cannot open ~/Documents/note.txt by its path')
time.sleep(5)
type_('note.txt\n')
check(since(n, r'portaltest: read \d+ bytes from note\.txt: a note for portaltest', 20), 'the chooser gives it note.txt, which it reads')
time.sleep(5)
key('ret')
check(since(n, r'portaltest: chooser-save ok', 20), 'it saves through the chooser (copy.txt, the name it suggests)')
time.sleep(5)
key('esc')
check(since(n, r'portaltest: passed (\d+) of \1\r?\n', 20), 'every check of the app passes (no folder beside the file, read only, the path still refused, a cancelled question gives nothing)')
out = log()[n:]
for l in out.splitlines():
    if 'portaltest:' in l and 'FAIL' in l:
        print('       ' + l.strip())
check('appctl: %s: the user chose /root/Documents/note.txt (open)' % P in out and 'appctl: %s: the user said no (open)' % P in out, 'appctl says what the user chose, and when they said no')
out = console('cat /root/Documents/copy.txt', 4)
check('a copy saved by portaltest' in out, 'copy.txt has what the app wrote')

# 4. The console
out = console('echo console works', 2)
check('\nconsole works' in out, 'the console works')

if fails:
    print('run-appupdates-test: %d checks failed' % fails)
    print(log()[-8000:])
    sys.exit(1)
PY
r=$?
kill $Q 2>/dev/null; wait $Q 2>/dev/null
[ $r -eq 0 ] && echo "run-appupdates-test: all checks passed" || echo "run-appupdates-test: some checks failed"
exit $r
