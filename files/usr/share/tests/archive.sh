# Archives: tar and gzip
d=/tmp/rt-arch
rm -rf $d; mkdir -p $d/src/inner; cd $d
echo first > src/one.txt
echo second > src/inner/two.txt
tar cf a.tar src
tar tf a.tar | sort
tar czf a.tgz src
mkdir out; tar xzf a.tgz -C out
cat out/src/inner/two.txt
seq 1 1000 > nums
gzip nums
test -f nums.gz && echo compressed
test -f nums || echo original-gone
zcat nums.gz | tail -1
gunzip nums.gz
wc -l < nums
find src -name '*.txt' | sort | xargs cat
cd /; rm -rf $d
