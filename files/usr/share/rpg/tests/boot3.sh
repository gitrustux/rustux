# Run by scripts/run-packages-test.sh, third boot: after the power cut
# while big 3's files were being written. rpg fix goes back: big stays at
# version 2, untouched. Then big is removed and the computer powered off.
echo PKGTEST boot3
cd /root/pk
echo "rpg-tmp files: $(ls /opt/big | grep -c rpg-tmp)"
rpg fix
rpg list | grep -v '^org\.rustica\.' | grep . || echo "Only the apps that come with the system are installed."
rpg verify big
echo "version 2 files: $(cat /opt/big/* | grep -c v2)"
echo "rpg-tmp files: $(ls /opt/big | grep -c rpg-tmp)"
rpg remove big
rpg list | grep -v '^org\.rustica\.' | grep . || echo "Only the apps that come with the system are installed."
ls /opt/big
rpg fix
echo PKGTEST done
poweroff
