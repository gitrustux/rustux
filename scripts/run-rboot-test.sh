#!/bin/bash
# The boot manager test (step 19, docs/BOOT.md). QEMU with OVMF's code and
# a variable store of its own (so the firmware keeps its variables between
# boots, as a computer does), two virtio disks with GPT:
#   ESP:   rboot as \EFI\BOOT\BOOTX64.EFI and \EFI\Rustica\rboot.efi, the
#          kernel, loader.conf (timeout 3), an entry "rustica" (quiet) and
#          an entry "bootfs" (the boot image as a separate file)
#   other: "Windows" and "Debian": rboot/src/bin/fakeos.rs, which writes
#          "FAKEOS <path> [<options>]" and powers off
# Boots:
#   1. the menu: Down x4, Enter starts Windows; 2. End, Up: Debian;
#   3. e, " noefi", Enter: the kernel gets the edited line, and efibootmgr
#      says there are no runtime services;
#   4. the default after the countdown: quiet (no kernel lines on the
#      screen), efibootmgr -c makes a firmware entry for rboot, bootctl
#      set-default rustica-recovery, set-timeout 1, reboot;
#   5. the firmware starts that entry (BootCurrent), rboot boots the
#      recovery entry after 1 s: rescue mode; bootctl reboot-to-firmware;
#   6. the bootfs entry: the kernel loads the boot image from the ESP.
#
#   scripts/run-rboot-test.sh [KERNEL.efi]
#
# Needs qemu-system-x86_64, OVMF (code and variables as separate files),
# mtools, dosfstools, socat, python3, and rboot built (build.sh does it).
set -u
cd "$(dirname "$0")/.."
EFI=${1:-target/x86_64-unknown-uefi/release/rustux.efi}
RB=rboot/target/x86_64-unknown-uefi/release
[ -f "$EFI" ] || { echo "no $EFI (build it first)"; exit 2; }
[ -f $RB/rboot.efi ] && [ -f $RB/fakeos.efi ] || { echo "no $RB/rboot.efi or fakeos.efi (cd rboot && cargo build --release --target x86_64-unknown-uefi)"; exit 2; }
CODE="" VARS=""
for d in /usr/share/OVMF /usr/share/edk2/ovmf /usr/share/edk2-ovmf/x64 /usr/share/qemu; do
    for s in _4M ""; do
        if [ -z "$CODE" ] && [ -f "$d/OVMF_CODE$s.fd" ] && [ -f "$d/OVMF_VARS$s.fd" ]; then CODE=$d/OVMF_CODE$s.fd; VARS=$d/OVMF_VARS$s.fd; fi
    done
done
[ -n "$CODE" ] || { echo "OVMF_CODE.fd and OVMF_VARS.fd not found (the variables must be kept)"; exit 2; }
export PATH=$PATH:/sbin:/usr/sbin
for t in qemu-system-x86_64 mkfs.fat mcopy socat python3; do
    command -v $t >/dev/null || { echo "$t is needed"; exit 2; }
done

W=$(mktemp -d)
QPID=""
cleanup() { [ -n "$QPID" ] && kill $QPID 2>/dev/null; rm -rf "$W"; }
trap cleanup EXIT
fail=0

# The disks
BOOTFS=$(ls -t target/x86_64-unknown-uefi/release/build/rustux-*/out/bootfs.img 2>/dev/null | head -1)
[ -n "$BOOTFS" ] || { echo "no bootfs.img in target/ (build the kernel first)"; exit 2; }
mkdir -p "$W/esp/EFI/BOOT" "$W/esp/EFI/Rustica" "$W/esp/loader/entries" "$W/other/EFI/Microsoft/Boot" "$W/other/EFI/debian"
cp $RB/rboot.efi "$W/esp/EFI/BOOT/BOOTX64.EFI"
cp $RB/rboot.efi "$W/esp/EFI/Rustica/rboot.efi"
cp "$EFI" "$W/esp/EFI/Rustica/rustux.efi"
printf 'default rustica\ntimeout 3\n' > "$W/esp/loader/loader.conf"
printf 'title Rustica\nefi /EFI/Rustica/rustux.efi\noptions quiet\nsort-key 1\n' > "$W/esp/loader/entries/rustica.conf"
if [ -n "$BOOTFS" ]; then
    cp "$BOOTFS" "$W/esp/EFI/Rustica/bootfs.img"
    printf 'title Rustica (boot image from a file)\nefi /EFI/Rustica/rustux.efi\nbootfs /EFI/Rustica/bootfs.img\nsort-key 2\n' > "$W/esp/loader/entries/bootfs.conf"
fi
cp $RB/fakeos.efi "$W/other/EFI/Microsoft/Boot/bootmgfw.efi"
cp $RB/fakeos.efi "$W/other/EFI/debian/shimx64.efi"
truncate -s 192M "$W/esp.fat"; mkfs.fat -F 32 -n ESP "$W/esp.fat" >/dev/null; mcopy -i "$W/esp.fat" -s "$W/esp/EFI" "$W/esp/loader" ::
truncate -s 40M "$W/other.fat"; mkfs.fat -F 32 "$W/other.fat" >/dev/null; mcopy -i "$W/other.fat" -s "$W/other/EFI" ::
python3 scripts/gpt-wrap.py --disk 1 "$W/esp.img" esp:"$W/esp.fat"
python3 scripts/gpt-wrap.py --disk 2 "$W/other.img" esp:"$W/other.fat"
cp "$VARS" "$W/vars.fd"

