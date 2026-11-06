# Files: create, copy, move, list, remove
d=/tmp/rt-files
rm -rf $d
mkdir -p $d/sub/deeper
cd $d
echo hello > a.txt
cp a.txt b.txt
mv b.txt sub/c.txt
ls
ls sub
cat sub/c.txt
cp -r sub copy
ls copy
find . -type f | sort
touch empty
test -s empty || echo empty-is-empty
test -f a.txt && echo a-exists
stat -c '%s %n' a.txt
du -s sub > /dev/null && echo du-ok
rmdir sub/deeper
rm -r sub copy
ls
basename /a/b/c.txt .txt
dirname /a/b/c.txt
realpath ./a.txt
cd /
rm -rf $d
test -e $d || echo gone
