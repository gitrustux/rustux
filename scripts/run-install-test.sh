#!/bin/bash
# The installer test (step 20, docs/INSTALL.md). QEMU with OVMF's code and
# a variable store of its own, a live medium as build-live-image.sh makes
# it (rboot; "Try Rustica Server" and "Install Rustica Server"), and:
#
#   A. an empty 2 GiB NVMe disk (the laptop's kind: write-locked until the
#      installer's confirmed choice): the menu's Install entry, the
#      installer answered on the console (erase), restart; the disk alone
#      boots through rboot to a login prompt; the user logs in, writes a
#      file, sudo poweroff; the next boot still has it; e2fsck -f clean;
#   B. the same disk again from the live session: replace the earlier
#      Rustica (rustica-install --mode replace): the new user logs in, the
#      old one is gone, one firmware entry "Rustica";
#   C. a disk with "Windows" (an EFI partition with a stand-in boot loader,
#      rboot/src/bin/fakeos.rs, as bootmgfw.efi and as the firmware's
#      fallback BOOTX64.EFI, and a data partition) and free space: a dry
#      run writes nothing; installed alongside; rboot's menu has Windows,
#      which starts; Rustica starts by default; the Windows partitions
#      and files are untouched;
#   D. that disk as it was, with the power cut during the installation:
#      the computer still starts Windows (the firmware's order changes
#      last).
#
#   scripts/run-install-test.sh [KERNEL.efi]
#
# Needs qemu-system-x86_64 (with nvme), OVMF code and variables, mtools,
# dosfstools, e2fsprogs, socat, python3, and rboot built.
set -u
cd "$(dirname "$0")/.."
EFI=${1:-target/x86_64-unknown-uefi/release/rustux.efi}
RB=rboot/target/x86_64-unknown-uefi/release
[ -f "$EFI" ] || { echo "no $EFI (build it first)"; exit 2; }
[ -f $RB/rboot.efi ] && [ -f $RB/fakeos.efi ] || { echo "no $RB/rboot.efi or fakeos.efi (build.sh builds them)"; exit 2; }
CODE="" VARS=""
for d in /usr/share/OVMF /usr/share/edk2/ovmf /usr/share/edk2-ovmf/x64 /usr/share/qemu; do
    for s in _4M ""; do
        if [ -z "$CODE" ] && [ -f "$d/OVMF_CODE$s.fd" ] && [ -f "$d/OVMF_VARS$s.fd" ]; then CODE=$d/OVMF_CODE$s.fd; VARS=$d/OVMF_VARS$s.fd; fi
    done
done
[ -n "$CODE" ] || { echo "OVMF_CODE.fd and OVMF_VARS.fd not found"; exit 2; }
export PATH=$PATH:/sbin:/usr/sbin
for t in qemu-system-x86_64 mkfs.fat mcopy mdir e2fsck debugfs socat python3 sha256sum; do
    command -v $t >/dev/null || { echo "$t is needed"; exit 2; }
done

W=$(mktemp -d)
QPID=""
cleanup() { [ -n "$QPID" ] && kill $QPID 2>/dev/null; rm -rf "$W"; }
trap cleanup EXIT
fail=0

# ---- the media ----
mkdir -p "$W/live/EFI/BOOT" "$W/live/EFI/Rustica" "$W/live/loader/entries"
cp $RB/rboot.efi "$W/live/EFI/BOOT/BOOTX64.EFI"
cp $RB/rboot.efi "$W/live/EFI/Rustica/rboot.efi"
cp "$EFI" "$W/live/EFI/Rustica/rustux.efi"
printf 'default rustica\ntimeout 2\n' > "$W/live/loader/loader.conf"
printf 'title Try Rustica Server\nefi /EFI/Rustica/rustux.efi\nsort-key 1\n' > "$W/live/loader/entries/rustica.conf"
printf 'title Install Rustica Server\nefi /EFI/Rustica/rustux.efi\noptions installer\nsort-key 2\n' > "$W/live/loader/entries/install.conf"
truncate -s 128M "$W/live.fat"; mkfs.fat -F 32 -n RUSTICA "$W/live.fat" >/dev/null; mcopy -i "$W/live.fat" -s "$W/live/EFI" "$W/live/loader" ::
python3 scripts/gpt-wrap.py --disk 9 "$W/live.img" esp:"$W/live.fat"
truncate -s 2G "$W/nvme.img"
# "Windows": its EFI partition (bootmgfw.efi and the fallback), a data partition, 1.6 GiB free
mkdir -p "$W/win/EFI/Microsoft/Boot" "$W/win/EFI/BOOT"
cp $RB/fakeos.efi "$W/win/EFI/Microsoft/Boot/bootmgfw.efi"
cp $RB/fakeos.efi "$W/win/EFI/BOOT/BOOTX64.EFI"
truncate -s 100M "$W/win.fat"; mkfs.fat -F 32 -n SYSTEM "$W/win.fat" >/dev/null; mcopy -i "$W/win.fat" -s "$W/win/EFI" ::
head -c 64M /dev/urandom > "$W/windata.bin"
python3 scripts/gpt-wrap.py --disk 5 "$W/dual.img" esp:"$W/win.fat" msdata:"$W/windata.bin" free:1650
cp "$W/dual.img" "$W/dual-cut.img"

