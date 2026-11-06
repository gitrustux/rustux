# Run by scripts/run-ext2-test.sh, second boot: what the first one left
echo EXT2TEST start
mount | head -1
cat /root/persist.txt
md5sum < /root/link
cat /home/user/note
ls -l /etc/mtab | tr -s ' ' | cut -d' ' -f9-
echo EXT2TEST done
poweroff
