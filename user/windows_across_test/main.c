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
 * GetDlgCtrlID, GetDlgItem, GetParent, GetWindow(GW_CHILD). After the
 * child ends, its windows are gone (IsWindow FALSE, FindWindowW NULL).
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
    return DefWindowProcW(h, m, w, l);
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
    if (!top || !button || !edit)
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
    CHECK(GetWindowRect(button, &r));
    CHECK(r.right - r.left == 80 && r.bottom - r.top == 24);
    DWORD bpid = 0;
    CHECK(GetWindowThreadProcessId(button, &bpid) == pi.dwThreadId);
    CHECK(bpid == pi.dwProcessId);

    /* The child ends: its windows go. */
    CHECK(SetEvent(done));
    CHECK(WaitForSingleObject(pi.hProcess, 20000) == WAIT_OBJECT_0);
    CHECK(!IsWindow(top));
    CHECK(!IsWindow(button));
    CHECK(FindWindowW(L"GalaAcross", NULL) == NULL);

    printf("windows_across_test: ok\n");
    return 0;
}
