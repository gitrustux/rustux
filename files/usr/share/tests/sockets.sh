# Sockets (network step D2) on the loopback interface: Rust's std::net
# (nettest) and nc, a listener and a connection refused
NETTEST_ADDR=127.0.0.1 nettest local
nc -l 9123 > /tmp/nc-test.txt &
sleep 1
echo through lo | nc 127.0.0.1 9123
sleep 1
cat /tmp/nc-test.txt
rm /tmp/nc-test.txt
nc 127.0.0.1 9124
echo status $?
