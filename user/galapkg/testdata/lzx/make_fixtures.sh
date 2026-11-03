#!/bin/sh
# Regenerates galapkg's LZX cabinet fixtures and what independent
# readers make of them (docs/WORKFLOW.md "Installing software").
#
# No tool on Linux writes LZX cabinets, so the cabinets are made by the
# test encoder in src/lzx/encode.rs (written from [MS-PATCH] and the
# Cabinet SDK's LZX description, like the decoder); cabextract
# (libmspack) and 7-Zip then decode them, and their outputs' SHA-256
# are checked in under expected/ as the tests' expected values
# (THIRD_PARTY.md: tools, run as separate programs, nothing linked).
# The two must agree with each other and with the encoder's input.
#
# Run from anywhere: sh user/galapkg/testdata/lzx/make_fixtures.sh
set -eu
here=$(cd "$(dirname "$0")" && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

GALA_LZX_FIXTURES="$here" cargo test -q -p galapkg --lib lzx::tests::write_fixtures \
    -- --ignored >/dev/null

mkdir -p "$here/expected"
for cab in "$here"/*.cab; do
    stem=$(basename "$cab" .cab)
    mkdir -p "$work/c-$stem" "$work/z-$stem"
    cabextract -q -d "$work/c-$stem" "$cab"
    7z x -y -o"$work/z-$stem" "$cab" > /dev/null
    (cd "$work/c-$stem" && find . -type f -print0 | sort -z | xargs -0 sha256sum) \
        > "$here/expected/$stem.cabextract.sha256"
    (cd "$work/z-$stem" && find . -type f -print0 | sort -z | xargs -0 sha256sum) \
        > "$here/expected/$stem.7z.sha256"
    cmp "$here/expected/$stem.cabextract.sha256" "$here/expected/$stem.7z.sha256"
done
ls -l "$here"/*.cab
