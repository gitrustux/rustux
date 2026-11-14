#!/bin/bash
# The input test (DESKTOP.md step F3, docs/TESTS.md). Boots Rustux in QEMU
# twice. With the PS/2 keyboard and mouse:
#   1. typing reaches the shell through the input service;
#   2. evtest takes the focus: keys (with the layout's text), mouse
#      motion, a button, the wheel; Esc gives the focus back;
#   3. fbtest's cursor follows the mouse and it reports a click;
#   4. the input service killed: init starts it again and typing works;
#   5. localectl set-keymap de: evtest sees z for the key labelled Y.
# With a USB keyboard, mouse and tablet (qemu-xhci):
#   6. the kernel finds all three; typing works; evtest sees relative
#      motion from the mouse and absolute positions from the tablet.
# Exit status 0 if all is well.
#
#   scripts/run-input-test.sh [KERNEL.efi]
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
boot() { # name, extra QEMU arguments
    rm -f "$W/mon" "$W/qmp" "$W/console.log"
    qemu-system-x86_64 -machine q35 -m 512M -smp 2 -bios "$OVMF" -drive file="$W/boot.img",format=raw \
        -display none -debugcon file:"$W/console.log" -no-reboot -monitor unix:"$W/mon",server,nowait \
        -qmp unix:"$W/qmp",server,nowait "${@:2}" >/dev/null 2>&1 &
    Q=$!
    python3 - "$W" "$1" <<'PY'
import json, os, re, socket, sys, time
W, PART = sys.argv[1], sys.argv[2]
LOG, MON = W + '/console.log', W + '/mon'
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

qmp = None
def qmp_send(events):
    global qmp
    if qmp is None:
        qmp = connect(W + '/qmp').makefile('rw')
        qmp.readline()
        qmp.write(json.dumps({'execute': 'qmp_capabilities'}) + '\n')
        qmp.flush()
        qmp.readline()
    qmp.write(json.dumps({'execute': 'input-send-event', 'arguments': {'events': events}}) + '\n')
    qmp.flush()
    qmp.readline()
    time.sleep(0.2)

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
         '|': 'shift-backslash', '&': 'shift-7', '$': 'shift-4', '(': 'shift-9', ')': 'shift-0'}

def type_(text):
    for ch in text:
        if ch.isupper():
            mon('sendkey shift-' + ch.lower())
        else:
            mon('sendkey ' + (ch if ch.isalnum() else NAMES[ch]))

def run(cmd, pat, t=30):
    n = len(log())
    type_(cmd + '\n')
    return wait(pat, t, n), n

if not wait(r'rustux.*:.*/root.* > ', 120):
    print('no shell prompt')
    print(log()[-3000:])
    sys.exit(1)

if PART == 'ps2':
    m, _ = run('echo typed through inputd', r'\ntyped through inputd')
    check(m is not None, 'typing reaches the shell through the input service')

    n = len(log())
    type_('evtest -t 20\n')
    check(wait(r'evtest: reading input', 10, n) is not None, 'evtest takes the focus')
    type_('aB')
    mon('mouse_move 5 -3')
    mon('mouse_button 1')
    mon('mouse_button 0')
    mon('mouse_move 0 0 1')
    mon('sendkey esc')
    out = (wait(r'evtest: done', 15, n) and log()[n:]) or log()[n:]
    check('key 0x04 down mods 0x00 text "a"' in out, 'evtest: a')
    check('key 0x05 down mods 0x02 text "B"' in out, 'evtest: Shift+b is B')
    check('pointer moved 5 -3 buttons 0x0' in out, 'evtest: the mouse moved 5, -3')
    check('buttons 0x1' in out, 'evtest: the left button')
    check(re.search(r'wheel 1\b', out) is not None, 'evtest: the wheel, one step away')
    check('evtest: done' in out, 'Esc ends evtest')
    m, _ = run('echo focus back', r'\nfocus back')
    check(m is not None, 'the console has the focus again')

    n = len(log())
    type_('fbtest -n 0\n')
    time.sleep(3)
    mon('mouse_move 5 -3')
    mon('mouse_button 1')
    mon('mouse_button 0')
    m = wait(r'fbtest: button 1 down at (\d+),(\d+)', 10, n)
    check(m is not None and (m.group(1), m.group(2)) == ('645', '397'), 'fbtest: a click where the cursor is: ' + (m.group(0) if m else 'none'))
    mon('sendkey q')
    check(wait(r'fbtest: \d+x\d+, \d+ frames', 10, n) is not None, 'q ends fbtest')

    m, n = run("kill $(ps | grep ' inputd' | cut -c1-6)", r'init: the input service ended \(code -1024\); starting it again', 20)
    check(m is not None, 'killing the input service: init starts it again')
    time.sleep(2)
    m, _ = run('echo typing again', r'\ntyping again')
    check(m is not None, 'typing works with the new input service')

    m, _ = run('localectl set-keymap de', r'rustux.*:.*/root.* > ', 15)
    time.sleep(1)
    n = len(log())
    type_('evtest\n')  # no '-': the de layout has another key for it
    wait(r'evtest: reading input', 10, n)
    mon('sendkey y')
    mon('sendkey esc')
    wait(r'evtest: done', 10, n)
    check('key 0x1c down mods 0x00 text "z"' in log()[n:], 'the de layout: the key labelled Y gives z')
else:
    l = log()
    check(re.search(r'keyboard [0-9A-F]{4}:[0-9A-F]{4} ready', l) is not None, 'USB keyboard found')
    check(re.search(r'mouse [0-9A-F]{4}:[0-9A-F]{4} ready', l) is not None, 'USB mouse found')
    check(re.search(r'tablet [0-9A-F]{4}:[0-9A-F]{4} ready', l) is not None, 'USB tablet found')
    m, _ = run('echo typed on USB', r'\ntyped on USB')
    check(m is not None, 'typing on the USB keyboard')
    n = len(log())
    type_('evtest -t 20\n')
    wait(r'evtest: reading input', 10, n)
    mon('mouse_move 7 4')
    qmp_send([{'type': 'abs', 'data': {'axis': 'x', 'value': 16384}}, {'type': 'abs', 'data': {'axis': 'y', 'value': 8192}}])
    mon('sendkey esc')
    wait(r'evtest: done', 15, n)
    out = log()[n:]
    check('pointer moved 7 4' in out, 'evtest: relative motion from the USB mouse')
    m = re.search(r'pointer at (\d+) (\d+)', out)
    check(m is not None and abs(int(m.group(1)) - 16384) <= 2 and abs(int(m.group(2)) - 8192) <= 2,
          'evtest: an absolute position from the USB tablet: ' + (m.group(0) if m else 'none'))

if fails:
    print('run-input-test (%s): %d checks failed' % (PART, fails))
    print(log()[-3000:])
    sys.exit(1)
PY
    r=$?
    kill $Q 2>/dev/null; wait $Q 2>/dev/null
    return $r
}
echo "PS/2 keyboard and mouse:"
boot ps2 || fail=1
echo "USB keyboard, mouse and tablet:"
boot usb -device qemu-xhci,id=x -device usb-kbd,bus=x.0 -device usb-mouse,bus=x.0 -device usb-tablet,bus=x.0 || fail=1
if [ -n "${fail:-}" ]; then echo "run-input-test: some checks failed"; exit 1; fi
echo "run-input-test: all checks passed"
