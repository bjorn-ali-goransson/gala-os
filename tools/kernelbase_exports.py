"""Writes user/kernelbase/exports.txt: the names Windows' kernelbase.dll
exports and the gala DLL each one is forwarded to (ARCHITECTURE "API
sets", "kernelbase").

The names come from Wine's dlls/kernelbase/kernelbase.spec (Wine mirrors
Windows' export lists; the commented-out entries, which Wine does not
export, are left out). The target is the DLL that had the function before
kernelbase existed, as Wine's own specs list it: advapi32, version,
shlwapi, user32 (in that order), else kernel32, where gala implements the
kernel32 family and what only kernelbase has (Wine's own forwards to ntdll
are its implementation, not followed: gala's ntdll lacks those Rtl
functions). The `arch` column: all, x86 or x64.

usage: python3 -I tools/kernelbase_exports.py <dir with Wine's kernelbase,
kernel32, advapi32, version, shlwapi, user32 .spec files> <wine commit>
"""
import os, re, sys

spec_dir, commit = sys.argv[1], sys.argv[2]
LINE = re.compile(r"^@\s+\w+((?:\s+-[\w=,!]+)*)\s+([A-Za-z_]\w*)(\([^)]*\))?\s*(\S*)")

def entries(dll):
    out = {}
    for line in open(os.path.join(spec_dir, dll + ".spec")):
        m = LINE.match(line)
        if m:
            out[m.group(2)] = (m.group(1), m.group(4))
    return out

def arch(flags):
    m = re.search(r"-arch=([\w,!]+)", flags)
    if not m:
        return "all"
    a = m.group(1)
    if a == "!i386" or a in ("x86_64", "win64"):
        return "x64"
    if a in ("i386", "win32"):
        return "x86"
    if set(a.split(",")) >= {"i386", "x86_64"}:
        return "all"
    raise SystemExit(f"unknown arch {a}")

kernelbase = entries("kernelbase")
homes = [(d, set(entries(d))) for d in ("advapi32", "version", "shlwapi", "user32")]
print(f"# Windows' kernelbase.dll exports (Wine {commit}, dlls/kernelbase/kernelbase.spec)")
print("# and the gala DLL each is forwarded to; written by tools/kernelbase_exports.py.")
print("# name target arch")
for name in sorted(kernelbase):
    flags, _ = kernelbase[name]
    target = next((d for d, names in homes if name in names), "kernel32")
    print(f"{name} {target} {arch(flags)}")
