#!/bin/sh
# Builds flaui_test's programs into user/bin:
# - flaui_test.dll from FlaUITest.cs and filezilla_flaui.dll from
#   FileZillaFlaUI.cs (the filezilla_flaui scenario's test) with Mono's C# compiler (mcs, package
#   mono-mcs), compiled against .NET 8's reference assemblies (the
#   Microsoft.NETCore.App.Ref package, fetched here and checked against
#   its SHA-256; mcs cannot read the runtime's own System.Private.CoreLib,
#   whose Span has ref fields) and FlaUI.Core, FlaUI.UIA3 and
#   Interop.UIAutomationClient, which xtask fetches (PREBUILT flaui-core,
#   flaui-uia3, uiautomation-interop): run `cargo xtask boot-test
#   flaui_test` once first. All of them are only read as metadata here,
#   never run on the host. The MVID and PE timestamp are pinned, as in
#   user/dotnet_hello/build.sh, so the build is reproducible.
# - flaui_target.exe from target.c with mingw-w64 (no timestamp, no debug
#   information).
set -eu

ROOT=$(cd "$(dirname "$0")/../.." && pwd)
DIR="$ROOT/user/flaui_test"
PRE="$ROOT/target/prebuilt"
OUT="$ROOT/user/bin/flaui_test.dll"
REF_VERSION=8.0.33
REF_SHA256=154d7e2ad85d731061a153f6dc0b14a1f3d05de0790547571f6abf3c556e642e
REF="$PRE/dotnet-ref/ref/net8.0"
if [ ! -f "$REF/System.Runtime.dll" ]; then
    mkdir -p "$PRE/dotnet-ref"
    pkg="$PRE/dotnet-ref/ref.nupkg"
    curl -sSfL -o "$pkg" \
        "https://api.nuget.org/v3-flatcontainer/microsoft.netcore.app.ref/$REF_VERSION/microsoft.netcore.app.ref.$REF_VERSION.nupkg"
    echo "$REF_SHA256  $pkg" | sha256sum -c -
    unzip -q -o "$pkg" 'ref/net8.0/*.dll' LICENSE.TXT -d "$PRE/dotnet-ref"
    rm -f "$pkg"
fi
FLAUI_CORE="$PRE/flaui-core/lib/net8.0-windows7.0/FlaUI.Core.dll"
FLAUI_UIA3="$PRE/flaui-uia3/lib/net8.0-windows7.0/FlaUI.UIA3.dll"
INTEROP="$PRE/uiautomation-interop/lib/netcoreapp3.0/Interop.UIAutomationClient.dll"
for f in "$FLAUI_CORE" "$FLAUI_UIA3" "$INTEROP"; do
    [ -f "$f" ] || { echo "missing $f (run cargo xtask boot-test flaui_test first)" >&2; exit 1; }
done

refs=""
for a in System.Runtime System.Console System.Collections System.ComponentModel.Primitives \
    System.Diagnostics.Process System.Drawing.Primitives System.Linq \
    System.Runtime.InteropServices System.Threading System.Threading.Thread \
    System.Collections.Concurrent netstandard; do
    refs="$refs -r:$REF/$a.dll"
done
# build <output> <source>: one test assembly.
build() {
# shellcheck disable=SC2086
mcs -nologo -nowarn:1702 -nostdlib -noconfig -optimize+ -debug- -target:exe -out:"$1" $refs \
    -r:"$FLAUI_CORE" -r:"$FLAUI_UIA3" -r:"$INTEROP" "$2"
python3 -I - "$1" <<'PY'
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
        b[meta + off:meta + off + 16] = bytes.fromhex('6761' + '6c61' * 6 + '6655')
open(path, 'wb').write(b)
PY
}
build "$OUT" "$DIR/FlaUITest.cs"
build "$ROOT/user/bin/filezilla_flaui.dll" "$DIR/FileZillaFlaUI.cs"
x86_64-w64-mingw32-gcc -O1 -g0 -s -Wl,--no-insert-timestamp \
    -o "$ROOT/user/bin/flaui_target.exe" "$DIR/target.c" -luser32 -lcomctl32
sha256sum "$OUT" "$ROOT/user/bin/filezilla_flaui.dll" "$ROOT/user/bin/flaui_target.exe"
