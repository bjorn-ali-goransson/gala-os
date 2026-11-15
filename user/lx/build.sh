#!/bin/sh
# Builds gala's Linux test programs (user/lx/<name>_test/main.c) with musl
# into user/lx/bin: each as a static executable (<name>_test, ET_EXEC) and
# as a static-pie (<name>_test-pie, ET_DYN without an interpreter).
# The tests in GLIBC_TESTS are built instead with the host's gcc against
# glibc, dynamically: <name>_test (ET_EXEC with an interpreter) and
# <name>_test-pie (a PIE), each of their lib*.c a shared library
# lib*.so beside them (found through DT_RUNPATH $ORIGIN).
#
# Needs musl-gcc (Ubuntu/Debian: apt install musl-tools; musl 1.2.5 and
# gcc 15 on Ubuntu 26.04, which CI uses) and, for GLIBC_TESTS, gcc with
# glibc's headers (gcc 15 and glibc 2.43 on Ubuntu 26.04), and for
# gtk_test GTK 3's libraries (apt install libgtk-3-0t64). The build is reproducible
# (stripped, no build id, no file paths), so CI rebuilds and checks the
# result against the checked-in binaries (`cargo xtask lx-expect`) -- with
# the plain amd64 toolchain, which is what the check below is about.
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
GLIBC_TESTS="dyn_test gtk_test"
# Linked against GTK 3 as well (the host's Ubuntu 26.04 libgtk-3-0t64, its
# libraries by soname: no development package needed; the program
# declares what it calls). The binary depends only on the sonames and
# the symbols' names, so any 3.24 build gives the same bytes.
GTK_LIBS="-l:libgtk-3.so.0 -l:libgdk-3.so.0 -l:libgobject-2.0.so.0 -l:libglib-2.0.so.0"
flags="-O2 -fno-stack-protector -fno-asynchronous-unwind-tables -fno-ident -Wall -Wextra -Werror -Wno-unused-function -Wl,--build-id=none -s"
cd "$here"
# Ubuntu 26.04 publishes two builds of each toolchain package under the same
# version: the plain amd64 one and an amd64v3 one optimised for x86-64-v3
# (apt-cache policy names it "resolute/main amd64v3 Packages"; an image opts
# in with APT::Architecture-Variants, as GitHub's runner image does). The
# amd64v3 gcc defaults to -march=x86-64-v3 and emits AVX2 where the plain one
# emits none, so it cannot reproduce the checked-in binaries. Say which gcc
# this is, rather than leave a byte diff to explain itself.
for probe in "$gcc" "$cc"; do
	command -v "$probe" >/dev/null 2>&1 || continue
	march=$("$probe" -Q --help=target 2>/dev/null | awk '$1 == "-march=" { print $2; exit }')
	if [ -z "$march" ] || [ "$march" = "x86-64" ]; then
		continue
	fi
	echo "user/lx/build.sh: $probe defaults to -march=$march, not x86-64." >&2
	echo "  $("$probe" --version 2>/dev/null | head -1)" >&2
	echo "  This is Ubuntu's amd64v3 build; its binaries are not the" >&2
	echo "  checked-in ones. Build them in the test image instead:" >&2
	echo "    cargo xtask docker-test lx-expect" >&2
	echo "  or take the toolchain from the plain amd64 archive, which one" >&2
	echo "  apt option asks for whatever set the variant and wherever it did:" >&2
	echo "    plain='-o APT::Architecture-Variants='" >&2
	echo "    sudo apt-get \$plain update" >&2
	echo "    sudo apt-get install -y --reinstall --allow-downgrades \$plain \\" >&2
	echo "      gcc-15 cpp-15-x86-64-linux-gnu gcc-15-x86-64-linux-gnu \\" >&2
	echo "      libgcc-15-dev binutils libc6-dev" >&2
	echo "  as .github/workflows/ci.yml does." >&2
	exit 1
done
for dir in $GLIBC_TESTS; do
	libs=
	[ "$dir" = gtk_test ] && libs=$GTK_LIBS
	for src in "$dir"/lib*.c; do
		[ -f "$src" ] || continue
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
