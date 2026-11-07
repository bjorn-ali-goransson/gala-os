"""A package with C extensions on gala: numpy 2.2.1's wheel for
CPython 3.13 on x64 (its extension modules, the OpenBLAS and msvcp140 it
bundles in numpy.libs, found through os.add_dll_directory), installed by
pip from the file and used, each check with the result real Windows
gives. Run by the `python_numpy` scenario (C:\\gala\\python_numpy.py,
the wheel beside it); on Windows: `python python_numpy.py` with the
full CPython layout's python.exe. Prints one line per check and
`python_numpy: ok`, or the failing check's traceback and exits 1."""

import os
import subprocess
import sys
import tempfile

WHEEL = os.path.join(os.path.dirname(os.path.abspath(__file__)), "numpy-2.2.1-cp313-cp313-win_amd64.whl")


def check(name, got, want):
    if got != want:
        raise AssertionError(f"{name}: got {got!r}, want {want!r}")
    print(f"python_numpy: {name} ok")


def install(target):
    out = subprocess.run(
        [sys.executable, "-m", "pip", "install", "--no-index", "--no-cache-dir",
         "--disable-pip-version-check", "--target", target, WHEEL],
        capture_output=True, text=True, timeout=900,
    )
    if out.returncode != 0:
        print(out.stdout)
        print(out.stderr)
    check("pip install numpy", out.returncode, 0)


def use():
    import numpy as np

    check("version", np.__version__, "2.2.1")
    check("arange sum", int(np.arange(1000).sum()), 499500)
    check("sqrt", round(float(np.sqrt(np.arange(10)).sum()), 9), 19.306000526)
    # PCG64 is the same on every platform.
    check("random", np.random.default_rng(42).integers(0, 100, 5).tolist(), [8, 77, 65, 43, 43])
    # LAPACK through the bundled OpenBLAS.
    a = np.array([[3.0, 2, -1], [2, -2, 4], [-1, 0.5, -1]])
    check("linalg.solve", np.linalg.solve(a, np.array([1.0, -2, 0])).round(9).tolist(), [1.0, -2.0, -2.0])
    check("det", round(float(np.linalg.det(a)), 9), -3.0)
    # A matrix product large enough for OpenBLAS's threads.
    m = np.arange(40000, dtype=np.float64).reshape(200, 200) / 40000
    check("matmul", round(float((m @ m.T).trace()), 6), 13332.833337)
    check("fft", np.fft.fft([1, 0, 0, 0]).tolist(), [1 + 0j] * 4)


def main():
    print(f"python_numpy: Python {sys.version.split()[0]} on {sys.platform}")
    with tempfile.TemporaryDirectory() as work:
        target = os.path.join(work, "site")
        install(target)
        sys.path.insert(0, target)
        use()
    print("python_numpy: ok")


if __name__ == "__main__":
    try:
        main()
    except BaseException:
        import traceback

        traceback.print_exc()
        sys.exit(1)
