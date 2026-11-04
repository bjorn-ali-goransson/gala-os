#!/bin/sh
# Cross-checks the MSIX fixtures made by make_fixtures.py with tools
# that share no code with it or with galapkg: Info-ZIP's unzip and
# zipinfo, 7-Zip, xmllint (libxml2), split, sha256sum, basenc, base64,
# dd and cmp (coreutils). Checks, for the well-formed packages:
#   - the zip structure (unzip -t, 7z t: CRC-32s, ZIP64 records);
#   - every AppxBlockMap.xml File: its Size against the extracted
#     file's size, its Block hashes against sha256sum over the extracted
#     file split into 64 KiB blocks (in base64);
#   - for the bundle, each inner package's Offset and Size: the bytes
#     there (dd) equal the extracted inner package (cmp);
#   - bad_hash.msix and wrong_size.msix fail the block map check.
# (check_blockmap uses globals: callers keep their own names apart.)
# Run: sh user/galapkg/testdata/msix/check_fixtures.sh
set -eu
here=$(cd "$(dirname "$0")" && pwd)
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
fail=0

for f in "$here"/*.msix "$here"/*.msixbundle; do
    unzip -tq "$f" >/dev/null || { printf '%s\n' "FAIL unzip -t $f"; fail=1; }
    7z t "$f" >/dev/null || { printf '%s\n' "FAIL 7z t $f"; fail=1; }
done
echo "zip structure: unzip -t and 7z t pass on every fixture"

xp() { xmllint --xpath "$1" "$2"; }

check_blockmap() {
    pkg=$1
    d="$tmp/$(basename "$pkg")"
    mkdir -p "$d"
    unzip -qo "$pkg" -d "$d"
    bm="$d/AppxBlockMap.xml"
    n=$(xp "count(//*[local-name()='File'])" "$bm")
    i=1
    blocks=0
    while [ "$i" -le "$n" ]; do
        file="(//*[local-name()='File'])[$i]"
        name=$(xp "string($file/@Name)" "$bm")
        size=$(xp "string($file/@Size)" "$bm")
        # Zip item names: '/' separators, a space as %20 (OPC).
        path="$d/$(printf '%s' "$name" | tr '\\' '/' | sed 's/ /%20/g')"
        actual=$(stat -c %s "$path")
        [ "$actual" = "$size" ] || { printf '%s\n' "FAIL $pkg $name: Size $size, file has $actual"; fail=1; }
        rm -rf "$tmp/blocks" && mkdir "$tmp/blocks"
        if [ "$actual" -gt 0 ]; then split -b 65536 -a 3 "$path" "$tmp/blocks/b"; fi
        j=1
        for b in $(ls "$tmp/blocks"); do
            want=$(xp "string($file/*[local-name()='Block'][$j]/@Hash)" "$bm")
            got=$(sha256sum "$tmp/blocks/$b" | cut -d' ' -f1 | tr a-f A-F | basenc --base16 -d | base64)
            [ "$want" = "$got" ] || { printf '%s\n' "FAIL $pkg $name block $((j - 1)): $want vs sha256sum $got"; fail=1; }
            j=$((j + 1))
            blocks=$((blocks + 1))
        done
        listed=$(xp "count($file/*[local-name()='Block'])" "$bm")
        [ "$listed" = "$((j - 1))" ] || { printf '%s\n' "FAIL $pkg $name: $listed blocks listed, $((j - 1)) in the file"; fail=1; }
        i=$((i + 1))
    done
    echo "$(basename "$pkg"): $n block map files, $blocks block hashes match sha256sum"
}

for p in app.msix zipfile.msix unsigned.msix app.msixbundle; do
    check_blockmap "$here/$p"
done

# The falsified ones must fail the same independent check.
for p in bad_hash.msix wrong_size.msix; do
    saved=$fail
    fail=0
    check_blockmap "$here/$p" >"$tmp/neg" 2>&1 || true
    if [ "$fail" = 1 ]; then
        printf '%s\n' "$p: rejected as expected: $(grep FAIL "$tmp/neg" | head -1 | sed "s|$here/||")"
    else
        printf '%s\n' "FAIL $p was not rejected"
        saved=1
    fi
    fail=$saved
done

# Inner packages: extracted, and as the bundle manifest's Offset/Size.
bundle="$here/app.msixbundle"
bd="$tmp/app.msixbundle"
m="$bd/AppxMetadata/AppxBundleManifest.xml"
bn=$(xp "count(//*[local-name()='Package'])" "$m")
bi=1
while [ "$bi" -le "$bn" ]; do
    pk="(//*[local-name()='Package'])[$bi]"
    name=$(xp "string($pk/@FileName)" "$m")
    off=$(xp "string($pk/@Offset)" "$m")
    size=$(xp "string($pk/@Size)" "$m")
    dd if="$bundle" of="$tmp/inner" bs=1 skip="$off" count="$size" 2>/dev/null
    cmp -s "$tmp/inner" "$bd/$name" || { printf '%s\n' "FAIL $name: Offset $off Size $size do not hold it"; fail=1; }
    inner=$name
    check_blockmap "$bd/$inner"
    bi=$((bi + 1))
done
echo "app.msixbundle: $bn inner packages at their Offset/Size (dd + cmp)"

[ "$fail" = 0 ] && echo "check_fixtures: all checks pass"
exit "$fail"
