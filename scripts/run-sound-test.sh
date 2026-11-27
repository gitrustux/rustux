#!/bin/bash
# The sound test (phase S, docs/SOUND.md, docs/TESTS.md): QEMU's Intel HD
# Audio controller with an output codec, whose sound QEMU writes to a WAV
# file that this script then listens to. Checks:
#   1. the hda driver finds the controller and the codec's output, soundd
#      takes the device (soundctl status);
#   2. a 1000 Hz tone at volume 100 is in the recording, at 1000 Hz;
#   3. the same tone at volume 50 is a quarter as loud (the square law);
#   4. muted, nothing is heard;
#   5. a 440 Hz tone made at 44100 Hz in one channel comes out at 440 Hz
#      (soundd's rate conversion), in both channels;
#   6. play: a WAV file the script makes (a 660 Hz tone at 22050 Hz);
#   7. the volume is kept across a restart of soundd.
# Exit status 0 if all is well.
#
#   scripts/run-sound-test.sh [KERNEL.efi]
#
# Needs qemu-system-x86_64 (with its wav audio backend), OVMF, mtools,
# python3.
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
Q=""

dd if=/dev/zero of="$W/boot.img" bs=1M count=128 status=none
mformat -i "$W/boot.img" -F ::
mmd -i "$W/boot.img" ::/EFI ::/EFI/BOOT
mcopy -i "$W/boot.img" "$EFI" ::/EFI/BOOT/BOOTX64.EFI
# A WAV file for play: 660 Hz, 22050 Hz, mono, 1.5 s
python3 - "$W/t660.wav" <<'PY'
import math, struct, sys
rate, n = 22050, int(22050 * 1.5)
data = b''.join(struct.pack('<h', int(12000 * math.sin(2 * math.pi * 660 * i / rate))) for i in range(n))
hdr = b'RIFF' + struct.pack('<I', 36 + len(data)) + b'WAVE' + b'fmt ' + struct.pack('<IHHIIHH', 16, 1, 1, rate, rate * 2, 2, 16)
open(sys.argv[1], 'wb').write(hdr + b'data' + struct.pack('<I', len(data)) + data)
PY
mcopy -i "$W/boot.img" "$W/t660.wav" ::/t660.wav

qemu-system-x86_64 -machine q35 -m 512M -smp 2 -bios "$OVMF" -drive file="$W/boot.img",format=raw,if=none,id=u \
    -device qemu-xhci -device usb-storage,drive=u \
    -audiodev wav,id=snd,path="$W/out.wav",out.frequency=48000 -device intel-hda -device hda-output,audiodev=snd \
    -display none -debugcon file:"$W/console.log" -no-reboot -monitor unix:"$W/mon",server,nowait >/dev/null 2>&1 &
Q=$!
python3 - "$W" <<'PY'
import re, socket, sys, time
W_DIR = sys.argv[1]
LOG, MON = W_DIR + '/console.log', W_DIR + '/mon'
fails = 0

def check(ok, what):
    global fails
    print(('  ok   ' if ok else '  FAIL ') + what, flush=True)
    if not ok:
        fails += 1

def mon(cmd):
    for _ in range(100):
        try:
            c = socket.socket(socket.AF_UNIX)
            c.connect(MON)
            break
        except OSError:
            time.sleep(0.1)
    c.recv(4096)
    c.sendall((cmd + '\n').encode())
    time.sleep(0.2)
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

NAMES = {' ': 'spc', '\n': 'ret', '-': 'minus', '/': 'slash', '.': 'dot', ';': 'semicolon', ':': 'shift-semicolon',
         '&': 'shift-7', '>': 'shift-dot', '<': 'shift-comma', '|': 'shift-backslash', '_': 'shift-minus', '=': 'equal',
         '$': 'shift-4', '(': 'shift-9', ')': 'shift-0', '+': 'shift-equal', ',': 'comma'}

def type_(text):
    for ch in text:
        mon('sendkey ' + ('shift-' + ch.lower() if ch.isupper() else ch if ch.isalnum() else NAMES[ch]))

marker = [0]
def run(cmd, t=60):
    """A command at the console; its output (up to an end marker)"""
    marker[0] += 1
    k = marker[0]
    n = len(log())
    type_(cmd + '; echo END-$((%d+1000))\n' % k)
    wait('END-%d' % (k + 1000), t, n)
    return log()[n:]

if not wait(r'rustux.*:.*/root.* > ', 150):
    print('no shell prompt')
    print(log()[-3000:])
    sys.exit(1)
time.sleep(3)

# 1. The driver and the service
out = run('dmesg | grep hda; soundctl status')
check(re.search(r'hda \S+: .*ready, 48000 Hz', out) is not None, 'hda found the controller and an output (%s)' % (re.findall(r'hda \S+: [^\n]*(?:line|speaker)[^\n]*', out)[:1],))
check(re.search(r'device QEMU.*\(in use\)', out) is not None, 'soundd has the device (soundctl status)')
check('volume 70' in out, 'the volume starts at 70')
ring = re.findall(r'hda \S+: the command ring[^\n]*', out)
check(not ring, 'the codec answers through the command ring (CORB/RIRB) %s' % ring)

