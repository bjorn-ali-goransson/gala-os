/* user32 as real Windows behaves (documented behaviour only), for what
 * wxWidgets (FileZilla) uses, added as FileZilla's missing-features
 * ledger names it:
 *
 * - Hooks: SetWindowsHookExW for the calling thread returns a handle;
 *   the newest hook is called first and CallNextHookEx passes on to the
 *   older one (0 after the last). WH_CBT: HCBT_CREATEWND before the
 *   window gets any message (wParam the new window, lParam a
 *   CBT_CREATEWND whose CREATESTRUCT has the title); a nonzero return
 *   makes CreateWindowEx fail; HCBT_DESTROYWND before WM_DESTROY.
 *   WH_GETMESSAGE: called by GetMessage/PeekMessage with HC_ACTION,
 *   wParam PM_REMOVE or PM_NOREMOVE and the MSG, which the hook may
 *   change. UnhookWindowsHookEx returns TRUE, after which the hook is no
 *   longer called; unhooking again fails.
 * - Timers: SetTimer for a window returns its id; WM_TIMER (wParam the
 *   id) comes no sooner than the interval, after posted messages;
 *   KillTimer returns TRUE once. A window-less timer with a TIMERPROC
 *   gets a new id; its WM_TIMER carries the procedure in lParam and
 *   DispatchMessage calls it.
 * - The display: EnumDisplayMonitors calls back once (one monitor) with
 *   the screen's rectangle and the data; MonitorFromPoint finds it for a
 *   point on the screen, and for one off it only by the default flags;
 *   GetMonitorInfoW gives rcMonitor (the size GetSystemMetrics reports),
 *   a work area within it, MONITORINFOF_PRIMARY and (EX) a \\.\DISPLAY
 *   device name; MonitorFromWindow finds it for a window on it.
 * - shell32's SetCurrentProcessExplicitAppUserModelID: S_OK.
 * - kernel32's SetThreadLocale: GetThreadLocale then returns it (per
 *   thread); an LCID without a language fails with
 *   ERROR_INVALID_PARAMETER.
 * - advapi32's SetSecurityDescriptorControl: the inheritance bits
 *   (SE_DACL_PROTECTED here) change, others are ERROR_INVALID_PARAMETER;
 *   SetSecurityDescriptorSacl sets SE_SACL_PRESENT; SetSecurityDescriptorGroup
 *   sets the group GetSecurityDescriptorGroup returns (SE_GROUP_DEFAULTED).
 * - ole32's OleInitialize (wx initializes OLE first): S_OK, then S_FALSE
 *   on the same thread; OleUninitialize once per call.
 *
 * Prints "user32_test: ok" or exits with the number of the failing
 * check. */
#include <windows.h>
#include <ole2.h>
#include <shobjidl.h>
#include <stdio.h>
#include <string.h>

static int check;
#define CHECK(cond) do { check++; if (!(cond)) { printf("user32_test: check %d (line %d) failed\n", check, __LINE__); fflush(stdout); ExitProcess(check); } } while (0)

static HHOOK cbt_hook;
static int cbt_creates, cbt_destroys, cbt_refuse, wndproc_messages;
static HWND cbt_created, cbt_destroyed;
static int title_seen;

static LRESULT CALLBACK cbt(int code, WPARAM wparam, LPARAM lparam) {
    if (code == HCBT_CREATEWND) {
        cbt_creates++;
        cbt_created = (HWND)wparam;
        CBT_CREATEWNDW *c = (CBT_CREATEWNDW *)lparam;
        if (c && c->lpcs && c->lpcs->lpszName && wcscmp(c->lpcs->lpszName, L"hooked") == 0)
            title_seen = 1;
        /* No message reached the window yet. */
        if (wndproc_messages != 0)
            title_seen = -1;
        if (cbt_refuse)
            return 1;
    } else if (code == HCBT_DESTROYWND) {
        cbt_destroys++;
        cbt_destroyed = (HWND)wparam;
    }
    return CallNextHookEx(cbt_hook, code, wparam, lparam);
}

static LRESULT CALLBACK wndproc(HWND hwnd, UINT msg, WPARAM wparam, LPARAM lparam) {
    wndproc_messages++;
    return DefWindowProcW(hwnd, msg, wparam, lparam);
}

static HHOOK first_hook, second_hook;
static int first_calls, second_calls, last_flag;
static UINT last_message;

static LRESULT CALLBACK getmsg_first(int code, WPARAM wparam, LPARAM lparam) {
    first_calls++;
    MSG *m = (MSG *)lparam;
    if (code == HC_ACTION && m->message == WM_USER + 1)
        m->wParam = 9; /* the hook may change the message */
    return CallNextHookEx(first_hook, code, wparam, lparam);
}

