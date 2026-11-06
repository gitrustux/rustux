# Shell: variables, quoting, expansions, arithmetic
a=hello
b="$a world"
echo "$b"
echo '$b stays'
echo ${a}x ${missing:-default} ${#a}
c=
echo "[${c:-empty}] [${c-unset}]"
path=/usr/share/tests/file.txt
echo ${path##*/} ${path%/*} ${path#/usr} ${path%.txt}
echo $((1 + 2 * 3)) $(( (1 + 2) * 3 )) $((10 / 3)) $((10 % 3)) $((2 > 1))
n=5
n=$((n + 1))
echo n=$n
echo "$(echo nested $(echo deep))"
echo `echo backticks`
set -- one "two words" three
echo $#
for x in "$@"; do echo "<$x>"; done
for x in $*; do echo "[$x]"; done
words="a b  c"
for w in $words; do echo "w=$w"; done
echo "quoted: $words"
echo tab\	escaped
x=1; y=2
echo "$x$y" '$x' "\$x"
echo ~ | grep -c /
