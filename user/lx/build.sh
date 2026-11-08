#!/bin/sh
# Builds gala's Linux test programs (user/lx/<name>_test/main.c) with musl
# into user/lx/bin: each as a static executable (<name>_test, ET_EXEC) and
# as a static-pie (<name>_test-pie, ET_DYN without an interpreter).
# The tests in GLIBC_TESTS are built instead with the host's gcc against
# glibc, dynamically: <name>_test (ET_EXEC with an interpreter) and
# <name>_test-pie (a PIE), each of their lib*.c a shared library
# lib*.so beside them (found through DT_RUNPATH $ORIGIN).
#
# Needs musl-gcc (Ubuntu/Debian: apt install musl-tools; musl 1.2.4 and
# gcc 13 on Ubuntu 24.04, which CI uses) and, for GLIBC_TESTS, gcc with
# glibc's headers (gcc 13 and glibc 2.39 on Ubuntu 24.04). The build is reproducible
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
gcc=${GLIBC_GCC:-gcc}
# Linked against glibc, dynamically.
GLIBC_TESTS="dyn_test"
flags="-O2 -fno-stack-protector -fno-asynchronous-unwind-tables -fno-ident -Wall -Wextra -Werror -Wno-unused-function -Wl,--build-id=none -s"
cd "$here"
for dir in $GLIBC_TESTS; do
	libs=
	for src in "$dir"/lib*.c; do
		lib=$(basename "$src" .c)
		"$gcc" $flags -shared -fPIC -Wl,-soname,"$lib.so" -o "$out/$lib.so" "$src"
		libs="$libs -l${lib#lib}"
	done
	# $ORIGIN: the libraries are found beside the program, wherever it is.
	# --as-needed: a library only dlopen()ed (libdyn2) is not DT_NEEDED.
	link="-L$out -Wl,--as-needed $libs -Wl,-rpath,\$ORIGIN -Wl,--enable-new-dtags"
	"$gcc" $flags -no-pie -o "$out/$dir" "$dir/main.c" $link
	"$gcc" $flags -fPIE -pie -o "$out/$dir-pie" "$dir/main.c" $link
done
for dir in *_test; do
	[ -f "$dir/main.c" ] || continue
	case " $GLIBC_TESTS " in *" $dir "*) continue ;; esac
	"$cc" $flags -static -no-pie -o "$out/$dir" "$dir/main.c"
	"$cc" $flags -static-pie -fPIE -o "$out/$dir-pie" "$dir/main.c"
done
