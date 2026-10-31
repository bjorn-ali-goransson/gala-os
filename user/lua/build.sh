#!/bin/sh
# Builds user/bin/lua.tar: the unmodified Lua 5.4.7 source release
# (lua.org, MIT, THIRD_PARTY.md) repacked as a plain tar with sorted
# names, fixed times and owners, for the `lua` scenario, which builds it
# on gala with busybox's make and TinyCC. Nothing is compiled here.
# Needs curl, sha256sum and GNU tar.
set -eu

VERSION=5.4.7
# Published on https://www.lua.org/ftp/ next to the tarball.
SHA256=9fbf5e28ef86c69858f6d3d34eccc32e911c1a28b4120ff3e84aaa70cfbf1e30
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
WORK=/tmp/gala-lua-build

rm -rf "$WORK"
mkdir -p "$WORK"
curl -sSf -o "$WORK/lua.tar.gz" "https://www.lua.org/ftp/lua-$VERSION.tar.gz"
echo "$SHA256  $WORK/lua.tar.gz" | sha256sum -c -
tar -C "$WORK" -xzf "$WORK/lua.tar.gz"
tar -C "$WORK" --sort=name --mtime=@1700000000 --owner=0 --group=0 \
    --numeric-owner --format=ustar -cf "$ROOT/user/bin/lua.tar" "lua-$VERSION"
sha256sum "$ROOT/user/bin/lua.tar"
