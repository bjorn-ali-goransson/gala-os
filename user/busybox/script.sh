# The shell language (H3), run as `busybox sh script.sh`: functions,
# loops, case, here-documents, command substitution, arithmetic, exit
# status. Every result is printed on one labelled line; the expected
# lines are the same on Windows and on gala (the `busybox_script`
# scenario).
greet() {
    echo "hello, $1"
    return 3
}
greet gala
echo "status: $?"

sum=0
for i in 1 2 3 4 5; do
    sum=$((sum + i))
done
echo "for: $sum"

n=0
while [ "$n" -lt 4 ]; do
    n=$((n + 1))
done
echo "while: $n"

classify() {
    case "$1" in
        *.txt) echo text ;;
        [0-9]*) echo number ;;
        *) echo other ;;
    esac
}
echo "case: $(classify a.txt) $(classify 42) $(classify x)"

cat <<END > "${TEMP:-/tmp}/gala_here.txt"
line one
line $((1 + 1))
END
echo "heredoc: $(wc -l < "${TEMP:-/tmp}/gala_here.txt" | tr -d ' ') lines, $(tail -n 1 "${TEMP:-/tmp}/gala_here.txt")"
rm "${TEMP:-/tmp}/gala_here.txt"

words=$(echo "a b c" | tr ' ' '\n' | sort -r | tr '\n' ',')
echo "subst: $words"

set -- x y z
echo "args: $# $2"

if false; then echo bad; elif true; then echo "if: elif"; fi

( exit 5 )
echo "subshell: $?"
echo "script: ok"
exit 4
