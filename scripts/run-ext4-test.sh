#!/bin/bash
# The ext4 test against the host's tools (step 16, docs/TESTS.md). Boots
# Rustux in QEMU with root=UUID=... (in \EFI\BOOT\cmdline.txt): the root
# filesystem on an ext4 disk made by the host's mkfs.ext4 (defaults).
#   1. /usr/share/ext4/boot1.sh copies a lot onto a second Linux-made ext4
#      disk (orphan_file, metadata_csum_seed, a hashed directory), runs
#      mkfs.ext4 on a third disk and fills it, fsck.ext4 checks both,
#      writes files on the root for the next boot, and powers off;
#   2. the files of boot 1 are there (boot2.sh);
#   3. the power-loss test, POWERLOSS_RUNS times (default 6): load.sh
#      writes hard on an ext4 disk and on the root until QEMU is killed
#      (after 3 to 20 seconds); the host's e2fsck -fy on copies of both
#      disks replays the journals Rustux wrote; then Rustux boots again
#      (recover.sh: its own replay, at mount) and the host's e2fsck -fn
#      must find both clean.
# The host's e2fsck -f checks the disks after each boot and debugfs reads
# the files back. Exit status 0 if all is well.
#
#   scripts/run-ext4-test.sh [KERNEL.efi]
#
# Needs qemu-system-x86_64, OVMF, mtools, e2fsprogs (mkfs.ext2, e2fsck,
# debugfs), dosfstools, socat, python3.
set -u
cd "$(dirname "$0")/.."
EFI=${1:-target/x86_64-unknown-uefi/release/rustux.efi}
[ -f "$EFI" ] || { echo "no kernel at $EFI (build it first)"; exit 2; }
OVMF=""
for p in /usr/share/ovmf/OVMF.fd /usr/share/qemu/OVMF.fd /usr/share/edk2-ovmf/x64/OVMF_CODE.fd /usr/share/OVMF/OVMF_CODE.fd; do
    [ -f "$p" ] && OVMF=$p && break
done
[ -n "$OVMF" ] || { echo "OVMF firmware not found"; exit 2; }
export PATH="$PATH:/sbin:/usr/sbin"
for t in qemu-system-x86_64 mcopy mformat mkfs.ext4 e2fsck debugfs mkfs.vfat socat python3; do
    command -v $t >/dev/null || { echo "$t is needed"; exit 2; }
done
W=$(mktemp -d)
Q=0
trap 'kill $Q 2>/dev/null; rm -rf "$W"' EXIT
ROOT_UUID=11111111-2222-4333-8444-555555555555

boot_image() { # with a command line, or none
    rm -f "$W/boot.img"
    dd if=/dev/zero of="$W/boot.img" bs=1M count=64 status=none
    mformat -i "$W/boot.img" -F ::
    mmd -i "$W/boot.img" ::/EFI ::/EFI/BOOT
    mcopy -i "$W/boot.img" "$EFI" ::/EFI/BOOT/BOOTX64.EFI
    if [ -n "$1" ]; then
        echo "$1" > "$W/cmdline.txt"
        mcopy -i "$W/boot.img" "$W/cmdline.txt" ::/EFI/BOOT/cmdline.txt
    fi
}

keys() {
    python3 - "$1" <<'PY'
import sys
m={' ':'spc','\n':'ret','-':'minus','/':'slash','.':'dot'}
print(' '.join('shift-'+c.lower() if c.isupper() else c if c.isalnum() else m[c] for c in sys.argv[1]))
PY
}

# Boot, type `sh SCRIPT`, wait until Rustux powers off (at most 10 min)
run() { # script, extra qemu args...
    local script=$1
    shift
    rm -f "$W/console.log" "$W/mon"
    qemu-system-x86_64 -machine q35 -m 1G -smp 2 -bios "$OVMF" -drive file="$W/boot.img",format=raw "$@" \
        -display none -debugcon file:"$W/console.log" -no-reboot -monitor unix:"$W/mon",server,nowait \
        >/dev/null 2>&1 &
    Q=$!
    for _ in $(seq 120); do
        grep -aq "rustux.*:.*/root.* > " "$W/console.log" 2>/dev/null && break
        sleep 1
    done
    for k in $(keys "sh $script"$'\n'); do
        echo "sendkey $k" | socat - UNIX-CONNECT:"$W/mon" > /dev/null
        sleep 0.1
    done
    for _ in $(seq ${RUN_WAIT:-600}); do
        kill -0 $Q 2>/dev/null || break
        sleep 1
    done
    kill $Q 2>/dev/null
    wait $Q 2>/dev/null
    sed 's/\x1b\[[0-9;?]*[a-zA-Z]//g; s/\r$//' "$W/console.log" | sed -n '/EXT4TEST start/,/EXT4TEST done/p'
    grep -aq "EXT4TEST done" "$W/console.log" || { echo "FAIL: $script did not finish"; fail=1; }
}

