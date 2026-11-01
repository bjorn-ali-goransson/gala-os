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
 * - GetClassNameW: the class name as registered (its case kept),
 *   truncated to the buffer (count without the NUL); 0 for a non-window.
 * - Menus: CreateMenu/CreatePopupMenu give empty menus (IsMenu);
 *   AppendMenuW adds strings, separators (id 0) and popups (GetSubMenu;
 *   GetMenuItemID -1); MF_BYCOMMAND reaches into submenus;
 *   EnableMenuItem and CheckMenuItem return the previous state (-1 for
 *   no such item); InsertMenuItemW/GetMenuItemInfoW/SetMenuItemInfoW with
 *   MENUITEMINFOW (cch: the length without the NUL when asked with no
 *   buffer); ModifyMenuW; RemoveMenu keeps a submenu, DeleteMenu and
 *   DestroyMenu destroy it; SetMenu/GetMenu/DrawMenuBar; GetSystemMenu
 *   (the same menu each time, with SC_CLOSE); SetMenuInfo/GetMenuInfo
 *   keep the style and menu data (MIM_APPLYTOSUBMENUS reaches submenus).
 * - RegisterWindowMessageW: a message in 0xC000-0xFFFF, the same for the
 *   same name in any case, another for another name.
 * - Child windows (WS_CHILD): GetParent, GetDlgCtrlID/GetDlgItem (the id
 *   given as hMenu), IsChild, GetAncestor(GA_ROOT), GetWindow(GW_CHILD,
 *   GW_HWNDNEXT: creation order), EnumChildWindows (every descendant);
 *   the client area is the whole window; ClientToScreen adds the
 *   parent's offset; SetWindowTextW/GetWindowTextW/GetWindowTextLengthW
 *   (through WM_SETTEXT/WM_GETTEXT); GetWindowLongW(GWL_STYLE, GWL_ID);
 *   EnableWindow returns whether it was disabled; MoveWindow and
 *   SetWindowPos send WM_SIZE and WM_MOVE (DefWindowProc, from
 *   WM_WINDOWPOSCHANGED), also resizing a top-level window; what a child
 *   draws shows through its parent's DC; ScrollWindow with no rectangle
 *   moves the children; DestroyWindow destroys the
 *   children too, WM_DESTROY reaching the parent first.
 * - SystemParametersInfoW: SPI_GETNONCLIENTMETRICS (cbSize as given; the
 *   caption height as SM_CYCAPTION; a message font with a negative height
 *   and a face), SPI_GETWORKAREA (within the screen), SPI_GETICONTITLELOGFONT,
 *   SPI_GETHIGHCONTRAST (off), SPI_GETWHEELSCROLLLINES (3); a wrong cbSize
 *   fails.
 * - Icons: CreateIconIndirect (a 32-bit colour bitmap and a mask) gives an
 *   icon; GetIconInfo hands out copies of its bitmaps (other handles, the
 *   same size); DrawIconEx draws its colours; a monochrome icon (no colour
 *   bitmap, the mask twice as tall: AND then XOR) draws white where the
 *   AND half is 0 and the XOR half 1; DestroyIcon succeeds once;
 *   LoadIconW/LoadCursorW(NULL, IDI_APPLICATION/IDC_ARROW) give the
 *   standard ones (GetIconInfo: fIcon TRUE/FALSE).
 * - uxtheme with themes off (classic): IsAppThemed/IsThemeActive FALSE,
 *   OpenThemeData NULL, GetThemeSysColor(NULL) the system colour,
 *   SetWindowTheme S_OK.
 * - The keyboard layout (US): VkKeyScanW gives the virtual key and the
 *   shift state ('a' -> 'A', 'A' -> Shift+'A', '!' -> Shift+'1', none:
 *   -1); MapVirtualKeyW between virtual keys, scan codes and characters.
 * - Accelerators: CreateAcceleratorTableW; CopyAcceleratorTableW (the
 *   count with no buffer, the entries back); TranslateAcceleratorW turns a
 *   matching WM_KEYDOWN into WM_COMMAND (high word 1) and returns TRUE, a
 *   key that needs Ctrl (not held) does not match; DestroyAcceleratorTable.
 * - The RECT helpers: SetRect/SetRectEmpty/IsRectEmpty, IntersectRect
 *   (empty: zeroed, FALSE), UnionRect, InflateRect, OffsetRect, PtInRect
 *   (right and bottom edges outside), EqualRect, SubtractRect (only when
 *   a whole side is covered).
 * - WM_SYSCOMMAND through DefWindowProc: SC_MAXIMIZE fills the work area
 *   (IsZoomed, WS_MAXIMIZE), SC_RESTORE brings the old rectangle back,
 *   SC_CLOSE closes (WM_CLOSE: the window is destroyed).
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
#include <uxtheme.h>
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

