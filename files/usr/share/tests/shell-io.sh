# Shell: redirections, here-documents, pipes, scripts, globbing
d=/tmp/rt-io
rm -rf $d; mkdir -p $d; cd $d
echo one > f.txt
echo two >> f.txt
cat < f.txt
cat f.txt | wc -l
ls nothing-here 2> err.txt; echo "status $?"
grep -c "No such" err.txt
echo to-stderr 1>&2 2>/dev/null
cat <<END
here $d
END
cat <<'END'
not $expanded
END
touch a.c b.c c.h
echo *.c
echo *.none
echo [ab].c
cat > script.sh <<'END'
#!/bin/sh
echo "script got $# args: $*"
exit 4
END
chmod +x script.sh
./script.sh x y
echo "script status $?"
sh script.sh z
read first rest < f.txt
echo "first=$first"
out=$(cat f.txt | tr a-z A-Z)
echo "$out"
cd /; rm -rf $d
