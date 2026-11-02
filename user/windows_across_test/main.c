/* Windows across processes, with the results Windows documents.
 *
 * The program starts itself again with the argument "child". The child
 * creates a top-level window (class "GalaAcross", title "across child",
 * 400 x 300 at (100, 100), shown) with a BUTTON (id 101, "Press") and an
 * EDIT (id 102, "some text"), sets the named event "GalaAcrossReady" and
 * pumps messages until "GalaAcrossDone" is set.
 *
 * The parent, a different process, then finds and reads them:
 * FindWindowW by class and by title, IsWindow, IsWindowVisible,
 * GetWindowThreadProcessId (the child's process and thread ids from
 * CreateProcess), GetWindowTextW/GetWindowTextLengthW, GetClassNameW,
 * GetWindowRect, EnumWindows (it is among the top-level windows),
 * EnumChildWindows (two children), FindWindowExW for the button,
 * GetDlgCtrlID, GetDlgItem, GetParent, GetWindow(GW_CHILD). The child's
 * popup "across owned" (a STATIC with WS_POPUP | WS_CAPTION), created with
 * its button as hWndParent, is owned by the button's top-level window
 * (CreateWindowEx: an owned window's owner is the top-level window of
 * the one given), which GetWindow(GW_OWNER) reports; the top-level window
 * and the button have no owner. After the
 * child ends, its windows are gone (IsWindow FALSE, FindWindowW NULL).
 *
 * Posted messages cross too: BM_CLICK posted to the child's button makes
 * the child's window get WM_COMMAND (BN_CLICKED from id 101), WM_APP + 1
 * posted to its window arrives with its parameters (the child shows what
 * it got in its title, which the parent reads); a system message that
 * carries a pointer (WM_SETTEXT) cannot be posted to another process
 * (ERROR_MESSAGE_SYNC_ONLY), and a window that is not one gives
 * ERROR_INVALID_WINDOW_HANDLE.
 *
 * Sent messages cross as well, the system's marshaling the pointers:
 * WM_GETTEXT reads the child's edit's text into our buffer, WM_SETTEXT
 * gives it ours, WM_GETTEXTLENGTH counts it, and WM_APP + 2's result is
 * what the child's procedure returned (wParam * lParam).
 *
 * The child's memory (what UI Automation's list proxies use to read
 * list-view items across processes): VirtualAllocEx in it,
 * WriteProcessMemory and ReadProcessMemory with the bytes copied; a
 * handle without PROCESS_VM_READ is refused (ERROR_ACCESS_DENIED); an
 * unmapped address fails.
 *
 * Prints "windows_across_test: ok" and exits 0, or the failing check's
 * number and line.
 */
#include <windows.h>
#include <stdio.h>
#include <string.h>

static int checks;

#define CHECK(c)                                                              \
    do {                                                                      \
        ++checks;                                                             \
        if (!(c)) {                                                           \
            printf("windows_across_test: check %d (line %d) failed\n",        \
                   checks, __LINE__);                                         \
            fflush(stdout);                                                   \
            ExitProcess(1);                                                   \
        }                                                                     \
    } while (0)

static LRESULT CALLBACK child_proc(HWND h, UINT m, WPARAM w, LPARAM l) {
    if (m == WM_COMMAND && LOWORD(w) == 101 && HIWORD(w) == BN_CLICKED) {
        SetWindowTextW(h, L"clicked");
        return 0;
    }
    if (m == WM_APP + 2)
        return (LRESULT)(w * l);
    if (m == WM_APP + 1) {
        WCHAR t[64];
        swprintf(t, 64, L"app %d %d", (int)w, (int)l);
        SetWindowTextW(h, t);
        return 0;
    }
    return DefWindowProcW(h, m, w, l);
}

/* Waits (up to 10 s) for window `h`'s title to become `want`. */
static int title_becomes(HWND h, const WCHAR *want) {
    WCHAR t[64];
    for (int i = 0; i < 1000; i++) {
        if (GetWindowTextW(h, t, 64) > 0 && wcscmp(t, want) == 0)
            return 1;
        Sleep(10);
    }
    return 0;
}

