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
 * Drawing into another process's window (Chromium's GPU process paints
 * its browser's window): GetDC of the child's window, FillRect of a red
 * square at client (10, 10) - (20, 20), ReleaseDC; the child then reads
 * the pixel through its own window DC (WM_APP + 16 returns GetPixel at
 * (15, 15)): red, one surface for both; outside the square it is not.
 *
 * The child's memory (what UI Automation's list proxies use to read
 * list-view items across processes): VirtualAllocEx in it,
 * WriteProcessMemory and ReadProcessMemory with the bytes copied; a
 * handle without PROCESS_VM_READ is refused (ERROR_ACCESS_DENIED); an
 * unmapped address fails.
 *
 * Sent messages also cross threads of one process: SendMessageW from
 * another thread to a window of the main thread runs the window
 * procedure on the main thread ("the system switches to that thread",
 * while it is in GetMessage/PeekMessage/MsgWaitForMultipleObjects), with
 * the result and pointers (this process's memory) as they are: WM_APP + 4
 * returns the thread it ran on, WM_APP + 3 writes through lParam,
 * WM_SETTEXT and WM_GETTEXT go through DefWindowProc.
 *
 * The other message functions, as documented: PostThreadMessageW to the
 * child's main thread arrives there with hwnd NULL; to an id that is no
 * thread (the child's process id: process and thread ids share one
 * namespace), to a thread that has no message queue yet (one that has
 * not called a User function; PeekMessage creates it, as PostThreadMessage's
 * remarks say) or to one that has ended it fails with
 * ERROR_INVALID_THREAD_ID; to our own thread it is in our queue.
 * SendMessageTimeoutW/A give the procedure's result through lpdwResult
 * (SMTO_NORMAL, SMTO_BLOCK, SMTO_ABORTIFHUNG) and return nonzero; when the
 * receiver takes longer than the time-out they return 0 with
 * ERROR_TIMEOUT (the message is still handled); a window of our own
 * thread is called directly. ReplyMessage in the receiver returns the
 * sender's result before the procedure ends (the procedure's own return
 * is then ignored); InSendMessageEx says ISMEX_SEND, then ISMEX_SEND |
 * ISMEX_REPLIED, ISMEX_NOTIFY for SendNotifyMessage, ISMEX_CALLBACK for
 * SendMessageCallback and ISMEX_NOSEND (InSendMessage FALSE, ReplyMessage
 * FALSE) outside a message sent by another thread. SendNotifyMessageW
 * returns at once for another thread's window (its procedure runs when
 * that thread next calls a message function) and calls our own thread's
 * procedure before returning; a system message with a pointer cannot be
 * sent that way (ERROR_MESSAGE_SYNC_ONLY). SendMessageCallbackW's callback
 * gets the window, message, data and result, only when we next call
 * PeekMessage (not before). WM_COPYDATA's COPYDATASTRUCT and its buffer
 * reach the other process. A thread's windows are destroyed when it ends:
 * IsWindow is FALSE, FindWindow does not find them and SendMessage fails
 * with ERROR_INVALID_WINDOW_HANDLE.
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
    if (m == WM_APP + 5) {
        /* Longer than the parent's time-out; handled all the same. */
        Sleep(500);
        SetWindowTextW(h, L"slept");
        return 5;
    }
    if (m == WM_APP + 9) {
        /* The sender gets 123 now, not this procedure's 0. */
        ReplyMessage(123);
        Sleep(300);
        return 0;
    }
    if (m == WM_APP + 13)
        return (LRESULT)InSendMessageEx(NULL);
    if (m == WM_APP + 16) {
        HDC dc = GetDC(h);
        COLORREF c = GetPixel(dc, (int)w, (int)l);
        ReleaseDC(h, dc);
        return (LRESULT)c;
    }
    if (m == WM_APP + 14) {
        WCHAR t[64];
        swprintf(t, 64, L"notify %d", (int)InSendMessageEx(NULL));
        SetWindowTextW(h, t);
        return 0;
    }
    if (m == WM_COPYDATA) {
        const COPYDATASTRUCT *cds = (const COPYDATASTRUCT *)l;
        WCHAR t[64];
        int n = swprintf(t, 64, L"copy %d ", (int)cds->dwData);
        const char *data = (const char *)cds->lpData;
        for (DWORD i = 0; i < cds->cbData && data[i] && n < 63; i++)
            t[n++] = (WCHAR)(unsigned char)data[i];
        t[n] = 0;
        SetWindowTextW(h, t);
        return (LRESULT)cds->cbData;
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
            if (msg.hwnd == NULL && msg.message == WM_APP + 10) {
                /* A thread message (PostThreadMessage): no window. */
                WCHAR t[64];
                swprintf(t, 64, L"thread %d %d", (int)msg.wParam, (int)msg.lParam);
                SetWindowTextW(top, t);
                continue;
            }
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    DestroyWindow(top);
    return 0;
}

