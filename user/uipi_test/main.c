/* User Interface Privilege Isolation (UIPI), with the results Microsoft
 * documents.
 *
 * The program starts itself again with the argument "child" under a copy
 * of its token lowered to Low integrity (DuplicateTokenEx,
 * SetTokenInformation(TokenIntegrityLevel, S-1-16-4096),
 * CreateProcessAsUserW: "Designing Applications to Run at a Low Integrity
 * Level"). The parent (Medium, or High when elevated: above Low either
 * way) has a window "uipi parent"; the two talk through inherited events
 * (a Low process may not signal a Medium process's named event: no write
 * up).
 *
 * What the child sees, as documented:
 * - PostMessage and PostThreadMessage to the parent's window and thread
 *   fail; "when a message is blocked by UIPI the last error is set to 5
 *   (access denied)". SendMessage of WM_APP + 2 fails the same way (the
 *   parent's procedure never sees it) and SendMessageTimeout returns 0.
 *   UIPI blocks messages above WM_USER by default (ChangeWindowMessageFilter).
 * - Some messages below WM_USER always pass ("certain messages whose value
 *   is smaller than WM_USER are required to pass through the filter"):
 *   WM_GETTEXT and WM_GETTEXTLENGTH read the parent's title, WM_NULL
 *   posts. GetWindowText (no message to another process) works.
 * - SetWindowPos of the parent's window fails with ERROR_ACCESS_DENIED and
 *   ShowWindow does not hide it (assumed: UIPI covers them as the messages
 *   they send; to verify on Windows).
 * - ChangeWindowMessageFilterEx and ChangeWindowMessageFilter fail at Low
 *   ("processes at or below SECURITY_MANDATORY_LOW_RID are not allowed to
 *   change the message filter": ERROR_ACCESS_DENIED for the Ex function).
 * The parent then opens messages:
 * - ChangeWindowMessageFilterEx(MSGFLT_ALLOW) of WM_APP + 1 for its window
 *   returns TRUE with ExtStatus MSGFLTINFO_NONE, then
 *   MSGFLTINFO_ALREADYALLOWED_FORWND; a CHANGEFILTERSTRUCT with a wrong
 *   cbSize fails with ERROR_INVALID_PARAMETER; ChangeWindowMessageFilter(
 *   WM_APP + 4, MSGFLT_ADD) opens WM_APP + 4 process-wide; for another
 *   process's window the Ex function fails with ERROR_ACCESS_DENIED.
 * - The child's post of WM_APP + 1 now arrives (the parent's title shows
 *   it), WM_APP + 4 is sent and answered, WM_APP + 2 stays blocked.
 * - MSGFLT_DISALLOW: MSGFLTINFO_NONE, then ALREADYDISALLOWED_FORWND; for
 *   WM_APP + 4 (allowed process-wide) MSGFLTINFO_ALLOWED_HIGHER; MSGFLT_RESET
 *   MSGFLTINFO_NONE. The child's post of WM_APP + 1 is blocked again,
 *   WM_APP + 4 still passes.
 * - A higher level reaches a lower one: the parent's SendMessage to the
 *   child's window is answered.
 *
 * Prints "uipi_test: ok" and exits 0, or the failing check's number and
 * line (the child's checks too: its exit code is the parent's last check).
 */
#include <windows.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static int checks;
static const char *who = "uipi_test";

#define CHECK(c)                                                              \
    do {                                                                      \
        ++checks;                                                             \
        if (!(c)) {                                                           \
            printf("%s: check %d (line %d) failed (last error %lu)\n", who,   \
                   checks, __LINE__, GetLastError());                         \
            fflush(stdout);                                                   \
            ExitProcess(1);                                                   \
        }                                                                     \
    } while (0)

#ifndef MSGFLT_RESET
#define MSGFLT_RESET 0
#define MSGFLT_ALLOW 1
#define MSGFLT_DISALLOW 2
#endif

