/* comctl32 as real Windows behaves (documented behaviour only), for what
 * wxWidgets (FileZilla) uses, added as FileZilla's missing-features
 * ledger names it:
 *
 * - Image lists: ImageList_Create gives an empty list of the size asked
 *   (GetImageCount 0, GetIconSize). ImageList_Add splits a bitmap into
 *   images of the list's width and returns the first one's index;
 *   ImageList_AddMasked makes the mask from a colour (those pixels are
 *   transparent). ImageList_Draw: ILD_NORMAL fills the background colour
 *   (SetBkColor, CLR_NONE at first: none) under transparent pixels,
 *   ILD_TRANSPARENT leaves them. ImageList_GetImageInfo gives the
 *   image's bitmaps and a rectangle of the image's size.
 *   ImageList_ReplaceIcon with -1 appends an icon; ImageList_GetIcon
 *   makes one. ImageList_Copy with ILCF_SWAP swaps two images.
 *   ImageList_Replace replaces one. ImageList_Remove removes one (-1:
 *   all). ImageList_Destroy.
 * - Dragging (ImageList_BeginDrag/DragEnter/DragMove/
 *   SetDragCursorImage/DragLeave/EndDrag): TRUE while a drag of an image
 *   that exists is going on; BeginDrag of a missing image fails.
 * - The status bar: at the bottom of its parent, full width (and after
 *   WM_SIZE); parts, their texts and rectangles.
 * - Tooltips: tools added, counted, deleted; TTM_SETMAXTIPWIDTH. Hover
 *   (TTF_SUBCLASS on a button under the pointer): the delay times'
 *   documented defaults (initial = GetDoubleClickTime(), autopop 10x,
 *   reshow 1/5); TTM_ACTIVATE FALSE keeps the tip hidden; a WM_MOUSEMOVE
 *   on the tool and the initial delay show it (TTN_SHOW to the tool's
 *   hwnd, from the tooltip, idFrom the tool's uId), near the pointer and
 *   on the screen; a click hides it (TTN_POP); LPSTR_TEXTCALLBACK asks
 *   TTN_GETDISPINFOW; TTM_POP hides it; the autopop delay hides it.
 * - The toolbar: buttons added and counted, enabled, checked, side by
 *   side; TB_AUTOSIZE at the top, full width; a click sends WM_COMMAND
 *   with the button's id; TB_DELETEBUTTON; TB_ADDBITMAP with a bitmap
 *   strip returns its first image's index.
 * - The list view (report view): its header and columns, items and
 *   subitems, selection (LVM_SETITEMSTATE, LVN_ITEMCHANGED,
 *   LVM_GETNEXTITEM: -1 when none), item rectangles, hit testing, a
 *   click (NM_CLICK), extended styles, image lists, deletion; a virtual
 *   list (LVS_OWNERDATA) asks for texts with LVN_GETDISPINFOW
 *   (LVM_GETITEMTEXTW too), selects (LVM_SETITEMSTATE, LVM_GETNEXTITEM,
 *   LVM_GETITEMSTATE), gives item rectangles; a double click on an item
 *   sends NM_DBLCLK and LVN_ITEMACTIVATE (NMITEMACTIVATE.iItem).
 * - The tree view: items inserted (first, last), counted, walked, read
 *   back; expanding (TVN_ITEMEXPANDINGW, TVIS_EXPANDED), selecting
 *   (TVN_SELCHANGEDW, the caret), children asked of the parent
 *   (I_CHILDRENCALLBACK), rectangles, hit testing, image lists, deleting
 *   (with children; TVI_ROOT: all).
 *
 * - Flat scroll bars (Delphi's VCL initializes them on every scrolling
 *   control): FlatSB_GetScrollProp(WSB_PROP_HSTYLE) is FALSE before
 *   InitializeFlatSB and TRUE after; properties set are read back;
 *   WSB_PROP_WINSTYLE is the window's WS_HSCROLL/WS_VSCROLL bits; the
 *   FlatSB_ scroll functions keep the range, page and position;
 *   UninitializeFlatSB is S_OK, then S_FALSE.
 *
 * Prints "comctl32_test: ok" or exits with the number of the failing
 * check. */
#include <windows.h>
#include <commctrl.h>
#include <stdio.h>

static int check;
#define CHECK(cond) do { check++; if (!(cond)) { printf("comctl32_test: check %d (line %d) failed\n", check, __LINE__); fflush(stdout); ExitProcess(check); } } while (0)

#define RED 0xff0000
#define GREEN 0x00ff00
#define BLUE 0x0000ff
#define MAGENTA 0xff00ff

/* A w x h 32-bit DIB section whose columns [0, split) are `left` and the
 * rest `right` (0x00RRGGBB). */
static HBITMAP dib(int w, int h, int split, DWORD left, DWORD right) {
    BITMAPINFO bi = {{sizeof(BITMAPINFOHEADER), w, -h, 1, 32, BI_RGB}};
    DWORD *bits;
    HBITMAP b = CreateDIBSection(NULL, &bi, DIB_RGB_COLORS, (void **)&bits, NULL, 0);
    for (int y = 0; y < h; y++)
        for (int x = 0; x < w; x++) bits[y * w + x] = x < split ? left : right;
    return b;
}

static COLORREF rgb(DWORD c) { return RGB(c >> 16, (c >> 8) & 0xff, c & 0xff); }

