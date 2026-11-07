/* user32 as real Windows behaves (documented behaviour only), for what
 * wxWidgets (FileZilla) uses, added as FileZilla's missing-features
 * ledger names it:
 *
 * - LoadBitmapW: a DDB of the screen's depth from an RT_BITMAP resource
 *   (resources.rc, square.bmp); an unknown name gives NULL.
 * - CreateWindowExW of an unregistered class: NULL,
 *   ERROR_CANNOT_FIND_WND_CLASS.
 * - GetUpdateRgn/GetUpdateRect: the client area while invalid, nothing
 *   once valid.
 * - RedrawWindow: RDW_INVALIDATE defers the WM_PAINT, RDW_UPDATENOW sends
 *   it before returning, RDW_VALIDATE takes the invalidation back.
 * - RegisterClipboardFormatW: 0xC000..0xFFFF, one per name, case-
 *   insensitive; GetClipboardFormatNameW (truncated to the buffer; none
 *   for predefined formats).
 * - SetWindowPos's messages: WM_WINDOWPOSCHANGING, WM_WINDOWPOSCHANGED,
 *   WM_SIZE in that order; a WINDOWPOS changed in WM_WINDOWPOSCHANGING is
 *   what the window gets (not asked with SWP_NOSENDCHANGING);
 *   GetWindowRect/GetClientRect are the new ones
 *   in WM_WINDOWPOSCHANGED and WM_SIZE; WM_SIZE's wParam is
 *   SIZE_MAXIMIZED after SC_MAXIMIZE and SIZE_RESTORED otherwise.
 * - WM_NCCALCSIZE (a custom frame, as Chromium's browser window draws
 *   its own): CreateWindowEx sends it (wParam FALSE, the window's
 *   rectangle); returning it unchanged makes the whole window the client
 *   area (GetClientRect the window's size, ClientToScreen(0, 0) its
 *   corner). SetWindowPos(SWP_FRAMECHANGED) sends it again (wParam TRUE,
 *   NCCALCSIZE_PARAMS): 8-pixel left, right and bottom borders and no top
 *   one give a client area that much smaller, at (8, 0); GetWindowRect
 *   stays. A window that leaves it to DefWindowProc has a caption.
 * - Activation: ShowWindow(SW_SHOW) of a top-level window activates it:
 *   WM_NCACTIVATE (TRUE), WM_ACTIVATE (WA_ACTIVE, lParam the window
 *   deactivated), then WM_SETFOCUS from DefWindowProc's WM_ACTIVATE;
 *   GetActiveWindow and GetFocus are it. Showing a second one deactivates
 *   the first (WM_NCACTIVATE FALSE, WM_ACTIVATE WA_INACTIVE with lParam
 *   the new one, WM_KILLFOCUS); SW_SHOWNOACTIVATE leaves the active one;
 *   SetActiveWindow activates again.
 * - Alt+F4: WM_SYSKEYDOWN with VK_F4 and the Alt bit (lParam bit 29)
 *   to DefWindowProc posts WM_SYSCOMMAND SC_CLOSE to the top-level
 *   window, which closes it once the message is dispatched; F4 alone
 *   does not.
 * - BeginDeferWindowPos/DeferWindowPos/EndDeferWindowPos: nothing moves
 *   until EndDeferWindowPos, which applies every position and size;
 *   SetWindowPos to the same size sends no WM_SIZE; WindowFromPoint finds
 *   the deepest visible window at a screen point.
 * - shell32's SHGetFileInfoW (SHGFI_USEFILEATTRIBUTES): the system image
 *   lists (small and large icon sizes) and type indexes, type names
 *   ("File folder", "<EXT> File"), display names, icons.
 * - Item ID lists: SHGetSpecialFolderLocation (the desktop's is empty),
 *   SHGetPathFromIDListW (none for CSIDL_DRIVES), SHGetMalloc.
 * - mpr's WNetGetConnectionW: ERROR_NOT_CONNECTED for C:, ERROR_BAD_DEVICE
 *   for a name that is no device.
 * - The posted-message queue is first in, first out: three posted, one
 *   taken, one more posted; the rest come out in posting order (a freed
 *   place in the queue does not put the newest first).
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
 * - What Inno Setup's IDE imports (Wine's menu.c test_getmenubarinfo and
 *   dialog.c record Windows' answers): GetMenuBarInfo(OBJID_MENU) of a
 *   window without a menu, an item past the last (-1, 200) or an unknown
 *   object id fails with the last error untouched; a NULL window is
 *   ERROR_INVALID_WINDOW_HANDLE, OBJID_CLIENT of a window that is no menu
 *   ERROR_INVALID_MENU_HANDLE, a wrong cbSize ERROR_INVALID_PARAMETER;
 *   item 0 of an empty bar is an empty rectangle with the menu; item n is
 *   GetMenuItemRect's rectangle of item n - 1, and the whole bar holds
 *   the items' rows. SendDlgItemMessageW sends to GetDlgItem's control
 *   (0 when there is none).
 * - What Scintilla (Inno Setup's IDE) imports: RegisterClassExA registers
 *   a class CreateWindowExW finds by its name (again:
 *   ERROR_CLASS_ALREADY_EXISTS); ImmNotifyIME of no input context fails,
 *   leaving ERROR_SUCCESS for NULL and ERROR_INVALID_HANDLE for a handle
 *   that is no context (Wine's imm32.c records Windows' answers).
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
 *   -1); MapVirtualKeyW between virtual keys, scan codes and characters;
 *   LoadKeyboardLayoutW("00000409") is the thread's layout; ToUnicode
 *   and ToUnicodeEx translate keys with a key state (Shift: capitals and
 *   the shifted row, Ctrl+letter: its control character, Enter '\r',
 *   arrows nothing).
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
 *   SC_MINIMIZE minimizes (IsIconic, WS_MINIMIZE, WM_SIZE SIZE_MINIMIZED,
 *   still visible) and SC_RESTORE restores it, SC_CLOSE closes (WM_CLOSE:
 *   the window is destroyed). ShowWindow SW_SHOWMAXIMIZED maximizes,
 *   SW_MINIMIZE minimizes, SW_RESTORE gives a minimized window back as
 *   it was (maximized) and a maximized one its rectangle.
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
 * - ole32's CoLockObjectExternal (a reference while locked) and
 *   RegisterDragDrop/RevokeDragDrop (a reference while registered;
 *   DRAGDROP_E_ALREADYREGISTERED, _NOTREGISTERED, _INVALIDHWND).
 * - ole32's OleInitialize (wx initializes OLE first): S_OK, then S_FALSE
 *   on the same thread; OleUninitialize once per call.
 * - Classes (Delphi's VCL registers one per control type and looks it
 *   up first): GetClassInfoW of a registered class gives its style,
 *   window procedure and extra bytes where WNDCLASSW has them (the
 *   procedure at offset 8, after the style's padding); GetClassInfoW of a
 *   system class (LISTBOX) gives a procedure a superclass can call;
 *   UnregisterClassW of a class with a window fails with
 *   ERROR_CLASS_HAS_WINDOWS, after which RegisterClassW of the same name
 *   fails with ERROR_CLASS_ALREADY_EXISTS; once the window is gone both
 *   succeed. A window's extra bytes (cbWndExtra 8) start zeroed and hold
 *   a LONG_PTR at 0 or LONGs at 0 and 4 (little-endian, overlapping);
 *   an offset past them fails with ERROR_INVALID_INDEX.
 * - Layered windows: GetLayeredWindowAttributes fails before
 *   SetLayeredWindowAttributes, then gives its colour key, alpha and
 *   flags; SetLayeredWindowAttributes fails on a window without
 *   WS_EX_LAYERED.
 * - AnimateWindow: shows a hidden window (visible after; activated with
 *   AW_ACTIVATE) and hides a shown one (AW_HIDE); it fails for a window
 *   already in that state and for a slide with no direction.
 * - DisableProcessWindowsGhosting (no result);
 *   ShutdownBlockReasonCreate/Query/Destroy: the reason given, its length
 *   with the NUL when asked without a buffer; none after Destroy.
 * - GetLastActivePopup: the window itself while none of its popups (a
 *   captioned one, as a dialog) was
 *   activated, then the popup activated last (a popup's own is itself);
 *   the owner again once the popup is gone. ShowOwnedPopups(FALSE) hides
 *   the owner's shown popups, TRUE shows them again.
 * - msctf.dll (loaded as Delphi's VCL loads it): SetInputScope and
 *   SetInputScopes on a window return S_OK.
 *
 * Prints "user32_test: ok" or exits with the number of the failing
 * check. */
