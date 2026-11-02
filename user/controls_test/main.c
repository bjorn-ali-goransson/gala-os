/* user32's built-in controls and DrawTextW as real Windows behaves
 * (documented behaviour only), for what wxWidgets (FileZilla) creates,
 * in the order its missing-features ledger names them:
 *
 * - DrawTextW: DT_CALCRECT measures without drawing (one line: the
 *   font's height, the text's width); DT_SINGLELINE | DT_CENTER |
 *   DT_VCENTER draws inside the rectangle; the return value is the
 *   text's height (with DT_VCENTER: the offset to the text's bottom).
 * - STATIC: keeps its text (GetWindowTextW, WM_GETTEXTLENGTH,
 *   SetWindowTextW) and paints it.
 * - Subclassing: SetWindowLongPtrW(GWLP_WNDPROC) returns the control's
 *   procedure, which CallWindowProcW calls (a NULL procedure: 0).
 * - BUTTON: BM_CLICK on a push button sends the parent WM_COMMAND with
 *   BN_CLICKED and the id; an auto check box toggles BM_GETCHECK on a
 *   click; BM_SETCHECK sets it.
 * - EDIT: WM_SETTEXT/WM_GETTEXT; EM_SETSEL/EM_GETSEL; EM_REPLACESEL
 *   replaces the selection; typed characters (WM_CHAR) insert at the
 *   caret and notify the parent with EN_CHANGE; ES_READONLY ignores
 *   them; EM_LIMITTEXT caps the length; ES_PASSWORD shows '*'
 *   (EM_GETPASSWORDCHAR).
 * - COMBOBOX: items (add, insert, find, delete, reset, data), the
 *   selection, a drop-down list's text; GetComboBoxInfo names its edit
 *   and list.
 * - IsDialogMessageW: Tab to the next WS_TABSTOP control
 *   (GetNextDlgTabItem), the dialog's other messages dispatched, other
 *   windows' messages not taken.
 * - Dialogs from templates: CreateDialogIndirectParamW (controls, their
 *   places in dialog units, WM_INITDIALOG), DWLP_USER, the dialog
 *   procedure's messages, WM_CLOSE as IDCANCEL, EndDialog.
 *
 * Prints "controls_test: ok" or exits with the number of the failing
 * check. */
#include <windows.h>
#include <stdio.h>
#include <string.h>

static int check;
#define CHECK(cond) do { check++; if (!(cond)) { printf("controls_test: check %d (line %d) failed\n", check, __LINE__); fflush(stdout); ExitProcess(check); } } while (0)

static WPARAM last_command;
static int commands;
static LRESULT CALLBACK parent_proc(HWND hwnd, UINT msg, WPARAM wparam, LPARAM lparam) {
    if (msg == WM_COMMAND) {
        last_command = wparam;
        commands++;
    }
    return DefWindowProcW(hwnd, msg, wparam, lparam);
}

static void draw_text(HWND parent) {
    HDC dc = GetDC(parent);
    TEXTMETRICW tm;
    CHECK(GetTextMetricsW(dc, &tm));
    RECT r = {0, 0, 0, 0};
    int h = DrawTextW(dc, L"Hello", -1, &r, DT_CALCRECT | DT_SINGLELINE);
    SIZE size;
    CHECK(GetTextExtentPoint32W(dc, L"Hello", 5, &size));
    CHECK(h == tm.tmHeight && r.right == size.cx && r.bottom == tm.tmHeight);
    RECT box = {10, 10, 110, 50};
    HBRUSH white = (HBRUSH)GetStockObject(WHITE_BRUSH);
    FillRect(dc, &box, white);
    SetTextColor(dc, RGB(0, 0, 0));
    SetBkMode(dc, TRANSPARENT);
    // With DT_VCENTER the result is the offset from the top to the
    // text's bottom.
    CHECK(DrawTextW(dc, L"Hi", -1, &box, DT_SINGLELINE | DT_CENTER | DT_VCENTER) ==
          (40 - tm.tmHeight) / 2 + tm.tmHeight);
    int inside = 0, outside = 0;
    for (int y = 10; y < 50; y++)
        for (int x = 10; x < 110; x++)
            if (GetPixel(dc, x, y) != RGB(255, 255, 255)) {
                if (x >= 40 && x < 80 && y >= 18 && y < 42) inside++; else outside++;
            }
    CHECK(inside > 10 && outside == 0);
    ReleaseDC(parent, dc);
}

