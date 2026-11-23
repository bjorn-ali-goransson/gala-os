/* WaitForInputIdle as Windows answers it, measured on Windows 11 24H2,
 * build 26100. Two things it does are in no documentation:
 * learn.microsoft.com ("WaitForInputIdle") says a process with no message
 * queue "returns immediately", and what it does is fail with
 * ERROR_NOT_GUI_PROCESS; and what it calls a GUI process is decided by the
 * subsystem byte in the image's PE header, not by whether the process has
 * a message queue at all. This test is built from one source twice to show
 * that: `waitidle_test.exe` is CUI and `waitidle_child.exe` is GUI, the
 * same code, and they are answered differently for it.
 *
 * Each binary is its own subject, spawned again with one argument:
 *   gui       a top-level window, then GetMessage, so its queue runs dry
 *   slowgui   the same, half a second later, so a call has to wait for it
 *   busy      a top-level window, then a loop that never pumps
 *   idlebusy  a top-level window, a message loop that runs dry, and
 *             then, 300 ms in, the same loop that never pumps again
 *   nowindow  no window and no message queue at all, just a wait
 * Each window is titled after its mode, and the parent waits for the
 * window to appear before asking about that process, so what the answers
 * are about is a fact and not a guess about timing.
 *
 * What the parent measures:
 *
 * 1. A GUI-subsystem child that reaches its message loop is
 *    WAIT_OBJECT_0 (0), and every later call on it is 0 at once. The
 *    first call waits as long as the child takes to get there (1157 ms
 *    for charmap.exe on Windows).
 * 2. The wait is a real wait, not a poll of a flag: `slowgui` asked with
 *    a 0 ms timeout is WAIT_TIMEOUT (0x102) and asked again with a long
 *    one is 0, having waited the half second the child sleeps out
 *    (526 ms measured).
 * 3. A GUI-subsystem child with a queue that never pumps (`busy`) is
 *    WAIT_TIMEOUT, and so is one that never makes a queue at all
 *    (`nowindow`) -- both wait the timeout out rather than failing. The
 *    wait overshoots what was asked (1521 ms for 1000, 2504 ms for 2000),
 *    so the test asks only that it waited at least as long as it was told.
 * 4. A CUI-subsystem process is WAIT_FAILED (0xffffffff) with
 *    ERROR_NOT_GUI_PROCESS (1471) at once, *even with its window up and
 *    its queue being pumped* -- which is what shows the subsystem is what
 *    decides. There is no settling time to wait out either: three fresh
 *    console children asked the instant they were spawned all answered
 *    1471 in 0 ms.
 * 5. NULL and a handle that is no handle are WAIT_FAILED with
 *    ERROR_INVALID_HANDLE (6).
 * 6. The pseudo handle for the current process (-1) is WAIT_FAILED and
 *    the pseudo handle for the current thread (-2) is WAIT_FAILED with
 *    ERROR_INVALID_PARAMETER (87), the one place this function tells the
 *    two pseudo handles apart. Which error the current process leaves is
 *    the caller's business and not the argument's -- 1471 asked from this
 *    program, nothing at all asked from a PowerShell host -- so the test
 *    asserts the failure and prints the error.
 * 7. The right the handle needs is a query right, not SYNCHRONIZE:
 *    PROCESS_QUERY_INFORMATION alone and PROCESS_QUERY_LIMITED_INFORMATION
 *    alone both answer 0, while SYNCHRONIZE alone and PROCESS_VM_READ
 *    alone are WAIT_FAILED with ERROR_INVALID_HANDLE -- a missing right
 *    reads as a bad handle, not as ERROR_ACCESS_DENIED.
 * 8. A child that has ended is WAIT_FAILED.
 * 9. The answer is a latch and not a reading: `idlebusy` reaches its
 *    message loop, goes idle there, and then spins for good without
 *    pumping, and it is still 0 -- asked with a 0 ms timeout, so no
 *    waiting can hide it. A process is "input idle" from the first time
 *    it waits on an empty queue and stays so, which is what
 *    learn.microsoft.com means by waiting only once for a process. (A
 *    `busy` child, which never pumped at all, is where the other side of
 *    that latch is measured: never set, so WAIT_TIMEOUT.)
 * 10. Which answers touch the last error, which a sentinel (12345, set
 *    before every call and read back after) is what proves: 0,
 *    WAIT_TIMEOUT, the ended child and the current-process pseudo handle
 *    all leave it exactly as it was, so a caller cannot tell those two
 *    failures apart by GetLastError at all; only the bad handle (6), the
 *    thread pseudo handle (87) and the CUI process (1471) set one.
 */

