#!/bin/sh
# Fetches user/bin/sqlite-amalgamation.zip: the SQLite 3.54.1 amalgamation
# (sqlite3.c, sqlite3.h, sqlite3ext.h, shell.c; public domain,
# THIRD_PARTY.md), unmodified from sqlite.org and checked against the
# SHA3-256 published on its download page. The `sqlite` scenario unzips
# and compiles it on gala with TinyCC; nothing is compiled here.
# Needs curl and python3 (for SHA3-256).
set -eu

URL=https://www.sqlite.org/2026/sqlite-amalgamation-3540100.zip
SHA3=92281c86e2957f9b9305890f9ac04204caa7b677cc868dc326d942689df28e30
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
OUT="$ROOT/user/bin/sqlite-amalgamation.zip"

curl -sSf -o "$OUT.part" "$URL"
GOT=$(python3 -c 'import hashlib,sys; print(hashlib.sha3_256(open(sys.argv[1],"rb").read()).hexdigest())' "$OUT.part")
if [ "$GOT" != "$SHA3" ]; then
    rm -f "$OUT.part"
    echo "SHA3-256 mismatch: $GOT" >&2
    exit 1
fi
mv "$OUT.part" "$OUT"
echo "$SHA3  $OUT"
