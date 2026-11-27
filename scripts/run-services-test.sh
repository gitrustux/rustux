#!/bin/bash
# The services test (step 18, docs/TESTS.md). Boots Rustux in QEMU three
# times with the root filesystem on an ext4 disk (root=UUID=...):
#   1. autologin as root: /usr/share/services/boot1.sh lists the units,
#      checks the built-in ones (no /etc/svc), runs a unit as another user
#      and finds its output in the log, then replaces the console's login
#      with a unit that cannot start, and powers off;
#   2. the login cannot start: init must start the emergency root shell;
#      it reads the last boot's log (the power-off), puts the login back;
#   3. /etc/svc removed on the host first (a disk from before step 18):
#      it comes back from the boot image; "rescue" on the command line:
#      a root shell, no authd, no login.
# Then the host's e2fsck -f finds the disk clean and /var/log/system.log
# has the three boots. Exit status 0 if all is well.
#
#   scripts/run-services-test.sh [KERNEL.efi]
#
# Needs qemu-system-x86_64, OVMF, mtools, e2fsprogs, socat, python3.
set -u
cd "$(dirname "$0")/.."
EFI=${1:-target/x86_64-unknown-uefi/release/rustux.efi}
[ -f "$EFI" ] || { echo "no $EFI (build it first)"; exit 2; }
OVMF=""
for p in /usr/share/ovmf/OVMF.fd /usr/share/qemu/OVMF.fd /usr/share/edk2-ovmf/x64/OVMF_CODE.fd /usr/share/OVMF/OVMF_CODE.fd; do
    [ -f "$p" ] && OVMF=$p && break
done
[ -n "$OVMF" ] || { echo "OVMF firmware not found"; exit 2; }
export PATH=$PATH:/sbin:/usr/sbin
for t in qemu-system-x86_64 mformat mmd mcopy mkfs.ext4 e2fsck debugfs socat python3; do
    command -v $t >/dev/null || { echo "$t is needed"; exit 2; }
done
W=$(mktemp -d)
trap 'rm -rf "$W"' EXIT
ROOT_UUID=33333333-4444-5555-8666-888888888888