static WPARAM last_command;
static int commands;
static int item_changes, last_changed_item, clicks, dispinfo_requests;
static int double_clicks, activations, activated_item = -2;
static int tv_expanding, tv_selchanged, tv_children_asked;
static HTREEITEM tv_new_selection;
static UINT last_new_state;
static int tt_shows, tt_pops, tt_dispinfo;
static HWND tt_from;
static UINT_PTR tt_id;
static LRESULT CALLBACK parent_proc(HWND hwnd, UINT msg, WPARAM wparam, LPARAM lparam) {
    if (msg == WM_COMMAND) {
        last_command = wparam;
        commands++;
    }
    if (msg == WM_NOTIFY) {
        NMHDR *h = (NMHDR *)lparam;
        if (h->code == LVN_ITEMCHANGED) {
            NMLISTVIEW *lv = (NMLISTVIEW *)lparam;
            item_changes++;
            last_changed_item = lv->iItem;
            last_new_state = lv->uNewState;
        } else if (h->code == NM_CLICK) {
            clicks++;
        } else if (h->code == NM_DBLCLK) {
            double_clicks++;
        } else if (h->code == LVN_ITEMACTIVATE) {
            activations++;
            activated_item = ((NMITEMACTIVATE *)lparam)->iItem;
        } else if (h->code == TVN_ITEMEXPANDINGW) {
            tv_expanding++;
        } else if (h->code == TVN_SELCHANGEDW) {
            tv_selchanged++;
            tv_new_selection = ((NMTREEVIEWW *)lparam)->itemNew.hItem;
        } else if (h->code == TVN_GETDISPINFOW) {
            NMTVDISPINFOW *di = (NMTVDISPINFOW *)lparam;
            if (di->item.mask & TVIF_CHILDREN) {
                tv_children_asked++;
                di->item.cChildren = 1;
            }
        } else if (h->code == TTN_SHOW) {
            tt_shows++;
            tt_from = h->hwndFrom;
            tt_id = h->idFrom;
        } else if (h->code == TTN_POP) {
            tt_pops++;
        } else if (h->code == TTN_GETDISPINFOW) {
            NMTTDISPINFOW *di = (NMTTDISPINFOW *)lparam;
            tt_dispinfo++;
            di->lpszText = L"Callback tip";
        } else if (h->code == LVN_GETDISPINFOW) {
            NMLVDISPINFOW *di = (NMLVDISPINFOW *)lparam;
            dispinfo_requests++;
            if (di->item.mask & LVIF_TEXT) {
                swprintf(di->item.pszText, di->item.cchTextMax, L"row %d", di->item.iItem);
            }
        }
    }
    return DefWindowProcW(hwnd, msg, wparam, lparam);
}

/* The list view in report view: a header (LVM_GETHEADER) with the
 * columns; columns inserted, sized and read back; items with subitem
 * texts; selection by LVM_SETITEMSTATE (LVN_ITEMCHANGED to the parent;
 * LVM_GETNEXTITEM with LVNI_SELECTED finds it, -1 when none), removal;
 * item rectangles one under the other below the header; hit testing; a
 * click selects (NM_CLICK); extended styles and image lists are kept; a
 * virtual list (LVS_OWNERDATA) has LVM_SETITEMCOUNT's count and asks the
 * parent for texts (LVN_GETDISPINFOW). */
