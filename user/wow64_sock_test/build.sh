#!/bin/sh
# Builds user/bin/wow64_sock_test.exe from main.c with mingw-w64's i686
# compiler (gcc-mingw-w64-i686-win32): a 32-bit (x86) program against
# ws2_32 (msvcrt for printf), reproducible (no timestamp, no debug
# information). Without the compiler keep the checked-in binary.
set -eu
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
i686-w64-mingw32-gcc -O1 -g0 -D__USE_MINGW_ANSI_STDIO=0 -Wall -Wno-format -s \
    -Wl,--no-insert-timestamp \
    -o "$ROOT/user/bin/wow64_sock_test.exe" "$ROOT/user/wow64_sock_test/main.c" -lws2_32
sha256sum "$ROOT/user/bin/wow64_sock_test.exe"
