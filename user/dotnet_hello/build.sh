#!/bin/sh
# Builds user/bin/dotnet_hello.dll from Hello.cs with Mono's C# compiler
# (mcs, Debian/Ubuntu package mono-mcs): a portable IL assembly that
# references mscorlib, whose types .NET 8 forwards to its own libraries.
# The `dotnet_hello` scenario runs it with gala's C:\dotnet\dotnet.exe
# (hello.runtimeconfig.json names the framework). The assembly's MVID and
# PE timestamp are pinned so the build is reproducible.
set -eu

ROOT=$(cd "$(dirname "$0")/../.." && pwd)
DIR="$ROOT/user/dotnet_hello"
OUT="$ROOT/user/bin/dotnet_hello.dll"
NATIVE="$ROOT/user/kernel32_clr_test/x86_native.dll"

mcs -nologo -optimize+ -debug- -target:exe -out:"$OUT" "$DIR/Hello.cs"
python3 -I - "$OUT" "$NATIVE" <<'PY'
import struct, sys
path = sys.argv[1]
b = bytearray(open(path, 'rb').read())
pe = struct.unpack_from('<I', b, 0x3c)[0]
struct.pack_into('<I', b, pe + 8, 0)  # TimeDateStamp
sections = struct.unpack_from('<H', b, pe + 6)[0]
opt = pe + 24
table = opt + struct.unpack_from('<H', b, pe + 20)[0]
magic = struct.unpack_from('<H', b, opt)[0]
dirs = opt + (96 if magic == 0x10b else 112)
def offset(rva):
    for i in range(sections):
        s = table + 40 * i
        size, va, _, raw = struct.unpack_from('<IIII', b, s + 8)
        if va <= rva < va + size:
            return raw + rva - va
    raise SystemExit('rva outside the sections')
cli = offset(struct.unpack_from('<I', b, dirs + 14 * 8)[0])  # the CLI header
meta = offset(struct.unpack_from('<I', b, cli + 8)[0])
version_len = struct.unpack_from('<I', b, meta + 12)[0]
at = meta + 16 + version_len + 2
streams = struct.unpack_from('<H', b, at)[0]
at += 2
for _ in range(streams):
    off, size = struct.unpack_from('<II', b, at)
    end = b.index(0, at + 8)
    name = b[at + 8:end].decode()
    at = (end + 4) & ~3
    if name == '#GUID':
        # The MVID: the module's only GUID (index 1).
        b[meta + off:meta + off + 16] = bytes.fromhex('6761' + '6c61' * 7)
open(path, 'wb').write(b)
# A 32-bit native (not .NET) image for kernel32_clr_test: the same file
# without its CLI header directory (data directory 14).
struct.pack_into('<II', b, dirs + 14 * 8, 0, 0)
open(sys.argv[2], 'wb').write(b)
PY
sha256sum "$OUT" "$NATIVE"