log() { sed 's/\x1b\[[0-9;]*[a-zA-Z]//g' "$W/dbg.log" 2>/dev/null; }
start_qemu() {
    rm -f "$W/dbg.log" "$W/mon.sock"
    qemu-system-x86_64 -machine q35 -m 1G -smp 2 \
        -drive if=pflash,format=raw,readonly=on,file="$CODE" -drive if=pflash,format=raw,file="$W/vars.fd" \
        -drive file="$W/esp.img",if=none,id=e,format=raw -device virtio-blk-pci,drive=e \
        -drive file="$W/other.img",if=none,id=o,format=raw -device virtio-blk-pci,drive=o \
        -display none -debugcon file:"$W/dbg.log" -no-reboot -monitor unix:"$W/mon.sock",server,nowait >/dev/null 2>&1 &
    QPID=$!
}
wait_for() {  # TEXT COUNT SECONDS
    for _ in $(seq $(($3 * 2))); do
        [ "$(log | grep -ac -- "$1")" -ge "$2" ] && return 0
        kill -0 $QPID 2>/dev/null || return 1
        sleep 0.5
    done
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
keys() { for k in "$@"; do echo "sendkey $k" | socat - UNIX-CONNECT:"$W/mon.sock" >/dev/null; sleep 0.15; done; }
type_text() { keys $(keymap "$1"); }
wait_end() { for _ in $(seq $(($1 * 2))); do kill -0 $QPID 2>/dev/null || { QPID=""; return 0; }; sleep 0.5; done; echo "FAIL: QEMU still running"; kill $QPID; QPID=""; fail=1; }
expect() { log | grep -aqF -- "$1" || { echo "FAIL: expected \"$1\""; fail=1; }; }
PROMPT="rustux:/root > "

echo "== boot 1: the menu, Windows"
start_qemu
wait_for "rboot: booting rustica in 3 s" 1 60 && keys down down down down ret
wait_end 30
log | grep -a "^rboot: [0-9]\|FAKEOS"
expect "entries: rustica rustica-recovery bootfs bootfs-recovery auto-windows auto-debian"
expect "FAKEOS \\EFI\\Microsoft\\Boot\\bootmgfw.efi []"

echo "== boot 2: Debian"
start_qemu
wait_for "rboot: booting rustica in 3 s" 1 60 && keys end up ret
wait_end 30
log | grep -a "FAKEOS"
expect "FAKEOS \\EFI\\debian\\shimx64.efi []"

echo "== boot 3: the command line edited (noefi)"
start_qemu
if wait_for "rboot: booting rustica in 3 s" 1 60; then
    keys e; type_text $' noefi\n'
    wait_for "$PROMPT" 1 90 && type_text $'efibootmgr\n'
    wait_for "$PROMPT" 2 20 && type_text $'poweroff\n'
fi
wait_end 60
log | grep -a "rboot: booting rustica (\|command line\|efibootmgr:"
expect "rboot: booting rustica (\\EFI\\Rustica\\rustux.efi quiet noefi)"
expect "[BOOT] Command line: quiet noefi"
expect "efibootmgr: cannot read the variables: no UEFI runtime services"

echo "== boot 4: the default (quiet); a firmware entry; rboot's default and timeout"
start_qemu
if wait_for "$PROMPT" 1 120; then
    type_text $'efibootmgr -q -c -L Rustica; efibootmgr | grep Rustica; bootctl set-default rustica-recovery; bootctl set-timeout 1; bootctl list\n'
    wait_for "$PROMPT" 2 30 && type_text $'reboot\n'
fi
wait_end 60
log | sed -n '/efibootmgr -q/,$p' | head -12
expect "rboot: booting rustica (\\EFI\\Rustica\\rustux.efi quiet)"
expect "[EFI] runtime services at kernel addresses"
expect "* Rustica"
expect "rustica-recovery             Rustica (recovery)  (default)"
expect "bootfs                       Rustica (boot image from a file)"
NUM=$(log | grep -ao "Boot[0-9A-F]\{4\}\* Rustica" | head -1 | cut -c5-8)

echo "== boot 5: the firmware starts rboot's entry ($NUM); the recovery entry after 1 s"
start_qemu
if wait_for "$PROMPT" 1 120; then
    type_text $'efibootmgr | head -1; bootctl | grep entry; bootctl reboot-to-firmware yes; bootctl reboot-to-firmware; bootctl reboot-to-firmware no\n'
    wait_for "$PROMPT" 2 30 && type_text $'poweroff\n'
fi
wait_end 60
log | grep -a "^rboot: booting\|rescue mode" | head -5
log | sed -n '/efibootmgr | head/,$p' | head -8
[ -n "$NUM" ] && expect "BootCurrent: $NUM"
expect "rboot: booting rustica-recovery in 1 s"
expect "rboot: booting rustica-recovery (\\EFI\\Rustica\\rustux.efi rescue)"
expect "init: rescue mode"
expect "Current entry: rustica-recovery"
expect "Default entry: rustica-recovery (set with bootctl set-default)"
log | sed -n '/reboot-to-firmware yes/,$p' | grep -aq "^yes" || { echo "FAIL: reboot-to-firmware did not read back yes"; fail=1; }

if [ -n "$BOOTFS" ]; then
    echo "== boot 6: the boot image from the ESP (bootfs=)"
    start_qemu
    if wait_for "rboot: booting rustica-recovery in 1 s" 1 60; then
        keys home down down ret
        wait_for "$PROMPT" 1 120 && type_text $'poweroff\n'
    fi
    wait_end 60
    log | grep -a "rboot: booting bootfs\|boot image from"
    expect "bootfs=/EFI/Rustica/bootfs.img"
    expect "[INIT] boot image from bootfs="
fi

[ $fail = 0 ] && echo "run-rboot-test: all checks passed"
exit $fail
