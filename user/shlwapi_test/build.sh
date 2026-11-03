#!/bin/sh
# Builds user/bin/shlwapi_test.exe from main.c with mingw-w64:
# reproducible (no timestamp, no debug info). Needs x86_64-w64-mingw32-gcc;
# without it keep the checked-in binary.
set -eu
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
x86_64-w64-mingw32-gcc -O1 -g0 -s -Wl,--no-insert-timestamp \
    -o "$ROOT/user/bin/shlwapi_test.exe" "$ROOT/user/shlwapi_test/main.c" -lshlwapi -ladvapi32
sha256sum "$ROOT/user/bin/shlwapi_test.exe"
