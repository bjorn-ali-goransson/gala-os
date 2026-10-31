# The `tcc_make` scenario (ROADMAP H6), run by busybox sh: unpacks
# TinyCC as tcc.sh does and builds its runtime, then busybox's make
# builds the program in make/ (next to this script) with it, rebuilds
# only what a touched source needs, finds nothing to do, and stops at a
# failing rule. make echoes each command it runs.
# Usage: sh make.sh path/to/tcc.tar
set -e
here=$(dirname "$0")
w=$TEMP/gala_make
rm -rf "$w"
mkdir -p "$w/proj"
tar xf "$1" -C "$w"
cd "$w/tcc"
for f in libtcc1.c crt1.c crt1w.c wincrt1.c wincrt1w.c dllcrt1.c dllmain.c \
    winex.c chkstk.S alloca.S alloca-bt.S stdatomic.c atomic.S builtin.c; do
    ./tcc.exe -B. -c "src/$f"
done
./tcc.exe -ar lib/libtcc1.a libtcc1.o crt1.o crt1w.o wincrt1.o wincrt1w.o \
    dllcrt1.o dllmain.o winex.o chkstk.o alloca.o alloca-bt.o stdatomic.o \
    atomic.o builtin.o
cp "$here/make/Makefile" "$here/make/calc.h" "$here/make/main.c" \
    "$here/make/add.c" "$here/make/mul.c" "$w/proj/"
cd "$w/proj"
set -- CC=../tcc/tcc.exe CFLAGS=-B../tcc
# Each step's lines carry its name (make echoes the commands it runs).
make "$@" | sed 's/^/build: /'
./calc.exe
# Make compares modification times: a second later is newer anywhere.
sleep 1
touch add.c
out=$(make "$@")
echo "$out" | sed 's/^/after touch: /'
echo "after touch: $(echo "$out" | grep -c tcc.exe) commands"
make "$@" | sed 's/^/again: /'
set +e
make "$@" broken 2>&1 | sed 's/^/broken: /'
make "$@" broken >/dev/null 2>&1
echo "broken status=$?"
