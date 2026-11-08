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
 * - Owned windows: the child's "UIA Owned" (WS_POPUP | WS_CAPTION, its
 *   main window as owner, as a program's dialog) is a child of its owner
 *   in the UIA tree, not of the root (UIA places owned top-level windows
 *   under their owner; FlaUI's Window.ModalWindows and its Win32 menu
 *   handling rely on it): FindFirst from the root's children does not
 *   find it, from the owner's children does; its parent is the owner;
 *   its "OK" button is found by Name below it.
 * - Menus (UIA's Win32 menu proxy): the child's window "UIA Menus" has a
 *   menu bar (&File: "&Open\tCtrl+O", a separator, "E&xit"). Its MenuBar
 *   child holds one MenuItem "File" (mnemonic removed) with the
 *   ExpandCollapse pattern, Collapsed; Expand opens the menu (as a click
 *   does): the item becomes Expanded and a Menu element named "File" (the
 *   popup, a window of class #32768 owned by the menu's window) appears
 *   under the window, its MenuItems "Open" (AcceleratorKey "Ctrl+O") and
 *   "Exit"; Invoke on Exit chooses it (the window gets WM_COMMAND 202 and
 *   retitles itself "exit chosen"), the menu closes and File is Collapsed
 *   again.
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
 * - Title bars (Windows' Win32 window proxy): a top-level window with a
 *   caption has a TitleBar child last, after its child windows (no HWND
 *   of its own, an empty Name; Windows 11 in process): "UIA Test"
 *   (WS_CAPTION, no system menu) has four children, the three controls
 *   and then the TitleBar (without buttons). A WS_OVERLAPPEDWINDOW window's title bar has three
 *   Buttons, AutomationId and Name "Minimize", "Maximize" ("Restore"
 *   while maximized), "Close" (what FlaUI's TitleBar looks them up by;
 *   unverified on Windows); a WS_SYSMENU dialog's only "Close".
 * - Window and Transform patterns (framed windows of this process and of
 *   the child's): CanMaximize/CanMinimize (the styles), IsModal (FALSE;
 *   TRUE for a window owned by a disabled window, which FindAll with an
 *   IsModal condition finds below the owner, as FlaUI's
 *   Window.ModalWindows does; the owner is BlockedByModalWindow),
 *   IsTopmost (WS_EX_TOPMOST), WindowVisualState, WaitForInputIdle;
 *   Transform.Move and Resize change GetWindowRect (and
 *   BoundingRectangle), Rotate fails (CanRotate FALSE).
 *   SetWindowVisualState Maximized: IsZoomed, another rectangle, the
 *   title bar's Restore button, CanMove FALSE; Normal: the rectangle
 *   back; Minimized: IsIconic (any process's window). The title bar's
 *   Maximize, Restore and Minimize buttons do the same through Invoke.
 *   Window.Close (or Invoke on the title bar's Close button): the window
 *   gets WM_CLOSE (counted; in the child signalled by an event) and is
 *   destroyed (IsWindow FALSE); its pattern then reports
 *   UIA_E_ELEMENTNOTAVAILABLE. Maximized on a window without
 *   WS_MAXIMIZEBOX is UIA_E_INVALIDOPERATION.
 * - Providers (a window's WM_GETOBJECT answer, UiaRootObjectId, through
 *   UiaReturnRawElementProvider, in this process): the window "UIA
 *   Provided" answers with its own IRawElementProviderSimple, whose host
 *   provider is UiaHostProviderFromHwnd(its window). ElementFromHandle
 *   gives the provider's Name ("Provided"), AutomationId ("provided-id")
 *   and ControlType (Button) instead of the window's, and the host's
 *   (the window's) ClassName, NativeWindowHandle and ProcessId where the
 *   provider gives none (VT_EMPTY); the provider was asked. FindFirst
 *   under the desktop by AutomationId "provided-id" finds the window.
 *   The same across processes: the child's provided window is found by
 *   its provider's AutomationId ("remote-provided-id") and gives its
 *   provider's Name and ControlType and the window's ClassName, handle
 *   and ProcessId (the child's).
 * - Fragments (IRawElementProviderFragment, FragmentRoot): the provided
 *   window's provider is a fragment root with two children, "Fragment
 *   A" (Text) and "Fragment B" (Hyperlink). They are the window's
 *   element's children (FindFirst by AutomationId "frag-b"; the raw
 *   walker's first, next, last child), with the provider's rectangles
 *   (BoundingRectangle), no window handle, distinct runtime ids, and the
 *   window as parent; in this process and in the child's window. Their
 *   patterns are the provider's (GetPatternProvider): Fragment B's
 *   Invoke (IsInvokePatternAvailable; the window is retitled "fragment
 *   invoked"), Fragment A's Value ("start", set to "typed" and read
 *   back, not read-only); A has no Invoke, B no Value.
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

/* An empty Name (NULL or ""; a client may see either). */
static int name_empty(IUIAutomationElement *e) {
    BSTR b = NULL;
    CHECK(IUIAutomationElement_get_CurrentName(e, &b) == S_OK);
    int empty = b == NULL || SysStringLen(b) == 0;
    SysFreeString(b);
    return empty;
}

static int class_is(IUIAutomationElement *e, const wchar_t *s) {
    BSTR b = NULL;
    CHECK(IUIAutomationElement_get_CurrentClassName(e, &b) == S_OK);
    int same = bstr_is(b, s);
    SysFreeString(b);
    return same;
}

/* Framed windows (WS_OVERLAPPEDWINDOW) for the Window and Transform
 * patterns and title bars: their WM_CLOSE is counted (and, in the child
 * process, signalled with the event "UiaFrameClosed") before
 * DefWindowProc destroys them. */
static volatile int frame_closes;
static HANDLE frame_closed;
static LRESULT CALLBACK frame_proc(HWND hwnd, UINT msg, WPARAM wparam, LPARAM lparam) {
    if (msg == WM_CLOSE) {
        frame_closes++;
        if (frame_closed)
            SetEvent(frame_closed);
    }
    return DefWindowProcW(hwnd, msg, wparam, lparam);
}

static ATOM register_frame_class(void) {
    WNDCLASSW wc = {0};
    wc.lpfnWndProc = frame_proc;
    wc.hInstance = GetModuleHandleW(NULL);
    wc.lpszClassName = L"UiaFrameWindow";
    return RegisterClassW(&wc);
}

static HWND make_frame(const wchar_t *title, int x, int y, HWND owner, DWORD style) {
    return CreateWindowExW(0, L"UiaFrameWindow", title, style | WS_VISIBLE, x, y, 320, 240, owner, NULL,
                           GetModuleHandleW(NULL), NULL);
}

/* Dispatches this thread's messages once, then sleeps 10 ms. */
static void pump_once(void) {
    MSG m;
    while (PeekMessageW(&m, NULL, 0, 0, PM_REMOVE)) {
        TranslateMessage(&m);
        DispatchMessageW(&m);
    }
    Sleep(10);
}

/* Waits (pumping messages, up to 5 s) for `cond`. */
#define WAIT(cond)                                                                                            \
    do {                                                                                                      \
        DWORD t0_ = GetTickCount();                                                                           \
        while (!(cond) && GetTickCount() - t0_ < 5000)                                                        \
            pump_once();                                                                                      \
    } while (0)

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

/* ---- A provider (WM_GETOBJECT), in this process and in the child ---- */

static HWND provided_hwnd;
/* What the provider answers (the child's differ: both windows are top-level). */
static const wchar_t *provided_name = L"Provided", *provided_id = L"provided-id";
static volatile LONG provider_refs = 1, provider_asked;

/* The provider is a fragment root (frags[0], the window's provider) with
 * two child fragments (frags[1], frags[2]); each object answers
 * IRawElementProviderSimple, IRawElementProviderFragment and (the root)
 * IRawElementProviderFragmentRoot. Static objects: references are
 * counted in one counter. */
typedef struct Frag {
    IRawElementProviderSimple simple;
    IRawElementProviderFragment frag;
    IRawElementProviderFragmentRoot root;
    IInvokeProvider invoke;  /* Fragment B's */
    IValueProvider value;    /* Fragment A's */
    int index;
} Frag;
/* Fragment A's value (the Value pattern's). */
static WCHAR frag_value[64] = L"start";
static Frag frags[3];

#define FRAG_OF(p, member) ((Frag *)((char *)(p) - offsetof(Frag, member)))

static HRESULT frag_qi(Frag *f, REFIID riid, void **out) {
    if (IsEqualIID(riid, &IID_IUnknown) || IsEqualIID(riid, &IID_IRawElementProviderSimple))
        *out = &f->simple;
    else if (IsEqualIID(riid, &IID_IRawElementProviderFragment))
        *out = &f->frag;
    else if (IsEqualIID(riid, &IID_IRawElementProviderFragmentRoot) && f->index == 0)
        *out = &f->root;
    else {
        *out = NULL;
        return E_NOINTERFACE;
    }
    InterlockedIncrement(&provider_refs);
    return S_OK;
}
static ULONG STDMETHODCALLTYPE frag_addref(void *This) {
    (void)This;
    return (ULONG)InterlockedIncrement(&provider_refs);
}
static ULONG STDMETHODCALLTYPE frag_release(void *This) {
    (void)This;
    return (ULONG)InterlockedDecrement(&provider_refs);
}

/* A child fragment's rectangle: 50x20 at (10 + 60 * (i - 1), 30) from the
 * window's corner. */
static RECT frag_rect(int i) {
    RECT w;
    GetWindowRect(provided_hwnd, &w);
    RECT r = { w.left + 10 + 60 * (i - 1), w.top + 30, 0, 0 };
    r.right = r.left + 50;
    r.bottom = r.top + 20;
    return r;
}

static HRESULT STDMETHODCALLTYPE prov_qi(IRawElementProviderSimple *This, REFIID riid, void **out) {
    return frag_qi(FRAG_OF(This, simple), riid, out);
}
static HRESULT STDMETHODCALLTYPE prov_options(IRawElementProviderSimple *This, enum ProviderOptions *out) {
    (void)This;
    *out = ProviderOptions_ServerSideProvider;
    return S_OK;
}
static HRESULT STDMETHODCALLTYPE prov_pattern(IRawElementProviderSimple *This, PATTERNID id, IUnknown **out) {
    Frag *f = FRAG_OF(This, simple);
    *out = NULL;
    if (f->index == 2 && id == UIA_InvokePatternId)
        *out = (IUnknown *)&f->invoke;
    else if (f->index == 1 && id == UIA_ValuePatternId)
        *out = (IUnknown *)&f->value;
    if (*out)
        InterlockedIncrement(&provider_refs);
    return S_OK;
}
static HRESULT STDMETHODCALLTYPE prov_property(IRawElementProviderSimple *This, PROPERTYID id, VARIANT *out) {
    int i = FRAG_OF(This, simple)->index;
    static const wchar_t *names[3] = { NULL, L"Fragment A", L"Fragment B" };
    static const wchar_t *ids[3] = { NULL, L"frag-a", L"frag-b" };
    InterlockedIncrement(&provider_asked);
    VariantInit(out);
    if (id == UIA_NamePropertyId) {
        out->vt = VT_BSTR;
        out->bstrVal = SysAllocString(i == 0 ? provided_name : names[i]);
    } else if (id == UIA_AutomationIdPropertyId) {
        out->vt = VT_BSTR;
        out->bstrVal = SysAllocString(i == 0 ? provided_id : ids[i]);
    } else if (id == UIA_ControlTypePropertyId) {
        out->vt = VT_I4;
        out->lVal = i == 0 ? UIA_ButtonControlTypeId : i == 1 ? UIA_TextControlTypeId : UIA_HyperlinkControlTypeId;
    }
    return S_OK;
}
static HRESULT STDMETHODCALLTYPE prov_host(IRawElementProviderSimple *This, IRawElementProviderSimple **out) {
    if (FRAG_OF(This, simple)->index != 0) {
        *out = NULL;
        return S_OK;
    }
    return UiaHostProviderFromHwnd(provided_hwnd, out);
}
static IRawElementProviderSimpleVtbl provider_vtbl = {
    prov_qi, (void *)frag_addref, (void *)frag_release, prov_options, prov_pattern, prov_property, prov_host,
};

static HRESULT STDMETHODCALLTYPE fr_qi(IRawElementProviderFragment *This, REFIID riid, void **out) {
    return frag_qi(FRAG_OF(This, frag), riid, out);
}
static HRESULT STDMETHODCALLTYPE fr_navigate(IRawElementProviderFragment *This, enum NavigateDirection d,
                                             IRawElementProviderFragment **out) {
    int i = FRAG_OF(This, frag)->index, to = -1;
    if (i == 0)
        to = d == NavigateDirection_FirstChild ? 1 : d == NavigateDirection_LastChild ? 2 : -1;
    else if (d == NavigateDirection_Parent)
        to = 0;
    else if (d == NavigateDirection_NextSibling && i == 1)
        to = 2;
    else if (d == NavigateDirection_PreviousSibling && i == 2)
        to = 1;
    *out = NULL;
    if (to >= 0) {
        *out = &frags[to].frag;
        InterlockedIncrement(&provider_refs);
    }
    return S_OK;
}
static HRESULT STDMETHODCALLTYPE fr_runtime_id(IRawElementProviderFragment *This, SAFEARRAY **out) {
    int i = FRAG_OF(This, frag)->index;
    *out = NULL;
    if (i == 0) /* The root's: its host's (the window's). */
        return S_OK;
    SAFEARRAY *sa = SafeArrayCreateVector(VT_I4, 0, 2);
    if (!sa)
        return E_OUTOFMEMORY;
    LONG at = 0, v = UiaAppendRuntimeId;
    SafeArrayPutElement(sa, &at, &v);
    at = 1;
    v = i;
    SafeArrayPutElement(sa, &at, &v);
    *out = sa;
    return S_OK;
}
static HRESULT STDMETHODCALLTYPE fr_rect(IRawElementProviderFragment *This, struct UiaRect *out) {
    int i = FRAG_OF(This, frag)->index;
    memset(out, 0, sizeof *out);
    if (i == 0) /* The host's. */
        return S_OK;
    RECT r = frag_rect(i);
    out->left = r.left;
    out->top = r.top;
    out->width = r.right - r.left;
    out->height = r.bottom - r.top;
    return S_OK;
}
static HRESULT STDMETHODCALLTYPE fr_embedded(IRawElementProviderFragment *This, SAFEARRAY **out) {
    (void)This;
    *out = NULL;
    return S_OK;
}
static HRESULT STDMETHODCALLTYPE fr_focus(IRawElementProviderFragment *This) {
    (void)This;
    return S_OK;
}
static HRESULT STDMETHODCALLTYPE fr_root(IRawElementProviderFragment *This, IRawElementProviderFragmentRoot **out) {
    (void)This;
    *out = &frags[0].root;
    InterlockedIncrement(&provider_refs);
    return S_OK;
}
static IRawElementProviderFragmentVtbl fragment_vtbl = {
    fr_qi,         (void *)frag_addref, (void *)frag_release, fr_navigate, fr_runtime_id,
    fr_rect,       fr_embedded,         fr_focus,             fr_root,
};

static HRESULT STDMETHODCALLTYPE rt_qi(IRawElementProviderFragmentRoot *This, REFIID riid, void **out) {
    return frag_qi(FRAG_OF(This, root), riid, out);
}
static HRESULT STDMETHODCALLTYPE rt_from_point(IRawElementProviderFragmentRoot *This, double x, double y,
                                               IRawElementProviderFragment **out) {
    (void)This;
    *out = NULL;
    for (int i = 1; i <= 2; i++) {
        RECT r = frag_rect(i);
        if (x >= r.left && x < r.right && y >= r.top && y < r.bottom) {
            *out = &frags[i].frag;
            InterlockedIncrement(&provider_refs);
        }
    }
    return S_OK;
}
static HRESULT STDMETHODCALLTYPE rt_focus(IRawElementProviderFragmentRoot *This, IRawElementProviderFragment **out) {
    (void)This;
    *out = NULL;
    return S_OK;
}
static IRawElementProviderFragmentRootVtbl root_vtbl = {
    rt_qi, (void *)frag_addref, (void *)frag_release, rt_from_point, rt_focus,
};

/* Fragment B's Invoke: the window is retitled "fragment invoked". */
static HRESULT STDMETHODCALLTYPE inv_qi(IInvokeProvider *This, REFIID riid, void **out) {
    if (IsEqualIID(riid, &IID_IUnknown) || IsEqualIID(riid, &IID_IInvokeProvider)) {
        *out = This;
        InterlockedIncrement(&provider_refs);
        return S_OK;
    }
    *out = NULL;
    return E_NOINTERFACE;
}
static HRESULT STDMETHODCALLTYPE inv_invoke(IInvokeProvider *This) {
    (void)This;
    SetWindowTextW(provided_hwnd, L"fragment invoked");
    return S_OK;
}
static IInvokeProviderVtbl invoke_vtbl = { inv_qi, (void *)frag_addref, (void *)frag_release, inv_invoke };

/* Fragment A's Value: frag_value, writable. */
static HRESULT STDMETHODCALLTYPE val_qi(IValueProvider *This, REFIID riid, void **out) {
    if (IsEqualIID(riid, &IID_IUnknown) || IsEqualIID(riid, &IID_IValueProvider)) {
        *out = This;
        InterlockedIncrement(&provider_refs);
        return S_OK;
    }
    *out = NULL;
    return E_NOINTERFACE;
}
static HRESULT STDMETHODCALLTYPE val_set(IValueProvider *This, LPCWSTR v) {
    (void)This;
    lstrcpynW(frag_value, v, 64);
    return S_OK;
}
static HRESULT STDMETHODCALLTYPE val_get(IValueProvider *This, BSTR *out) {
    (void)This;
    *out = SysAllocString(frag_value);
    return *out ? S_OK : E_OUTOFMEMORY;
}
static HRESULT STDMETHODCALLTYPE val_readonly(IValueProvider *This, BOOL *out) {
    (void)This;
    *out = FALSE;
    return S_OK;
}
static IValueProviderVtbl value_vtbl = {
    val_qi, (void *)frag_addref, (void *)frag_release, val_set, val_get, val_readonly,
};

static void init_frags(void) {
    for (int i = 0; i < 3; i++) {
        frags[i].simple.lpVtbl = &provider_vtbl;
        frags[i].frag.lpVtbl = &fragment_vtbl;
        frags[i].root.lpVtbl = &root_vtbl;
        frags[i].invoke.lpVtbl = &invoke_vtbl;
        frags[i].value.lpVtbl = &value_vtbl;
        frags[i].index = i;
    }
}

static LRESULT CALLBACK provided_proc(HWND hwnd, UINT msg, WPARAM wparam, LPARAM lparam) {
    if (msg == WM_GETOBJECT && (LONG)lparam == UiaRootObjectId)
        return UiaReturnRawElementProvider(hwnd, wparam, lparam, &frags[0].simple);
    return DefWindowProcW(hwnd, msg, wparam, lparam);
}

/* The provided window, titled `title`: no caption, so its children are
 * only its provider's fragments (a frame's title bar comes first). */
static HWND make_provided(const wchar_t *title) {
    init_frags();
    WNDCLASSW wc = { 0 };
    wc.lpfnWndProc = provided_proc;
    wc.hInstance = GetModuleHandleW(NULL);
    wc.lpszClassName = L"UiaProvided";
    if (!RegisterClassW(&wc))
        return NULL;
    provided_hwnd = CreateWindowExW(0, L"UiaProvided", title, WS_POPUP | WS_BORDER | WS_VISIBLE, 40, 40, 200, 100,
                                    NULL, NULL, wc.hInstance, NULL);
    return provided_hwnd;
}

static LRESULT CALLBACK child_proc(HWND hwnd, UINT msg, WPARAM wparam, LPARAM lparam) {
    if (msg == WM_COMMAND && LOWORD(wparam) == 101 && HIWORD(wparam) == BN_CLICKED) {
        SetWindowTextW(hwnd, L"pressed");
        return 0;
    }
    if (msg == WM_COMMAND && LOWORD(wparam) == 202 && HIWORD(wparam) == 0) {
        SetWindowTextW(hwnd, L"exit chosen");
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
    HWND owned = CreateWindowExW(0, L"UiaChildWindow", L"UIA Owned", WS_POPUP | WS_CAPTION | WS_VISIBLE, 260,
                                 200, 200, 100, top, NULL, wc.hInstance, NULL);
    if (!owned || !CreateWindowExW(0, L"BUTTON", L"OK", WS_CHILD | WS_VISIBLE | BS_PUSHBUTTON, 10, 10, 80, 24,
                                   owned, (HMENU)1, wc.hInstance, NULL))
        return 5;
    HMENU bar = CreateMenu(), file = CreatePopupMenu();
    AppendMenuW(file, MF_STRING, 201, L"&Open\tCtrl+O");
    AppendMenuW(file, MF_SEPARATOR, 0, NULL);
    AppendMenuW(file, MF_STRING, 202, L"E&xit");
    AppendMenuW(bar, MF_POPUP, (UINT_PTR)file, L"&File");
    if (!CreateWindowExW(0, L"UiaChildWindow", L"UIA Menus", WS_OVERLAPPED | WS_CAPTION | WS_VISIBLE, 520, 150,
                         300, 200, NULL, bar, wc.hInstance, NULL))
        return 6;
    SendMessageW(lb, LB_ADDSTRING, 0, (LPARAM)L"r-one");
    SendMessageW(lb, LB_ADDSTRING, 0, (LPARAM)L"r-two");
    // Framed windows for the Window and Transform patterns.
    frame_closed = OpenEventW(EVENT_MODIFY_STATE, FALSE, L"UiaFrameClosed");
    if (!frame_closed || !register_frame_class() || !make_frame(L"UIA Frame", 300, 300, NULL, WS_OVERLAPPEDWINDOW) ||
        !make_frame(L"UIA Closable", 340, 340, NULL, WS_OVERLAPPEDWINDOW))
        return 7;
    provided_name = L"Remote provided";
    provided_id = L"remote-provided-id";
    if (!make_provided(L"UIA Remote Provided"))
        return 8;
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

static enum ExpandCollapseState expand_state(IUIAutomationExpandCollapsePattern *p) {
    enum ExpandCollapseState state = ExpandCollapseState_LeafNode;
    CHECK(IUIAutomationExpandCollapsePattern_get_CurrentExpandCollapseState(p, &state) == S_OK);
    return state;
}

static int state_becomes(IUIAutomationExpandCollapsePattern *p, enum ExpandCollapseState want) {
    for (int i = 0; i < 500; i++) {
        if (expand_state(p) == want)
            return 1;
        Sleep(10);
    }
    return 0;
}

/* The child's menu bar: File expanded, Exit invoked. */
static void menus(IUIAutomation *uia, IUIAutomationElement *root) {
    IUIAutomationCondition *named = prop_str(uia, UIA_NamePropertyId, L"UIA Menus");
    IUIAutomationElement *win = NULL;
    CHECK(IUIAutomationElement_FindFirst(root, TreeScope_Children, named, &win) == S_OK && win);
    IUIAutomationCondition *is_bar = prop_int(uia, UIA_ControlTypePropertyId, UIA_MenuBarControlTypeId);
    IUIAutomationElement *bar = NULL;
    CHECK(IUIAutomationElement_FindFirst(win, TreeScope_Children, is_bar, &bar) == S_OK && bar);
    IUIAutomationCondition *is_item = prop_int(uia, UIA_ControlTypePropertyId, UIA_MenuItemControlTypeId);
    IUIAutomationElementArray *bar_items = NULL;
    CHECK(IUIAutomationElement_FindAll(bar, TreeScope_Children, is_item, &bar_items) == S_OK && bar_items);
    int n = 0;
    CHECK(IUIAutomationElementArray_get_Length(bar_items, &n) == S_OK && n == 1);
    IUIAutomationElement *file = NULL;
    CHECK(IUIAutomationElementArray_GetElement(bar_items, 0, &file) == S_OK && file);
    CHECK(name_is(file, L"File"));
    IUIAutomationExpandCollapsePattern *ec = (IUIAutomationExpandCollapsePattern *)pattern_of(
        file, UIA_ExpandCollapsePatternId, &IID_IUIAutomationExpandCollapsePattern);
    CHECK(ec != NULL);
    CHECK(expand_state(ec) == ExpandCollapseState_Collapsed);
    CHECK(IUIAutomationExpandCollapsePattern_Expand(ec) == S_OK);
    CHECK(state_becomes(ec, ExpandCollapseState_Expanded));

    // The popup: a Menu named "File" under the window.
    IUIAutomationCondition *is_menu = prop_int(uia, UIA_ControlTypePropertyId, UIA_MenuControlTypeId);
    IUIAutomationElement *menu = NULL;
    for (int i = 0; i < 500 && !menu; i++) {
        CHECK(IUIAutomationElement_FindFirst(win, TreeScope_Children, is_menu, &menu) == S_OK);
        if (!menu)
            Sleep(10);
    }
    CHECK(menu != NULL);
    CHECK(name_is(menu, L"File"));
    CHECK(class_is(menu, L"#32768"));
    IUIAutomationCondition *open_name = prop_str(uia, UIA_NamePropertyId, L"Open");
    IUIAutomationElement *open = NULL;
    CHECK(IUIAutomationElement_FindFirst(menu, TreeScope_Children, open_name, &open) == S_OK && open);
    CHECK(control_type(open) == UIA_MenuItemControlTypeId);
    BSTR accel = NULL;
    CHECK(IUIAutomationElement_get_CurrentAcceleratorKey(open, &accel) == S_OK && bstr_is(accel, L"Ctrl+O"));
    SysFreeString(accel);
    IUIAutomationCondition *exit_name = prop_str(uia, UIA_NamePropertyId, L"Exit");
    IUIAutomationElement *exit_item = NULL;
    CHECK(IUIAutomationElement_FindFirst(menu, TreeScope_Children, exit_name, &exit_item) == S_OK && exit_item);
    CHECK(control_type(exit_item) == UIA_MenuItemControlTypeId);
    IUIAutomationInvokePattern *invoke = (IUIAutomationInvokePattern *)pattern_of(
        exit_item, UIA_InvokePatternId, &IID_IUIAutomationInvokePattern);
    CHECK(invoke != NULL);
    CHECK(IUIAutomationInvokePattern_Invoke(invoke) == S_OK);
    CHECK(name_becomes(win, L"exit chosen"));
    CHECK(state_becomes(ec, ExpandCollapseState_Collapsed));

    IUIAutomationInvokePattern_Release(invoke);
    IUIAutomationElement_Release(exit_item);
    IUIAutomationCondition_Release(exit_name);
    IUIAutomationElement_Release(open);
    IUIAutomationCondition_Release(open_name);
    IUIAutomationElement_Release(menu);
    IUIAutomationCondition_Release(is_menu);
    IUIAutomationExpandCollapsePattern_Release(ec);
    IUIAutomationElement_Release(file);
    IUIAutomationElementArray_Release(bar_items);
    IUIAutomationCondition_Release(is_item);
    IUIAutomationElement_Release(bar);
    IUIAutomationCondition_Release(is_bar);
    IUIAutomationElement_Release(win);
    IUIAutomationCondition_Release(named);
}

static IUIAutomationWindowPattern *window_pattern(IUIAutomationElement *e) {
    return (IUIAutomationWindowPattern *)pattern_of(e, UIA_WindowPatternId, &IID_IUIAutomationWindowPattern);
}

static enum WindowVisualState visual_state(IUIAutomationWindowPattern *w) {
    enum WindowVisualState state = (enum WindowVisualState)-1;
    CHECK(IUIAutomationWindowPattern_get_CurrentWindowVisualState(w, &state) == S_OK);
    return state;
}

static int automation_id_is(IUIAutomationElement *e, const wchar_t *s) {
    BSTR b = NULL;
    CHECK(IUIAutomationElement_get_CurrentAutomationId(e, &b) == S_OK);
    int same = bstr_is(b, s);
    SysFreeString(b);
    return same;
}

static int rect_is(HWND h, int x, int y, int w, int hgt) {
    RECT r;
    return GetWindowRect(h, &r) && r.left == x && r.top == y && r.right - r.left == w && r.bottom - r.top == hgt;
}

/* The title bar of `win` (titled `title`, which the bar does not carry):
 * its last child, a TitleBar without a window and with an empty Name; its
 * buttons (Minimize, Maximize, Close) in `buttons`. */
static IUIAutomationElement *title_bar(IUIAutomation *uia, IUIAutomationElement *win, const wchar_t *title,
                                       IUIAutomationElement *buttons[3]) {
    IUIAutomationCondition *is_bar = prop_int(uia, UIA_ControlTypePropertyId, UIA_TitleBarControlTypeId);
    IUIAutomationElement *bar = NULL;
    CHECK(IUIAutomationElement_FindFirst(win, TreeScope_Children, is_bar, &bar) == S_OK && bar);
    IUIAutomationCondition_Release(is_bar);
    IUIAutomationTreeWalker *raw = NULL;
    CHECK(IUIAutomation_get_RawViewWalker(uia, &raw) == S_OK && raw);
    IUIAutomationElement *last = NULL;
    CHECK(IUIAutomationTreeWalker_GetLastChildElement(raw, win, &last) == S_OK && last);
    CHECK(same_element(uia, last, bar));
    IUIAutomationElement_Release(last);
    IUIAutomationTreeWalker_Release(raw);
    (void)title;
    CHECK(name_empty(bar) && element_hwnd(bar) == NULL);
    IUIAutomationCondition *all = NULL;
    CHECK(IUIAutomation_CreateTrueCondition(uia, &all) == S_OK);
    IUIAutomationElementArray *kids = NULL;
    int n = 0;
    CHECK(IUIAutomationElement_FindAll(bar, TreeScope_Children, all, &kids) == S_OK && kids);
    CHECK(IUIAutomationElementArray_get_Length(kids, &n) == S_OK && n == 3);
    const wchar_t *ids[3] = {L"Minimize", L"Maximize", L"Close"};
    for (int i = 0; i < 3; i++) {
        CHECK(IUIAutomationElementArray_GetElement(kids, i, &buttons[i]) == S_OK && buttons[i]);
        CHECK(control_type(buttons[i]) == UIA_ButtonControlTypeId);
        CHECK(automation_id_is(buttons[i], ids[i]) && name_is(buttons[i], ids[i]));
        CHECK(element_hwnd(buttons[i]) == NULL);
    }
    IUIAutomationElementArray_Release(kids);
    IUIAutomationCondition_Release(all);
    return bar;
}

static void invoke(IUIAutomationElement *e) {
    IUIAutomationInvokePattern *p =
        (IUIAutomationInvokePattern *)pattern_of(e, UIA_InvokePatternId, &IID_IUIAutomationInvokePattern);
    CHECK(IUIAutomationInvokePattern_Invoke(p) == S_OK);
    IUIAutomationInvokePattern_Release(p);
}

/* The Window and Transform patterns and the title bar of a framed window
 * (WS_OVERLAPPEDWINDOW, titled `title`, in this process or the child's):
 * its state, Move and Resize, the visual states and the title bar's
 * Maximize/Restore and Minimize buttons; then Window.Close (`by_button`:
 * the title bar's Close button instead) closes it: WM_CLOSE (`closes()`
 * grows), the window gone. */
static void frame_window(IUIAutomation *uia, IUIAutomationElement *win, const wchar_t *title, int by_button,
                         int (*closes)(void)) {
    HWND h = element_hwnd(win);
    CHECK(IsWindow(h) && control_type(win) == UIA_WindowControlTypeId);
    IUIAutomationElement *buttons[3];
    IUIAutomationElement *bar = title_bar(uia, win, title, buttons);

    IUIAutomationWindowPattern *w = window_pattern(win);
    BOOL b = -1;
    CHECK(IUIAutomationWindowPattern_get_CurrentCanMaximize(w, &b) == S_OK && b == TRUE);
    CHECK(IUIAutomationWindowPattern_get_CurrentCanMinimize(w, &b) == S_OK && b == TRUE);
    CHECK(IUIAutomationWindowPattern_get_CurrentIsModal(w, &b) == S_OK && b == FALSE);
    CHECK(IUIAutomationWindowPattern_get_CurrentIsTopmost(w, &b) == S_OK && b == FALSE);
    CHECK(visual_state(w) == WindowVisualState_Normal);
    enum WindowInteractionState is = (enum WindowInteractionState)-1;
    CHECK(IUIAutomationWindowPattern_get_CurrentWindowInteractionState(w, &is) == S_OK &&
          is == WindowInteractionState_ReadyForUserInteraction);
    BOOL idle = FALSE;
    CHECK(IUIAutomationWindowPattern_WaitForInputIdle(w, 1000, &idle) == S_OK && idle);
    VARIANT v;
    VariantInit(&v);
    CHECK(IUIAutomationElement_GetCurrentPropertyValue(win, UIA_WindowWindowVisualStatePropertyId, &v) == S_OK &&
          v.vt == VT_I4 && v.lVal == WindowVisualState_Normal);
    CHECK(IUIAutomationElement_GetCurrentPropertyValue(win, UIA_IsWindowPatternAvailablePropertyId, &v) == S_OK &&
          v.vt == VT_BOOL && v.boolVal == VARIANT_TRUE);

    // Transform: Move and Resize change the window rectangle.
    IUIAutomationTransformPattern *t = (IUIAutomationTransformPattern *)pattern_of(
        win, UIA_TransformPatternId, &IID_IUIAutomationTransformPattern);
    CHECK(IUIAutomationTransformPattern_get_CurrentCanMove(t, &b) == S_OK && b == TRUE);
    CHECK(IUIAutomationTransformPattern_get_CurrentCanResize(t, &b) == S_OK && b == TRUE);
    CHECK(IUIAutomationTransformPattern_get_CurrentCanRotate(t, &b) == S_OK && b == FALSE);
    CHECK(IUIAutomationTransformPattern_Move(t, 150, 120) == S_OK);
    WAIT(rect_is(h, 150, 120, 320, 240));
    CHECK(rect_is(h, 150, 120, 320, 240));
    CHECK(IUIAutomationTransformPattern_Resize(t, 360, 260) == S_OK);
    WAIT(rect_is(h, 150, 120, 360, 260));
    CHECK(rect_is(h, 150, 120, 360, 260));
    CHECK(FAILED(IUIAutomationTransformPattern_Rotate(t, 90)));
    RECT r;
    CHECK(IUIAutomationElement_get_CurrentBoundingRectangle(win, &r) == S_OK && r.left == 150 && r.top == 120 &&
          r.right == 510 && r.bottom == 380);

    // Maximized: IsZoomed, another rectangle, the title bar's middle
    // button is Restore, the window cannot move; Normal: back.
    CHECK(IUIAutomationWindowPattern_SetWindowVisualState(w, WindowVisualState_Maximized) == S_OK);
    WAIT(visual_state(w) == WindowVisualState_Maximized);
    CHECK(visual_state(w) == WindowVisualState_Maximized && IsZoomed(h) && !IsIconic(h));
    CHECK(!rect_is(h, 150, 120, 360, 260));
    CHECK(automation_id_is(buttons[1], L"Restore") && name_is(buttons[1], L"Restore"));
    CHECK(IUIAutomationTransformPattern_get_CurrentCanMove(t, &b) == S_OK && b == FALSE);
    CHECK(IUIAutomationWindowPattern_SetWindowVisualState(w, WindowVisualState_Normal) == S_OK);
    WAIT(visual_state(w) == WindowVisualState_Normal);
    CHECK(!IsZoomed(h) && rect_is(h, 150, 120, 360, 260));
    CHECK(automation_id_is(buttons[1], L"Maximize"));
    // Minimized: IsIconic; Normal: back.
    CHECK(IUIAutomationWindowPattern_SetWindowVisualState(w, WindowVisualState_Minimized) == S_OK);
    WAIT(IsIconic(h));
    CHECK(IsIconic(h) && visual_state(w) == WindowVisualState_Minimized);
    CHECK(IUIAutomationWindowPattern_SetWindowVisualState(w, WindowVisualState_Normal) == S_OK);
    WAIT(!IsIconic(h));
    CHECK(!IsIconic(h) && visual_state(w) == WindowVisualState_Normal && rect_is(h, 150, 120, 360, 260));
    // The title bar's buttons (Invoke posts their commands): Maximize,
    // then Restore, then Minimize.
    invoke(buttons[1]);
    WAIT(IsZoomed(h));
    CHECK(IsZoomed(h));
    invoke(buttons[1]);
    WAIT(!IsZoomed(h));
    CHECK(!IsZoomed(h) && rect_is(h, 150, 120, 360, 260));
    invoke(buttons[0]);
    WAIT(IsIconic(h));
    CHECK(IsIconic(h));
    CHECK(IUIAutomationWindowPattern_SetWindowVisualState(w, WindowVisualState_Normal) == S_OK);
    WAIT(!IsIconic(h));
    CHECK(!IsIconic(h));

    // Close: WM_CLOSE, then the window is gone.
    int before = closes();
    if (by_button)
        invoke(buttons[2]);
    else
        CHECK(IUIAutomationWindowPattern_Close(w) == S_OK);
    WAIT(closes() > before && !IsWindow(h));
    CHECK(closes() > before && !IsWindow(h));
    CHECK(IUIAutomationWindowPattern_get_CurrentCanMaximize(w, &b) == UIA_E_ELEMENTNOTAVAILABLE);

    IUIAutomationTransformPattern_Release(t);
    IUIAutomationWindowPattern_Release(w);
    for (int i = 0; i < 3; i++)
        IUIAutomationElement_Release(buttons[i]);
    IUIAutomationElement_Release(bar);
}

static int local_closes(void) {
    return frame_closes;
}

/* Framed windows of this process: the Window and Transform patterns,
 * title bars; a modal window (owned by a disabled window, topmost). */
static void frames(IUIAutomation *uia) {
    CHECK(register_frame_class());
    HWND h = make_frame(L"UIA Frame Local", 200, 200, NULL, WS_OVERLAPPEDWINDOW);
    CHECK(h != NULL);
    IUIAutomationElement *win = NULL;
    CHECK(IUIAutomation_ElementFromHandle(uia, h, &win) == S_OK && win);
    frame_window(uia, win, L"UIA Frame Local", 0, local_closes);
    IUIAutomationElement_Release(win);
    h = make_frame(L"UIA Closable Local", 220, 220, NULL, WS_OVERLAPPEDWINDOW);
    CHECK(h != NULL);
    CHECK(IUIAutomation_ElementFromHandle(uia, h, &win) == S_OK && win);
    frame_window(uia, win, L"UIA Closable Local", 1, local_closes);
    IUIAutomationElement_Release(win);

    // A dialog-like window: owned by a disabled window (modal), topmost,
    // a system menu but no minimize/maximize boxes: only Close on its
    // title bar, and the Window pattern refuses Maximized.
    HWND owner = make_frame(L"UIA Owner", 240, 240, NULL, WS_OVERLAPPEDWINDOW);
    HWND modal = CreateWindowExW(WS_EX_TOPMOST, L"UiaFrameWindow", L"UIA Modal",
                                 WS_POPUP | WS_CAPTION | WS_SYSMENU | WS_VISIBLE, 260, 260, 200, 120, owner, NULL,
                                 GetModuleHandleW(NULL), NULL);
    CHECK(owner && modal);
    EnableWindow(owner, FALSE);
    IUIAutomationElement *o = NULL, *m = NULL;
    CHECK(IUIAutomation_ElementFromHandle(uia, owner, &o) == S_OK && o);
    CHECK(IUIAutomation_ElementFromHandle(uia, modal, &m) == S_OK && m);
    IUIAutomationWindowPattern *w = window_pattern(m);
    BOOL b = -1;
    CHECK(IUIAutomationWindowPattern_get_CurrentIsModal(w, &b) == S_OK && b == TRUE);
    CHECK(IUIAutomationWindowPattern_get_CurrentIsTopmost(w, &b) == S_OK && b == TRUE);
    CHECK(IUIAutomationWindowPattern_get_CurrentCanMaximize(w, &b) == S_OK && b == FALSE);
    CHECK(IUIAutomationWindowPattern_SetWindowVisualState(w, WindowVisualState_Maximized) ==
          UIA_E_INVALIDOPERATION);
    IUIAutomationWindowPattern *ow = window_pattern(o);
    enum WindowInteractionState is = (enum WindowInteractionState)-1;
    CHECK(IUIAutomationWindowPattern_get_CurrentWindowInteractionState(ow, &is) == S_OK &&
          is == WindowInteractionState_BlockedByModalWindow);
    // FlaUI's Window.ModalWindows: the owner's descendants that are
    // modal windows.
    VARIANT v;
    VariantInit(&v);
    v.vt = VT_BOOL;
    v.boolVal = VARIANT_TRUE;
    IUIAutomationCondition *is_modal = NULL;
    CHECK(IUIAutomation_CreatePropertyCondition(uia, UIA_WindowIsModalPropertyId, v, &is_modal) == S_OK);
    IUIAutomationElementArray *found = NULL;
    int n = 0;
    CHECK(IUIAutomationElement_FindAll(o, TreeScope_Descendants, is_modal, &found) == S_OK && found);
    CHECK(IUIAutomationElementArray_get_Length(found, &n) == S_OK && n == 1);
    IUIAutomationElement *e = NULL;
    CHECK(IUIAutomationElementArray_GetElement(found, 0, &e) == S_OK && element_hwnd(e) == modal);
    IUIAutomationElement_Release(e);
    IUIAutomationElementArray_Release(found);
    IUIAutomationCondition_Release(is_modal);
    // Its title bar: Close only.
    IUIAutomationCondition *is_bar = prop_int(uia, UIA_ControlTypePropertyId, UIA_TitleBarControlTypeId);
    IUIAutomationElement *bar = NULL;
    CHECK(IUIAutomationElement_FindFirst(m, TreeScope_Children, is_bar, &bar) == S_OK && bar);
    IUIAutomationCondition *all = NULL;
    CHECK(IUIAutomation_CreateTrueCondition(uia, &all) == S_OK);
    CHECK(IUIAutomationElement_FindAll(bar, TreeScope_Children, all, &found) == S_OK && found);
    CHECK(IUIAutomationElementArray_get_Length(found, &n) == S_OK && n == 1);
    CHECK(IUIAutomationElementArray_GetElement(found, 0, &e) == S_OK && automation_id_is(e, L"Close"));
    IUIAutomationElement_Release(e);
    IUIAutomationElementArray_Release(found);
    IUIAutomationCondition_Release(all);
    IUIAutomationElement_Release(bar);
    IUIAutomationCondition_Release(is_bar);
    IUIAutomationWindowPattern_Release(ow);
    IUIAutomationWindowPattern_Release(w);
    IUIAutomationElement_Release(m);
    IUIAutomationElement_Release(o);
    DestroyWindow(modal);
    DestroyWindow(owner);
}

static HANDLE remote_closed;
static int remote_close_count;
static int remote_closes(void) {
    if (WaitForSingleObject(remote_closed, 0) == WAIT_OBJECT_0)
        remote_close_count++;
    return remote_close_count;
}

/* The child's framed windows: the same, across processes. */
static void remote_frames(IUIAutomation *uia, IUIAutomationElement *root) {
    const wchar_t *titles[2] = {L"UIA Frame", L"UIA Closable"};
    for (int i = 0; i < 2; i++) {
        IUIAutomationCondition *named = prop_str(uia, UIA_NamePropertyId, titles[i]);
        IUIAutomationElement *win = NULL;
        CHECK(IUIAutomationElement_FindFirst(root, TreeScope_Children, named, &win) == S_OK && win);
        frame_window(uia, win, titles[i], i, remote_closes);
        IUIAutomationElement_Release(win);
        IUIAutomationCondition_Release(named);
    }
}

/* The provided window `win` (handle `hwnd`)'s fragments, as elements:
 * its children "Fragment A" (Text) and "Fragment B" (Hyperlink) in
 * order, found by AutomationId and walked to, their rectangles the
 * provider's, no window handle, distinct runtime ids, their parent the
 * window. */
static void fragments(IUIAutomation *uia, IUIAutomationElement *win, HWND hwnd) {
    IUIAutomationCondition *c = prop_str(uia, UIA_AutomationIdPropertyId, L"frag-b");
    IUIAutomationElement *b = NULL;
    CHECK(IUIAutomationElement_FindFirst(win, TreeScope_Children, c, &b) == S_OK && b != NULL);
    IUIAutomationCondition_Release(c);
    CHECK(name_is(b, L"Fragment B"));
    CHECK(control_type(b) == UIA_HyperlinkControlTypeId);
    CHECK(element_hwnd(b) == NULL);
    RECT w, r;
    CHECK(GetWindowRect(hwnd, &w));
    CHECK(IUIAutomationElement_get_CurrentBoundingRectangle(b, &r) == S_OK);
    CHECK(r.left == w.left + 70 && r.top == w.top + 30 && r.right == r.left + 50 && r.bottom == r.top + 20);
    IUIAutomationTreeWalker *raw = NULL;
    CHECK(IUIAutomation_get_RawViewWalker(uia, &raw) == S_OK && raw);
    IUIAutomationElement *a = NULL, *next = NULL, *last = NULL, *parent = NULL;
    CHECK(IUIAutomationTreeWalker_GetFirstChildElement(raw, win, &a) == S_OK && a != NULL);
    CHECK(name_is(a, L"Fragment A") && control_type(a) == UIA_TextControlTypeId);
    CHECK(IUIAutomationTreeWalker_GetNextSiblingElement(raw, a, &next) == S_OK && next != NULL);
    CHECK(same_element(uia, next, b));
    CHECK(!same_element(uia, a, b));
    CHECK(IUIAutomationTreeWalker_GetLastChildElement(raw, win, &last) == S_OK && last != NULL);
    CHECK(same_element(uia, last, b));
    CHECK(IUIAutomationTreeWalker_GetParentElement(raw, b, &parent) == S_OK && parent != NULL);
    CHECK(same_element(uia, parent, win));
    IUIAutomationElement *none = NULL;
    CHECK(IUIAutomationTreeWalker_GetNextSiblingElement(raw, b, &none) == S_OK && none == NULL);
    /* Patterns from the provider: B's Invoke retitles the window; A's
     * Value is set and read back; neither has the other's. */
    VARIANT avail;
    CHECK(IUIAutomationElement_GetCurrentPropertyValue(b, UIA_IsInvokePatternAvailablePropertyId, &avail) == S_OK);
    CHECK(avail.vt == VT_BOOL && avail.boolVal == VARIANT_TRUE);
    CHECK(IUIAutomationElement_GetCurrentPropertyValue(b, UIA_IsValuePatternAvailablePropertyId, &avail) == S_OK);
    CHECK(avail.vt == VT_BOOL && avail.boolVal == VARIANT_FALSE);
    IUnknown *unk = NULL;
    CHECK(IUIAutomationElement_GetCurrentPattern(b, UIA_InvokePatternId, &unk) == S_OK && unk != NULL);
    IUIAutomationInvokePattern *invoke = NULL;
    CHECK(IUnknown_QueryInterface(unk, &IID_IUIAutomationInvokePattern, (void **)&invoke) == S_OK && invoke);
    IUnknown_Release(unk);
    CHECK(IUIAutomationInvokePattern_Invoke(invoke) == S_OK);
    IUIAutomationInvokePattern_Release(invoke);
    WCHAR title[64] = L"";
    for (int i = 0; i < 100 && wcscmp(title, L"fragment invoked") != 0; i++) {
        Sleep(20);
        GetWindowTextW(hwnd, title, 64);
    }
    CHECK(wcscmp(title, L"fragment invoked") == 0);
    CHECK(IUIAutomationElement_GetCurrentPattern(a, UIA_InvokePatternId, &unk) == S_OK && unk == NULL);
    CHECK(IUIAutomationElement_GetCurrentPattern(a, UIA_ValuePatternId, &unk) == S_OK && unk != NULL);
    IUIAutomationValuePattern *value = NULL;
    CHECK(IUnknown_QueryInterface(unk, &IID_IUIAutomationValuePattern, (void **)&value) == S_OK && value);
    IUnknown_Release(unk);
    BSTR text = NULL;
    CHECK(IUIAutomationValuePattern_get_CurrentValue(value, &text) == S_OK && bstr_is(text, L"start"));
    SysFreeString(text);
    BSTR typed = SysAllocString(L"typed");
    CHECK(IUIAutomationValuePattern_SetValue(value, typed) == S_OK);
    SysFreeString(typed);
    CHECK(IUIAutomationValuePattern_get_CurrentValue(value, &text) == S_OK && bstr_is(text, L"typed"));
    SysFreeString(text);
    BOOL ro = TRUE;
    CHECK(IUIAutomationValuePattern_get_CurrentIsReadOnly(value, &ro) == S_OK && !ro);
    IUIAutomationValuePattern_Release(value);
    IUIAutomationElement_Release(parent);
    IUIAutomationElement_Release(last);
    IUIAutomationElement_Release(next);
    IUIAutomationElement_Release(a);
    IUIAutomationTreeWalker_Release(raw);
    IUIAutomationElement_Release(b);
}

/* UI Automation over another process's windows: found from the root by
 * Name, its controls by AutomationId, driven by Invoke, Value and Toggle. */
static void other_process(IUIAutomation *uia, IUIAutomationElement *root) {
    HANDLE ready = CreateEventW(NULL, TRUE, FALSE, L"UiaChildReady");
    HANDLE done = CreateEventW(NULL, TRUE, FALSE, L"UiaChildDone");
    CHECK(ready && done);
    remote_closed = CreateEventW(NULL, FALSE, FALSE, L"UiaFrameClosed");
    CHECK(remote_closed != NULL);
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

    menus(uia, root);

    // The owned window: under its owner, not the root.
    IUIAutomationCondition *owned_name = prop_str(uia, UIA_NamePropertyId, L"UIA Owned");
    IUIAutomationElement *owned = NULL;
    CHECK(IUIAutomationElement_FindFirst(root, TreeScope_Children, owned_name, &owned) == S_OK && !owned);
    CHECK(IUIAutomationElement_FindFirst(win, TreeScope_Children, owned_name, &owned) == S_OK && owned);
    CHECK(control_type(owned) == UIA_WindowControlTypeId);
    IUIAutomationTreeWalker *raw = NULL;
    CHECK(IUIAutomation_get_RawViewWalker(uia, &raw) == S_OK && raw);
    IUIAutomationElement *owner = NULL;
    CHECK(IUIAutomationTreeWalker_GetParentElement(raw, owned, &owner) == S_OK && owner);
    CHECK(same_element(uia, owner, win));
    IUIAutomationCondition *ok_name = prop_str(uia, UIA_NamePropertyId, L"OK");
    IUIAutomationElement *ok = NULL;
    CHECK(IUIAutomationElement_FindFirst(owned, TreeScope_Descendants, ok_name, &ok) == S_OK && ok);
    CHECK(control_type(ok) == UIA_ButtonControlTypeId);
    IUIAutomationElement_Release(ok);
    IUIAutomationCondition_Release(ok_name);
    IUIAutomationElement_Release(owner);
    IUIAutomationTreeWalker_Release(raw);
    IUIAutomationElement_Release(owned);
    IUIAutomationCondition_Release(owned_name);

    remote_frames(uia, root);

    // The child's provided window: the child's provider answers across
    // processes; the host (the window) gives the rest.
    IUIAutomationCondition *rid = prop_str(uia, UIA_AutomationIdPropertyId, L"remote-provided-id");
    IUIAutomationElement *rp = NULL;
    CHECK(IUIAutomationElement_FindFirst(root, TreeScope_Children, rid, &rp) == S_OK && rp != NULL);
    CHECK(name_is(rp, L"Remote provided"));
    CHECK(control_type(rp) == UIA_ButtonControlTypeId);
    CHECK(class_is(rp, L"UiaProvided"));
    CHECK(IUIAutomationElement_get_CurrentProcessId(rp, &pid) == S_OK && pid == (int)pi.dwProcessId);
    CHECK(element_hwnd(rp) == FindWindowW(L"UiaProvided", L"UIA Remote Provided"));
    fragments(uia, rp, element_hwnd(rp));
    IUIAutomationElement_Release(rp);
    IUIAutomationCondition_Release(rid);

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

static void providers(IUIAutomation *uia) {
    CHECK(make_provided(L"UIA Provided") != NULL);
    IUIAutomationElement *e = NULL;
    CHECK(IUIAutomation_ElementFromHandle(uia, provided_hwnd, &e) == S_OK && e != NULL);
    CHECK(name_is(e, L"Provided"));
    BSTR id = NULL;
    CHECK(IUIAutomationElement_get_CurrentAutomationId(e, &id) == S_OK && bstr_is(id, L"provided-id"));
    SysFreeString(id);
    CHECK(control_type(e) == UIA_ButtonControlTypeId);
    CHECK(provider_asked > 0);
    /* The host's: what the provider leaves empty. */
    CHECK(class_is(e, L"UiaProvided"));
    CHECK(element_hwnd(e) == provided_hwnd);
    int pid = 0;
    CHECK(IUIAutomationElement_get_CurrentProcessId(e, &pid) == S_OK && pid == (int)GetCurrentProcessId());
    fragments(uia, e, provided_hwnd);
    IUIAutomationElement_Release(e);
    /* Searches see the provider's values: the desktop's child with
     * AutomationId "provided-id" is the window. */
    IUIAutomationElement *root = NULL, *found = NULL;
    CHECK(IUIAutomation_GetRootElement(uia, &root) == S_OK && root != NULL);
    IUIAutomationCondition *c = prop_str(uia, UIA_AutomationIdPropertyId, L"provided-id");
    CHECK(IUIAutomationElement_FindFirst(root, TreeScope_Children, c, &found) == S_OK && found != NULL);
    CHECK(element_hwnd(found) == provided_hwnd);
    IUIAutomationElement_Release(found);
    IUIAutomationCondition_Release(c);
    IUIAutomationElement_Release(root);
    DestroyWindow(provided_hwnd);
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
    CHECK(IUIAutomationElementArray_get_Length(list, &n) == S_OK && n == 4);
    IUIAutomationElement *first = NULL;
    CHECK(IUIAutomationElementArray_GetElement(list, 3, &first) == S_OK && first != NULL);
    CHECK(control_type(first) == UIA_TitleBarControlTypeId && name_empty(first) && element_hwnd(first) == NULL);
    // No system menu: a title bar without buttons.
    IUIAutomationElementArray *bar_kids = NULL;
    CHECK(IUIAutomationElement_FindAll(first, TreeScope_Children, all, &bar_kids) == S_OK && bar_kids != NULL);
    CHECK(IUIAutomationElementArray_get_Length(bar_kids, &n) == S_OK && n == 0);
    IUIAutomationElementArray_Release(bar_kids);
    IUIAutomationElement_Release(first);
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
    frames(uia);
    providers(uia);
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
