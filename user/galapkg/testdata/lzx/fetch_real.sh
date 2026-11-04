#!/bin/sh
# Fetches a cabinet Microsoft made with LZX (the DirectX End-User Runtime
# web installer's embedded cabinet: LZX:21 with E8 translation, five
# files, 2.2 MB unpacked) into target/galapkg-fixtures, for the test
# lzx::tests::microsoft_cabinet_matches_cabextract (which skips when it
# is absent). The cabinet holds Microsoft's code, so it is not checked
# in; its files' SHA-256 as cabextract and 7-Zip extract them are, in
# expected/dxwebsetup.sha256. THIRD_PARTY.md has the provenance.
#
# Run from anywhere: sh user/galapkg/testdata/lzx/fetch_real.sh
set -eu
here=$(cd "$(dirname "$0")" && pwd)
out="$here/../../../../target/galapkg-fixtures"
mkdir -p "$out"
url=https://download.microsoft.com/download/1/7/1/1718CCC4-6315-4D8E-9543-8E28A4E18C4C/dxwebsetup.exe
exe_sha=2cf71d098c608c56e07f4655855a886c3102553f648df88458df616b26fd612f
cab_sha=82a3dba19cf203ccf41b637e81922d5400a9bf8b7629036d0d2cc11ed4271634
curl -sSfL -o "$out/dxwebsetup.exe" "$url"
echo "$exe_sha  $out/dxwebsetup.exe" | sha256sum -c -
# The cabinet is a resource of the executable: the first "MSCF" header,
# as long as its own size field says.
python3 -I -c '
import struct, sys
d = open(sys.argv[1], "rb").read()
i = d.find(b"MSCF\0\0\0\0")
n = struct.unpack_from("<I", d, i + 8)[0]
open(sys.argv[2], "wb").write(d[i:i + n])
' "$out/dxwebsetup.exe" "$out/dxwebsetup.cab"
echo "$cab_sha  $out/dxwebsetup.cab" | sha256sum -c -
rm -f "$out/dxwebsetup.exe"
