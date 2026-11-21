#!/bin/sh
# Builds user/bin/glob_test.exe from main.c with mingw-w64 against
# msvcrt, with mingw's _dowildcard set (main.c), as busybox-w32 is:
# reproducible (no timestamp, no debug info). Needs
# x86_64-w64-mingw32-gcc; without it keep the checked-in binary.
set -eu
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
x86_64-w64-mingw32-gcc -O1 -g0 -D__USE_MINGW_ANSI_STDIO=0 -s -Wl,--no-insert-timestamp \
    -o "$ROOT/user/bin/glob_test.exe" "$ROOT/user/glob_test/main.c"
sha256sum "$ROOT/user/bin/glob_test.exe"
