# Run by scripts/run-users-test.sh (step 17), first boot (autologin as
# root, the root filesystem on ext4): a user with sudo, passwords, the
# system settings, files of the user's own; then a clean poweroff.
echo USERSTEST start
mount | head -1
useradd -m -G sudo -c "Alice Test" alice
echo alice:secret | chpasswd
echo root:rootpw | chpasswd
hostname testbox
timedatectl set-timezone Europe/Berlin
localectl set-keymap de
timedatectl | grep "Time zone"
localectl | grep Keymap
su alice -c 'echo "alice was here" > ~/note; mkdir ~/work; chmod 700 ~/work; echo private > ~/work/p; ls -l ~ | tr -s " " | cut -d" " -f1,3,4,9'
su alice -c 'cat /etc/shadow; touch /etc/x; echo secret | sudo -S id 2>&1; echo secret | sudo -S touch /etc/by-sudo'
ls -l /etc/by-sudo | tr -s " " | cut -d" " -f1,3,4
# login asks from the next boot on
rm /etc/autologin
cat /etc/hostname /etc/timezone /etc/keymap
echo USERSTEST done
poweroff
