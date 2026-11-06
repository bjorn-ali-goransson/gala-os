#!/bin/sh
# Builds user/bin/shell32_test.exe from main.c with mingw-w64:
# reproducible (no timestamp, no debug info), with two icon resources made
# by make_icons.py (python3) for the ExtractIcon checks. Needs
# x86_64-w64-mingw32-gcc and -windres; without them keep the checked-in
# binary.
set -eu
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT
python3 -I "$ROOT/user/shell32_test/make_icons.py" "$WORK"
printf '1 ICON "one.ico"\n2 ICON "two.ico"\n' > "$WORK/icons.rc"
(cd "$WORK" && x86_64-w64-mingw32-windres --preprocessor=cat -i icons.rc -O coff -o icons.o)
x86_64-w64-mingw32-gcc -O1 -g0 -s -Wl,--no-insert-timestamp \
    -o "$ROOT/user/bin/shell32_test.exe" "$ROOT/user/shell32_test/main.c" "$WORK/icons.o" \
    -lshell32 -luser32 -lgdi32 -lole32 -luuid
sha256sum "$ROOT/user/bin/shell32_test.exe"