# The live medium first, whatever the firmware's boot order says (as choosing
# the stick in the firmware's boot menu). OVMF then also drops the boot
# entries QEMU did not name (a real computer keeps them): the installed
# system's entry is made again by each installation
LIVE="-drive file=$W/live.img,if=none,id=l,format=raw -device virtio-blk-pci,drive=l,bootindex=0"
NVME="-drive file=$W/nvme.img,if=none,id=n,format=raw -device nvme,serial=rustica01,drive=n"
DUAL="-drive file=$W/dual.img,if=none,id=d,format=raw -device virtio-blk-pci,drive=d"
CUT="-drive file=$W/dual-cut.img,if=none,id=d,format=raw -device virtio-blk-pci,drive=d"

log() { sed 's/\x1b\[[0-9;]*[a-zA-Z]//g' "$W/dbg.log" 2>/dev/null; }
start_qemu() {  # VARS-FILE DEVICES...
    local vars=$1; shift
    [ -f "$vars" ] || cp "$VARS" "$vars"
    rm -f "$W/dbg.log" "$W/mon.sock"
    qemu-system-x86_64 -machine q35 -m 1G -smp 2 \
        -drive if=pflash,format=raw,readonly=on,file="$CODE" -drive if=pflash,format=raw,file="$vars" \
        $* -nic none -display none -debugcon file:"$W/dbg.log" -no-reboot -monitor unix:"$W/mon.sock",server,nowait >/dev/null 2>&1 &
    QPID=$!
}
wait_for() {  # TEXT COUNT SECONDS
    for _ in $(seq $(($3 * 2))); do
        [ "$(log | grep -ac -- "$1")" -ge "$2" ] && return 0
        kill -0 $QPID 2>/dev/null || break
        sleep 0.5
    done
    [ "$(log | grep -ac -- "$1")" -ge "$2" ] && return 0
    echo "FAIL: no \"$1\" (x$2) within $3 s"; fail=1; return 1
}
keymap() {
    python3 - "$1" <<'PY'
import sys
m={' ':'spc','\n':'ret','-':'minus','=':'equal','[':'bracket_left',']':'bracket_right',';':'semicolon',"'":'apostrophe','`':'grave_accent','\\':'backslash',',':'comma','.':'dot','/':'slash'}
sh={'!':'1','@':'2','#':'3','$':'4','%':'5','^':'6','&':'7','*':'8','(':'9',')':'0','_':'minus','+':'equal','{':'bracket_left','}':'bracket_right',':':'semicolon','"':'apostrophe','~':'grave_accent','|':'backslash','<':'comma','>':'dot','?':'slash'}
out=[]
for c in sys.argv[1]:
    if c.isupper(): out.append('shift-'+c.lower())
    elif c.isalnum(): out.append(c)
    elif c in m: out.append(m[c])
    elif c in sh: out.append('shift-'+m.get(sh[c],sh[c]))
print(' '.join(out))
PY
}
keys() { for k in "$@"; do echo "sendkey $k" | socat - UNIX-CONNECT:"$W/mon.sock" >/dev/null; sleep 0.12; done; }
type_text() { keys $(keymap "$1"); }
answer() { wait_for "$1" 1 "${3:-30}" && type_text "$2"; }  # PROMPT TEXT [SECONDS]
wait_end() { for _ in $(seq $(($1 * 2))); do kill -0 $QPID 2>/dev/null || { QPID=""; return 0; }; sleep 0.5; done; echo "FAIL: QEMU still running"; kill $QPID; QPID=""; fail=1; }
stop_qemu() { [ -n "$QPID" ] && kill $QPID 2>/dev/null; wait $QPID 2>/dev/null; QPID=""; }
expect() { log | grep -aqF -- "$1" || { echo "FAIL: expected \"$1\""; fail=1; }; }
# A partition of a disk image: its first and last block
part_range() { python3 - "$1" "$2" <<'PY'
import struct, sys
d = open(sys.argv[1], 'rb'); d.seek(1024 + (int(sys.argv[2]) - 1) * 128 + 32)
first, last = struct.unpack('<QQ', d.read(16)); print(first, last)
PY
}
fsck_part() {  # IMAGE N LABEL
    read -r first last < <(part_range "$1" "$2")
    dd if="$1" of="$W/part.img" bs=512 skip=$first count=$((last - first + 1)) status=none
    if e2fsck -fn "$W/part.img" > "$W/fsck.out" 2>&1; then echo "$3: e2fsck clean"; else echo "FAIL: $3: e2fsck"; cat "$W/fsck.out"; fail=1; fi
}
win_sums() {  # the "Windows" partitions' contents
    read -r f1 l1 < <(part_range "$1" 1); read -r f2 l2 < <(part_range "$1" 2)
    { dd if="$1" bs=512 skip=$f2 count=$((l2 - f2 + 1)) status=none | sha256sum
      dd if="$1" of="$W/esp.part" bs=512 skip=$f1 count=$((l1 - f1 + 1)) status=none
      mcopy -n -i "$W/esp.part" ::/EFI/Microsoft/Boot/bootmgfw.efi - | sha256sum
      mcopy -n -i "$W/esp.part" ::/EFI/BOOT/BOOTX64.EFI - | sha256sum; } | cut -d' ' -f1 | tr '\n' ' '
}
LOGIN="login: "