# Boot, type `sh SCRIPT`, and once it says it is loading, kill QEMU after
# a while: a power loss in the middle of writing
run_kill() { # seconds, script, extra qemu args...
    local secs=$1 script=$2
    shift 2
    rm -f "$W/console.log" "$W/mon"
    qemu-system-x86_64 -machine q35 -m 1G -smp 2 -bios "$OVMF" -drive file="$W/boot.img",format=raw "$@" \
        -display none -debugcon file:"$W/console.log" -no-reboot -monitor unix:"$W/mon",server,nowait \
        >/dev/null 2>&1 &
    Q=$!
    for _ in $(seq 120); do
        grep -aq "rustux.*:.*/root.* > " "$W/console.log" 2>/dev/null && break
        sleep 1
    done
    for k in $(keys "sh $script"$'\n'); do
        echo "sendkey $k" | socat - UNIX-CONNECT:"$W/mon" > /dev/null
        sleep 0.1
    done
    for _ in $(seq 120); do
        grep -aq "EXT4TEST loading" "$W/console.log" && break
        sleep 1
    done
    grep -aq "EXT4TEST loading" "$W/console.log" || { echo "FAIL: $script did not start loading"; fail=1; }
    sleep "$secs"
    kill -9 $Q 2>/dev/null
    wait $Q 2>/dev/null
}

host_check() { # image, what
    if e2fsck -fn "$W/$1.img" > "$W/fsck-$1.txt" 2>&1; then
        echo "host e2fsck -f $1 ($2): $(tail -1 "$W/fsck-$1.txt")"
    else
        echo "FAIL: host e2fsck -f $1 ($2):"; sed -n '2,30p' "$W/fsck-$1.txt"; fail=1
    fi
}

fail=0
# The disks: the root (empty ext4), a Linux-made ext4 with files, a blank one
truncate -s 256M "$W/root.img"
mkfs.ext4 -q -L rootfs -U $ROOT_UUID "$W/root.img"
mkdir -p "$W/src/from-linux/hashed"
echo "hello from the host" > "$W/src/from-linux/hello.txt"
cp "$W/src/from-linux/hello.txt" "$W/src/from-linux/delete-me"
ln -s hello.txt "$W/src/from-linux/link"
ln -s /etc/hostname "$W/src/from-linux/abs"
ln "$W/src/from-linux/hello.txt" "$W/src/from-linux/hard"
truncate -s 30M "$W/src/from-linux/sparse"
echo end >> "$W/src/from-linux/sparse"
seq 1 3000000 > "$W/src/from-linux/fragmented"
for i in $(seq 1 500); do echo "$i" > "$W/src/from-linux/hashed/name-$i"; done
truncate -s 512M "$W/a.img"
mkfs.ext4 -q -L linuxa -O orphan_file,metadata_csum_seed -d "$W/src" "$W/a.img"
e2fsck -fyD "$W/a.img" > /dev/null 2>&1   # index the directories (htree)
debugfs -R "stat /from-linux/hashed" "$W/a.img" 2>/dev/null | grep -q "Flags: 0x81000" || { echo "FAIL: no hashed directory to test with"; fail=1; }
truncate -s 128M "$W/b.img"
mkfs.vfat -n BLANKB "$W/b.img" > /dev/null
truncate -s 128M "$W/pl.img"
mkfs.ext4 -q -L powerloss "$W/pl.img"
DISKS=(-drive file="$W/root.img",if=none,id=r,format=raw -device virtio-blk-pci,drive=r
       -drive file="$W/a.img",if=none,id=a,format=raw -device virtio-blk-pci,drive=a
       -drive file="$W/b.img",if=none,id=b,format=raw -device virtio-blk-pci,drive=b
       -drive file="$W/pl.img",if=none,id=p,format=raw -device virtio-blk-pci,drive=p)

