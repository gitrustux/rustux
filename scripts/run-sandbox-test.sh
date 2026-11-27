#!/bin/bash
# The sandbox test (FoxDen step FD2, docs/FOXDEN.md, docs/TESTS.md). Boots
# Rustux in QEMU with "desktop" on the kernel command line, a network
# (QEMU's user network: 10.0.2.2 is this computer, where a small server
# says "hello") and a second disk (FAT, APPDISK) with two apps built on
# the host around sandtest, which tries to leave its sandbox:
#   Aardvark (org.rustica.sandtest.plain) asks for no permissions,
#   Abacus (org.rustica.sandtest.full) for network and files:documents.
# Then:
#   1. at the console: both installed; appctl view and appctl permissions
#      show what each gets; narrowing Abacus in ~/.config/apps/permissions
#      takes its network away (and putting it back returns it);
#      sandtest started without appctl fails its checks (it gets out);
#   2. Applications starts each (appctl run ID): every check sandtest
#      makes passes (its own folder and data only, nothing of the home,
#      /etc/shadow, /bin, other apps, services such as /svc/auth, links
#      out of the view, the calls the job policy refuses; the network and
#      ~/Documents only for Abacus), and each one's window comes;
#   3. Abacus's file is in ~/Documents, nothing was planted in /apps;
#   4. the console works throughout.
# Exit status 0 if all is well.
#
#   scripts/run-sandbox-test.sh [KERNEL.efi]
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

# The apps: sandtest as their program, a manifest each
mkdir -p "$W/disk"
mkapp() {   # ID NAME PERMISSIONS EXEC-ARGS
    A=$W/app-$1
    mkdir -p "$A/RPG" "$A/bin"
    cp userspace/bin/sandtest "$A/bin/sandtest"; chmod 755 "$A/bin/sandtest"
    printf 'Package: %s\nVersion: 1.0\nArchitecture: x86_64\nKind: app\nMaintainer: The Rustux tests\nDescription: the sandbox test\n' "$1" > "$A/RPG/control"
    printf 'Id: %s\nName: %s\nSummary: tries to leave its sandbox\nExec: bin/sandtest %s\nCategories: Utility\n' "$1" "$2" "$4" > "$A/manifest"
    [ -n "$3" ] && printf 'Permissions: %s\n' "$3" >> "$A/manifest"
    printf 'Platform: 1\n' >> "$A/manifest"
    "$RPG" build "$A" -o "$W/disk/$1.rpg" > /dev/null || { echo "rpg build of $1 failed"; exit 2; }
}
mkapp org.rustica.sandtest.plain Aardvark "" "color=00a0ff at=200,200 hold=40"
mkapp org.rustica.sandtest.full Abacus "network files:documents" "net docs color=ff8000 at=700,200 hold=40"
truncate -s 16M "$W/app.fat"; mkfs.fat -n APPDISK "$W/app.fat" >/dev/null
mcopy -i "$W/app.fat" "$W/disk/"*.rpg ::

dd if=/dev/zero of="$W/boot.img" bs=1M count=128 status=none
mformat -i "$W/boot.img" -F ::
mmd -i "$W/boot.img" ::/EFI ::/EFI/BOOT
mcopy -i "$W/boot.img" "$EFI" ::/EFI/BOOT/BOOTX64.EFI
echo desktop > "$W/cmdline.txt"
mcopy -i "$W/boot.img" "$W/cmdline.txt" ::/EFI/BOOT/cmdline.txt

rm -f "$W/mon" "$W/qmp" "$W/console.log"
qemu-system-x86_64 -machine q35 -m 512M -smp 2 -bios "$OVMF" -drive file="$W/boot.img",format=raw \
    -drive file="$W/app.fat",if=none,id=r,format=raw -device virtio-blk-pci,drive=r \
    -device qemu-xhci,id=x -device usb-tablet,bus=x.0 -nic user,model=virtio-net-pci \
    -display none -debugcon file:"$W/console.log" -no-reboot -monitor unix:"$W/mon",server,nowait \
    -qmp unix:"$W/qmp",server,nowait >/dev/null 2>&1 &
Q=$!
python3 - "$W" <<'PY'
import json, math, os, re, socket, sys, time
W_DIR = sys.argv[1]
LOG, MON = W_DIR + '/console.log', W_DIR + '/mon'
fails = 0

# 10.0.2.2:18765 says hello (what sandtest's network check reaches)
import threading
srv = socket.socket()
srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
srv.bind(('0.0.0.0', 18765))
srv.listen(8)
reached = []
def hello():
    while True:
        c, a = srv.accept()
        reached.append(a)
        c.sendall(b'hello\n')
        c.close()
threading.Thread(target=hello, daemon=True).start()

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
P, F = 'org.rustica.sandtest.plain', 'org.rustica.sandtest.full'

