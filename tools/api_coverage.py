"""API coverage: how much of Windows' API gala's DLLs export, and how much
of what real programs import, written as the website's data
(`web/src/coverage.json`, WORKFLOW "API coverage").

Two measures, both counted the way gala's loader resolves names
(`tools/import_survey.py`'s rules: API set names mapped, forwards
followed, an export in a `.gstub` section is a stub and does not count):

- **Of Windows**: every named export of System32's 64-bit DLLs on build
  26100 (`tools/windows_exports.py`'s list), and how many of those gala's
  DLLs export for real. Everything counts, legacy included: a DLL gala
  does not have counts as nothing.
- **Of a program**: the functions a program imports from the system
  (static and delay imports, of its executables and of its own DLLs they
  load), and how many of those gala's DLLs export for real. A program's
  own DLLs are followed, not counted.

Neither sees what a program looks up at run time (GetProcAddress) or the
methods of COM interfaces, and an export that exists may still do only
part of its job: these are upper bounds of what loads, not of what works.

usage: python3 -I tools/api_coverage.py <gala dll dir> <windows-exports dir>
           <prebuilt dir> [<office-survey dir>] > web/src/coverage.json
"""
import datetime, importlib.util, json, os, re, subprocess, sys
from collections import defaultdict

here = os.path.dirname(os.path.abspath(__file__))
spec = importlib.util.spec_from_file_location("import_survey", os.path.join(here, "import_survey.py"))
survey = importlib.util.module_from_spec(spec)
spec.loader.exec_module(survey)

# The programs measured: the prebuilt ones the boot scenarios run
# (`target/prebuilt/<dir>`), each from the files named. A program that
# loads its engine with LoadLibrary names that DLL too, since no import
# table leads to it: chrome.dll, .NET's host and runtime, the JVM.
DOTNET = ["hostfxr.dll", "hostpolicy.dll", "coreclr.dll", "clrjit.dll"]
PROGRAMS = [
    ("Chromium", "chromium", ["chrome.exe", "chrome.dll", "chrome_elf.dll"]),
    ("Python", "python", ["python.exe"]),
    ("PowerShell", "pwsh", ["pwsh.exe", *DOTNET]),
    (".NET", "dotnet", ["dotnet.exe", *DOTNET]),
    ("Java (JDK)", "jdk", ["java.exe", "jvm.dll"]),
    ("PostgreSQL", "postgresql", ["postgres.exe", "psql.exe"]),
    ("Git (MinGit)", "mingit", ["git.exe"]),
    ("OpenSSH", "openssh", ["ssh.exe", "sshd.exe"]),
    ("curl", "curl", ["curl.exe"]),
    ("Caddy", "caddy", ["caddy.exe"]),
    ("FileZilla", "filezilla", ["filezilla.exe"]),
    ("FileZilla Server", "filezilla-server", ["filezilla-server.exe"]),
]
# The Visual C++ runtime is a program's own, brought by its installer
# (the redistributable), not Windows' API: its imports are not counted.
REDIST = re.compile(r"(vcruntime|msvcp1|concrt|vccorlib|vcomp|mfc1)\d")
# Word's and PowerPoint's executables are launchers that load their
# engines (WWLIB.DLL, PPCORE.DLL) with LoadLibrary.
OFFICE = ("Microsoft Office (Word, Excel, PowerPoint)",
          ["WINWORD.EXE", "WWLIB.DLL", "EXCEL.EXE", "POWERPNT.EXE", "PPCORE.DLL"])


def parse_imports(text):
    """(kind, dll, symbol) of `llvm-readobj --coff-imports` output."""
    out, kind, dll = [], None, None
    for line in text.splitlines():
        s = line.strip()
        if line.startswith("Import {"): kind = "static"
        elif line.startswith("DelayImport {"): kind = "delay"
        elif s.startswith("Name:"): dll = s[5:].strip().lower()
        elif s.startswith("Symbol:"):
            m = re.match(r"Symbol: (\S*) \((\d+)\)", s)
            out.append((kind, dll, m.group(1) if m.group(1) else f"#{m.group(2)}"))
    return out


def program_files(root):
    """{file name: [imports]} of every 64-bit PE under `root`."""
    files = {}
    for dirpath, _, names in os.walk(root):
        for n in names:
            if not n.lower().endswith((".exe", ".dll")): continue
            p = os.path.join(dirpath, n)
            text = survey.readobj(["--coff-imports"], p)
            if "Format: COFF-x86-64" not in text: continue
            files.setdefault(n.lower(), parse_imports(text))
    return files