static void list_view(HWND parent, HIMAGELIST il) {
    HWND lv = CreateWindowExW(0, WC_LISTVIEWW, NULL, WS_CHILD | WS_VISIBLE | LVS_REPORT | LVS_SHOWSELALWAYS, 0, 0,
                              300, 200, parent, (HMENU)50, GetModuleHandleW(NULL), NULL);
    CHECK(lv != NULL);
    HWND header = (HWND)SendMessageW(lv, LVM_GETHEADER, 0, 0);
    CHECK(header != NULL);
    LVCOLUMNW col = {0};
    col.mask = LVCF_TEXT | LVCF_WIDTH;
    col.cx = 100;
    col.pszText = L"Name";
    CHECK(SendMessageW(lv, LVM_INSERTCOLUMNW, 0, (LPARAM)&col) == 0);
    col.pszText = L"Size";
    col.cx = 60;
    CHECK(SendMessageW(lv, LVM_INSERTCOLUMNW, 1, (LPARAM)&col) == 1);
    CHECK(SendMessageW(header, HDM_GETITEMCOUNT, 0, 0) == 2);
    CHECK(SendMessageW(lv, LVM_GETCOLUMNWIDTH, 1, 0) == 60);
    CHECK(SendMessageW(lv, LVM_SETCOLUMNWIDTH, 0, 120) && SendMessageW(lv, LVM_GETCOLUMNWIDTH, 0, 0) == 120);
    WCHAR text[32];
    LVCOLUMNW back = {0};
    back.mask = LVCF_TEXT;
    back.pszText = text;
    back.cchTextMax = 32;
    CHECK(SendMessageW(lv, LVM_GETCOLUMNW, 1, (LPARAM)&back) && wcscmp(text, L"Size") == 0);

    LVITEMW item = {0};
    item.mask = LVIF_TEXT;
    item.pszText = L"a";
    CHECK(SendMessageW(lv, LVM_INSERTITEMW, 0, (LPARAM)&item) == 0);
    item.iItem = 1;
    item.pszText = L"b";
    CHECK(SendMessageW(lv, LVM_INSERTITEMW, 0, (LPARAM)&item) == 1);
    CHECK(SendMessageW(lv, LVM_GETITEMCOUNT, 0, 0) == 2);
    LVITEMW sub = {0};
    sub.iSubItem = 1;
    sub.pszText = L"x";
    CHECK(SendMessageW(lv, LVM_SETITEMTEXTW, 0, (LPARAM)&sub));
    sub.pszText = text;
    sub.cchTextMax = 32;
    CHECK(SendMessageW(lv, LVM_GETITEMTEXTW, 0, (LPARAM)&sub) == 1 && wcscmp(text, L"x") == 0);
    sub.iSubItem = 0;
    CHECK(SendMessageW(lv, LVM_GETITEMTEXTW, 1, (LPARAM)&sub) == 1 && wcscmp(text, L"b") == 0);

    CHECK(SendMessageW(lv, LVM_GETNEXTITEM, -1, LVNI_SELECTED) == -1);
    LVITEMW st = {0};
    st.stateMask = LVIS_SELECTED;
    st.state = LVIS_SELECTED;
    item_changes = 0;
    CHECK(SendMessageW(lv, LVM_SETITEMSTATE, 1, (LPARAM)&st));
    CHECK(item_changes == 1 && last_changed_item == 1 && (last_new_state & LVIS_SELECTED));
    CHECK(SendMessageW(lv, LVM_GETNEXTITEM, -1, LVNI_SELECTED) == 1);
    CHECK(SendMessageW(lv, LVM_GETITEMSTATE, 1, LVIS_SELECTED) == LVIS_SELECTED);
    CHECK(SendMessageW(lv, LVM_GETSELECTEDCOUNT, 0, 0) == 1);
    st.state = 0;
    CHECK(SendMessageW(lv, LVM_SETITEMSTATE, -1, (LPARAM)&st));
    CHECK(SendMessageW(lv, LVM_GETNEXTITEM, -1, LVNI_SELECTED) == -1);

    RECT hr, a, b;
    GetWindowRect(header, &hr);
    MapWindowPoints(NULL, lv, (POINT *)&hr, 2);
    a.left = b.left = LVIR_BOUNDS;
    CHECK(SendMessageW(lv, LVM_GETITEMRECT, 0, (LPARAM)&a) && SendMessageW(lv, LVM_GETITEMRECT, 1, (LPARAM)&b));
    CHECK(a.top >= hr.bottom && b.top == a.bottom && a.bottom > a.top);
    LVHITTESTINFO ht = {{20, (b.top + b.bottom) / 2}, 0, 0, 0, 0};
    CHECK(SendMessageW(lv, LVM_HITTEST, 0, (LPARAM)&ht) == 1 && (ht.flags & LVHT_ONITEM));
    clicks = 0;
    LPARAM at = MAKELPARAM(20, (a.top + a.bottom) / 2);
    SendMessageW(lv, WM_LBUTTONDOWN, MK_LBUTTON, at);
    SendMessageW(lv, WM_LBUTTONUP, 0, at);
    CHECK(SendMessageW(lv, LVM_GETNEXTITEM, -1, LVNI_SELECTED) == 0 && clicks >= 1);

    CHECK(SendMessageW(lv, LVM_SETEXTENDEDLISTVIEWSTYLE, LVS_EX_FULLROWSELECT, LVS_EX_FULLROWSELECT) == 0);
    CHECK(SendMessageW(lv, LVM_GETEXTENDEDLISTVIEWSTYLE, 0, 0) == LVS_EX_FULLROWSELECT);
    CHECK(SendMessageW(lv, LVM_SETIMAGELIST, LVSIL_SMALL, (LPARAM)il) == 0);
    CHECK((HIMAGELIST)SendMessageW(lv, LVM_GETIMAGELIST, LVSIL_SMALL, 0) == il);
    CHECK(SendMessageW(lv, LVM_GETCOUNTPERPAGE, 0, 0) > 0 && SendMessageW(lv, LVM_GETTOPINDEX, 0, 0) == 0);
    CHECK(SendMessageW(lv, LVM_ENSUREVISIBLE, 1, FALSE));

    CHECK(SendMessageW(lv, LVM_DELETEITEM, 0, 0) && SendMessageW(lv, LVM_GETITEMCOUNT, 0, 0) == 1);
    CHECK(SendMessageW(lv, LVM_DELETEALLITEMS, 0, 0) && SendMessageW(lv, LVM_GETITEMCOUNT, 0, 0) == 0);
    DestroyWindow(lv);

    HWND v = CreateWindowExW(0, WC_LISTVIEWW, NULL, WS_CHILD | WS_VISIBLE | LVS_REPORT | LVS_OWNERDATA, 0, 0, 300,
                             200, parent, (HMENU)51, GetModuleHandleW(NULL), NULL);
    CHECK(v != NULL);
    col.pszText = L"Name";
    col.cx = 150;
    SendMessageW(v, LVM_INSERTCOLUMNW, 0, (LPARAM)&col);
    CHECK(SendMessageW(v, LVM_SETITEMCOUNT, 1000, 0) && SendMessageW(v, LVM_GETITEMCOUNT, 0, 0) == 1000);
    dispinfo_requests = 0;
    InvalidateRect(v, NULL, TRUE);
    UpdateWindow(v);
    CHECK(dispinfo_requests > 0);
    CHECK(SendMessageW(v, LVM_GETNEXTITEM, -1, LVNI_SELECTED) == -1);
    // What UI Automation's list proxy sends a virtual list: an item's
    // text (asked of the parent), its rectangle, its selection.
    WCHAR vtext[32] = {0};
    LVITEMW vi = {0};
    vi.iSubItem = 0;
    vi.pszText = vtext;
    vi.cchTextMax = 32;
    CHECK(SendMessageW(v, LVM_GETITEMTEXTW, 500, (LPARAM)&vi) == 7 && wcscmp(vtext, L"row 500") == 0);
    RECT vr = {LVIR_BOUNDS};
    CHECK(SendMessageW(v, LVM_GETITEMRECT, 1, (LPARAM)&vr) && vr.bottom > vr.top);
    st.stateMask = LVIS_SELECTED | LVIS_FOCUSED;
    st.state = LVIS_SELECTED | LVIS_FOCUSED;
    item_changes = 0;
    CHECK(SendMessageW(v, LVM_SETITEMSTATE, 1, (LPARAM)&st));
    CHECK(item_changes >= 1 && last_changed_item == 1);
    CHECK(SendMessageW(v, LVM_GETNEXTITEM, -1, LVNI_SELECTED) == 1);
    CHECK(SendMessageW(v, LVM_GETNEXTITEM, -1, LVNI_FOCUSED) == 1);
    CHECK(SendMessageW(v, LVM_GETITEMSTATE, 1, LVIS_SELECTED) == LVIS_SELECTED);
    CHECK(SendMessageW(v, LVM_GETSELECTEDCOUNT, 0, 0) == 1);
    // A double click on an item: NM_DBLCLK, then LVN_ITEMACTIVATE for it.
    double_clicks = activations = 0;
    SendMessageW(v, WM_LBUTTONDBLCLK, MK_LBUTTON, MAKELPARAM(20, (vr.top + vr.bottom) / 2));
    CHECK(double_clicks == 1 && activations == 1 && activated_item == 1);
    st.state = 0;
    CHECK(SendMessageW(v, LVM_SETITEMSTATE, -1, (LPARAM)&st));
    CHECK(SendMessageW(v, LVM_GETNEXTITEM, -1, LVNI_SELECTED) == -1);
    DestroyWindow(v);
}

/* The status bar sits at the bottom of its parent, full width; parts
 * (SB_SETPARTS/SB_GETPARTS, -1: to the right edge) hold texts
 * (SB_SETTEXTW, SB_GETTEXTW and SB_GETTEXTLENGTHW: the length in the
 * low word); SB_GETRECT gives a part's rectangle. */