static LRESULT CALLBACK getmsg_second(int code, WPARAM wparam, LPARAM lparam) {
    second_calls++;
    last_flag = (int)wparam;
    last_message = ((MSG *)lparam)->message;
    /* Called before the older hook. */
    if (first_calls >= second_calls)
        second_calls = -1000;
    return CallNextHookEx(second_hook, code, wparam, lparam);
}

static void hooks(void) {
    WNDCLASSW wc = {0};
    wc.lpfnWndProc = wndproc;
    wc.hInstance = GetModuleHandleW(NULL);
    wc.lpszClassName = L"User32TestHooks";
    CHECK(RegisterClassW(&wc) != 0);

    cbt_hook = SetWindowsHookExW(WH_CBT, cbt, NULL, GetCurrentThreadId());
    CHECK(cbt_hook != NULL);
    HWND w = CreateWindowExW(0, L"User32TestHooks", L"hooked", WS_OVERLAPPEDWINDOW, 10, 10,
                             200, 100, NULL, NULL, wc.hInstance, NULL);
    CHECK(w != NULL && cbt_creates == 1 && cbt_created == w && title_seen == 1);
    cbt_refuse = 1;
    HWND refused = CreateWindowExW(0, L"User32TestHooks", L"no", WS_OVERLAPPEDWINDOW, 10, 10,
                                   200, 100, NULL, NULL, wc.hInstance, NULL);
    CHECK(refused == NULL && cbt_creates == 2);
    cbt_refuse = 0;

    first_hook = SetWindowsHookExW(WH_GETMESSAGE, getmsg_first, NULL, GetCurrentThreadId());
    second_hook = SetWindowsHookExW(WH_GETMESSAGE, getmsg_second, NULL, GetCurrentThreadId());
    CHECK(first_hook != NULL && second_hook != NULL && first_hook != second_hook);
    CHECK(PostMessageW(w, WM_USER + 1, 5, 6));
    MSG m;
    CHECK(PeekMessageW(&m, w, WM_USER + 1, WM_USER + 1, PM_NOREMOVE));
    CHECK(second_calls == 1 && first_calls == 1 && last_flag == PM_NOREMOVE);
    CHECK(GetMessageW(&m, w, WM_USER + 1, WM_USER + 1) == 1);
    CHECK(m.message == WM_USER + 1 && m.wParam == 9 && m.lParam == 6);
    CHECK(second_calls == 2 && first_calls == 2 && last_flag == PM_REMOVE);
    CHECK(last_message == WM_USER + 1);
    CHECK(UnhookWindowsHookEx(second_hook));
    CHECK(!UnhookWindowsHookEx(second_hook));
    CHECK(PostMessageW(w, WM_USER + 1, 5, 6));
    CHECK(GetMessageW(&m, w, WM_USER + 1, WM_USER + 1) == 1 && m.wParam == 9);
    CHECK(second_calls == 2 && first_calls == 3);
    CHECK(UnhookWindowsHookEx(first_hook));

    CHECK(DestroyWindow(w));
    CHECK(cbt_destroys == 1 && cbt_destroyed == w);
    CHECK(UnhookWindowsHookEx(cbt_hook));
    /* With no hooks, CallNextHookEx returns 0. */
    CHECK(CallNextHookEx(NULL, HC_ACTION, 0, 0) == 0);
}

static void ole(void) {
    CHECK(OleInitialize(NULL) == S_OK);
    CHECK(OleInitialize(NULL) == S_FALSE);
    OleUninitialize();
    OleUninitialize();
}

static int timer_proc_calls;
static UINT_PTR timer_proc_id;
static void CALLBACK timer_proc(HWND hwnd, UINT msg, UINT_PTR id, DWORD time) {
    if (hwnd == NULL && msg == WM_TIMER && id == timer_proc_id && time != 0)
        timer_proc_calls++;
}

static void timers(void) {
    HWND w = CreateWindowExW(0, L"User32TestHooks", L"timers", WS_OVERLAPPEDWINDOW, 10, 10,
                             200, 100, NULL, NULL, GetModuleHandleW(NULL), NULL);
    CHECK(w != NULL);
    DWORD start = GetTickCount();
    CHECK(SetTimer(w, 7, 50, NULL) == 7);
    CHECK(PostMessageW(w, WM_USER + 2, 0, 0));
    MSG m;
    CHECK(GetMessageW(&m, w, 0, 0) == 1 && m.message == WM_USER + 2);
    CHECK(GetMessageW(&m, w, 0, 0) == 1 && m.message == WM_TIMER && m.wParam == 7 && m.hwnd == w);
    CHECK(GetTickCount() - start >= 40);
    CHECK(KillTimer(w, 7) && !KillTimer(w, 7));

    timer_proc_id = SetTimer(NULL, 0, 20, timer_proc);
    CHECK(timer_proc_id != 0);
    CHECK(GetMessageW(&m, NULL, WM_TIMER, WM_TIMER) == 1 && m.hwnd == NULL);
    CHECK(m.wParam == timer_proc_id && m.lParam == (LPARAM)timer_proc);
    DispatchMessageW(&m);
    CHECK(timer_proc_calls == 1);
    CHECK(KillTimer(NULL, timer_proc_id));
    CHECK(DestroyWindow(w));
}

