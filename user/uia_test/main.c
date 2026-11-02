/* UI Automation's client API (UIAutomationCore.dll, IUIAutomation) over
 * Win32 windows and standard controls, as Microsoft documents it; the
 * first step towards screen tests that find and drive controls by
 * AutomationId, Name and ControlType on Windows and gala alike.
 *
 * - Another process (the program again, "child"): its window is found
 *   from the root by Name ("UIA Child"), its ProcessId is the child's,
 *   its button found by AutomationId and invoked (the child's window
 *   gets BN_CLICKED and retitles itself "pressed"), its edit's Value set
 *   and read back, its check box toggled.
 * - List items (UIA's Win32 list proxies; Microsoft's ListItem control
 *   type): a LISTBOX's strings and a report-view list view's items are
 *   ListItem children of their List (after its child windows: the list
 *   view's header), in index order: Name the item's text (column 0),
 *   ClassName empty, no NativeWindowHandle (0), the list's ProcessId,
 *   IsEnabled, a BoundingRectangle inside the list's, distinct
 *   RuntimeIds (CompareElements), list-view AutomationIds
 *   "ListViewItem-<index>"; walkers go from the list to its items and
 *   back. SelectionItem: Select selects (LB_GETCURSEL and LVM_GETNEXTITEM
 *   LVNI_SELECTED agree), IsSelected, SelectionContainer the list,
 *   AddToSelection and RemoveFromSelection on a multiple-selection list
 *   view (LVM_GETSELECTEDCOUNT). Selection on the list:
 *   GetCurrentSelection, CanSelectMultiple (a single-selection list box:
 *   FALSE; a list view: TRUE), the Selection property (VT_UNKNOWN, an
 *   element array). ScrollItem.ScrollIntoView. Invoke on a list-view
 *   item activates it (its parent gets LVN_ITEMACTIVATE). The child
 *   process's list view and list box are read and selected the same
 *   way, across processes.
 * - CoCreateInstance(CLSID_CUIAutomation, IID_IUIAutomation) in an STA.
 * - ElementFromHandle(top-level window): Name is the title, ControlType
 *   Window (it has a caption), ClassName the window class,
 *   NativeWindowHandle the HWND, ProcessId this process.
 * - GetRootElement: the desktop (Pane, NativeWindowHandle
 *   GetDesktopWindow()); its children include the window (FindFirst
 *   with a NativeWindowHandle property condition, VT_I4).
 * - Win32 controls: AutomationId is the control id in decimal, Name the
 *   button's or static's text, ControlType Button / Edit / Text /
 *   CheckBox by class and style, ClassName "Button" / "Edit" / "Static",
 *   BoundingRectangle the control's GetWindowRect, IsEnabled.
 * - Conditions: property (BSTR and int), true, and, or, not; FindFirst
 *   and FindAll with TreeScope_Element, _Children, _Descendants (a find
 *   with no match gives S_OK and NULL); GetCurrentPropertyValue's VARIANT
 *   types (VT_I4, VT_BSTR, VT_BOOL); CompareElements.
 * - Patterns: GetCurrentPattern gives NULL (S_OK) for one the element
 *   lacks. Invoke on a push button makes its parent get WM_COMMAND
 *   BN_CLICKED (Invoke may return before that: messages are pumped until
 *   it arrives). Value on an edit: SetValue sets the window text,
 *   CurrentValue reads it, not read-only. Toggle on an auto check box:
 *   Off, then On after Toggle (BM_GETCHECK agrees).
 *
 * The window has WS_CAPTION but no system menu: on Windows its UIA
 * children then include a TitleBar element (no HWND of its own) besides
 * the three controls, so the children with a NativeWindowHandle are
 * counted exactly and all children only as at least three.
 *
 * Prints "uia_test: ok" or exits with the number of the failing check. */
#define COBJMACROS
#include <windows.h>
#include <initguid.h>
#include <uiautomation.h>
#include <commctrl.h>
#include <stdio.h>
#include <string.h>

/* TreeScope_Subtree (mingw-w64 spells it TreeScope_SubTree). */
#define SUBTREE ((enum TreeScope)(TreeScope_Element | TreeScope_Children | TreeScope_Descendants))

static int check;
#define CHECK(cond) do { check++; if (!(cond)) { printf("uia_test: check %d (line %d) failed\n", check, __LINE__); fflush(stdout); ExitProcess(check); } } while (0)

static WPARAM last_command;
static LPARAM last_command_from;
static int commands;
static volatile int activations, activated_item = -1;
static LRESULT CALLBACK parent_proc(HWND hwnd, UINT msg, WPARAM wparam, LPARAM lparam) {
    if (msg == WM_NOTIFY && ((NMHDR *)lparam)->code == LVN_ITEMACTIVATE) {
        activated_item = ((NMITEMACTIVATE *)lparam)->iItem;
        activations++;
    }
    if (msg == WM_COMMAND) {
        last_command = wparam;
        last_command_from = lparam;
        commands++;
    }
    return DefWindowProcW(hwnd, msg, wparam, lparam);
}

/* Dispatches messages until `*flag` changes from `old` or 5 seconds pass. */
static void pump_until(volatile int *flag, int old) {
    DWORD start = GetTickCount();
    while (*flag == old && GetTickCount() - start < 5000) {
        MSG m;
        while (PeekMessageW(&m, NULL, 0, 0, PM_REMOVE)) {
            TranslateMessage(&m);
            DispatchMessageW(&m);
        }
        if (*flag == old)
            Sleep(10);
    }
}

static int bstr_is(BSTR b, const wchar_t *s) {
    return b != NULL && wcscmp(b, s) == 0 && SysStringLen(b) == wcslen(s);
}

static IUIAutomationCondition *prop_str(IUIAutomation *uia, PROPERTYID id, const wchar_t *s) {
    VARIANT v;
    VariantInit(&v);
    v.vt = VT_BSTR;
    v.bstrVal = SysAllocString(s);
    IUIAutomationCondition *c = NULL;
    CHECK(IUIAutomation_CreatePropertyCondition(uia, id, v, &c) == S_OK && c != NULL);
    VariantClear(&v);
    return c;
}

static IUIAutomationCondition *prop_int(IUIAutomation *uia, PROPERTYID id, int i) {
    VARIANT v;
    VariantInit(&v);
    v.vt = VT_I4;
    v.lVal = i;
    IUIAutomationCondition *c = NULL;
    CHECK(IUIAutomation_CreatePropertyCondition(uia, id, v, &c) == S_OK && c != NULL);
    return c;
}

