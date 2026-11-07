# Run by scripts/run-ext4-test.sh (step 16), first boot: the root
# filesystem is ext4 made by Linux's mkfs.ext4 (root=UUID=...). Then a
# large copy onto another Linux-made ext4 disk (orphan_file,
# metadata_csum_seed), and mkfs.ext4 on an empty disk which is then
# filled up; fsck.ext4 and e2fsck -f check all three.
echo EXT4TEST start
mount | head -1
a=$(blkid -L linuxa)
b=$(blkid -L BLANKB)
programs="coreutils tools shell fsd ext2fs fatfs mount devmgr"
mkdir -p /tmp/b
mount LABEL=linuxa /mnt
mount | grep /mnt
cat /mnt/from-linux/hello.txt /mnt/from-linux/link
readlink /mnt/from-linux/link /mnt/from-linux/abs
stat -c '%h %s' /mnt/from-linux/hard /mnt/from-linux/sparse
md5sum < /mnt/from-linux/fragmented
ls /mnt/from-linux/hashed | wc -l
seq 1 2000000 > /tmp/seq.txt
mkdir "/mnt/Large Copy" /mnt/programs /mnt/many
for i in 1 2 3; do cp /tmp/seq.txt "/mnt/Large Copy/numbers $i.txt"; done
for p in $programs; do cp /bin/$p /mnt/programs/$p; done
for i in $(seq 1 600); do echo "file $i" > /mnt/many/file-$i; done
rm /mnt/many/file-1*
echo "added to a hashed directory" > /mnt/from-linux/hashed/added
rm /mnt/from-linux/hashed/name-7
rm "/mnt/Large Copy/numbers 2.txt"
mv "/mnt/Large Copy/numbers 3.txt" /mnt/from-linux/moved.txt
ln -s ../programs/shell /mnt/many/shell-link
ln /mnt/from-linux/moved.txt /mnt/second-name
rm /mnt/from-linux/delete-me
truncate -s 50M /mnt/sparse-rustux
echo tail >> /mnt/sparse-rustux
df /mnt | tail -1
umount /mnt
fsck.ext4 $a | tail -1
mkfs.ext4 -L rustuxb -U 22222222-3333-4444-8555-666666666666 $b | tail -1
tune2fs -l $b | grep -e features -e "Journal inode" -e "Flex block"
mount $b /tmp/b
mount | grep /tmp/b
for p in $programs; do cp /bin/$p /tmp/b/$p; done
mkdir /tmp/b/fill
for i in 1 2 3 4 5 6 7 8 9 10 11 12; do cp /tmp/seq.txt /tmp/b/fill/$i 2>/dev/null || break; done
df /tmp/b | tail -1
rm /tmp/b/fill/2 /tmp/b/fill/4
umount /tmp/b
fsck.ext4 $b | tail -1
# On the root filesystem, for the next boot
echo "written in boot one" > /root/persist.txt
seq 1 100000 > /root/numbers
ln -s numbers /root/link
mkdir -p /home/user
echo hello > /home/user/note
rm /tmp/seq.txt
echo EXT4TEST done
poweroff
