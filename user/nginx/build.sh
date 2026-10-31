#!/bin/sh
# Builds user/bin/nginx.exe: Nginx for x64 Windows, cross-compiled with
# mingw-w64 (ROADMAP N2a). Reproducible: pinned source (version and
# SHA-256), a fixed build directory, no debug info, no link timestamp.
# Needs x86_64-w64-mingw32-gcc (Ubuntu: gcc-mingw-w64-x86-64) and curl.
# Without the toolchain, keep using the checked-in user/bin/nginx.exe.
#
# With zlib (gzip), PCRE2 (rewrite) and OpenSSL (TLS), built from pinned
# sources too. Nginx is BSD-2-Clause, zlib zlib, PCRE2 BSD-3-Clause,
# OpenSSL Apache-2.0; see THIRD_PARTY.md. OpenSSL makes the build take a
# few minutes.
set -eu

VERSION=1.30.6
SHA256=3cf46f5ed7504d460125866b7bea99fb357f31d887e2e63b35503310e9b8b6c8
ZLIB=1.3.2
ZLIB_SHA256=bb329a0a2cd0274d05519d61c667c062e06990d72e125ee2dfa8de64f0119d16
PCRE2=10.49
PCRE2_SHA256=929f0b20e62879252a15886b06c89f1edef61a363cbd5826fb041080a5e557ae
OPENSSL=3.5.9
OPENSSL_SHA256=603f5602e2eef00d77fbd429d34dcd5822bb301757a1bc9cdb24c670f1eb859a
# OpenSSL records its build date; a fixed one keeps the binary the same.
export SOURCE_DATE_EPOCH=1700000000
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
WORK=/tmp/gala-nginx-build

rm -rf "$WORK"
mkdir -p "$WORK"
cd "$WORK"
curl -sSfL -o nginx.tar.gz "https://nginx.org/download/nginx-$VERSION.tar.gz"
curl -sSfL -o zlib.tar.gz "https://zlib.net/zlib-$ZLIB.tar.gz"
curl -sSfL -o pcre2.tar.gz \
    "https://github.com/PCRE2Project/pcre2/releases/download/pcre2-$PCRE2/pcre2-$PCRE2.tar.gz"
curl -sSfL -o openssl.tar.gz \
    "https://github.com/openssl/openssl/releases/download/openssl-$OPENSSL/openssl-$OPENSSL.tar.gz"
sha256sum -c - <<SUMS
$SHA256  nginx.tar.gz
$ZLIB_SHA256  zlib.tar.gz
$PCRE2_SHA256  pcre2.tar.gz
$OPENSSL_SHA256  openssl.tar.gz
SUMS
for f in nginx zlib pcre2 openssl; do tar -xzf "$f.tar.gz"; done
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
    --with-zlib="../zlib-$ZLIB" \
    --with-pcre="../pcre2-$PCRE2" \
    --with-openssl="../openssl-$OPENSSL" \
    --with-http_ssl_module >/dev/null
# Nginx runs PCRE2's configure and OpenSSL's config as for a native build:
# tell them the target (PCRE2 would try to run Windows test programs here,
# OpenSSL would build for Linux).
sed -i \
    -e 's|\./configure --disable-shared|./configure --disable-shared --host=x86_64-w64-mingw32|' \
    -e 's|\./config --prefix=|./Configure mingw64 --cross-compile-prefix=x86_64-w64-mingw32- no-tests no-docs --prefix=|' \
    objs/Makefile
make -j"$(nproc)" >/dev/null
cp objs/nginx.exe "$ROOT/user/bin/nginx.exe"
sha256sum "$ROOT/user/bin/nginx.exe"