static HWND element_hwnd(IUIAutomationElement *e) {
    UIA_HWND h = (UIA_HWND)-1;
    CHECK(IUIAutomationElement_get_CurrentNativeWindowHandle(e, &h) == S_OK);
    return (HWND)h;
}

static int control_type(IUIAutomationElement *e) {
    CONTROLTYPEID t = 0;
    CHECK(IUIAutomationElement_get_CurrentControlType(e, &t) == S_OK);
    return t;
}

static int name_is(IUIAutomationElement *e, const wchar_t *s) {
    BSTR b = NULL;
    CHECK(IUIAutomationElement_get_CurrentName(e, &b) == S_OK);
    int same = bstr_is(b, s);
    SysFreeString(b);
    return same;
}

static int class_is(IUIAutomationElement *e, const wchar_t *s) {
    BSTR b = NULL;
    CHECK(IUIAutomationElement_get_CurrentClassName(e, &b) == S_OK);
    int same = bstr_is(b, s);
    SysFreeString(b);
    return same;
}

/* The second process: a window "UIA Child" with a button (101,
 * "Press": clicking it sets the title to "pressed"), an edit (102) and a
 * check box (103), until the parent sets "UiaChildDone". */
/* A report-view list view (two columns, three items "<prefix> 0".."2"
 * with sizes) at (x, y) in `parent`. */
static HWND make_list_view(HWND parent, int id, int x, int y, const wchar_t *prefix) {
    HWND lv = CreateWindowExW(0, WC_LISTVIEWW, NULL, WS_CHILD | WS_VISIBLE | WS_BORDER | LVS_REPORT, x, y, 220, 120,
                              parent, (HMENU)(INT_PTR)id, GetModuleHandleW(NULL), NULL);
    if (!lv)
        return NULL;
    LVCOLUMNW col = {0};
    col.mask = LVCF_TEXT | LVCF_WIDTH;
    col.cx = 100;
    col.pszText = L"Name";
    SendMessageW(lv, LVM_INSERTCOLUMNW, 0, (LPARAM)&col);
    col.cx = 60;
    col.pszText = L"Size";
    SendMessageW(lv, LVM_INSERTCOLUMNW, 1, (LPARAM)&col);
    for (int i = 0; i < 3; i++) {
        wchar_t text[32], size[16];
        swprintf(text, 32, L"%ls %d", prefix, i);
        swprintf(size, 16, L"%d", (i + 1) * 100);
        LVITEMW item = {0};
        item.mask = LVIF_TEXT;
        item.iItem = i;
        item.pszText = text;
        SendMessageW(lv, LVM_INSERTITEMW, 0, (LPARAM)&item);
        item.iSubItem = 1;
        item.pszText = size;
        SendMessageW(lv, LVM_SETITEMTEXTW, i, (LPARAM)&item);
    }
    return lv;
}

static LRESULT CALLBACK child_proc(HWND hwnd, UINT msg, WPARAM wparam, LPARAM lparam) {
    if (msg == WM_COMMAND && LOWORD(wparam) == 101 && HIWORD(wparam) == BN_CLICKED) {
        SetWindowTextW(hwnd, L"pressed");
        return 0;
    }
    return DefWindowProcW(hwnd, msg, wparam, lparam);
}

static int child(void) {
    HANDLE ready = OpenEventW(EVENT_MODIFY_STATE, FALSE, L"UiaChildReady");
    HANDLE done = OpenEventW(SYNCHRONIZE, FALSE, L"UiaChildDone");
    if (!ready || !done)
        return 2;
    WNDCLASSW wc = {0};
    wc.lpfnWndProc = child_proc;
    wc.hInstance = GetModuleHandleW(NULL);
    wc.lpszClassName = L"UiaChildWindow";
    if (!RegisterClassW(&wc))
        return 3;
    HWND top = CreateWindowExW(0, L"UiaChildWindow", L"UIA Child", WS_OVERLAPPED | WS_CAPTION | WS_VISIBLE, 200,
                               150, 300, 200, NULL, NULL, wc.hInstance, NULL);
    CreateWindowExW(0, L"BUTTON", L"Press", WS_CHILD | WS_VISIBLE | BS_PUSHBUTTON, 10, 10, 80, 24, top,
                    (HMENU)101, wc.hInstance, NULL);
    CreateWindowExW(0, L"EDIT", L"", WS_CHILD | WS_VISIBLE | WS_BORDER | ES_LEFT, 10, 40, 150, 22, top,
                    (HMENU)102, wc.hInstance, NULL);
    CreateWindowExW(0, L"BUTTON", L"Remote option", WS_CHILD | WS_VISIBLE | BS_AUTOCHECKBOX, 10, 70, 150, 20,
                    top, (HMENU)103, wc.hInstance, NULL);
    INITCOMMONCONTROLSEX icc = {sizeof icc, ICC_LISTVIEW_CLASSES};
    InitCommonControlsEx(&icc);
    HWND lv = make_list_view(top, 104, 160, 10, L"remote");
    HWND lb = CreateWindowExW(0, L"LISTBOX", NULL, WS_CHILD | WS_VISIBLE | WS_BORDER, 10, 100, 140, 60, top,
                              (HMENU)105, wc.hInstance, NULL);
    if (!lv || !lb)
        return 4;
    SendMessageW(lb, LB_ADDSTRING, 0, (LPARAM)L"r-one");
    SendMessageW(lb, LB_ADDSTRING, 0, (LPARAM)L"r-two");
    SetEvent(ready);
    for (;;) {
        DWORD r = MsgWaitForMultipleObjects(1, &done, FALSE, 30000, QS_ALLINPUT);
        if (r == WAIT_OBJECT_0 || r == WAIT_TIMEOUT)
            break;
        MSG m;
        while (PeekMessageW(&m, NULL, 0, 0, PM_REMOVE)) {
            TranslateMessage(&m);
            DispatchMessageW(&m);
        }
    }
    DestroyWindow(top);
    return 0;
}

static IUnknown *pattern_of(IUIAutomationElement *e, PATTERNID id, REFIID iid) {
    IUnknown *unk = NULL, *p = NULL;
    CHECK(IUIAutomationElement_GetCurrentPattern(e, id, &unk) == S_OK && unk != NULL);
    CHECK(IUnknown_QueryInterface(unk, iid, (void **)&p) == S_OK && p != NULL);
    IUnknown_Release(unk);
    return p;
}