static void status_bar(HWND parent) {
    HWND sb = CreateWindowExW(0, STATUSCLASSNAMEW, NULL, WS_CHILD | WS_VISIBLE, 0, 0, 0, 0, parent, (HMENU)40,
                              GetModuleHandleW(NULL), NULL);
    CHECK(sb != NULL);
    RECT pc, sr;
    GetClientRect(parent, &pc);
    GetWindowRect(sb, &sr);
    MapWindowPoints(NULL, parent, (POINT *)&sr, 2);
    CHECK(sr.bottom == pc.bottom && sr.left == 0 && sr.right == pc.right && sr.bottom - sr.top > 10);
    int widths[3] = {100, 200, -1}, back[3] = {0};
    CHECK(SendMessageW(sb, SB_SETPARTS, 3, (LPARAM)widths));
    CHECK(SendMessageW(sb, SB_GETPARTS, 3, (LPARAM)back) == 3 && back[0] == 100 && back[1] == 200 && back[2] == -1);
    CHECK(SendMessageW(sb, SB_SETTEXTW, 1, (LPARAM)L"hello"));
    CHECK(LOWORD(SendMessageW(sb, SB_GETTEXTLENGTHW, 1, 0)) == 5);
    WCHAR text[16];
    CHECK(LOWORD(SendMessageW(sb, SB_GETTEXTW, 1, (LPARAM)text)) == 5 && wcscmp(text, L"hello") == 0);
    RECT part;
    CHECK(SendMessageW(sb, SB_GETRECT, 1, (LPARAM)&part) && part.left >= 100 && part.right <= 202);
    /* The parent grows: on WM_SIZE the bar follows. */
    SetWindowPos(parent, NULL, 0, 0, 500, 400, SWP_NOMOVE | SWP_NOZORDER);
    SendMessageW(sb, WM_SIZE, 0, 0);
    GetClientRect(parent, &pc);
    GetWindowRect(sb, &sr);
    MapWindowPoints(NULL, parent, (POINT *)&sr, 2);
    CHECK(sr.bottom == pc.bottom && sr.right == pc.right);
    DestroyWindow(sb);
}

/* Tooltips: tools are added (TTM_ADDTOOLW), counted (TTM_GETTOOLCOUNT)
 * and deleted; TTM_SETMAXTIPWIDTH returns the previous width (-1 at
 * first). */
static void tooltips(HWND parent) {
    HWND tt = CreateWindowExW(0, TOOLTIPS_CLASSW, NULL, WS_POPUP | TTS_ALWAYSTIP, CW_USEDEFAULT, CW_USEDEFAULT,
                              CW_USEDEFAULT, CW_USEDEFAULT, parent, NULL, GetModuleHandleW(NULL), NULL);
    CHECK(tt != NULL);
    TOOLINFOW ti = {0};
    ti.cbSize = sizeof ti;
    ti.uFlags = TTF_IDISHWND | TTF_SUBCLASS;
    ti.hwnd = parent;
    ti.uId = (UINT_PTR)parent;
    ti.lpszText = L"tip";
    CHECK(SendMessageW(tt, TTM_ADDTOOLW, 0, (LPARAM)&ti));
    CHECK(SendMessageW(tt, TTM_GETTOOLCOUNT, 0, 0) == 1);
    CHECK(SendMessageW(tt, TTM_SETMAXTIPWIDTH, 0, 300) == -1);
    SendMessageW(tt, TTM_DELTOOLW, 0, (LPARAM)&ti);
    CHECK(SendMessageW(tt, TTM_GETTOOLCOUNT, 0, 0) == 0);
    DestroyWindow(tt);
}

/* Dispatches this thread's messages for `ms` milliseconds, or until the
 * window's visibility is `visible` (then TRUE). */
static BOOL pump_until(HWND hwnd, BOOL visible, DWORD ms) {
    DWORD start = GetTickCount();
    for (;;) {
        MSG m;
        while (PeekMessageW(&m, NULL, 0, 0, PM_REMOVE)) {
            TranslateMessage(&m);
            DispatchMessageW(&m);
        }
        if (hwnd != NULL && !!IsWindowVisible(hwnd) == !!visible) return TRUE;
        if (GetTickCount() - start >= ms) return FALSE;
        Sleep(10);
    }
}

/* A tooltip on hover: a window is made under the pointer with a button
 * centred on it (so the tool is where the pointer really rests, also on
 * Windows); the tool (TTF_IDISHWND | TTF_SUBCLASS) gets WM_MOUSEMOVE as
 * the pointer would give it, and the messages are dispatched. */
