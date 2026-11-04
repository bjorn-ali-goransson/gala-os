#!/bin/sh
# Regenerates galapkg's MSI and CAB fixtures and the expected values the
# tests compare against (docs/WORKFLOW.md "Installing software").
#
# Tools (THIRD_PARTY.md, "Tools used to make test fixtures"; none is
# linked into gala): wixl (msitools) builds the MSIs from the .wxs
# sources, gcab makes the stand-alone cabinets; msiinfo,
# msiextract (msitools), gcab -t and 7z l record what independent
# readers see, into expected/. The tests check gala's reader against
# those files.
#
# Run from anywhere: sh user/galapkg/testdata/msi/make_fixtures.sh
set -eu
here=$(cd "$(dirname "$0")" && pwd)
cd "$here"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

# The payload files (sources for wixl and gcab).
printf 'Hello from gala\r\n' > "$work/hello.txt"
printf 'A file with a short and a long name\r\n' > "$work/long.txt"
python3 -I -c '
import sys
sys.stdout.write("".join("line %05d of the big file\r\n" % i for i in range(4000)))
' > "$work/big.txt"
printf 'MZ not a real DLL, a Binary row for the tests\r\n' > "$work/script.dll"
# Incompressible bytes (a fixed LCG) for a stored-block MSZIP test.
python3 -I -c '
import sys
x = 12345
out = bytearray()
for _ in range(40000):
    x = (x * 1103515245 + 12345) & 0x7fffffff
    out.append((x >> 16) & 0xff)
sys.stdout.buffer.write(bytes(out))
' > "$work/noise.bin"

# Fixed times, so cabinets (which keep them) come out the same each run.
find "$work" -type f -exec touch -d '2026-01-02 03:04:06' {} +

# MSIs. wixl resolves Source= relative to the current directory.
for wxs in basic; do
    (cd "$work" && wixl -o "$here/$wxs.msi" "$here/$wxs.wxs")
done

# The install-plan packages (src/msi/plan/tests.rs). wixl does not write
# everything WiX would (Condition elements, a custom action's Directory,
# the #x and #% registry prefixes, HKMU as Root -1, $ in formatted
# text), so msibuild (msitools) puts those rows in afterwards with SQL.
printf 'core\r\n' > "$work/core.txt"
printf 'MZ not a real program\r\n' > "$work/tool.exe"
printf 'readme\r\n' > "$work/readme.txt"
printf 'extra\r\n' > "$work/extra.txt"
printf 'never\r\n' > "$work/never.txt"
printf 'MZ not a real DLL\r\n' > "$work/ca.dll"
find "$work" -type f -exec touch -d '2026-01-02 03:04:06' {} +
(cd "$work" && wixl -a x64 -o "$here/plan.msi" "$here/plan.wxs")
(cd "$work" && wixl -o "$here/dual.msi" "$here/dual.wxs")
msibuild plan.msi \
    -q "UPDATE \`Registry\` SET \`Root\` = -1 WHERE \`Root\` = 4" \
    -q "UPDATE \`Registry\` SET \`Value\` = '#x0A0B0C' WHERE \`Name\` = 'Blob'" \
    -q "UPDATE \`Registry\` SET \`Value\` = '#%%ProgramFiles%\\Gala Plan' WHERE \`Name\` = 'Path'" \
    -q "UPDATE \`Registry\` SET \`Value\` = '[\$Core]|[!core.txt]|[%TEMP]|{[NOSUCH]x}|{plain}|[\\[]x[\\]]' WHERE \`Name\` = 'Where'" \
    -q "UPDATE \`InstallExecuteSequence\` SET \`Condition\` = '\$Core=3 AND NOT REMOVE' WHERE \`Action\` = 'Deferred'" \
    -q "UPDATE \`Component\` SET \`Condition\` = 'NOT NOEXTRA' WHERE \`Component\` = 'Extra'" \
    -q "UPDATE \`Component\` SET \`Attributes\` = 258 WHERE \`Component\` = 'Docs'" \
    -q "UPDATE \`Feature\` SET \`Attributes\` = 1 WHERE \`Feature\` = 'Documentation'" \
    -q "CREATE TABLE \`Condition\` (\`Feature_\` CHAR(38) NOT NULL, \`Level\` SHORT NOT NULL, \`Condition\` CHAR(255) PRIMARY KEY \`Feature_\`, \`Level\`)" \
    -q "INSERT INTO \`Condition\` (\`Feature_\`, \`Level\`, \`Condition\`) VALUES ('Extras', 1, 'WANTEXTRAS')" \
    -q "INSERT INTO \`CustomAction\` (\`Action\`, \`Type\`, \`Source\`, \`Target\`) VALUES ('RunTool', 226, 'INSTALLDIR', '\"[#tool.exe]\" /setup')" \
    -q "INSERT INTO \`InstallExecuteSequence\` (\`Action\`, \`Condition\`, \`Sequence\`) VALUES ('RunTool', 'GREETING ~= \"HELLO\"', 6601)" \
    -q "INSERT INTO \`InstallUISequence\` (\`Action\`, \`Condition\`, \`Sequence\`) VALUES ('SetGreeting', 'UILevel > 3', 1001)" \
    -q "INSERT INTO \`LaunchCondition\` (\`Condition\`, \`Description\`) VALUES ('VersionNT >= 600', '[ProductName] needs Windows Vista or later')" \
    -q "INSERT INTO \`LaunchCondition\` (\`Condition\`, \`Description\`) VALUES ('NOT BLOCKME', '[ProductName] is blocked: BLOCKME=[BLOCKME]')"
