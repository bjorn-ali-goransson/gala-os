#!/bin/sh
# Builds user/bin/comctl32_test.exe from main.c with mingw-w64:
# reproducible (no timestamp, no debug info). Needs x86_64-w64-mingw32-gcc;
# without it keep the checked-in binary.
set -eu
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
x86_64-w64-mingw32-gcc -O1 -g0 -s -Wl,--no-insert-timestamp \
    -o "$ROOT/user/bin/comctl32_test.exe" "$ROOT/user/comctl32_test/main.c" -lcomctl32 -lgdi32 -luser32
sha256sum "$ROOT/user/bin/comctl32_test.exe"
