#!/bin/bash
# The updates test (package step E6, docs/PACKAGES.md, docs/TESTS.md).
# Boots Rustux in QEMU with "desktop" on the kernel command line and a
# second disk (FAT, RPGREPO): a package hello-up 1.0 and a repository with
# hello-up 1.1 (which needs a new package, hello-dep), signed with a test
# key, all made with the host's rpg. Then:
#   1. at the console: hello-up 1.0 installed, the test repository as the
#      only source, the installer service's daily check made to come at
#      once (/etc/rpg/updates.conf: delay 1); its check finds the two
#      packages (updates, /var/lib/rpg/updates);
#   2. the panel shows "Updates (2)" and a notification says so;
#   3. the installer service refuses a user who is not an administrator,
#      and an administrator with a wrong password (updates install -S);
#   4. Settings > Updates (root) lists them and installs them: hello-up
#      1.1 and hello-dep 1.0 are installed, updates says everything is up
#      to date, the panel's button goes;
#   5. the console works throughout.
# Exit status 0 if all is well.
#
#   scripts/run-updates-test.sh [KERNEL.efi]
#
# Needs qemu-system-x86_64, OVMF, mtools, dosfstools, python3, openssl, cargo.
set -u
cd "$(dirname "$0")/.."
EFI=${1:-target/x86_64-unknown-uefi/release/rustux.efi}
[ -f "$EFI" ] || { echo "no $EFI (build it first)"; exit 2; }
OVMF=""
for p in /usr/share/ovmf/OVMF.fd /usr/share/qemu/OVMF.fd /usr/share/edk2-ovmf/x64/OVMF_CODE.fd /usr/share/OVMF/OVMF_CODE.fd; do
    [ -f "$p" ] && OVMF=$p && break
done
[ -n "$OVMF" ] || { echo "OVMF firmware not found"; exit 2; }
for t in qemu-system-x86_64 mformat mmd mcopy mkfs.fat python3 openssl cargo; do
    command -v $t >/dev/null || { echo "$t is needed"; exit 2; }
done
W=$(mktemp -d)
trap 'kill $Q 2>/dev/null; rm -rf "$W"' EXIT
HOSTT=$(rustc -vV | sed -n 's/^host: //p')
(cd userspace/rust/rpg && cargo build -q --release --target "$HOSTT" --target-dir "$W/cargo") || { echo "rpg did not build for this computer"; exit 2; }
RPG="$W/cargo/$HOSTT/release/rpg"

# The packages
mkpkg() { # name version depends
    local d="$W/src/$1-$2"
    mkdir -p "$d/RPG" "$d/opt/$1"
    echo "$2" > "$d/opt/$1/version"
    { echo "Package: $1"; echo "Version: $2"; echo "Architecture: all"; echo "Maintainer: The Rustux tests";
      [ -n "$3" ] && echo "Depends: $3"; echo "Description: a package of the updates test"; } > "$d/RPG/control"
    "$RPG" build "$d" -o "$4" > /dev/null || { echo "rpg build $1 failed"; exit 2; }
}
mkdir -p "$W/disk/repo"
mkpkg hello-up 1.0 "" "$W/disk/hello-up_1.0_all.rpg"
mkpkg hello-up 1.1 "hello-dep (>= 1.0)" "$W/new1.rpg"
mkpkg hello-dep 1.0 "" "$W/new2.rpg"
for f in "$W/new1.rpg" "$W/new2.rpg"; do
    p="$W/disk/repo/$("$RPG" pool-path "$f")"; mkdir -p "$(dirname "$p")"; mv "$f" "$p"