msibuild dual.msi \
    -q "UPDATE \`Registry\` SET \`Root\` = -1 WHERE \`Root\` = 4" \
    -q "INSERT INTO \`Registry\` (\`Registry\`, \`Root\`, \`Key\`, \`Name\`, \`Value\`, \`Component_\`) VALUES ('Cls', 0, '.galadual', '', 'GalaDual.File', 'App')" \
    -q "INSERT INTO \`InstallUISequence\` (\`Action\`, \`Condition\`, \`Sequence\`) VALUES ('FatalErrorDlg', '', -3)" \
    -q "INSERT INTO \`InstallUISequence\` (\`Action\`, \`Condition\`, \`Sequence\`) VALUES ('NeverRuns', '', 0)"
# A package whose sequence holds a condition that does not parse.
cp dual.msi badcond.msi
msibuild badcond.msi \
    -q "INSERT INTO \`InstallExecuteSequence\` (\`Action\`, \`Condition\`, \`Sequence\`) VALUES ('Broken', 'A = = B', 6700)"

# A package with a string of 64 KiB or more (the string pool's long
# entry), generated here and kept zlib-compressed (it is mostly "x").
python3 -I -c '
print("<?xml version=\"1.0\"?>")
print("<Wix xmlns=\"http://schemas.microsoft.com/wix/2006/wi\">")
print("<Product Id=\"11111111-2222-3333-4444-555555555555\" Name=\"L\" Language=\"1033\"")
print(" Version=\"1.0\" Manufacturer=\"G\" UpgradeCode=\"11111111-2222-3333-4444-666666666666\">")
print("<Package Compressed=\"yes\"/>")
print("<Property Id=\"LONG\" Value=\"" + "x" * 70000 + "y\"/>")
print("<Property Id=\"AFTER\" Value=\"after\"/>")
print("</Product></Wix>")
' > "$work/long.wxs"
(cd "$work" && wixl -o long.msi long.wxs)
python3 -I -c '
import sys, zlib
sys.stdout.buffer.write(zlib.compress(open(sys.argv[1], "rb").read(), 9))
' "$work/long.msi" > long.msi.zlib

# Stand-alone cabinets: MSZIP (-z) and uncompressed, with a nested name.
mkdir -p "$work/cab/dir"
cp "$work/hello.txt" "$work/cab/hello.txt"
cp "$work/big.txt" "$work/cab/dir/big.txt"
cp "$work/noise.bin" "$work/cab/noise.bin"
cp "$work/noise.bin" "$work/cab/dir/noise.bin"
find "$work/cab" -type f -exec touch -d '2026-01-02 03:04:06' {} +
rm -f mszip.cab stored.cab
(cd "$work/cab" && gcab -c -z "$here/mszip.cab" hello.txt dir/big.txt noise.bin)
(cd "$work/cab" && gcab -c "$here/stored.cab" hello.txt dir/noise.bin)

# Expected values from independent readers.
rm -rf expected
mkdir -p expected
for msi in basic; do
    msiinfo tables "$msi.msi" | sort > "expected/$msi.tables"
    msiinfo streams "$msi.msi" | sort > "expected/$msi.streams"
    for t in File Component Directory Feature Registry Shortcut Media CustomAction Binary _ForceCodepage; do
        # export writes a Binary/ directory of the stream columns: kept
        # out of the tree by running it in the work directory.
        (cd "$work" && msiinfo export "$here/$msi.msi" "$t") > "expected/$msi.$t.idt"
    done
    (cd "$work" && sha256sum Binary/*) > "expected/$msi.binary.sha256"
    mkdir -p "$work/x-$msi"
    msiextract -C "$work/x-$msi" "$msi.msi" | sort > "expected/$msi.extract"
    (cd "$work/x-$msi" && find . -type f -print0 | sort -z | xargs -0 sha256sum) > "expected/$msi.sha256"
done
msiinfo suminfo basic.msi > expected/basic.suminfo
# The plan packages' tables as msiinfo reads them (the tests check the
# reader agrees, then plan from them).
for t in Property Directory Component Feature FeatureComponents Condition Registry Shortcut \
         CustomAction InstallExecuteSequence InstallUISequence LaunchCondition File; do
    (cd "$work" && msiinfo export "$here/plan.msi" "$t") > "expected/plan.$t.idt"
done
for t in Property Directory Component Registry Shortcut; do
    (cd "$work" && msiinfo export "$here/dual.msi" "$t") > "expected/dual.$t.idt"
done
mkdir -p "$work/x-plan"
msiextract -C "$work/x-plan" plan.msi | sort > expected/plan.extract
# The long package's Property table as name, value length, SHA-256.
msiinfo export "$work/long.msi" Property | python3 -I -c '
import sys, hashlib
for line in sys.stdin.buffer.read().split(b"\r\n")[3:]:
    if line:
        k, v = line.split(b"\t", 1)
        print(k.decode(), len(v), hashlib.sha256(v).hexdigest(), sep="\t")
' > expected/long.Property
for cab in mszip stored; do
    gcab -l "$cab.cab" > "expected/$cab.gcab-t"
    7z l "$cab.cab" | sed -n '/^---/,$p' > "expected/$cab.7z-l"
    mkdir -p "$work/c-$cab"
    gcab -x -C "$work/c-$cab" "$cab.cab" > /dev/null
    (cd "$work/c-$cab" && find . -type f -print0 | sort -z | xargs -0 sha256sum) > "expected/$cab.sha256"
done
ls -l ./*.msi ./*.cab ./*.zlib