static void tooltip_hover(void) {
    POINT cur;
    CHECK(GetCursorPos(&cur));
    HWND win = CreateWindowExW(0, L"Comctl32TestParent", L"tips", WS_OVERLAPPEDWINDOW | WS_VISIBLE, cur.x - 150,
                               cur.y - 100, 300, 200, NULL, NULL, GetModuleHandleW(NULL), NULL);
    CHECK(win != NULL);
    POINT c = cur;
    CHECK(ScreenToClient(win, &c));
    HWND button = CreateWindowExW(0, L"BUTTON", L"hover", WS_CHILD | WS_VISIBLE, c.x - 60, c.y - 20, 120, 40, win,
                                  (HMENU)70, GetModuleHandleW(NULL), NULL);
    CHECK(button != NULL);
    HWND tt = CreateWindowExW(WS_EX_TOPMOST, TOOLTIPS_CLASSW, NULL, WS_POPUP | TTS_ALWAYSTIP, CW_USEDEFAULT,
                              CW_USEDEFAULT, CW_USEDEFAULT, CW_USEDEFAULT, win, NULL, GetModuleHandleW(NULL), NULL);
    CHECK(tt != NULL);
    // The documented defaults, from the double-click time.
    UINT dbl = GetDoubleClickTime();
    CHECK(SendMessageW(tt, TTM_GETDELAYTIME, TTDT_INITIAL, 0) == dbl);
    CHECK(SendMessageW(tt, TTM_GETDELAYTIME, TTDT_AUTOPOP, 0) == dbl * 10);
    CHECK(SendMessageW(tt, TTM_GETDELAYTIME, TTDT_RESHOW, 0) == dbl / 5);
    SendMessageW(tt, TTM_SETDELAYTIME, TTDT_INITIAL, MAKELPARAM(50, 0));
    CHECK(SendMessageW(tt, TTM_GETDELAYTIME, TTDT_INITIAL, 0) == 50);

    TOOLINFOW ti = {0};
    ti.cbSize = sizeof ti;
    ti.uFlags = TTF_IDISHWND | TTF_SUBCLASS;
    ti.hwnd = win;
    ti.uId = (UINT_PTR)button;
    ti.lpszText = L"Tip text";
    CHECK(SendMessageW(tt, TTM_ADDTOOLW, 0, (LPARAM)&ti));
    CHECK(!IsWindowVisible(tt));
    // The pointer in the button's client coordinates (its centre).
    LPARAM at = MAKELPARAM(60, 20), at2 = MAKELPARAM(61, 20), at3 = MAKELPARAM(60, 21);

    // Inactive: the pointer rests on the tool, nothing shows.
    SendMessageW(tt, TTM_ACTIVATE, FALSE, 0);
    SendMessageW(button, WM_MOUSEMOVE, 0, at);
    CHECK(!pump_until(tt, TRUE, 400));
    CHECK(tt_shows == 0);

    // Active: after the initial delay the tip shows (TTN_SHOW first).
    SendMessageW(tt, TTM_ACTIVATE, TRUE, 0);
    SendMessageW(button, WM_MOUSEMOVE, 0, at2);
    CHECK(pump_until(tt, TRUE, 3000));
    CHECK(tt_shows == 1 && tt_from == tt && tt_id == (UINT_PTR)button);
    RECT r;
    CHECK(GetWindowRect(tt, &r));
    CHECK(r.right - r.left > 16 && r.bottom - r.top > 8);
    // Near the pointer, and on the screen.
    CHECK(r.left - 64 <= cur.x && cur.x <= r.right + 64 && r.top - 64 <= cur.y && cur.y <= r.bottom + 64);
    CHECK(r.left >= 0 && r.top >= 0 && r.right <= GetSystemMetrics(SM_CXSCREEN) &&
          r.bottom <= GetSystemMetrics(SM_CYSCREEN));

    // A click hides it.
    SendMessageW(button, WM_LBUTTONDOWN, MK_LBUTTON, at2);
    SendMessageW(button, WM_LBUTTONUP, 0, at2);
    CHECK(pump_until(tt, FALSE, 1000));
    CHECK(tt_pops == 1);

    // A callback text: TTN_GETDISPINFOW asked of the tool's hwnd; TTM_POP
    // hides the tip.
    SendMessageW(tt, TTM_DELTOOLW, 0, (LPARAM)&ti);
    ti.lpszText = LPSTR_TEXTCALLBACKW;
    CHECK(SendMessageW(tt, TTM_ADDTOOLW, 0, (LPARAM)&ti));
    SendMessageW(button, WM_MOUSEMOVE, 0, at3);
    CHECK(pump_until(tt, TRUE, 3000));
    CHECK(tt_dispinfo >= 1 && tt_shows == 2);
    SendMessageW(tt, TTM_POP, 0, 0);
    CHECK(!IsWindowVisible(tt) && tt_pops == 2);

    // The autopop delay hides it.
    SendMessageW(tt, TTM_DELTOOLW, 0, (LPARAM)&ti);
    ti.lpszText = L"Tip text";
    CHECK(SendMessageW(tt, TTM_ADDTOOLW, 0, (LPARAM)&ti));
    SendMessageW(tt, TTM_SETDELAYTIME, TTDT_AUTOPOP, MAKELPARAM(300, 0));
    CHECK(SendMessageW(tt, TTM_GETDELAYTIME, TTDT_AUTOPOP, 0) == 300);
    SendMessageW(button, WM_MOUSEMOVE, 0, at);
    CHECK(pump_until(tt, TRUE, 3000));
    CHECK(tt_shows == 3);
    CHECK(pump_until(tt, FALSE, 3000));
    CHECK(tt_pops == 3);

    DestroyWindow(tt);
    DestroyWindow(win);
    pump_until(NULL, FALSE, 50);
}

/* The toolbar: buttons added (TB_ADDBUTTONSW after TB_BUTTONSTRUCTSIZE),
 * counted, enabled/disabled, checked, their rectangles side by side;
 * TB_AUTOSIZE puts it at the top, full width; a click on a button sends
 * the parent WM_COMMAND with its id; TB_DELETEBUTTON removes one. */