static volatile LONG thread_messages;

static LRESULT CALLBACK parent_proc(HWND h, UINT m, WPARAM w, LPARAM l) {
    if (m == WM_APP + 1) {
        WCHAR t[64];
        swprintf(t, 64, L"app %d %d", (int)w, (int)l);
        SetWindowTextW(h, t);
        return 0;
    }
    if (m == WM_APP + 2)
        return (LRESULT)(w * l);
    if (m == WM_APP + 4)
        return (LRESULT)(w + l);
    return DefWindowProcW(h, m, w, l);
}

/* Handles messages until `h` is signaled (or 20 s); whether it was. */
static int pump_until(HANDLE h) {
    for (int i = 0; i < 2000; i++) {
        MSG msg;
        while (PeekMessageW(&msg, NULL, 0, 0, PM_REMOVE)) {
            if (msg.hwnd == NULL && msg.message >= WM_APP) {
                InterlockedIncrement(&thread_messages);
                continue;
            }
            DispatchMessageW(&msg);
        }
        DWORD r = MsgWaitForMultipleObjects(1, &h, FALSE, 10, QS_ALLINPUT);
        if (r == WAIT_OBJECT_0)
            return 1;
    }
    return 0;
}

/* The integrity level (the label's RID) of this process's token. */
static DWORD my_level(void) {
    HANDLE t;
    if (!OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &t))
        return 0xffffffff;
    BYTE buf[64];
    DWORD len = 0;
    DWORD level = 0xffffffff;
    if (GetTokenInformation(t, TokenIntegrityLevel, buf, sizeof buf, &len)) {
        PSID sid = ((TOKEN_MANDATORY_LABEL *)buf)->Label.Sid;
        level = *GetSidSubAuthority(sid, *GetSidSubAuthorityCount(sid) - 1);
    }
    CloseHandle(t);
    return level;
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

static LRESULT CALLBACK child_proc(HWND h, UINT m, WPARAM w, LPARAM l) {
    if (m == WM_APP + 2)
        return (LRESULT)(w * l);
    return DefWindowProcW(h, m, w, l);
}