static void statics(HWND parent) {
    HWND s = CreateWindowExW(0, L"STATIC", L"Hello", WS_CHILD | WS_VISIBLE | SS_LEFT, 10, 60, 100, 20, parent,
                             (HMENU)10, GetModuleHandleW(NULL), NULL);
    CHECK(s != NULL);
    WCHAR text[32];
    CHECK(GetWindowTextW(s, text, 32) == 5 && wcscmp(text, L"Hello") == 0);
    CHECK(SendMessageW(s, WM_GETTEXTLENGTH, 0, 0) == 5);
    CHECK(SetWindowTextW(s, L"Bye") && GetWindowTextW(s, text, 32) == 3);
    UpdateWindow(s);
    HDC dc = GetDC(s);
    int dark = 0;
    for (int y = 0; y < 20; y++)
        for (int x = 0; x < 40; x++) {
            COLORREF c = GetPixel(dc, x, y);
            if (GetRValue(c) < 128 && GetGValue(c) < 128 && GetBValue(c) < 128) dark++;
        }
    CHECK(dark > 5);
    ReleaseDC(s, dc);
}

/* Subclassing: SetWindowLongPtrW(GWLP_WNDPROC) returns the control's
 * procedure; the new one passes messages on with CallWindowProcW. */
static WNDPROC static_old;
static int subclassed;
static LRESULT CALLBACK static_sub(HWND hwnd, UINT msg, WPARAM wparam, LPARAM lparam) {
    subclassed++;
    return CallWindowProcW(static_old, hwnd, msg, wparam, lparam);
}

static void subclass(HWND parent) {
    HWND s = CreateWindowExW(0, L"STATIC", L"sub", WS_CHILD | WS_VISIBLE, 200, 60, 60, 20, parent, (HMENU)11,
                             GetModuleHandleW(NULL), NULL);
    static_old = (WNDPROC)SetWindowLongPtrW(s, GWLP_WNDPROC, (LONG_PTR)static_sub);
    CHECK(static_old != NULL && (WNDPROC)GetWindowLongPtrW(s, GWLP_WNDPROC) == static_sub);
    CHECK(SetWindowTextW(s, L"changed") && subclassed > 0);
    WCHAR text[16];
    CHECK(GetWindowTextW(s, text, 16) == 7 && wcscmp(text, L"changed") == 0);
    CHECK(CallWindowProcW(NULL, s, WM_GETTEXTLENGTH, 0, 0) == 0);
}

static void buttons(HWND parent) {
    HWND b = CreateWindowExW(0, L"BUTTON", L"OK", WS_CHILD | WS_VISIBLE | BS_PUSHBUTTON, 10, 90, 80, 24, parent,
                             (HMENU)20, GetModuleHandleW(NULL), NULL);
    CHECK(b != NULL);
    commands = 0;
    SendMessageW(b, BM_CLICK, 0, 0);
    CHECK(commands == 1 && LOWORD(last_command) == 20 && HIWORD(last_command) == BN_CLICKED);
    HWND c = CreateWindowExW(0, L"BUTTON", L"Check", WS_CHILD | WS_VISIBLE | BS_AUTOCHECKBOX, 10, 120, 80, 20,
                             parent, (HMENU)21, GetModuleHandleW(NULL), NULL);
    CHECK(c != NULL && SendMessageW(c, BM_GETCHECK, 0, 0) == BST_UNCHECKED);
    SendMessageW(c, BM_CLICK, 0, 0);
    CHECK(SendMessageW(c, BM_GETCHECK, 0, 0) == BST_CHECKED && LOWORD(last_command) == 21);
    SendMessageW(c, BM_SETCHECK, BST_UNCHECKED, 0);
    CHECK(SendMessageW(c, BM_GETCHECK, 0, 0) == BST_UNCHECKED);
}

