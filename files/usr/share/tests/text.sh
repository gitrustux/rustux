# Text: sort, uniq, wc, head, tail, cut, tr, grep, sed-like tools
d=/tmp/rt-text
rm -rf $d; mkdir $d; cd $d
printf 'banana\napple\ncherry\napple\nbanana\napple\n' > fruit
sort fruit | uniq -c | sort -n | head
sort -u fruit
wc -l < fruit
head -2 fruit
tail -1 fruit
cut -c1-3 fruit | sort | uniq
echo "a,b,c" | cut -d, -f2
echo hello | tr a-z A-Z
grep -n apple fruit
grep -c an fruit
grep -v a fruit
grep -E '^(apple|cherry)$' fruit | sort -u
grep -i APPLE fruit | head -1
printf 'x 3\ny 1\nz 2\n' | sort -k2 -n
seq 1 5 | paste -s -d+
echo $(( $(seq 1 10 | paste -s -d+) ))
printf 'one\ntwo\n' | nl
printf 'b\na\n' > l1; printf 'a\nb\n' > l2
diff l1 l2 > /dev/null; echo "diff status $?"
sort l1 | diff - l2 && echo same
cd /; rm -rf $d
