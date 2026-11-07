/* UI Automation over Chromium's accessibility tree (BR stage 2): the
 * browser's own UIA provider (WM_GETOBJECT, UiaRootObjectId), read and
 * driven through UIAutomationCore as a tester's client does on Windows.
 *
 * - Chromium (C:\chromium\chrome.exe, software rendering) opens
 *   C:\gala\window.html in an 800 x 600 window at (0, 0).
 * - Its top-level window (class Chrome_WidgetWin_1, title "gala window -
 *   Chromium") is found with FindWindow and as an element
 *   (ElementFromHandle).
 * - The page's link (an <a> with aria-label "gala link") is found under
 *   the window by ControlType Hyperlink and Name "gala link", within 300
 *   s (Chromium builds its tree once a client asks): its
 *   BoundingRectangle lies inside the window.
 * - Invoke on it follows the link: the window's title becomes "gala link
 *   - Chromium" (window2.html).
 * - WM_CLOSE ends Chromium with 0.
 *
 * Each step prints a line with its time; "uia_chromium_test: ok" at the
 * end, else the failing check's line and an exit with its number. */
#define COBJMACROS
#include <windows.h>
#include <initguid.h>
#include <uiautomation.h>
#include <stdio.h>
#include <wchar.h>

static DWORD start;
static int failed_line;