static void edits(HWND parent) {
    HWND e = CreateWindowExW(WS_EX_CLIENTEDGE, L"EDIT", L"abc", WS_CHILD | WS_VISIBLE | ES_AUTOHSCROLL, 10, 150,
                             120, 24, parent, (HMENU)30, GetModuleHandleW(NULL), NULL);
    CHECK(e != NULL);
    WCHAR text[64];
    CHECK(GetWindowTextW(e, text, 64) == 3 && wcscmp(text, L"abc") == 0);
    SendMessageW(e, EM_SETSEL, 1, 2);
    DWORD start = 0, end = 0;
    SendMessageW(e, EM_GETSEL, (WPARAM)&start, (LPARAM)&end);
    CHECK(start == 1 && end == 2);
    SendMessageW(e, EM_REPLACESEL, FALSE, (LPARAM)L"XY");
    CHECK(GetWindowTextW(e, text, 64) == 4 && wcscmp(text, L"aXYc") == 0);
    SendMessageW(e, EM_GETSEL, (WPARAM)&start, (LPARAM)&end);
    CHECK(start == 3 && end == 3);
    commands = 0;
    SendMessageW(e, WM_CHAR, 'z', 1);
    CHECK(GetWindowTextW(e, text, 64) == 5 && wcscmp(text, L"aXYzc") == 0);
    CHECK(commands >= 1 && LOWORD(last_command) == 30 && HIWORD(last_command) == EN_CHANGE);
    SendMessageW(e, EM_SETREADONLY, TRUE, 0);
    SendMessageW(e, WM_CHAR, 'q', 1);
    CHECK(GetWindowTextLengthW(e) == 5);
    SendMessageW(e, EM_SETREADONLY, FALSE, 0);
    SetWindowTextW(e, L"");
    SendMessageW(e, EM_LIMITTEXT, 2, 0);
    SendMessageW(e, WM_CHAR, '1', 1);
    SendMessageW(e, WM_CHAR, '2', 1);
    SendMessageW(e, WM_CHAR, '3', 1);
    CHECK(GetWindowTextW(e, text, 64) == 2 && wcscmp(text, L"12") == 0);
    HWND p = CreateWindowExW(0, L"EDIT", L"", WS_CHILD | WS_VISIBLE | ES_PASSWORD, 10, 180, 120, 24, parent,
                             (HMENU)31, GetModuleHandleW(NULL), NULL);
    CHECK(p != NULL && SendMessageW(p, EM_GETPASSWORDCHAR, 0, 0) != 0);
}

/* COMBOBOX: items added, inserted, found, deleted, counted, with data;
 * the selection (CB_SETCURSEL, -1 at first and for an index past the
 * end); a drop-down list's text is the selected item's; GetComboBoxInfo
 * names the combo box, its edit (a child, for CBS_DROPDOWN) and its
 * list. */
static void combos(HWND parent) {
    HWND c = CreateWindowExW(0, L"COMBOBOX", NULL, WS_CHILD | WS_VISIBLE | CBS_DROPDOWNLIST, 10, 210, 150, 200,
                             parent, (HMENU)40, GetModuleHandleW(NULL), NULL);
    CHECK(c != NULL);
    CHECK(SendMessageW(c, CB_GETCURSEL, 0, 0) == CB_ERR && SendMessageW(c, CB_GETCOUNT, 0, 0) == 0);
    CHECK(SendMessageW(c, CB_ADDSTRING, 0, (LPARAM)L"alpha") == 0);
    CHECK(SendMessageW(c, CB_ADDSTRING, 0, (LPARAM)L"gamma") == 1);
    CHECK(SendMessageW(c, CB_INSERTSTRING, 1, (LPARAM)L"beta") == 1);
    CHECK(SendMessageW(c, CB_GETCOUNT, 0, 0) == 3);
    WCHAR text[32];
    CHECK(SendMessageW(c, CB_GETLBTEXTLEN, 2, 0) == 5);
    CHECK(SendMessageW(c, CB_GETLBTEXT, 2, (LPARAM)text) == 5 && wcscmp(text, L"gamma") == 0);
    CHECK(SendMessageW(c, CB_FINDSTRINGEXACT, -1, (LPARAM)L"BETA") == 1);
    CHECK(SendMessageW(c, CB_FINDSTRING, -1, (LPARAM)L"gam") == 2);
    CHECK(SendMessageW(c, CB_FINDSTRINGEXACT, -1, (LPARAM)L"delta") == CB_ERR);
    CHECK(SendMessageW(c, CB_SETITEMDATA, 1, 1234) != CB_ERR && SendMessageW(c, CB_GETITEMDATA, 1, 0) == 1234);
    CHECK(SendMessageW(c, CB_SETCURSEL, 1, 0) == 1 && SendMessageW(c, CB_GETCURSEL, 0, 0) == 1);
    CHECK(GetWindowTextW(c, text, 32) == 4 && wcscmp(text, L"beta") == 0);
    CHECK(SendMessageW(c, CB_SETCURSEL, 7, 0) == CB_ERR && SendMessageW(c, CB_GETCURSEL, 0, 0) == CB_ERR);
    CHECK(SendMessageW(c, CB_DELETESTRING, 0, 0) == 2);
    CHECK(SendMessageW(c, CB_RESETCONTENT, 0, 0) == CB_OKAY && SendMessageW(c, CB_GETCOUNT, 0, 0) == 0);

    HWND d = CreateWindowExW(0, L"COMBOBOX", NULL, WS_CHILD | WS_VISIBLE | CBS_DROPDOWN, 10, 240, 150, 200, parent,
                             (HMENU)41, GetModuleHandleW(NULL), NULL);
    CHECK(d != NULL);
    COMBOBOXINFO info = {sizeof info};
    CHECK(GetComboBoxInfo(d, &info) && info.hwndCombo == d);
    CHECK(info.hwndItem != NULL && GetParent(info.hwndItem) == d && info.hwndList != NULL);
    CHECK(SetWindowTextW(d, L"typed") && GetWindowTextW(info.hwndItem, text, 32) == 5);
    CHECK(GetWindowTextW(d, text, 32) == 5 && wcscmp(text, L"typed") == 0);
}

