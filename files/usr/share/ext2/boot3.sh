# Run by scripts/run-ext2-test.sh, third boot: no command line, no disk:
# the system runs from memory as before
echo EXT2TEST start
mount | head -1
cat /run/cmdline
echo EXT2TEST done
poweroff
