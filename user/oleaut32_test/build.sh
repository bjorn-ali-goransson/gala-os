#!/bin/sh
# Builds user/bin/oleaut32_test.exe from main.c with mingw-w64:
# reproducible (no timestamp, no debug info). Needs x86_64-w64-mingw32-gcc;
# without it keep the checked-in binary.
set -eu
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
x86_64-w64-mingw32-gcc -O1 -g0 -s -Wl,--no-insert-timestamp \
    -o "$ROOT/user/bin/oleaut32_test.exe" "$ROOT/user/oleaut32_test/main.c" -loleaut32 -lole32
sha256sum "$ROOT/user/bin/oleaut32_test.exe"
