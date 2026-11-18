# Run by scripts/run-packages-test.sh, second boot: after the power cut
# half-way through the renames of big 2. rpg fix goes forward: every
# file is version 2. Then an upgrade to big 3 is cut while its files are
# being written beside the old ones.
echo PKGTEST boot2
cd /root/pk
rpg list
rpg fix
rpg list
rpg verify big
echo "version 2 files: $(cat /opt/big/* | grep -c v2)"
echo "rpg-tmp files: $(ls /opt/big | grep -c rpg-tmp)"
ls /var/lib/rpg
echo "PKGTEST cut"
RPG_TEST_PAUSE=stage:100 rpg install ./big_3_all.rpg