#define _WIN32_WINNT 0x0A00
#include <windows.h>
#include <stdio.h>
#include <string.h>
#include <wchar.h>

/* The program's name in its output, and the GUI-subsystem binary built
 * from this same source (build.sh sets both for the x86 twins). */
#ifndef NAME
#define NAME "waitidle_test"
#endif
#ifndef CHILD
#define CHILD L"waitidle_child.exe"
#endif

static int check;
#define CHECK(cond) do { check++; if (!(cond)) { printf(NAME ": check %d (line %d) failed, error %lu\n", check, __LINE__, GetLastError()); fflush(stdout); ExitProcess(check); } } while (0)

/* Set before every call and read back after it: an answer that leaves it
 * there is one that touched no error (9). */
#define SENTINEL 12345u

#define WAIT_FAILED_U 0xffffffffu

static DWORD call(HANDLE p, DWORD ms, DWORD *err, DWORD *elapsed) {
    SetLastError(SENTINEL);
    ULONGLONG start = GetTickCount64();
    DWORD r = WaitForInputIdle(p, ms);
    DWORD e = GetLastError();
    if (elapsed)
        *elapsed = (DWORD)(GetTickCount64() - start);
    if (err)
        *err = e;
    return r;
}

static const wchar_t *CLASS_NAME = L"waitidle_test_child";

static LRESULT CALLBACK wndproc(HWND h, UINT m, WPARAM w, LPARAM l) {
    return DefWindowProcW(h, m, w, l);
}

/* A child with a message queue, titled after its mode so the parent can
 * see it is up. `slow` delays making it; `busy` never pumps, so the queue
 * exists and the thread is never waiting on it; `then_busy` pumps first
 * and stops pumping when its own timer arrives, so the process has been
 * idle and is not any more. */
static int child_window(const wchar_t *title, int slow, int busy, int then_busy) {
    if (slow)
        Sleep(500);
    WNDCLASSEXW wc;
    ZeroMemory(&wc, sizeof wc);
    wc.cbSize = sizeof wc;
    wc.lpfnWndProc = wndproc;
    wc.hInstance = GetModuleHandleW(NULL);
    wc.lpszClassName = CLASS_NAME;
    if (RegisterClassExW(&wc) == 0)
        return 101;
    HWND h = CreateWindowExW(0, CLASS_NAME, title, WS_OVERLAPPEDWINDOW,
                             40, 40, 200, 120, NULL, NULL, wc.hInstance, NULL);
    if (h == NULL)
        return 102;
    if (busy) {
        /* A queue, and a thread that is never waiting on it. */
        for (;;)
            Sleep(10);
    }
    if (then_busy && SetTimer(h, 1, 300, NULL) == 0)
        return 103;
    MSG msg;
    while (GetMessageW(&msg, NULL, 0, 0) > 0) {
        if (then_busy && msg.message == WM_TIMER) {
            /* Idle once (the wait for this timer), then never again. */
            KillTimer(h, 1);
            for (;;)
                Sleep(10);
        }
        TranslateMessage(&msg);
        DispatchMessageW(&msg);
    }
    return 0;
}

/* The path of a binary beside this one. */
static void beside(const wchar_t *name, wchar_t *out, size_t n) {
    wchar_t self[MAX_PATH];
    CHECK(GetModuleFileNameW(NULL, self, MAX_PATH) > 0);
    wchar_t *slash = wcsrchr(self, L'\\');
    CHECK(slash != NULL);
    slash[1] = 0;
    CHECK(wcslen(self) + wcslen(name) < n);
    wcscpy(out, self);
    wcscat(out, name);
}

/* Spawn `exe` (NULL for this binary) in `mode`. */
static HANDLE spawn(const wchar_t *exe, const wchar_t *mode, DWORD *pid) {
    wchar_t path[MAX_PATH];
    if (exe != NULL)
        beside(exe, path, MAX_PATH);
    else
        CHECK(GetModuleFileNameW(NULL, path, MAX_PATH) > 0);
    wchar_t cmd[MAX_PATH + 64];
    _snwprintf(cmd, MAX_PATH + 63, L"\"%s\" %s", path, mode);
    cmd[MAX_PATH + 63] = 0;
    STARTUPINFOW si;
    ZeroMemory(&si, sizeof si);
    si.cb = sizeof si;
    PROCESS_INFORMATION pi;
    ZeroMemory(&pi, sizeof pi);
    CHECK(CreateProcessW(NULL, cmd, NULL, NULL, FALSE, 0, NULL, NULL, &si, &pi));
    CHECK(CloseHandle(pi.hThread));
    if (pid)
        *pid = pi.dwProcessId;
    return pi.hProcess;
}

