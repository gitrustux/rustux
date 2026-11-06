# Devices (step 12): devmgr, lspci, block devices, the edu test driver
lspci > /tmp/lspci.txt; echo lspci $?
test -s /tmp/lspci.txt && echo devices listed
lspci --json | head -c 1
echo
lspci -s 00:00.0 -n | cut -c1-8
blktest -q | tail -1
# A size no other disk is likely to have; other RAM disks may be running
ramdisk -q 3M &
sleep 1
lsblk -n | tr -s ' ' | grep ' 3M ' | cut -d' ' -f2-
lsblk --json -b | grep -c '"size":3145728[,}]'
kill $!
# The edu device is there under QEMU (-device edu): its checks must pass
if lspci -n | grep -q 1234:11e8; then edutest -q || echo edutest failed; fi
rm /tmp/lspci.txt
