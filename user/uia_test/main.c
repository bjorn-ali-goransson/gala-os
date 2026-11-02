/* UI Automation's client API (UIAutomationCore.dll, IUIAutomation) over
 * Win32 windows and standard controls, as Microsoft documents it; the
 * first step towards screen tests that find and drive controls by
 * AutomationId, Name and ControlType on Windows and gala alike.
 *
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
#include <stdio.h>
#include <string.h>

/* TreeScope_Subtree (mingw-w64 spells it TreeScope_SubTree). */
#define SUBTREE ((enum TreeScope)(TreeScope_Element | TreeScope_Children | TreeScope_Descendants))

static int check;
#define CHECK(cond) do { check++; if (!(cond)) { printf("uia_test: check %d (line %d) failed\n", check, __LINE__); fflush(stdout); ExitProcess(check); } } while (0)

static WPARAM last_command;
static LPARAM last_command_from;
static int commands;
static LRESULT CALLBACK parent_proc(HWND hwnd, UINT msg, WPARAM wparam, LPARAM lparam) {
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

int main(void) {
    CHECK(CoInitializeEx(NULL, COINIT_APARTMENTTHREADED) == S_OK);

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
    IUIAutomationElement_Release(root);
    IUIAutomationElement_Release(win);
    IUIAutomation_Release(uia);
    DestroyWindow(second);
    DestroyWindow(hwnd);
    CoUninitialize();
    printf("uia_test: ok\n");
    return 0;
}