static int same_element(IUIAutomation *uia, IUIAutomationElement *a, IUIAutomationElement *b) {
    BOOL same = FALSE;
    CHECK(IUIAutomation_CompareElements(uia, a, b, &same) == S_OK);
    return same;
}

static int is_selected(IUIAutomationElement *item) {
    IUIAutomationSelectionItemPattern *si = (IUIAutomationSelectionItemPattern *)pattern_of(
        item, UIA_SelectionItemPatternId, &IID_IUIAutomationSelectionItemPattern);
    BOOL sel = -1;
    CHECK(IUIAutomationSelectionItemPattern_get_CurrentIsSelected(si, &sel) == S_OK);
    IUIAutomationSelectionItemPattern_Release(si);
    return sel;
}

/* Select (0), AddToSelection (1) or RemoveFromSelection (2). */
static void select_item(IUIAutomationElement *item, int how) {
    IUIAutomationSelectionItemPattern *si = (IUIAutomationSelectionItemPattern *)pattern_of(
        item, UIA_SelectionItemPatternId, &IID_IUIAutomationSelectionItemPattern);
    HRESULT hr = how == 0   ? IUIAutomationSelectionItemPattern_Select(si)
                 : how == 1 ? IUIAutomationSelectionItemPattern_AddToSelection(si)
                            : IUIAutomationSelectionItemPattern_RemoveFromSelection(si);
    CHECK(hr == S_OK);
    IUIAutomationSelectionItemPattern_Release(si);
}

/* The list `list`'s ListItem children: `n` of them, named `names`, each
 * inside the list's rectangle and below the one before, with no window,
 * of process `pid`; returned in `items`. */
static void list_items(IUIAutomation *uia, IUIAutomationElement *list, int n, const wchar_t **names,
                       IUIAutomationElement **items, int pid) {
    IUIAutomationCondition *is_item = prop_int(uia, UIA_ControlTypePropertyId, UIA_ListItemControlTypeId);
    IUIAutomationElementArray *found = NULL;
    CHECK(IUIAutomationElement_FindAll(list, TreeScope_Children, is_item, &found) == S_OK && found != NULL);
    int count = -1;
    CHECK(IUIAutomationElementArray_get_Length(found, &count) == S_OK && count == n);
    RECT lr;
    CHECK(IUIAutomationElement_get_CurrentBoundingRectangle(list, &lr) == S_OK);
    IUIAutomationTreeWalker *raw = NULL;
    CHECK(IUIAutomation_get_RawViewWalker(uia, &raw) == S_OK && raw != NULL);
    for (int i = 0; i < n; i++) {
        items[i] = NULL;
        CHECK(IUIAutomationElementArray_GetElement(found, i, &items[i]) == S_OK && items[i] != NULL);
        CHECK(control_type(items[i]) == UIA_ListItemControlTypeId);
        CHECK(name_is(items[i], names[i]));
        CHECK(class_is(items[i], L""));
        CHECK(element_hwnd(items[i]) == NULL);
        int ipid = 0;
        CHECK(IUIAutomationElement_get_CurrentProcessId(items[i], &ipid) == S_OK && ipid == pid);
        BOOL enabled = FALSE;
        CHECK(IUIAutomationElement_get_CurrentIsEnabled(items[i], &enabled) == S_OK && enabled);
        BOOL offscreen = TRUE;
        CHECK(IUIAutomationElement_get_CurrentIsOffscreen(items[i], &offscreen) == S_OK && !offscreen);
        BSTR lct = NULL;
        CHECK(IUIAutomationElement_get_CurrentLocalizedControlType(items[i], &lct) == S_OK &&
              bstr_is(lct, L"list item"));
        SysFreeString(lct);
        RECT r;
        CHECK(IUIAutomationElement_get_CurrentBoundingRectangle(items[i], &r) == S_OK);
        CHECK(r.left >= lr.left && r.right <= lr.right && r.top >= lr.top && r.bottom <= lr.bottom);
        CHECK(r.right > r.left && r.bottom > r.top);
        if (i > 0) {
            RECT p;
            CHECK(IUIAutomationElement_get_CurrentBoundingRectangle(items[i - 1], &p) == S_OK);
            CHECK(r.top >= p.bottom);
            CHECK(!same_element(uia, items[i - 1], items[i]));
        }
        IUIAutomationElement *parent = NULL;
        CHECK(IUIAutomationTreeWalker_GetParentElement(raw, items[i], &parent) == S_OK && parent != NULL);
        CHECK(same_element(uia, parent, list));
        IUIAutomationElement_Release(parent);
    }
    IUIAutomationTreeWalker_Release(raw);
    IUIAutomationElementArray_Release(found);
    IUIAutomationCondition_Release(is_item);
}

static void release_all(IUIAutomationElement **items, int n) {
    for (int i = 0; i < n; i++)
        IUIAutomationElement_Release(items[i]);
}

/* The list's selection (Selection pattern): `n` elements, the first
 * being `first` (if given). */
static void selection_is(IUIAutomation *uia, IUIAutomationElement *list, int n, IUIAutomationElement *first) {
    IUIAutomationSelectionPattern *sp =
        (IUIAutomationSelectionPattern *)pattern_of(list, UIA_SelectionPatternId, &IID_IUIAutomationSelectionPattern);
    IUIAutomationElementArray *sel = NULL;
    CHECK(IUIAutomationSelectionPattern_GetCurrentSelection(sp, &sel) == S_OK && sel != NULL);
    int count = -1;
    CHECK(IUIAutomationElementArray_get_Length(sel, &count) == S_OK && count == n);
    if (first) {
        IUIAutomationElement *e = NULL;
        CHECK(IUIAutomationElementArray_GetElement(sel, 0, &e) == S_OK && e != NULL);
        CHECK(same_element(uia, e, first));
        IUIAutomationElement_Release(e);
    }
    IUIAutomationElementArray_Release(sel);
    IUIAutomationSelectionPattern_Release(sp);
}

static int can_select_multiple(IUIAutomationElement *list) {
    IUIAutomationSelectionPattern *sp =
        (IUIAutomationSelectionPattern *)pattern_of(list, UIA_SelectionPatternId, &IID_IUIAutomationSelectionPattern);
    BOOL multi = -1, required = -1;
    CHECK(IUIAutomationSelectionPattern_get_CurrentCanSelectMultiple(sp, &multi) == S_OK);
    CHECK(IUIAutomationSelectionPattern_get_CurrentIsSelectionRequired(sp, &required) == S_OK && !required);
    IUIAutomationSelectionPattern_Release(sp);
    return multi;
}

