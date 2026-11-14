#!/bin/sh
# Builds user/bin/wininet_test.exe (x64) and wow64_wininet_test.exe (x86,
# the same source through WOW64) from main.c with mingw-w64:
# reproducible (no timestamp, no debug info). Needs x86_64-w64-mingw32-gcc
# and i686-w64-mingw32-gcc; without them keep the checked-in binaries.
set -eu
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
x86_64-w64-mingw32-gcc -O1 -g0 -s -Wl,--no-insert-timestamp \
    -o "$ROOT/user/bin/wininet_test.exe" "$ROOT/user/wininet_test/main.c" -lwininet
i686-w64-mingw32-gcc -O1 -g0 -s -Wl,--no-insert-timestamp -DNAME='"wow64_wininet_test"' \
    -o "$ROOT/user/bin/wow64_wininet_test.exe" "$ROOT/user/wininet_test/main.c" -lwininet
sha256sum "$ROOT/user/bin/wininet_test.exe" "$ROOT/user/bin/wow64_wininet_test.exe"
