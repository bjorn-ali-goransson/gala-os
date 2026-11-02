/* WinEvents (SetWinEventHook, UnhookWinEvent, NotifyWinEvent), with the
 * results Windows documents (and Wine's conformance tests, validated on
 * Windows, check).
 *
 * - SetWinEventHook without a callback fails with
 *   ERROR_INVALID_FILTER_PROC; WINEVENT_INCONTEXT without the module of
 *   the callback's DLL fails with ERROR_HOOK_NEEDS_HMOD; UnhookWinEvent of
 *   no hook fails with ERROR_INVALID_HANDLE.
 * - An out-of-context hook (WINEVENT_OUTOFCONTEXT) of this thread for
 *   EVENT_OBJECT_NAMECHANGE only: SetWindowTextW on our window raises it
 *   (DefWindowProc's WM_SETTEXT), with OBJID_WINDOW, CHILDID_SELF and our
 *   thread as idEventThread; the callback runs while the thread reads its
 *   messages (PeekMessage). NotifyWinEvent's own event arrives with its
 *   idObject and idChild. Events outside the hook's range do not
 *   (EVENT_OBJECT_FOCUS raised with NotifyWinEvent). After UnhookWinEvent
 *   (TRUE; again: FALSE) nothing more arrives.
 * - WINEVENT_SKIPOWNPROCESS: our own events are not delivered.
 * - Across processes: a hook for EVENT_OBJECT_CREATE..EVENT_OBJECT_NAMECHANGE
 *   of every process but ours sees the child process (this program with
 *   the argument "child") create its window (EVENT_OBJECT_CREATE), show
 *   it (EVENT_OBJECT_SHOW), rename it (EVENT_OBJECT_NAMECHANGE), and
 *   destroy it (EVENT_OBJECT_HIDE, then EVENT_OBJECT_DESTROY), all with
 *   OBJID_WINDOW, CHILDID_SELF and the child's thread as idEventThread, in
 *   that order; the child's button is created too (EVENT_OBJECT_CREATE for
 *   another window of the child). The child's exit code is its window's
 *   handle (HWNDs fit in 32 bits).
 *
 * Prints "winevent_test: ok" and returns 0, or the number of the failing
 * check. Built by build.sh with mingw-w64. */
#include <windows.h>
#include <stdio.h>
#include <string.h>

typedef struct {
    DWORD event;
    HWND hwnd;
    LONG object;
    LONG child;
    DWORD thread;
} Seen;

static Seen seen[512];
static int nseen;

static void CALLBACK proc(HWINEVENTHOOK hook, DWORD event, HWND hwnd, LONG object, LONG child, DWORD thread,
                          DWORD time) {
    (void)hook;
    (void)time;
    if (nseen < 512) {
        Seen s = {event, hwnd, object, child, thread};
        seen[nseen++] = s;
    }
}

/* Reads the thread's messages for `ms` milliseconds (the callbacks run). */
static void pump(DWORD ms) {
    DWORD start = GetTickCount();
    do {
        MSG m;
        while (PeekMessageW(&m, NULL, 0, 0, PM_REMOVE)) {
            TranslateMessage(&m);
            DispatchMessageW(&m);
        }
        MsgWaitForMultipleObjects(0, NULL, FALSE, 20, QS_ALLINPUT);
    } while (GetTickCount() - start < ms);
}

/* Reads messages until `n` events were seen or `ms` passed. */
static void pump_until(int n, DWORD ms) {
    DWORD start = GetTickCount();
    while (nseen < n && GetTickCount() - start < ms)
        pump(20);
}

static int child(void) {
    WNDCLASSW wc = {0};
    wc.lpfnWndProc = DefWindowProcW;
    wc.hInstance = GetModuleHandleW(NULL);
    wc.lpszClassName = L"GalaWinEventChild";
    RegisterClassW(&wc);
    HWND top = CreateWindowExW(0, L"GalaWinEventChild", L"winevent child", WS_OVERLAPPEDWINDOW | WS_VISIBLE, 50,
                               50, 300, 200, NULL, NULL, wc.hInstance, NULL);
    if (!top)
        return 0;
    CreateWindowExW(0, L"BUTTON", L"Press", WS_CHILD | WS_VISIBLE | BS_PUSHBUTTON, 10, 10, 80, 24, top,
                    (HMENU)101, wc.hInstance, NULL);
    pump(100);
    SetWindowTextW(top, L"renamed");
    pump(100);
    DestroyWindow(top);
    pump(100);
    return (int)(UINT_PTR)top;
}

/* The index of the first event `event` for `hwnd` (OBJID_WINDOW,
 * CHILDID_SELF, thread `thread`) at or after `from`, or -1. */
static int find(DWORD event, HWND hwnd, DWORD thread, int from) {
    for (int i = from; i < nseen; i++)
        if (seen[i].event == event && seen[i].hwnd == hwnd && seen[i].object == OBJID_WINDOW &&
            seen[i].child == CHILDID_SELF && seen[i].thread == thread)
            return i;
    return -1;
}

