#!/bin/sh
# Builds user/bin/shlwapi_test.exe (x64) and wow64_shlwapi_test.exe (x86,
# the same source through WOW64) from main.c with mingw-w64:
# reproducible (no timestamp, no debug info). Needs x86_64-w64-mingw32-gcc
# and i686-w64-mingw32-gcc; without them keep the checked-in binaries.
set -eu
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
x86_64-w64-mingw32-gcc -O1 -g0 -s -Wl,--no-insert-timestamp \
    -o "$ROOT/user/bin/shlwapi_test.exe" "$ROOT/user/shlwapi_test/main.c" -lshlwapi -ladvapi32 -lole32
i686-w64-mingw32-gcc -O1 -g0 -s -Wl,--no-insert-timestamp -DNAME='"wow64_shlwapi_test"' \
    -o "$ROOT/user/bin/wow64_shlwapi_test.exe" "$ROOT/user/shlwapi_test/main.c" -lshlwapi -ladvapi32 -lole32
sha256sum "$ROOT/user/bin/shlwapi_test.exe" "$ROOT/user/bin/wow64_shlwapi_test.exe"
