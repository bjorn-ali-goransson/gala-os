"""asyncio on gala: the default event loop on Windows is the proactor
(I/O completion ports through _overlapped: AcceptEx, ConnectEx, WSASend,
WSARecv, overlapped named pipes for subprocesses), each check with the
result real Windows gives. Run by the `python_async` scenario
(C:\\gala\\python_async.py); on Windows: `python python_async.py`.
Prints one line per check and `python_async: ok`, or the failing check's
traceback and exits 1."""

import asyncio
import sys


def check(name, got, want):
    if got != want:
        raise AssertionError(f"{name}: got {got!r}, want {want!r}")
    print(f"python_async: {name} ok")


async def loop_kind():
    loop = asyncio.get_running_loop()
    check("loop", type(loop).__name__, "ProactorEventLoop")


async def sleeps():
    loop = asyncio.get_running_loop()
    start = loop.time()
    results = await asyncio.gather(*(asyncio.sleep(0.05 * i, result=i) for i in range(5)))
    check("gather", results, [0, 1, 2, 3, 4])
    check("sleep elapsed", loop.time() - start >= 0.2, True)
    try:
        await asyncio.wait_for(asyncio.sleep(30), timeout=0.1)
        timed_out = False
    except TimeoutError:
        timed_out = True
    check("wait_for timeout", timed_out, True)


async def tcp_echo():
    async def handle(reader, writer):
        while data := await reader.read(4096):
            writer.write(data.upper())
            await writer.drain()
        writer.close()
        await writer.wait_closed()

    server = await asyncio.start_server(handle, "127.0.0.1", 0)
    port = server.sockets[0].getsockname()[1]
    async with server:
        clients = []
        for i in range(3):
            clients.append(await asyncio.open_connection("127.0.0.1", port))
        replies = []
        for i, (reader, writer) in enumerate(clients):
            writer.write(f"gala {i} ".encode() * 1000)
            await writer.drain()
            writer.write_eof()
        for reader, writer in clients:
            replies.append(await reader.read())
            writer.close()
            await writer.wait_closed()
    check("tcp echo", [len(r) for r in replies], [7000, 7000, 7000])
    check("tcp echo data", replies[2][:7], b"GALA 2 ")


async def udp():
    loop = asyncio.get_running_loop()
    got = loop.create_future()

    class Echo(asyncio.DatagramProtocol):
        def connection_made(self, transport):
            self.transport = transport

        def datagram_received(self, data, addr):
            self.transport.sendto(data[::-1], addr)

    class Client(asyncio.DatagramProtocol):
        def datagram_received(self, data, addr):
            got.set_result(data)

    server, _ = await loop.create_datagram_endpoint(Echo, local_addr=("127.0.0.1", 0))
    port = server.get_extra_info("sockname")[1]
    client, _ = await loop.create_datagram_endpoint(Client, remote_addr=("127.0.0.1", port))
    client.sendto(b"gala")
    check("udp", await asyncio.wait_for(got, 30), b"alag")
    client.close()
    server.close()


async def subprocess_pipes():
    proc = await asyncio.create_subprocess_exec(
        sys.executable,
        "-c",
        "import sys; data = sys.stdin.read(); print(data.upper()); print('err', file=sys.stderr); sys.exit(3)",
        stdin=asyncio.subprocess.PIPE,
        stdout=asyncio.subprocess.PIPE,
        stderr=asyncio.subprocess.PIPE,
    )
    out, err = await asyncio.wait_for(proc.communicate(b"gala"), 120)
    check("subprocess", (proc.returncode, out.strip(), err.strip()), (3, b"GALA", b"err"))


async def executor():
    import threading

    loop = asyncio.get_running_loop()
    names = await asyncio.gather(
        *(loop.run_in_executor(None, lambda: threading.current_thread().name != "MainThread") for _ in range(4))
    )
    check("run_in_executor", names, [True] * 4)


async def queues_and_tasks():
    q = asyncio.Queue()

    async def produce():
        for i in range(100):
            await q.put(i)
        await q.put(None)

    async def consume():
        total = 0
        while (item := await q.get()) is not None:
            total += item
        return total

    async with asyncio.TaskGroup() as tg:
        tg.create_task(produce())
        consumer = tg.create_task(consume())
    check("queue", consumer.result(), 4950)
    task = asyncio.create_task(asyncio.sleep(30))
    await asyncio.sleep(0)
    task.cancel()
    try:
        await task
        cancelled = False
    except asyncio.CancelledError:
        cancelled = True
    check("cancel", cancelled, True)


async def main():
    print(f"python_async: Python {sys.version.split()[0]} on {sys.platform}")
    for test in (loop_kind, sleeps, tcp_echo, udp, subprocess_pipes, executor, queues_and_tasks):
        await test()
    print("python_async: ok")


if __name__ == "__main__":
    try:
        asyncio.run(main())
    except BaseException:
        import traceback

        traceback.print_exc()
        sys.exit(1)
