#!/bin/bash
# The applications test (DESKTOP.md step F7, docs/TESTS.md). Boots
# Rustux in QEMU with "desktop" on the kernel command line, a USB tablet
# and a USB stick (a FAT image), and checks:
#   1. Terminal: the shell in a window (a command, a second tab, top),
#      tabs close when their shell ends and the window with the last;
#   2. Files: copy and paste (to the USB stick, mounted from the side
#      bar), a folder opened, rename, move to the trash, a new folder,
#      properties, a text file opened in the Text Editor (an app: appctl
#      open) and saved, the
#      stick ejected;
#   3. Settings (root): mouse speed, time zone, scale, a new user (an
#      administrator), the keyboard layout; the network (step D6): the
#      panel's Net shows the address DHCP gave, Settings > Network gives
#      eth0 a fixed address and a DNS server (the panel follows), then
#      DHCP again;
#   4. Settings as that user: a change of the system's asks for her
#      password (sudo -S);
#   5. the console works throughout.
# Exit status 0 if all is well.
#
#   scripts/run-apps-test.sh [KERNEL.efi]
#
# Needs qemu-system-x86_64, OVMF, mtools, python3.
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
# The USB stick: FAT, label STICK, a file on it
dd if=/dev/zero of="$W/stick.img" bs=1M count=32 status=none
mformat -i "$W/stick.img" -v STICK ::
echo "on the stick" > "$W/note.txt"
mcopy -i "$W/stick.img" "$W/note.txt" ::/NOTE.TXT
mcopy -i "$W/boot.img" "$W/cmdline.txt" ::/EFI/BOOT/cmdline.txt
boot() { # name, extra QEMU arguments
    rm -f "$W/mon" "$W/qmp" "$W/console.log"
    qemu-system-x86_64 -machine q35 -m 512M -smp 2 -bios "$OVMF" -drive file="$W/boot.img",format=raw \
        -display none -debugcon file:"$W/console.log" -no-reboot -monitor unix:"$W/mon",server,nowait \
        -qmp unix:"$W/qmp",server,nowait "${@:2}" >/dev/null 2>&1 &
    Q=$!
    python3 - "$W" "$1" <<'PY'
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

SW, SH = 1280, 800

def to(x, y):
    # The compositor maps 0..32767 onto 0..SW-1: the smallest value that lands on x
    qmp_events([{'type': 'abs', 'data': {'axis': 'x', 'value': math.ceil(x * 32767 / (SW - 1))}},
                {'type': 'abs', 'data': {'axis': 'y', 'value': math.ceil(y * 32767 / (SH - 1))}}])

def button(down):
    qmp_events([{'type': 'btn', 'data': {'down': down, 'button': 'left'}}])

def click(x, y, hold=0.0):
    to(x, y)
    button(True)
    time.sleep(hold)
    button(False)
    time.sleep(0.3)

def drag(x0, y0, x1, y1):
    to(x0, y0)
    button(True)
    for i in range(1, 5):
        to(x0 + (x1 - x0) * i // 4, y0 + (y1 - y0) * i // 4)
    button(False)
    time.sleep(0.5)

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
        time.sleep(0.2)
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

shot = [0]
def screen():
    shot[0] += 1
    f = '%s/s%d.ppm' % (W_DIR, shot[0])
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

def near(c, want, tol=40):
    return all(abs(((c >> s) & 255) - ((want >> s) & 255)) <= tol for s in (0, 8, 16))

def key(k):
    mon('sendkey ' + k)

ACCENT, LOCKBOX, TOAST = 0xFF6500, 0x202024, 0x202024  # the Rustica theme (ACCENT: the fox)

def wait_pixel(x, y, want, t, tol=8):
    end = time.time() + t
    while time.time() < end:
        if near(screen()(x, y), want, tol):
            return True
        time.sleep(1)
    return False

if not wait(r'rustux.*:.*/root.* > ', 120):
    print('no shell prompt')
    print(log()[-3000:])
    sys.exit(1)
check(wait_pixel(25, 787, ACCENT, 90), 'the desktop starts at boot')
time.sleep(3)

def console(cmd, t=3):
    """A command at the console (Ctrl+Alt+F1), then back to graphics; its output"""
    key('ctrl-alt-f1')
    time.sleep(1)
    n = len(log())
    type_(cmd + '\n')
    time.sleep(t)
    key('ctrl-alt-f7')
    time.sleep(1.5)
    return log()[n:]

def keys(*ks, pause=0.6):
    for k in ks:
        key(k)
        time.sleep(pause)

# 1. Terminal
console('terminal &', 8)
type_('echo term-$((40+2)) > /tmp/term.txt\n')
time.sleep(2)
px = screen()
check(near(px(300, 300), 0x101010, 4), 'Terminal: a window with the terminal background')
key('ctrl-shift-t')
time.sleep(5)
type_('echo tab2 > /tmp/tab2.txt; exit\n')
time.sleep(4)
type_('top\n')
time.sleep(5)
px = screen()
# top's header line is drawn in reverse video: light pixels in the second tab's first rows
check(any(near(px(x, y), 0xFFFFFF, 30) for x in range(60, 600, 7) for y in range(150, 190, 3)), 'Terminal: top runs (its reverse-video header)')
key('q')
time.sleep(2)
type_('exit\n')
time.sleep(4)
out = console('cat /tmp/term.txt /tmp/tab2.txt; ps | grep -c terminal', 4)
check('term-42' in out, 'Terminal: a command in the first tab (term-42)')
check('tab2' in out.split('cat /tmp/term.txt', 1)[-1], 'Terminal: a command in a second tab')
check(re.search(r'\n0\s*\n', out) is not None, 'Terminal: the window closes with its last tab')

# 2. Files
console('mkdir -p /root/ft/docs; echo hello > /root/ft/a.txt; RUSTICA_WINDOW_AT=40,64 files /root/ft &', 8)
keys('down', 'down', 'ctrl-c')
# The stick in the side bar (Home, Desktop, Documents, Downloads, Pictures,
# Computer, Trash, then it): mount it
# (the button held a while: Files reads the stick every 3 seconds, and a
# short click on a busy USB bus can come and go between two reports)
n = len(log())
click(100, 348, 0.5)
if not wait(r'files: folder /media/STICK', 10, n):
    click(100, 348, 0.5)
    wait(r'files: folder /media/STICK', 10, n)
time.sleep(1)
keys('ctrl-v')
time.sleep(2)
# Back home by the path field
click(500, 122)
keys('ctrl-a')
type_('/root/ft\n')
time.sleep(2)
keys('home', 'ret')                 # into docs
keys('ctrl-v', 'backspace')         # paste there, up again
time.sleep(1)
keys('end', 'f2', 'ctrl-a')
type_('b.txt\n')
time.sleep(2)
keys('delete')
time.sleep(1)
keys('ctrl-shift-n', 'ctrl-a')
type_('newdir\n')
time.sleep(2)
keys('alt-ret')
time.sleep(2)
px = screen()
check(near(px(470, 200), 0x202024, 6), 'Files: the properties dialog')
keys('ret')
# A text file: Enter opens it in the Text Editor; type, save
keys('home', 'ret')                 # docs
time.sleep(2)
keys('down', 'ret')                 # docs/a.txt
time.sleep(8)
keys('ctrl-end')
type_('more text')
keys('ctrl-s')
time.sleep(2)
keys('ctrl-q')
time.sleep(2)
# Eject the stick: to it, then the button under the side bar
click(100, 348, 0.5)
time.sleep(3)
click(85, 557, 0.5)
time.sleep(4)
keys('ret')
time.sleep(1)
keys('alt-f4')
time.sleep(2)
out = console('ls -R /root/ft; cat /root/ft/docs/a.txt; ls /root/.local/share/Trash/files; grep Path= /root/.local/share/Trash/info/b.txt.trashinfo; cat /etc/mtab', 4)
check(re.search(r'/root/ft:\s+docs\s+newdir', out) is not None, 'Files: a.txt renamed and moved to the trash, newdir made')
check(re.search(r'/root/ft/docs:\s+a\.txt', out) is not None, 'Files: a.txt pasted into docs')
check('hello\nmore text' in out or 'hellomore text' in out, 'Text Editor: the file opened from Files, changed and saved')
check('Path=/root/ft/b.txt' in out, 'Files: the trash knows where b.txt came from')
check('/media/STICK' not in out.split('cat /etc/mtab', 1)[-1], 'Files: the stick is ejected (not in /etc/mtab)')
lg = log()
for what in ['files: mount sda /media/STICK', 'files: copied /root/ft/a.txt -> /media/STICK/a.txt', 'files: rename /root/ft/a.txt -> /root/ft/b.txt',
             'files: trash /root/ft/b.txt', 'files: new folder /root/ft/newdir', 'files: open /root/ft/docs/a.txt with /bin/appctl open org.rustica.texteditor',
             'editor: saved /root/ft/docs/a.txt', 'files: eject sda']:
    check(what in lg, what)

# 3. Settings as root
def settings(section, ks):
    console('settings %s &' % section, 7)
    for k in ks:
        if k.startswith('T:'):
            type_(k[2:])
        else:
            key(k)
        time.sleep(0.6)
    time.sleep(3)
    key('alt-f4')
    time.sleep(2)

settings('mouse', ['tab'] * 5 + ['ret'])                                    # 150%
settings('time', ['tab'] * 13 + ['T:Berlin', 'tab', 'down', 'tab', 'ret'])   # past the clock's formats (10)
settings('display', ['tab', 'tab', 'ret'])                                  # Large
settings('users', ['tab', 'tab', 'T:carol', 'tab', 'T:secret', 'tab', 'spc', 'tab', 'ret'])
out = console('cat /etc/input.conf /etc/timezone /root/.config/rustica.conf; grep carol /etc/passwd /etc/group; echo root:rootpw | chpasswd', 4)
check('pointer_speed=150' in out, 'Settings: mouse speed 150% (/etc/input.conf)')
check('Europe/Berlin' in out, 'Settings: time zone Europe/Berlin (/etc/timezone)')
check('scale=2' in out, 'Settings: scale 2 (~/.config/rustica.conf)')
check(re.search(r'carol:x:1\d\d\d:', out) is not None and re.search(r'sudo:x:27:[^\n]*carol', out) is not None, 'Settings: a new user, carol, an administrator')

# The network (step D6): a fixed address and a DNS server of our own, then DHCP again
# (focus: the sections, the interfaces, Automatic, Fixed address, Off, the
# address, the gateway (the one DHCP gave), Apply, the DNS servers, Use these servers)
settings('network', ['tab', 'tab', 'tab', 'ret', 'tab', 'tab', 'ctrl-a', 'T:10.0.2.50/24', 'tab', 'tab', 'ret',
                     'tab', 'ctrl-a', 'T:9.9.9.9', 'tab', 'ret'])
out = console('cat /etc/network.conf /etc/resolv.conf; ip addr show eth0; ping -c 1 10.0.2.2', 6)
check('eth0 static 10.0.2.50/24 10.0.2.2' in out, 'Settings > Network: eth0 static 10.0.2.50/24 10.0.2.2 (/etc/network.conf)')
check('nameserver 9.9.9.9' in out and 'dns 9.9.9.9' in out, 'Settings > Network: DNS server 9.9.9.9 (network.conf, resolv.conf)')
check('inet 10.0.2.50/24' in out and 'inet 10.0.2.15' not in out, 'Settings > Network: eth0 has 10.0.2.50 now, not 10.0.2.15')
check(re.search(r'1 (packets )?received', out) is not None, 'Settings > Network: the gateway answers from the fixed address')
settings('network', ['tab', 'tab', 'ret', 'tab', 'tab', 'tab', 'ret', 'tab', 'ctrl-a', 'backspace', 'tab', 'ret'])
time.sleep(4)
out = console('cat /etc/network.conf /etc/resolv.conf; logs | grep -c "configured again"; logs | grep "panel: Net"', 5)
check(re.search(r'\n[2-9]\s', out.split('configured again', 1)[-1]) is not None, "netd applied the changes without a restart (its log)")
check(re.search(r'(?m)^eth0 dhcp', out) is not None and re.search(r'(?m)^(eth0 static|dns )', out) is None and 'nameserver 9.9.9.9' not in out,
      'Settings > Network: eth0 dhcp again, no DNS server of our own')
nets = re.findall(r'panel: Net: (\S+ \S+)', out)
check(nets[:1] == ['eth0 10.0.2.15'], "the panel's Net: eth0 and the address DHCP gave (%s)" % nets)
check('eth0 10.0.2.50' in nets and nets[-1:] == ['eth0 10.0.2.15'], "the panel's Net follows: the fixed address, then DHCP's again")
lg = log()
for what in ['settings: network eth0 static 10.0.2.50/24 10.0.2.2', 'settings: dns 9.9.9.9', 'settings: network eth0 dhcp', 'settings: dns']:
    check(what in lg, what)

# 4. Settings as carol: her password for the system's settings
click(1187, 782)                        # Log out
time.sleep(5)
type_('carol')
key('tab')
type_('secret')
key('ret')
check(wait_pixel(25, 787, ACCENT, 30), 'carol logs in')
time.sleep(3)
click(25, 782)                          # Apps: Settings is the 6th in the menu's All
time.sleep(4)
keys('down', 'down', 'down', 'down', 'down', 'ret')
time.sleep(8)
keys('down', 'down', 'down', 'down', 'down')  # Mouse (after Display, Appearance, Panel, Desktop, Keyboard)
keys(*(['tab'] * 6), 'ret')             # 200%: the password bar
time.sleep(2)
type_('secret\n')
time.sleep(5)
keys('alt-f4')
time.sleep(1)
out = console('cat /etc/input.conf', 3)
check('pointer_speed=200' in out, 'Settings as carol: with her password (sudo -S), mouse speed 200%')

# The keyboard layout last (the console types differently afterwards)
click(1187, 782)                        # Log out carol
time.sleep(5)
type_('root')
key('tab')
type_('rootpw')
key('ret')
time.sleep(3)
settings('keyboard', ['tab', 'home', 'tab', 'ret'])
out = console('localectl', 3)
check('VC Keymap: de' in out, 'Settings: keyboard layout de (localectl)')

# 5. The console
check(re.search(r'\nlocalectl\s', out) is not None or 'Keymap' in out, 'the console works')

if fails:
    print('run-apps-test: %d checks failed' % fails)
    print(log()[-6000:])
    sys.exit(1)
PY
    r=$?
    kill $Q 2>/dev/null; wait $Q 2>/dev/null
    return $r
}
boot desktop -nic user,model=virtio-net-pci -device qemu-xhci,id=x -device usb-tablet,bus=x.0 \
    -drive if=none,id=stick,file="$W/stick.img",format=raw -device usb-storage,bus=x.0,drive=stick ||
    { echo "run-apps-test: some checks failed"; exit 1; }
# What Files copied onto the stick
mdir -i "$W/stick.img" ::/ | grep -qi "^a  *txt" || { echo "  FAIL the copy on the stick (mdir)"; mdir -i "$W/stick.img" ::/; exit 1; }
echo "  ok   a.txt is on the stick (mdir)"
echo "run-apps-test: all checks passed"