boot_image() { # with a command line
    rm -f "$W/boot.img"
    dd if=/dev/zero of="$W/boot.img" bs=1M count=128 status=none
    mformat -i "$W/boot.img" -F ::
    mmd -i "$W/boot.img" ::/EFI ::/EFI/BOOT
    mcopy -i "$W/boot.img" "$EFI" ::/EFI/BOOT/BOOTX64.EFI
    echo "$1" > "$W/cmdline.txt"
    mcopy -i "$W/boot.img" "$W/cmdline.txt" ::/EFI/BOOT/cmdline.txt
}
keys() {
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
type_text() {
    for k in $(keys "$1"); do
        echo "sendkey $k" | socat - UNIX-CONNECT:"$W/mon" > /dev/null
        sleep 0.08
    done
}
log() { [ -f "$W/console.log" ] || return 0; sed 's/\x1b\[[0-9;?]*[a-zA-Z]//g; s/\r$//' "$W/console.log"; }
# Wait until `pattern` appears for the n-th time (seconds at most)
wait_for() { # pattern, n, seconds
    for _ in $(seq $(( $3 * 2 ))); do
        [ "$(log | grep -ac -- "$1")" -ge "$2" ] && return 0
        sleep 0.5
    done
    echo "FAIL: did not see \"$1\" (${2}x) in time"; fail=1
    return 1
}
start_qemu() {
    rm -f "$W/console.log" "$W/mon"
    qemu-system-x86_64 -machine q35 -m 1G -smp 2 -bios "$OVMF" -drive file="$W/boot.img",format=raw \
        -drive file="$W/root.img",if=none,id=r,format=raw -device virtio-blk-pci,drive=r \
        -display none -debugcon file:"$W/console.log" -no-reboot -monitor unix:"$W/mon",server,nowait \
        >/dev/null 2>&1 &
    Q=$!
}
wait_end() { # seconds
    for _ in $(seq $1); do
        kill -0 $Q 2>/dev/null || return 0
        sleep 1
    done
    echo "FAIL: QEMU did not power off"; fail=1
    kill $Q 2>/dev/null
    wait $Q 2>/dev/null
}
expect() { # text that must be in the console log
    log | grep -aqF -- "$1" || { echo "FAIL: expected: $1"; fail=1; }
}
host_check() {
    if e2fsck -fn "$W/root.img" > "$W/fsck.txt" 2>&1; then
        echo "host e2fsck -f ($1): $(tail -1 "$W/fsck.txt")"
    else
        echo "FAIL: host e2fsck -f ($1):"; sed -n '2,30p' "$W/fsck.txt"; fail=1
    fi
    state=$(dumpe2fs -h "$W/root.img" 2>/dev/null | sed -n 's/^Filesystem state: *//p')
    [ "$state" = clean ] || { echo "FAIL: not marked clean after $1 ($state)"; fail=1; }
}

fail=0
truncate -s 256M "$W/root.img"
mkfs.ext4 -q -L rootfs -U $ROOT_UUID "$W/root.img"
PROMPT="rustux.*:.*/root.* > "

echo "== boot 1 (autologin as root)"
boot_image "root=UUID=$ROOT_UUID"
start_qemu
wait_for "$PROMPT" 1 120 && type_text $'sh /usr/share/services/boot1.sh\n'
wait_end 300
log | sed -n '/SVCTEST start/,/SVCTEST done/p'
expect "logd running enabled"
expect "authd running enabled"
expect "console running enabled"
expect "mounts "
expect "builtin"
expect "hello from hello as svcuser"
expect "Enabled badlogin (/etc/svc/enabled/badlogin)"
expect "init: power-off: stopping the services, writing out the disks"
host_check "boot 1"

echo "== boot 2 (the login cannot start: the emergency shell)"
start_qemu
if wait_for "nothing runs on the console" 1 120 && wait_for "$PROMPT" 1 30; then
    type_text $'svc is-active badlogin emergency; logs --list-boots | tr -s " " | cut -d" " -f2,3\n'
    wait_for "$PROMPT" 2 30
    type_text $'logs -b -1 -u init -o cat | tail -3; logs -b -1 -t boot1 -o cat\n'
    wait_for "$PROMPT" 3 30
    type_text $'svc disable badlogin; svc enable console; rm /etc/svc/badlogin.svc; poweroff\n'
fi
wait_end 120
log | sed -n '/nothing runs on the console/,$p' | grep -av "^\s*$" | head -30
expect "init: badlogin failed: cannot start /bin/no-such-login"
expect "init: nothing runs on the console"
expect "failed"
expect "stopping the services"
expect "the services have stopped; writing out the disks"
expect "the first boot ends"
host_check "boot 2"

echo "== boot 3 (/etc/svc removed: it comes back; rescue mode)"
{
    for f in $(debugfs -R "ls -p /etc/svc/enabled" "$W/root.img" 2>/dev/null | cut -d/ -f6 | grep -v '^\.*$'); do echo "rm /etc/svc/enabled/$f"; done
    echo "rmdir /etc/svc/enabled"
    for f in $(debugfs -R "ls -p /etc/svc" "$W/root.img" 2>/dev/null | cut -d/ -f6 | grep '\.svc$'); do echo "rm /etc/svc/$f"; done
    echo "rmdir /etc/svc"
} > "$W/rm.cmds"
debugfs -w -f "$W/rm.cmds" "$W/root.img" > /dev/null 2>&1
debugfs -R "stat /etc/svc" "$W/root.img" 2>&1 | grep -q "not found" || { echo "FAIL: could not remove /etc/svc on the host"; fail=1; }
boot_image "root=UUID=$ROOT_UUID rescue"
start_qemu
if wait_for "init: rescue mode" 1 120 && wait_for "$PROMPT" 1 30; then
    type_text $'id -u; svc get-default; svc is-active authd console rescue-shell; svc is-enabled console; ls /etc/svc | wc -l\n'
    wait_for "$PROMPT" 2 30
    type_text $'logs --list-boots | wc -l; poweroff\n'
fi
wait_end 120
log | sed -n '/init: rescue mode/,$p' | grep -av "^\s*$" | head -30
expect "init: rescue mode"
expect "rescue"
log | grep -aq "no /etc/svc" && { echo "FAIL: /etc/svc did not come back"; fail=1; }
host_check "boot 3 (rescue)"

# The log on the disk: three boots, root's and adm's
debugfs -R "cat /var/log/system.log" "$W/root.img" 2>/dev/null > "$W/system.log"
boots=$(cut -f2 "$W/system.log" | sort -un | tr '\n' ' ')
[ "$boots" = "1 2 3 " ] || { echo "FAIL: boots in the log: $boots"; fail=1; }
grep -q $'\tlogd\t' "$W/system.log" || { echo "FAIL: no logd lines"; fail=1; }
grep -q $'\tkernel\t0\t0\t' "$W/system.log" || { echo "FAIL: no kernel lines"; fail=1; }
grep -q $'\thello\t[0-9]*\t1000\thello from hello as svcuser$' "$W/system.log" || { echo "FAIL: hello's line (as uid 1000)"; fail=1; }
m=$(debugfs -R "stat /var/log/system.log" "$W/root.img" 2>/dev/null | sed -n 's/.*Mode: *\([0-7]*\).*/\1/p; s/^User: *\([0-9]*\) *Group: *\([0-9]*\).*/\1 \2/p' | tr '\n' ' ')
[ "$m" = "0640 0 4 " ] || { echo "FAIL: /var/log/system.log is \"$m\", not \"0640 0 4\""; fail=1; }
echo "the log: $(wc -l < "$W/system.log") lines, boots $boots"
[ $fail = 0 ] && echo "run-services-test: all checks passed"
exit $fail