static int child(char **argv) {
    who = "uipi_test child";
    HWND parent = (HWND)(ULONG_PTR)_strtoui64(argv[2], NULL, 16);
    DWORD tid = (DWORD)strtoul(argv[3], NULL, 10);
    HANDLE go2 = (HANDLE)(ULONG_PTR)_strtoui64(argv[4], NULL, 16);
    HANDLE go3 = (HANDLE)(ULONG_PTR)_strtoui64(argv[5], NULL, 16);
    HANDLE step1 = (HANDLE)(ULONG_PTR)_strtoui64(argv[6], NULL, 16);
    HANDLE step2 = (HANDLE)(ULONG_PTR)_strtoui64(argv[7], NULL, 16);
    CHECK(my_level() == SECURITY_MANDATORY_LOW_RID);
    CHECK(IsWindow(parent));

    WNDCLASSW wc = {0};
    wc.lpfnWndProc = child_proc;
    wc.hInstance = GetModuleHandleW(NULL);
    wc.lpszClassName = L"GalaUipiChild";
    CHECK(RegisterClassW(&wc));
    HWND mine = CreateWindowExW(0, L"GalaUipiChild", L"uipi child", WS_OVERLAPPEDWINDOW, 0, 0,
                                200, 100, NULL, NULL, wc.hInstance, NULL);
    CHECK(mine != NULL);

    /* Blocked: messages above WM_USER, posted, sent or to the thread. */
    SetLastError(0);
    CHECK(SendMessageW(parent, WM_APP + 2, 6, 7) == 0);
    CHECK(GetLastError() == ERROR_ACCESS_DENIED);
    SetLastError(0);
    CHECK(!PostMessageW(parent, WM_APP + 1, 1, 2));
    CHECK(GetLastError() == ERROR_ACCESS_DENIED);
    SetLastError(0);
    CHECK(!PostThreadMessageW(tid, WM_APP + 6, 0, 0));
    CHECK(GetLastError() == ERROR_ACCESS_DENIED);
    DWORD_PTR res = 99;
    CHECK(SendMessageTimeoutW(parent, WM_APP + 2, 6, 7, SMTO_NORMAL, 2000, &res) == 0);
    /* Passing: the title (sent and read), WM_NULL. */
    CHECK(SendMessageW(parent, WM_GETTEXTLENGTH, 0, 0) == 11);
    WCHAR text[64] = {0};
    CHECK(SendMessageW(parent, WM_GETTEXT, 64, (LPARAM)text) == 11);
    CHECK(wcscmp(text, L"uipi parent") == 0);
    CHECK(GetWindowTextW(parent, text, 64) == 11);
    CHECK(PostMessageW(parent, WM_NULL, 0, 0));
    /* Not moved, not hidden. */
    SetLastError(0);
    CHECK(!SetWindowPos(parent, NULL, 0, 0, 123, 45, SWP_NOZORDER | SWP_NOMOVE));
    CHECK(GetLastError() == ERROR_ACCESS_DENIED);
    ShowWindow(parent, SW_HIDE);
    /* Low may not change filters, not even its own window's. */
    SetLastError(0);
    CHECK(!ChangeWindowMessageFilterEx(mine, WM_APP + 1, MSGFLT_ALLOW, NULL));
    CHECK(GetLastError() == ERROR_ACCESS_DENIED);
    CHECK(!ChangeWindowMessageFilter(WM_APP + 1, MSGFLT_ADD));
    CHECK(SetEvent(step1));
    CHECK(pump_until(go2));

    /* The parent opened WM_APP + 1 for its window, WM_APP + 4 process-wide. */
    CHECK(PostMessageW(parent, WM_APP + 1, 7, 9));
    CHECK(title_becomes(parent, L"app 7 9"));
    CHECK(SendMessageW(parent, WM_APP + 4, 3, 5) == 8);
    SetLastError(0);
    CHECK(SendMessageW(parent, WM_APP + 2, 6, 7) == 0);
    CHECK(GetLastError() == ERROR_ACCESS_DENIED);
    CHECK(SetEvent(step2));
    CHECK(pump_until(go3));

    /* WM_APP + 1 closed again; WM_APP + 4 still open process-wide. */
    SetLastError(0);
    CHECK(!PostMessageW(parent, WM_APP + 1, 1, 1));
    CHECK(GetLastError() == ERROR_ACCESS_DENIED);
    CHECK(SendMessageW(parent, WM_APP + 4, 1, 1) == 2);
    DestroyWindow(mine);
    return 0;
}

