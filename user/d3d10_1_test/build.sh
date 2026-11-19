#!/bin/sh
# Builds user/bin/d3d10_1_test.exe from main.c with mingw-w64:
# reproducible (no timestamp, no debug info). Needs
# x86_64-w64-mingw32-gcc; without it keep the checked-in binary. The
# Direct3D 10.1, Direct2D and DXGI headers are mingw-w64's own, so
# nothing is fetched.
set -eu
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
x86_64-w64-mingw32-gcc -O1 -g0 -s -Wall -Wextra -Wl,--no-insert-timestamp \
    -o "$ROOT/user/bin/d3d10_1_test.exe" "$ROOT/user/d3d10_1_test/main.c" \
    -luser32 -ldxguid
sha256sum "$ROOT/user/bin/d3d10_1_test.exe"
