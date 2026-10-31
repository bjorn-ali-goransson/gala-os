#!/bin/sh
# Builds user/bin/nginx.exe: Nginx for x64 Windows, cross-compiled with
# mingw-w64 (ROADMAP N2a). Reproducible: pinned source (version and
# SHA-256), a fixed build directory, no debug info, no link timestamp.
# Needs x86_64-w64-mingw32-gcc (Ubuntu: gcc-mingw-w64-x86-64) and curl.
# Without the toolchain, keep using the checked-in user/bin/nginx.exe.
#
# Minimal configuration: no PCRE (rewrite module), zlib (gzip) or
# OpenSSL yet. Nginx is BSD-2-Clause; see THIRD_PARTY.md.
set -eu

VERSION=1.30.6
SHA256=3cf46f5ed7504d460125866b7bea99fb357f31d887e2e63b35503310e9b8b6c8
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
WORK=/tmp/gala-nginx-build

rm -rf "$WORK"
mkdir -p "$WORK"
cd "$WORK"
curl -sSfL -o nginx.tar.gz "https://nginx.org/download/nginx-$VERSION.tar.gz"
echo "$SHA256  nginx.tar.gz" | sha256sum -c -
tar -xzf nginx.tar.gz
cd "nginx-$VERSION"

./configure \
    --crossbuild=win32 \
    --with-cc=x86_64-w64-mingw32-gcc \
    --with-cc-opt='-DFD_SETSIZE=1024 -g0' \
    --with-ld-opt='-s -Wl,--no-insert-timestamp' \
    --prefix= \
    --sbin-path=nginx.exe \
    --conf-path=conf/nginx.conf \
    --pid-path=logs/nginx.pid \
    --http-log-path=logs/access.log \
    --error-log-path=logs/error.log \
    --http-client-body-temp-path=temp/client_body_temp \
    --http-proxy-temp-path=temp/proxy_temp \
    --http-fastcgi-temp-path=temp/fastcgi_temp \
    --http-scgi-temp-path=temp/scgi_temp \
    --http-uwsgi-temp-path=temp/uwsgi_temp \
    --without-http_rewrite_module \
    --without-http_gzip_module \
    --without-pcre >/dev/null
make -j"$(nproc)" >/dev/null
cp objs/nginx.exe "$ROOT/user/bin/nginx.exe"
sha256sum "$ROOT/user/bin/nginx.exe"