static int monitors_seen;
static HMONITOR monitor_seen;
static RECT monitor_rect;
static BOOL CALLBACK monitor_proc(HMONITOR m, HDC dc, LPRECT r, LPARAM data) {
    if (data == 42 && dc == NULL) {
        monitors_seen++;
        monitor_seen = m;
        monitor_rect = *r;
    }
    return TRUE;
}

static void monitors(void) {
    int cx = GetSystemMetrics(SM_CXSCREEN), cy = GetSystemMetrics(SM_CYSCREEN);
    CHECK(cx > 0 && cy > 0 && GetSystemMetrics(SM_CMONITORS) == 1);
    CHECK(EnumDisplayMonitors(NULL, NULL, monitor_proc, 42));
    CHECK(monitors_seen == 1 && monitor_seen != NULL);
    CHECK(monitor_rect.left == 0 && monitor_rect.top == 0 && monitor_rect.right == cx
          && monitor_rect.bottom == cy);
    POINT on = {10, 10}, off = {-5000, -5000};
    CHECK(MonitorFromPoint(on, MONITOR_DEFAULTTONULL) == monitor_seen);
    CHECK(MonitorFromPoint(off, MONITOR_DEFAULTTONULL) == NULL);
    CHECK(MonitorFromPoint(off, MONITOR_DEFAULTTOPRIMARY) == monitor_seen);
    MONITORINFOEXW mi;
    mi.cbSize = sizeof(MONITORINFO);
    CHECK(GetMonitorInfoW(monitor_seen, (MONITORINFO *)&mi));
    CHECK(mi.rcMonitor.right == cx && mi.rcMonitor.bottom == cy && (mi.dwFlags & MONITORINFOF_PRIMARY));
    CHECK(mi.rcWork.left >= 0 && mi.rcWork.top >= 0 && mi.rcWork.right <= cx && mi.rcWork.bottom <= cy
          && mi.rcWork.bottom > 0);
    mi.cbSize = sizeof mi;
    CHECK(GetMonitorInfoW(monitor_seen, (MONITORINFO *)&mi) && wcsncmp(mi.szDevice, L"\\\\.\\DISPLAY", 11) == 0);
    HWND w = CreateWindowExW(0, L"User32TestHooks", L"monitors", WS_OVERLAPPEDWINDOW, 10, 10,
                             200, 100, NULL, NULL, GetModuleHandleW(NULL), NULL);
    CHECK(MonitorFromWindow(w, MONITOR_DEFAULTTONULL) == monitor_seen);
    DestroyWindow(w);
}

int main(void) {
    hooks();
    timers();
    monitors();
    ole();
    CHECK(SetCurrentProcessExplicitAppUserModelID(L"Gala.Test") == S_OK);
    CHECK(SetThreadLocale(0x0407) && GetThreadLocale() == 0x0407);
    CHECK(!SetThreadLocale(0) && GetLastError() == ERROR_INVALID_PARAMETER);
    CHECK(SetThreadLocale(0x0409) && GetThreadLocale() == 0x0409);
    SECURITY_DESCRIPTOR sd;
    SECURITY_DESCRIPTOR_CONTROL control;
    DWORD revision;
    CHECK(InitializeSecurityDescriptor(&sd, SECURITY_DESCRIPTOR_REVISION));
    CHECK(SetSecurityDescriptorControl(&sd, SE_DACL_PROTECTED, SE_DACL_PROTECTED));
    CHECK(GetSecurityDescriptorControl(&sd, &control, &revision) && (control & SE_DACL_PROTECTED));
    CHECK(!SetSecurityDescriptorControl(&sd, SE_DACL_PRESENT, SE_DACL_PRESENT)
          && GetLastError() == ERROR_INVALID_PARAMETER);
    CHECK(SetSecurityDescriptorSacl(&sd, TRUE, NULL, FALSE));
    CHECK(GetSecurityDescriptorControl(&sd, &control, &revision) && (control & SE_SACL_PRESENT));
    BYTE world[SECURITY_MAX_SID_SIZE];
    DWORD world_size = sizeof world;
    CHECK(CreateWellKnownSid(WinWorldSid, NULL, world, &world_size));
    CHECK(SetSecurityDescriptorGroup(&sd, world, TRUE));
    PSID group = NULL;
    BOOL group_defaulted = FALSE;
    CHECK(GetSecurityDescriptorGroup(&sd, &group, &group_defaulted) && group == world && group_defaulted);
    printf("user32_test: ok\n");
    return 0;
}
