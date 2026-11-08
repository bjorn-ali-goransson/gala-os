"""Import survey: each import (static and delay-loaded) of the given PE
files against gala's built DLLs, as gala's loader resolves them (API set
names mapped as nt name normalization maps them). An export counts only if
it is not a gala_stubs stub (.gstub: GetProcAddress skips those).

usage: python3 -I tools/import_survey.py target/x86_64-pc-windows-msvc/release <pe>...
(docs/WORKFLOW.md "Chromium")."""
import os, re, subprocess, sys
from collections import defaultdict

dlldir = sys.argv[1]

def readobj(args, path):
    return subprocess.run(["llvm-readobj", *args, path], capture_output=True, text=True).stdout

def sections(text):
    out, cur = [], {}
    for line in text.splitlines():
        line = line.strip()
        m = re.match(r"Name: (\S+)", line)
        if m and "(" in line:
            cur = {"name": m.group(1)}
            out.append(cur)
        m = re.match(r"VirtualSize: 0x([0-9A-F]+)", line)
        if m: cur["size"] = int(m.group(1), 16)
        m = re.match(r"VirtualAddress: 0x([0-9A-F]+)", line)
        if m: cur["va"] = int(m.group(1), 16)
    return out

cache = {}
def exports(dll):
    key = dll.lower()
    if key in cache: return cache[key]
    path = os.path.join(dlldir, key)
    if not os.path.exists(path):
        cache[key] = None
        return None
    secs = sections(readobj(["--sections"], path))
    real, stub = set(), set()
    text = readobj(["--coff-exports"], path)
    ordinal = name = None
    forwards = []
    for line in text.splitlines():
        line = line.strip()
        if line.startswith("Ordinal:"): ordinal = int(line.split()[1]); name = None
        elif line.startswith("Name:"): name = line[5:].strip()
        elif line.startswith("ForwardedTo:"):
            # Counted if its target has it (kernelbase forwards Windows'
            # whole list, some to functions gala lacks).
            forwards.append((name, ordinal, line.split(None, 1)[1].strip()))
        elif line.startswith("RVA:"):
            rva = int(line.split()[1], 16)
            if rva == 0: continue
            sec = next((s["name"] for s in secs if s.get("va", 0) <= rva < s.get("va", 0) + s.get("size", 0)), "")
            target = stub if sec == ".gstub" else real
            if name: target.add(name)
            target.add(f"#{ordinal}")
    cache[key] = (real, stub)
    for name, ordinal, to in forwards:
        dll, _, func = to.rpartition(".")
        # (The cache entry is in place: a cycle ends.)
        ex = exports(resolve(dll + ".dll"))
        target = ex and (real if func in ex[0] else stub if func in ex[1] else None)
        if target is not None:
            if name: target.add(name)
            target.add(f"#{ordinal}")
    return cache[key]

# The core sets not hosted by kernelbase: user/testntdll's CORE_HOSTS.
CORE_HOSTS = [
    ("ntdll.dll", "apiquery-l1- rtlsupport-l1- xstate-l1-"),
    ("kernel32.dll", "appinit-l1- atoms-l1- calendar-l1- console-ansi-l2- file-ansi-l1- "
     "file-ansi-l2- firmware-l1- heap-obsolete-l1- io-l1- job-l2- kernel32-legacy-ansi-l1- "
     "kernel32-legacy-l1- kernel32-private-l1- localization-ansi-l1- namedpipe-ansi-l1- "
     "namespace-ansi-l1- privateprofile-l1- processenvironment-ansi-l1- processsecurity-l1- "
     "processthreads-l1- processtopology-obsolete-l1- sidebyside-ansi-l1- string-obsolete-l1- "
     "synch-ansi-l1- toolhelp-l1- ums-l1- util-l1-"),
    ("advapi32.dll", "registry-l2- registry-private-l1- shutdown-ansi-l1- shutdown-l1- sysinfo-l2-"),
    ("ole32.dll", "com- marshal-l1- winrt-"),
    ("msvcrt.dll", "crt-"),
    ("mpr.dll", "multipleproviderrouter-l1-"),
    ("shcore.dll", "featurestaging-l1-"),
    ("slc.dll", "slapi-l1-"),
    ("twinapi.appcore.dll", "biplmapi- biptcltapi- psm-"),
]
def resolve(dll):
    d = dll.lower()
    m = re.match(r"api-ms-win-downlevel-(\w+?)-l", d)
    if m: return m.group(1) + ".dll"
    if d.startswith("api-ms-win-"):
        r = d[len("api-ms-win-"):]
        if r.startswith("crt-"): return "ucrtbase.dll"
        if r.startswith("core-"):
            c = r[len("core-"):]
            return next((h for h, ps in CORE_HOSTS if c.startswith(tuple(ps.split()))), "kernelbase.dll")
        if r.startswith(("security-", "eventing-", "service-")): return "advapi32.dll"
        if r.startswith("shcore-"): return "shcore.dll"
        if r.startswith("power-"): return "powrprof.dll"
    return d

for pe in sys.argv[2:]:
    text = readobj(["--coff-imports"], pe)
    missing = defaultdict(list)
    stubbed = defaultdict(list)
    nodll = set()
    kind = dll = None
    for line in text.splitlines():
        s = line.strip()
        if line.startswith("Import {"): kind = "static"
        elif line.startswith("DelayImport {"): kind = "delay"
        elif s.startswith("Name:"): dll = s[5:].strip()
        elif s.startswith("Symbol:"):
            m = re.match(r"Symbol: (\S*) \((\d+)\)", s)
            sym = m.group(1) if m.group(1) else f"#{m.group(2)}"
            ex = exports(resolve(dll))
            if ex is None: nodll.add((kind, dll)); continue
            real, stub = ex
            if sym in real: continue
            (stubbed if sym in stub else missing)[(kind, dll)].append(sym)
    print(f"== {os.path.basename(pe)}")
    for k, d in sorted(nodll): print(f"  no dll [{k}] {d}")
    for (k, d), syms in sorted(missing.items()):
        print(f"  missing [{k}] {d} ({len(syms)}): {' '.join(syms)}")
    for (k, d), syms in sorted(stubbed.items()):
        print(f"  stub-only [{k}] {d} ({len(syms)}): {' '.join(syms)}")