/* IsDialogMessageW: Tab moves the focus to the next WS_TABSTOP control
 * (GetNextDlgTabItem, Shift+Tab back); other messages for the dialog's
 * windows are dispatched (TRUE); messages for other windows are not its
 * (FALSE). */
static void dialog_keys(HWND parent) {
    HWND a = CreateWindowExW(0, L"EDIT", L"", WS_CHILD | WS_VISIBLE | WS_TABSTOP, 200, 150, 80, 24, parent, (HMENU)70,
                             GetModuleHandleW(NULL), NULL);
    HWND b = CreateWindowExW(0, L"EDIT", L"", WS_CHILD | WS_VISIBLE | WS_TABSTOP, 200, 180, 80, 24, parent, (HMENU)71,
                             GetModuleHandleW(NULL), NULL);
    CHECK(GetNextDlgTabItem(parent, a, FALSE) == b);
    CHECK(GetNextDlgTabItem(parent, b, TRUE) == a);
    SetFocus(a);
    MSG m = {a, WM_KEYDOWN, VK_TAB, 0};
    CHECK(IsDialogMessageW(parent, &m) && GetFocus() == b);
    MSG c = {b, WM_CHAR, 'q', 1};
    CHECK(IsDialogMessageW(parent, &c));
    WCHAR text[8];
    CHECK(GetWindowTextW(b, text, 8) == 1 && text[0] == 'q');
    HWND other = CreateWindowExW(0, L"STATIC", L"", WS_OVERLAPPEDWINDOW, 0, 0, 50, 50, NULL, NULL,
                                 GetModuleHandleW(NULL), NULL);
    MSG o = {other, WM_CHAR, 'q', 1};
    CHECK(!IsDialogMessageW(parent, &o));
    DestroyWindow(other);
    DestroyWindow(a);
    DestroyWindow(b);
}

/* Dialogs from a template in memory (DLGTEMPLATE with DS_SETFONT and two
 * items, a STATIC and a BUTTON by class atom): CreateDialogIndirectParamW
 * creates the dialog and its controls (their ids, classes, texts, places:
 * MapDialogRect of their dialog units), sends WM_INITDIALOG with the
 * parameter to the dialog procedure; DWLP_USER is the procedure's to
 * keep; messages reach the procedure (WM_COMMAND), and WM_CLOSE by
 * default becomes WM_COMMAND IDCANCEL; EndDialog ends it. */
static int init_dialogs, dialog_commands;
static WPARAM dialog_last_command;
static LPARAM init_param;
static INT_PTR CALLBACK dialog_proc(HWND dlg, UINT msg, WPARAM wparam, LPARAM lparam) {
    if (msg == WM_INITDIALOG) {
        init_dialogs++;
        init_param = lparam;
        return TRUE;
    }
    if (msg == WM_COMMAND) {
        dialog_commands++;
        dialog_last_command = wparam;
        return TRUE;
    }
    return FALSE;
}

static WORD *put_text(WORD *p, const WCHAR *s) {
    while ((*p++ = *s++) != 0) {
    }
    return p;
}

static WORD *align4(WORD *p) {
    return (WORD *)(((ULONG_PTR)p + 3) & ~(ULONG_PTR)3);
}