static void toolbar(HWND parent, HIMAGELIST il) {
    HWND tb = CreateWindowExW(0, TOOLBARCLASSNAMEW, NULL, WS_CHILD | WS_VISIBLE | TBSTYLE_FLAT, 0, 0, 0, 0, parent,
                              (HMENU)41, GetModuleHandleW(NULL), NULL);
    CHECK(tb != NULL);
    SendMessageW(tb, TB_BUTTONSTRUCTSIZE, sizeof(TBBUTTON), 0);
    CHECK(SendMessageW(tb, TB_SETIMAGELIST, 0, (LPARAM)il) == 0);
    CHECK((HIMAGELIST)SendMessageW(tb, TB_GETIMAGELIST, 0, 0) == il);
    TBBUTTON buttons[3] = {
        {0, 100, TBSTATE_ENABLED, BTNS_BUTTON, {0}, 0, 0},
        {0, 0, 0, BTNS_SEP, {0}, 0, 0},
        {1, 101, TBSTATE_ENABLED, BTNS_CHECK, {0}, 0, 0},
    };
    CHECK(SendMessageW(tb, TB_ADDBUTTONSW, 3, (LPARAM)buttons));
    CHECK(SendMessageW(tb, TB_BUTTONCOUNT, 0, 0) == 3);
    SendMessageW(tb, TB_AUTOSIZE, 0, 0);
    RECT pc, tr;
    GetClientRect(parent, &pc);
    GetWindowRect(tb, &tr);
    MapWindowPoints(NULL, parent, (POINT *)&tr, 2);
    CHECK(tr.top == 0 && tr.left == 0 && tr.right == pc.right && tr.bottom > 16);
    RECT a, b;
    CHECK(SendMessageW(tb, TB_GETITEMRECT, 0, (LPARAM)&a) && a.right > a.left);
    CHECK(SendMessageW(tb, TB_GETITEMRECT, 2, (LPARAM)&b) && b.left >= a.right);
    CHECK(SendMessageW(tb, TB_ISBUTTONENABLED, 100, 0));
    CHECK(SendMessageW(tb, TB_ENABLEBUTTON, 100, FALSE) && !SendMessageW(tb, TB_ISBUTTONENABLED, 100, 0));
    CHECK((SendMessageW(tb, TB_GETSTATE, 100, 0) & TBSTATE_ENABLED) == 0);
    SendMessageW(tb, TB_ENABLEBUTTON, 100, TRUE);
    CHECK(SendMessageW(tb, TB_CHECKBUTTON, 101, TRUE) && SendMessageW(tb, TB_ISBUTTONCHECKED, 101, 0));
    commands = 0;
    LPARAM at = MAKELPARAM((a.left + a.right) / 2, (a.top + a.bottom) / 2);
    SendMessageW(tb, WM_LBUTTONDOWN, MK_LBUTTON, at);
    SendMessageW(tb, WM_LBUTTONUP, 0, at);
    CHECK(commands == 1 && LOWORD(last_command) == 100);
    CHECK(SendMessageW(tb, TB_DELETEBUTTON, 0, 0) && SendMessageW(tb, TB_BUTTONCOUNT, 0, 0) == 2);
    DestroyWindow(tb);
    /* TB_ADDBITMAP with an HBITMAP strip (hInst NULL): its images after
     * the bitmap size, the first one's index returned (0, then 2). */
    HWND tb2 = CreateWindowExW(0, TOOLBARCLASSNAMEW, NULL, WS_CHILD | WS_VISIBLE, 0, 0, 0, 0, parent, (HMENU)42,
                               GetModuleHandleW(NULL), NULL);
    SendMessageW(tb2, TB_SETBITMAPSIZE, 0, MAKELPARAM(16, 16));
    HBITMAP strip = dib(32, 16, 16, RED, BLUE);
    TBADDBITMAP ab = {NULL, (UINT_PTR)strip};
    CHECK(SendMessageW(tb2, TB_ADDBITMAP, 2, (LPARAM)&ab) == 0);
    CHECK(SendMessageW(tb2, TB_ADDBITMAP, 2, (LPARAM)&ab) == 2);
    DeleteObject(strip);
    DestroyWindow(tb2);
}

/* The tree view: items inserted under the root and under each other
 * (TVM_INSERTITEMW returns their handles), counted, walked
 * (TVM_GETNEXTITEM: root, child, next, previous, parent), read back
 * (TVM_GETITEMW); expanding sends TVN_ITEMEXPANDINGW and sets
 * TVIS_EXPANDED; selecting sends TVN_SELCHANGEDW and is the caret;
 * I_CHILDRENCALLBACK asks the parent (TVN_GETDISPINFOW); item
 * rectangles, hit testing, image lists; deleting an item deletes its
 * children; TVI_ROOT deletes everything. */
static void tree_view(HWND parent, HIMAGELIST il) {
    HWND tv = CreateWindowExW(0, WC_TREEVIEWW, NULL,
                              WS_CHILD | WS_VISIBLE | TVS_HASLINES | TVS_HASBUTTONS | TVS_LINESATROOT, 0, 0, 300, 200,
                              parent, (HMENU)60, GetModuleHandleW(NULL), NULL);
    CHECK(tv != NULL);
    TVINSERTSTRUCTW ins = {0};
    ins.hParent = TVI_ROOT;
    ins.hInsertAfter = TVI_LAST;
    ins.item.mask = TVIF_TEXT | TVIF_PARAM;
    ins.item.pszText = L"C:";
    ins.item.lParam = 7;
    HTREEITEM c = (HTREEITEM)SendMessageW(tv, TVM_INSERTITEMW, 0, (LPARAM)&ins);
    ins.item.pszText = L"D:";
    HTREEITEM d = (HTREEITEM)SendMessageW(tv, TVM_INSERTITEMW, 0, (LPARAM)&ins);
    CHECK(c != NULL && d != NULL && c != d);
    ins.hParent = c;
    ins.item.pszText = L"Users";
    HTREEITEM users = (HTREEITEM)SendMessageW(tv, TVM_INSERTITEMW, 0, (LPARAM)&ins);
    ins.item.pszText = L"Windows";
    HTREEITEM windows = (HTREEITEM)SendMessageW(tv, TVM_INSERTITEMW, 0, (LPARAM)&ins);
    ins.hInsertAfter = TVI_FIRST;
    ins.item.pszText = L"Apps";
    HTREEITEM apps = (HTREEITEM)SendMessageW(tv, TVM_INSERTITEMW, 0, (LPARAM)&ins);
    CHECK(users && windows && apps);
    CHECK(SendMessageW(tv, TVM_GETCOUNT, 0, 0) == 5);
    CHECK((HTREEITEM)SendMessageW(tv, TVM_GETNEXTITEM, TVGN_ROOT, 0) == c);
    CHECK((HTREEITEM)SendMessageW(tv, TVM_GETNEXTITEM, TVGN_NEXT, (LPARAM)c) == d);
    CHECK((HTREEITEM)SendMessageW(tv, TVM_GETNEXTITEM, TVGN_NEXT, (LPARAM)d) == NULL);
    CHECK((HTREEITEM)SendMessageW(tv, TVM_GETNEXTITEM, TVGN_CHILD, (LPARAM)c) == apps);
    CHECK((HTREEITEM)SendMessageW(tv, TVM_GETNEXTITEM, TVGN_NEXT, (LPARAM)apps) == users);
    CHECK((HTREEITEM)SendMessageW(tv, TVM_GETNEXTITEM, TVGN_PREVIOUS, (LPARAM)windows) == users);
    CHECK((HTREEITEM)SendMessageW(tv, TVM_GETNEXTITEM, TVGN_PARENT, (LPARAM)windows) == c);
    WCHAR text[32];
    TVITEMW it = {0};
    it.mask = TVIF_TEXT | TVIF_PARAM;
    it.hItem = c;
    it.pszText = text;
    it.cchTextMax = 32;
    CHECK(SendMessageW(tv, TVM_GETITEMW, 0, (LPARAM)&it) && wcscmp(text, L"C:") == 0 && it.lParam == 7);

    tv_expanding = 0;
    CHECK(SendMessageW(tv, TVM_EXPAND, TVE_EXPAND, (LPARAM)c) && tv_expanding == 1);
    CHECK(SendMessageW(tv, TVM_GETITEMSTATE, (WPARAM)c, TVIS_EXPANDED) & TVIS_EXPANDED);
    tv_selchanged = 0;
    CHECK(SendMessageW(tv, TVM_SELECTITEM, TVGN_CARET, (LPARAM)users));
    CHECK(tv_selchanged == 1 && tv_new_selection == users);
    CHECK((HTREEITEM)SendMessageW(tv, TVM_GETNEXTITEM, TVGN_CARET, 0) == users);

    RECT a, b;
    *(HTREEITEM *)&a = c;
    *(HTREEITEM *)&b = apps;
    CHECK(SendMessageW(tv, TVM_GETITEMRECT, FALSE, (LPARAM)&a) && SendMessageW(tv, TVM_GETITEMRECT, FALSE, (LPARAM)&b));
    CHECK(b.top == a.bottom && b.bottom > b.top);
    TVHITTESTINFO ht = {{20, (b.top + b.bottom) / 2}, 0, 0};
    CHECK((HTREEITEM)SendMessageW(tv, TVM_HITTEST, 0, (LPARAM)&ht) == apps && ht.hItem == apps);

    CHECK(SendMessageW(tv, TVM_SETIMAGELIST, TVSIL_NORMAL, (LPARAM)il) == 0);
    CHECK((HIMAGELIST)SendMessageW(tv, TVM_GETIMAGELIST, TVSIL_NORMAL, 0) == il);

    /* I_CHILDRENCALLBACK: the parent says whether there are children. */
    ins.hParent = d;
    ins.hInsertAfter = TVI_LAST;
    ins.item.mask = TVIF_TEXT | TVIF_CHILDREN;
    ins.item.pszText = L"lazy";
    ins.item.cChildren = I_CHILDRENCALLBACK;
    HTREEITEM lazy = (HTREEITEM)SendMessageW(tv, TVM_INSERTITEMW, 0, (LPARAM)&ins);
    tv_children_asked = 0;
    it.mask = TVIF_CHILDREN;
    it.hItem = lazy;
    CHECK(SendMessageW(tv, TVM_GETITEMW, 0, (LPARAM)&it) && it.cChildren == 1 && tv_children_asked >= 1);

    CHECK(SendMessageW(tv, TVM_DELETEITEM, 0, (LPARAM)c) && SendMessageW(tv, TVM_GETCOUNT, 0, 0) == 2);
    CHECK(SendMessageW(tv, TVM_DELETEITEM, 0, (LPARAM)TVI_ROOT) && SendMessageW(tv, TVM_GETCOUNT, 0, 0) == 0);
    DestroyWindow(tv);
}

