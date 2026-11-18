# Run by scripts/run-packages-test.sh (package step E2), fourth boot:
# repositories. The host serves https://10.0.2.2:8443/live, signed with
# a test key, behind a test CA; GET /switch/NAME (curl) chooses what is
# behind it: repo1, repo2 (a newer release), bad-index (Packages.gz
# changed after signing), bad-sig (signed by another key), bad-pkg (a
# package changed in the pool).
echo PKGTEST boot4
sw() { curl -sk https://10.0.2.2:8443/switch/$1; echo; }
rpg version
echo "== the image's source and key"
rpg source list
rpg key list
echo "== the test repository"
rpg source remove 1
cp /tmp/p/ca.pem /etc/rpg/ca-certificates.pem
rpg key add /tmp/p/test.pub.pem rpg-test
sw repo1
rpg source add https://10.0.2.2:8443/live redfox main
rpg update
rpg search greet
rpg show hello
rpg install -y hello
cat /opt/hello/version
rpg policy libgreet
rpg install -y tool
rpg list
echo "== a newer release"
sw repo2
rpg update
rpg list --upgradable
rpg hold libgreet
rpg upgrade -y
rpg unhold libgreet
rpg upgrade -y
cat /opt/hello/version
rpg list
echo "== autoremove"
rpg remove hello
rpg autoremove -y
rpg list
echo "== no answer, nothing done"
rpg install hello < /dev/null
rpg list
echo "== tampering"
sw bad-index
rpg update
echo "update status $?"
rpg search tool
sw bad-sig
rpg update
echo "update status $?"
sw bad-pkg
rpg update
rpg clean
rpg install -y hello
echo "install status $?"
rpg list
echo "== an unknown CA"
rm /etc/rpg/ca-certificates.pem
sw repo2
rpg update
echo "== as a user"
sudo -u nobody rpg update
sudo -u nobody rpg search tool
echo PKGTEST done4
poweroff
