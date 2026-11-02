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
    DestroyWindow(parent);
    printf("controls_test: ok\n");
    return 0;
}
