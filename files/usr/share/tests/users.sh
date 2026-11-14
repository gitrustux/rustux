# Users and permissions (step 17): useradd, chpasswd, su, sudo -S,
# permission bits on tmpfs and ext4, the sticky /tmp, chmod and chown
# rules, the umask, raw disks, other users' processes, root-only commands.
# Leaves the users and files it makes removed again.
cd /root
useradd -m -G sudo tst1
useradd -m tst2
echo tst1:secret1 | chpasswd
echo tst2:secret2 | chpasswd
grep tst /etc/passwd
grep '^sudo' /etc/group
cut -d: -f1 /etc/shadow | grep tst
cut -c1-8 /etc/shadow | grep tst1
ls -ld /home/tst1 | cut -d' ' -f1,3,4
# As tst1
su tst1 -c 'id; cat /etc/shadow; echo x > /etc/passwd; ls /root; echo mine > /tmp/t1; chmod 600 /tmp/t1; ls -l /tmp/t1 | cut -d" " -f1,3,4'
su tst2 -c 'cat /tmp/t1; rm /tmp/t1; mv /tmp/t1 /tmp/t2; chmod 777 /tmp/t1; chown tst2 /tmp/t1; echo x >> /tmp/t1'
su tst1 -c 'chown tst2 /tmp/t1; chgrp sudo /tmp/t1; ls -l /tmp/t1 | cut -d" " -f1,3,4; rm /tmp/t1 && echo removed'
# The umask is inherited
su tst1 -c 'umask 077; shell -c "touch /tmp/t3; ls -l /tmp/t3 | cut -c1-10"; rm /tmp/t3'
# Root-only things
su tst1 -c 'poweroff; reboot; hostname nope; timedatectl set-timezone Europe/Berlin; localectl set-keymap de'
su tst1 -c 'kill $(ps | grep " fsd$" | tr -s " " | cut -d" " -f2) 2>&1 | cut -d: -f1,3'
ps | grep " fsd$" | tr -s " " | cut -d" " -f3
su tst1 -c 'ps | grep " shell$" | tr -s " " | cut -d" " -f3 | sort -u'
# sudo with the password on stdin (a right one is remembered for 5 minutes)
su tst1 -c 'echo wrong | sudo -S id'
su tst1 -c 'echo secret1 | sudo -S id'
su tst1 -c 'sudo id'
su tst2 -c 'echo secret2 | sudo -S id'
su tst1 -c 'echo secret1 | sudo -S -u tst2 id'
su tst2 -c 'echo secret2 | sudo -S -l'
tail -4 /var/log/auth.log | cut -d' ' -f3-
# ext4: owners and modes kept on disk
ramdisk -q -n u4 16M &
ra=$!
sleep 1
mkfs.ext4 -q u4 > /dev/null
mkdir -p /tmp/u4
mount u4 /tmp/u4
mkdir /tmp/u4/shared /tmp/u4/private
chmod 1777 /tmp/u4/shared
chmod 700 /tmp/u4/private
su tst1 -c 'echo a > /tmp/u4/shared/a; mkdir /tmp/u4/shared/d; ls /tmp/u4/private; touch /tmp/u4/x; stat -c "%A %U %G %n" /tmp/u4/shared/a /tmp/u4/shared/d'
su tst1 -c 'blkid u4; umount /tmp/u4; mkfs.ext4 u4'
su tst2 -c 'rm /tmp/u4/shared/a; rmdir /tmp/u4/shared/d; echo b > /tmp/u4/shared/b; chmod 640 /tmp/u4/shared/b'
su tst1 -c 'cat /tmp/u4/shared/b; sudo cat /tmp/u4/shared/b'
stat -c "%A %U %G %n" /tmp/u4/shared/b
umount /tmp/u4
fsck.ext4 u4 | tail -1
kill $ra
rmdir /tmp/u4
# Cleaning up
userdel -r tst1
userdel -r tst2
grep -c tst /etc/passwd /etc/group /etc/shadow
ls /home
