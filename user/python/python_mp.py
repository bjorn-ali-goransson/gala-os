"""multiprocessing on gala: Windows has no fork, so every child is
spawned (CreateProcess of python.exe with an inherited pipe handle; the
parent pickles the work over it), with semaphores, named pipes and
shared memory between them, each check with the result real Windows
gives. Run by the `python_mp` scenario (C:\\gala\\python_mp.py); on
Windows: `python python_mp.py`. Prints one line per check and
`python_mp: ok`, or the failing check's traceback and exits 1."""

import multiprocessing as mp
import os
import sys


def check(name, got, want):
    if got != want:
        raise AssertionError(f"{name}: got {got!r}, want {want!r}")
    print(f"python_mp: {name} ok")


def square(x):
    return x * x


def child_pid(_):
    return os.getpid()


def put_parent(q):
    q.put((os.getppid(), mp.current_process().name))


def exit_with(code):
    sys.exit(code)


def add_shared(counter, lock, n):
    for _ in range(n):
        with lock:
            counter.value += 1


def echo_pipe(conn):
    while (msg := conn.recv()) is not None:
        conn.send(msg[::-1])
    conn.close()


def fill_shared(name, size):
    from multiprocessing import shared_memory

    shm = shared_memory.SharedMemory(name=name)
    shm.buf[:size] = bytes(range(size))
    shm.close()


def start_method():
    check("start method", mp.get_start_method(), "spawn")


def in_one_process():
    sem = mp.BoundedSemaphore(2)
    check("semaphore", (sem.acquire(timeout=5), sem.acquire(timeout=5), sem.acquire(timeout=0.1)), (True, True, False))
    sem.release()
    sem.release()
    try:
        sem.release()
        over = False
    except ValueError:
        over = True
    check("semaphore bound", over, True)
    lock = mp.Lock()
    check("lock", (lock.acquire(timeout=5), lock.acquire(timeout=0.1)), (True, False))
    lock.release()
    q = mp.Queue()
    q.put("gala")
    check("queue", q.get(timeout=30), "gala")


def process_and_queue():
    q = mp.Queue()
    p = mp.Process(target=put_parent, args=(q,), name="gala-child")
    p.start()
    got = q.get(timeout=120)
    p.join(120)
    check("process", (got, p.exitcode), ((os.getpid(), "gala-child"), 0))
    p = mp.Process(target=exit_with, args=(7,))
    p.start()
    p.join(120)
    check("exitcode", p.exitcode, 7)


def pool():
    with mp.Pool(2) as workers:
        check("pool map", workers.map(square, range(10)), [x * x for x in range(10)])
        pids = set(workers.map(child_pid, range(8), chunksize=1))
        check("pool workers", len(pids) <= 2 and os.getpid() not in pids, True)
        check("pool apply_async", workers.apply_async(square, (12,)).get(120), 144)


def shared_value():
    counter = mp.Value("i", 0, lock=False)
    lock = mp.Lock()
    procs = [mp.Process(target=add_shared, args=(counter, lock, 200)) for _ in range(2)]
    for p in procs:
        p.start()
    for p in procs:
        p.join(120)
    check("shared value", (counter.value, [p.exitcode for p in procs]), (400, [0, 0]))


def pipes():
    parent, child = mp.Pipe()
    p = mp.Process(target=echo_pipe, args=(child,))
    p.start()
    replies = []
    for msg in ("gala", b"bytes", [1, 2, 3]):
        parent.send(msg)
        replies.append(parent.recv())
    parent.send(None)
    p.join(120)
    check("pipe", replies, ["alag", b"setyb", [3, 2, 1]])


def shared_memory_block():
    from multiprocessing import shared_memory

    shm = shared_memory.SharedMemory(create=True, size=4096)
    try:
        p = mp.Process(target=fill_shared, args=(shm.name, 256))
        p.start()
        p.join(120)
        check("shared memory", bytes(shm.buf[:256]), bytes(range(256)))
    finally:
        shm.close()
        shm.unlink()


def main():
    print(f"python_mp: Python {sys.version.split()[0]} on {sys.platform}")
    for test in (start_method, in_one_process, process_and_queue, pool, shared_value, pipes, shared_memory_block):
        test()
    print("python_mp: ok")


if __name__ == "__main__":
    try:
        main()
    except BaseException:
        import traceback

        traceback.print_exc()
        sys.exit(1)
