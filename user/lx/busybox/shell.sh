# busybox's shell and coreutils-style applets on gala's Linux personality
# (ROADMAP LX milestone 2): `busybox sh /lx/shell.sh` with PATH=/lx (the
# applets run as `busybox <applet>`: busybox.net's static build has no
# standalone shell, and there are no symlinks to name them by yet). One
# labelled line per check; the transcript is lx_busybox.txt (POSIX's and
# busybox's documented output; the same as dash with GNU coreutils
# prints for this script).
cd /lx || exit 1
busybox mkdir bb.dir && cd bb.dir || exit 2
echo "pwd: $(busybox pwd)"
echo "pipe: $(echo hello | busybox tr a-z A-Z)"
printf 'b\na\nc\n' > list
echo "sort: $(busybox sort list | busybox tr '\n' ' ')"
echo "sort -r: $(busybox sort -r < list | busybox tr '\n' ' ')"
echo "wc -l: $(busybox wc -l < list)"
echo "head -n 2: $(busybox head -n 2 list | busybox tr '\n' ' ')"
echo "tail -n 1: $(busybox tail -n 1 list)"
echo "grep -c: $(busybox grep -c a list)"
echo "sed: $(echo one two | busybox sed 's/two/three/')"
busybox cp list copy
echo "cmp: $(busybox cmp list copy && echo same)"
busybox mv copy moved
echo "ls: $(busybox ls | busybox tr '\n' ' ')"
busybox mkdir -p a/b/c
echo x > a/b/c/f
echo "find: $(busybox find a -type f)"
busybox rm -r a
echo "rm -r: $(busybox ls | busybox tr '\n' ' ')"
echo "append: $(echo one > app; echo two >> app; busybox cat app | busybox tr '\n' ' ')"
echo "test -d: $([ -d . ] && echo yes)"
echo "status of false: $(busybox false; echo $?)"
x=0
for i in 1 2 3; do x=$((x + i)); done
echo "arithmetic: $x"
echo "pipeline of three: $(busybox seq 1 1000 | busybox grep 7 | busybox wc -l)"
echo "more than a pipe holds: $(busybox seq 1 20000 | busybox tail -n 1)"
f() { echo "function $1"; }
echo "$(f arg)"
echo "here-document: $(busybox cat <<END
line
END
)"
case abc in a*) echo "case: a*" ;; *) echo "case: no" ;; esac
echo "basename: $(busybox basename /a/b/c.txt .txt)"
echo "dirname: $(busybox dirname /a/b/c.txt)"
echo "expr: $(busybox expr 6 \* 7)"
echo "md5sum: $(echo abc | busybox md5sum)"
echo "env: $(A=1 busybox env | busybox grep '^A=')"
echo "/dev/null: $(echo gone > /dev/null; busybox cat /dev/null | busybox wc -c)"
echo "stderr into the pipe: $(busybox ls missing 2>&1 | busybox wc -l)"
echo "xargs: $(echo 1 2 3 | busybox xargs busybox echo n)"
echo "sleep: $(busybox sleep 1 && echo slept)"
busybox touch t
echo "touch: $(busybox ls t)"
echo "stat -c %s: $(busybox stat -c %s list)"
echo "subshell exit: $( (exit 7); echo $?)"
echo "background and wait: $(busybox true & wait $!; echo $?)"
cd ..
busybox rm -r bb.dir
echo "cleaned up: $([ -e bb.dir ] && echo no || echo yes)"
echo "shell.sh: ok"
