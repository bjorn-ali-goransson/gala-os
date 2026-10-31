# The `sqlite` scenario (ROADMAP H9), run by busybox sh: unpacks TinyCC
# and builds its runtime as tcc.sh does, unzips the SQLite amalgamation
# (user/bin/sqlite-amalgamation.zip, unmodified from sqlite.org),
# compiles it with tcc together with sqlrun.c (a small driver, next to
# this script), then runs test.sql on a new database file and reopen.sql
# on the same file from a second process.
# Usage: sh sqlite.sh path/to/tcc.tar path/to/sqlite-amalgamation.zip
set -e
here=$(dirname "$0")
w=$TEMP/gala_sqlite
rm -rf "$w"
mkdir -p "$w"
tar xf "$1" -C "$w"
unzip -q "$2" -d "$w"
cd "$w/tcc"
for f in libtcc1.c crt1.c crt1w.c wincrt1.c wincrt1w.c dllcrt1.c dllmain.c \
    winex.c chkstk.S alloca.S alloca-bt.S stdatomic.c atomic.S builtin.c; do
    ./tcc.exe -B. -c "src/$f"
done
./tcc.exe -ar lib/libtcc1.a libtcc1.o crt1.o crt1w.o wincrt1.o wincrt1w.o \
    dllcrt1.o dllmain.o winex.o chkstk.o alloca.o alloca-bt.o stdatomic.o \
    atomic.o builtin.o
cd "$w/sqlite-amalgamation-3540100"
# One thread (no locking between threads needed), no loadable extensions
# (they would need LoadLibrary of DLLs tcc did not build), and no
# compiler intrinsics (MSVC's __umulh on _WIN64; tcc has none).
# TinyCC's kernel32.def and headers predate functions SQLite calls: the
# imports come from the system's own kernel32.dll (tcc -impdef, as
# TinyCC's documentation suggests), the declarations from tcc_win.h.
../tcc/tcc.exe -impdef C:/Windows/System32/kernel32.dll -o kernel32.def
../tcc/tcc.exe -B../tcc -O2 -DSQLITE_THREADSAFE=0 -DSQLITE_OMIT_LOAD_EXTENSION \
    -DSQLITE_DISABLE_INTRINSIC \
    -include "$here/tcc_win.h" -I. -o sqlrun.exe "$here/sqlrun.c" sqlite3.c kernel32.def
echo "built: $(ls sqlrun.exe)"
echo "version: [$(./sqlrun.exe -version)]"
./sqlrun.exe "$w/test.db" "$here/test.sql"
echo "first session status=$?"
./sqlrun.exe "$w/test.db" "$here/reopen.sql"
echo "second session status=$?"
