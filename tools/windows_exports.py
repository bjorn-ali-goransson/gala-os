"""Windows' export lists: the named exports of every 64-bit DLL directly
in System32, run on a Windows machine (the `windows-exports` workflow
runs it on CI's windows-2025 runner, build 26100). Black-box metadata
only, never a binary (CLEAN_ROOM: the export table is the PE header's
own list, as `tools/office_survey.py` reads Office's):

  <out>/version.txt    the Windows product, edition and build
  <out>/exports.tsv    dll, ordinal, name, forwarded-to ("" when not)

API set contract DLLs (api-ms-win-*, ext-ms-win-*) are left out: they
name contracts other DLLs host, not functions of their own. Exports
with no name (ordinal only) are left out too.

`tools/api_coverage.py` measures gala's DLLs against the list.

usage: python -I tools/windows_exports.py <out>"""
import os, subprocess, sys, winreg

out = sys.argv[1]
SYSTEM32 = os.path.join(os.environ.get("SystemRoot", r"C:\Windows"), "System32")
READOBJ = next(
    (p for p in [r"C:\Program Files\LLVM\bin\llvm-readobj.exe", "llvm-readobj"]
     if p == "llvm-readobj" or os.path.exists(p)),
    "llvm-readobj")


def machine(path):
    try:
        with open(path, "rb") as f:
            head = f.read(4096)
        if head[:2] != b"MZ":
            return None
        pe = int.from_bytes(head[0x3C:0x40], "little")
        if head[pe:pe + 4] != b"PE\0\0":
            return None
        return int.from_bytes(head[pe + 4:pe + 6], "little")
    except OSError:
        return None


os.makedirs(out, exist_ok=True)

k = winreg.OpenKey(winreg.HKEY_LOCAL_MACHINE, r"SOFTWARE\Microsoft\Windows NT\CurrentVersion")
lines = []
for name in ["ProductName", "EditionID", "InstallationType", "DisplayVersion",
             "CurrentBuild", "UBR"]:
    try:
        lines.append(f"{name}\t{winreg.QueryValueEx(k, name)[0]}")
    except OSError:
        pass
with open(os.path.join(out, "version.txt"), "w", encoding="utf-8") as f:
    f.write("\n".join(lines) + "\n")

dlls = sorted(
    n for n in os.listdir(SYSTEM32)
    if n.lower().endswith(".dll")
    and not n.lower().startswith(("api-ms-win-", "ext-ms-win-"))
    and machine(os.path.join(SYSTEM32, n)) == 0x8664)

rows = []
# llvm-readobj takes many files at once and heads each with `File:`.
for i in range(0, len(dlls), 64):
    batch = dlls[i:i + 64]
    text = subprocess.run([READOBJ, "--coff-exports", *[os.path.join(SYSTEM32, n) for n in batch]],
                          capture_output=True, text=True, errors="replace").stdout
    dll = None
    ordinal = name = None
    def flush():
        if dll and name:
            rows.append((dll, ordinal, name, forward))
    forward = ""
    for line in text.splitlines():
        s = line.strip()
        if s.startswith("File:"):
            flush(); name = None
            dll = os.path.basename(s[5:].strip()).lower()
        elif s.startswith("Ordinal:"):
            flush(); name = None; forward = ""
            ordinal = int(s.split()[1])
        elif s.startswith("Name:"):
            name = s[5:].strip()
        elif s.startswith("ForwardedTo:"):
            forward = s.split(None, 1)[1].strip()
    flush()

with open(os.path.join(out, "exports.tsv"), "w", encoding="utf-8") as f:
    for dll, ordinal, name, forward in sorted(set(rows)):
        f.write(f"{dll}\t{ordinal}\t{name}\t{forward}\n")
print(f"{len(dlls)} x64 DLLs, {len(set(r[0] for r in rows))} with named exports, "
      f"{len(set(rows))} named exports")