#include <windows.h>
#include <imm.h>
#include <shellapi.h>
#include <winnetwk.h>
#include <shlobj.h>
#include <commctrl.h>
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
static int destroy_order[4], destroys, paints;
static LRESULT CALLBACK wndproc(HWND hwnd, UINT msg, WPARAM wparam, LPARAM lparam) {
    wndproc_messages++;
    if (msg == WM_PAINT)
        paints++;
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

/* A drop target that only counts its references. */
static LONG target_refs = 1;
static HRESULT STDMETHODCALLTYPE dt_query(IDropTarget *t, REFIID iid, void **out) {
    *out = t;
    target_refs++;
    return S_OK;
}
static ULONG STDMETHODCALLTYPE dt_addref(IDropTarget *t) { return ++target_refs; }
static ULONG STDMETHODCALLTYPE dt_release(IDropTarget *t) { return --target_refs; }
static HRESULT STDMETHODCALLTYPE dt_enter(IDropTarget *t, IDataObject *d, DWORD k, POINTL p, DWORD *e) { return S_OK; }
static HRESULT STDMETHODCALLTYPE dt_over(IDropTarget *t, DWORD k, POINTL p, DWORD *e) { return S_OK; }
static HRESULT STDMETHODCALLTYPE dt_leave(IDropTarget *t) { return S_OK; }
static HRESULT STDMETHODCALLTYPE dt_drop(IDropTarget *t, IDataObject *d, DWORD k, POINTL p, DWORD *e) { return S_OK; }
static IDropTargetVtbl dt_vtbl = {dt_query, dt_addref, dt_release, dt_enter, dt_over, dt_leave, dt_drop};
static IDropTarget drop_target = {&dt_vtbl};

static void ole(void) {
    CHECK(OleInitialize(NULL) == S_OK);
    CHECK(OleInitialize(NULL) == S_FALSE);
    /* CoLockObjectExternal holds a reference while locked. */
    CHECK(CoLockObjectExternal((IUnknown *)&drop_target, TRUE, TRUE) == S_OK && target_refs >= 2);
    CHECK(CoLockObjectExternal((IUnknown *)&drop_target, FALSE, TRUE) == S_OK && target_refs == 1);
    /* RegisterDragDrop keeps a reference until RevokeDragDrop. */
    HWND w = CreateWindowExW(0, L"User32TestHooks", L"drop", WS_OVERLAPPEDWINDOW, 0, 0, 100, 100, NULL, NULL,
                             GetModuleHandleW(NULL), NULL);
    CHECK(RegisterDragDrop(w, &drop_target) == S_OK && target_refs >= 2);
    CHECK(RegisterDragDrop(w, &drop_target) == DRAGDROP_E_ALREADYREGISTERED);
    CHECK(RevokeDragDrop(w) == S_OK && target_refs == 1);
    CHECK(RevokeDragDrop(w) == DRAGDROP_E_NOTREGISTERED);
    CHECK(RegisterDragDrop((HWND)(UINT_PTR)0x7ff0, &drop_target) == DRAGDROP_E_INVALIDHWND);
    DestroyWindow(w);
    OleUninitialize();
    OleUninitialize();
}

static int timer_proc_calls;
static UINT_PTR timer_proc_id;
static void CALLBACK timer_proc(HWND hwnd, UINT msg, UINT_PTR id, DWORD time) {
    if (hwnd == NULL && msg == WM_TIMER && id == timer_proc_id && time != 0)
        timer_proc_calls++;
}

static int enum_seen;
static HWND enum_want;
static BOOL CALLBACK enum_proc(HWND h, LPARAM data) {
    if (data == 42 && h == enum_want)
        enum_seen++;
    return TRUE;
}
static BOOL CALLBACK enum_stop(HWND h, LPARAM data) {
    (void)h;
    (void)data;
    return FALSE;
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

    // MsgWaitForMultipleObjects: WAIT_OBJECT_0 + count when a message of the
    // wake mask is queued, a handle's index when it is signalled.
    HANDLE ev = CreateEventW(NULL, TRUE, FALSE, NULL);
    CHECK(MsgWaitForMultipleObjects(0, NULL, FALSE, 20, QS_ALLINPUT) == WAIT_TIMEOUT);
    CHECK(MsgWaitForMultipleObjects(1, &ev, FALSE, 20, QS_ALLINPUT) == WAIT_TIMEOUT);
    CHECK(PostMessageW(w, WM_USER + 3, 0, 0));
    CHECK(MsgWaitForMultipleObjects(1, &ev, FALSE, 1000, QS_ALLINPUT) == WAIT_OBJECT_0 + 1);
    CHECK(MsgWaitForMultipleObjectsEx(0, NULL, 1000, QS_POSTMESSAGE, MWMO_INPUTAVAILABLE) == WAIT_OBJECT_0);
    CHECK(PeekMessageW(&m, w, 0, 0, PM_REMOVE) && m.message == WM_USER + 3);
    SetEvent(ev);
    CHECK(MsgWaitForMultipleObjects(1, &ev, FALSE, 1000, QS_ALLINPUT) == WAIT_OBJECT_0);
    start = GetTickCount();
    CHECK(SetTimer(w, 8, 30, NULL) == 8);
    CHECK(MsgWaitForMultipleObjects(0, NULL, FALSE, 5000, QS_TIMER) == WAIT_OBJECT_0);
    CHECK(GetTickCount() - start < 4000);
    CHECK(GetMessageW(&m, w, 0, 0) == 1 && m.message == WM_TIMER && m.wParam == 8);
    CHECK(KillTimer(w, 8));
    CloseHandle(ev);

    // GetMessageTime and GetMessagePos: those of the last message retrieved.
    CHECK(PostMessageW(w, WM_USER + 4, 0, 0));
    CHECK(GetMessageW(&m, w, 0, 0) == 1 && m.message == WM_USER + 4);
    CHECK((DWORD)GetMessageTime() == m.time);
    DWORD pos = GetMessagePos();
    CHECK((short)LOWORD(pos) == m.pt.x && (short)HIWORD(pos) == m.pt.y);

    // TrackMouseEvent (and comctl32's _TrackMouseEvent): WM_MOUSELEAVE asked.
    TRACKMOUSEEVENT tme = {sizeof tme, TME_LEAVE, w, HOVER_DEFAULT};
    CHECK(TrackMouseEvent(&tme));
    CHECK(_TrackMouseEvent(&tme));

    // EnumThreadWindows and EnumWindows: the top-level windows.
    enum_want = w;
    CHECK(EnumThreadWindows(GetCurrentThreadId(), enum_proc, 42) && enum_seen == 1);
    CHECK(!EnumThreadWindows(GetCurrentThreadId(), enum_stop, 0));
    CHECK(EnumWindows(enum_proc, 42) && enum_seen == 2);

    // A WS_POPUP window without a caption has no frame: all client area.
    HWND pop = CreateWindowExW(0, L"User32TestHooks", L"", WS_POPUP, 40, 50, 120, 70, NULL, NULL,
                               GetModuleHandleW(NULL), NULL);
    RECT pr;
    POINT po = {0, 0};
    CHECK(pop != NULL && GetClientRect(pop, &pr) && pr.right == 120 && pr.bottom == 70);
    CHECK(GetWindowRect(pop, &pr) && pr.left == 40 && pr.top == 50 && pr.right == 160);
    CHECK(ClientToScreen(pop, &po) && po.x == 40 && po.y == 50);
    CHECK(DestroyWindow(pop));

    // Scroll bars as data: SetScrollInfo returns the new position, kept
    // within min..max - (page - 1); GetScrollInfo gives what was set.
    SCROLLINFO si = {sizeof si, SIF_RANGE | SIF_PAGE | SIF_POS, 0, 100, 10, 50, 0};
    CHECK(SetScrollInfo(w, SB_VERT, &si, FALSE) == 50);
    SCROLLINFO got = {sizeof got, SIF_ALL};
    CHECK(GetScrollInfo(w, SB_VERT, &got) && got.nMin == 0 && got.nMax == 100 && got.nPage == 10
          && got.nPos == 50);
    si.fMask = SIF_POS;
    si.nPos = 200;
    CHECK(SetScrollInfo(w, SB_VERT, &si, FALSE) == 91 && GetScrollPos(w, SB_VERT) == 91);
    CHECK(SetScrollPos(w, SB_VERT, 5, FALSE) == 91 && GetScrollPos(w, SB_VERT) == 5);
    int lo = -1, hi = -1;
    CHECK(SetScrollRange(w, SB_HORZ, 0, 30, FALSE) && GetScrollRange(w, SB_HORZ, &lo, &hi) && lo == 0
          && hi == 30);
    CHECK(ShowScrollBar(w, SB_VERT, TRUE) && EnableScrollBar(w, SB_VERT, ESB_ENABLE_BOTH));

    // FlashWindowEx: whether the caption was active before.
    FLASHWINFO fw = {sizeof fw, w, FLASHW_ALL, 2, 0};
    CHECK(!FlashWindowEx(&fw) == (GetActiveWindow() != w));
    fw.dwFlags = FLASHW_STOP;
    CHECK(!FlashWindowEx(&fw) == (GetActiveWindow() != w));

    // UnregisterClassW: not while a window of it is there; then gone.
    WNDCLASSW wc = {0};
    wc.lpfnWndProc = DefWindowProcW;
    wc.hInstance = GetModuleHandleW(NULL);
    wc.lpszClassName = L"User32TestGoing";
    CHECK(RegisterClassW(&wc) != 0);
    HWND going = CreateWindowExW(0, L"User32TestGoing", L"", WS_OVERLAPPED, 0, 0, 10, 10, NULL, NULL,
                                 GetModuleHandleW(NULL), NULL);
    CHECK(going != NULL);
    CHECK(!UnregisterClassW(L"User32TestGoing", GetModuleHandleW(NULL))
          && GetLastError() == ERROR_CLASS_HAS_WINDOWS);
    CHECK(DestroyWindow(going));
    CHECK(UnregisterClassW(L"User32TestGoing", GetModuleHandleW(NULL)));
    CHECK(!UnregisterClassW(L"User32TestGoing", GetModuleHandleW(NULL))
          && GetLastError() == ERROR_CLASS_DOES_NOT_EXIST);

    CHECK(DestroyWindow(w));
}

static int sizes_seen;
static LRESULT CALLBACK size_proc(HWND h, UINT msg, WPARAM wp, LPARAM lp) {
    if (msg == WM_SIZE)
        sizes_seen++;
    return DefWindowProcW(h, msg, wp, lp);
}

/* A window's menu bar is part of its frame: SM_CYMENU high. */
static void menu_bar(void) {
    int bar = GetSystemMetrics(SM_CYMENU);
    CHECK(bar > 0);
    RECT plain = {0, 0, 100, 100}, with_menu = {0, 0, 100, 100};
    CHECK(AdjustWindowRectEx(&plain, WS_OVERLAPPEDWINDOW, FALSE, 0));
    CHECK(AdjustWindowRectEx(&with_menu, WS_OVERLAPPEDWINDOW, TRUE, 0));
    CHECK(plain.top < 0 && plain.right > 100 && with_menu.top == plain.top - bar);
    CHECK(with_menu.bottom == plain.bottom && with_menu.left == plain.left);
    RECT popup = {10, 10, 50, 50};
    CHECK(AdjustWindowRectEx(&popup, WS_POPUP, FALSE, 0) && popup.left == 10 && popup.bottom == 50);

    WNDCLASSW wc = {0};
    wc.lpfnWndProc = size_proc;
    wc.hInstance = GetModuleHandleW(NULL);
    wc.lpszClassName = L"User32TestMenuBar";
    CHECK(RegisterClassW(&wc) != 0);
    HMENU file = CreatePopupMenu(), bar_menu = CreateMenu();
    CHECK(AppendMenuW(file, MF_STRING, 100, L"E&xit") && AppendMenuW(bar_menu, MF_POPUP, (UINT_PTR)file, L"&File"));
    // A window made with the menu's frame size and the menu: 100 x 100 inside.
    HWND w = CreateWindowExW(0, L"User32TestMenuBar", L"menu", WS_OVERLAPPEDWINDOW, 0, 0,
                             with_menu.right - with_menu.left, with_menu.bottom - with_menu.top, NULL,
                             bar_menu, GetModuleHandleW(NULL), NULL);
    RECT c;
    CHECK(w != NULL && GetMenu(w) == bar_menu);
    CHECK(GetClientRect(w, &c) && c.right == 100 && c.bottom == 100);
    // Without it the client area grows by the bar, with WM_SIZE.
    sizes_seen = 0;
    CHECK(SetMenu(w, NULL) && GetMenu(w) == NULL);
    CHECK(GetClientRect(w, &c) && c.right == 100 && c.bottom == 100 + bar && sizes_seen > 0);
    POINT o = {0, 0};
    CHECK(ClientToScreen(w, &o) && o.y == -plain.top);
    CHECK(SetMenu(w, bar_menu) && GetClientRect(w, &c) && c.bottom == 100);
    o.x = o.y = 0;
    CHECK(ClientToScreen(w, &o) && o.y == -with_menu.top);
    CHECK(DrawMenuBar(w));
    CHECK(DestroyWindow(w));
    // The menu went with the window.
    CHECK(!IsMenu(bar_menu) && !IsMenu(file));
}

/* GetMenuBarInfo and SendDlgItemMessageW (what Inno Setup's IDE imports). */
static void menu_bar_info(void) {
    MENUBARINFO mbi;
    mbi.cbSize = sizeof mbi;
    HWND w = CreateWindowExW(0, L"User32TestHooks", L"bar info", WS_OVERLAPPEDWINDOW | WS_VISIBLE, 20, 20, 200,
                             150, NULL, NULL, GetModuleHandleW(NULL), NULL);
    CHECK(w != NULL);
    SetLastError(0xdeadbeef);
    CHECK(!GetMenuBarInfo(w, OBJID_MENU, 0, &mbi) && GetLastError() == 0xdeadbeef);
    HMENU bar = CreateMenu();
    CHECK(bar != NULL && SetMenu(w, bar));
    SetLastError(0xdeadbeef);
    CHECK(!GetMenuBarInfo(NULL, OBJID_CLIENT, 0, &mbi) && GetLastError() == ERROR_INVALID_WINDOW_HANDLE);
    SetLastError(0xdeadbeef);
    CHECK(!GetMenuBarInfo(w, OBJID_CLIENT, 0, &mbi) && GetLastError() == ERROR_INVALID_MENU_HANDLE);
    SetLastError(0xdeadbeef);
    CHECK(!GetMenuBarInfo(w, OBJID_MENU, -1, &mbi) && GetLastError() == 0xdeadbeef);
    mbi.cbSize = 1000;
    SetLastError(0xdeadbeef);
    CHECK(!GetMenuBarInfo(w, OBJID_MENU, 0, &mbi) && GetLastError() == ERROR_INVALID_PARAMETER);
    mbi.cbSize = sizeof mbi;
    SetLastError(0xdeadbeef);
    CHECK(!GetMenuBarInfo(w, 123, 0, &mbi) && GetLastError() == 0xdeadbeef);
    CHECK(GetMenuBarInfo(w, OBJID_MENU, 0, &mbi));
    CHECK(mbi.rcBar.left == 0 && mbi.rcBar.top == 0 && mbi.rcBar.right == 0 && mbi.rcBar.bottom == 0);
    CHECK(mbi.hMenu == bar && !mbi.fBarFocused && !mbi.fFocused);
    CHECK(AppendMenuW(bar, MF_STRING, 100, L"item 1") && AppendMenuW(bar, MF_STRING, 101, L"item 2"));
    CHECK(SetMenu(w, bar));
    SetLastError(0xdeadbeef);
    CHECK(!GetMenuBarInfo(w, OBJID_MENU, 200, &mbi) && GetLastError() == 0xdeadbeef);
    RECT first, second;
    CHECK(GetMenuItemRect(w, bar, 0, &first) && GetMenuItemRect(w, bar, 1, &second));
    CHECK(first.right <= second.left && first.top == second.top && first.bottom > first.top);
    CHECK(GetMenuBarInfo(w, OBJID_MENU, 0, &mbi) && mbi.hMenu == bar);
    CHECK(mbi.rcBar.top == first.top && mbi.rcBar.bottom == first.bottom);
    CHECK(mbi.rcBar.left <= first.left && mbi.rcBar.right >= second.right);
    CHECK(GetMenuBarInfo(w, OBJID_MENU, 2, &mbi) && EqualRect(&mbi.rcBar, &second) && mbi.hMenu == bar);
    CHECK(!mbi.fBarFocused && !mbi.fFocused);

    HWND edit = CreateWindowExW(0, L"EDIT", L"", WS_CHILD | WS_VISIBLE, 0, 0, 80, 20, w, (HMENU)5,
                                GetModuleHandleW(NULL), NULL);
    CHECK(edit != NULL);
    CHECK(SendDlgItemMessageW(w, 5, WM_SETTEXT, 0, (LPARAM)L"sent") == TRUE);
    wchar_t text[16];
    CHECK(GetWindowTextW(edit, text, 16) == 4 && wcscmp(text, L"sent") == 0);
    CHECK(SendDlgItemMessageW(w, 5, WM_GETTEXTLENGTH, 0, 0) == 4);
    CHECK(SendDlgItemMessageW(w, 99, WM_GETTEXTLENGTH, 0, 0) == 0);
    CHECK(DestroyWindow(w));
}

/* What Scintilla (in Inno Setup's IDE) imports besides: RegisterClassExA
 * and imm32's input-context functions. */
static void scintilla_imports(void) {
    WNDCLASSEXA wca;
    memset(&wca, 0, sizeof wca);
    wca.cbSize = sizeof wca;
    wca.lpfnWndProc = DefWindowProcA;
    wca.hInstance = GetModuleHandleW(NULL);
    wca.lpszClassName = "User32TestAnsiClass";
    ATOM atom = RegisterClassExA(&wca);
    CHECK(atom >= 0xC000);
    SetLastError(0);
    CHECK(!RegisterClassExA(&wca) && GetLastError() == ERROR_CLASS_ALREADY_EXISTS);
    HWND w = CreateWindowExW(0, L"User32TestAnsiClass", L"ansi", WS_POPUP, 0, 0, 10, 10, NULL, NULL,
                             GetModuleHandleW(NULL), NULL);
    wchar_t cls[32];
    CHECK(w != NULL && GetClassNameW(w, cls, 32) == 19 && wcscmp(cls, L"User32TestAnsiClass") == 0);
    CHECK(DestroyWindow(w));
    /* No input context: ImmNotifyIME fails, a NULL one leaving no error,
     * one that is not a context ERROR_INVALID_HANDLE. */
    SetLastError(0xdeadbeef);
    CHECK(!ImmNotifyIME(NULL, NI_COMPOSITIONSTR, CPS_CANCEL, 0) && GetLastError() == ERROR_SUCCESS);
    SetLastError(0xdeadbeef);
    CHECK(!ImmNotifyIME((HIMC)0xdeadcafe, NI_COMPOSITIONSTR, CPS_CANCEL, 0) && GetLastError() == ERROR_INVALID_HANDLE);
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

    /* A window's icons: WM_SETICON returns the previous one, WM_GETICON
     * what was set (the taskbar and caption show it). */
    HWND iw = CreateWindowExW(0, L"User32TestHooks", L"icons", WS_OVERLAPPEDWINDOW, 10, 10, 200, 100,
                              NULL, NULL, GetModuleHandleW(NULL), NULL);
    CHECK(iw != NULL);
    CHECK(SendMessageW(iw, WM_SETICON, ICON_BIG, (LPARAM)icon) == 0);
    CHECK(SendMessageW(iw, WM_SETICON, ICON_SMALL, (LPARAM)mono_icon) == 0);
    CHECK(SendMessageW(iw, WM_GETICON, ICON_BIG, 0) == (LRESULT)icon);
    CHECK(SendMessageW(iw, WM_GETICON, ICON_SMALL, 0) == (LRESULT)mono_icon);
    CHECK(SendMessageW(iw, WM_SETICON, ICON_BIG, 0) == (LRESULT)icon);
    CHECK(SendMessageW(iw, WM_GETICON, ICON_BIG, 0) == 0);
    CHECK(DestroyWindow(iw));

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

// LoadBitmapW: a DDB of the screen's depth from the module's RT_BITMAP
// resource (named "square" in resources.rc: bottom row blue, green; top
// row red, white); an unknown name gives NULL.
static void bitmaps(void) {
    HBITMAP b = LoadBitmapW(GetModuleHandleW(NULL), L"SQUARE");
    CHECK(b != NULL);
    BITMAP bm;
    CHECK(GetObjectW(b, sizeof bm, &bm) == sizeof bm && bm.bmWidth == 2 && bm.bmHeight == 2 && bm.bmBitsPixel == 32);
    HDC screen = GetDC(NULL);
    HDC mem = CreateCompatibleDC(screen);
    HGDIOBJ old = SelectObject(mem, b);
    CHECK(GetPixel(mem, 0, 0) == RGB(255, 0, 0) && GetPixel(mem, 1, 0) == RGB(255, 255, 255));
    CHECK(GetPixel(mem, 0, 1) == RGB(0, 0, 255) && GetPixel(mem, 1, 1) == RGB(0, 255, 0));
    SelectObject(mem, old);
    DeleteDC(mem);
    ReleaseDC(NULL, screen);
    CHECK(DeleteObject(b));
    CHECK(LoadBitmapW(GetModuleHandleW(NULL), L"NOSUCH") == NULL);
}

// RedrawWindow: RDW_INVALIDATE marks the window for painting (WM_PAINT
// comes later), RDW_UPDATENOW paints it before returning, RDW_VALIDATE
// takes the invalidation back.
/* GetDialogBaseUnits: the system font's average width (low word) and
 * height (high word), both positive. */
static void dialog_units(void) {
    LONG u = GetDialogBaseUnits();
    CHECK(LOWORD(u) > 0 && HIWORD(u) > LOWORD(u));
    /* EndMenu with no menu open succeeds. */
    CHECK(EndMenu());
    /* Activation: SetForegroundWindow makes a window the foreground and
     * (its thread's) active one; SetActiveWindow returns the previous
     * active window; BringWindowToTop succeeds. */
    HWND w = CreateWindowExW(0, L"User32TestHooks", L"active", WS_OVERLAPPEDWINDOW | WS_VISIBLE, 0, 0, 100, 100, NULL,
                             NULL, GetModuleHandleW(NULL), NULL);
    CHECK(SetForegroundWindow(w) && GetForegroundWindow() == w && GetActiveWindow() == w);
    CHECK(SetActiveWindow(w) == w && BringWindowToTop(w));
    DestroyWindow(w);
}

static WNDPROC listbox_proc;
static LRESULT CALLBACK sub_listbox(HWND hwnd, UINT msg, WPARAM wparam, LPARAM lparam) {
    return CallWindowProcW(listbox_proc, hwnd, msg, wparam, lparam);
}

static void classes(void) {
    HINSTANCE inst = GetModuleHandleW(NULL);
    WNDCLASSW wc = {0};
    wc.style = CS_HREDRAW | CS_VREDRAW;
    wc.lpfnWndProc = wndproc;
    wc.cbWndExtra = 8;
    wc.hInstance = inst;
    wc.lpszClassName = L"User32TestClassInfo";
    CHECK(RegisterClassW(&wc) != 0);
    WNDCLASSW got;
    memset(&got, 0xcc, sizeof got);
    CHECK(GetClassInfoW(inst, L"User32TestClassInfo", &got));
    CHECK(got.lpfnWndProc == wndproc && got.style == (CS_HREDRAW | CS_VREDRAW));
    /* A superclass of a system class, as Delphi's controls are. */
    WNDCLASSW lb;
    CHECK(GetClassInfoW(NULL, L"LISTBOX", &lb) && lb.lpfnWndProc != NULL);
    listbox_proc = lb.lpfnWndProc;
    lb.lpfnWndProc = sub_listbox;
    lb.hInstance = inst;
    lb.lpszClassName = L"User32TestListBox";
    CHECK(RegisterClassW(&lb) != 0);
    HWND parent = CreateWindowExW(0, L"User32TestClassInfo", L"classes", WS_OVERLAPPEDWINDOW, 0, 0, 200, 100, NULL,
                                  NULL, inst, NULL);
    HWND list = CreateWindowExW(0, L"User32TestListBox", L"", WS_CHILD | LBS_NOTIFY, 0, 0, 100, 50, parent, NULL,
                                inst, NULL);
    CHECK(parent != NULL && list != NULL);
    /* The class's 8 extra bytes: zeroed, a pointer-sized long at 0, 4-byte
     * longs at 0 and 4; nothing past them (ERROR_INVALID_INDEX). */
    CHECK(got.cbWndExtra == 8);
    CHECK(GetWindowLongPtrW(parent, 0) == 0);
    CHECK(SetWindowLongPtrW(parent, 0, (LONG_PTR)0x1122334455667788ll) == 0);
    CHECK(GetWindowLongPtrW(parent, 0) == (LONG_PTR)0x1122334455667788ll);
    CHECK(GetWindowLongW(parent, 4) == 0x11223344 && GetWindowLongW(parent, 0) == 0x55667788);
    CHECK(SetWindowLongW(parent, 4, -2) == 0x11223344 && GetWindowLongPtrW(parent, 0) == (LONG_PTR)0xfffffffe55667788ll);
    SetLastError(0);
    CHECK(GetWindowLongPtrW(parent, 4) == 0 && GetLastError() == ERROR_INVALID_INDEX);
    SetLastError(0);
    CHECK(SetWindowLongW(parent, 8, 1) == 0 && GetLastError() == ERROR_INVALID_INDEX);
    CHECK(SendMessageW(list, LB_ADDSTRING, 0, (LPARAM)L"one") == 0 && SendMessageW(list, LB_GETCOUNT, 0, 0) == 1);
    SetLastError(0);
    CHECK(!UnregisterClassW(L"User32TestClassInfo", inst) && GetLastError() == ERROR_CLASS_HAS_WINDOWS);
    SetLastError(0);
    CHECK(RegisterClassW(&wc) == 0 && GetLastError() == ERROR_CLASS_ALREADY_EXISTS);
    DestroyWindow(parent);
    CHECK(UnregisterClassW(L"User32TestClassInfo", inst) && RegisterClassW(&wc) != 0);
    CHECK(UnregisterClassW(L"User32TestListBox", inst));
}

static void effects(void) {
    HINSTANCE inst = GetModuleHandleW(NULL);
    HWND w = CreateWindowExW(WS_EX_LAYERED, L"User32TestHooks", L"layered", WS_OVERLAPPEDWINDOW, 0, 0, 200, 100,
                             NULL, NULL, inst, NULL);
    COLORREF key = 1;
    BYTE alpha = 1;
    DWORD flags = 1;
    CHECK(w != NULL && !GetLayeredWindowAttributes(w, &key, &alpha, &flags));
    CHECK(SetLayeredWindowAttributes(w, RGB(1, 2, 3), 200, LWA_ALPHA | LWA_COLORKEY));
    CHECK(GetLayeredWindowAttributes(w, &key, &alpha, &flags));
    CHECK(key == RGB(1, 2, 3) && alpha == 200 && flags == (LWA_ALPHA | LWA_COLORKEY));
    CHECK(SetLayeredWindowAttributes(w, 0, 255, LWA_ALPHA) && GetLayeredWindowAttributes(w, NULL, &alpha, &flags));
    CHECK(alpha == 255 && flags == LWA_ALPHA);
    DestroyWindow(w);
    w = CreateWindowExW(0, L"User32TestHooks", L"plain", WS_OVERLAPPEDWINDOW, 0, 0, 200, 100, NULL, NULL, inst,
                        NULL);
    CHECK(!SetLayeredWindowAttributes(w, 0, 128, LWA_ALPHA));
    /* AnimateWindow. */
    CHECK(!IsWindowVisible(w) && !AnimateWindow(w, 10, AW_HIDE | AW_BLEND));
    CHECK(!AnimateWindow(w, 10, AW_SLIDE) && !IsWindowVisible(w));
    CHECK(AnimateWindow(w, 10, AW_BLEND | AW_ACTIVATE) && IsWindowVisible(w) && GetActiveWindow() == w);
    CHECK(!AnimateWindow(w, 10, AW_BLEND));
    CHECK(AnimateWindow(w, 10, AW_HIDE | AW_SLIDE | AW_VER_POSITIVE) && !IsWindowVisible(w));
    CHECK(AnimateWindow(w, 10, AW_CENTER) && IsWindowVisible(w));
    /* Ghosting and shutdown reasons. */
    DisableProcessWindowsGhosting();
    WCHAR reason[32];
    DWORD n = 0;
    CHECK(ShutdownBlockReasonCreate(w, L"Installing"));
    CHECK(ShutdownBlockReasonQuery(w, NULL, &n) && n == 11);
    n = 32;
    CHECK(ShutdownBlockReasonQuery(w, reason, &n) && wcscmp(reason, L"Installing") == 0);
    CHECK(ShutdownBlockReasonDestroy(w));
    n = 32;
    CHECK(!ShutdownBlockReasonQuery(w, reason, &n));
    /* Owned popups. */
    HWND p = CreateWindowExW(0, L"User32TestHooks", L"popup", WS_POPUP | WS_CAPTION, 0, 0, 50, 50, w, NULL, inst, NULL);
    CHECK(p != NULL && GetLastActivePopup(w) == w);
    ShowWindow(p, SW_SHOW);
    CHECK(GetActiveWindow() == p && GetLastActivePopup(w) == p && GetLastActivePopup(p) == p);
    CHECK(ShowOwnedPopups(w, FALSE) && !IsWindowVisible(p));
    CHECK(ShowOwnedPopups(w, TRUE) && IsWindowVisible(p));
    DestroyWindow(p);
    CHECK(GetLastActivePopup(w) == w);
    /* Input scopes (msctf.dll). */
    HMODULE tsf = LoadLibraryW(L"msctf.dll");
    CHECK(tsf != NULL);
    HRESULT (WINAPI *set_scope)(HWND, int) = (void *)GetProcAddress(tsf, "SetInputScope");
    HRESULT (WINAPI *set_scopes)(HWND, const int *, UINT, WCHAR **, UINT, WCHAR *, WCHAR *) =
        (void *)GetProcAddress(tsf, "SetInputScopes");
    int scopes[2] = {1 /* IS_URL */, 5 /* IS_EMAIL_SMTPEMAILADDRESS */};
    CHECK(set_scope != NULL && set_scope(w, 1) == S_OK);
    CHECK(set_scopes != NULL && set_scopes(w, scopes, 2, NULL, 0, NULL, NULL) == S_OK);
    DestroyWindow(w);
}

static void unknown_class(void) {
    SetLastError(0);
    CHECK(CreateWindowExW(0, L"GalaNoSuchClass", L"x", WS_OVERLAPPEDWINDOW, 0, 0, 10, 10, NULL, NULL,
                          GetModuleHandleW(NULL), NULL) == NULL);
    CHECK(GetLastError() == ERROR_CANNOT_FIND_WND_CLASS);
}

static void redraw(void) {
    HWND w = CreateWindowExW(0, L"User32TestHooks", L"redraw", WS_OVERLAPPEDWINDOW | WS_VISIBLE, 10, 10, 200, 100,
                             NULL, NULL, GetModuleHandleW(NULL), NULL);
    CHECK(w != NULL && UpdateWindow(w));
    int before = paints;
    CHECK(RedrawWindow(w, NULL, NULL, RDW_INVALIDATE | RDW_UPDATENOW) && paints == before + 1);
    CHECK(RedrawWindow(w, NULL, NULL, RDW_INVALIDATE | RDW_ERASE) && paints == before + 1);
    CHECK(RedrawWindow(w, NULL, NULL, RDW_VALIDATE) && UpdateWindow(w) && paints == before + 1);
    /* The update region: the client area while invalid (gala keeps no
     * finer one), empty once valid; GetUpdateRect alike. */
    HRGN upd = CreateRectRgn(0, 0, 0, 0);
    RECT cr, ur, box;
    GetClientRect(w, &cr);
    CHECK(GetUpdateRgn(w, upd, FALSE) == NULLREGION && !GetUpdateRect(w, &ur, FALSE));
    InvalidateRect(w, NULL, FALSE);
    CHECK(GetUpdateRgn(w, upd, FALSE) == SIMPLEREGION && GetRgnBox(upd, &box) == SIMPLEREGION);
    CHECK(box.right == cr.right && box.bottom == cr.bottom);
    CHECK(GetUpdateRect(w, &ur, FALSE) && ur.right == cr.right);
    ValidateRect(w, NULL);
    DeleteObject(upd);
    RECT r = {0, 0, 20, 20};
    CHECK(RedrawWindow(w, &r, NULL, RDW_INVALIDATE | RDW_FRAME | RDW_ALLCHILDREN | RDW_UPDATENOW));
    CHECK(paints == before + 2);
    CHECK(DestroyWindow(w));
}

// Registered clipboard formats: one value in 0xC000..0xFFFF per name,
// case-insensitive; GetClipboardFormatNameW gives the name as first
// registered; the predefined formats have none.
static void clipboard_formats(void) {
    UINT f = RegisterClipboardFormatW(L"Gala Format");
    CHECK(f >= 0xC000 && f <= 0xFFFF && RegisterClipboardFormatW(L"GALA FORMAT") == f);
    CHECK(RegisterClipboardFormatW(L"Another") != f);
    WCHAR name[64];
    CHECK(GetClipboardFormatNameW(f, name, 64) == 11 && wcscmp(name, L"Gala Format") == 0);
    CHECK(GetClipboardFormatNameW(f, name, 5) == 4 && wcscmp(name, L"Gala") == 0);
    CHECK(GetClipboardFormatNameW(CF_TEXT, name, 64) == 0);
}

// SetWindowPos's messages (what wx's layout relies on when a window is
// resized, e.g. snapped): WM_WINDOWPOSCHANGING may change the WINDOWPOS
// and the window gets what it says; WM_WINDOWPOSCHANGED and then WM_SIZE
// (DefWindowProc) see the new rectangle; WM_SIZE's wParam is
// SIZE_MAXIMIZED after SC_MAXIMIZE, else SIZE_RESTORED.
static int pos_force_cx, pos_seq[8], pos_n, pos_changed_w, pos_size_client_w;
static WPARAM pos_size_wparam;
static LRESULT CALLBACK pos_proc(HWND hwnd, UINT msg, WPARAM wparam, LPARAM lparam) {
    RECT r;
    if (msg == WM_WINDOWPOSCHANGING) {
        WINDOWPOS *p = (WINDOWPOS *)lparam;
        if (pos_n < 8)
            pos_seq[pos_n++] = 1;
        if (pos_force_cx && !(p->flags & SWP_NOSIZE))
            p->cx = pos_force_cx;
    } else if (msg == WM_WINDOWPOSCHANGED) {
        if (pos_n < 8)
            pos_seq[pos_n++] = 2;
        GetWindowRect(hwnd, &r);
        pos_changed_w = r.right - r.left;
    } else if (msg == WM_SIZE) {
        if (pos_n < 8)
            pos_seq[pos_n++] = 3;
        pos_size_wparam = wparam;
        GetClientRect(hwnd, &r);
        pos_size_client_w = r.right;
    }
    return DefWindowProcW(hwnd, msg, wparam, lparam);
}

static void positions(void) {
    WNDCLASSW wc = {0};
    wc.lpfnWndProc = pos_proc;
    wc.hInstance = GetModuleHandleW(NULL);
    wc.lpszClassName = L"User32TestPos";
    CHECK(RegisterClassW(&wc));
    HWND w = CreateWindowExW(0, L"User32TestPos", L"pos", WS_OVERLAPPEDWINDOW, 30, 40, 300, 200, NULL, NULL,
                             GetModuleHandleW(NULL), NULL);
    CHECK(w != NULL);
    RECT r, c;
    pos_n = 0;
    pos_size_wparam = 99;
    CHECK(SetWindowPos(w, NULL, 0, 0, 320, 220, SWP_NOMOVE | SWP_NOZORDER));
    CHECK(pos_n == 3 && pos_seq[0] == 1 && pos_seq[1] == 2 && pos_seq[2] == 3);
    CHECK(pos_changed_w == 320 && pos_size_wparam == SIZE_RESTORED);
    CHECK(GetClientRect(w, &c) && pos_size_client_w == c.right);
    /* The window procedure narrows it in WM_WINDOWPOSCHANGING. */
    pos_force_cx = 260;
    CHECK(SetWindowPos(w, NULL, 0, 0, 400, 220, SWP_NOMOVE | SWP_NOZORDER));
    CHECK(GetWindowRect(w, &r) && r.right - r.left == 260 && r.left == 30 && pos_changed_w == 260);
    CHECK(GetClientRect(w, &c) && pos_size_client_w == c.right && c.right < 260);
    /* SWP_NOSENDCHANGING: no WM_WINDOWPOSCHANGING, so no say in it. */
    pos_n = 0;
    CHECK(SetWindowPos(w, NULL, 0, 0, 280, 220, SWP_NOMOVE | SWP_NOZORDER | SWP_NOSENDCHANGING));
    CHECK(pos_n == 2 && pos_seq[0] == 2 && pos_seq[1] == 3);
    CHECK(GetWindowRect(w, &r) && r.right - r.left == 280 && pos_changed_w == 280);
    pos_force_cx = 0;
    CHECK(SetWindowPos(w, NULL, 0, 0, 260, 220, SWP_NOMOVE | SWP_NOZORDER));
    SendMessageW(w, WM_SYSCOMMAND, SC_MAXIMIZE, 0);
    CHECK(IsZoomed(w) && pos_size_wparam == SIZE_MAXIMIZED);
    SendMessageW(w, WM_SYSCOMMAND, SC_RESTORE, 0);
    CHECK(!IsZoomed(w) && pos_size_wparam == SIZE_RESTORED);
    CHECK(GetWindowRect(w, &r) && r.right - r.left == 260);
    /* SC_MINIMIZE: IsIconic, WS_MINIMIZE, WM_SIZE SIZE_MINIMIZED; still
       visible. SC_RESTORE: back (SIZE_RESTORED). */
    SendMessageW(w, WM_SYSCOMMAND, SC_MINIMIZE, 0);
    CHECK(IsIconic(w) && !IsZoomed(w) && pos_size_wparam == SIZE_MINIMIZED);
    CHECK((GetWindowLongW(w, GWL_STYLE) & WS_MINIMIZE) && IsWindowVisible(w));
    SendMessageW(w, WM_SYSCOMMAND, SC_RESTORE, 0);
    CHECK(!IsIconic(w) && pos_size_wparam == SIZE_RESTORED && !(GetWindowLongW(w, GWL_STYLE) & WS_MINIMIZE));
    /* ShowWindow's state commands: SW_SHOWMAXIMIZED, SW_MINIMIZE (a
       maximized window comes back maximized), SW_RESTORE twice. */
    ShowWindow(w, SW_SHOWMAXIMIZED);
    CHECK(IsZoomed(w) && !IsIconic(w));
    ShowWindow(w, SW_MINIMIZE);
    CHECK(IsIconic(w));
    ShowWindow(w, SW_RESTORE);
    CHECK(!IsIconic(w) && IsZoomed(w) && pos_size_wparam == SIZE_MAXIMIZED);
    ShowWindow(w, SW_RESTORE);
    CHECK(!IsZoomed(w) && GetWindowRect(w, &r) && r.right - r.left == 260);
    DestroyWindow(w);
}

static int ncc_seen, ncc_mode;
static WPARAM ncc_wparam;

static LRESULT CALLBACK ncc_proc(HWND hwnd, UINT msg, WPARAM wparam, LPARAM lparam) {
    if (msg == WM_NCCALCSIZE) {
        /* Both forms start with the window's new rectangle. */
        RECT *r = (RECT *)lparam;
        ncc_seen++;
        ncc_wparam = wparam;
        if (ncc_mode == 1) {
            r->left += 8;
            r->right -= 8;
            r->bottom -= 8;
        }
        return 0;
    }
    return DefWindowProcW(hwnd, msg, wparam, lparam);
}

/* Activation messages seen: message, LOWORD(wParam), lParam. */
static UINT act_msg[16];
static WPARAM act_w[16];
static LPARAM act_l[16];
static int act_n;

static LRESULT CALLBACK act_proc(HWND hwnd, UINT msg, WPARAM wparam, LPARAM lparam) {
    if ((msg == WM_NCACTIVATE || msg == WM_ACTIVATE || msg == WM_SETFOCUS || msg == WM_KILLFOCUS) && act_n < 16) {
        act_msg[act_n] = msg;
        act_w[act_n] = LOWORD(wparam);
        act_l[act_n] = lparam;
        act_n++;
    }
    return DefWindowProcW(hwnd, msg, wparam, lparam);
}

#define ANY ((WPARAM)-1)

/* Message `m` with wParam's low word `w` (ANY: any), seen at or after
   index `from`: its index, or -1. */
static int act_find(int from, UINT m, WPARAM w) {
    for (int i = from; i < act_n; i++)
        if (act_msg[i] == m && (w == ANY || act_w[i] == w))
            return i;
    return -1;
}

static void activation(void) {
    WNDCLASSW wc = {0};
    wc.lpfnWndProc = act_proc;
    wc.hInstance = GetModuleHandleW(NULL);
    wc.lpszClassName = L"User32TestAct";
    CHECK(RegisterClassW(&wc));
    HWND a = CreateWindowExW(0, L"User32TestAct", L"a", WS_OVERLAPPEDWINDOW, 10, 10, 200, 100, NULL, NULL,
                             GetModuleHandleW(NULL), NULL);
    HWND b = CreateWindowExW(0, L"User32TestAct", L"b", WS_OVERLAPPEDWINDOW, 40, 40, 200, 100, NULL, NULL,
                             GetModuleHandleW(NULL), NULL);
    HWND c = CreateWindowExW(0, L"User32TestAct", L"c", WS_OVERLAPPEDWINDOW, 70, 70, 200, 100, NULL, NULL,
                             GetModuleHandleW(NULL), NULL);
    CHECK(a && b && c);
    act_n = 0;
    ShowWindow(a, SW_SHOW);
    int nc = act_find(0, WM_NCACTIVATE, TRUE), ac = act_find(0, WM_ACTIVATE, WA_ACTIVE);
    CHECK(nc >= 0 && ac > nc && act_find(ac, WM_SETFOCUS, ANY) > ac);
    CHECK(GetActiveWindow() == a && GetFocus() == a);
    act_n = 0;
    ShowWindow(b, SW_SHOW);
    int off = act_find(0, WM_ACTIVATE, WA_INACTIVE);
    CHECK(off >= 0 && act_l[off] == (LPARAM)b && act_find(0, WM_NCACTIVATE, FALSE) >= 0);
    CHECK(act_find(0, WM_KILLFOCUS, ANY) >= 0);
    int on = act_find(off, WM_ACTIVATE, WA_ACTIVE);
    CHECK(on > off && act_l[on] == (LPARAM)a);
    CHECK(GetActiveWindow() == b && GetFocus() == b);
    ShowWindow(c, SW_SHOWNOACTIVATE);
    CHECK(GetActiveWindow() == b && IsWindowVisible(c));
    CHECK(SetActiveWindow(a) == b && GetActiveWindow() == a && GetFocus() == a);
    DestroyWindow(c);
    DestroyWindow(b);
    DestroyWindow(a);
}

static void custom_frame(void) {
    WNDCLASSW wc = {0};
    wc.lpfnWndProc = ncc_proc;
    wc.hInstance = GetModuleHandleW(NULL);
    wc.lpszClassName = L"User32TestFrame";
    CHECK(RegisterClassW(&wc));
    ncc_seen = 0;
    ncc_mode = 0;
    HWND w = CreateWindowExW(0, L"User32TestFrame", L"custom", WS_OVERLAPPEDWINDOW, 50, 60, 300, 200, NULL, NULL,
                             GetModuleHandleW(NULL), NULL);
    CHECK(w != NULL && ncc_seen >= 1);
    RECT c, r;
    POINT p = {0, 0};
    CHECK(GetClientRect(w, &c) && c.right == 300 && c.bottom == 200);
    CHECK(ClientToScreen(w, &p) && p.x == 50 && p.y == 60);
    ncc_mode = 1;
    ncc_seen = 0;
    CHECK(SetWindowPos(w, NULL, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_FRAMECHANGED));
    CHECK(ncc_seen >= 1 && ncc_wparam == TRUE);
    CHECK(GetClientRect(w, &c) && c.right == 284 && c.bottom == 192);
    p.x = p.y = 0;
    CHECK(ClientToScreen(w, &p) && p.x == 58 && p.y == 60);
    CHECK(GetWindowRect(w, &r) && r.left == 50 && r.top == 60 && r.right == 350 && r.bottom == 260);
    DestroyWindow(w);
    /* Alt+F4 closes a window through DefWindowProc. */
    HWND f4 = CreateWindowExW(0, L"User32TestPos", L"alt f4", WS_OVERLAPPEDWINDOW, 50, 60, 300, 200, NULL, NULL,
                              GetModuleHandleW(NULL), NULL);
    CHECK(f4 != NULL);
    DefWindowProcW(f4, WM_SYSKEYDOWN, VK_F4, 0);
    MSG m;
    while (PeekMessageW(&m, NULL, 0, 0, PM_REMOVE))
        DispatchMessageW(&m);
    CHECK(IsWindow(f4));
    DefWindowProcW(f4, WM_SYSKEYDOWN, VK_F4, 1 << 29);
    CHECK(IsWindow(f4));
    while (PeekMessageW(&m, NULL, 0, 0, PM_REMOVE))
        DispatchMessageW(&m);
    CHECK(!IsWindow(f4));

    /* DefWindowProc's frame: a caption above the client area. */
    HWND plain = CreateWindowExW(0, L"User32TestPos", L"plain", WS_OVERLAPPEDWINDOW, 50, 60, 300, 200, NULL,
                                 NULL, GetModuleHandleW(NULL), NULL);
    p.x = p.y = 0;
    CHECK(plain != NULL && GetClientRect(plain, &c) && c.bottom < 200);
    CHECK(ClientToScreen(plain, &p) && p.y > 60);
    DestroyWindow(plain);
}

// Deferred positioning: DeferWindowPos records, EndDeferWindowPos moves
// every window at once.
static void deferred(void) {
    HWND top = CreateWindowExW(0, L"User32TestHooks", L"defer", WS_OVERLAPPEDWINDOW, 0, 0, 300, 200, NULL, NULL,
                               GetModuleHandleW(NULL), NULL);
    HWND a = CreateWindowExW(0, L"User32TestHooks", L"a", WS_CHILD | WS_VISIBLE, 0, 0, 10, 10, top, (HMENU)1,
                             GetModuleHandleW(NULL), NULL);
    HWND b = CreateWindowExW(0, L"User32TestHooks", L"b", WS_CHILD | WS_VISIBLE, 0, 0, 10, 10, top, (HMENU)2,
                             GetModuleHandleW(NULL), NULL);
    HDWP d = BeginDeferWindowPos(2);
    CHECK(d != NULL);
    d = DeferWindowPos(d, a, NULL, 5, 6, 20, 30, SWP_NOZORDER);
    CHECK(d != NULL);
    d = DeferWindowPos(d, b, NULL, 40, 50, 0, 0, SWP_NOZORDER | SWP_NOSIZE);
    CHECK(d != NULL);
    RECT r;
    GetClientRect(a, &r);
    CHECK(r.right == 10); // nothing moved yet
    CHECK(EndDeferWindowPos(d));
    POINT p = {0, 0};
    CHECK(GetClientRect(a, &r) && r.right == 20 && r.bottom == 30);
    CHECK(MapWindowPoints(a, top, &p, 1) || p.x == 5);
    CHECK(p.x == 5 && p.y == 6);
    p.x = p.y = 0;
    MapWindowPoints(b, top, &p, 1);
    CHECK(GetClientRect(b, &r) && r.right == 10 && p.x == 40 && p.y == 50);
    CHECK(DestroyWindow(top));
}

// shell32's SHGetFileInfoW with SHGFI_USEFILEATTRIBUTES (no file needed):
// SHGFI_SYSICONINDEX returns the system image list (small: the small
// icon size) and the type's index in it; folders and files differ;
// SHGFI_TYPENAME gives "File folder" for a folder and "<EXT> File" for
// an extension nothing describes; SHGFI_DISPLAYNAME the name part;
// SHGFI_ICON an icon.
static void file_info(void) {
    SHFILEINFOW fi;
    UINT flags = SHGFI_SYSICONINDEX | SHGFI_SMALLICON | SHGFI_USEFILEATTRIBUTES;
    HIMAGELIST small = (HIMAGELIST)SHGetFileInfoW(L"a.zzq", FILE_ATTRIBUTE_NORMAL, &fi, sizeof fi, flags);
    CHECK(small != NULL);
    int file = fi.iIcon;
    int cx = 0, cy = 0;
    CHECK(ImageList_GetIconSize(small, &cx, &cy) && cx == GetSystemMetrics(SM_CXSMICON) && cy == cx);
    CHECK(file >= 0 && file < ImageList_GetImageCount(small));
    CHECK((HIMAGELIST)SHGetFileInfoW(L"dir", FILE_ATTRIBUTE_DIRECTORY, &fi, sizeof fi, flags) == small);
    CHECK(fi.iIcon >= 0 && fi.iIcon != file);
    HIMAGELIST large = (HIMAGELIST)SHGetFileInfoW(L"a.zzq", FILE_ATTRIBUTE_NORMAL, &fi, sizeof fi,
                                                  SHGFI_SYSICONINDEX | SHGFI_USEFILEATTRIBUTES);
    CHECK(large != NULL && large != small && ImageList_GetIconSize(large, &cx, &cy) && cx == GetSystemMetrics(SM_CXICON));
    CHECK(SHGetFileInfoW(L"dir", FILE_ATTRIBUTE_DIRECTORY, &fi, sizeof fi, SHGFI_TYPENAME | SHGFI_USEFILEATTRIBUTES));
    CHECK(wcscmp(fi.szTypeName, L"File folder") == 0);
    CHECK(SHGetFileInfoW(L"C:\\x\\a.zzq", FILE_ATTRIBUTE_NORMAL, &fi, sizeof fi,
                         SHGFI_TYPENAME | SHGFI_DISPLAYNAME | SHGFI_USEFILEATTRIBUTES));
    CHECK(wcscmp(fi.szTypeName, L"ZZQ File") == 0 && wcscmp(fi.szDisplayName, L"a.zzq") == 0);
    CHECK(SHGetFileInfoW(L"a.zzq", FILE_ATTRIBUTE_NORMAL, &fi, sizeof fi,
                         SHGFI_ICON | SHGFI_SMALLICON | SHGFI_USEFILEATTRIBUTES));
    CHECK(fi.hIcon != NULL && DestroyIcon(fi.hIcon));
}

// Item ID lists: SHGetSpecialFolderLocation gives a folder's list (the
// desktop's is empty, the root), SHGetPathFromIDListW its path (none for
// a virtual folder such as CSIDL_DRIVES); SHGetMalloc the task allocator.
static void item_ids(void) {
    LPITEMIDLIST pidl = NULL;
    WCHAR want[MAX_PATH], got[MAX_PATH];
    CHECK(SHGetSpecialFolderLocation(NULL, CSIDL_APPDATA, &pidl) == S_OK && pidl != NULL);
    CHECK(SHGetFolderPathW(NULL, CSIDL_APPDATA, NULL, 0, want) == S_OK);
    char want_a[MAX_PATH], got_a[MAX_PATH];
    CHECK(SHGetFolderPathA(NULL, CSIDL_APPDATA, NULL, 0, want_a) == S_OK);
    CHECK(SHGetSpecialFolderPathA(NULL, got_a, CSIDL_APPDATA, FALSE) && strcmp(got_a, want_a) == 0);
    WCHAR got_w[MAX_PATH];
    CHECK(SHGetSpecialFolderPathW(NULL, got_w, CSIDL_APPDATA, FALSE) && wcscmp(got_w, want) == 0);
    CHECK(SHGetPathFromIDListW(pidl, got) && wcscmp(got, want) == 0);
    CoTaskMemFree(pidl);
    CHECK(SHGetSpecialFolderLocation(NULL, CSIDL_DESKTOP, &pidl) == S_OK && pidl->mkid.cb == 0);
    CHECK(SHGetFolderPathW(NULL, CSIDL_DESKTOPDIRECTORY, NULL, 0, want) == S_OK);
    CHECK(SHGetPathFromIDListW(pidl, got) && wcscmp(got, want) == 0);
    CoTaskMemFree(pidl);
    CHECK(SHGetSpecialFolderLocation(NULL, CSIDL_DRIVES, &pidl) == S_OK && pidl->mkid.cb != 0);
    CHECK(!SHGetPathFromIDListW(pidl, got));
    CoTaskMemFree(pidl);
    IMalloc *m = NULL;
    CHECK(SHGetMalloc(&m) == S_OK && m != NULL);
    void *p = m->lpVtbl->Alloc(m, 16);
    CHECK(p != NULL);
    m->lpVtbl->Free(m, p);
    m->lpVtbl->Release(m);
}

// mpr's WNetGetConnectionW: a local drive is not a network connection
// (ERROR_NOT_CONNECTED); a name that is no device, ERROR_BAD_DEVICE.
static void wnet(void) {
    WCHAR remote[MAX_PATH];
    DWORD n = MAX_PATH;
    CHECK(WNetGetConnectionW(L"C:", remote, &n) == ERROR_NOT_CONNECTED);
    n = MAX_PATH;
    CHECK(WNetGetConnectionW(L"nothing", remote, &n) == ERROR_BAD_DEVICE);
    CHECK(WNetCancelConnection2W(L"Z:", 0, FALSE) == ERROR_NOT_CONNECTED);
}

/* Posted messages come out in the order they were posted. */
static void posted_order(void) {
    MSG m;
    for (int i = 1; i <= 3; i++)
        CHECK(PostMessageW(NULL, WM_APP + 7, i, 0));
    CHECK(PeekMessageW(&m, NULL, WM_APP + 7, WM_APP + 7, PM_REMOVE) && m.wParam == 1);
    CHECK(PostMessageW(NULL, WM_APP + 7, 4, 0));
    for (int i = 2; i <= 4; i++)
        CHECK(PeekMessageW(&m, NULL, WM_APP + 7, WM_APP + 7, PM_REMOVE) && m.wParam == (WPARAM)i);
    CHECK(!PeekMessageW(&m, NULL, WM_APP + 7, WM_APP + 7, PM_REMOVE));
}

int main(void) {
    bitmaps();
    wnet();
    item_ids();
    file_info();
    clipboard_formats();
    hooks();
    posted_order();
    redraw();
    unknown_class();
    classes();
    effects();
    dialog_units();
    deferred();
    positions();
    custom_frame();
    activation();
    timers();
    menu_bar();
    menu_bar_info();
    scintilla_imports();
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
    /* The US layout, loaded by its KLID, is the thread's; ToUnicode
     * translates with the given key state. */
    HKL us = LoadKeyboardLayoutW(L"00000409", 0);
    CHECK(us != NULL && us == GetKeyboardLayout(0));
    BYTE kbstate[256] = { 0 };
    WCHAR typed[4] = { 0 };
    CHECK(ToUnicode('A', 0x1e, kbstate, typed, 4, 0) == 1 && typed[0] == L'a');
    CHECK(ToUnicode(VK_SPACE, 0x39, kbstate, typed, 4, 0) == 1 && typed[0] == L' ');
    CHECK(ToUnicode(VK_RETURN, 0x1c, kbstate, typed, 4, 0) == 1 && typed[0] == L'\r');
    CHECK(ToUnicode(VK_LEFT, 0x4b, kbstate, typed, 4, 0) == 0);
    kbstate[VK_SHIFT] = 0x80;
    CHECK(ToUnicode('A', 0x1e, kbstate, typed, 4, 0) == 1 && typed[0] == L'A');
    CHECK(ToUnicode('1', 0x02, kbstate, typed, 4, 0) == 1 && typed[0] == L'!');
    CHECK(ToUnicodeEx(VK_OEM_1, 0x27, kbstate, typed, 4, 0, us) == 1 && typed[0] == L':');
    kbstate[VK_SHIFT] = 0;
    kbstate[VK_CONTROL] = 0x80;
    CHECK(ToUnicode('C', 0x2e, kbstate, typed, 4, 0) == 1 && typed[0] == 3);
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
