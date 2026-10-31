#!/bin/sh
# Builds user/bin/busybox.exe: busybox-w32 (sh, coreutils, grep, sed, ...
# in one executable) for x64 Windows, cross-compiled with mingw-w64
# (ROADMAP H2). Reproducible: a pinned release (tag and commit checked),
# a fixed build directory, no debug info, no link timestamp, a fixed
# build date. Needs x86_64-w64-mingw32-gcc (Ubuntu: gcc-mingw-w64-x86-64)
# and git. Without them, keep using the checked-in user/bin/busybox.exe.
#
# busybox-w32 is GPL-2.0 (THIRD_PARTY.md): the source is the pinned
# commit of https://github.com/rmyorston/busybox-w32, the maintainer's
# repository (releases are published at https://frippery.org/busybox/).
set -eu

TAG=FRP-6354-g5f09fb446
COMMIT=5f09fb446adf81d005f0aad9dc47eca19ac3b74c
export SOURCE_DATE_EPOCH=1700000000
export KBUILD_BUILD_TIMESTAMP='@1700000000'
export KBUILD_BUILD_USER=gala KBUILD_BUILD_HOST=gala
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
WORK=/tmp/gala-busybox-build

rm -rf "$WORK"
git clone -q --depth 1 --branch "$TAG" https://github.com/rmyorston/busybox-w32 "$WORK"
cd "$WORK"
test "$(git rev-parse HEAD)" = "$COMMIT"

make -s mingw64_defconfig
make -s -j"$(nproc)" CROSS_COMPILE=x86_64-w64-mingw32- \
    EXTRA_CFLAGS='-g0' EXTRA_LDFLAGS='-s -Wl,--no-insert-timestamp'
cp busybox.exe "$ROOT/user/bin/busybox.exe"
sha256sum "$ROOT/user/bin/busybox.exe"