int main(int argc, char **argv) {
    if (argc > 1 && strcmp(argv[1], "child") == 0)
        return child();

    /* Refused hooks and handles. */
    SetLastError(0xdeadbeef);
    HWINEVENTHOOK h = SetWinEventHook(EVENT_MIN, EVENT_MAX, NULL, NULL, 0, 0, WINEVENT_OUTOFCONTEXT);
    if (h || GetLastError() != ERROR_INVALID_FILTER_PROC) {
        printf("winevent_test: no callback: %p %lu\n", (void *)h, GetLastError());
        return 1;
    }
    SetLastError(0xdeadbeef);
    h = SetWinEventHook(EVENT_MIN, EVENT_MAX, NULL, proc, 0, 0, WINEVENT_INCONTEXT);
    if (h || GetLastError() != ERROR_HOOK_NEEDS_HMOD) {
        printf("winevent_test: in context without a module: %p %lu\n", (void *)h, GetLastError());
        return 2;
    }
    SetLastError(0xdeadbeef);
    if (UnhookWinEvent(NULL) || GetLastError() != ERROR_INVALID_HANDLE) {
        printf("winevent_test: unhook of nothing: %lu\n", GetLastError());
        return 3;
    }

    /* Our own window, our own hook. */
    WNDCLASSW wc = {0};
    wc.lpfnWndProc = DefWindowProcW;
    wc.hInstance = GetModuleHandleW(NULL);
    wc.lpszClassName = L"GalaWinEventTest";
    RegisterClassW(&wc);
    HWND w = CreateWindowExW(0, L"GalaWinEventTest", L"winevent", WS_OVERLAPPEDWINDOW, 10, 10, 200, 100, NULL, NULL,
                             wc.hInstance, NULL);
    if (!w)
        return 4;
    HWINEVENTHOOK names = SetWinEventHook(EVENT_OBJECT_NAMECHANGE, EVENT_OBJECT_NAMECHANGE, NULL, proc, 0, 0,
                                          WINEVENT_OUTOFCONTEXT);
    if (!names) {
        printf("winevent_test: hook: %lu\n", GetLastError());
        return 5;
    }
    SetWindowTextW(w, L"renamed");
    pump_until(1, 3000);
    printf("winevent_test: own name change: %d events\n", nseen);
    if (find(EVENT_OBJECT_NAMECHANGE, w, GetCurrentThreadId(), 0) < 0)
        return 6;
    nseen = 0;
    NotifyWinEvent(EVENT_OBJECT_FOCUS, w, OBJID_CLIENT, CHILDID_SELF); /* outside the range */
    NotifyWinEvent(EVENT_OBJECT_NAMECHANGE, w, OBJID_CLIENT, 5);
    pump_until(1, 3000);
    pump(100);
    if (nseen != 1 || seen[0].event != EVENT_OBJECT_NAMECHANGE || seen[0].hwnd != w ||
        seen[0].object != OBJID_CLIENT || seen[0].child != 5 || seen[0].thread != GetCurrentThreadId()) {
        printf("winevent_test: NotifyWinEvent: %d events, first %lx %ld %ld\n", nseen,
               nseen ? seen[0].event : 0, nseen ? seen[0].object : 0, nseen ? seen[0].child : 0);
        return 7;
    }
    if (!UnhookWinEvent(names))
        return 8;
    SetLastError(0xdeadbeef);
    if (UnhookWinEvent(names) || GetLastError() != ERROR_INVALID_HANDLE)
        return 9;
    nseen = 0;
    SetWindowTextW(w, L"again");
    pump(200);
    if (nseen != 0) {
        printf("winevent_test: %d events after UnhookWinEvent\n", nseen);
        return 10;
    }
    printf("winevent_test: own events ok\n");

    /* Every process but ours. */
    HWINEVENTHOOK others = SetWinEventHook(EVENT_OBJECT_CREATE, EVENT_OBJECT_NAMECHANGE, NULL, proc, 0, 0,
                                           WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS);
    if (!others)
        return 11;
    SetWindowTextW(w, L"skipped");
    pump(200);
    if (nseen != 0) {
        printf("winevent_test: own event with WINEVENT_SKIPOWNPROCESS\n");
        return 12;
    }
    wchar_t path[MAX_PATH], cmd[MAX_PATH + 16];
    GetModuleFileNameW(NULL, path, MAX_PATH);
    swprintf(cmd, MAX_PATH + 16, L"\"%ls\" child", path);
    STARTUPINFOW si = {sizeof si};
    PROCESS_INFORMATION pi;
    if (!CreateProcessW(NULL, cmd, NULL, NULL, FALSE, 0, NULL, NULL, &si, &pi))
        return 13;
    DWORD start = GetTickCount();
    while (WaitForSingleObject(pi.hProcess, 0) == WAIT_TIMEOUT && GetTickCount() - start < 30000)
        pump(20);
    DWORD code = 0;
    GetExitCodeProcess(pi.hProcess, &code);
    HWND top = (HWND)(UINT_PTR)code;
    /* Its last events. */
    pump(300);
    DWORD t = pi.dwThreadId;
    int create = find(EVENT_OBJECT_CREATE, top, t, 0);
    int show = find(EVENT_OBJECT_SHOW, top, t, 0);
    int name = show < 0 ? -1 : find(EVENT_OBJECT_NAMECHANGE, top, t, show);
    int hide = name < 0 ? -1 : find(EVENT_OBJECT_HIDE, top, t, name);
    int destroy = hide < 0 ? -1 : find(EVENT_OBJECT_DESTROY, top, t, hide);
    int button = -1;
    for (int i = 0; i < nseen; i++)
        if (seen[i].event == EVENT_OBJECT_CREATE && seen[i].hwnd != top && seen[i].thread == t)
            button = i;
    printf("winevent_test: child %p: %d events; create %d show %d name %d hide %d destroy %d button %d\n",
           (void *)top, nseen, create, show, name, hide, destroy, button);
    if (!top || create < 0 || show < create || name < 0 || hide < 0 || destroy < 0 || button < 0)
        return 14;
    if (!UnhookWinEvent(others))
        return 15;
    CloseHandle(pi.hProcess);
    CloseHandle(pi.hThread);
    DestroyWindow(w);
    printf("winevent_test: ok\n");
    return 0;
}