#define CHECK(cond)                                                                                   \
    do {                                                                                              \
        if (!(cond)) {                                                                                \
            printf("uia_chromium_test: check failed (line %d) at %lu ms: %s\n", __LINE__,             \
                   GetTickCount() - start, #cond);                                                    \
            fflush(stdout);                                                                           \
            ExitProcess(__LINE__);                                                                    \
        }                                                                                             \
    } while (0)

static void step(const char *what) {
    printf("uia_chromium_test: %s (%lu ms)\n", what, GetTickCount() - start);
    fflush(stdout);
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

/* The window whose title is `title`, waited for up to `ms`. */
static HWND wait_window(const wchar_t *title, DWORD ms) {
    for (DWORD t0 = GetTickCount(); GetTickCount() - t0 < ms; Sleep(500)) {
        HWND h = FindWindowW(L"Chrome_WidgetWin_1", title);
        if (h)
            return h;
    }
    return NULL;
}

/* The elements under `e` (depth-first, `depth` levels), printed: what a
 * failure to find the link leaves behind. */
static void dump(IUIAutomationTreeWalker *raw, IUIAutomationElement *e, int depth, int *budget) {
    IUIAutomationElement *c = NULL;
    if (IUIAutomationTreeWalker_GetFirstChildElement(raw, e, &c) != S_OK)
        return;
    while (c && (*budget)-- > 0) {
        BSTR name = NULL;
        CONTROLTYPEID t = 0;
        IUIAutomationElement_get_CurrentName(c, &name);
        IUIAutomationElement_get_CurrentControlType(c, &t);
        printf("uia_chromium_test: tree %*s%d \"%ls\"\n", 2 * (4 - depth), "", t, name ? name : L"");
        SysFreeString(name);
        if (depth > 1)
            dump(raw, c, depth - 1, budget);
        IUIAutomationElement *next = NULL;
        IUIAutomationTreeWalker_GetNextSiblingElement(raw, c, &next);
        IUIAutomationElement_Release(c);
        c = next;
    }
    if (c)
        IUIAutomationElement_Release(c);
    fflush(stdout);
}

int main(void) {
    start = GetTickCount();
    (void)failed_line;
    CHECK(CoInitializeEx(NULL, COINIT_APARTMENTTHREADED) == S_OK);
    IUIAutomation *uia = NULL;
    CHECK(CoCreateInstance(&CLSID_CUIAutomation, NULL, CLSCTX_INPROC_SERVER, &IID_IUIAutomation, (void **)&uia) ==
              S_OK &&
          uia);

    wchar_t cmd[] = L"\"C:\\chromium\\chrome.exe\" --no-sandbox --disable-gpu --disable-direct-composition "
                    L"--no-first-run --user-data-dir=C:\\tmp\\chr --enable-logging=stderr "
                    L"--ipc-connection-timeout=120 --window-position=0,0 --window-size=800,600 "
                    L"file:///C:/gala/window.html";
    STARTUPINFOW si = { sizeof si };
    PROCESS_INFORMATION pi;
    CHECK(CreateProcessW(NULL, cmd, NULL, NULL, TRUE, 0, NULL, NULL, &si, &pi));
    step("chromium started");

    HWND win = wait_window(L"gala window - Chromium", 600000);
    CHECK(win != NULL);
    step("window found");
    IUIAutomationElement *top = NULL;
    CHECK(IUIAutomation_ElementFromHandle(uia, (UIA_HWND)win, &top) == S_OK && top);

    IUIAutomationCondition *conds[2] = {
        prop_int(uia, UIA_ControlTypePropertyId, UIA_HyperlinkControlTypeId),
        prop_str(uia, UIA_NamePropertyId, L"gala link"),
    };
    IUIAutomationCondition *link_cond = NULL;
    CHECK(IUIAutomation_CreateAndCondition(uia, conds[0], conds[1], &link_cond) == S_OK);
    IUIAutomationElement *link = NULL;
    for (DWORD t0 = GetTickCount(); !link && GetTickCount() - t0 < 300000; Sleep(2000))
        CHECK(IUIAutomationElement_FindFirst(top, TreeScope_Descendants, link_cond, &link) == S_OK);
    if (!link) {
        IUIAutomationTreeWalker *raw = NULL;
        CHECK(IUIAutomation_get_RawViewWalker(uia, &raw) == S_OK);
        int budget = 200;
        dump(raw, top, 4, &budget);
    }
    CHECK(link != NULL);
    step("link found");
    RECT r, w;
    CHECK(IUIAutomationElement_get_CurrentBoundingRectangle(link, &r) == S_OK);
    CHECK(GetWindowRect(win, &w));
    printf("uia_chromium_test: link at (%ld, %ld) - (%ld, %ld) in (%ld, %ld) - (%ld, %ld)\n", r.left, r.top,
           r.right, r.bottom, w.left, w.top, w.right, w.bottom);
    CHECK(r.right - r.left == 200 && r.bottom - r.top == 100);
    CHECK(r.left >= w.left && r.right <= w.right && r.top >= w.top && r.bottom <= w.bottom);

    IUnknown *unk = NULL;
    CHECK(IUIAutomationElement_GetCurrentPattern(link, UIA_InvokePatternId, &unk) == S_OK && unk);
    IUIAutomationInvokePattern *invoke = NULL;
    CHECK(IUnknown_QueryInterface(unk, &IID_IUIAutomationInvokePattern, (void **)&invoke) == S_OK && invoke);
    IUnknown_Release(unk);
    CHECK(IUIAutomationInvokePattern_Invoke(invoke) == S_OK);
    IUIAutomationInvokePattern_Release(invoke);
    step("link invoked");
    CHECK(wait_window(L"gala link - Chromium", 300000) == win);
    step("link followed");

    IUIAutomationElement_Release(link);
    IUIAutomationCondition_Release(link_cond);
    IUIAutomationCondition_Release(conds[0]);
    IUIAutomationCondition_Release(conds[1]);
    IUIAutomationElement_Release(top);
    IUIAutomation_Release(uia);

    CHECK(PostMessageW(win, WM_CLOSE, 0, 0));
    CHECK(WaitForSingleObject(pi.hProcess, 300000) == WAIT_OBJECT_0);
    DWORD code = 1;
    CHECK(GetExitCodeProcess(pi.hProcess, &code) && code == 0);
    step("chromium closed");
    printf("uia_chromium_test: ok\n");
    return 0;
}
