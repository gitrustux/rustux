# Run by scripts/run-ext4-test.sh after QEMU was killed: mounting
# replays the journals (the root's at boot); then a clean unmount
echo EXT4TEST start
mount | head -1
mount LABEL=powerloss /mnt && tail -1 /mnt/load/log
umount /mnt
echo EXT4TEST done
poweroff
