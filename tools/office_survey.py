"""Office survey (ROADMAP OF step 1), run on a Windows machine where
Microsoft 365 Apps are installed (the `office-survey` workflow installs
them, unactivated, with the Office Deployment Tool). Writes black-box
metadata only, never a binary (CLEAN_ROOM: no disassembly; import and
export tables are the PE headers' own lists):

  <out>/version.txt          Click-to-Run's version, channel and products
  <out>/files.tsv            every PE file: path, size, machine
  <out>/imports/<path>.txt   `llvm-readobj --coff-imports` of each PE
                             (only where llvm-readobj is installed)
  <out>/exports/<name>.txt   the export names of each Office DLL (read
                             here when llvm-readobj is not installed)
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


def have_readobj():
    """Whether llvm-readobj runs here. The imports need it; the exports
    do not (`export_names`), so a machine without LLVM still surveys the
    DLLs gala's loader is measured against."""
    try:
        return subprocess.run([READOBJ, "--version"], capture_output=True).returncode == 0
    except OSError:
        return False


READOBJ_OK = have_readobj()


def export_names(path):
    """The names a DLL exports, in the order the export name table holds
    them, which is the order llvm-readobj prints and the PE format keeps
    sorted by name.

    llvm-readobj's list where it is installed, and otherwise the export
    directory read here: the PE header's own name and ordinal arrays,
    which is metadata and not code (CLEAN_ROOM), as `machine` above
    already reads the file header. The two agree -- `--coff-exports`
    prints one `Name:` per named export -- so which one ran does not
    change what a survey says.
    """
    if READOBJ_OK:
        return [l.split(":", 1)[1].strip()
                for l in readobj(["--coff-exports"], path).splitlines()
                if l.strip().startswith("Name:")]
    with open(path, "rb") as f:
        data = f.read()
    pe = int.from_bytes(data[0x3C:0x40], "little")
    nsec = int.from_bytes(data[pe + 6:pe + 8], "little")
    opt_size = int.from_bytes(data[pe + 20:pe + 22], "little")
    opt = pe + 24
    # The data directories follow the optional header, of which PE32+
    # (0x20b) has eight bytes more than PE32; the export directory is
    # the first entry of either.
    dirs = opt + (112 if int.from_bytes(data[opt:opt + 2], "little") == 0x20B else 96)
    edir = int.from_bytes(data[dirs:dirs + 4], "little")
    if not edir:
        return []
    secs = []
    for i in range(nsec):
        s = opt + opt_size + i * 40
        vsize = int.from_bytes(data[s + 8:s + 12], "little")
        vaddr = int.from_bytes(data[s + 12:s + 16], "little")
        rsize = int.from_bytes(data[s + 16:s + 20], "little")
        raddr = int.from_bytes(data[s + 20:s + 24], "little")
        secs.append((vaddr, max(vsize, rsize), raddr))

    def off(rva):
        """The file offset an address holds. Every address wanted here
        lies in the section the export directory is in."""
        for vaddr, size, raddr in secs:
            if vaddr <= rva < vaddr + size:
                return raddr + (rva - vaddr)
        raise ValueError("address %#x is in no section of %s" % (rva, path))

    e = off(edir)
    count = int.from_bytes(data[e + 24:e + 28], "little")
    if not count:
        return []  # exported by ordinal only: there is no name array
    anames = off(int.from_bytes(data[e + 32:e + 36], "little"))
    names = []
    for i in range(count):
        o = off(int.from_bytes(data[anames + 4 * i:anames + 4 * i + 4], "little"))
        names.append(data[o:data.index(b"\0", o)].decode("latin1"))
    return names


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
            if READOBJ_OK:
                write(os.path.join("imports", rel + ".txt"), readobj(["--coff-imports"], p))
            if n.lower().endswith(".dll"):
                write(os.path.join("exports", n.lower() + ".txt"),
                      "\n".join(export_names(p)) + "\n")
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
