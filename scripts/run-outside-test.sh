#!/bin/bash
# The outside developers test (FoxDen step FD7a, docs/FOXDEN.md "Outside
# developers", docs/TESTS.md). On the host, as the foxden-apps repository
# and rustux.com's publishing would:
#   1. a developer (alice) signs her app's folder (rpg dev digest, openssl)
#      and it is built from its source (scripts/build-outside-app.sh: the
#      Calculator's source as io.github.alice.dice); rpg dev verify and
#      rpg dev check-package pass; a folder changed after signing, a
#      package with a file the folder lacks, and an app outside its
#      developer's namespace are refused;
#   2. scripts/fetch-outside-apps.sh takes it, scripts/publish-apps.sh
#      --add signs it into a test apps repository with the project's apps;
#      one in org.rustica is refused;
# then boots Rustux in QEMU with "desktop", and at the console:
#   3. rpg update; rpg show lists its developer and source; it installs and
#      runs sandboxed (typed keys reach it);
#   4. a newer version of the same ID from another developer (io.github)
#      is refused by rpg upgrade: an app does not pass to another
#      developer; it stays at 1.0.0;
#   5. the console works.
# Exit status 0 if all is well.
#
#   scripts/run-outside-test.sh [KERNEL.efi]
#
# Needs qemu-system-x86_64, OVMF, mtools, dosfstools, python3, openssl,
# cargo with rust-src, gcc, and the apps' programs in userspace/apps/bin.
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
hfail=0
hcheck() { # ok? what
    if [ "$1" = 0 ]; then echo "  ok   $2"; else echo "  FAIL $2"; hfail=$((hfail + 1)); fi
}
D=$W/disk
mkdir -p "$D"
openssl genpkey -algorithm ed25519 -out "$W/apps.key" 2>/dev/null
openssl pkey -in "$W/apps.key" -pubout -out "$W/apps.pem"
openssl pkey -in "$W/apps.key" -pubout -outform DER | tail -c 32 | od -An -tx1 | tr -d ' \n' | sed 's/^/rpg-ed25519 /; s/$/ apps-test\n/' > "$W/apps.pub"
cp "$W/apps.pem" "$D/apps.pem"

# 1. alice's app: the foxden-apps layout
F=$W/foxden-apps
A=$F/apps/io.github.alice.dice
mkdir -p "$F/developers/io.github.alice" "$A/RPG" "$A/src"
openssl genpkey -algorithm ed25519 -out "$W/alice.key" 2>/dev/null
openssl pkey -in "$W/alice.key" -pubout -out "$F/developers/io.github.alice/key.pub"
printf 'Id: io.github.alice\nName: Alice\nGitHub: alice\n' > "$F/developers/io.github.alice/developer"
cp userspace/rust/foxden-apps/src/bin/calculator.rs "$A/src/main.rs"
cp userspace/apps/org.rustica.calculator/icon.png "$A/icon.png"
printf 'Id: io.github.alice.dice\nName: Alice Dice\nSummary: the outside test app\nExec: bin/app\nIcon: icon.png\nCategories: Utility\nPlatform: 1\nDeveloper: io.github.alice\nDeveloper-Name: Alice\nSource: https://github.com/gitrustux/foxden-apps/tree/main/apps/io.github.alice.dice\n' > "$A/manifest"
printf 'Package: io.github.alice.dice\nVersion: 1.0.0\nArchitecture: x86_64\nKind: app\nLicense: MIT\nMaintainer: Alice <alice@example.org>\nDescription: Alice Dice\n An outside developer'"'"'s app.\n' > "$A/RPG/control"
signit() { # dir key
    "$RPG" dev digest "$1" > "$W/digest" && openssl pkeyutl -sign -inkey "$2" -rawin -in "$W/digest" -out "$1/SIGNATURE"
}
signit "$A" "$W/alice.key"
"$RPG" dev verify "$A" "$F/developers/io.github.alice/key.pub" > /dev/null; hcheck $? "rpg dev verify: alice's folder is signed by her key"
scripts/build-outside-app.sh "$A" "$W/built" > "$W/build.out" 2>&1; hcheck $? "build-outside-app.sh builds it from its source"
P=$W/built/io.github.alice.dice_1.0.0_x86_64.rpg
"$RPG" dev check-package "$P" "$A" > /dev/null; hcheck $? "rpg dev check-package: the package is the folder built"
cp -r "$A" "$W/changed"; echo '// changed' >> "$W/changed/src/main.rs"
! "$RPG" dev verify "$W/changed" "$F/developers/io.github.alice/key.pub" > /dev/null 2>&1; hcheck $? "a folder changed after signing is refused"
cp -r "$A" "$W/x"; rm -rf "$W/x/src" "$W/x/SIGNATURE"; mkdir -p "$W/x/bin"; cp userspace/apps/bin/calculator "$W/x/bin/app"; echo hidden > "$W/x/hidden"
"$RPG" build "$W/x" -o "$W/extra.rpg" > /dev/null
! "$RPG" dev check-package "$W/extra.rpg" "$A" > /dev/null 2>&1; hcheck $? "a package with a file the folder lacks is refused"
cp -r "$W/x" "$W/y"; rm "$W/y/hidden"; sed -i 's/^Developer: .*/Developer: io.github.bob/' "$W/y/manifest"
! "$RPG" build "$W/y" -o "$W/y.rpg" > /dev/null 2>&1; hcheck $? "an app outside its developer's namespace is refused (rpg build)"

