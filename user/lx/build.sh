#!/bin/sh
# Builds gala's Linux test programs (user/lx/<name>_test/main.c) with musl
# into user/lx/bin: each as a static executable (<name>_test, ET_EXEC) and
# as a static-pie (<name>_test-pie, ET_DYN without an interpreter).
#
# Needs musl-gcc (Ubuntu/Debian: apt install musl-tools; musl 1.2.4 and
# gcc 13 on Ubuntu 24.04, which CI uses). The build is reproducible
# (stripped, no build id, no file paths), so CI rebuilds and checks the
# result against the checked-in binaries (`cargo xtask lx-expect`).
#
# Usage: user/lx/build.sh [out_dir]   (default user/lx/bin)
set -eu
here=$(cd "$(dirname "$0")" && pwd)
out=${1:-$here/bin}
mkdir -p "$out"
out=$(cd "$out" && pwd)
cc=${MUSL_GCC:-musl-gcc}
flags="-O2 -fno-stack-protector -fno-asynchronous-unwind-tables -fno-ident -Wall -Wextra -Werror -Wno-unused-function -Wl,--build-id=none -s"
cd "$here"
for dir in *_test; do
	[ -f "$dir/main.c" ] || continue
	"$cc" $flags -static -no-pie -o "$out/$dir" "$dir/main.c"
	"$cc" $flags -static-pie -fPIE -o "$out/$dir-pie" "$dir/main.c"
done