/* A list box and a list view in this process (see the top). */
static void lists(IUIAutomation *uia) {
    HWND top = CreateWindowExW(0, L"UiaTestWindow", L"UIA Lists", WS_OVERLAPPED | WS_CAPTION | WS_VISIBLE, 100, 420,
                               420, 220, NULL, NULL, GetModuleHandleW(NULL), NULL);
    CHECK(top != NULL);
    HWND lb = CreateWindowExW(0, L"LISTBOX", NULL, WS_CHILD | WS_VISIBLE | WS_BORDER, 10, 10, 140, 80, top,
                              (HMENU)301, GetModuleHandleW(NULL), NULL);
    CHECK(lb != NULL);
    SendMessageW(lb, LB_ADDSTRING, 0, (LPARAM)L"alpha");
    SendMessageW(lb, LB_ADDSTRING, 0, (LPARAM)L"beta");
    SendMessageW(lb, LB_ADDSTRING, 0, (LPARAM)L"gamma");
    HWND lv = make_list_view(top, 302, 170, 10, L"file");
    CHECK(lv != NULL);
    UpdateWindow(top);
    IUIAutomationElement *win = NULL;
    CHECK(IUIAutomation_ElementFromHandle(uia, top, &win) == S_OK && win != NULL);

    // The list box.
    IUIAutomationCondition *id301 = prop_str(uia, UIA_AutomationIdPropertyId, L"301");
    IUIAutomationElement *list = NULL;
    CHECK(IUIAutomationElement_FindFirst(win, TreeScope_Children, id301, &list) == S_OK && list != NULL);
    CHECK(control_type(list) == UIA_ListControlTypeId);
    const wchar_t *lb_names[3] = {L"alpha", L"beta", L"gamma"};
    IUIAutomationElement *items[3];
    list_items(uia, list, 3, lb_names, items, (int)GetCurrentProcessId());
    // Walkers: the list's first child is its first item, then the next.
    IUIAutomationTreeWalker *walker = NULL;
    CHECK(IUIAutomation_get_ControlViewWalker(uia, &walker) == S_OK && walker != NULL);
    IUIAutomationElement *e = NULL, *next = NULL;
    CHECK(IUIAutomationTreeWalker_GetFirstChildElement(walker, list, &e) == S_OK && e && same_element(uia, e, items[0]));
    CHECK(IUIAutomationTreeWalker_GetNextSiblingElement(walker, e, &next) == S_OK && next &&
          same_element(uia, next, items[1]));
    IUIAutomationElement_Release(next);
    IUIAutomationElement_Release(e);
    CHECK(IUIAutomationTreeWalker_GetLastChildElement(walker, list, &e) == S_OK && e && same_element(uia, e, items[2]));
    next = (IUIAutomationElement *)1;
    CHECK(IUIAutomationTreeWalker_GetNextSiblingElement(walker, e, &next) == S_OK && next == NULL);
    IUIAutomationElement_Release(e);
    // Selection.
    CHECK(!can_select_multiple(list));
    selection_is(uia, list, 0, NULL);
    CHECK(!is_selected(items[1]));
    select_item(items[1], 0);
    CHECK(SendMessageW(lb, LB_GETCURSEL, 0, 0) == 1);
    CHECK(is_selected(items[1]) && !is_selected(items[0]));
    selection_is(uia, list, 1, items[1]);
    select_item(items[2], 0);
    CHECK(SendMessageW(lb, LB_GETCURSEL, 0, 0) == 2 && !is_selected(items[1]));
    IUIAutomationSelectionItemPattern *si = (IUIAutomationSelectionItemPattern *)pattern_of(
        items[2], UIA_SelectionItemPatternId, &IID_IUIAutomationSelectionItemPattern);
    e = NULL;
    CHECK(IUIAutomationSelectionItemPattern_get_CurrentSelectionContainer(si, &e) == S_OK && e != NULL);
    CHECK(same_element(uia, e, list));
    IUIAutomationElement_Release(e);
    IUIAutomationSelectionItemPattern_Release(si);
    VARIANT v;
    VariantInit(&v);
    CHECK(IUIAutomationElement_GetCurrentPropertyValue(items[2], UIA_SelectionItemIsSelectedPropertyId, &v) ==
              S_OK &&
          v.vt == VT_BOOL && v.boolVal == VARIANT_TRUE);
    CHECK(IUIAutomationElement_GetCurrentPropertyValue(list, UIA_SelectionSelectionPropertyId, &v) == S_OK &&
          v.vt == VT_UNKNOWN && v.punkVal != NULL);
    IUIAutomationElementArray *arr = NULL;
    int n = -1;
    CHECK(IUnknown_QueryInterface(v.punkVal, &IID_IUIAutomationElementArray, (void **)&arr) == S_OK && arr);
    CHECK(IUIAutomationElementArray_get_Length(arr, &n) == S_OK && n == 1);
    IUIAutomationElementArray_Release(arr);
    VariantClear(&v);
    // A find from the window reaches items (by Name).
    IUIAutomationCondition *beta = prop_str(uia, UIA_NamePropertyId, L"beta");
    e = NULL;
    CHECK(IUIAutomationElement_FindFirst(win, TreeScope_Descendants, beta, &e) == S_OK && e != NULL);
    CHECK(same_element(uia, e, items[1]));
    IUIAutomationElement_Release(e);
    IUIAutomationCondition_Release(beta);
    release_all(items, 3);
    IUIAutomationElement_Release(list);

    // The list view: its header is a child window before the items.
    IUIAutomationCondition *id302 = prop_str(uia, UIA_AutomationIdPropertyId, L"302");
    CHECK(IUIAutomationElement_FindFirst(win, TreeScope_Children, id302, &list) == S_OK && list != NULL);
    CHECK(control_type(list) == UIA_ListControlTypeId);
    const wchar_t *lv_names[3] = {L"file 0", L"file 1", L"file 2"};
    list_items(uia, list, 3, lv_names, items, (int)GetCurrentProcessId());
    BSTR aid = NULL;
    CHECK(IUIAutomationElement_get_CurrentAutomationId(items[1], &aid) == S_OK && bstr_is(aid, L"ListViewItem-1"));
    SysFreeString(aid);
    CHECK(IUIAutomationTreeWalker_GetLastChildElement(walker, list, &e) == S_OK && e && same_element(uia, e, items[2]));
    IUIAutomationElement_Release(e);
    CHECK(can_select_multiple(list));
    selection_is(uia, list, 0, NULL);
    select_item(items[2], 0);
    CHECK(SendMessageW(lv, LVM_GETNEXTITEM, -1, LVNI_SELECTED) == 2);
    CHECK(is_selected(items[2]) && !is_selected(items[0]));
    selection_is(uia, list, 1, items[2]);
    select_item(items[0], 1);
    CHECK(SendMessageW(lv, LVM_GETSELECTEDCOUNT, 0, 0) == 2 && is_selected(items[0]) && is_selected(items[2]));
    selection_is(uia, list, 2, items[0]);
    select_item(items[0], 2);
    CHECK(SendMessageW(lv, LVM_GETSELECTEDCOUNT, 0, 0) == 1 && !is_selected(items[0]));
    // ScrollIntoView.
    IUIAutomationScrollItemPattern *scroll = (IUIAutomationScrollItemPattern *)pattern_of(
        items[1], UIA_ScrollItemPatternId, &IID_IUIAutomationScrollItemPattern);
    CHECK(IUIAutomationScrollItemPattern_ScrollIntoView(scroll) == S_OK);
    IUIAutomationScrollItemPattern_Release(scroll);
    // Invoke: the item is activated (LVN_ITEMACTIVATE to the parent).
    IUIAutomationInvokePattern *invoke =
        (IUIAutomationInvokePattern *)pattern_of(items[1], UIA_InvokePatternId, &IID_IUIAutomationInvokePattern);
    activations = 0;
    CHECK(IUIAutomationInvokePattern_Invoke(invoke) == S_OK);
    pump_until((volatile int *)&activations, 0);
    CHECK(activations == 1 && activated_item == 1);
    IUIAutomationInvokePattern_Release(invoke);
    release_all(items, 3);
    IUIAutomationElement_Release(list);
    IUIAutomationCondition_Release(id302);
    IUIAutomationTreeWalker_Release(walker);
    IUIAutomationCondition_Release(id301);
    IUIAutomationElement_Release(win);
    DestroyWindow(top);
}

