#!/bin/sh
# Builds gala's Linux test programs that have an aarch64 form
# (AARCH64_TESTS) for the aarch64 kernel (ROADMAP AR) into
# user/lx/bin/aarch64: static executables (ET_EXEC), each from
# user/lx/<name>/main.c, as build.sh builds the x86_64 ones; and the
# glibc ones (AARCH64_GLIBC_TESTS, below) against arm64 glibc.
#
# The toolchain: clang (--target=aarch64-linux-musl) and ld.lld, against
# musl 1.2.5 built here for aarch64 with that clang (its release tarball,
# checked against MUSL_SHA256, unpacked and built once into
# target/lx-aarch64-musl). The binaries depend on clang's version: they
# were built with Ubuntu clang 18.1.3 and LLD 18.1.3; CI does not rebuild
# them (its runner has another clang), so a change to a test's source
# means running this script and committing what it builds.
#
# Usage: user/lx/build-aarch64.sh [out_dir]   (default user/lx/bin/aarch64)
set -eu
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
out=${1:-$here/bin/aarch64}
mkdir -p "$out"
out=$(cd "$out" && pwd)
AARCH64_TESTS="altstack_test poll_test auxv_test epoll_test thread_test brk_test caps_test clock_test eventfd_test exec_test fdbits_test fdioctl_test fileio_test forkmem_test fs_test fsid_test hello_test id_test inotify_test memfd_test mmap_test pidfd_test pipe_test prctl_test proc_test procfs_test pty_test run_test seccomp_test shm_test socket_test statfs_test sysinfo_test timerfd_test unix_test wl_input_test wl_maximize_test wl_popup_test wl_test write_exit_test"
MUSL=musl-1.2.5
MUSL_SHA256=a9a118bbe84d8764da0ea0d28b3ab3fae8477fc7e4085d90102b8596fc7c75e4
cache=$root/target/lx-aarch64-musl
musl=$cache/install
if [ ! -f "$musl/lib/libc.a" ]; then
	mkdir -p "$cache"
	tarball=$cache/$MUSL.tar.gz
	[ -f "$tarball" ] || curl -sSfL -o "$tarball" "https://musl.libc.org/releases/$MUSL.tar.gz"
	echo "$MUSL_SHA256  $tarball" | sha256sum -c - >/dev/null
	rm -rf "$cache/src" "$cache/build"
	mkdir -p "$cache/src" "$cache/build"
	tar xzf "$tarball" -C "$cache/src"
	(cd "$cache/build" && "$cache/src/$MUSL/configure" --target=aarch64-linux-musl \
		--prefix="$musl" --disable-shared CC="clang --target=aarch64-linux-musl" \
		AR=llvm-ar RANLIB=llvm-ranlib CFLAGS=-O2 >/dev/null &&
		make -j"$(nproc)" >/dev/null && make install >/dev/null)
fi
# build.sh's flags; clang also warns of an unused static volatile, which
# gcc does not (thread_test has one): not an error here either.
flags="-O2 -fno-stack-protector -fno-asynchronous-unwind-tables -fno-ident -Wall -Wextra -Werror -Wno-unused-function -Wno-unused-variable"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
for t in $AARCH64_TESTS; do
	clang --target=aarch64-linux-musl $flags -nostdinc -isystem "$musl/include" \
		-c "$here/$t/main.c" -o "$tmp/$t.o"
	ld.lld -static --build-id=none -s -o "$out/$t" "$musl/lib/crt1.o" "$musl/lib/crti.o" \
		"$tmp/$t.o" "$musl/lib/libc.a" "$musl/lib/crtn.o"
done
# The glibc tests (build.sh's GLIBC_TESTS that have an aarch64 form):
# linked dynamically against Ubuntu 26.04's arm64 glibc 2.43, the one
# the images ship (xtask's glibc-arm64), through a sysroot of the
# release's arm64 packages from ports.ubuntu.com, each checked against
# the SHA-256 in its signed Packages index and unpacked once into
# target/lx-aarch64-glibc (/lib is /usr/lib, as on Ubuntu).
AARCH64_GLIBC_TESTS="dyn_test"
gcache=$root/target/lx-aarch64-glibc
sysroot=$gcache/sysroot
if [ ! -f "$sysroot/usr/lib/aarch64-linux-gnu/libc.so.6" ]; then
	mkdir -p "$gcache/debs"
	rm -rf "$sysroot"
	mkdir -p "$sysroot"
	while read -r path sha; do
		deb=$gcache/debs/$(basename "$path")
		[ -f "$deb" ] || curl -sSfL -o "$deb" "https://ports.ubuntu.com/ubuntu-ports/$path"
		echo "$sha  $deb" | sha256sum -c - >/dev/null
		dpkg-deb -x "$deb" "$sysroot"
	done <<-EOF
	pool/main/g/glibc/libc6_2.43-2ubuntu2_arm64.deb 6be782d90425acbdf9b7523f791a69a0d3f8770be0750b5cce71d4cdc18397c0
	pool/main/g/glibc/libc6-dev_2.43-2ubuntu2_arm64.deb 26060610ffbcb3e7260d113e659dbedfe0bacd5dd116affa3bde72431184d328
	pool/main/g/gcc-15/libgcc-15-dev_15.2.0-16ubuntu1_arm64.deb d088c97ff459ab243eb369666dfc43b92f278556e45d72c2def4d7657d728fe4
	pool/main/g/gcc-16/libgcc-s1_16-20260322-1ubuntu1_arm64.deb 106e53a42bebdc55f253d737a818991eba5b4f22c8b1a035a4202b133ade9743
	pool/main/l/linux/linux-libc-dev_7.0.0-14.14_arm64.deb e86f6bbf7bebdc1958241b546ca9467c35fc29f57c01d4f0336c1908ca21f2fe
	EOF
	ln -s usr/lib "$sysroot/lib"
fi
gcc_glibc="clang --target=aarch64-linux-gnu --sysroot=$sysroot --gcc-toolchain=$sysroot/usr -fuse-ld=lld -Wl,--build-id=none"
for dir in $AARCH64_GLIBC_TESTS; do
	libs=
	for src in "$here/$dir"/lib*.c; do
		[ -f "$src" ] || continue
		lib=$(basename "$src" .c)
		$gcc_glibc $flags -shared -fPIC -Wl,-soname,"$lib.so" -o "$out/$lib.so" "$src"
		libs="$libs -l${lib#lib}"
	done
	# As build.sh: $ORIGIN, and --as-needed (libdyn2 is only dlopen()ed).
	link="-L$out -Wl,--as-needed $libs -Wl,-rpath,\$ORIGIN -Wl,--enable-new-dtags"
	$gcc_glibc $flags -no-pie -o "$out/$dir" "$here/$dir/main.c" $link
	$gcc_glibc $flags -fPIE -pie -o "$out/$dir-pie" "$here/$dir/main.c" $link
done
echo "built $AARCH64_TESTS $AARCH64_GLIBC_TESTS into $out"
