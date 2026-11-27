#!/bin/bash
# The installer service test (FoxDen step FD4, docs/FOXDEN.md,
# docs/TESTS.md). A second disk (FAT, SVCREPO) holds an apps source (the
# Calculator, the System Monitor, and org.test.homeapp asking for
# files:home; then a Calculator 1.1.0) and a system source with a system
# package, hello, made on the host and signed with test keys. Boots
# Rustux in QEMU (the console). Users: carol (not an administrator) and
# erin (group sudo). rpg run by them asks the installer service:
#   1. carol installs the Calculator: no password;
#   2. carol is refused hello (a system package), org.test.homeapp
#      (files:home) and a package file without an administrator's password,
#      and with erin's wrong one; with erin's name and password she gets
#      org.test.homeapp; erin installs hello with her own password;
#   3. carol: rpg update, and rpg upgrade brings the Calculator 1.1.0 (an
#      app: no password);
#   4. carol: rpg remove --purge is refused, rpg remove works;
#   5. the log says who asked and who allowed it.
# Exit status 0 if all is well.
#
#   scripts/run-installsvc-test.sh [KERNEL.efi]
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
put() { rel=$("$RPG" pool-path "$2"); mkdir -p "$1/$(dirname "$rel")"; cp "$2" "$1/$rel"; }
sign() { openssl pkeyutl -sign -inkey "$3" -rawin -in "$1/$2/Release" -out "$1/$2/Release.sig"; }
scripts/build-apps.sh "$W/apps1" org.rustica.calculator org.rustica.sysmon > /dev/null || { echo "the apps did not build"; exit 2; }
# org.test.homeapp: the Calculator's program, asking for files:home
cp -r userspace/apps "$W/src"
cp -r "$W/src/org.rustica.calculator" "$W/src/org.test.homeapp"
sed -i 's/org\.rustica\.calculator/org.test.homeapp/; s/^Name: .*/Name: Home App/' "$W/src/org.test.homeapp/RPG/control" "$W/src/org.test.homeapp/manifest"
printf 'Permissions: files:home\n' >> "$W/src/org.test.homeapp/manifest"
APPS_SRC=$W/src scripts/build-apps.sh "$W/apps1" org.test.homeapp > /dev/null || { echo "homeapp did not build"; exit 2; }
for f in "$W"/apps1/*.rpg; do put "$D/good" "$f"; done
"$RPG" index --apps "$D/good" > /dev/null && sign "$D/good" platform/1 "$W/apps.key"
# good2: the Calculator 1.1.0 and the others as they were
sed -i 's/^Version: .*/Version: 1.1.0/' "$W/src/org.rustica.calculator/RPG/control"
APPS_SRC=$W/src scripts/build-apps.sh "$W/apps2" org.rustica.calculator > /dev/null
for f in "$W"/apps2/*.rpg "$W"/apps1/org.rustica.sysmon_*.rpg "$W"/apps1/org.test.homeapp_*.rpg; do put "$D/good2" "$f"; done
"$RPG" index --apps "$D/good2" > /dev/null && sign "$D/good2" platform/1 "$W/apps.key"
# A system source with hello, a system package
mkdir -p "$W/hello/RPG" "$W/hello/opt/hello"
printf 'Package: hello\nVersion: 1.0\nArchitecture: all\nDescription: a test system package\n' > "$W/hello/RPG/control"
echo "hello from a system package" > "$W/hello/opt/hello/README"
"$RPG" build "$W/hello" -o "$W/hello_1.0_all.rpg" > /dev/null
mkdir -p "$D/sys/pool/main/h/hello" && cp "$W/hello_1.0_all.rpg" "$D/sys/pool/main/h/hello/"
"$RPG" index "$D/sys" > /dev/null && sign "$D/sys" dists/redfox "$W/archive.key"
# A package file (an app): only with an administrator
cp "$W"/apps1/org.rustica.sysmon_*.rpg "$D/sysmon.rpg"
truncate -s 32M "$W/repo.fat"; mkfs.fat -n SVCREPO "$W/repo.fat" >/dev/null
mcopy -s -i "$W/repo.fat" "$D"/* ::

dd if=/dev/zero of="$W/boot.img" bs=1M count=128 status=none
mformat -i "$W/boot.img" -F ::
mmd -i "$W/boot.img" ::/EFI ::/EFI/BOOT
mcopy -i "$W/boot.img" "$EFI" ::/EFI/BOOT/BOOTX64.EFI
echo "" > "$W/cmdline.txt"
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
if not wait(r'rustux.*:.*/root.* > ', 120):
    print('no shell prompt')
    print(log()[-3000:])
    sys.exit(1)
time.sleep(2)

def sh(cmd, mark, t=90):
    """A command at the console; its output, up to the mark it prints (not
    the mark in the echoed command line)"""
    n = len(log())
    type_(cmd + '; echo ' + mark + '\n')
    if not wait(r'(?<!echo )' + mark + r'\r?\n', t, n):
        print('  (no %s after %ds)' % (mark, t))
    time.sleep(0.5)
    return log()[n:]

def apps_line(out):
    m = re.search(r'(?<!echo )APPS:([^\r\n]*)', out)
    return m.group(1).split() if m else None

