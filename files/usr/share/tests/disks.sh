# Disks (step 13). Under QEMU the command tests attach the test disk of
# scripts/make-test-disk.py three times: as virtio-blk (vda), as NVMe
# (nvme0n1) and as a USB stick (sda). Without them (a real machine) this
# prints nothing.
check() { # what, got, expected
    test "$2" = "$3" || { echo "$1: got:"; echo "$2"; }
}
want='1 EFI system partition EFI System
2 Microsoft reserved partition Microsoft reserved
3 Basic data partition Microsoft basic data
4 rustica Linux filesystem'
if lsblk vda > /dev/null 2>&1; then
    check vda "$(lsblk -n -l -o PARTN,PARTLABEL,PARTTYPENAME vda | tr -s ' ' | cut -d' ' -f2- | grep -v '^$')" "$want"
    check vda3 "$(blkid -o value -s TYPE vda3)" ntfs
    check vda-add "$(gpt add -t linux -l scratch vda)" "vda: partition 5 added, blocks 86016-131038 (22M)"
    seq 1 3000 > /tmp/disk.txt
    blk write vda5 < /tmp/disk.txt
    check vda5 "$(blk read vda5 -n $(wc -c < /tmp/disk.txt) | md5sum)" "$(md5sum < /tmp/disk.txt)"
    check vda-check "$(gpt show vda | tail -1 | tr -s ' ' | cut -d' ' -f2-)" "5 86016 131038 45023 22M Linux filesystem scratch"
    rm /tmp/disk.txt
fi
if lsblk sda > /dev/null 2>&1 && test "$(lsblk -n -d -o MODEL sda)" = "QEMU QEMU HARDDISK"; then
    check sda "$(lsblk -n -l -o PARTN,PARTLABEL,PARTTYPENAME sda | tr -s ' ' | cut -d' ' -f2- | grep -v '^$')" "$want"
    check sda-rm "$(lsblk -n -d -o RM sda | tr -d ' ')" 1
    check sda-add "$(gpt add -t linux -l stick sda)" "sda: partition 5 added, blocks 86016-131038 (22M)"
    seq 1 3000 > /tmp/disk.txt
    blk write sda5 < /tmp/disk.txt
    check sda5 "$(blk read sda5 -n $(wc -c < /tmp/disk.txt) | md5sum)" "$(md5sum < /tmp/disk.txt)"
    rm /tmp/disk.txt
fi
if lsblk nvme0n1 > /dev/null 2>&1; then
    check nvme "$(lsblk -n -l -o PARTN,PARTLABEL,PARTTYPENAME nvme0n1 | tr -s ' ' | cut -d' ' -f2- | grep -v '^$')" "$want"
    check nvme-ro "$(lsblk -n -d -o RO nvme0n1 | tr -d ' ')" 1
    check nvme-write "$(echo x | blk write nvme0n1p4 2>&1)" "blk: nvme0n1p4: write: Read-only file system"
    check nvme-gpt "$(gpt add nvme0n1 2>&1 | cut -d: -f4)" " Read-only file system"
fi