boot_image "root=UUID=$ROOT_UUID"
echo "== boot 1"
run /usr/share/ext4/boot1.sh "${DISKS[@]}"
sed 's/\x1b\[[0-9;?]*[a-zA-Z]//g' "$W/console.log" | grep -aq " on / type ext4" || { echo "FAIL: the root is not ext4"; fail=1; }
echo "== boot 2"
run /usr/share/ext4/boot2.sh "${DISKS[@]}"
for d in root a b; do
    host_check $d "boot 2"
    state=$(dumpe2fs -h "$W/$d.img" 2>/dev/null | sed -n 's/^Filesystem state: *//p')
    feat=$(dumpe2fs -h "$W/$d.img" 2>/dev/null | sed -n 's/^Filesystem features: *//p')
    [ "$state" = clean ] || { echo "FAIL: $d is not marked clean ($state)"; fail=1; }
    case " $feat " in *" needs_recovery "*) echo "FAIL: $d still needs recovery"; fail=1;; esac
done
seq 1 2000000 > "$W/seq.txt"
same() { # image, path, local file
    debugfs -R "dump \"$2\" $W/got" "$W/$1.img" > /dev/null 2>&1
    cmp -s "$W/got" "$3" || { echo "FAIL: $1:$2 differs"; fail=1; }
    rm -f "$W/got"
}
same a "/Large Copy/numbers 1.txt" "$W/seq.txt"
same a /from-linux/moved.txt "$W/seq.txt"
same a /second-name "$W/seq.txt"
same a /from-linux/fragmented "$W/src/from-linux/fragmented"
for p in coreutils tools shell fsd ext2fs fatfs mount devmgr; do
    same a /programs/$p userspace/bin/$p
    same b /$p userspace/bin/$p
done
for i in 1 3; do same b /fill/$i "$W/seq.txt"; done
n=$(debugfs -R "ls /many" "$W/a.img" 2>/dev/null | tr -s ' ' '\n' | grep -c '^file-')
[ "$n" = 489 ] || { echo "FAIL: a:/many has $n files, not 489"; fail=1; }
n=$(debugfs -R "ls /from-linux/hashed" "$W/a.img" 2>/dev/null | tr -s ' ' '\n' | grep -c '^name-\|^added')
[ "$n" = 500 ] || { echo "FAIL: a:/from-linux/hashed has $n names, not 500"; fail=1; }
debugfs -R "cat /root/persist.txt" "$W/root.img" 2>/dev/null | grep -q "written in boot one" || { echo "FAIL: root:/root/persist.txt"; fail=1; }
debugfs -R "stat /from-linux/delete-me" "$W/a.img" 2>&1 | grep -q "not found" || { echo "FAIL: a: a removed file is still there"; fail=1; }
feat=$(dumpe2fs -h "$W/b.img" 2>/dev/null | sed -n 's/^Filesystem features: *//p')
[ "$feat" = "has_journal ext_attr dir_index filetype extent 64bit flex_bg sparse_super large_file huge_file dir_nlink extra_isize metadata_csum" ] ||
    { echo "FAIL: b (Rustux's mkfs.ext4) has features: $feat"; fail=1; }

# The power-loss test
RUNS=${POWERLOSS_RUNS:-6}
for r in $(seq 1 $RUNS); do
    secs=$((3 + RANDOM % 18))
    echo "== power loss $r of $RUNS: QEMU killed after $secs s of writing"
    run_kill $secs /usr/share/ext4/load.sh "${DISKS[@]}"
    for d in pl root; do
        dumpe2fs -h "$W/$d.img" 2>/dev/null | grep -q "^Filesystem features:.*needs_recovery" || echo "  ($d: no recovery needed)"
        cp "$W/$d.img" "$W/copy.img"
        e2fsck -fy "$W/copy.img" > "$W/lx.txt" 2>&1
        lrc=$?
        if [ $lrc -gt 1 ] || grep -q "? yes" "$W/lx.txt" || ! e2fsck -fn "$W/copy.img" > /dev/null 2>&1; then
            echo "FAIL: power loss $r: Linux's replay of $d's journal leaves errors:"; sed -n '2,30p' "$W/lx.txt"; fail=1
        else
            echo "  host e2fsck -fy ($d, a copy): $(grep -a "recovering journal" "$W/lx.txt" | head -1 | sed 's/.*: //')$(grep -q recovering "$W/lx.txt" || echo 'nothing to recover'), clean"
        fi
        rm -f "$W/copy.img"
    done
    RUN_WAIT=180 run /usr/share/ext4/recover.sh "${DISKS[@]}"
    host_check pl "power loss $r, after Rustux's replay"
    host_check root "power loss $r, after Rustux's replay"
done
[ $fail = 0 ] && echo "run-ext4-test: all checks passed"
exit $fail