static void dialogs(HWND parent) {
    static DWORD buf[256];
    memset(buf, 0, sizeof buf);
    DLGTEMPLATE *t = (DLGTEMPLATE *)buf;
    t->style = WS_POPUP | WS_CAPTION | WS_SYSMENU | DS_SETFONT | DS_MODALFRAME;
    t->cdit = 2;
    t->x = 10;
    t->y = 10;
    t->cx = 200;
    t->cy = 100;
    WORD *p = (WORD *)(t + 1);
    *p++ = 0; /* no menu */
    *p++ = 0; /* the dialog class */
    p = put_text(p, L"Gala dialog");
    *p++ = 9; /* 9 point */
    p = put_text(p, L"Segoe UI");
    p = align4(p);
    DLGITEMTEMPLATE *it = (DLGITEMTEMPLATE *)p;
    it->style = WS_CHILD | WS_VISIBLE | SS_LEFT;
    it->x = 7;
    it->y = 7;
    it->cx = 100;
    it->cy = 10;
    it->id = 100;
    p = (WORD *)(it + 1);
    *p++ = 0xffff;
    *p++ = 0x0082; /* STATIC */
    p = put_text(p, L"Label");
    *p++ = 0;
    p = align4(p);
    it = (DLGITEMTEMPLATE *)p;
    it->style = WS_CHILD | WS_VISIBLE | WS_TABSTOP | BS_DEFPUSHBUTTON;
    it->x = 140;
    it->y = 80;
    it->cx = 50;
    it->cy = 14;
    it->id = IDOK;
    p = (WORD *)(it + 1);
    *p++ = 0xffff;
    *p++ = 0x0080; /* BUTTON */
    p = put_text(p, L"OK");
    *p++ = 0;

    init_dialogs = 0;
    HWND dlg = CreateDialogIndirectParamW(GetModuleHandleW(NULL), t, parent, dialog_proc, 1234);
    CHECK(dlg != NULL && init_dialogs == 1 && init_param == 1234);
    WCHAR text[32];
    CHECK(GetWindowTextW(dlg, text, 32) == 11 && wcscmp(text, L"Gala dialog") == 0);
    HWND label = GetDlgItem(dlg, 100), ok = GetDlgItem(dlg, IDOK);
    CHECK(label != NULL && ok != NULL && GetDlgCtrlID(ok) == IDOK);
    CHECK(GetClassNameW(label, text, 32) && lstrcmpiW(text, L"Static") == 0);
    CHECK(GetClassNameW(ok, text, 32) && lstrcmpiW(text, L"Button") == 0);
    CHECK(GetWindowTextW(ok, text, 32) == 2 && wcscmp(text, L"OK") == 0);
    RECT du = {140, 80, 190, 94}, wr;
    CHECK(MapDialogRect(dlg, &du));
    GetWindowRect(ok, &wr);
    MapWindowPoints(NULL, dlg, (POINT *)&wr, 2);
    CHECK(wr.left == du.left && wr.top == du.top && wr.right == du.right && wr.bottom == du.bottom);
    CHECK(SetWindowLongPtrW(dlg, DWLP_USER, 77) == 0 && GetWindowLongPtrW(dlg, DWLP_USER) == 77);
    dialog_commands = 0;
    SendMessageW(dlg, WM_COMMAND, MAKEWPARAM(IDOK, BN_CLICKED), (LPARAM)ok);
    CHECK(dialog_commands == 1 && LOWORD(dialog_last_command) == IDOK);
    SendMessageW(dlg, WM_CLOSE, 0, 0);
    CHECK(dialog_commands == 2 && LOWORD(dialog_last_command) == IDCANCEL && IsWindow(dlg));
    CHECK(EndDialog(dlg, 5));
    DestroyWindow(dlg);
    CHECK(!IsWindow(dlg));
}

int main(void) {
    WNDCLASSW wc = {0};
    wc.lpfnWndProc = parent_proc;
    wc.hInstance = GetModuleHandleW(NULL);
    wc.lpszClassName = L"ControlsTestParent";
    wc.hbrBackground = (HBRUSH)(COLOR_WINDOW + 1);
    CHECK(RegisterClassW(&wc) != 0);
    HWND parent = CreateWindowExW(0, L"ControlsTestParent", L"controls", WS_OVERLAPPEDWINDOW | WS_VISIBLE, 50, 50,
                                  400, 300, NULL, NULL, GetModuleHandleW(NULL), NULL);
    CHECK(parent != NULL);
    draw_text(parent);
    statics(parent);
    subclass(parent);
    buttons(parent);
    edits(parent);
    combos(parent);
    dialog_keys(parent);
    dialogs(parent);
    DestroyWindow(parent);
    printf("controls_test: ok\n");
    return 0;
}