# 1. Installing them; what each gets
out = console('mount LABEL=APPDISK /mnt && rpg install /mnt/%s.rpg; rpg install /mnt/%s.rpg; mkdir -p /root/Documents; echo a note > /root/Documents/note.txt; echo secret > /root/secret; appctl view %s; appctl permissions %s; echo VIEW-DONE' % (P, F, P, F), 30, r'VIEW-DONE\r?\n')
check('%s 1.0: installed' % P in out and '%s 1.0: installed' % F in out, 'rpg install installs both apps')
check('ro /apps/%s' % P in out and 'rw /root/.local/share/apps/%s' % P in out, 'appctl view: its folder read-only, its data read-write')
plain_view = out[out.find('ro /apps/%s' % P):out.find('asks for:')]
check('netsock' not in plain_view and 'Documents' not in plain_view and '/bin' not in plain_view.replace('/apps/%s/bin' % P, ''), 'appctl view: no network, no documents, no /bin for the app without permissions')
check(re.search(r'asks for: network files:documents\r?\ngets: network files:documents', out) is not None, 'appctl permissions: Abacus gets what it asks for')
out = console('mkdir -p /root/.config/apps; echo "%s: files:documents" > /root/.config/apps/permissions; appctl permissions %s; appctl view %s | grep -c netsock; rm /root/.config/apps/permissions; appctl view %s | grep -c netsock; echo NARROW-DONE' % (F, F, F, F), 10, r'NARROW-DONE\r?\n')
check(re.search(r'gets: files:documents\r?\n\(narrowed', out) is not None, 'the user narrows an app\'s permissions (~/.config/apps/permissions)')
check(re.search(r'\n0\r?\n1\r?\nNARROW-DONE', out) is not None, 'narrowed, its view has no network; without the line, it has')

# The same checks outside the sandbox must fail (else they prove nothing)
out = console('mkdir -p /tmp/ns; APP_ID=%s XDG_DATA_HOME=/tmp/ns sandtest | grep -E "FAIL|passed"; rm -rf /tmp/ns /apps/%s/planted /root/Documents/from-*; echo NS-DONE' % (P, P), 15, r'NS-DONE\r?\n')
m = re.search(r'passed (\d+) of (\d+)', out)
nf = len(re.findall(r'sandtest: \S+ FAIL', out))
check(m is not None and int(m.group(1)) < int(m.group(2)) and nf >= 10, 'outside the sandbox the same program gets out (%d checks fail)' % nf)
del reached[:]

# 2. Applications: Aardvark first, Abacus second
qmp_click(25, 782)
time.sleep(4)
key('ret'); time.sleep(4)
qmp_click(25, 782)
time.sleep(4)
key('down'); time.sleep(0.6)
key('ret'); time.sleep(8)
check(wait_pixel(320, 260, 0x00a0ff, 20), 'Aardvark\'s window comes')
check(wait_pixel(820, 260, 0xff8000, 20), 'Abacus\'s window comes')
out = console('logs | grep -E "launcher: started|appctl: |sandtest: "', 5)
check('launcher: started /bin/appctl run %s' % P in out and 'launcher: started /bin/appctl run %s' % F in out, 'the launcher starts apps with appctl run ID')
check('appctl: started %s (sandboxed)' % P in out, 'appctl starts it sandboxed')
for line in out.splitlines():
    if 'sandtest: ' in line and 'FAIL' in line:
        print('       ' + line.strip())
m1 = re.search(r'sandtest: %s passed (\d+) of (\d+)' % re.escape(P), out)
m2 = re.search(r'sandtest: %s passed (\d+) of (\d+)' % re.escape(F), out)
check(m1 is not None and m1.group(1) == m1.group(2), 'Aardvark (no permissions): every check passes (%s)' % (m1.group(0) if m1 else 'no result'))
check(m2 is not None and m2.group(1) == m2.group(2), 'Abacus (network, files:documents): every check passes (%s)' % (m2.group(0) if m2 else 'no result'))
check(len(reached) == 1, 'only Abacus reached 10.0.2.2 (%d connections)' % len(reached))

# 3. What they left
out = console('ls /root/Documents; ls /apps/%s; cat /root/.local/share/apps/%s/result | tail -1; echo LS-DONE' % (P, P), 6, r'LS-DONE\r?\n')
check('from-%s.txt' % F in out and 'from-%s' % P not in out, 'Abacus wrote to ~/Documents, Aardvark did not')
check('planted' not in out, 'nothing was planted in an app\'s folder')
check(re.search(r'%s passed (\d+) of \1' % re.escape(P), out) is not None, 'its result is in its data folder')

# 4. The console
out = console('echo console works', 2)
check('\nconsole works' in out, 'the console works')

if fails:
    print('run-sandbox-test: %d checks failed' % fails)
    print(log()[-6000:])
    sys.exit(1)
PY
r=$?
kill $Q 2>/dev/null; wait $Q 2>/dev/null
[ $r -eq 0 ] && echo "run-sandbox-test: all checks passed" || echo "run-sandbox-test: some checks failed"
exit $r
