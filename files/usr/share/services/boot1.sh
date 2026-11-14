# Run by scripts/run-services-test.sh (step 18), first boot (autologin as
# root, the root filesystem on ext4): the units, the built-in ones when
# /etc/svc is missing, a unit as another user writing to the log; then the
# console's login is replaced by a unit that cannot start (the next boot
# must fall back to the emergency shell), and a clean poweroff.
echo SVCTEST start
logs --list-boots | tr -s ' ' | cut -d' ' -f2,3
svc list | tr -s ' ' | cut -d' ' -f1-3
# Without /etc/svc: the built-in units
mv /etc/svc /etc/svc.off
svc reload
svc is-enabled logd
mv /etc/svc.off /etc/svc
svc reload
svc is-enabled logd
# A unit as nobody, its output in the log
useradd -c "Service user" svcuser
cat > /etc/svc/hello.svc << END
description=Says hello
type=oneshot
exec=/bin/shell -c "echo hello from \$SERVICE as \$(id -un)"
user=svcuser
END
svc reload
svc start hello
sleep 1
logs -b -u hello -o cat
logger -t boot1 "the first boot ends"
# The next boot: no login on the console (it cannot start)
cat > /etc/svc/badlogin.svc << END
description=A login that is not there
exec=/bin/no-such-login
console=yes
END
svc reload
svc disable console
svc enable badlogin
echo SVCTEST done
poweroff
