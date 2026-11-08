#!/bin/sh
# Builds user/bin/gl_test.exe from main.c with mingw-w64: reproducible (no
# timestamp, no debug info). Needs x86_64-w64-mingw32-gcc; without it keep
# the checked-in binary.
set -eu
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
x86_64-w64-mingw32-gcc -O1 -g0 -s -Wl,--no-insert-timestamp \
    -o "$ROOT/user/bin/gl_test.exe" "$ROOT/user/gl_test/main.c" -lopengl32 -lgdi32 -luser32
sha256sum "$ROOT/user/bin/gl_test.exe"