/* Wait for a child's window to be there, so an answer about that process
 * is an answer about a process with a queue (4). */
static void window_is_up(const wchar_t *title) {
    for (int i = 0; i < 100; i++) {
        if (FindWindowW(CLASS_NAME, title) != NULL)
            return;
        Sleep(50);
    }
    CHECK(!"the child's window never appeared");
}

static void kill_child(HANDLE p) {
    TerminateProcess(p, 0);
    WaitForSingleObject(p, 5000);
    CHECK(CloseHandle(p));
}

/* WaitForInputIdle through a handle that carries `access` alone (7). */
static void with_access(DWORD pid, const char *what, DWORD access, DWORD want, DWORD want_err) {
    HANDLE h = OpenProcess(access, FALSE, pid);
    CHECK(h != NULL);
    DWORD err = 0;
    DWORD r = call(h, 2000, &err, NULL);
    printf(NAME ": %s: ret 0x%08lx, error %lu\n", what, r, err);
    CHECK(r == want);
    CHECK(err == want_err);
    CHECK(CloseHandle(h));
}

int main(int argc, char **argv) {
    if (argc > 1) {
        if (strcmp(argv[1], "gui") == 0)
            return child_window(L"gui", 0, 0, 0);
        if (strcmp(argv[1], "slowgui") == 0)
            return child_window(L"slowgui", 1, 0, 0);
        if (strcmp(argv[1], "busy") == 0)
            return child_window(L"busy", 0, 1, 0);
        if (strcmp(argv[1], "idlebusy") == 0)
            return child_window(L"idlebusy", 0, 0, 1);
        if (strcmp(argv[1], "nowindow") == 0) {
            Sleep(30000);
            return 0;
        }
        return 100;
    }

    DWORD err = 0, elapsed = 0, r = 0;

    /* 1. A GUI-subsystem child that reaches its message loop. */
    DWORD gui_pid = 0;
    HANDLE gui = spawn(CHILD, L"gui", &gui_pid);
    r = call(gui, 10000, &err, &elapsed);
    printf(NAME ": gui child: ret 0x%08lx, error %lu, %lu ms\n", r, err, elapsed);
    CHECK(r == 0);
    CHECK(err == SENTINEL);
    r = call(gui, 10000, &err, NULL);
    CHECK(r == 0 && err == SENTINEL);
    r = call(gui, 0, &err, NULL);
    CHECK(r == 0 && err == SENTINEL);

    /* 2. The wait is a wait: the child is not there yet at 0 ms, and a
     * long timeout waits the half second out. */
    HANDLE slow = spawn(CHILD, L"slowgui", NULL);
    r = call(slow, 0, &err, NULL);
    printf(NAME ": slowgui at 0 ms: ret 0x%08lx, error %lu\n", r, err);
    CHECK(r == WAIT_TIMEOUT);
    CHECK(err == SENTINEL);
    r = call(slow, 10000, &err, &elapsed);
    printf(NAME ": slowgui waited: ret 0x%08lx, error %lu, %lu ms\n", r, err, elapsed);
    CHECK(r == 0);
    CHECK(err == SENTINEL);
    CHECK(elapsed >= 300);
    kill_child(slow);

    /* 3. A queue nobody pumps, and no queue at all: both wait it out. */
    HANDLE busy = spawn(CHILD, L"busy", NULL);
    window_is_up(L"busy");
    r = call(busy, 500, &err, &elapsed);
    printf(NAME ": busy child: ret 0x%08lx, error %lu, %lu ms\n", r, err, elapsed);
    CHECK(r == WAIT_TIMEOUT);
    CHECK(err == SENTINEL);
    CHECK(elapsed >= 500);
    kill_child(busy);

    HANDLE quiet = spawn(CHILD, L"nowindow", NULL);
    r = call(quiet, 500, &err, &elapsed);
    printf(NAME ": no window at all: ret 0x%08lx, error %lu, %lu ms\n", r, err, elapsed);
    CHECK(r == WAIT_TIMEOUT);
    CHECK(err == SENTINEL);
    CHECK(elapsed >= 500);
    kill_child(quiet);

    /* 9. The latch: idle once, then busy for good, and still 0 -- with a
     * 0 ms timeout, so the answer cannot be a wait that happened to end. */
    HANDLE latch = spawn(CHILD, L"idlebusy", NULL);
    window_is_up(L"idlebusy");
    r = call(latch, 10000, &err, &elapsed);
    printf(NAME ": idlebusy, first call: ret 0x%08lx, error %lu, %lu ms\n", r, err, elapsed);
    CHECK(r == 0);
    CHECK(err == SENTINEL);
    /* Past its timer, so the child is spinning and pumping nothing. */
    Sleep(700);
    r = call(latch, 0, &err, &elapsed);
    printf(NAME ": idlebusy, busy since: ret 0x%08lx, error %lu, %lu ms\n", r, err, elapsed);
    CHECK(r == 0);
    CHECK(err == SENTINEL);
    kill_child(latch);

    /* 4. The subsystem is what decides: this binary is CUI, and a CUI
     * process with its window up and its queue being pumped is still
     * ERROR_NOT_GUI_PROCESS. */
    HANDLE cui = spawn(NULL, L"gui", NULL);
    window_is_up(L"gui");
    r = call(cui, 2000, &err, &elapsed);
    printf(NAME ": CUI child with a window up: ret 0x%08lx, error %lu, %lu ms\n", r, err, elapsed);
    CHECK(r == WAIT_FAILED_U);
    CHECK(err == ERROR_NOT_GUI_PROCESS);
    kill_child(cui);

    /* 5. Handles that are not handles. */
    r = call(NULL, 100, &err, NULL);
    CHECK(r == WAIT_FAILED_U);
    CHECK(err == ERROR_INVALID_HANDLE);
    r = call((HANDLE)(ULONG_PTR)0x1234, 100, &err, NULL);
    CHECK(r == WAIT_FAILED_U);
    CHECK(err == ERROR_INVALID_HANDLE);

    /* 6. The two pseudo handles, which this function tells apart. The
     * current process always fails, but which error it leaves depends on
     * the caller and not on the argument, so only the failure is asserted:
     * from this program (CUI, no queue of its own) it is 1471, and from a
     * PowerShell host it left the sentinel standing. */
    r = call(GetCurrentProcess(), 100, &err, NULL);
    printf(NAME ": current process: ret 0x%08lx, error %lu\n", r, err);
    CHECK(r == WAIT_FAILED_U);
    r = call(GetCurrentThread(), 100, &err, NULL);
    printf(NAME ": current thread: ret 0x%08lx, error %lu\n", r, err);
    CHECK(r == WAIT_FAILED_U);
    CHECK(err == ERROR_INVALID_PARAMETER);

    /* 7. A query right is what it wants. */
    with_access(gui_pid, "SYNCHRONIZE only", SYNCHRONIZE, WAIT_FAILED_U, ERROR_INVALID_HANDLE);
    with_access(gui_pid, "PROCESS_VM_READ only", PROCESS_VM_READ, WAIT_FAILED_U, ERROR_INVALID_HANDLE);
    with_access(gui_pid, "PROCESS_QUERY_INFORMATION only", PROCESS_QUERY_INFORMATION, 0, SENTINEL);
    with_access(gui_pid, "PROCESS_QUERY_LIMITED_INFORMATION only", PROCESS_QUERY_LIMITED_INFORMATION, 0, SENTINEL);

    /* 8 and 9. A child that has ended fails and says nothing about why. */
    kill_child(gui);
    HANDLE ended = spawn(CHILD, L"gui", NULL);
    /* Idle first, so this is the subject measured on Windows: a process
     * that had a queue and does not any more. */
    CHECK(call(ended, 10000, &err, NULL) == 0);
    TerminateProcess(ended, 0);
    CHECK(WaitForSingleObject(ended, 5000) == WAIT_OBJECT_0);
    r = call(ended, 100, &err, NULL);
    printf(NAME ": ended child: ret 0x%08lx, error %lu\n", r, err);
    CHECK(r == WAIT_FAILED_U);
    CHECK(err == SENTINEL);
    CHECK(CloseHandle(ended));

    printf(NAME ": ok\n");
    return 0;
}