static void flat_scroll_bars(HWND parent) {
    HWND w = CreateWindowExW(0, L"Comctl32TestParent", L"flat", WS_CHILD | WS_VISIBLE | WS_VSCROLL, 0, 0, 100, 100,
                             parent, NULL, GetModuleHandleW(NULL), NULL);
    int v = -1;
    CHECK(w != NULL && !FlatSB_GetScrollProp(w, WSB_PROP_HSTYLE, &v));
    CHECK(InitializeFlatSB(w));
    CHECK(FlatSB_GetScrollProp(w, WSB_PROP_HSTYLE, &v));
    CHECK(FlatSB_SetScrollProp(w, WSB_PROP_VSTYLE, FSB_REGULAR_MODE, TRUE));
    CHECK(FlatSB_GetScrollProp(w, WSB_PROP_VSTYLE, &v) && v == FSB_REGULAR_MODE);
    CHECK(FlatSB_SetScrollProp(w, WSB_PROP_CXVSCROLL, 21, FALSE));
    CHECK(FlatSB_GetScrollProp(w, WSB_PROP_CXVSCROLL, &v) && v == 21);
    CHECK(FlatSB_SetScrollProp(w, WSB_PROP_VBKGCOLOR, RGB(1, 2, 3), FALSE));
    CHECK(FlatSB_GetScrollProp(w, WSB_PROP_VBKGCOLOR, &v) && v == RGB(1, 2, 3));
    CHECK(FlatSB_GetScrollProp(w, WSB_PROP_WINSTYLE, &v) && v == WS_VSCROLL);
    SCROLLINFO si = {sizeof si, SIF_RANGE | SIF_PAGE | SIF_POS, 0, 100, 10, 30};
    FlatSB_SetScrollInfo(w, SB_VERT, &si, TRUE);
    SCROLLINFO got = {sizeof got, SIF_RANGE | SIF_PAGE | SIF_POS};
    CHECK(FlatSB_GetScrollInfo(w, SB_VERT, &got));
    CHECK(got.nMin == 0 && got.nMax == 100 && got.nPage == 10 && got.nPos == 30);
    CHECK(FlatSB_SetScrollPos(w, SB_VERT, 40, TRUE) == 30 && FlatSB_GetScrollPos(w, SB_VERT) == 40);
    CHECK(FlatSB_SetScrollRange(w, SB_VERT, 0, 50, TRUE));
    int lo = -1, hi = -1;
    CHECK(FlatSB_GetScrollRange(w, SB_VERT, &lo, &hi) && lo == 0 && hi == 50);
    CHECK(FlatSB_ShowScrollBar(w, SB_VERT, TRUE));
    CHECK(UninitializeFlatSB(w) == S_OK && UninitializeFlatSB(w) == S_FALSE);
    DestroyWindow(w);
}