static int child(void) {
    HANDLE ready = OpenEventW(EVENT_MODIFY_STATE, FALSE, L"GalaAcrossReady");
    HANDLE done = OpenEventW(SYNCHRONIZE, FALSE, L"GalaAcrossDone");
    if (!ready || !done)
        return 2;
    WNDCLASSW wc = {0};
    wc.lpfnWndProc = child_proc;
    wc.hInstance = GetModuleHandleW(NULL);
    wc.lpszClassName = L"GalaAcross";
    if (!RegisterClassW(&wc))
        return 3;
    HWND top = CreateWindowExW(0, L"GalaAcross", L"across child",
                               WS_OVERLAPPEDWINDOW | WS_VISIBLE, 100, 100, 400,
                               300, NULL, NULL, wc.hInstance, NULL);
    HWND button = CreateWindowExW(0, L"BUTTON", L"Press",
                                  WS_CHILD | WS_VISIBLE | BS_PUSHBUTTON, 10, 10,
                                  80, 24, top, (HMENU)101, wc.hInstance, NULL);
    HWND edit = CreateWindowExW(0, L"EDIT", L"some text",
                                WS_CHILD | WS_VISIBLE | WS_BORDER, 10, 40, 150,
                                24, top, (HMENU)102, wc.hInstance, NULL);
    HWND owned = CreateWindowExW(0, L"STATIC", L"across owned",
                                 WS_POPUP | WS_CAPTION | WS_VISIBLE, 150, 150, 200,
                                 100, button, NULL, wc.hInstance, NULL);
    if (!top || !button || !edit || !owned)
        return 4;
    SetEvent(ready);
    for (;;) {
        DWORD r = MsgWaitForMultipleObjects(1, &done, FALSE, 20000, QS_ALLINPUT);
        if (r == WAIT_OBJECT_0 || r == WAIT_TIMEOUT)
            break;
        MSG msg;
        while (PeekMessageW(&msg, NULL, 0, 0, PM_REMOVE)) {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    DestroyWindow(top);
    return 0;
}

static HWND seen_top;
static int top_found, children_found;

static BOOL CALLBACK each_top(HWND h, LPARAM l) {
    (void)l;
    if (h == seen_top)
        top_found = 1;
    return TRUE;
}

static BOOL CALLBACK each_child(HWND h, LPARAM l) {
    (void)l;
    if (GetParent(h) == seen_top)
        children_found++;
    return TRUE;
}

int main(int argc, char **argv) {
    if (argc > 1 && strcmp(argv[1], "child") == 0)
        return child();

    HANDLE ready = CreateEventW(NULL, TRUE, FALSE, L"GalaAcrossReady");
    HANDLE done = CreateEventW(NULL, TRUE, FALSE, L"GalaAcrossDone");
    CHECK(ready && done);
    WCHAR self[MAX_PATH];
    CHECK(GetModuleFileNameW(NULL, self, MAX_PATH) > 0);
    WCHAR cmd[MAX_PATH + 16];
    swprintf(cmd, MAX_PATH + 16, L"\"%ls\" child", self);
    STARTUPINFOW si = {sizeof si};
    PROCESS_INFORMATION pi;
    CHECK(CreateProcessW(self, cmd, NULL, NULL, FALSE, 0, NULL, NULL, &si, &pi));
    HANDLE wait[2] = {ready, pi.hProcess};
    CHECK(WaitForMultipleObjects(2, wait, FALSE, 20000) == WAIT_OBJECT_0);

    /* The child's top-level window, by class and by title. */
    HWND top = FindWindowW(L"GalaAcross", NULL);
    CHECK(top != NULL);
    CHECK(FindWindowW(NULL, L"across child") == top);
    CHECK(FindWindowW(L"GalaAcross", L"across child") == top);
    CHECK(FindWindowW(L"NoSuchClass", NULL) == NULL);
    CHECK(IsWindow(top));
    CHECK(IsWindowVisible(top));
    DWORD pid = 0;
    DWORD tid = GetWindowThreadProcessId(top, &pid);
    CHECK(pid == pi.dwProcessId);
    CHECK(tid == pi.dwThreadId);
    CHECK(pid != GetCurrentProcessId());

    WCHAR text[64];
    CHECK(GetWindowTextLengthW(top) == 12);
    CHECK(GetWindowTextW(top, text, 64) == 12);
    CHECK(wcscmp(text, L"across child") == 0);
    /* A short buffer: truncated, NUL-terminated. */
    CHECK(GetWindowTextW(top, text, 7) == 6);
    CHECK(wcscmp(text, L"across") == 0);
    CHECK(GetClassNameW(top, text, 64) == 10);
    CHECK(wcscmp(text, L"GalaAcross") == 0);
    RECT r;
    CHECK(GetWindowRect(top, &r));
    CHECK(r.left == 100 && r.top == 100 && r.right == 500 && r.bottom == 400);

    seen_top = top;
    CHECK(EnumWindows(each_top, 0));
    CHECK(top_found);
    CHECK(EnumChildWindows(top, each_child, 0));
    CHECK(children_found == 2);

    /* The button and the edit. */
    HWND button = FindWindowExW(top, NULL, L"Button", NULL);
    CHECK(button != NULL);
    CHECK(GetDlgCtrlID(button) == 101);
    CHECK(GetParent(button) == top);
    CHECK(GetWindowTextW(button, text, 64) == 5);
    CHECK(wcscmp(text, L"Press") == 0);
    CHECK(FindWindowExW(top, NULL, NULL, L"Press") == button);
    HWND edit = GetDlgItem(top, 102);
    CHECK(edit != NULL && edit != button);
    CHECK(GetClassNameW(edit, text, 64) == 4);
    CHECK(_wcsicmp(text, L"Edit") == 0);
    CHECK(GetWindowTextW(edit, text, 64) == 9);
    CHECK(wcscmp(text, L"some text") == 0);
    CHECK(GetWindow(top, GW_CHILD) == button || GetWindow(top, GW_CHILD) == edit);

    /* The owned popup: its owner is the button's top-level window. */
    HWND owned = FindWindowW(NULL, L"across owned");
    CHECK(owned != NULL && owned != top);
    CHECK(GetWindow(owned, GW_OWNER) == top);
    CHECK(GetWindow(top, GW_OWNER) == NULL);
    CHECK(GetWindow(button, GW_OWNER) == NULL);
    CHECK(GetWindowRect(button, &r));
    CHECK(r.right - r.left == 80 && r.bottom - r.top == 24);
    DWORD bpid = 0;
    CHECK(GetWindowThreadProcessId(button, &bpid) == pi.dwThreadId);
    CHECK(bpid == pi.dwProcessId);

    /* Posted messages to the other process. */
    CHECK(PostMessageW(button, BM_CLICK, 0, 0));
    CHECK(title_becomes(top, L"clicked"));
    CHECK(PostMessageW(top, WM_APP + 1, 7, 9));
    CHECK(title_becomes(top, L"app 7 9"));
    SetLastError(0);
    CHECK(!PostMessageW(top, WM_SETTEXT, 0, (LPARAM)L"no"));
    CHECK(GetLastError() == ERROR_MESSAGE_SYNC_ONLY);
    SetLastError(0);
    CHECK(!PostMessageW((HWND)(ULONG_PTR)0x7ff0, WM_APP, 0, 0));
    CHECK(GetLastError() == ERROR_INVALID_WINDOW_HANDLE);

    /* Sent messages to the other process. */
    WCHAR got[64] = {0};
    CHECK(SendMessageW(edit, WM_GETTEXT, 64, (LPARAM)got) == 9);
    CHECK(wcscmp(got, L"some text") == 0);
    CHECK(SendMessageW(edit, WM_GETTEXT, 5, (LPARAM)got) == 4);
    CHECK(wcscmp(got, L"some") == 0);
    CHECK(SendMessageW(edit, WM_SETTEXT, 0, (LPARAM)L"changed") == TRUE);
    CHECK(SendMessageW(edit, WM_GETTEXTLENGTH, 0, 0) == 7);
    CHECK(SendMessageW(edit, WM_GETTEXT, 64, (LPARAM)got) == 7);
    CHECK(wcscmp(got, L"changed") == 0);
    CHECK(SendMessageW(top, WM_APP + 2, 6, 7) == 42);

    /* The child's memory. */
    HANDLE proc = OpenProcess(PROCESS_VM_OPERATION | PROCESS_VM_READ | PROCESS_VM_WRITE, FALSE,
                              pi.dwProcessId);
    CHECK(proc != NULL);
    char *remote = VirtualAllocEx(proc, NULL, 4096, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE);
    CHECK(remote != NULL);
    SIZE_T copied = 0;
    CHECK(WriteProcessMemory(proc, remote + 100, "across", 7, &copied));
    CHECK(copied == 7);
    char back[16] = {0};
    copied = 0;
    CHECK(ReadProcessMemory(proc, remote + 100, back, 7, &copied));
    CHECK(copied == 7 && strcmp(back, "across") == 0);
    CHECK(ReadProcessMemory(proc, remote, back, 4, NULL));
    CHECK(back[0] == 0 && back[3] == 0); /* fresh pages are zero */
    CHECK(!ReadProcessMemory(proc, (void *)0x1000, back, 4, NULL));
    CHECK(VirtualFreeEx(proc, remote, 0, MEM_RELEASE));
    CloseHandle(proc);
    HANDLE weak = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, FALSE, pi.dwProcessId);
    CHECK(weak != NULL);
    SetLastError(0);
    CHECK(!ReadProcessMemory(weak, remote, back, 4, NULL));
    CHECK(GetLastError() == ERROR_ACCESS_DENIED);
    CloseHandle(weak);

    /* The child ends: its windows go. */
    CHECK(SetEvent(done));
    CHECK(WaitForSingleObject(pi.hProcess, 20000) == WAIT_OBJECT_0);
    CHECK(!IsWindow(top));
    CHECK(!IsWindow(button));
    CHECK(FindWindowW(L"GalaAcross", NULL) == NULL);

    printf("windows_across_test: ok\n");
    return 0;
}