/* Waits (up to 10 s) for element `e`'s Name to become `want`. */
static int name_becomes(IUIAutomationElement *e, const wchar_t *want) {
    for (int i = 0; i < 1000; i++) {
        if (name_is(e, want))
            return 1;
        Sleep(10);
    }
    return 0;
}

/* UI Automation over another process's windows: found from the root by
 * Name, its controls by AutomationId, driven by Invoke, Value and Toggle. */
static void other_process(IUIAutomation *uia, IUIAutomationElement *root) {
    HANDLE ready = CreateEventW(NULL, TRUE, FALSE, L"UiaChildReady");
    HANDLE done = CreateEventW(NULL, TRUE, FALSE, L"UiaChildDone");
    CHECK(ready && done);
    WCHAR self[MAX_PATH], cmd[MAX_PATH + 16];
    CHECK(GetModuleFileNameW(NULL, self, MAX_PATH) > 0);
    swprintf(cmd, MAX_PATH + 16, L"\"%ls\" child", self);
    STARTUPINFOW si = {sizeof si};
    PROCESS_INFORMATION pi;
    CHECK(CreateProcessW(self, cmd, NULL, NULL, FALSE, 0, NULL, NULL, &si, &pi));
    HANDLE wait[2] = {ready, pi.hProcess};
    CHECK(WaitForMultipleObjects(2, wait, FALSE, 20000) == WAIT_OBJECT_0);

    IUIAutomationCondition *named = prop_str(uia, UIA_NamePropertyId, L"UIA Child");
    IUIAutomationElement *win = NULL;
    CHECK(IUIAutomationElement_FindFirst(root, TreeScope_Children, named, &win) == S_OK && win != NULL);
    CHECK(control_type(win) == UIA_WindowControlTypeId);
    CHECK(class_is(win, L"UiaChildWindow"));
    int pid = 0;
    CHECK(IUIAutomationElement_get_CurrentProcessId(win, &pid) == S_OK && pid == (int)pi.dwProcessId);

    // Invoke the button: the child's window gets BN_CLICKED.
    IUIAutomationCondition *id101 = prop_str(uia, UIA_AutomationIdPropertyId, L"101");
    IUIAutomationElement *press = NULL;
    CHECK(IUIAutomationElement_FindFirst(win, TreeScope_Descendants, id101, &press) == S_OK && press);
    CHECK(control_type(press) == UIA_ButtonControlTypeId && name_is(press, L"Press"));
    IUnknown *unk = NULL;
    CHECK(IUIAutomationElement_GetCurrentPattern(press, UIA_InvokePatternId, &unk) == S_OK && unk);
    IUIAutomationInvokePattern *invoke = NULL;
    CHECK(IUnknown_QueryInterface(unk, &IID_IUIAutomationInvokePattern, (void **)&invoke) == S_OK && invoke);
    IUnknown_Release(unk);
    CHECK(IUIAutomationInvokePattern_Invoke(invoke) == S_OK);
    CHECK(name_becomes(win, L"pressed"));
    IUIAutomationInvokePattern_Release(invoke);

    // Value: set and read the child's edit.
    IUIAutomationCondition *id102 = prop_str(uia, UIA_AutomationIdPropertyId, L"102");
    IUIAutomationElement *field = NULL;
    CHECK(IUIAutomationElement_FindFirst(win, TreeScope_Children, id102, &field) == S_OK && field);
    CHECK(control_type(field) == UIA_EditControlTypeId);
    CHECK(IUIAutomationElement_GetCurrentPattern(field, UIA_ValuePatternId, &unk) == S_OK && unk);
    IUIAutomationValuePattern *value = NULL;
    CHECK(IUnknown_QueryInterface(unk, &IID_IUIAutomationValuePattern, (void **)&value) == S_OK && value);
    IUnknown_Release(unk);
    BSTR remote = SysAllocString(L"remote");
    CHECK(IUIAutomationValuePattern_SetValue(value, remote) == S_OK);
    SysFreeString(remote);
    BSTR current = NULL;
    CHECK(IUIAutomationValuePattern_get_CurrentValue(value, &current) == S_OK && bstr_is(current, L"remote"));
    SysFreeString(current);
    IUIAutomationValuePattern_Release(value);

    // Toggle the child's check box.
    IUIAutomationCondition *id103 = prop_str(uia, UIA_AutomationIdPropertyId, L"103");
    IUIAutomationElement *box = NULL;
    CHECK(IUIAutomationElement_FindFirst(win, TreeScope_Children, id103, &box) == S_OK && box);
    CHECK(control_type(box) == UIA_CheckBoxControlTypeId);
    CHECK(IUIAutomationElement_GetCurrentPattern(box, UIA_TogglePatternId, &unk) == S_OK && unk);
    IUIAutomationTogglePattern *toggle = NULL;
    CHECK(IUnknown_QueryInterface(unk, &IID_IUIAutomationTogglePattern, (void **)&toggle) == S_OK && toggle);
    IUnknown_Release(unk);
    enum ToggleState state = ToggleState_Indeterminate;
    CHECK(IUIAutomationTogglePattern_get_CurrentToggleState(toggle, &state) == S_OK && state == ToggleState_Off);
    CHECK(IUIAutomationTogglePattern_Toggle(toggle) == S_OK);
    CHECK(IUIAutomationTogglePattern_get_CurrentToggleState(toggle, &state) == S_OK && state == ToggleState_On);
    IUIAutomationTogglePattern_Release(toggle);

    // The child's list view: items read and selected across processes.
    IUIAutomationCondition *id104 = prop_str(uia, UIA_AutomationIdPropertyId, L"104");
    IUIAutomationElement *list = NULL;
    CHECK(IUIAutomationElement_FindFirst(win, TreeScope_Children, id104, &list) == S_OK && list);
    const wchar_t *names[3] = {L"remote 0", L"remote 1", L"remote 2"};
    IUIAutomationElement *items[3];
    list_items(uia, list, 3, names, items, (int)pi.dwProcessId);
    select_item(items[2], 0);
    CHECK(SendMessageW(element_hwnd(list), LVM_GETNEXTITEM, -1, LVNI_SELECTED) == 2);
    CHECK(is_selected(items[2]) && !is_selected(items[1]));
    selection_is(uia, list, 1, items[2]);
    release_all(items, 3);
    IUIAutomationElement_Release(list);
    IUIAutomationCondition_Release(id104);
    // The child's list box.
    IUIAutomationCondition *id105 = prop_str(uia, UIA_AutomationIdPropertyId, L"105");
    CHECK(IUIAutomationElement_FindFirst(win, TreeScope_Children, id105, &list) == S_OK && list);
    const wchar_t *lb_names[2] = {L"r-one", L"r-two"};
    list_items(uia, list, 2, lb_names, items, (int)pi.dwProcessId);
    select_item(items[1], 0);
    CHECK(SendMessageW(element_hwnd(list), LB_GETCURSEL, 0, 0) == 1);
    CHECK(is_selected(items[1]) && !is_selected(items[0]));
    release_all(items, 2);
    IUIAutomationElement_Release(list);
    IUIAutomationCondition_Release(id105);

    IUIAutomationElement_Release(box);
    IUIAutomationCondition_Release(id103);
    IUIAutomationElement_Release(field);
    IUIAutomationCondition_Release(id102);
    IUIAutomationElement_Release(press);
    IUIAutomationCondition_Release(id101);
    IUIAutomationElement_Release(win);
    IUIAutomationCondition_Release(named);
    CHECK(SetEvent(done));
    CHECK(WaitForSingleObject(pi.hProcess, 20000) == WAIT_OBJECT_0);
}

