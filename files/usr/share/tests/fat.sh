# FAT filesystems (step 14): mkfs.fat, mount and umount, files through
# fsd's forwarding, long names, fsck.fat; on RAM disks, so it runs on any
# machine
ramdisk -q -n fa 40M &
ra=$!
sleep 1
mkfs.fat -F 32 -n Rustux -i 12345678 fa
blkid fa
mkdir /tmp/fat
mount fa /tmp/fat
mount | grep '^/dev/fa '
df /tmp/fat | tail -1 | tr -s ' ' | cut -d' ' -f2-4
echo hello fat > /tmp/fat/hello.txt
cat /tmp/fat/hello.txt
mkdir "/tmp/fat/A Long Directory Name"
echo umlaut > "/tmp/fat/A Long Directory Name/Grüße und Ärger.txt"
echo short > /tmp/fat/lower.txt
echo mixed > /tmp/fat/MixedCase.Txt
echo dots > /tmp/fat/archive.tar.gz
ls /tmp/fat "/tmp/fat/A Long Directory Name"
cat "/tmp/fat/A LONG DIRECTORY NAME/grüße und ärger.txt" 2>&1
cat /tmp/fat/LOWER.TXT /tmp/fat/mixedcase.txt
# a large file, and many files (the directory grows past a cluster)
seq 1 150000 > /tmp/big.txt
cp /tmp/big.txt /tmp/fat/big.txt
cmp /tmp/big.txt /tmp/fat/big.txt && echo big copy ok
wc -c < /tmp/fat/big.txt
mkdir /tmp/fat/many
for i in $(seq 1 120); do echo $i > /tmp/fat/many/file-number-$i.txt; done
ls /tmp/fat/many | wc -l
cat /tmp/fat/many/file-number-77.txt
# changes
echo more >> /tmp/fat/hello.txt
cat /tmp/fat/hello.txt
truncate -s 3 /tmp/fat/hello.txt
cat /tmp/fat/hello.txt; echo
truncate -s 70000 /tmp/fat/lower.txt
wc -c < /tmp/fat/lower.txt
mv /tmp/fat/MixedCase.Txt "/tmp/fat/A Long Directory Name/moved.txt"
mv /tmp/fat/archive.tar.gz /tmp/fat/ARCHIVE.TAR.GZ
mv "/tmp/fat/A Long Directory Name" /tmp/fat/many/inside
ls /tmp/fat /tmp/fat/many/inside
rm /tmp/fat/many/file-number-1*.txt
ls /tmp/fat/many | wc -l
rmdir /tmp/fat/many 2>&1 | cut -d: -f3
mv /tmp/fat/hello.txt /tmp/moved-out.txt
cat /tmp/moved-out.txt; echo
ln /tmp/fat/lower.txt /tmp/fat/link 2>&1 | cut -d: -f2-
touch -d '2024-02-29 13:37:42' /tmp/fat/ARCHIVE.TAR.GZ
ls -l --time-style=+%F_%T /tmp/fat/ARCHIVE.TAR.GZ | tr -s ' ' | cut -d' ' -f6-
# FAT keeps one permission bit: read-only
chmod a-w /tmp/fat/ARCHIVE.TAR.GZ
ls -l /tmp/fat/ARCHIVE.TAR.GZ | cut -c1-10
chmod u+w /tmp/fat/ARCHIVE.TAR.GZ
ls -l /tmp/fat/ARCHIVE.TAR.GZ | cut -c1-10
echo bad > '/tmp/fat/a:b' 2>&1 | cut -d: -f3
df /tmp/fat | tail -1 | tr -s ' ' | cut -d' ' -f2-4
# busy while a file is open; fsck of the result
sleep 3 < /tmp/fat/big.txt &
umount /tmp/fat
kill $!
sleep 1
umount /tmp/fat
ls /tmp/fat
mount | grep -c '^/dev/fa '
fsck.fat fa
# read-only, and the files as they were left
mount -o ro fa /tmp/fat
mount | grep '^/dev/fa '
md5sum < /tmp/fat/big.txt; md5sum < /tmp/big.txt
echo x > /tmp/fat/new.txt 2>&1 | cut -d: -f3
umount /tmp/fat
kill $ra
# FAT16 and FAT12, chosen by size; 4 KiB sectors
ramdisk -q -n fb 20M &
sleep 1
mkfs.fat -i 0000ABCD fb
mount -t vfat fb /tmp/fat
cp /tmp/big.txt /tmp/fat/
mkdir /tmp/fat/d1 /tmp/fat/d1/d2
echo deep > /tmp/fat/d1/d2/file
cat /tmp/fat/d1/d2/file
umount fb
fsck.fat fb | tail -1
kill $!
ramdisk -q -n fc 2M &
sleep 1
mkfs.fat -n tiny -i 00000012 fc
mount fc /tmp/fat
seq 1 300 > /tmp/fat/numbers
tail -1 /tmp/fat/numbers
umount /tmp/fat
fsck.fat fc | tail -1
kill $!
ramdisk -q -n fd -b 4096 80M &
sleep 1
mkfs.fat -F 32 -i 00004096 fd 2>&1
mkfs.fat -i 00004096 fd
mount fd /tmp/fat
cp /tmp/big.txt /tmp/fat/b
cmp /tmp/big.txt /tmp/fat/b && echo 4k ok
umount /tmp/fat
fsck.fat fd | tail -1
kill $!
mkfs.fat nothing 2>&1
mount nothing /tmp/fat 2>&1
umount /tmp/fat 2>&1
rmdir /tmp/fat
rm /tmp/big.txt /tmp/moved-out.txt
