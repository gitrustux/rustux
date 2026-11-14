#!/bin/bash
# The users test (step 17, docs/TESTS.md). Boots Rustux in QEMU twice with
# the root filesystem on an ext4 disk (root=UUID=...):
#   1. autologin as root: /usr/share/users/boot1.sh makes a user alice
#      (in group sudo) and passwords, sets the host name, time zone and
#      keyboard layout, has alice make files, and checks what she may not
#      do; it removes /etc/autologin and powers off;
#   2. with "login" on the command line: a wrong password (refused), then
#      alice logs in at the login prompt, cannot touch /etc or /root, and
#      powers the computer off with sudo (her password typed at sudo's
#      prompt).
# Then the host's e2fsck -f finds the disk clean, and debugfs shows the
# files with their owners and modes. Exit status 0 if all is well.
#
#   scripts/run-users-test.sh [KERNEL.efi]
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
ROOT_UUID=22222222-3333-4444-8555-777777777777

boot_image() { # with a command line
    rm -f "$W/boot.img"
    dd if=/dev/zero of="$W/boot.img" bs=1M count=64 status=none
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

echo "== boot 1 (autologin as root)"
boot_image "root=UUID=$ROOT_UUID"
start_qemu
wait_for "rustux.*:.*/root.* > " 1 120 && type_text $'sh /usr/share/users/boot1.sh\n'
wait_end 300
log | sed -n '/USERSTEST start/,/USERSTEST done/p'
expect "uid=0(root) gid=0(root) groups=0(root)"
expect "cat: /etc/shadow: Permission denied"
expect "touch: cannot touch '/etc/x': Permission denied"
expect "-rw-r--r-- root root"
host_check "boot 1"

echo "== boot 2 (the login prompt)"
boot_image "root=UUID=$ROOT_UUID login"
start_qemu
if wait_for "testbox login: " 1 120; then
    type_text $'alice\n'
    wait_for "Password: " 1 20 && type_text $'wrong\n'
    wait_for "Login incorrect" 1 20
    wait_for "testbox login: " 2 20 && type_text $'alice\n'
    wait_for "Password: " 2 20 && type_text $'secret\n'
    if wait_for "alice@rustux:/home/alice \$ " 1 60; then
        type_text $'id; pwd; cat note; touch /etc/x; ls /root; cat /etc/timezone\n'
        wait_for "alice@rustux:/home/alice \$ " 2 30
        type_text $'sudo poweroff\n'
        wait_for "password for alice" 1 30 && type_text $'secret\n'
    fi
fi
wait_end 120
log | sed -n '/testbox login:/,$p' | grep -av "^\s*$" | head -40
expect "Login incorrect"
expect "uid=1000(alice) gid=1000(alice) groups=1000(alice),27(sudo)"
expect "alice was here"
expect "touch: cannot touch '/etc/x': Permission denied"
expect "ls: cannot open directory '/root': Permission denied"
expect "Europe/Berlin"
expect "init: power-off: stopping the services, writing out the disks"
host_check "boot 2 (sudo poweroff)"

# The files on the disk, with their owners and modes
st() { debugfs -R "stat $1" "$W/root.img" 2>/dev/null | sed -n 's/.*Mode: *\([0-7]*\).*/\1/p; s/^User: *\([0-9]*\) *Group: *\([0-9]*\).*/\1 \2/p' | tr '\n' ' '; }
check_file() { # path, "mode uid gid"
    got=$(st "$1" | sed 's/ *$//')
    [ "$got" = "$2" ] || { echo "FAIL: $1 is \"$got\", not \"$2\""; fail=1; }
}
check_file /home/alice "0750 1000 1000"
check_file /home/alice/note "0644 1000 1000"
check_file /home/alice/work "0700 1000 1000"
check_file /etc/shadow "0600 0 0"
check_file /etc/by-sudo "0644 0 0"
check_file /root "0700 0 0"
debugfs -R "cat /etc/shadow" "$W/root.img" 2>/dev/null | grep -q '^alice:\$6\$' || { echo "FAIL: no SHA-512 hash for alice"; fail=1; }
debugfs -R "cat /etc/passwd" "$W/root.img" 2>/dev/null | grep -q '^alice:x:1000:1000:Alice Test:/home/alice:/bin/shell$' || { echo "FAIL: alice's /etc/passwd line"; fail=1; }
debugfs -R "stat /etc/autologin" "$W/root.img" 2>&1 | grep -q "not found" || { echo "FAIL: /etc/autologin is still there"; fail=1; }
[ $fail = 0 ] && echo "run-users-test: all checks passed"
exit $fail
