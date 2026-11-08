# busybox's shell and coreutils-style applets on gala's Linux personality
# (ROADMAP LX milestone 2): `busybox sh /lx/shell.sh` with PATH=/lx. The
# applets are called by their own names, through symbolic links to
# busybox in /lx/bb.bin (busybox picks the applet from argv[0];
# busybox.net's static build has no standalone shell to find them
# itself). One labelled line per check; the transcript is lx_busybox.txt
# (POSIX's and busybox's documented output; the same as dash with GNU
# coreutils prints for this script).
cd /lx || exit 1
busybox mkdir bb.bin || exit 2
for a in basename cat cmp cp dirname echo env expr find grep head ln ls md5sum mkdir mv pwd \
	readlink rm sed seq sleep sort stat tail touch tr true wc xargs; do
	busybox ln -s /lx/busybox bb.bin/$a || exit 3
done
PATH=/lx/bb.bin:$PATH
mkdir bb.dir && cd bb.dir || exit 4
echo "pwd: $(pwd)"
echo "pipe: $(echo hello | tr a-z A-Z)"
printf 'b\na\nc\n' > list
echo "sort: $(sort list | tr '\n' ' ')"
echo "sort -r: $(sort -r < list | tr '\n' ' ')"
echo "wc -l: $(wc -l < list)"
echo "head -n 2: $(head -n 2 list | tr '\n' ' ')"
echo "tail -n 1: $(tail -n 1 list)"
echo "grep -c: $(grep -c a list)"
echo "sed: $(echo one two | sed 's/two/three/')"
cp list copy
echo "cmp: $(cmp list copy && echo same)"
mv copy moved
echo "ls: $(ls | tr '\n' ' ')"
mkdir -p a/b/c
echo x > a/b/c/f
echo "find: $(find a -type f)"
rm -r a
echo "rm -r: $(ls | tr '\n' ' ')"
echo "append: $(echo one > app; echo two >> app; cat app | tr '\n' ' ')"
echo "test -d: $([ -d . ] && echo yes)"
echo "status of false: $(false; echo $?)"
x=0
for i in 1 2 3; do x=$((x + i)); done
echo "arithmetic: $x"
echo "pipeline of three: $(seq 1 1000 | grep 7 | wc -l)"
echo "more than a pipe holds: $(seq 1 20000 | tail -n 1)"
f() { echo "function $1"; }
echo "$(f arg)"
echo "here-document: $(cat <<END
line
END
)"
case abc in a*) echo "case: a*" ;; *) echo "case: no" ;; esac
echo "basename: $(basename /a/b/c.txt .txt)"
echo "dirname: $(dirname /a/b/c.txt)"
echo "expr: $(expr 6 \* 7)"
echo "md5sum: $(echo abc | md5sum)"
echo "env: $(A=1 env | grep '^A=')"
echo "/dev/null: $(echo gone > /dev/null; cat /dev/null | wc -c)"
echo "stderr into the pipe: $(ls missing 2>&1 | wc -l)"
echo "xargs: $(echo 1 2 3 | xargs echo n)"
echo "sleep: $(sleep 1 && echo slept)"
touch t
echo "touch: $(ls t)"
ln -s list sl
echo "ln -s, readlink: $(readlink sl)"
echo "through the link: $(cat sl | wc -l)"
ln list hl
echo "ln, stat -c %h: $(stat -c %h list)"
echo "stat -c %s: $(stat -c %s list)"
echo "subshell exit: $( (exit 7); echo $?)"
echo "background and wait: $(true & wait $!; echo $?)"
cd ..
rm -r bb.dir bb.bin
echo "cleaned up: $([ -e bb.dir ] || [ -e bb.bin ] && echo no || echo yes)"
echo "shell.sh: ok"