int main(int argc, char **argv) {
    if (argc > 7 && strcmp(argv[1], "child") == 0)
        return child(argv);

    DWORD level = my_level();
    CHECK(level != 0xffffffff && level > SECURITY_MANDATORY_LOW_RID);

    WNDCLASSW wc = {0};
    wc.lpfnWndProc = parent_proc;
    wc.hInstance = GetModuleHandleW(NULL);
    wc.lpszClassName = L"GalaUipi";
    CHECK(RegisterClassW(&wc));
    HWND top = CreateWindowExW(0, L"GalaUipi", L"uipi parent", WS_OVERLAPPEDWINDOW | WS_VISIBLE,
                               50, 60, 300, 200, NULL, NULL, wc.hInstance, NULL);
    CHECK(top != NULL);
    RECT before;
    CHECK(GetWindowRect(top, &before));
    /* A thread message queue for the child's PostThreadMessage. */
    MSG m;
    PeekMessageW(&m, NULL, WM_USER, WM_USER, PM_NOREMOVE);

    /* The events the two share (inherited handles). */
    SECURITY_ATTRIBUTES sa = {sizeof sa, NULL, TRUE};
    HANDLE go2 = CreateEventW(&sa, TRUE, FALSE, NULL);
    HANDLE go3 = CreateEventW(&sa, TRUE, FALSE, NULL);
    HANDLE step1 = CreateEventW(&sa, TRUE, FALSE, NULL);
    HANDLE step2 = CreateEventW(&sa, TRUE, FALSE, NULL);
    CHECK(go2 && go3 && step1 && step2);

    /* A Low integrity copy of our token. */
    HANDLE token, low;
    CHECK(OpenProcessToken(GetCurrentProcess(),
                           TOKEN_DUPLICATE | TOKEN_ADJUST_DEFAULT | TOKEN_QUERY |
                               TOKEN_ASSIGN_PRIMARY,
                           &token));
    CHECK(DuplicateTokenEx(token, 0, NULL, SecurityImpersonation, TokenPrimary, &low));
    SID_IDENTIFIER_AUTHORITY label = SECURITY_MANDATORY_LABEL_AUTHORITY;
    PSID low_sid = NULL;
    CHECK(AllocateAndInitializeSid(&label, 1, SECURITY_MANDATORY_LOW_RID, 0, 0, 0, 0, 0, 0, 0,
                                   &low_sid));
    TOKEN_MANDATORY_LABEL tml = {{low_sid, SE_GROUP_INTEGRITY}};
    CHECK(SetTokenInformation(low, TokenIntegrityLevel, &tml,
                              sizeof tml + GetLengthSid(low_sid)));

    WCHAR self[MAX_PATH];
    CHECK(GetModuleFileNameW(NULL, self, MAX_PATH) > 0);
    WCHAR cmd[MAX_PATH + 160];
    swprintf(cmd, MAX_PATH + 160, L"\"%ls\" child %llx %lu %llx %llx %llx %llx", self,
             (unsigned long long)(ULONG_PTR)top, GetCurrentThreadId(),
             (unsigned long long)(ULONG_PTR)go2, (unsigned long long)(ULONG_PTR)go3,
             (unsigned long long)(ULONG_PTR)step1, (unsigned long long)(ULONG_PTR)step2);
    STARTUPINFOW si = {sizeof si};
    PROCESS_INFORMATION pi;
    CHECK(CreateProcessAsUserW(low, self, cmd, NULL, NULL, TRUE, 0, NULL, NULL, &si, &pi));

    /* Step 1: everything the child tried was blocked but what passes. */
    HANDLE w1[2] = {step1, pi.hProcess};
    int ok = 0;
    for (int i = 0; i < 2000 && !ok; i++) {
        MSG msg;
        while (PeekMessageW(&msg, NULL, 0, 0, PM_REMOVE)) {
            if (msg.hwnd == NULL && msg.message >= WM_APP) {
                InterlockedIncrement(&thread_messages);
                continue;
            }
            DispatchMessageW(&msg);
        }
        DWORD r = MsgWaitForMultipleObjects(2, w1, FALSE, 10, QS_ALLINPUT);
        if (r == WAIT_OBJECT_0)
            ok = 1;
        else if (r == WAIT_OBJECT_0 + 1)
            break;
    }
    if (!ok) {
        DWORD code = 0;
        GetExitCodeProcess(pi.hProcess, &code);
        printf("uipi_test: the child ended early with %lu\n", code);
    }
    CHECK(ok);
    WCHAR text[64];
    CHECK(GetWindowTextW(top, text, 64) == 11 && wcscmp(text, L"uipi parent") == 0);
    RECT now;
    CHECK(GetWindowRect(top, &now));
    CHECK(EqualRect(&now, &before));
    CHECK(IsWindowVisible(top));
    CHECK(thread_messages == 0);
    /* Higher to lower: allowed. */
    HWND theirs = FindWindowW(L"GalaUipiChild", NULL);
    CHECK(theirs != NULL);
    CHECK(SendMessageW(theirs, WM_APP + 2, 6, 7) == 42);
    /* Not our window: ours to filter only. */
    SetLastError(0);
    CHECK(!ChangeWindowMessageFilterEx(theirs, WM_APP + 1, MSGFLT_ALLOW, NULL));
    CHECK(GetLastError() == ERROR_ACCESS_DENIED);

    /* Open WM_APP + 1 for our window and WM_APP + 4 process-wide. */
    CHANGEFILTERSTRUCT cfs = {sizeof cfs, 99};
    CHECK(ChangeWindowMessageFilterEx(top, WM_APP + 1, MSGFLT_ALLOW, &cfs));
    CHECK(cfs.ExtStatus == MSGFLTINFO_NONE);
    cfs.ExtStatus = 99;
    CHECK(ChangeWindowMessageFilterEx(top, WM_APP + 1, MSGFLT_ALLOW, &cfs));
    CHECK(cfs.ExtStatus == MSGFLTINFO_ALREADYALLOWED_FORWND);
    CHANGEFILTERSTRUCT bad = {sizeof bad + 4, 0};
    SetLastError(0);
    CHECK(!ChangeWindowMessageFilterEx(top, WM_APP + 1, MSGFLT_ALLOW, &bad));
    CHECK(GetLastError() == ERROR_INVALID_PARAMETER);
    CHECK(ChangeWindowMessageFilter(WM_APP + 4, MSGFLT_ADD));
    CHECK(SetEvent(go2));
    HANDLE w2[2] = {step2, pi.hProcess};
    ok = 0;
    for (int i = 0; i < 2000 && !ok; i++) {
        MSG msg;
        while (PeekMessageW(&msg, NULL, 0, 0, PM_REMOVE))
            DispatchMessageW(&msg);
        DWORD r = MsgWaitForMultipleObjects(2, w2, FALSE, 10, QS_ALLINPUT);
        if (r == WAIT_OBJECT_0)
            ok = 1;
        else if (r == WAIT_OBJECT_0 + 1)
            break;
    }
    CHECK(ok);
    CHECK(GetWindowTextW(top, text, 64) > 0 && wcscmp(text, L"app 7 9") == 0);

    /* Close them again: the ExtStatus table. */
    cfs.ExtStatus = 99;
    CHECK(ChangeWindowMessageFilterEx(top, WM_APP + 1, MSGFLT_DISALLOW, &cfs));
    CHECK(cfs.ExtStatus == MSGFLTINFO_NONE);
    cfs.ExtStatus = 99;
    CHECK(ChangeWindowMessageFilterEx(top, WM_APP + 1, MSGFLT_DISALLOW, &cfs));
    CHECK(cfs.ExtStatus == MSGFLTINFO_ALREADYDISALLOWED_FORWND);
    cfs.ExtStatus = 99;
    CHECK(ChangeWindowMessageFilterEx(top, WM_APP + 4, MSGFLT_DISALLOW, &cfs));
    CHECK(cfs.ExtStatus == MSGFLTINFO_ALLOWED_HIGHER);
    cfs.ExtStatus = 99;
    CHECK(ChangeWindowMessageFilterEx(top, WM_APP + 1, MSGFLT_RESET, &cfs));
    CHECK(cfs.ExtStatus == MSGFLTINFO_NONE);
    CHECK(SetEvent(go3));
    for (int i = 0; i < 2000; i++) {
        MSG msg;
        while (PeekMessageW(&msg, NULL, 0, 0, PM_REMOVE))
            DispatchMessageW(&msg);
        if (MsgWaitForMultipleObjects(1, &pi.hProcess, FALSE, 10, QS_ALLINPUT) == WAIT_OBJECT_0)
            break;
    }
    DWORD code = 1;
    CHECK(GetExitCodeProcess(pi.hProcess, &code));
    CHECK(code == 0);
    CHECK(GetWindowTextW(top, text, 64) > 0 && wcscmp(text, L"app 7 9") == 0);
    DestroyWindow(top);
    printf("uipi_test: ok\n");
    return 0;
}