# 2-5. Tones, with silence between them
run('soundctl volume 100 && soundctl tone 1000 2', 60)
time.sleep(3)
run('soundctl volume 50 && soundctl tone 1000 2', 60)
time.sleep(3)
out = run('soundctl mute && soundctl tone 1000 1.5; soundctl status | grep muted; soundctl unmute', 60)
check('muted yes' in out, 'mute: soundctl status says so')
time.sleep(3)
run('soundctl volume 100 && soundctl tone 440 2 44100 1', 60)
time.sleep(3)
# 6. play
out = run('mount -o ro sda /mnt; play /mnt/t660.wav', 60)
check('soundctl:' not in out and 'not found' not in out, 'play: no complaint (%s)' % out.strip().splitlines()[-3:])
time.sleep(3)
# 7. The volume kept across a restart of soundd
out = run('soundctl volume 35 >/dev/null; svc restart sound; sleep 2; soundctl status | grep volume')
check('volume 35' in out, 'the volume is kept when soundd restarts (%s)' % re.findall(r'volume \d+', out)[-1:])
type_('poweroff\n')
sys.exit(1 if fails else 0)
PY
r=$?
# QEMU ends with the power-off (the recording's header is written then)
for _ in $(seq 60); do kill -0 $Q 2>/dev/null || break; sleep 0.5; done
kill $Q 2>/dev/null
wait $Q 2>/dev/null
# Listen to the recording
python3 - "$W/out.wav" <<'PY'
import math, struct, sys
d = open(sys.argv[1], 'rb').read()
fails = 0
def check(ok, what):
    global fails
    print(('  ok   ' if ok else '  FAIL ') + what, flush=True)
    if not ok:
        fails += 1
i = d.find(b'fmt ')
ch, rate, bits = struct.unpack('<HI', d[i + 10:i + 16])[0], struct.unpack('<I', d[i + 12:i + 16])[0], struct.unpack('<H', d[i + 22:i + 24])[0]
j = d.find(b'data') + 8
pcm = d[j:]
n = len(pcm) // (2 * ch)
L = struct.unpack('<%dh' % (n * ch), pcm[:n * 2 * ch])
left, right = L[0::ch], L[1::ch] if ch > 1 else L[0::ch]
print('  recording: %.1f s, %d Hz, %d channels' % (n / rate, rate, ch))
win = rate // 20
segs = []
for k in range(0, n - win, win):
    a = left[k:k + win]
    b = right[k:k + win]
    rms = math.sqrt(sum(x * x for x in a) / win)
    rmsb = math.sqrt(sum(x * x for x in b) / win)
    zc = sum(1 for m in range(1, win) if (a[m - 1] < 0) != (a[m] < 0))
    freq = zc / 2 * rate / win
    segs.append((rms, rmsb, freq))
def windows(f0, lo, hi):
    return [s for s in segs if abs(s[2] - f0) < f0 * 0.05 and lo <= s[0] <= hi]
loud = windows(1000, 4000, 40000)
quiet = [s for s in segs if abs(s[2] - 1000) < 50 and 200 < s[0] < 4000]
a440 = windows(440, 1000, 40000)
a660 = windows(660, 1000, 40000)
print('  windows of 50 ms: 1000 Hz loud %d, quiet %d; 440 Hz %d; 660 Hz %d' % (len(loud), len(quiet), len(a440), len(a660)))
check(30 <= len(loud) <= 45, 'the 1000 Hz tone at volume 100: about 2 s of it')
check(30 <= len(quiet) <= 45, 'the 1000 Hz tone at volume 50: about 2 s of it')
if loud and quiet:
    rl = sorted(s[0] for s in loud)[len(loud) // 2]
    rq = sorted(s[0] for s in quiet)[len(quiet) // 2]
    check(0.2 < rq / rl < 0.3, 'volume 50 is a quarter as loud as 100 (%.3f)' % (rq / rl))
check(len([s for s in segs if abs(s[2] - 1000) < 50 and s[0] > 200]) <= 45 + 45 + 2, 'muted: the third 1000 Hz tone is not heard')
check(30 <= len(a440) <= 45, 'the 440 Hz tone made at 44100 Hz, mono: 440 Hz, about 2 s')
if a440:
    check(all(abs(s[0] - s[1]) < s[0] * 0.1 for s in a440), 'mono comes out in both channels')
check(22 <= len(a660) <= 34, 'play: the WAV file (660 Hz at 22050 Hz, 1.5 s)')
sys.exit(1 if fails else 0)
PY
r2=$?
[ $r = 0 ] && [ $r2 = 0 ] && { echo "run-sound-test: all checks passed"; exit 0; }
echo "run-sound-test: some checks failed"
[ $r = 0 ] || tail -c 3000 "$W/console.log"
exit 1