echo "== A1: install on the empty NVMe disk from the menu's Install entry"
start_qemu "$W/vars-a.fd" $LIVE $NVME
if wait_for "rboot: booting rustica in 2 s" 1 60; then
    keys down down ret
    answer "Keyboard layout" $'\n' 120
    answer "Host name" $'testbox\n'
    answer "Time zone" $'Europe/Berlin\n'
    answer "Full name" $'Alice Test\n'
    answer "User name" $'alice\n'
    answer "Password for alice" $'secret1\n'
    answer "The same again" $'secret1\n'
    answer "Lock root" $'\n'
    answer "The disk to install on" $'\n' 60
    answer "Choice" $'\n'
    answer "Type install" $'install\n'
    answer "Type its name" $'nvme0n1\n'
    answer "Restart now" $'\n' 300
fi
wait_end 60
log | sed -n '/====== The installation/,$p' | grep -av "^\s*$" | head -30
expect "  1) nvme0n1"
expect "(the installation medium: not offered)"
expect "  - ERASE the disk: a new partition table; everything on it is lost"
expect "  - create nvme0n1p1: EFI system partition, 512M, FAT32"
expect "nvme0n1: writes allowed"
expect "Rustica Server is installed on nvme0n1."
expect "\"Rustica\" is first in the boot order."

echo "== A2: the disk alone: rboot, the login prompt; a file; sudo poweroff"
start_qemu "$W/vars-a.fd" $NVME
if answer "testbox $LOGIN" $'alice\n' 120; then
    answer "Password: " $'secret1\n'
    answer "alice@" $'id; cat /etc/timezone; echo kept > notes; mount | grep nvme; bootctl list\n'
    wait_for "alice@" 2 30 && type_text $'sudo poweroff\n'
    answer "assword" $'secret1\n'
fi
wait_end 60
log | grep -a "rboot: booting rustica (\|uid=\|nvme0n1p\|rustica \|power-off" | head -12
expect "root=UUID="
expect "nvme=rw)"
expect "uid=1000(alice) gid=1000(alice) groups=1000(alice),27(sudo)"
expect "Europe/Berlin"
expect "/dev/nvme0n1p2 on / type ext4 (rw)"
expect "/dev/nvme0n1p1 on /boot/efi type vfat (rw)"
expect "Rustica Server  (default)  (selected)"
expect "init: power-off"

echo "== A3: again: the file is there"
start_qemu "$W/vars-a.fd" $NVME
if answer "testbox $LOGIN" $'alice\n' 120; then
    answer "Password: " $'secret1\n'
    answer "alice@" $'cat notes; sudo -S poweroff\n'
    sleep 1; type_text $'secret1\n'
fi
wait_end 60
expect "kept"
fsck_part "$W/nvme.img" 2 "A: nvme0n1p2"

