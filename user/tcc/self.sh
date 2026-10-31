# The `tcc_self` scenario (ROADMAP H5), run by busybox sh: tcc.sh first
# (unpack, build the runtime, compile and run hello.c), then the tcc.exe
# built by mingw compiles TinyCC's own sources on gala into tcc2.exe,
# which compiles hello.c and gala's C runtime test (msvcrt_test's
# main.c) again; both run.
# Usage: sh self.sh path/to/tcc.tar path/to/msvcrt_test/main.c
set -e
here=$(dirname "$0")
sh "$here/tcc.sh" "$1"
cd "$TEMP/gala_tcc/tcc"
./tcc.exe -B. -DTCC_TARGET_PE -DTCC_TARGET_X86_64 -DONE_SOURCE=1 \
    -DCONFIG_TCC_PREDEFS=0 -o tcc2.exe src/tcc/tcc.c
echo "tcc2: $(./tcc2.exe -v)"
./tcc2.exe -B. -o hello2.exe "$here/hello.c"
set +e
./hello2.exe one two three
echo "status2=$?"
set -e
./tcc2.exe -B. -o msvcrt_test.exe "$2"
set +e
./msvcrt_test.exe
echo "msvcrt_test status=$?"
