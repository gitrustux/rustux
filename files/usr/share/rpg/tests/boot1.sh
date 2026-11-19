# Run by scripts/run-packages-test.sh (package step E1), first boot: the
# root filesystem on ext4, autologin as root, the disk RPGTEST mounted on
# /tmp/p with a package built on the host (hello 1.0-1). Installs it,
# builds more packages here and installs, upgrades and removes them; then
# starts an upgrade of 300 files and pauses it half-way through the
# renames, where the host cuts the power. (rpg list leaves out the apps
# that come with the system, FoxDen step FD6b, which the installer service
# may be installing meanwhile.)
echo PKGTEST start
rpg version
rpg list | grep -v '^org\.rustica\.' | grep . || echo "Only the apps that come with the system are installed."
echo "== the host's package"
rpg install /tmp/p/hello_1.0-1_all.rpg
cat /var/tmp-hello
rpg owner /etc/hello.conf /opt/hello/bin/hi
rpg files hello
rpg verify hello
readlink /opt/hello/bin/hi
/bin/shell /opt/hello/bin/hello-rpg
echo "== the administrator changes /etc/hello.conf"
echo "greeting=hi there" >> /etc/hello.conf
echo "== hello 1.1-1, built here"
mkdir -p /root/pk/h2/RPG /root/pk/h2/etc/hello.d /root/pk/h2/opt/hello/share /root/pk/h2/opt/hello/bin
cd /root/pk/h2
printf 'Package: hello\nVersion: 1.1-1\nArchitecture: all\nDescription: says hello\n A test package for rpg (step E1).\n' > RPG/control
printf 'echo "postinst $*" > "$RPG_ROOT/var/tmp-hello"\n' > RPG/postinst
printf 'greeting=hello\nloud=no\n' > etc/hello.conf
printf 'colour=blue\n' > etc/hello.d/defaults
printf 'Hello, version 2\n' > opt/hello/share/README
printf 'new\n' > opt/hello/share/NEW
printf 'echo hello from version 2\n' > opt/hello/bin/hello-rpg
chmod 755 opt/hello/bin/hello-rpg
ln -s hello-rpg opt/hello/bin/hi
cd /root/pk
rpg build h2
rpg install ./hello_1.1-1_all.rpg
cat /var/tmp-hello
echo "-- /etc/hello.conf:"
cat /etc/hello.conf
echo "-- /etc/hello.conf.rpg-new:"
cat /etc/hello.conf.rpg-new
echo "-- /etc/hello.d/defaults:"
cat /etc/hello.d/defaults
ls /opt/hello/share
ls /opt/hello/share/OLD
/bin/shell /opt/hello/bin/hi
rpg verify hello
rpg list | grep -v '^org\.rustica\.' | grep . || echo "Only the apps that come with the system are installed."
echo "== a file of another package"
mkdir -p o/RPG o/opt/hello/share
printf 'Package: other\nVersion: 1\nArchitecture: all\nDescription: takes a file of hello\n' > o/RPG/control
echo x > o/opt/hello/share/README
rpg build o
rpg install ./other_1_all.rpg
cat /opt/hello/share/README
echo "== dependencies"
mkdir -p n/RPG n/opt/needs
printf 'Package: needs-hello\nVersion: 1\nArchitecture: all\nDepends: hello (>= 2.0)\nDescription: needs hello 2\n' > n/RPG/control
echo n > n/opt/needs/f
rpg build n
rpg install ./needs-hello_1_all.rpg
printf 'Package: needs-hello\nVersion: 1\nArchitecture: all\nDepends: hello (>= 1.1)\nDescription: needs hello 1.1\n' > n/RPG/control
rpg build n
rpg install ./needs-hello_1_all.rpg
rpg remove hello
rpg remove hello needs-hello
rpg list | grep -v '^org\.rustica\.' | grep . || echo "Only the apps that come with the system are installed."
ls /etc/hello.conf /etc/hello.d/defaults /opt/hello
rpg remove --purge hello
ls /etc/hello.conf /etc/hello.d
echo "== a file in the boot image (/usr): refused, nothing left"
mkdir -p u/RPG u/usr/share/u
printf 'Package: in-usr\nVersion: 1\nArchitecture: all\nDescription: a file in /usr\n' > u/RPG/control
echo u > u/usr/share/u/f
rpg build u
rpg install ./in-usr_1_all.rpg
rpg list | grep -v '^org\.rustica\.' | grep . || echo "Only the apps that come with the system are installed."
ls /var/lib/rpg
echo "== not as root"
sudo -u nobody rpg list
# (through the installer service, FoxDen step FD4: a package file needs
# an administrator, and no name is given)
echo | sudo -u nobody rpg install /tmp/p/hello_1.0-1_all.rpg
echo "== big: 300 files"
for v in 1 2 3; do
    mkdir -p big$v/RPG big$v/opt/big
    printf "Package: big\nVersion: $v\nArchitecture: all\nDescription: many files\n" > big$v/RPG/control
    for i in $(seq 300); do echo "v$v $i" > big$v/opt/big/f$i; done
    rpg build big$v
done
rpg install ./big_1_all.rpg
echo "PKGTEST cut"
RPG_TEST_PAUSE=rename:150 rpg install ./big_2_all.rpg