int main(int argc, char **argv) {
    if (argc > 1 && strcmp(argv[1], "child") == 0)
        return child();
    CHECK(CoInitializeEx(NULL, COINIT_APARTMENTTHREADED) == S_OK);
    INITCOMMONCONTROLSEX icc = {sizeof icc, ICC_LISTVIEW_CLASSES};
    CHECK(InitCommonControlsEx(&icc));

    WNDCLASSW wc = {0};
    wc.lpfnWndProc = parent_proc;
    wc.hInstance = GetModuleHandleW(NULL);
    wc.lpszClassName = L"UiaTestWindow";
    wc.hbrBackground = (HBRUSH)(COLOR_WINDOW + 1);
    CHECK(RegisterClassW(&wc) != 0);
    HWND hwnd = CreateWindowExW(0, L"UiaTestWindow", L"UIA Test", WS_OVERLAPPED | WS_CAPTION | WS_VISIBLE, 100,
                                100, 400, 300, NULL, NULL, GetModuleHandleW(NULL), NULL);
    CHECK(hwnd != NULL);
    HWND button = CreateWindowExW(0, L"BUTTON", L"Press", WS_CHILD | WS_VISIBLE | WS_TABSTOP | BS_PUSHBUTTON, 10,
                                  10, 80, 24, hwnd, (HMENU)101, GetModuleHandleW(NULL), NULL);
    HWND edit = CreateWindowExW(0, L"EDIT", L"", WS_CHILD | WS_VISIBLE | WS_TABSTOP | WS_BORDER | ES_LEFT, 10, 40,
                                150, 22, hwnd, (HMENU)102, GetModuleHandleW(NULL), NULL);
    HWND label = CreateWindowExW(0, L"STATIC", L"Label", WS_CHILD | WS_VISIBLE | SS_LEFT, 10, 70, 100, 20, hwnd,
                                 (HMENU)103, GetModuleHandleW(NULL), NULL);
    CHECK(button != NULL && edit != NULL && label != NULL);
    // A second window with a check box (Toggle).
    HWND second = CreateWindowExW(0, L"UiaTestWindow", L"UIA Second", WS_OVERLAPPED | WS_CAPTION | WS_VISIBLE, 520,
                                  100, 300, 200, NULL, NULL, GetModuleHandleW(NULL), NULL);
    HWND box = CreateWindowExW(0, L"BUTTON", L"Option", WS_CHILD | WS_VISIBLE | WS_TABSTOP | BS_AUTOCHECKBOX, 10,
                               10, 120, 20, second, (HMENU)201, GetModuleHandleW(NULL), NULL);
    CHECK(second != NULL && box != NULL);
    UpdateWindow(hwnd);
    UpdateWindow(second);

    IUIAutomation *uia = NULL;
    CHECK(CoCreateInstance(&CLSID_CUIAutomation, NULL, CLSCTX_INPROC_SERVER, &IID_IUIAutomation, (void **)&uia) ==
              S_OK &&
          uia != NULL);

    // The window.
    IUIAutomationElement *win = NULL;
    CHECK(IUIAutomation_ElementFromHandle(uia, hwnd, &win) == S_OK && win != NULL);
    CHECK(name_is(win, L"UIA Test"));
    CHECK(control_type(win) == UIA_WindowControlTypeId);
    CHECK(element_hwnd(win) == hwnd);
    CHECK(class_is(win, L"UiaTestWindow"));
    int pid = 0;
    CHECK(IUIAutomationElement_get_CurrentProcessId(win, &pid) == S_OK && pid == (int)GetCurrentProcessId());

    // The desktop and its children.
    IUIAutomationElement *root = NULL;
    CHECK(IUIAutomation_GetRootElement(uia, &root) == S_OK && root != NULL);
    CHECK(control_type(root) == UIA_PaneControlTypeId);
    CHECK(element_hwnd(root) == GetDesktopWindow());
    IUIAutomationCondition *is_win = prop_int(uia, UIA_NativeWindowHandlePropertyId, (int)(INT_PTR)hwnd);
    IUIAutomationElement *found = NULL;
    CHECK(IUIAutomationElement_FindFirst(root, TreeScope_Children, is_win, &found) == S_OK && found != NULL);
    BOOL same = FALSE;
    CHECK(IUIAutomation_CompareElements(uia, found, win, &same) == S_OK && same);
    IUIAutomationElement_Release(found);
    IUIAutomationCondition_Release(is_win);

    // The button by AutomationId.
    IUIAutomationCondition *id101 = prop_str(uia, UIA_AutomationIdPropertyId, L"101");
    IUIAutomationElement *press = NULL;
    CHECK(IUIAutomationElement_FindFirst(win, TreeScope_Descendants, id101, &press) == S_OK && press != NULL);
    CHECK(control_type(press) == UIA_ButtonControlTypeId);
    CHECK(name_is(press, L"Press"));
    CHECK(class_is(press, L"Button"));
    CHECK(element_hwnd(press) == button);
    BSTR aid = NULL;
    CHECK(IUIAutomationElement_get_CurrentAutomationId(press, &aid) == S_OK && bstr_is(aid, L"101"));
    SysFreeString(aid);
    BOOL enabled = FALSE;
    CHECK(IUIAutomationElement_get_CurrentIsEnabled(press, &enabled) == S_OK && enabled);
    RECT ur, wr;
    CHECK(IUIAutomationElement_get_CurrentBoundingRectangle(press, &ur) == S_OK);
    GetWindowRect(button, &wr);
    CHECK(ur.left == wr.left && ur.top == wr.top && ur.right == wr.right && ur.bottom == wr.bottom);
    // Property values as VARIANTs.
    VARIANT v;
    VariantInit(&v);
    CHECK(IUIAutomationElement_GetCurrentPropertyValue(press, UIA_ControlTypePropertyId, &v) == S_OK &&
          v.vt == VT_I4 && v.lVal == UIA_ButtonControlTypeId);
    CHECK(IUIAutomationElement_GetCurrentPropertyValue(press, UIA_NamePropertyId, &v) == S_OK && v.vt == VT_BSTR &&
          bstr_is(v.bstrVal, L"Press"));
    VariantClear(&v);
    CHECK(IUIAutomationElement_GetCurrentPropertyValue(press, UIA_IsEnabledPropertyId, &v) == S_OK &&
          v.vt == VT_BOOL && v.boolVal == VARIANT_TRUE);
    // TreeScope_Element matches the element itself; a button has no
    // children.
    found = NULL;
    CHECK(IUIAutomationElement_FindFirst(press, TreeScope_Element, id101, &found) == S_OK && found != NULL);
    CHECK(IUIAutomation_CompareElements(uia, found, press, &same) == S_OK && same);
    IUIAutomationElement_Release(found);
    found = (IUIAutomationElement *)1;
    CHECK(IUIAutomationElement_FindFirst(press, TreeScope_Children, id101, &found) == S_OK && found == NULL);

    // Invoke: the parent gets WM_COMMAND BN_CLICKED.
    IUnknown *unk = NULL;
    CHECK(IUIAutomationElement_GetCurrentPattern(press, UIA_InvokePatternId, &unk) == S_OK && unk != NULL);
    IUIAutomationInvokePattern *invoke = NULL;
    CHECK(IUnknown_QueryInterface(unk, &IID_IUIAutomationInvokePattern, (void **)&invoke) == S_OK && invoke);
    IUnknown_Release(unk);
    commands = 0;
    CHECK(IUIAutomationInvokePattern_Invoke(invoke) == S_OK);
    pump_until(&commands, 0);
    CHECK(commands == 1 && last_command == MAKEWPARAM(101, BN_CLICKED) && last_command_from == (LPARAM)button);
    IUIAutomationInvokePattern_Release(invoke);

    // The edit by ControlType and ClassName (an and condition).
    IUIAutomationCondition *is_edit = prop_int(uia, UIA_ControlTypePropertyId, UIA_EditControlTypeId);
    IUIAutomationCondition *edit_class = prop_str(uia, UIA_ClassNamePropertyId, L"Edit");
    IUIAutomationCondition *both = NULL;
    CHECK(IUIAutomation_CreateAndCondition(uia, is_edit, edit_class, &both) == S_OK && both != NULL);
    IUIAutomationElement *field = NULL;
    CHECK(IUIAutomationElement_FindFirst(win, TreeScope_Descendants, both, &field) == S_OK && field != NULL);
    CHECK(element_hwnd(field) == edit);
    unk = (IUnknown *)1;
    CHECK(IUIAutomationElement_GetCurrentPattern(field, UIA_InvokePatternId, &unk) == S_OK && unk == NULL);
    CHECK(IUIAutomationElement_GetCurrentPattern(field, UIA_ValuePatternId, &unk) == S_OK && unk != NULL);
    IUIAutomationValuePattern *value = NULL;
    CHECK(IUnknown_QueryInterface(unk, &IID_IUIAutomationValuePattern, (void **)&value) == S_OK && value);
    IUnknown_Release(unk);
    BSTR hello = SysAllocString(L"hello");
    CHECK(IUIAutomationValuePattern_SetValue(value, hello) == S_OK);
    SysFreeString(hello);
    wchar_t text[32] = {0};
    CHECK(GetWindowTextW(edit, text, 32) == 5 && wcscmp(text, L"hello") == 0);
    BSTR current = NULL;
    CHECK(IUIAutomationValuePattern_get_CurrentValue(value, &current) == S_OK && bstr_is(current, L"hello"));
    SysFreeString(current);
    BOOL read_only = TRUE;
    CHECK(IUIAutomationValuePattern_get_CurrentIsReadOnly(value, &read_only) == S_OK && !read_only);
    CHECK(IUIAutomationElement_GetCurrentPropertyValue(field, UIA_ValueValuePropertyId, &v) == S_OK &&
          v.vt == VT_BSTR && bstr_is(v.bstrVal, L"hello"));
    VariantClear(&v);
    IUIAutomationValuePattern_Release(value);

    // The static by Name.
    IUIAutomationCondition *named = prop_str(uia, UIA_NamePropertyId, L"Label");
    IUIAutomationElement *text_el = NULL;
    CHECK(IUIAutomationElement_FindFirst(win, TreeScope_Children, named, &text_el) == S_OK && text_el != NULL);
    CHECK(control_type(text_el) == UIA_TextControlTypeId && element_hwnd(text_el) == label);
    CHECK(class_is(text_el, L"Static"));

    // The children: all of them (true condition), and those with a window
    // of their own (not NativeWindowHandle == 0), in z order.
    IUIAutomationCondition *all = NULL;
    CHECK(IUIAutomation_CreateTrueCondition(uia, &all) == S_OK && all != NULL);
    IUIAutomationElementArray *list = NULL;
    int n = 0;
    CHECK(IUIAutomationElement_FindAll(win, TreeScope_Children, all, &list) == S_OK && list != NULL);
    CHECK(IUIAutomationElementArray_get_Length(list, &n) == S_OK && n >= 3);
    IUIAutomationElementArray_Release(list);
    IUIAutomationCondition *no_hwnd = prop_int(uia, UIA_NativeWindowHandlePropertyId, 0);
    IUIAutomationCondition *has_hwnd = NULL;
    CHECK(IUIAutomation_CreateNotCondition(uia, no_hwnd, &has_hwnd) == S_OK && has_hwnd != NULL);
    CHECK(IUIAutomationElement_FindAll(win, TreeScope_Children, has_hwnd, &list) == S_OK && list != NULL);
    CHECK(IUIAutomationElementArray_get_Length(list, &n) == S_OK && n == 3);
    HWND order[3] = {button, edit, label};
    for (int i = 0; i < 3; i++) {
        IUIAutomationElement *e = NULL;
        CHECK(IUIAutomationElementArray_GetElement(list, i, &e) == S_OK && e != NULL);
        CHECK(element_hwnd(e) == order[i]);
        IUIAutomationElement_Release(e);
    }
    IUIAutomationElement *e = (IUIAutomationElement *)1;
    CHECK(FAILED(IUIAutomationElementArray_GetElement(list, 3, &e)));
    IUIAutomationElementArray_Release(list);

    // An or condition; the false condition finds nothing.
    IUIAutomationCondition *id102 = prop_str(uia, UIA_AutomationIdPropertyId, L"102");
    IUIAutomationCondition *either = NULL;
    CHECK(IUIAutomation_CreateOrCondition(uia, id101, id102, &either) == S_OK && either != NULL);
    CHECK(IUIAutomationElement_FindAll(win, SUBTREE, either, &list) == S_OK && list != NULL);
    CHECK(IUIAutomationElementArray_get_Length(list, &n) == S_OK && n == 2);
    IUIAutomationElementArray_Release(list);
    IUIAutomationCondition *none = NULL;
    CHECK(IUIAutomation_CreateFalseCondition(uia, &none) == S_OK && none != NULL);
    found = (IUIAutomationElement *)1;
    CHECK(IUIAutomationElement_FindFirst(win, SUBTREE, none, &found) == S_OK && found == NULL);

    // Toggle on a check box.
    IUIAutomationElement *win2 = NULL;
    CHECK(IUIAutomation_ElementFromHandle(uia, second, &win2) == S_OK && win2 != NULL);
    IUIAutomationCondition *id201 = prop_str(uia, UIA_AutomationIdPropertyId, L"201");
    IUIAutomationElement *option = NULL;
    CHECK(IUIAutomationElement_FindFirst(win2, TreeScope_Children, id201, &option) == S_OK && option != NULL);
    CHECK(control_type(option) == UIA_CheckBoxControlTypeId && name_is(option, L"Option"));
    CHECK(IUIAutomationElement_GetCurrentPattern(option, UIA_TogglePatternId, &unk) == S_OK && unk != NULL);
    IUIAutomationTogglePattern *toggle = NULL;
    CHECK(IUnknown_QueryInterface(unk, &IID_IUIAutomationTogglePattern, (void **)&toggle) == S_OK && toggle);
    IUnknown_Release(unk);
    enum ToggleState state = ToggleState_Indeterminate;
    CHECK(IUIAutomationTogglePattern_get_CurrentToggleState(toggle, &state) == S_OK && state == ToggleState_Off);
    CHECK(IUIAutomationTogglePattern_Toggle(toggle) == S_OK);
    volatile int checked = 0;
    DWORD start = GetTickCount();
    while (!(checked = SendMessageW(box, BM_GETCHECK, 0, 0) == BST_CHECKED) && GetTickCount() - start < 5000) {
        MSG m;
        while (PeekMessageW(&m, NULL, 0, 0, PM_REMOVE)) {
            TranslateMessage(&m);
            DispatchMessageW(&m);
        }
        Sleep(10);
    }
    CHECK(checked);
    CHECK(IUIAutomationTogglePattern_get_CurrentToggleState(toggle, &state) == S_OK && state == ToggleState_On);
    IUIAutomationTogglePattern_Release(toggle);

    IUIAutomationElement_Release(option);
    IUIAutomationCondition_Release(id201);
    IUIAutomationElement_Release(win2);
    IUIAutomationCondition_Release(none);
    IUIAutomationCondition_Release(either);
    IUIAutomationCondition_Release(id102);
    IUIAutomationCondition_Release(has_hwnd);
    IUIAutomationCondition_Release(no_hwnd);
    IUIAutomationCondition_Release(all);
    IUIAutomationElement_Release(text_el);
    IUIAutomationCondition_Release(named);
    IUIAutomationElement_Release(field);
    IUIAutomationCondition_Release(both);
    IUIAutomationCondition_Release(edit_class);
    IUIAutomationCondition_Release(is_edit);
    IUIAutomationElement_Release(press);
    IUIAutomationCondition_Release(id101);
    lists(uia);
    other_process(uia, root);
    IUIAutomationElement_Release(root);
    IUIAutomationElement_Release(win);
    IUIAutomation_Release(uia);
    DestroyWindow(second);
    DestroyWindow(hwnd);
    CoUninitialize();
    printf("uia_test: ok\n");
    return 0;
}