/* The main thread's window and what the other thread saw. */
static HWND local_window;
static DWORD local_results[9];
/* What the main thread's procedure saw of WM_APP + 7 (ReplyMessage). */
static DWORD in_send, ex_before, replied, ex_after;
/* SendNotifyMessage's calls of the procedure and what InSendMessageEx said. */
static volatile LONG notify_calls;
static DWORD notify_ex;

static LRESULT CALLBACK local_proc(HWND h, UINT m, WPARAM w, LPARAM l) {
    if (m == WM_APP + 3) {
        *(int *)l = (int)w * 2;
        return 5;
    }
    if (m == WM_APP + 4)
        return GetCurrentThreadId();
    if (m == WM_APP + 7) {
        in_send = InSendMessage();
        ex_before = InSendMessageEx(NULL);
        replied = ReplyMessage(77);
        ex_after = InSendMessageEx(NULL);
        return 5;
    }
    if (m == WM_APP + 8)
        return (LRESULT)InSendMessageEx(NULL);
    if (m == WM_APP + 15) {
        notify_ex = InSendMessageEx(NULL);
        InterlockedIncrement(&notify_calls);
        return 0;
    }
    return DefWindowProcW(h, m, w, l);
}

static DWORD WINAPI notifier(void *arg) {
    (void)arg;
    /* Returns at once: the main thread is not in a message function. */
    return SendNotifyMessageW(local_window, WM_APP + 15, 0, 0) ? 0 : 1;
}

/* A thread that has no message queue until told to make one. */
static HANDLE make_queue, queue_made;
static MSG thread_msg;

static DWORD WINAPI queue_thread(void *arg) {
    (void)arg;
    WaitForSingleObject(make_queue, 20000);
    MSG m;
    PeekMessageW(&m, NULL, WM_USER, WM_USER, PM_NOREMOVE);
    SetEvent(queue_made);
    if (GetMessageW(&m, NULL, 0, 0) <= 0)
        return 1;
    thread_msg = m;
    return 0;
}

/* A thread that makes a window and ends: the window goes with it. */
static HWND doomed;

static DWORD WINAPI window_thread(void *arg) {
    (void)arg;
    doomed = CreateWindowExW(0, L"GalaAcrossLocal", L"doomed", WS_OVERLAPPEDWINDOW, 0, 0, 100, 50,
                             NULL, NULL, GetModuleHandleW(NULL), NULL);
    return doomed ? 0 : 1;
}

/* SendMessageCallback's callback. */
static int cb_calls;
static HWND cb_hwnd;
static UINT cb_msg;
static ULONG_PTR cb_data;
static LRESULT cb_result;

static VOID CALLBACK on_result(HWND h, UINT m, ULONG_PTR data, LRESULT result) {
    cb_calls++;
    cb_hwnd = h;
    cb_msg = m;
    cb_data = data;
    cb_result = result;
}

