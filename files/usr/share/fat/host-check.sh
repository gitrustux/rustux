# Run by scripts/run-fat-test.sh (step 14) under QEMU: writes to two FAT
# disks for the host's fsck.fat and mtools to check. vda: made by the
# host's mkfs.fat; vdb: empty, formatted here, then filled up.
echo FATTEST start
programs="coreutils tools shell fsd fatfs mount mkfs.fat fsck.fat devmgr lsblk"
mkdir -p /tmp/a /tmp/b
mount vda /tmp/a
cat /tmp/a/from-linux/hello.txt
seq 1 2000000 > /tmp/seq.txt
mkdir "/tmp/a/Large Copy" "/tmp/a/Programs from Rustux"
for i in 1 2 3; do cp /tmp/seq.txt "/tmp/a/Large Copy/numbers $i.txt"; done
for p in $programs; do cp /bin/$p "/tmp/a/Programs from Rustux/$p"; done
rm "/tmp/a/Large Copy/numbers 2.txt"
mv "/tmp/a/Large Copy/numbers 3.txt" "/tmp/a/from-linux/Zahlen – drei.txt"
echo "written by Rustux" > /tmp/a/from-linux/rustux.txt
rm /tmp/a/from-linux/delete-me.txt
df /tmp/a | tail -1
umount /tmp/a
mkfs.fat -F 32 -n RUSTUXB -i 0B0B0B0B vdb
mount vdb /tmp/b
mkdir /tmp/b/bin
for p in $programs; do cp /bin/$p /tmp/b/bin/$p; done
cp /tmp/seq.txt /tmp/b/numbers.txt
mkdir -p "/tmp/b/EFI/Rustux Files/deeper"
echo "made on Rustux" > "/tmp/b/EFI/Rustux Files/deeper/README.txt"
# Fill the disk: the last copy stops short ("No space left on device")
mkdir /tmp/b/fill
for i in 1 2 3 4 5 6 7 8 9; do cp /tmp/seq.txt /tmp/b/fill/$i 2>&1 | cut -d: -f3; done
df /tmp/b | tail -1
rm /tmp/b/fill/2 /tmp/b/fill/4
umount /tmp/b
fsck.fat vda | tail -1
fsck.fat vdb | tail -1
rm /tmp/seq.txt
echo FATTEST done
