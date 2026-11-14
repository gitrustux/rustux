# Services and the system log (step 18): the units of /etc/svc, svc
# (start, stop, restart, enable, a crash restarted, the start limit, a
# unit run as nobody), logs, logger, who may do what. Leaves /etc/svc as
# it was.
cd /root
svc is-active logd authd console mounts
svc is-enabled authd emergency
svc get-default
svc list | tr -s ' ' | cut -d' ' -f1-3 | grep -v '^$'
# logger, and logs by unit, as text and JSON
logger -t svctest -p warning hello from the test
sleep 1
logs -b -u svctest -o cat
logs -b -u svctest -o json | cut -d, -f3-4,7
logs -b -k -n 400 | grep -c 'init started'
# A unit that fails: as nobody, restarted after 0.2 s, failed after 3 starts
cat > /etc/svc/crashy.svc << END
description=A test unit that fails
exec=/bin/shell -c "id -u; echo crashing; exit 3"
user=nobody
restart=on-failure
restart_delay=0.2
start_limit=3
END
svc reload
svc start crashy 2> /dev/null
sleep 3
svc is-active crashy
svc show crashy | grep -E '^(State|Starts|LastExit|User|Reason)='
logs -b -u crashy -o cat
logs -b -u init -o cat | grep -c '^started crashy (pid [0-9]*, user nobody)'
logs -b -u init -o cat | grep 'crashy exited' | cut -d';' -f1 | uniq -c | tr -s ' '
svc status crashy | grep -E 'Reason|Starts'
svc reset-failed crashy
svc is-active crashy
# A unit killed is restarted
cat > /etc/svc/sleeper.svc << END
description=A test unit that sleeps
exec=sleep 1000
restart=always
END
svc reload
svc start sleeper
svc is-active sleeper
kill $(svc show sleeper | grep MainPID | cut -d= -f2)
sleep 2
svc show sleeper | grep -E '^(State|Starts|LastExit)='
logs -b -u init -o cat | grep 'sleeper was killed' | cut -d';' -f1
svc restart sleeper
svc show sleeper | grep -E '^(State|Starts)='
svc stop sleeper
sleep 1
svc is-active sleeper
# enable, disable
svc is-enabled sleeper
svc enable sleeper
ls /etc/svc/enabled | grep sleeper
svc disable sleeper
svc is-enabled sleeper
svc start nosuch
# Not root: looking is fine, changing is not; the log is root's and adm's
useradd tst3
su tst3 -c 'svc is-active authd; svc stop authd; svc enable sleeper; logs -n 1'
ls -l /var/log/system.log | cut -d' ' -f1,3,4
userdel tst3
rm /etc/svc/crashy.svc /etc/svc/sleeper.svc
svc reload
svc is-active crashy