int main(void) {
    INITCOMMONCONTROLSEX icc = {sizeof icc, ICC_WIN95_CLASSES | ICC_BAR_CLASSES};
    CHECK(InitCommonControlsEx(&icc));
    WNDCLASSW wc = {0};
    wc.lpfnWndProc = parent_proc;
    wc.hInstance = GetModuleHandleW(NULL);
    wc.lpszClassName = L"Comctl32TestParent";
    CHECK(RegisterClassW(&wc) != 0);
    HWND parent = CreateWindowExW(0, L"Comctl32TestParent", L"comctl32", WS_OVERLAPPEDWINDOW | WS_VISIBLE, 40, 40,
                                  400, 300, NULL, NULL, GetModuleHandleW(NULL), NULL);
    CHECK(parent != NULL);
    flat_scroll_bars(parent);
    HDC screen = GetDC(NULL);
    HDC dc = CreateCompatibleDC(screen);
    HBITMAP canvas = CreateCompatibleBitmap(screen, 64, 64);
    HGDIOBJ old = SelectObject(dc, canvas);

    HIMAGELIST il = ImageList_Create(16, 16, ILC_COLOR32 | ILC_MASK, 4, 4);
    CHECK(il != NULL && ImageList_GetImageCount(il) == 0);
    int cx = 0, cy = 0;
    CHECK(ImageList_GetIconSize(il, &cx, &cy) && cx == 16 && cy == 16);

    // Two images from one 32 x 16 bitmap: red, then blue.
    HBITMAP two = dib(32, 16, 16, RED, BLUE);
    CHECK(ImageList_Add(il, two, NULL) == 0 && ImageList_GetImageCount(il) == 2);
    // A green image whose left half is magenta, the mask colour.
    HBITMAP keyed = dib(16, 16, 8, MAGENTA, GREEN);
    CHECK(ImageList_AddMasked(il, keyed, rgb(MAGENTA)) == 2 && ImageList_GetImageCount(il) == 3);

    PatBlt(dc, 0, 0, 64, 64, WHITENESS);
    CHECK(ImageList_Draw(il, 0, dc, 0, 0, ILD_NORMAL));
    CHECK(GetPixel(dc, 0, 0) == RGB(255, 0, 0) && GetPixel(dc, 15, 15) == RGB(255, 0, 0));
    CHECK(GetPixel(dc, 16, 0) == RGB(255, 255, 255));
    CHECK(ImageList_Draw(il, 1, dc, 20, 0, ILD_TRANSPARENT) && GetPixel(dc, 20, 0) == RGB(0, 0, 255));
    // Transparent where the mask colour was.
    CHECK(ImageList_Draw(il, 2, dc, 40, 0, ILD_TRANSPARENT));
    CHECK(GetPixel(dc, 40, 0) == RGB(255, 255, 255) && GetPixel(dc, 48, 0) == RGB(0, 255, 0));
    // The background colour: CLR_NONE at first; ILD_NORMAL fills it.
    CHECK(ImageList_SetBkColor(il, RGB(0, 0, 0)) == CLR_NONE && ImageList_GetBkColor(il) == RGB(0, 0, 0));
    CHECK(ImageList_Draw(il, 2, dc, 40, 20, ILD_NORMAL));
    CHECK(GetPixel(dc, 40, 20) == RGB(0, 0, 0) && GetPixel(dc, 48, 20) == RGB(0, 255, 0));
    ImageList_SetBkColor(il, CLR_NONE);

    IMAGEINFO ii;
    CHECK(ImageList_GetImageInfo(il, 1, &ii) && ii.hbmImage != NULL);
    CHECK(ii.rcImage.right - ii.rcImage.left == 16 && ii.rcImage.bottom - ii.rcImage.top == 16);

    // An icon appended, and one made from an image.
    HBITMAP icolor = dib(16, 16, 16, GREEN, GREEN);
    HBITMAP imask = CreateBitmap(16, 16, 1, 1, NULL);
    ICONINFO info = {TRUE, 0, 0, imask, icolor};
    HICON icon = CreateIconIndirect(&info);
    CHECK(icon != NULL && ImageList_ReplaceIcon(il, -1, icon) == 3 && ImageList_GetImageCount(il) == 4);
    CHECK(ImageList_Draw(il, 3, dc, 0, 40, ILD_NORMAL) && GetPixel(dc, 5, 45) == RGB(0, 255, 0));
    HICON made = ImageList_GetIcon(il, 0, ILD_NORMAL);
    ICONINFO back;
    CHECK(made != NULL && GetIconInfo(made, &back) && back.hbmColor != NULL);
    DeleteObject(back.hbmColor);
    DeleteObject(back.hbmMask);
    DestroyIcon(made);
    DestroyIcon(icon);

    // Swap 0 and 1: image 0 is blue now.
    CHECK(ImageList_Copy(il, 0, il, 1, ILCF_SWAP));
    CHECK(ImageList_Draw(il, 0, dc, 0, 0, ILD_NORMAL) && GetPixel(dc, 0, 0) == RGB(0, 0, 255));
    // Replace image 1 (red now) with green.
    CHECK(ImageList_Replace(il, 1, icolor, NULL));
    CHECK(ImageList_Draw(il, 1, dc, 0, 0, ILD_NORMAL) && GetPixel(dc, 0, 0) == RGB(0, 255, 0));
    // Remove the first: the green one is first.
    CHECK(ImageList_Remove(il, 0) && ImageList_GetImageCount(il) == 3);
    CHECK(ImageList_Draw(il, 0, dc, 0, 0, ILD_NORMAL) && GetPixel(dc, 0, 0) == RGB(0, 255, 0));
    CHECK(ImageList_Remove(il, -1) && ImageList_GetImageCount(il) == 0);
    status_bar(parent);
    tooltips(parent);
    tooltip_hover();
    CHECK(ImageList_Add(il, two, NULL) == 0);
    toolbar(parent, il);
    list_view(parent, il);
    tree_view(parent, il);
    // Dragging an image: an image that is not there fails; then begin,
    // enter the window, move, change the cursor image, leave, end.
    CHECK(!ImageList_BeginDrag(il, 99, 0, 0));
    CHECK(ImageList_BeginDrag(il, 0, 2, 3));
    CHECK(ImageList_DragEnter(parent, 10, 10));
    CHECK(ImageList_DragMove(20, 25));
    CHECK(ImageList_SetDragCursorImage(il, 0, 1, 1));
    CHECK(ImageList_DragLeave(parent));
    ImageList_EndDrag();
    CHECK(ImageList_Destroy(il));

    DeleteObject(two);
    DeleteObject(keyed);
    DeleteObject(icolor);
    DeleteObject(imask);
    SelectObject(dc, old);
    DeleteObject(canvas);
    DeleteDC(dc);
    ReleaseDC(NULL, screen);
    DestroyWindow(parent);
    printf("comctl32_test: ok\n");
    return 0;
}
