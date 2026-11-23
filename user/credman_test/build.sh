#!/bin/sh
# Builds user/bin/credman_test.exe (x64) and wow64_credman_test.exe (x86,
# the same source through WOW64) from main.c with mingw-w64: reproducible
# (no timestamp, no debug info). Needs x86_64-w64-mingw32-gcc and
# i686-w64-mingw32-gcc; without them keep the checked-in binaries.
set -eu
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
x86_64-w64-mingw32-gcc -O1 -g0 -s -Wl,--no-insert-timestamp \
    -o "$ROOT/user/bin/credman_test.exe" "$ROOT/user/credman_test/main.c" -ladvapi32
i686-w64-mingw32-gcc -O1 -g0 -s -Wl,--no-insert-timestamp -DNAME='"wow64_credman_test"' \
    -o "$ROOT/user/bin/wow64_credman_test.exe" "$ROOT/user/credman_test/main.c" -ladvapi32
sha256sum "$ROOT/user/bin/credman_test.exe" "$ROOT/user/bin/wow64_credman_test.exe"
