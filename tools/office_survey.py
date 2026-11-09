"""Office survey (ROADMAP OF step 1), run on a Windows machine where
Microsoft 365 Apps are installed (the `office-survey` workflow installs
them, unactivated, with the Office Deployment Tool). Writes black-box
metadata only, never a binary (CLEAN_ROOM: no disassembly; import and
export tables are the PE headers' own lists):

  <out>/version.txt          Click-to-Run's version, channel and products
  <out>/files.tsv            every PE file: path, size, machine
  <out>/imports/<path>.txt   `llvm-readobj --coff-imports` of each PE
  <out>/exports/<name>.txt   the export names of each Office DLL
  <out>/com.tsv              COM classes Office registers (its virtual
                             registry and the real one): CLSID, server
                             kind, server path, ProgID
  <out>/services.tsv         Office's services
  <out>/tasks.txt            Office's scheduled tasks

`tools/import_survey.py --dumps <out>/imports ...` then ranks them
against gala's DLLs.

usage: python -I tools/office_survey.py <out>"""
import os, subprocess, sys, winreg

out = sys.argv[1]
ROOTS = [
    r"C:\Program Files\Microsoft Office",
    r"C:\Program Files\Common Files\microsoft shared\ClickToRun",
    r"C:\Program Files\Common Files\microsoft shared\OFFICE16",
]
READOBJ = next(
    (p for p in [r"C:\Program Files\LLVM\bin\llvm-readobj.exe", "llvm-readobj"]
     if p == "llvm-readobj" or os.path.exists(p)),
    "llvm-readobj")


def readobj(args, path):
    return subprocess.run([READOBJ, *args, path], capture_output=True, text=True,
                          errors="replace").stdout


def machine(path):
    try:
        with open(path, "rb") as f:
            head = f.read(4096)
        if head[:2] != b"MZ":
            return None
        pe = int.from_bytes(head[0x3C:0x40], "little")
        if head[pe:pe + 4] != b"PE\0\0":
            return None
        return {0x8664: "x64", 0x14C: "x86", 0xAA64: "arm64"}.get(
            int.from_bytes(head[pe + 4:pe + 6], "little"), "other")
    except OSError:
        return None


def write(rel, text):
    path = os.path.join(out, rel)
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w", encoding="utf-8") as f:
        f.write(text)


os.makedirs(out, exist_ok=True)

# The version and products.
lines = []
try:
    k = winreg.OpenKey(winreg.HKEY_LOCAL_MACHINE,
                       r"SOFTWARE\Microsoft\Office\ClickToRun\Configuration")
    for name in ["VersionToReport", "CDNBaseUrl", "ProductReleaseIds", "Platform",
                 "UpdateChannel"]:
        try:
            lines.append(f"{name}\t{winreg.QueryValueEx(k, name)[0]}")
        except OSError:
            pass
except OSError as e:
    lines.append(f"no Click-to-Run configuration: {e}")
write("version.txt", "\n".join(lines) + "\n")

# Every PE file, its imports, and an Office DLL's exports.
files = []
for root in ROOTS:
    for dirpath, _, names in os.walk(root):
        for n in names:
            if not n.lower().endswith((".exe", ".dll")):
                continue
            p = os.path.join(dirpath, n)
            m = machine(p)
            if m is None:
                continue
            rel = os.path.relpath(p, r"C:\Program Files")
            files.append(f"{rel}\t{os.path.getsize(p)}\t{m}")
            if m != "x64":
                continue
            write(os.path.join("imports", rel + ".txt"), readobj(["--coff-imports"], p))
            if n.lower().endswith(".dll"):
                names_ = [l.split(":", 1)[1].strip() for l in
                          readobj(["--coff-exports"], p).splitlines()
                          if l.strip().startswith("Name:")]
                write(os.path.join("exports", n.lower() + ".txt"), "\n".join(names_) + "\n")
write("files.tsv", "\n".join(sorted(files)) + "\n")

# COM classes: Click-to-Run's virtual registry, then the real one's
# classes whose server lives under Office's directories.
com = []


def classes(hive, path, where):
    try:
        clsid = winreg.OpenKey(hive, path)
    except OSError:
        return
    i = 0
    while True:
        try:
            c = winreg.EnumKey(clsid, i)
        except OSError:
            break
        i += 1
        progid = ""
        try:
            progid = winreg.QueryValue(clsid, c + r"\ProgID")
        except OSError:
            pass
        for kind in ["InprocServer32", "LocalServer32"]:
            try:
                server = winreg.QueryValue(clsid, c + "\\" + kind)
            except OSError:
                continue
            if where == "real" and "office" not in server.lower():
                continue
            com.append(f"{c}\t{kind}\t{server}\t{progid}\t{where}")


classes(winreg.HKEY_LOCAL_MACHINE,
        r"SOFTWARE\Microsoft\Office\ClickToRun\REGISTRY\MACHINE\Software\Classes\CLSID",
        "c2r")
classes(winreg.HKEY_LOCAL_MACHINE, r"SOFTWARE\Classes\CLSID", "real")
write("com.tsv", "\n".join(sorted(set(com))) + "\n")

# Services and scheduled tasks.
svc = subprocess.run(
    ["powershell", "-NoProfile", "-Command",
     "Get-CimInstance Win32_Service | Where-Object { $_.PathName -match 'Office|ClickToRun' } | "
     "ForEach-Object { \"$($_.Name)`t$($_.StartMode)`t$($_.StartName)`t$($_.PathName)\" }"],
    capture_output=True, text=True, errors="replace").stdout
write("services.tsv", svc)
tasks = subprocess.run(
    ["powershell", "-NoProfile", "-Command",
     "Get-ScheduledTask | Where-Object { $_.TaskPath -match 'Office' } | "
     "ForEach-Object { \"$($_.TaskPath)$($_.TaskName)`t$($_.Actions.Execute) $($_.Actions.Arguments)\" }"],
    capture_output=True, text=True, errors="replace").stdout
write("tasks.txt", tasks)
print(f"office survey: {len(files)} PE files, {len(com)} COM classes -> {out}")
