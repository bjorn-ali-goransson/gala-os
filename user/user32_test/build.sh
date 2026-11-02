#!/bin/sh
# Builds user/bin/user32_test.exe from main.c and resources.rc with
# mingw-w64: reproducible (no timestamp, no debug info). Needs
# x86_64-w64-mingw32-gcc and -windres; without them keep the checked-in
# binary.
set -eu
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
DIR="$ROOT/user/user32_test"
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
x86_64-w64-mingw32-windres --include-dir "$DIR" -i "$DIR/resources.rc" -o "$TMP/resources.o"
x86_64-w64-mingw32-gcc -O1 -g0 -s -Wl,--no-insert-timestamp \
    -o "$ROOT/user/bin/user32_test.exe" "$DIR/main.c" "$TMP/resources.o" -luser32 -lgdi32 -lcomctl32 -lole32 -lshell32 -ladvapi32 -luxtheme -lmpr
sha256sum "$ROOT/user/bin/user32_test.exe"
