#!/usr/bin/env python3
"""The exception code, faulting module and offset of a program that crashes.

run-all.ps1's `fault` field reads the Application log's Application Error
event (1000), which is never written for these programs: the runner sets
SEM_NOGPFAULTERRORBOX (so that crash.exe opens no dialog) and the error
mode is inherited, and a process with it set does not report its fault to
Windows Error Reporting. The log here holds no such event from any sweep,
while holding older ones, so the field reads "no Application Error event
found" for every program, crashes included.

This gets the same three facts by being the debugger instead, through
documented APIs (no disassembly, no decompilation: docs/CLEAN_ROOM.md):
CreateProcess with DEBUG_ONLY_THIS_PROCESS, then the second-chance
EXCEPTION_DEBUG_EVENT gives the code and address, VirtualQueryEx gives the
address's allocation base and GetModuleFileNameExW the module there, so
the offset is the address less that base. First-chance exceptions are
handed to the program, so one it handles itself is not reported.

    python fault.py user/bin/kernel32_sync_test.exe [...]

Prints one line per program: exception code, module+offset, and the
address. A program that exits without a fatal exception says so.
"""

import ctypes as C
import os
import sys
from ctypes import wintypes as W

k32 = C.WinDLL("kernel32", use_last_error=True)
psapi = C.WinDLL("psapi", use_last_error=True)

DEBUG_ONLY_THIS_PROCESS = 0x00000002
CREATE_NO_WINDOW = 0x08000000
EXCEPTION_DEBUG_EVENT = 1
EXIT_PROCESS_DEBUG_EVENT = 5
DBG_CONTINUE = 0x00010002
DBG_EXCEPTION_NOT_HANDLED = 0x80010001
STATUS_BREAKPOINT = 0x80000003
STATUS_WX86_BREAKPOINT = 0x4000001F
INFINITE = 0xFFFFFFFF

# The codes these programs are expected to produce, for the printout.
NAMES = {
    0xC0000005: "STATUS_ACCESS_VIOLATION",
    0xC000001D: "STATUS_ILLEGAL_INSTRUCTION",
    0xC0000025: "STATUS_NONCONTINUABLE_EXCEPTION",
    0xC0000026: "STATUS_INVALID_DISPOSITION",
    0xC000008C: "STATUS_ARRAY_BOUNDS_EXCEEDED",
    0xC0000094: "STATUS_INTEGER_DIVIDE_BY_ZERO",
    0xC0000096: "STATUS_PRIVILEGED_INSTRUCTION",
    0xC00000FD: "STATUS_STACK_OVERFLOW",
    0xC0000135: "STATUS_DLL_NOT_FOUND",
    0xC0000139: "STATUS_ENTRYPOINT_NOT_FOUND",
    0xC0000374: "STATUS_HEAP_CORRUPTION",
    0xC0000409: "STATUS_STACK_BUFFER_OVERRUN",
    0xC000041D: "STATUS_FATAL_USER_CALLBACK_EXCEPTION",
}


class EXCEPTION_RECORD(C.Structure):
    _fields_ = [
        ("ExceptionCode", C.c_uint32),
        ("ExceptionFlags", C.c_uint32),
        ("ExceptionRecord", C.c_void_p),
        ("ExceptionAddress", C.c_void_p),
        ("NumberParameters", C.c_uint32),
        ("ExceptionInformation", C.c_ulonglong * 15),
    ]


class EXCEPTION_DEBUG_INFO(C.Structure):
    _fields_ = [("ExceptionRecord", EXCEPTION_RECORD), ("dwFirstChance", C.c_uint32)]


class DEBUG_EVENT_U(C.Union):
    _fields_ = [("Exception", EXCEPTION_DEBUG_INFO), ("raw", C.c_byte * 176)]


class DEBUG_EVENT(C.Structure):
    _fields_ = [
        ("dwDebugEventCode", C.c_uint32),
        ("dwProcessId", C.c_uint32),
        ("dwThreadId", C.c_uint32),
        ("u", DEBUG_EVENT_U),
    ]


class STARTUPINFOW(C.Structure):
    _fields_ = [
        ("cb", C.c_uint32), ("lpReserved", C.c_wchar_p), ("lpDesktop", C.c_wchar_p),
        ("lpTitle", C.c_wchar_p), ("dwX", C.c_uint32), ("dwY", C.c_uint32),
        ("dwXSize", C.c_uint32), ("dwYSize", C.c_uint32), ("dwXCountChars", C.c_uint32),
        ("dwYCountChars", C.c_uint32), ("dwFillAttribute", C.c_uint32),
        ("dwFlags", C.c_uint32), ("wShowWindow", C.c_uint16), ("cbReserved2", C.c_uint16),
        ("lpReserved2", C.c_void_p), ("hStdInput", C.c_void_p),
        ("hStdOutput", C.c_void_p), ("hStdError", C.c_void_p),
    ]


