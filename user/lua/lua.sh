# The `lua` scenario (ROADMAP H7), run by busybox sh: unpacks TinyCC and
# builds its runtime as tcc.sh does, unpacks the Lua 5.4.7 source release
# (user/bin/lua.tar), builds lua.exe and luac.exe with Lua's own
# src/Makefile under busybox's make and tcc, then runs test.lua (next to
# this script) with it.
# Usage: sh lua.sh path/to/tcc.tar path/to/lua.tar
set -e
here=$(dirname "$0")
w=$TEMP/gala_lua
rm -rf "$w"
mkdir -p "$w"
tar xf "$1" -C "$w"
tar xf "$2" -C "$w"
cd "$w/tcc"
for f in libtcc1.c crt1.c crt1w.c wincrt1.c wincrt1w.c dllcrt1.c dllmain.c \
    winex.c chkstk.S alloca.S alloca-bt.S stdatomic.c atomic.S builtin.c; do
    ./tcc.exe -B. -c "src/$f"
done
./tcc.exe -ar lib/libtcc1.a libtcc1.o crt1.o crt1w.o wincrt1.o wincrt1w.o \
    dllcrt1.o dllmain.o winex.o chkstk.o alloca.o alloca-bt.o stdatomic.o \
    atomic.o builtin.o
cd "$w/lua-5.4.7/src"
# Lua's makefile with its documented settings overridden: tcc for gcc,
# tcc -ar for ar, no ranlib, no libm (msvcrt has the math functions).
tcc=../../tcc/tcc.exe
make "CC=$tcc" "CFLAGS=-B../../tcc -O2 -DLUA_COMPAT_5_3" "AR=$tcc -ar rcs" \
    "RANLIB=:" "LIBS=" "LUA_T=lua.exe" "LUAC_T=luac.exe" lua.exe luac.exe \
    > "$w/make.log" 2>&1 || { cat "$w/make.log"; exit 1; }
echo "built: $(grep -c "tcc.exe" "$w/make.log") commands, $(ls lua.exe luac.exe | tr '\n' ' ')"
./lua.exe -v
./lua.exe "$here/test.lua"
echo "lua status=$?"
