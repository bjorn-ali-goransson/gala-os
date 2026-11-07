#!/bin/sh
# Builds user/bin/uia_test.exe from main.c with mingw-w64: reproducible
# (no timestamp, no debug info). mingw-w64 ships no UIAutomationCore
# import library, so one is made from uiautomationcore.def (the provider
# functions the test calls). Needs x86_64-w64-mingw32-gcc and -dlltool;
# without them keep the checked-in binary.
set -eu
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
x86_64-w64-mingw32-dlltool -d "$ROOT/user/uia_test/uiautomationcore.def" -l "$TMP/libuiautomationcore.a"
x86_64-w64-mingw32-gcc -O1 -g0 -s -Wl,--no-insert-timestamp \
    -o "$ROOT/user/bin/uia_test.exe" "$ROOT/user/uia_test/main.c" -L"$TMP" \
    -luser32 -lole32 -loleaut32 -lcomctl32 -luiautomationcore -luuid
sha256sum "$ROOT/user/bin/uia_test.exe"
