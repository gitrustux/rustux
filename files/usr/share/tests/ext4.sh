# ext4 (step 16): mkfs.ext4, mount, extents (many of them: interleaved
# appends), checksums, the journal, files removed while open, a full
# disk, fsck.ext4 and tune2fs -l; on RAM disks, so it runs on any machine
ramdisk -q -n e4 96M &
ra=$!
sleep 1
mkfs.ext4 -L test4 -U 01234567-89ab-4cde-8f01-23456789abce e4
blkid e4
tune2fs -l e4 | grep -e "features" -e "Journal inode" -e "Flex block" -e "Group descriptor size" -e "Checksum type"
mkdir /tmp/e
mount e4 /tmp/e
mount | grep '^/dev/e4 '
ls -a /tmp/e
seq 1 1000000 > /tmp/seq.txt
cp /tmp/seq.txt /tmp/e/big
cmp /tmp/seq.txt /tmp/e/big && echo big copy ok
stat -c '%s %b' /tmp/e/big
# Many extents: two files appended to in turns
for i in $(seq 1 300); do seq $i $((i + 500)) >> /tmp/e/x1; seq $i $((i + 400)) >> /tmp/e/x2; done
md5sum < /tmp/e/x1
md5sum < /tmp/e/x2
truncate -s 100000 /tmp/e/x1
md5sum < /tmp/e/x1
mkdir -p /tmp/e/d/sub
for i in $(seq 1 300); do echo $i > /tmp/e/d/name-$i; done
ls /tmp/e/d | wc -l
ln -s a-target-of-more-than-sixty-bytes-is-kept-in-a-block-of-its-own-xxxxxxxxxxxx /tmp/e/long
readlink /tmp/e/long
ln /tmp/e/big /tmp/e/d/hard
stat -c '%h' /tmp/e/big
mv /tmp/e/d/sub /tmp/e/sub
# Removed while open: the space comes back at close
sleep 3 < /tmp/e/big &
rm /tmp/e/big /tmp/e/d/hard
df /tmp/e | tail -1 | tr -s ' ' | cut -d' ' -f3
sleep 4
df /tmp/e | tail -1 | tr -s ' ' | cut -d' ' -f3
umount /tmp/e
fsck.ext4 e4
tune2fs -l e4 | grep -e "state" -e "features"
mount -o ro e4 /tmp/e
echo x > /tmp/e/new 2>&1 | cut -d: -f3
md5sum < /tmp/e/x2
umount /tmp/e
kill $ra
# 1 KiB blocks, no journal, a full disk
ramdisk -q -n e5 12M &
rb=$!
sleep 1
mkfs.ext4 -b 1024 -O ^has_journal -U 00000000-0000-4000-8000-000000000002 e5 | head -1
mount e5 /tmp/e
for i in 1 2 3 4 5 6 7 8; do cp /tmp/seq.txt /tmp/e/$i 2>&1 | cut -d: -f3; done
df /tmp/e | tail -1 | tr -s ' ' | cut -d' ' -f5
rm /tmp/e/2
echo still writable > /tmp/e/after; cat /tmp/e/after
cmp /tmp/seq.txt /tmp/e/1 && echo first copy ok
umount /tmp/e
fsck.ext4 e5 | tail -1
kill $rb
mkfs.ext4 nothing 2>&1
tune2fs -l nothing 2>&1
rmdir /tmp/e
rm /tmp/seq.txt