done
"$RPG" index "$W/disk/repo" > /dev/null || { echo "rpg index failed"; exit 2; }
openssl genpkey -algorithm ed25519 -out "$W/test.key" 2>/dev/null
openssl pkey -in "$W/test.key" -pubout -out "$W/disk/test.pub.pem"
openssl pkeyutl -sign -inkey "$W/test.key" -rawin -in "$W/disk/repo/dists/redfox/Release" -out "$W/disk/repo/dists/redfox/Release.sig"
cat > "$W/disk/setup.sh" <<'EOF'
rpg install /mnt/hello-up_1.0_all.rpg
rpg key add /mnt/test.pub.pem rpg-test
rpg source remove 1
rpg source remove 1
rpg source add file:///mnt/repo redfox main
printf 'check daily\ndelay 1\n' > /etc/rpg/updates.conf
useradd -m dave; echo dave:pwd | chpasswd
useradd -m -G sudo erin; echo erin:pwe | chpasswd
svc restart packagesd
# its check: soon after the start (delay 1), and the sources changed
for i in 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20; do
    grep -q "^step " /var/lib/rpg/updates 2>/dev/null && break
    sleep 2
done
updates
echo SETUP-DONE
EOF
truncate -s 16M "$W/repo.fat"; mkfs.fat -n RPGREPO "$W/repo.fat" >/dev/null
mcopy -i "$W/repo.fat" -s "$W"/disk/* ::

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

# 1. The setup, and the installer service's own check
n0 = len(log())
out = console('mount LABEL=RPGREPO /mnt && sh /mnt/setup.sh', 90, r'SETUP-DONE')
check('hello-up 1.0: installed' in out, 'hello-up 1.0 installed')
check(re.search(r'hello-up +1\.0 -> 1\.1', out) is not None and re.search(r'hello-dep +new 1\.0 .*for hello-up', out) is not None,
      'the installer service\'s daily check found hello-up 1.0 -> 1.1 and the new hello-dep')
check('2 packages to upgrade' in out, 'updates: 2 packages to upgrade')

# 2. The panel
time.sleep(35)   # the panel looks every 30 seconds
out = console('logs | grep -E "notification|packagesd|panel: updates"', 4)
check('panel: updates 2' in out, 'the panel shows "Updates (2)"')
check(re.search(r'compositor: notification "2 updates for the system: Settings > Updates"', out) is not None, 'a notification says so')
check(re.search(r'packagesd.*checked for updates: 2 packages to upgrade', out) is not None, 'packagesd logged its check')

# 3. Who may install
console('su dave -c "echo pwd | updates install -S"', 6)
console('su erin -c "echo wrong | updates install -S"', 8)
lg = log()[n0:]
check('updates: refused: dave is not an administrator (group sudo)' in lg, 'a user who is not an administrator is refused')
check('updates: refused: wrong password for erin' in lg, 'an administrator with a wrong password is refused')

# 4. Settings > Updates installs them (root: no password)
n = len(log())
console('settings updates &', 7)
check(wait(r'settings: section Updates|settings: started', 10, n) is not None, 'Settings opens at Updates')
for k in ['tab', 'tab', 'tab', 'ret']:   # the table, Check now, Install updates
    key(k); time.sleep(0.6)
check(wait(r'settings: updates installed', 120, n) is not None, 'Settings > Updates installed them')
time.sleep(2)
key('alt-f4'); time.sleep(2)
out = console('rpg list --installed | grep hello; cat /opt/hello-up/version; updates', 5)
check(re.search(r'hello-up +1\.1 ', out) is not None and re.search(r'hello-dep +1\.0 ', out) is not None, 'hello-up 1.1 and hello-dep 1.0 are installed')
check(re.search(r'\n1\.1\r?\n', out) is not None, '/opt/hello-up/version says 1.1')
check('Everything is up to date.' in out, 'updates: everything is up to date')
time.sleep(35)
out = console('logs | grep "panel: updates"', 4)
check('panel: updates 0' in out, 'the panel\'s button goes')

# 5. The console
out = console('echo console works', 2)
check('\nconsole works' in out, 'the console works')

if fails:
    print('run-updates-test: %d checks failed' % fails)
    print(log()[-6000:])
    sys.exit(1)
PY
r=$?
kill $Q 2>/dev/null; wait $Q 2>/dev/null
[ $r -eq 0 ] && echo "run-updates-test: all checks passed" || echo "run-updates-test: some checks failed"
exit $r
