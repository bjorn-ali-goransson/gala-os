# The `tcc` scenario (ROADMAP H4), run by busybox sh: unpacks TinyCC
# (user/bin/tcc.tar, from user/tcc/build.sh), builds its runtime library
# with it, compiles hello.c (next to this script) and runs the result.
# Usage: sh tcc.sh path/to/tcc.tar
set -e
here=$(dirname "$0")
w=$TEMP/gala_tcc
rm -rf "$w"
mkdir -p "$w"
tar xf "$1" -C "$w"
cd "$w/tcc"
echo "unpacked=$(ls include/stdio.h)"
./tcc.exe -v
# libtcc1.a: as win32/build-tcc.bat makes it.
for f in libtcc1.c crt1.c crt1w.c wincrt1.c wincrt1w.c dllcrt1.c dllmain.c \
    winex.c chkstk.S alloca.S alloca-bt.S stdatomic.c atomic.S builtin.c; do
    ./tcc.exe -B. -c "src/$f"
done
./tcc.exe -ar lib/libtcc1.a libtcc1.o crt1.o crt1w.o wincrt1.o wincrt1w.o \
    dllcrt1.o dllmain.o winex.o chkstk.o alloca.o alloca-bt.o stdatomic.o \
    atomic.o builtin.o
echo "libtcc1=$(ls lib/libtcc1.a)"
./tcc.exe -B. -o "$w/hello.exe" "$here/hello.c"
echo "compiled=$(ls "$w/hello.exe" | sed 's|.*/||')"
set +e
"$w/hello.exe" one two three
echo "status=$?"
