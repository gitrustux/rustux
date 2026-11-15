#!/bin/bash
# The virtio-gpu test (DESKTOP.md step F8, docs/DISPLAY.md). Boots Rustux
# with "desktop" on the kernel command line in QEMU three times:
#   1. virtio-vga (QEMU -vga virtio, as Proxmox's "VirtIO-GPU"), with a
#      VNC server and a USB tablet: the virtio-gpu driver serves the
#      screen instead of display; the desktop; the pointer is the host's
#      cursor (not in the screen's pixels); Ctrl+Alt+F1 shows the text
#      console and Ctrl+Alt+F7 the desktop again; the VNC client asks for
#      1600x900 and the desktop follows (the panel across the new width);
#   2. virtio-vga with two heads (the second one sized by a VNC client on
#      it): one screen, the second head showing its part; the panel stays
#      on the first;
#   3. virtio-gpu-pci, which leaves no UEFI framebuffer: the desktop still
#      comes up.
# GOP without virtio-gpu is every other graphical test's case.
# Exit status 0 if all is well.
#
#   scripts/run-virtio-gpu-test.sh [KERNEL.efi]
#
# Needs qemu-system-x86_64 (with virtio-vga), OVMF, mtools, python3.
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
NAME = sys.argv[2]
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
    px = lambda x, y: int.from_bytes(data[3 * (y * w + x):3 * (y * w + x) + 3], 'big')
    px.size = (w, h)
    return px

def near(c, want, tol=40):
    return all(abs(((c >> s) & 255) - ((want >> s) & 255)) <= tol for s in (0, 8, 16))

def key(k):
    mon('sendkey ' + k)

ACCENT, LOCKBOX, TOAST = 0xBD93F9, 0x343746, 0x44475A

def wait_pixel(x, y, want, t, tol=8):
    end = time.time() + t
    while time.time() < end:
        if near(screen()(x, y), want, tol):
            return True
        time.sleep(1)
    return False

import struct

def rfb_size(path, w, h):
    """A VNC client asks for a w x h desktop (RFB SetDesktopSize)"""
    s = connect(path)
    def rd(n):
        b = b''
        while len(b) < n:
            c = s.recv(n - len(b))
            if not c:
                raise EOFError
            b += c
        return b
    rd(12)
    s.sendall(b'RFB 003.008\n')
    rd(rd(1)[0])
    s.sendall(bytes([1]))
    rd(4)
    s.sendall(bytes([1]))
    fw, fh = struct.unpack('>HH', rd(4))
    rd(16)
    rd(struct.unpack('>I', rd(4))[0])
    encs = [0, -308]
    s.sendall(struct.pack('>BBH', 2, 0, len(encs)) + b''.join(struct.pack('>i', e) for e in encs))
    s.sendall(struct.pack('>BBHHHH', 3, 0, 0, 0, fw, fh))
    time.sleep(0.5)
    s.sendall(struct.pack('>BBHHBB', 251, 0, w, h, 1, 0) + struct.pack('>IHHHHI', 0, 0, 0, w, h, 0))
    time.sleep(1)
    s.close()

def qmp_dump(f, head):
    qmp.write(json.dumps({'execute': 'screendump', 'arguments': {'filename': f, 'device': 'gpu', 'head': head}}) + '\n')
    qmp.flush()
    qmp.readline()
    for _ in range(50):
        if os.path.exists(f) and os.path.getsize(f) > 0:
            break
        time.sleep(0.1)
    time.sleep(0.5)
    d = open(f, 'rb').read()
    hd = d.split(b'\n', 3)
    w, h = map(int, hd[1].split())
    return w, h, hd[3]

if not wait(r'rustux.*:.*/root.* > ', 120):
    print('no shell prompt')
    print(log()[-3000:])
    sys.exit(1)