static DWORD WINAPI sender(void *arg) {
    (void)arg;
    WCHAR text[32] = {0};
    int value = 0;
    local_results[0] = (DWORD)SendMessageW(local_window, WM_APP + 4, 0, 0);
    local_results[1] = (DWORD)SendMessageW(local_window, WM_APP + 3, 21, (LPARAM)&value);
    local_results[2] = (DWORD)value;
    local_results[3] = (DWORD)SendMessageW(local_window, WM_SETTEXT, 0, (LPARAM)L"from thread");
    local_results[4] = (DWORD)SendMessageW(local_window, WM_GETTEXT, 32, (LPARAM)text);
    local_results[5] = wcscmp(text, L"from thread") == 0;
    local_results[6] = (DWORD)SendMessageW(local_window, WM_APP + 7, 0, 0);
    DWORD_PTR r = 0;
    local_results[7] =
        SendMessageTimeoutW(local_window, WM_APP + 4, 0, 0, SMTO_NORMAL, 5000, &r) != 0;
    local_results[8] = (DWORD)r;
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

    /* WM_COPYDATA: the structure and its buffer are copied across. */
    COPYDATASTRUCT cds = {0x1234, 11, (void *)"hello copy"};
    CHECK(SendMessageW(top, WM_COPYDATA, 0, (LPARAM)&cds) == 11);
    CHECK(title_becomes(top, L"copy 4660 hello copy"));

    /* PostThreadMessage to the child's main thread: a message without a
     * window. A process id is no thread id. */
    CHECK(PostThreadMessageW(pi.dwThreadId, WM_APP + 10, 5, 6));
    CHECK(title_becomes(top, L"thread 5 6"));
    CHECK(PostThreadMessageA(pi.dwThreadId, WM_APP + 10, 8, 9));
    CHECK(title_becomes(top, L"thread 8 9"));
    SetLastError(0);
    CHECK(!PostThreadMessageW(pi.dwProcessId, WM_APP + 10, 0, 0));
    CHECK(GetLastError() == ERROR_INVALID_THREAD_ID);

    /* SendMessageTimeout: the result through lpdwResult. */
    DWORD_PTR res = 0;
    /* Drawing into the child's window. */
    {
        HDC dc = GetDC(top);
        CHECK(dc != NULL);
        RECT square = {10, 10, 20, 20};
        HBRUSH red = CreateSolidBrush(RGB(255, 0, 0));
        CHECK(FillRect(dc, &square, red));
        CHECK(ReleaseDC(top, dc) == 1);
        CHECK(DeleteObject(red));
        CHECK(SendMessageW(top, WM_APP + 16, 15, 15) == RGB(255, 0, 0));
        CHECK(SendMessageW(top, WM_APP + 16, 25, 15) != RGB(255, 0, 0));
    }
    CHECK(SendMessageTimeoutW(top, WM_APP + 2, 6, 7, SMTO_NORMAL, 5000, &res) != 0);
    CHECK(res == 42);
    res = 0;
    CHECK(SendMessageTimeoutW(top, WM_APP + 2, 3, 5, SMTO_ABORTIFHUNG | SMTO_BLOCK, 5000, &res) !=
          0);
    CHECK(res == 15);
    res = 0;
    CHECK(SendMessageTimeoutA(top, WM_APP + 2, 2, 2, SMTO_NORMAL, 5000, &res) != 0);
    CHECK(res == 4);
    res = 0;
    CHECK(SendMessageTimeoutW(edit, WM_GETTEXT, 64, (LPARAM)got, SMTO_BLOCK, 5000, &res) != 0);
    CHECK(res == 7 && wcscmp(got, L"changed") == 0);
    /* The child's procedure takes 500 ms: the time-out comes first. */
    SetLastError(0);
    CHECK(SendMessageTimeoutW(top, WM_APP + 5, 0, 0, SMTO_NORMAL, 50, &res) == 0);
    CHECK(GetLastError() == ERROR_TIMEOUT);
    /* ReplyMessage answers before the procedure returns 0. */
    res = 0;
    CHECK(SendMessageTimeoutW(top, WM_APP + 9, 0, 0, SMTO_NORMAL, 5000, &res) != 0);
    CHECK(res == 123);
    /* The timed-out message was handled all the same (before this one). */
    CHECK(title_becomes(top, L"slept"));
    SetLastError(0);
    CHECK(SendMessageTimeoutW((HWND)(ULONG_PTR)0x7ff0, WM_APP, 0, 0, SMTO_NORMAL, 100, &res) == 0);
    CHECK(GetLastError() == ERROR_INVALID_WINDOW_HANDLE);

    /* What the receiver's InSendMessageEx says. */
    CHECK(SendMessageW(top, WM_APP + 13, 0, 0) == ISMEX_SEND);
    CHECK(SendNotifyMessageW(top, WM_APP + 14, 0, 0));
    CHECK(title_becomes(top, L"notify 2")); /* ISMEX_NOTIFY */
    SetLastError(0);
    CHECK(!SendNotifyMessageW(top, WM_SETTEXT, 0, (LPARAM)L"no"));
    CHECK(GetLastError() == ERROR_MESSAGE_SYNC_ONLY);

    /* SendMessageCallback: the callback runs when we next pump. */
    CHECK(SendMessageCallbackW(top, WM_APP + 13, 0, 0, on_result, 99));
    CHECK(cb_calls == 0);
    for (int i = 0; i < 1000 && cb_calls == 0; i++) {
        MSG m;
        PeekMessageW(&m, NULL, 0, 0, PM_NOREMOVE);
        if (cb_calls == 0)
            Sleep(10);
    }
    CHECK(cb_calls == 1);
    CHECK(cb_hwnd == top && cb_msg == WM_APP + 13 && cb_data == 99);
    CHECK(cb_result == ISMEX_CALLBACK);
    SetLastError(0);
    CHECK(!SendMessageCallbackW(top, WM_COPYDATA, 0, (LPARAM)&cds, on_result, 0));
    CHECK(GetLastError() == ERROR_MESSAGE_SYNC_ONLY);

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

    /* Sent messages across threads of this process. */
    WNDCLASSW lc = {0};
    lc.lpfnWndProc = local_proc;
    lc.hInstance = GetModuleHandleW(NULL);
    lc.lpszClassName = L"GalaAcrossLocal";
    CHECK(RegisterClassW(&lc));
    local_window = CreateWindowExW(0, L"GalaAcrossLocal", L"local", WS_OVERLAPPEDWINDOW, 0, 0, 200,
                                   100, NULL, NULL, lc.hInstance, NULL);
    CHECK(local_window != NULL);
    HANDLE thread = CreateThread(NULL, 0, sender, NULL, 0, NULL);
    CHECK(thread != NULL);
    for (;;) {
        MSG m;
        while (PeekMessageW(&m, NULL, 0, 0, PM_REMOVE))
            DispatchMessageW(&m);
        DWORD w = MsgWaitForMultipleObjects(1, &thread, FALSE, 20000, QS_ALLINPUT);
        if (w == WAIT_OBJECT_0)
            break;
        CHECK(w == WAIT_OBJECT_0 + 1);
    }
    CloseHandle(thread);
    CHECK(local_results[0] == GetCurrentThreadId());
    CHECK(local_results[1] == 5);
    CHECK(local_results[2] == 42);
    CHECK(local_results[3] == TRUE);
    CHECK(local_results[4] == 11);
    CHECK(local_results[5]);
    CHECK(GetWindowTextW(local_window, text, 64) == 11);
    /* ReplyMessage and InSendMessage(Ex) in the main thread's procedure. */
    CHECK(local_results[6] == 77);
    CHECK(in_send == TRUE);
    CHECK(ex_before == ISMEX_SEND);
    CHECK(replied == TRUE);
    CHECK(ex_after == (ISMEX_SEND | ISMEX_REPLIED));
    CHECK(local_results[7] == TRUE && local_results[8] == GetCurrentThreadId());
    /* Not in a message sent by another thread. */
    CHECK(SendMessageW(local_window, WM_APP + 8, 0, 0) == ISMEX_NOSEND);
    CHECK(!InSendMessage());
    CHECK(!ReplyMessage(1));
    /* SendMessageTimeout to our own window: called directly. */
    DWORD_PTR own = 0;
    CHECK(SendMessageTimeoutW(local_window, WM_APP + 4, 0, 0, SMTO_NORMAL, 100, &own) != 0);
    CHECK(own == GetCurrentThreadId());

    /* SendNotifyMessage from another thread returns before the procedure
     * runs (we do not pump while it runs); ours runs it at once. */
    thread = CreateThread(NULL, 0, notifier, NULL, 0, NULL);
    CHECK(thread != NULL);
    CHECK(WaitForSingleObject(thread, 20000) == WAIT_OBJECT_0);
    DWORD code = 1;
    CHECK(GetExitCodeThread(thread, &code) && code == 0);
    CloseHandle(thread);
    CHECK(notify_calls == 0);
    {
        MSG m;
        PeekMessageW(&m, NULL, 0, 0, PM_NOREMOVE);
    }
    CHECK(notify_calls == 1);
    CHECK(notify_ex == ISMEX_NOTIFY);
    CHECK(SendNotifyMessageW(local_window, WM_APP + 15, 0, 0));
    CHECK(notify_calls == 2);
    CHECK(notify_ex == ISMEX_NOSEND);

    /* SendMessageCallback to our own window: the callback at once. */
    cb_calls = 0;
    CHECK(SendMessageCallbackW(local_window, WM_APP + 4, 0, 0, on_result, 7));
    CHECK(cb_calls == 1 && cb_hwnd == local_window && cb_msg == WM_APP + 4 && cb_data == 7);
    CHECK(cb_result == (LRESULT)GetCurrentThreadId());

    /* PostThreadMessage to a thread without a message queue fails; once
     * it has one (PeekMessage) the message arrives without a window. */
    make_queue = CreateEventW(NULL, TRUE, FALSE, NULL);
    queue_made = CreateEventW(NULL, TRUE, FALSE, NULL);
    CHECK(make_queue && queue_made);
    DWORD qtid = 0;
    thread = CreateThread(NULL, 0, queue_thread, NULL, 0, &qtid);
    CHECK(thread != NULL);
    SetLastError(0);
    CHECK(!PostThreadMessageW(qtid, WM_APP + 11, 1, 2));
    CHECK(GetLastError() == ERROR_INVALID_THREAD_ID);
    CHECK(SetEvent(make_queue));
    CHECK(WaitForSingleObject(queue_made, 20000) == WAIT_OBJECT_0);
    CHECK(PostThreadMessageW(qtid, WM_APP + 11, 1, 2));
    CHECK(WaitForSingleObject(thread, 20000) == WAIT_OBJECT_0);
    CHECK(GetExitCodeThread(thread, &code) && code == 0);
    CloseHandle(thread);
    CHECK(thread_msg.hwnd == NULL && thread_msg.message == WM_APP + 11);
    CHECK(thread_msg.wParam == 1 && thread_msg.lParam == 2);
    /* The thread has ended. */
    SetLastError(0);
    CHECK(!PostThreadMessageW(qtid, WM_APP + 11, 1, 2));
    CHECK(GetLastError() == ERROR_INVALID_THREAD_ID);
    /* Our own thread: in our queue, without a window. */
    CHECK(PostThreadMessageW(GetCurrentThreadId(), WM_APP + 12, 3, 4));
    {
        MSG m;
        CHECK(PeekMessageW(&m, NULL, WM_APP + 12, WM_APP + 12, PM_REMOVE));
        CHECK(m.hwnd == NULL && m.wParam == 3 && m.lParam == 4);
    }

    /* A thread's windows are destroyed when it ends. */
    thread = CreateThread(NULL, 0, window_thread, NULL, 0, NULL);
    CHECK(thread != NULL);
    CHECK(WaitForSingleObject(thread, 20000) == WAIT_OBJECT_0);
    CloseHandle(thread);
    CHECK(doomed != NULL);
    CHECK(!IsWindow(doomed));
    CHECK(FindWindowW(L"GalaAcrossLocal", L"doomed") == NULL);
    SetLastError(0);
    CHECK(SendMessageW(doomed, WM_APP + 4, 0, 0) == 0);
    CHECK(GetLastError() == ERROR_INVALID_WINDOW_HANDLE);

    CHECK(DestroyWindow(local_window));

    /* The child ends: its windows go. */
    CHECK(SetEvent(done));
    CHECK(WaitForSingleObject(pi.hProcess, 20000) == WAIT_OBJECT_0);
    CHECK(!IsWindow(top));
    CHECK(!IsWindow(button));
    CHECK(FindWindowW(L"GalaAcross", NULL) == NULL);

    printf("windows_across_test: ok\n");
    return 0;
}
