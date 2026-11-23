/* Three top-level windows, shown one after another, for the kernel's
 * window hook (`hook=window=shot`, docs/OBSERVABILITY.md "Hooks"). The
 * program is the event source and nothing more: what it proves is in
 * the kernel's log and in the screendumps the host writes, so its own
 * checks are only that Windows let it do what the hook watches.
 *
 * Each window is a solid colour, painted before the next one is shown,
 * so a picture taken at any of the three says which one it caught. The
 * titles differ in their last words, which is what a hook's match
 * (`hook=window/three=shot`) selects on. Two of the four hold "three",
 * so a hook matching that with one shot to give has a window left over
 * after its count is spent: the line that says the count is spent is
 * then written, and it carries the number of windows the match passed
 * over, which is what proves the match chose rather than the count.
 *
 * The program stays up for a while after the last window: a hook may
 * ask for its picture a second or two after the event, and a process
 * that exited takes its windows (and, under `qemu_exit`, the machine)
 * with it before the host can take one.
 *
 * Prints "hook_test: ok" and returns 0, or the number of the failing
 * check. Built by build.sh with mingw-w64. */
#include <windows.h>
#include <stdio.h>

#define WINDOWS 4

static const struct {
    const wchar_t *title;
    COLORREF colour;
    int x, y, w, h;
} WANTED[WINDOWS] = {
    {L"hook_test one", RGB(0x20, 0x60, 0xc0), 60, 50, 420, 300},
    {L"hook_test two", RGB(0xc0, 0x50, 0x20), 180, 170, 420, 300},
    {L"hook_test three", RGB(0x20, 0x90, 0x50), 300, 290, 420, 300},
    {L"hook_test three again", RGB(0x80, 0x30, 0x90), 420, 410, 420, 300},
};

static HBRUSH brush[WINDOWS];

/* Reads the thread's messages for `ms` milliseconds, so the windows
 * paint and the desktop has something to show. */
static void pump(DWORD ms)
{
    DWORD start = GetTickCount();
    do {
        MSG m;
        while (PeekMessageW(&m, NULL, 0, 0, PM_REMOVE)) {
            TranslateMessage(&m);
            DispatchMessageW(&m);
        }
        Sleep(10);
    } while (GetTickCount() - start < ms);
}

static LRESULT CALLBACK proc(HWND w, UINT msg, WPARAM wp, LPARAM lp)
{
    if (msg == WM_PAINT) {
        PAINTSTRUCT ps;
        HDC dc = BeginPaint(w, &ps);
        int which = (int)(INT_PTR)GetWindowLongPtrW(w, GWLP_USERDATA);
        RECT r;
        GetClientRect(w, &r);
        FillRect(dc, &r, brush[which]);
        EndPaint(w, &ps);
        return 0;
    }
    return DefWindowProcW(w, msg, wp, lp);
}

int main(void)
{
    WNDCLASSEXW cls = {sizeof cls};
    HWND window[WINDOWS];
    int i;

    for (i = 0; i < WINDOWS; i++)
        brush[i] = CreateSolidBrush(WANTED[i].colour);

    cls.lpfnWndProc = proc;
    cls.hInstance = GetModuleHandleW(NULL);
    cls.lpszClassName = L"hook_test";
    cls.hCursor = LoadCursorW(NULL, (LPCWSTR)IDC_ARROW);
    if (!RegisterClassExW(&cls)) {
        printf("hook_test: RegisterClassExW failed (%lu)\n", GetLastError());
        return 1;
    }

    /* One window at a time: created hidden, shown, then painted, so the
     * kernel's event for each is on its own and a picture of one window
     * cannot be a picture of the next. */
    for (i = 0; i < WINDOWS; i++) {
        window[i] = CreateWindowExW(0, cls.lpszClassName, WANTED[i].title, WS_OVERLAPPEDWINDOW,
                                    WANTED[i].x, WANTED[i].y, WANTED[i].w, WANTED[i].h, NULL, NULL,
                                    cls.hInstance, NULL);
        if (!window[i]) {
            printf("hook_test: CreateWindowExW %d failed (%lu)\n", i, GetLastError());
            return 2;
        }
        SetWindowLongPtrW(window[i], GWLP_USERDATA, i);
        if (IsWindowVisible(window[i])) {
            printf("hook_test: window %d was visible before ShowWindow\n", i);
            return 3;
        }
        /* FALSE: it was not visible before (what the hook fires on). */
        if (ShowWindow(window[i], SW_SHOW)) {
            printf("hook_test: ShowWindow %d says it was visible\n", i);
            return 4;
        }
        if (!IsWindowVisible(window[i])) {
            printf("hook_test: window %d is not visible after ShowWindow\n", i);
            return 5;
        }
        UpdateWindow(window[i]);
        printf("hook_test: window %d shown\n", i);
        fflush(stdout);
        pump(700);
    }

    /* Showing a shown window again is nothing happening, so no hook
     * fires for it: the count of requests in the log is what says so. */
    if (!ShowWindow(window[0], SW_SHOW)) {
        printf("hook_test: ShowWindow of a shown window says it was hidden\n");
        return 6;
    }

    printf("hook_test: all %d windows shown\n", WINDOWS);
    fflush(stdout);
    /* Long enough for a hook's `+<secs>` to come due while the windows
     * are still on the desktop. */
    pump(3000);

    for (i = 0; i < WINDOWS; i++)
        DestroyWindow(window[i]);
    printf("hook_test: ok\n");
    return 0;
}
