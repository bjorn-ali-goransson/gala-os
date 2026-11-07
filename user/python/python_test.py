"""CPython on gala: the standard library's pieces that lean on the OS,
each with the result real Windows gives. Run by the `python_script`
scenario (C:\\gala\\python_test.py); on Windows: `python python_test.py`.
Prints one line per check and `python_test: ok`, or the failing check's
traceback and exits 1."""

import sys


def check(name, got, want):
    if got != want:
        raise AssertionError(f"{name}: got {got!r}, want {want!r}")
    print(f"python_test: {name} ok")


def threads():
    import threading

    total = []
    lock = threading.Lock()

    def add(n):
        for _ in range(1000):
            with lock:
                total.append(n)

    workers = [threading.Thread(target=add, args=(i,)) for i in range(4)]
    for w in workers:
        w.start()
    for w in workers:
        w.join()
    check("threads", len(total), 4000)


def sockets():
    import socket
    import threading

    server = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    server.bind(("127.0.0.1", 0))
    server.listen(1)
    port = server.getsockname()[1]

    def serve():
        conn, _ = server.accept()
        with conn:
            data = conn.recv(64)
            conn.sendall(data.upper())

    t = threading.Thread(target=serve)
    t.start()
    with socket.create_connection(("127.0.0.1", port), timeout=30) as c:
        c.sendall(b"gala")
        reply = c.recv(64)
    t.join()
    server.close()
    check("sockets", reply, b"GALA")


def select_poll():
    import select
    import socket

    a, b = socket.socketpair()
    b.sendall(b"x")
    readable, _, _ = select.select([a], [], [], 10)
    check("select", readable, [a])
    a.close()
    b.close()


def subprocesses():
    import subprocess

    out = subprocess.run(
        [sys.executable, "-c", "import os; print(os.getppid() > 0)"],
        capture_output=True,
        text=True,
        timeout=120,
    )
    check("subprocess", (out.returncode, out.stdout.strip()), (0, "True"))


def extensions():
    import ctypes
    import hashlib
    import json
    import os
    import sqlite3
    import zlib

    check("hashlib", hashlib.sha256(b"gala").hexdigest()[:16], "2a431f021a224c45")
    check("zlib", zlib.decompress(zlib.compress(b"gala" * 100)), b"gala" * 100)
    check("json", json.loads(json.dumps({"a": [1, 2]})), {"a": [1, 2]})
    db = sqlite3.connect(":memory:")
    db.execute("create table t (x)")
    db.executemany("insert into t values (?)", [(1,), (2,), (3,)])
    check("sqlite3", db.execute("select sum(x) from t").fetchone()[0], 6)
    db.close()
    check("ctypes", ctypes.windll.kernel32.GetCurrentProcessId(), os.getpid())


def main():
    print(f"python_test: Python {sys.version.split()[0]} on {sys.platform}")
    for test in (threads, sockets, select_poll, subprocesses, extensions):
        test()
    print("python_test: ok")


if __name__ == "__main__":
    try:
        main()
    except BaseException:
        import traceback

        traceback.print_exc()
        sys.exit(1)