def dump_files(imports_dir):
    """The same, from office_survey.py's `imports/` dumps."""
    files = {}
    for dirpath, _, names in os.walk(imports_dir):
        for n in names:
            with open(os.path.join(dirpath, n), encoding="utf-8", errors="replace") as f:
                files.setdefault(n[:-len(".txt")].lower(), parse_imports(f.read()))
    return files


def measure(files, roots):
    """The system imports of what `roots` load, through the program's
    own DLLs: {(dll, symbol): "static" | "delay"}."""
    need, seen = {}, {}
    todo = [(r.lower(), "static") for r in roots if r.lower() in files]
    while todo:
        name, how = todo.pop()
        if seen.get(name) == "static" or seen.get(name) == how: continue
        seen[name] = how
        for kind, dll, sym in files[name]:
            k = "static" if how == "static" and kind == "static" else "delay"
            if dll in files:
                todo.append((dll, k))
                continue
            if REDIST.match(dll): continue
            key = (survey.resolve(dll), sym)
            if need.get(key) != "static": need[key] = k
    return need, seen


def score(need):
    real = stub = 0
    absent = defaultdict(int)
    for (dll, sym), _ in need.items():
        ex = survey.exports(dll)
        if ex is None: absent[dll] += 1
        elif sym in ex[0]: real += 1
        elif sym in ex[1]: stub += 1
    missing_dlls = sorted(absent, key=lambda d: -absent[d])
    return real, stub, missing_dlls


def program_entry(name, need, files_loaded, version=None):
    real, stub, missing_dlls = score(need)
    static = {k: v for k, v in need.items() if v == "static"}
    sreal, sstub, _ = score(static)
    e = {"name": name, "files": len(files_loaded), "imports": len(need), "real": real, "stubs": stub,
         "static": {"imports": len(static), "real": sreal, "stubs": sstub},
         "missingDlls": missing_dlls[:8]}
    if version: e["version"] = version
    return e


def main():
    gala_dir, win_dir, prebuilt = sys.argv[1:4]
    office = sys.argv[4] if len(sys.argv) > 4 else None
    survey.dlldir = gala_dir

    windows = defaultdict(set)
    with open(os.path.join(win_dir, "exports.tsv"), encoding="utf-8") as f:
        for line in f:
            dll, _, name, _ = line.rstrip("\r\n").split("\t")
            windows[dll].add(name)
    version = {}
    with open(os.path.join(win_dir, "version.txt"), encoding="utf-8") as f:
        for line in f:
            k, _, v = line.rstrip("\r\n").partition("\t")
            version[k] = v

    dlls = []
    for dll, names in windows.items():
        ex = survey.exports(dll)
        real = len(names & ex[0]) if ex else 0
        stub = len(names & (ex[1] - ex[0])) if ex else 0
        if ex is not None:
            dlls.append({"name": dll[:-len(".dll")], "windows": len(names), "real": real, "stubs": stub})
    dlls.sort(key=lambda d: (-d["windows"], d["name"]))
    total = sum(len(n) for n in windows.values())

    programs = []
    for name, d, roots in PROGRAMS:
        files = program_files(os.path.join(prebuilt, d))
        need, loaded = measure(files, roots)
        if not need:
            sys.exit(f"api_coverage: {name}: none of {roots} under {prebuilt}/{d}")
        programs.append(program_entry(name, need, loaded))
    if office:
        files = dump_files(os.path.join(office, "imports"))
        need, loaded = measure(files, OFFICE[1])
        over = {}
        with open(os.path.join(office, "version.txt"), encoding="utf-8") as f:
            for line in f:
                k, _, v = line.rstrip("\r\n").partition("\t")
                over[k] = v
        programs.append(program_entry(OFFICE[0], need, loaded, over.get("VersionToReport")))
    programs.sort(key=lambda p: -p["real"] / p["imports"])

    for p in programs:
        if p["files"] < 1:
            sys.exit(f"api_coverage: {p['name']}: nothing measured")
    commit = subprocess.run(["git", "-C", here, "rev-parse", "--short=10", "HEAD"],
                            capture_output=True, text=True).stdout.strip()
    out = {
        "measured": datetime.date.today().isoformat(),
        "galaCommit": commit,
        "windows": {"product": version.get("ProductName", ""),
                    "build": f"{version.get('CurrentBuild', '')}.{version.get('UBR', '')}",
                    "dlls": len(windows), "exports": total},
        "gala": {"dlls": len(dlls), "real": sum(d["real"] for d in dlls),
                 "stubs": sum(d["stubs"] for d in dlls)},
        "dlls": dlls,
        "programs": programs,
    }
    json.dump(out, sys.stdout, indent=1)
    sys.stdout.write("\n")


if __name__ == "__main__":
    main()
