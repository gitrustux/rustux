# Run by scripts/run-ext4-test.sh, the power-loss test: heavy writes on
# the ext4 disk (and the ext4 root) until QEMU is killed
echo EXT4TEST start
mount LABEL=powerloss /mnt
mkdir -p /mnt/load
seq 1 200000 > /root/seq.txt
echo EXT4TEST loading
n=0
while true; do
    n=$((n + 1))
    d=/mnt/load/d$((n % 7))
    mkdir -p $d
    cp /root/seq.txt $d/f$n
    echo "line $n" >> /mnt/load/log
    echo "line $n" >> /root/log
    for i in 1 2 3 4 5; do echo "$n $i" > $d/small-$i-$n; done
    mv $d/f$n $d/g$n
    rm -f $d/small-3-$n /mnt/load/d$(((n + 3) % 7))/g$((n - 7))
    truncate -s 1000 $d/small-1-$n
    ln -s g$n $d/l$n
done