static HWND watched;
static LPARAM last_size, last_move;
static WPARAM last_command;
static int destroy_order[4], destroys;
static LRESULT CALLBACK wndproc(HWND hwnd, UINT msg, WPARAM wparam, LPARAM lparam) {
    wndproc_messages++;
    if (hwnd == watched && msg == WM_SIZE)
        last_size = lparam;
    if (hwnd == watched && msg == WM_MOVE)
        last_move = lparam;
    if (msg == WM_COMMAND)
        last_command = wparam;
    if (msg == WM_DESTROY && destroys < 4)
        destroy_order[destroys++] = (int)(INT_PTR)GetWindowLongPtrW(hwnd, GWLP_ID);
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
    WCHAR cls[64];
    CHECK(GetClassNameW(w, cls, 64) == 15 && wcscmp(cls, L"User32TestHooks") == 0);
    CHECK(GetClassNameW(w, cls, 5) == 4 && wcscmp(cls, L"User") == 0);
    CHECK(GetClassNameW((HWND)0x1234, cls, 64) == 0);
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

static void menus(void) {
    HMENU bar = CreateMenu(), file = CreatePopupMenu();
    CHECK(bar != NULL && file != NULL && IsMenu(bar) && GetMenuItemCount(bar) == 0);
    CHECK(AppendMenuW(file, MF_STRING, 101, L"&Open"));
    CHECK(AppendMenuW(file, MF_SEPARATOR, 0, NULL));
    CHECK(AppendMenuW(file, MF_STRING | MF_GRAYED, 102, L"E&xit"));
    CHECK(GetMenuItemCount(file) == 3 && GetMenuItemID(file, 0) == 101 && GetMenuItemID(file, 1) == 0);
    CHECK(AppendMenuW(bar, MF_POPUP, (UINT_PTR)file, L"&File"));
    CHECK(GetSubMenu(bar, 0) == file && GetMenuItemID(bar, 0) == (UINT)-1);
    WCHAR text[64];
    CHECK(GetMenuStringW(bar, 0, text, 64, MF_BYPOSITION) == 5 && wcscmp(text, L"&File") == 0);
    CHECK(GetMenuStringW(bar, 102, text, 64, MF_BYCOMMAND) == 5 && wcscmp(text, L"E&xit") == 0);
    CHECK(EnableMenuItem(bar, 102, MF_BYCOMMAND | MF_ENABLED) == MF_GRAYED);
    CHECK((GetMenuState(file, 102, MF_BYCOMMAND) & MF_GRAYED) == 0);
    CHECK(CheckMenuItem(file, 101, MF_BYCOMMAND | MF_CHECKED) == MF_UNCHECKED);
    CHECK(CheckMenuItem(file, 101, MF_BYCOMMAND | MF_CHECKED) == MF_CHECKED);
    CHECK(EnableMenuItem(file, 999, MF_BYCOMMAND | MF_GRAYED) == (UINT)-1);

    MENUITEMINFOW mii = {0};
    mii.cbSize = sizeof mii;
    mii.fMask = MIIM_ID | MIIM_STRING;
    mii.wID = 100;
    mii.dwTypeData = L"New";
    CHECK(InsertMenuItemW(file, 0, TRUE, &mii));
    CHECK(GetMenuItemCount(file) == 4 && GetMenuItemID(file, 0) == 100);
    MENUITEMINFOW q = {0};
    q.cbSize = sizeof q;
    q.fMask = MIIM_STRING;
    CHECK(GetMenuItemInfoW(file, 100, FALSE, &q) && q.cch == 3);
    q.dwTypeData = text;
    q.cch = 64;
    CHECK(GetMenuItemInfoW(file, 100, FALSE, &q) && q.cch == 3 && wcscmp(text, L"New") == 0);
    MENUITEMINFOW st = {0};
    st.cbSize = sizeof st;
    st.fMask = MIIM_STATE;
    st.fState = MFS_CHECKED;
    CHECK(SetMenuItemInfoW(file, 100, FALSE, &st));
    CHECK(GetMenuState(file, 100, MF_BYCOMMAND) & MF_CHECKED);
    CHECK(ModifyMenuW(file, 100, MF_BYCOMMAND | MF_STRING, 105, L"Create"));
    CHECK(GetMenuItemID(file, 0) == 105);
    CHECK(RemoveMenu(file, 0, MF_BYPOSITION) && GetMenuItemCount(file) == 3);
    CHECK(DeleteMenu(file, 102, MF_BYCOMMAND) && GetMenuItemCount(file) == 2);

    HWND w = CreateWindowExW(0, L"User32TestHooks", L"menus", WS_OVERLAPPEDWINDOW, 10, 10, 300,
                             200, NULL, NULL, GetModuleHandleW(NULL), NULL);
    CHECK(SetMenu(w, bar) && GetMenu(w) == bar && DrawMenuBar(w));
    HMENU sys = GetSystemMenu(w, FALSE);
    CHECK(sys != NULL && GetSystemMenu(w, FALSE) == sys);
    CHECK(GetMenuState(sys, SC_CLOSE, MF_BYCOMMAND) != (UINT)-1);
    CHECK(SetMenu(w, NULL) && GetMenu(w) == NULL);
    MENUINFO mi = {sizeof mi, MIM_STYLE | MIM_MENUDATA | MIM_APPLYTOSUBMENUS, MNS_NOTIFYBYPOS, 0, NULL, 0, 77};
    CHECK(SetMenuInfo(bar, &mi));
    MENUINFO got = {sizeof got, MIM_STYLE | MIM_MENUDATA};
    CHECK(GetMenuInfo(file, &got) && got.dwStyle == MNS_NOTIFYBYPOS && got.dwMenuData == 77);
    CHECK(DestroyMenu(bar) && !IsMenu(bar) && !IsMenu(file));
    DestroyWindow(w);
}

static int enumerated;
static BOOL CALLBACK count_children(HWND hwnd, LPARAM data) {
    if (data == 5 && IsWindow(hwnd))
        enumerated++;
    return TRUE;
}

static void children(void) {
    HINSTANCE inst = GetModuleHandleW(NULL);
    HWND p = CreateWindowExW(0, L"User32TestHooks", L"parent", WS_OVERLAPPEDWINDOW, 50, 60, 400, 300,
                             NULL, NULL, inst, NULL);
    CHECK(p != NULL);
    HWND c = CreateWindowExW(0, L"User32TestHooks", L"child", WS_CHILD | WS_VISIBLE, 10, 20, 100, 50, p,
                             (HMENU)7, inst, NULL);
    HWND c2 = CreateWindowExW(0, L"User32TestHooks", L"second", WS_CHILD | WS_VISIBLE, 10, 100, 100, 50,
                              p, (HMENU)8, inst, NULL);
    CHECK(c != NULL && c2 != NULL);
    CHECK(GetParent(c) == p && GetDlgCtrlID(c) == 7 && GetDlgItem(p, 8) == c2 && IsChild(p, c));
    CHECK(GetAncestor(c, GA_ROOT) == p && GetWindow(p, GW_CHILD) == c && GetWindow(c, GW_HWNDNEXT) == c2);
    CHECK(EnumChildWindows(p, count_children, 5) && enumerated == 2);
    RECT r;
    CHECK(GetClientRect(c, &r) && r.left == 0 && r.top == 0 && r.right == 100 && r.bottom == 50);
    POINT pc = {0, 0}, pp = {10, 20};
    CHECK(ClientToScreen(c, &pc) && ClientToScreen(p, &pp) && pc.x == pp.x && pc.y == pp.y);
    CHECK(GetWindowRect(c, &r) && r.left == pc.x && r.top == pc.y && r.right - r.left == 100);
    CHECK((GetWindowLongW(c, GWL_STYLE) & WS_CHILD) && GetWindowLongW(c, GWL_ID) == 7);
    CHECK(SetWindowTextW(c, L"hello") && GetWindowTextLengthW(c) == 5);
    WCHAR text[64];
    CHECK(GetWindowTextW(c, text, 64) == 5 && wcscmp(text, L"hello") == 0);
    CHECK(GetWindowTextW(p, text, 64) == 6 && wcscmp(text, L"parent") == 0);
    CHECK(EnableWindow(c, FALSE) == 0 && !IsWindowEnabled(c) && EnableWindow(c, TRUE) != 0);
    CHECK(IsWindowEnabled(c));

    watched = c;
    CHECK(MoveWindow(c, 30, 40, 120, 60, TRUE));
    CHECK(last_size == MAKELPARAM(120, 60) && last_move == MAKELPARAM(30, 40));
    CHECK(GetClientRect(c, &r) && r.right == 120 && r.bottom == 60);
    watched = p;
    CHECK(SetWindowPos(p, NULL, 0, 0, 500, 400, SWP_NOMOVE | SWP_NOZORDER));
    CHECK(GetWindowRect(p, &r) && r.right - r.left == 500 && r.bottom - r.top == 400);
    RECT client;
    CHECK(GetClientRect(p, &client) && client.right > 400 && client.right <= 500);
    CHECK(last_size == MAKELPARAM(client.right, client.bottom));

    /* A child draws in its parent's area: the parent's DC sees it. */
    ShowWindow(p, SW_SHOW);
    HDC cdc = GetDC(c);
    HBRUSH red = CreateSolidBrush(RGB(255, 0, 0));
    RECT all = {0, 0, 120, 60};
    CHECK(FillRect(cdc, &all, red));
    ReleaseDC(c, cdc);
    HDC pdc = GetDC(p);
    CHECK(GetPixel(pdc, 31, 41) == RGB(255, 0, 0) && GetPixel(pdc, 29, 39) != RGB(255, 0, 0));
    ReleaseDC(p, pdc);
    DeleteObject(red);

    RECT before, after;
    GetWindowRect(c2, &before);
    CHECK(ScrollWindow(p, 5, -7, NULL, NULL));
    GetWindowRect(c2, &after);
    CHECK(after.left == before.left + 5 && after.top == before.top - 7);
    watched = NULL;
    SetWindowLongPtrW(p, GWLP_ID, 1);
    destroys = 0;
    CHECK(DestroyWindow(p));
    CHECK(!IsWindow(c) && !IsWindow(c2) && destroys == 3 && destroy_order[0] == 1);
}

static void icons(void) {
    HDC screen = GetDC(NULL);
    BITMAPINFO bi = {0};
    bi.bmiHeader.biSize = sizeof bi.bmiHeader;
    bi.bmiHeader.biWidth = 16;
    bi.bmiHeader.biHeight = -16;
    bi.bmiHeader.biPlanes = 1;
    bi.bmiHeader.biBitCount = 32;
    DWORD *bits = NULL;
    HBITMAP color = CreateDIBSection(screen, &bi, DIB_RGB_COLORS, (void **)&bits, NULL, 0);
    CHECK(color != NULL && bits != NULL);
    for (int i = 0; i < 256; i++)
        bits[i] = 0x00ff0000; /* red, no alpha: the mask decides */
    BYTE zeros[32] = {0};
    HBITMAP mask = CreateBitmap(16, 16, 1, 1, zeros);
    ICONINFO ii = {TRUE, 0, 0, mask, color};
    HICON icon = CreateIconIndirect(&ii);
    CHECK(icon != NULL);
    ICONINFO back;
    CHECK(GetIconInfo(icon, &back) && back.fIcon && back.hbmColor != NULL && back.hbmColor != color);
    BITMAP bm;
    CHECK(GetObjectW(back.hbmColor, sizeof bm, &bm) && bm.bmWidth == 16 && bm.bmHeight == 16);
    DeleteObject(back.hbmColor);
    DeleteObject(back.hbmMask);

    HDC mem = CreateCompatibleDC(screen);
    HBITMAP canvas = CreateCompatibleBitmap(screen, 40, 40);
    HGDIOBJ old = SelectObject(mem, canvas);
    PatBlt(mem, 0, 0, 40, 40, WHITENESS);
    CHECK(DrawIconEx(mem, 2, 2, icon, 0, 0, 0, NULL, DI_NORMAL));
    CHECK(GetPixel(mem, 5, 5) == RGB(255, 0, 0) && GetPixel(mem, 20, 20) == RGB(255, 255, 255));

    /* Monochrome: AND half 0 (keep nothing), XOR half 1 (white). */
    BYTE mono[64];
    memset(mono, 0x00, 32);
    memset(mono + 32, 0xff, 32);
    HBITMAP mono_mask = CreateBitmap(16, 32, 1, 1, mono);
    ICONINFO mi = {TRUE, 0, 0, mono_mask, NULL};
    HICON mono_icon = CreateIconIndirect(&mi);
    CHECK(mono_icon != NULL);
    PatBlt(mem, 0, 0, 40, 40, BLACKNESS);
    CHECK(DrawIconEx(mem, 20, 20, mono_icon, 0, 0, 0, NULL, DI_NORMAL));
    CHECK(GetPixel(mem, 25, 25) == RGB(255, 255, 255) && GetPixel(mem, 5, 5) == RGB(0, 0, 0));
    SelectObject(mem, old);
    DeleteDC(mem);
    DeleteObject(canvas);

    CHECK(DestroyIcon(icon) && !DestroyIcon(icon) && DestroyIcon(mono_icon));
    DeleteObject(color);
    DeleteObject(mask);
    DeleteObject(mono_mask);
    HICON app = LoadIconW(NULL, MAKEINTRESOURCEW(32512));
    HCURSOR arrow = LoadCursorW(NULL, MAKEINTRESOURCEW(32512));
    CHECK(app != NULL && arrow != NULL);
    CHECK(GetIconInfo(arrow, &back) && !back.fIcon);
    DeleteObject(back.hbmColor);
    DeleteObject(back.hbmMask);
    ReleaseDC(NULL, screen);
}

int main(void) {
    hooks();
    timers();
    monitors();
    menus();
    children();
    icons();
    HWND zw = CreateWindowExW(0, L"User32TestHooks", L"zoom", WS_OVERLAPPEDWINDOW, 30, 40, 300, 200, NULL, NULL,
                              GetModuleHandleW(NULL), NULL);
    RECT wa, zr;
    SystemParametersInfoW(SPI_GETWORKAREA, 0, &wa, 0);
    SendMessageW(zw, WM_SYSCOMMAND, SC_MAXIMIZE, 0);
    CHECK(IsZoomed(zw) && (GetWindowLongW(zw, GWL_STYLE) & WS_MAXIMIZE));
    CHECK(GetWindowRect(zw, &zr) && zr.left == wa.left && zr.top == wa.top && zr.right == wa.right
          && zr.bottom == wa.bottom);
    SendMessageW(zw, WM_SYSCOMMAND, SC_RESTORE, 0);
    CHECK(!IsZoomed(zw) && GetWindowRect(zw, &zr) && zr.left == 30 && zr.top == 40 && zr.right == 330);
    SendMessageW(zw, WM_SYSCOMMAND, SC_CLOSE, 0);
    CHECK(!IsWindow(zw));
    RECT ra, rb, rc;
    CHECK(SetRect(&ra, 0, 0, 10, 10) && SetRect(&rb, 5, 5, 15, 15) && !IsRectEmpty(&ra));
    CHECK(IntersectRect(&rc, &ra, &rb) && rc.left == 5 && rc.right == 10);
    SetRect(&rb, 20, 20, 30, 30);
    CHECK(!IntersectRect(&rc, &ra, &rb) && IsRectEmpty(&rc) && rc.left == 0 && rc.right == 0);
    CHECK(UnionRect(&rc, &ra, &rb) && rc.left == 0 && rc.right == 30 && rc.bottom == 30);
    CHECK(InflateRect(&ra, 2, 3) && ra.left == -2 && ra.bottom == 13 && OffsetRect(&ra, 2, 3) && ra.left == 0);
    POINT in = {0, 0}, edge = {14, 5};
    CHECK(PtInRect(&ra, in) && !PtInRect(&ra, edge) && EqualRect(&ra, &ra));
    SetRect(&ra, 0, 0, 10, 10);
    SetRect(&rb, -1, -1, 11, 4);
    CHECK(SubtractRect(&rc, &ra, &rb) && rc.top == 4 && rc.bottom == 10 && rc.left == 0);
    SetRect(&rb, 2, 2, 4, 4);
    CHECK(SubtractRect(&rc, &ra, &rb) && EqualRect(&rc, &ra));
    CHECK(SetRectEmpty(&ra) && IsRectEmpty(&ra));
    ACCEL keys[2] = {{FVIRTKEY | FCONTROL, 'O', 100}, {FVIRTKEY, VK_F5, 101}};
    HACCEL acc = CreateAcceleratorTableW(keys, 2);
    CHECK(acc != NULL && CopyAcceleratorTableW(acc, NULL, 0) == 2);
    ACCEL back[2];
    CHECK(CopyAcceleratorTableW(acc, back, 2) == 2 && back[1].key == VK_F5 && back[1].cmd == 101);
    HWND aw = CreateWindowExW(0, L"User32TestHooks", L"accel", WS_OVERLAPPEDWINDOW, 0, 0, 200, 100, NULL, NULL,
                              GetModuleHandleW(NULL), NULL);
    MSG key = {aw, WM_KEYDOWN, VK_F5, 0};
    CHECK(TranslateAcceleratorW(aw, acc, &key) && last_command == MAKEWPARAM(101, 1));
    MSG ctrl_o = {aw, WM_KEYDOWN, 'O', 0};
    CHECK(!TranslateAcceleratorW(aw, acc, &ctrl_o));
    CHECK(DestroyAcceleratorTable(acc));
    DestroyWindow(aw);
    CHECK(VkKeyScanW(L'a') == 'A' && VkKeyScanW(L'A') == 0x141 && VkKeyScanW(L'!') == 0x131);
    CHECK(VkKeyScanW(L' ') == 0x20 && VkKeyScanW(L';') == 0xba && VkKeyScanW(0x263a) == -1);
    CHECK(MapVirtualKeyW('A', MAPVK_VK_TO_VSC) == 0x1e && MapVirtualKeyW(0x1e, MAPVK_VSC_TO_VK) == 'A');
    CHECK(MapVirtualKeyW('A', MAPVK_VK_TO_CHAR) == 'A' && MapVirtualKeyW(VK_OEM_1, MAPVK_VK_TO_CHAR) == ';');
    CHECK(!IsAppThemed() && !IsThemeActive());
    HWND tw = CreateWindowExW(0, L"User32TestHooks", L"themes", WS_OVERLAPPEDWINDOW, 0, 0, 200, 100, NULL, NULL,
                              GetModuleHandleW(NULL), NULL);
    CHECK(OpenThemeData(tw, L"BUTTON") == NULL && SetWindowTheme(tw, L"", L"") == S_OK);
    CHECK(GetThemeSysColor(NULL, COLOR_WINDOW) == GetSysColor(COLOR_WINDOW));
    DestroyWindow(tw);
    NONCLIENTMETRICSW ncm = {0};
    ncm.cbSize = sizeof ncm;
    CHECK(SystemParametersInfoW(SPI_GETNONCLIENTMETRICS, sizeof ncm, &ncm, 0));
    CHECK(ncm.iCaptionHeight == GetSystemMetrics(SM_CYCAPTION) && ncm.lfMessageFont.lfHeight < 0);
    CHECK(ncm.lfMessageFont.lfFaceName[0] != 0 && ncm.lfMenuFont.lfHeight != 0);
    ncm.cbSize = 7;
    CHECK(!SystemParametersInfoW(SPI_GETNONCLIENTMETRICS, 7, &ncm, 0));
    RECT work;
    CHECK(SystemParametersInfoW(SPI_GETWORKAREA, 0, &work, 0));
    CHECK(work.left == 0 && work.top == 0 && work.right == GetSystemMetrics(SM_CXSCREEN)
          && work.bottom <= GetSystemMetrics(SM_CYSCREEN) && work.bottom > 0);
    LOGFONTW icon;
    CHECK(SystemParametersInfoW(SPI_GETICONTITLELOGFONT, sizeof icon, &icon, 0) && icon.lfHeight != 0);
    HIGHCONTRASTW hc = {sizeof hc};
    CHECK(SystemParametersInfoW(SPI_GETHIGHCONTRAST, sizeof hc, &hc, 0) && !(hc.dwFlags & HCF_HIGHCONTRASTON));
    UINT lines = 0;
    CHECK(SystemParametersInfoW(SPI_GETWHEELSCROLLLINES, 0, &lines, 0) && lines == 3);
    UINT a = RegisterWindowMessageW(L"GalaTestMessage"), b = RegisterWindowMessageW(L"galatestmessage");
    CHECK(a >= 0xC000 && a <= 0xFFFF && a == b && RegisterWindowMessageW(L"Other") != a);
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
