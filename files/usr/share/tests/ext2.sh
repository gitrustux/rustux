# ext2 (step 15): mkfs.ext2, mount, files through fsd's forwarding,
# symbolic and hard links (on tmpfs too), sparse files, indirect blocks,
# a full disk, fsck.ext2; on RAM disks, so it runs on any machine
# Links on tmpfs
mkdir /tmp/lk
echo target > /tmp/lk/file
ln -s file /tmp/lk/rel
ln -s /tmp/lk /tmp/lk-abs
cat /tmp/lk/rel /tmp/lk-abs/rel
readlink /tmp/lk/rel /tmp/lk-abs
ls -l /tmp/lk/rel | tr -s ' ' | cut -d' ' -f1,9-
ln -s nowhere /tmp/lk/dangling
cat /tmp/lk/dangling 2>&1 | cut -d: -f3
ln -s loop /tmp/lk/loop
cat /tmp/lk/loop 2>&1 | cut -d: -f3
realpath /tmp/lk-abs/rel
rm -r /tmp/lk /tmp/lk-abs
ls -l /etc/mtab | tr -s ' ' | cut -d' ' -f1,9-
# A filesystem
ramdisk -q -n ea 64M &
ra=$!
sleep 1
mkfs.ext2 -L test -U 01234567-89ab-4cde-8f01-23456789abcd ea
blkid ea
mkdir /tmp/e
mount ea /tmp/e
mount | grep '^/dev/ea '
ls -a /tmp/e
echo hello ext2 > /tmp/e/hello.txt
cat /tmp/e/hello.txt
mkdir -p /tmp/e/a/b/c
seq 1 1000000 > /tmp/seq.txt
cp /tmp/seq.txt /tmp/e/a/b/c/big
cmp /tmp/seq.txt /tmp/e/a/b/c/big && echo big copy ok
stat -c '%s %b' /tmp/e/a/b/c/big
mkdir /tmp/e/many
for i in $(seq 1 400); do echo $i > /tmp/e/many/a-fairly-long-file-name-number-$i; done
ls /tmp/e/many | wc -l
stat -c '%s' /tmp/e/many
rm /tmp/e/many/a-fairly-long-file-name-number-1*
ls /tmp/e/many | wc -l
# Links
ln -s hello.txt /tmp/e/rel
ln -s /tmp/e/a/b /tmp/e/abs
ln -s ../../hello.txt /tmp/e/a/b/up
ln -s /tmp/seq.txt /tmp/e/out
ln -s a-target-of-more-than-sixty-bytes-is-kept-in-a-block-of-its-own-xxxxxxxxxxxx /tmp/e/long
cat /tmp/e/rel /tmp/e/a/b/up
ls /tmp/e/abs/c
cmp /tmp/e/out /tmp/seq.txt && echo link out of the filesystem ok
readlink /tmp/e/long
ls -l /tmp/e/rel | tr -s ' ' | cut -d' ' -f1,9-
ln /tmp/e/hello.txt /tmp/e/a/hard
stat -c '%h' /tmp/e/hello.txt
rm /tmp/e/hello.txt
cat /tmp/e/a/hard
ln /tmp/e/a /tmp/e/dirlink 2>&1 | cut -d: -f2-
ln /tmp/e/a/hard /tmp/seq-hard 2>&1 | cut -d: -f2-
# Sparse files, truncation, times, permissions
truncate -s 100M /tmp/e/sparse
echo end >> /tmp/e/sparse
stat -c '%s %b' /tmp/e/sparse
tail -c 4 /tmp/e/sparse
truncate -s 10 /tmp/e/a/b/c/big
cat /tmp/e/a/b/c/big; echo
truncate -s 20 /tmp/e/a/b/c/big
od -c /tmp/e/a/b/c/big | head -2
touch -d '2001-02-03 04:05:06' /tmp/e/a/hard
ls -l --time-style=+%F_%T /tmp/e/a/hard | tr -s ' ' | cut -d' ' -f1,6-
chmod 4750 /tmp/e/a/hard
ls -l /tmp/e/a/hard | cut -c1-10
# Renames
mv /tmp/e/a/b /tmp/e/many/b
ls /tmp/e/many/b/c
mv /tmp/e/many /tmp/e/many/b/c 2>&1 | cut -d: -f2-
mv /tmp/e/a/hard /tmp/e/renamed
mkdir /tmp/e/empty
mv /tmp/e/empty /tmp/e/a
ls /tmp/e /tmp/e/a
rmdir /tmp/e/a 2>&1 | cut -d: -f3
mv /tmp/e/renamed /tmp/seq-moved.txt
cat /tmp/seq-moved.txt; rm /tmp/seq-moved.txt
df /tmp/e | tail -1 | tr -s ' ' | cut -d' ' -f2-4
# Busy, then checked; read-only
sleep 3 < /tmp/e/sparse &
umount /tmp/e
kill $!
sleep 1
umount /tmp/e
fsck.ext2 ea
mount -o ro ea /tmp/e
mount | grep '^/dev/ea '
echo x > /tmp/e/new 2>&1 | cut -d: -f3
ls /tmp/e
umount /tmp/e
kill $ra
# 1 KiB blocks, double indirect blocks, a full disk
ramdisk -q -n eb 8M &
rb=$!
sleep 1
mkfs.ext2 -b 1024 -U 00000000-0000-4000-8000-000000000001 eb | head -1
mount eb /tmp/e
cp /tmp/seq.txt /tmp/e/one
cmp /tmp/seq.txt /tmp/e/one && echo double indirect ok
for i in 2 3 4 5 6; do cp /tmp/seq.txt /tmp/e/$i 2>&1 | cut -d: -f3; done
df /tmp/e | tail -1 | tr -s ' ' | cut -d' ' -f5
rm /tmp/e/2
echo still writable > /tmp/e/after; cat /tmp/e/after
umount /tmp/e
fsck.ext2 eb | tail -1
kill $rb
mkfs.ext2 nothing 2>&1
fsck.ext2 nothing 2>&1
rmdir /tmp/e
rm /tmp/seq.txt
