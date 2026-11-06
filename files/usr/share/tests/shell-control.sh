# Shell: if, while, until, for, case, functions, &&, ||, !, exit status
if true; then echo yes; else echo no; fi
if false; then echo no; elif [ 1 -eq 1 ]; then echo elif; fi
i=0
while [ $i -lt 3 ]; do echo "while $i"; i=$((i + 1)); done
until [ $i -eq 0 ]; do i=$((i - 1)); done
echo "i=$i"
for f in a b c; do
    case $f in
        a) echo "case a" ;;
        b|c) echo "case $f" ;;
    esac
done
case hello.txt in *.txt) echo text ;; *) echo other ;; esac
greet() {
    local name=$1
    echo "hello, $name ($#)"
    return 3
}
greet world extra
echo "status $?"
echo "name is [${name}]"
true && echo and1
false && echo and2
false || echo or1
! false && echo not
for i in 1 2 3 4 5; do
    [ $i -eq 2 ] && continue
    [ $i -eq 4 ] && break
    echo "loop $i"
done
count() { n=0; while read line; do n=$((n + 1)); done; echo "$n lines"; }
printf 'a\nb\nc\n' | count
{ echo group1; echo group2; } | sort -r
(cd /tmp; x=inside; echo "sub $x")
echo "after ${x:-unset}"
fact() { if [ $1 -le 1 ]; then echo 1; else echo $(( $1 * $(fact $(($1 - 1))) )); fi; }
fact 6
false; echo "false gives $?"
sh -c 'exit 7'; echo "exit $?"
