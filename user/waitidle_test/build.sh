#!/bin/sh
# Builds user/bin/waitidle_test.exe (x64) and wow64_waitidle_test.exe (x86,
# the same source through WOW64) from main.c with mingw-w64, and from that
# same source the GUI-subsystem child each of them spawns
# (waitidle_child.exe, wow64_waitidle_child.exe): one source, two
# subsystems, which is the measurement itself -- what WaitForInputIdle
# answers from is the subsystem byte in the PE header and nothing else, so
# the test spawns one binary as a GUI program and the other (itself) as a
# console one and compares the two answers. Reproducible (no timestamp, no
# debug info). Needs x86_64-w64-mingw32-gcc and i686-w64-mingw32-gcc;
# without them keep the checked-in binaries.
set -eu
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
SRC="$ROOT/user/waitidle_test/main.c"
BIN="$ROOT/user/bin"
x86_64-w64-mingw32-gcc -O1 -g0 -s -Wl,--no-insert-timestamp \
    -o "$BIN/waitidle_test.exe" "$SRC" -luser32
i686-w64-mingw32-gcc -O1 -g0 -s -Wl,--no-insert-timestamp \
    -DNAME='"wow64_waitidle_test"' -DCHILD='L"wow64_waitidle_child.exe"' \
    -o "$BIN/wow64_waitidle_test.exe" "$SRC" -luser32
# The child: the same source, linked as a GUI-subsystem program
# (-mwindows). It takes its mode from argv[1] and never reaches the
# parent's checks.
x86_64-w64-mingw32-gcc -O1 -g0 -s -Wl,--no-insert-timestamp -mwindows \
    -DNAME='"waitidle_child"' \
    -o "$BIN/waitidle_child.exe" "$SRC" -luser32
i686-w64-mingw32-gcc -O1 -g0 -s -Wl,--no-insert-timestamp -mwindows \
    -DNAME='"wow64_waitidle_child"' \
    -o "$BIN/wow64_waitidle_child.exe" "$SRC" -luser32
sha256sum "$BIN/waitidle_test.exe" "$BIN/wow64_waitidle_test.exe" \
    "$BIN/waitidle_child.exe" "$BIN/wow64_waitidle_child.exe"