# 2. fetched and published with the project's apps
"$PWD/scripts/fetch-outside-apps.sh" "$F" "$W/fetched" --from "$W/built" > "$W/fetch.out" 2>&1
grep -q 'io.github.alice.dice 1.0.0 by io.github.alice: signed, the folder built' "$W/fetch.out"; hcheck $? "fetch-outside-apps.sh checks and takes it"
# (the repository two folders below a web root, as on the server: the key
# must not be inside it)
scripts/publish-apps.sh --key "$W/apps.key" --pub "$W/apps.pub" --add "$W/fetched" "$W/web/download/apps" > "$W/publish.out" 2>&1
grep -q 'io.github.alice.dice 1.0.0' "$W/publish.out"; hcheck $? "publish-apps.sh --add publishes it with the project's apps"
cp -r "$W/web/download/apps" "$D/good"
mkdir -p "$W/own"; cp userspace/apps/org.rustica.calculator/icon.png "$W/own/org.rustica.fake_1.0_x86_64.rpg"
! scripts/publish-apps.sh --key "$W/apps.key" --pub "$W/apps.pub" --add "$W/own" "$W/web/download/other" > "$W/own.out" 2>&1 && grep -q "project's own namespace" "$W/own.out"; hcheck $? "publish-apps.sh --add refuses the project's own namespace"
# evil: 1.0.1 of the same ID, from "io.github" (another developer)
cp -r "$A" "$W/evil"
sed -i 's/^Developer: .*/Developer: io.github/; s/^Developer-Name: .*/Developer-Name: Mallory/' "$W/evil/manifest"
sed -i 's/^Version: .*/Version: 1.0.1/' "$W/evil/RPG/control"
rm -f "$W/evil/SIGNATURE"
scripts/build-outside-app.sh "$W/evil" "$W/evilbuilt" > /dev/null 2>&1; hcheck $? "another developer's 1.0.1 of the same ID builds"
mkdir -p "$D/evil/pool"
rel=$("$RPG" pool-path "$W/evilbuilt/io.github.alice.dice_1.0.1_x86_64.rpg"); mkdir -p "$D/evil/$(dirname "$rel")"; cp "$W/evilbuilt/io.github.alice.dice_1.0.1_x86_64.rpg" "$D/evil/$rel"
"$RPG" index --apps "$D/evil" > /dev/null && openssl pkeyutl -sign -inkey "$W/apps.key" -rawin -in "$D/evil/platform/1/Release" -out "$D/evil/platform/1/Release.sig"
hcheck $? "it is in a test apps repository (signed with the apps key)"
[ $hfail = 0 ] || { echo "run-outside-test: $hfail host checks failed"; cat "$W/build.out" "$W/fetch.out" "$W/publish.out"; exit 1; }

truncate -s 48M "$W/repo.fat"; mkfs.fat -n APPSREPO "$W/repo.fat" >/dev/null
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
X = 'io.github.alice.dice'
NAMES['?'] = 'shift-slash'

def sh(cmd, mark, t=60):
    n = len(log())
    console(cmd + '; echo ' + mark, t, r'(?<!echo )' + mark + r'\r?\n')
    return log()[n:]

# 3. From the test repository
out = sh('mount LABEL=APPSREPO /mnt && rpg key add --apps /mnt/apps.pem apps-test; rpg source remove 1; rpg source remove 1; rpg source add apps file:///mnt/good platform 1; rpg update; rpg show %s; rpg install -y %s; rpg list --apps' % (X, X), 'S3-DONE', 60)
check(re.search(r'file:///mnt/good platform 1: \d+ apps', out) is not None, 'rpg update reads the repository with the outside app')
check('App-Developer: io.github.alice' in out and 'App-Developer-Name: Alice' in out and 'App-Source: https://github.com/gitrustux/foxden-apps/tree/main/apps/io.github.alice.dice' in out, 'rpg show: its developer and source')
check(re.search(r'%s +1\.0\.0 +Alice Dice' % re.escape(X), out) is not None, 'it installs (rpg list --apps)')
out = sh('rpg list --apps --available | grep -A0 "App-Developer"', 'S3B', 10)
check('App-Developer: io.github.alice' in out, 'FoxDen\'s list (rpg list --apps --available) has its developer')
n = len(log())
console('RUSTICA_WINDOW_AT=300,150 appctl run %s & sleep 1; echo S3C' % X, 6, r'(?<!echo )S3C\r?\n')
time.sleep(5)
type_('12+30*2\n')
check(wait(r'calculator: 12\+30\*2 = 72', 15, n) is not None, 'it runs in its sandbox (typed keys reach it: 12+30*2 = 72)')
key('esc')
time.sleep(1)

# 4. Another developer's version of the same ID
out = sh('rpg source remove 1; rpg source add apps file:///mnt/evil platform 1; rpg update; rpg upgrade -y; echo CODE-$?; rpg list --apps', 'S4-DONE', 60)
check('does not pass to another developer' in out, 'rpg upgrade refuses a version from another developer')
check(re.search(r'%s +1\.0\.0 +Alice Dice' % re.escape(X), out) is not None, 'it stays at 1.0.0')

# 5. The console
out = console('echo console works', 2)
check('\nconsole works' in out, 'the console works')

if fails:
    print('run-outside-test: %d checks failed' % fails)
    print(log()[-6000:])
    sys.exit(1)
PY
r=$?
kill $Q 2>/dev/null; wait $Q 2>/dev/null
[ $r -eq 0 ] && echo "run-outside-test: all checks passed" || echo "run-outside-test: some checks failed"
exit $r