NAMES['\\'] = 'backslash'
NAMES['^'] = 'shift-6'
C, H = 'org.rustica.calculator', 'org.test.homeapp'
V = re.search(r'^Version: *(\S+)', open('userspace/apps/org.rustica.calculator/RPG/control').read(), re.M).group(1)   # the Calculator's version in userspace/apps
out = sh('mount LABEL=SVCREPO /mnt && rpg key add --apps /mnt/apps.pem apps-test && rpg key add /mnt/archive.pem archive-test; rpg source remove 1; rpg source remove 1; rpg source add apps file:///mnt/good platform 1; rpg source add file:///mnt/sys redfox main; useradd -m carol; echo carol:pwc | chpasswd; useradd -m -G sudo erin; echo erin:pwe | chpasswd; svc restart packagesd; sleep 2; rpg update', 'SETUP-DONE', 150)
check('file:///mnt/good platform 1: 3 apps' in out and 'file:///mnt/sys redfox: 1 packages' in out, 'the sources are read (3 apps, 1 system package)')

# 1. carol, an app from the apps source: no password
out = sh("su carol -c 'rpg install %s'; echo APPS: $(ls /apps)" % C, 'S1-DONE')
check('%s %s: installed' % (C, V) in out and 'password' not in out, 'carol installs the Calculator, no password asked')

# 2. what needs an administrator
out = sh("echo | su carol -c 'rpg install hello'; rpg list --installed | grep -c '^hello '", 'S2A-DONE')
check("hello is a system package: an administrator's password is needed" in out and 'rpg: no administrator named' in out and re.search(r'\n0\r?\nS2A-DONE', out) is not None, 'carol is refused hello (a system package) without a password')
out = sh("echo | su carol -c 'rpg install %s'; echo APPS: $(ls /apps)" % H, 'S2B-DONE')
check("%s asks for files:home (the whole home folder): an administrator's password is needed" % H in out and H not in (apps_line(out) or [H]), 'carol is refused an app with files:home')
out = sh("echo | su carol -c 'rpg install /mnt/sysmon.rpg'; echo APPS: $(ls /apps)", 'S2C-DONE')
check("installing a package file (not from FoxDen): an administrator's password is needed" in out and 'org.rustica.sysmon' not in (apps_line(out) or ['org.rustica.sysmon']), 'carol is refused a package file')
out = sh("printf 'erin\\nwrong\\n' | su carol -c 'rpg install hello'; rpg list --installed | grep -c '^hello '", 'S2D-DONE')
check('wrong password for erin' in out and re.search(r'\n0\r?\nS2D-DONE', out) is not None, "erin's wrong password is refused")
out = sh("printf 'dave\\nx\\n' | su carol -c 'rpg install hello'", 'S2E-DONE')
check('dave is not an administrator' in out, 'someone who is not an administrator cannot say yes')
out = sh("printf 'erin\\npwe\\n' | su carol -c 'rpg install %s'; echo APPS: $(ls /apps)" % H, 'S2F-DONE')
check('%s %s: installed' % (H, V) in out and 'asks for: files:home' in out, "with erin's name and password carol gets the files:home app")
out = sh("echo pwe | su erin -c 'rpg install hello'; cat /opt/hello/README", 'S2G-DONE')
check('hello 1.0: installed' in out and 'hello from a system package' in out, 'erin installs hello with her own password')

# 3. update and upgrade, an app only: no password
# (apps ask: the check upgrades no app by itself, FoxDen step FD6, so
# carol's own rpg upgrade does)
out = sh("echo 'apps ask' >> /etc/rpg/updates.conf; rpg source remove 1; rpg source add apps file:///mnt/good2 platform 1; su carol -c 'rpg update'; su carol -c 'rpg upgrade'; rpg list --apps", 'S3-DONE')
check('file:///mnt/good2 platform 1: 3 apps' in out, 'carol: rpg update (through the service)')
check('%s %s -> 1.1.0' % (C, V) in out and re.search(r'org\.rustica\.calculator +1\.1\.0', out) is not None and 'password' not in out, 'carol: rpg upgrade brings the Calculator 1.1.0, no password')

# 4. removing
out = sh("echo | su carol -c 'rpg remove --purge %s'; su carol -c 'rpg remove %s'; echo APPS: $(ls /apps)" % (C, C), 'S4-DONE')
check("--purge removes every user's data for it: an administrator's password is needed" in out, 'carol: rpg remove --purge needs an administrator')
check('%s 1.1.0: removed' % C in out and C not in (apps_line(out) or [C]), 'carol: rpg remove removes the Calculator')

# 5. the log
out = sh('logs -t packagesd | grep -E "asked by|refused"', 'S5-DONE')
check('install asked by carol' in out and 'install asked by carol, allowed by erin' in out and 'install asked by erin' in out, 'the log says who asked and who allowed it')
check('remove asked by carol' in out and 'upgrade asked by carol' in out, 'and the upgrade and removal')

if fails:
    print('run-installsvc-test: %d checks failed' % fails)
    print(log()[-8000:])
    sys.exit(1)
PY
r=$?
kill $Q 2>/dev/null; wait $Q 2>/dev/null
[ $r -eq 0 ] && echo "run-installsvc-test: all checks passed" || echo "run-installsvc-test: some checks failed"
exit $r
