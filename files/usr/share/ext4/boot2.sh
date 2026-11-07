# Run by scripts/run-ext4-test.sh, second boot: what the first one left
echo EXT4TEST start
mount | head -1
cat /root/persist.txt
md5sum < /root/link
cat /home/user/note
echo EXT4TEST done
poweroff
