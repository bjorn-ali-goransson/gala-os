"""pip and venv on gala, on the full CPython layout (the NuGet `python`
package: pip in site-packages, the venv launchers), each check with the
result real Windows gives. Run by the `python_pip` scenario
(C:\\gala\\python_pip.py); on Windows: `python python_pip.py` with that
layout's python.exe. Nothing comes from the network: the script builds
a small wheel itself (a module and a console script), pip installs it
from the file, and the console script runs through the launcher .exe pip
writes (distlib's t64.exe with the script appended). Then a virtual
environment: `python -m venv` with its launcher python.exe, which starts
the base interpreter with the environment's prefix. Prints one line per
check and `python_pip: ok`, or the failing check's traceback and exits
1."""

import base64
import hashlib
import os
import subprocess
import sys
import tempfile
import zipfile

NAME, VERSION = "galahello", "1.0"


def check(name, got, want):
    if got != want:
        raise AssertionError(f"{name}: got {got!r}, want {want!r}")
    print(f"python_pip: {name} ok")


def run(*args, **kw):
    out = subprocess.run(list(args), capture_output=True, text=True, timeout=600, **kw)
    if out.returncode != 0:
        print(out.stdout)
        print(out.stderr)
    return out


def build_wheel(directory):
    """A pure-Python wheel (PEP 427): the package, its dist-info with a
    console script, and the RECORD of every file's hash."""
    dist_info = f"{NAME}-{VERSION}.dist-info"
    files = {
        f"{NAME}/__init__.py": "def main():\n    print('galahello from a console script')\n    return 0\n\nGREETING = 'hello from a wheel'\n",
        f"{dist_info}/METADATA": f"Metadata-Version: 2.1\nName: {NAME}\nVersion: {VERSION}\n",
        f"{dist_info}/WHEEL": "Wheel-Version: 1.0\nGenerator: python_pip.py\nRoot-Is-Purelib: true\nTag: py3-none-any\n",
        f"{dist_info}/entry_points.txt": f"[console_scripts]\n{NAME} = {NAME}:main\n",
    }
    record = []
    for path, text in files.items():
        data = text.encode()
        digest = base64.urlsafe_b64encode(hashlib.sha256(data).digest()).rstrip(b"=").decode()
        record.append(f"{path},sha256={digest},{len(data)}")
    record.append(f"{dist_info}/RECORD,,")
    files[f"{dist_info}/RECORD"] = "\n".join(record) + "\n"
    wheel = os.path.join(directory, f"{NAME}-{VERSION}-py3-none-any.whl")
    with zipfile.ZipFile(wheel, "w", zipfile.ZIP_DEFLATED) as z:
        for path, text in files.items():
            z.writestr(path, text)
    return wheel


def layout():
    import ctypes

    check("ctypes", ctypes.windll.kernel32.GetCurrentProcessId(), os.getpid())
    # Where pip keeps its cache: the shell's known folder.
    buf = ctypes.create_unicode_buffer(260)
    check("SHGetFolderPathW", ctypes.windll.shell32.SHGetFolderPathW(None, 28, None, 0, buf), 0)  # CSIDL_LOCAL_APPDATA
    check("local appdata", os.path.normcase(buf.value), os.path.normcase(os.environ["LOCALAPPDATA"]))


def pip_version():
    out = run(sys.executable, "-m", "pip", "--version")
    check("pip --version", (out.returncode, out.stdout.split()[:2]), (0, ["pip", "24.3.1"]))


def pip_install(work):
    wheel = build_wheel(work)
    target = os.path.join(work, "site")
    out = run(
        sys.executable, "-m", "pip", "install", "--no-index", "--no-cache-dir",
        "--disable-pip-version-check", "--target", target, wheel,
    )
    check("pip install", out.returncode, 0)
    out = run(sys.executable, "-c", f"import {NAME}; print({NAME}.GREETING)", env={**os.environ, "PYTHONPATH": target})
    check("import installed", out.stdout.strip(), "hello from a wheel")
    launcher = os.path.join(target, "bin", f"{NAME}.exe")
    check("console script written", os.path.isfile(launcher), True)
    out = run(launcher, env={**os.environ, "PYTHONPATH": target})
    check("console script runs", (out.returncode, out.stdout.strip()), (0, "galahello from a console script"))
    out = run(sys.executable, "-m", "pip", "list", "--disable-pip-version-check", "--path", target)
    check("pip list", [line.split() for line in out.stdout.splitlines()][2:], [[NAME, VERSION]])


def venv(work):
    env_dir = os.path.join(work, "venv")
    out = run(sys.executable, "-m", "venv", "--without-pip", env_dir)
    check("venv created", out.returncode, 0)
    python = os.path.join(env_dir, "Scripts", "python.exe")
    check("venv launcher", os.path.isfile(python), True)
    out = run(python, "-c", "import sys; print(sys.prefix != sys.base_prefix, sys.prefix)")
    flag, prefix = out.stdout.split(maxsplit=1)
    check("venv interpreter", (out.returncode, flag, os.path.normcase(prefix.strip())), (0, "True", os.path.normcase(env_dir)))
    with open(os.path.join(env_dir, "pyvenv.cfg")) as f:
        check("pyvenv.cfg", any(line.startswith("home = ") for line in f), True)


def main():
    print(f"python_pip: Python {sys.version.split()[0]} on {sys.platform}")
    layout()
    pip_version()
    with tempfile.TemporaryDirectory() as work:
        pip_install(work)
        venv(work)
    print("python_pip: ok")


if __name__ == "__main__":
    try:
        main()
    except BaseException:
        import traceback

        traceback.print_exc()
        sys.exit(1)