class PROCESS_INFORMATION(C.Structure):
    _fields_ = [("hProcess", C.c_void_p), ("hThread", C.c_void_p),
                ("dwProcessId", C.c_uint32), ("dwThreadId", C.c_uint32)]


class MEMORY_BASIC_INFORMATION(C.Structure):
    _fields_ = [
        ("BaseAddress", C.c_void_p), ("AllocationBase", C.c_void_p),
        ("AllocationProtect", C.c_uint32), ("__pad0", C.c_uint32),
        ("RegionSize", C.c_size_t), ("State", C.c_uint32),
        ("Protect", C.c_uint32), ("Type", C.c_uint32), ("__pad1", C.c_uint32),
    ]


def module_at(h_process, addr):
    """(module path, allocation base) holding `addr` in that process."""
    mbi = MEMORY_BASIC_INFORMATION()
    if not k32.VirtualQueryEx(h_process, C.c_void_p(addr), C.byref(mbi), C.sizeof(mbi)):
        return None, None
    base = mbi.AllocationBase
    if not base:
        return None, None
    buf = C.create_unicode_buffer(32768)
    n = psapi.GetModuleFileNameExW(h_process, C.c_void_p(base), buf, len(buf))
    return (buf.value if n else None), base


def run(path):
    si, pi = STARTUPINFOW(), PROCESS_INFORMATION()
    si.cb = C.sizeof(si)
    ok = k32.CreateProcessW(
        C.c_wchar_p(path), None, None, None, False,
        C.c_uint32(DEBUG_ONLY_THIS_PROCESS | CREATE_NO_WINDOW),
        None, C.c_wchar_p(os.path.dirname(os.path.abspath(path))),
        C.byref(si), C.byref(pi))
    if not ok:
        return f"CreateProcess failed, error {C.get_last_error()}"
    ev = DEBUG_EVENT()
    seen_initial_bp = False
    verdict = None
    # STATUS_FATAL_USER_CALLBACK_EXCEPTION is ntdll reporting that an
    # exception escaped a callback, always at the same dispatcher address:
    # the cause is the first-chance exception before it, so keep those.
    earlier = []
    while True:
        if not k32.WaitForDebugEvent(C.byref(ev), C.c_uint32(60000)):
            verdict = verdict or "no debug event within 60 s"
            k32.TerminateProcess(pi.hProcess, 1)
            break
        code = DBG_CONTINUE
        if ev.dwDebugEventCode == EXCEPTION_DEBUG_EVENT:
            rec = ev.u.Exception.ExceptionRecord
            first = ev.u.Exception.dwFirstChance
            exc, addr = rec.ExceptionCode, rec.ExceptionAddress or 0
            if exc in (STATUS_BREAKPOINT, STATUS_WX86_BREAKPOINT) and not seen_initial_bp:
                seen_initial_bp = True          # the loader's own breakpoint
                code = DBG_CONTINUE
            elif first:
                # Let the program's own handlers have it first, but remember
                # it: a handled one is invisible afterwards.
                mod, base = module_at(pi.hProcess, addr)
                where = f"{os.path.basename(mod)}+0x{addr - base:x}" if mod else f"0x{addr:x}"
                item = f"0x{exc:08x} {NAMES.get(exc, '')} at {where}".rstrip()
                if not earlier or earlier[-1] != item:
                    earlier.append(item)
                code = DBG_EXCEPTION_NOT_HANDLED
            else:
                name = NAMES.get(exc, "")
                mod, base = module_at(pi.hProcess, addr)
                where = "unknown module"
                if mod:
                    where = f"{os.path.basename(mod)}+0x{addr - base:x}"
                extra = ""
                if exc == 0xC0000005 and rec.NumberParameters >= 2:
                    kind = {0: "read", 1: "write", 8: "execute"}.get(
                        rec.ExceptionInformation[0], str(rec.ExceptionInformation[0]))
                    extra = f", {kind} of 0x{rec.ExceptionInformation[1]:x}"
                verdict = (f"0x{exc:08x} {name} at {where} "
                           f"(address 0x{addr:x}{extra})")
                if earlier:
                    verdict += "; first-chance before it: " + " then ".join(earlier[-3:])
                k32.TerminateProcess(pi.hProcess, 1)
                code = DBG_CONTINUE
        elif ev.dwDebugEventCode == EXIT_PROCESS_DEBUG_EVENT:
            k32.ContinueDebugEvent(ev.dwProcessId, ev.dwThreadId, C.c_uint32(DBG_CONTINUE))
            break
        k32.ContinueDebugEvent(ev.dwProcessId, ev.dwThreadId, C.c_uint32(code))
    k32.CloseHandle(pi.hThread)
    k32.CloseHandle(pi.hProcess)
    return verdict or "exited with no fatal exception"


def main(argv):
    if not argv:
        print(__doc__)
        return 2
    for path in argv:
        print(f"{os.path.basename(path):24} {run(path)}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