if NAME == 'vga':
    check(wait(r'virtio-gpu: screen 1280x800 \(1 head\)', 30) is not None, 'virtio-gpu serves the screen (1280x800, 1 head)')
    check(wait_pixel(64, 782, ACCENT, 90), 'the desktop on virtio-gpu (the panel)')
    # The pointer is the host's cursor: not in the screen's pixels
    to(640, 400)
    time.sleep(1.5)
    px = screen()
    arrow = [px(640 + k, 400 + r) for r in range(2, 9) for k in range(1, 3)]
    check(not any(near(c, 0xFFFFFF, 30) for c in arrow), 'the pointer is a hardware cursor (none drawn into the screen)')
    # The text console and back
    key('ctrl-alt-f1')
    time.sleep(2)
    n = len(log())
    type_('echo console here\n')
    time.sleep(3)
    px = screen()
    bg = sum(1 for y in range(0, 800, 20) for x in range(0, 1280, 20) if near(px(x, y), 0x282A36, 6))
    check('console here' in log()[n:] and bg > 2000, 'Ctrl+Alt+F1: the text console on virtio-gpu (%d of 2560 samples its background)' % bg)
    check(any(not near(px(x, y), 0x282A36, 30) for y in range(0, 800, 2) for x in range(0, 300, 2)), 'the console shows text')
    key('ctrl-alt-f7')
    check(wait_pixel(64, 782, ACCENT, 10), 'Ctrl+Alt+F7: the desktop again')
    # The host window changes size
    n = len(log())
    rfb_size(W_DIR + '/vnc', 1600, 900)
    check(wait(r'virtio-gpu: screen 1600x900', 15, n) is not None, 'a VNC client asks for 1600x900: the driver changes the screen')
    time.sleep(3)
    px = screen()
    check(px.size == (1600, 900), 'the screen is 1600x900 (%dx%d)' % px.size)
    check(near(px(64, 882), ACCENT, 8) and near(px(1595, 882), 0x343746, 8), 'the desktop follows: the panel across 1600 pixels at the new bottom')

elif NAME == 'heads':
    check(wait(r'virtio-gpu: screen 1280x800 \(1 head\)', 30) is not None, 'two heads, the second off at first')
    check(wait_pixel(64, 782, ACCENT, 90), 'the desktop')
    n = len(log())
    rfb_size(W_DIR + '/vnc1', 800, 600)
    check(wait(r'virtio-gpu: screen 2080x800 \(2 heads\)', 15, n) is not None, 'the second head comes on: one screen of 2080x800')
    time.sleep(3)
    w0, h0, d0 = qmp_dump(W_DIR + '/h0.ppm', 0)
    w1, h1, d1 = qmp_dump(W_DIR + '/h1.ppm', 1)
    p0 = lambda x, y: int.from_bytes(d0[3 * (y * w0 + x):3 * (y * w0 + x) + 3], 'big')
    p1 = lambda x, y: int.from_bytes(d1[3 * (y * w1 + x):3 * (y * w1 + x) + 3], 'big')
    check((w0, h0, w1, h1) == (1280, 800, 800, 600), 'head 0 is 1280x800, head 1 800x600 (%dx%d, %dx%d)' % (w0, h0, w1, h1))
    check(near(p0(64, 782), ACCENT, 8) and near(p0(1275, 782), 0x343746, 8), 'the panel on the first head, its width')
    check(not near(p1(400, 300), 0, 8) and near(p1(400, 300), 0x262836, 24), 'the second head shows its part of the wallpaper (%06x)' % p1(400, 300))

else:  # virtio-gpu-pci
    check(wait(r'virtio-gpu: screen 1280x800 \(1 head\)', 30) is not None, 'virtio-gpu-pci (no UEFI framebuffer): the driver serves the screen')
    check(wait_pixel(64, 782, ACCENT, 90), 'the desktop on virtio-gpu-pci')

# Which display service runs: virtio-gpu, not display
key('ctrl-alt-f1')
time.sleep(1)
n = len(log())
type_('ps; logs | grep "the screen is"\n')
time.sleep(4)
key('ctrl-alt-f7')
out = log()[n:]
if NAME == 'vga':
    check(re.search(r'compositor: the screen is 1600x900', out) is not None, 'the compositor was told (the system log)')
check(re.search(r' virtio-gpu\s*\n', out) is not None and re.search(r' display\s*\n', out) is None, 'ps: virtio-gpu runs, /bin/display does not')

if fails:
    print('run-virtio-gpu-test (%s): %d checks failed' % (NAME, fails))
    print(log()[-5000:])
    sys.exit(1)
PY
    r=$?
    kill $Q 2>/dev/null; wait $Q 2>/dev/null
    return $r
}
ok=0
T="-device qemu-xhci,id=x -device usb-tablet,bus=x.0"
echo "virtio-vga:"
boot vga -vga none -device virtio-vga,id=gpu $T -vnc unix:"$W/vnc" || ok=1
echo "virtio-vga, two heads:"
boot heads -vga none -device virtio-vga,id=gpu,max_outputs=2 $T -vnc unix:"$W/vnc1",display=gpu,head=1 || ok=1
echo "virtio-gpu-pci:"
boot pci -vga none -device virtio-gpu-pci,id=gpu $T || ok=1
[ $ok = 0 ] || { echo "run-virtio-gpu-test: some checks failed"; exit 1; }
echo "run-virtio-gpu-test: all checks passed"
