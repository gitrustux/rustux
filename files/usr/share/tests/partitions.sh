# Partition tables (step 13): gpt, lsblk, blkid, fdisk -l and blk on RAM
# disks, so it runs on any machine
ramdisk -q -n pt 32M &
sleep 1
gpt create pt | cut -d, -f1
gpt add -s 8M -t efi -l boot pt
gpt add -t linux -l data pt
lsblk -o NAME,SIZE,TYPE,PARTLABEL,PARTTYPENAME pt
echo hello partition | blk write pt2
blk read pt2 -n 15; echo
# the same bytes through the disk: pt2 starts at block 18432
blk read pt -o $((18432 * 512)) -n 15; echo
gpt label -i 2 -l renamed pt
blkid -s PARTLABEL pt2
blk info pt2 | grep -e start -e number -e label
gpt remove -i 1 pt
lsblk -n -l -o NAME pt
gpt show pt | tail -1 | tr -s ' ' | cut -d' ' -f2-
fdisk -l pt | grep -e Disklabel -e '^pt2'
gpt create pt 2>&1; echo create $?
gpt add -s 999M pt 2>&1; echo add $?
gpt destroy pt
blk info pt | grep table
lsblk -n -l -o NAME pt
blk write pt -o 33554430 < /etc/hostname 2>&1 | cut -d';' -f1
kill $!
# 4 KiB blocks: a smaller table, different usable range
ramdisk -q -n pq -b 4096 16M &
sleep 1
gpt create pq
gpt add -l four pq
lsblk -o NAME,SIZE,LOG-SEC,PARTLABEL pq
kill $!