echo "== B: replace that Rustica from the live session"
start_qemu "$W/vars-a.fd" $LIVE $NVME
if wait_for "rboot: booting rustica in 2 s" 1 60 && wait_for "rustux:/root > " 1 120; then
    # twice: the second time reuses the firmware entry of the first
    type_text $'rustica-install --disk nvme0n1 --mode replace --user carol --password pw2 --hostname again --yes --no-reboot\n'
    wait_for "installed on nvme0n1" 1 240
    type_text $'rustica-install --disk nvme0n1 --mode replace --user carol --password pw2 --hostname again --yes --no-reboot; echo COUNT=$(efibootmgr | grep -c Rustica); poweroff\n'
fi
wait_end 300
log | sed -n '/====== The installation/,/installed on/p' | head -20
expect "  - keep nvme0n1p1 (EFI"
expect "  - FORMAT nvme0n1p2"
expect "Rustica Server is installed on nvme0n1."
expect "COUNT=1"
start_qemu "$W/vars-a.fd" $NVME
if answer "again $LOGIN" $'carol\n' 120; then
    answer "Password: " $'pw2\n'
    answer "carol@" $'ls /home; grep -c alice /etc/passwd; sudo -S poweroff\n'
    sleep 1; type_text $'pw2\n'
fi
wait_end 60
log | sed -n '/carol@/,$p' | head -5
expect "carol"
fsck_part "$W/nvme.img" 2 "B: nvme0n1p2"

echo "== C: alongside \"Windows\" (a dry run first)"
BEFORE=$(win_sums "$W/dual.img")
SUM0=$(sha256sum < "$W/dual.img")
start_qemu "$W/vars-c.fd" $LIVE $DUAL
if wait_for "rboot: booting rustica in 2 s" 1 60 && wait_for "rustux:/root > " 1 120; then
    type_text $'lsblk; rustica-install --dry-run --disk auto --mode alongside --user bob --password pw3 --yes; poweroff\n'
fi
wait_end 120
SUM1=$(sha256sum < "$W/dual.img")
log | sed -n '/====== The installation/,/Dry run/p' | head -20
expect "Dry run: nothing was written."
expect "  - keep vd"
expect "Windows Boot Manager"
[ "$SUM0" = "$SUM1" ] || { echo "FAIL: the dry run changed the disk"; fail=1; }
start_qemu "$W/vars-c.fd" $LIVE $DUAL
if wait_for "rboot: booting rustica in 2 s" 1 60 && wait_for "rustux:/root > " 1 120; then
    type_text $'rustica-install --disk auto --mode alongside --user bob --password pw3 --hostname dual --yes --no-reboot; poweroff\n'
fi
wait_end 240
expect "Rustica Server is installed on vd"
expect "the hardware clock keeps local time"
AFTER=$(win_sums "$W/dual.img")
[ "$BEFORE" = "$AFTER" ] && echo "the Windows partition and boot files are unchanged" || { echo "FAIL: Windows' data or boot files changed"; fail=1; }
echo "-- the menu: Windows"
start_qemu "$W/vars-c.fd" $DUAL
if wait_for "rboot: booting rustica in 5 s" 1 90; then
    log | grep -a "rboot: [0-9]* entries"
    keys end up ret
fi
wait_end 60
expect "auto-windows"
expect "FAKEOS \\EFI\\Microsoft\\Boot\\bootmgfw.efi"
echo "-- and Rustica by default"
start_qemu "$W/vars-c.fd" $DUAL
if answer "dual $LOGIN" $'bob\n' 120; then
    answer "Password: " $'pw3\n'
    answer "bob@" $'cat /etc/adjtime | tail -1; sudo -S poweroff\n'
    sleep 1; type_text $'pw3\n'
fi
wait_end 60
expect "LOCAL"
fsck_part "$W/dual.img" 3 "C: the root"

echo "== D: the power cut during the installation"
start_qemu "$W/vars-d.fd" $LIVE $CUT
if wait_for "rboot: booting rustica in 2 s" 1 60 && wait_for "rustux:/root > " 1 120; then
    type_text $'rustica-install --disk auto --mode alongside --user bob --password pw3 --yes --no-reboot\n'
    wait_for "Setting up the root filesystem" 1 120
fi
stop_qemu
echo "-- power cut after: $(log | grep -a "^Setting up\|^Making\|^Installing" | tail -1)"
start_qemu "$W/vars-d.fd" $CUT
wait_for "FAKEOS" 1 90
wait_end 30
expect "FAKEOS \\EFI\\BOOT\\BOOTX64.EFI"
[ "$BEFORE" = "$(win_sums "$W/dual-cut.img")" ] || { echo "FAIL: Windows' data or boot files changed (power cut)"; fail=1; }

[ $fail = 0 ] && echo "run-install-test: all checks passed"
exit $fail
