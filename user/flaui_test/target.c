/* The program flaui_test drives (FlaUI, through UI Automation): a window
 * "FlaUI Target" with a push button (control id 101, "Press": clicking it
 * sets the window's title to "FlaUI Pressed"), an edit (102) and a check
 * box (103, "Option") and a report-view list view (104, columns "Name"
 * and "Size", items "alpha.txt", "beta.txt", "gamma.txt"). The window is a WS_OVERLAPPEDWINDOW (minimize,
 * maximize and close buttons, a sizing frame) for FlaUI's Window API. It ends when its window is closed
 * (FlaUI's Window.Close: the title bar's Close button, WM_SYSCOMMAND SC_CLOSE, then WM_CLOSE).
 * Built by build.sh with mingw-w64 into user/bin/flaui_target.exe. */
#include <windows.h>
#include <commctrl.h>

static LRESULT CALLBACK proc(HWND hwnd, UINT msg, WPARAM wparam, LPARAM lparam) {
    switch (msg) {
    case WM_COMMAND:
        if (LOWORD(wparam) == 101 && HIWORD(wparam) == BN_CLICKED) {
            SetWindowTextW(hwnd, L"FlaUI Pressed");
            return 0;
        }
        break;
    case WM_DESTROY:
        PostQuitMessage(0);
        return 0;
    }
    return DefWindowProcW(hwnd, msg, wparam, lparam);
}

int main(void) {
    HINSTANCE instance = GetModuleHandleW(NULL);
    WNDCLASSW wc = {0};
    wc.lpfnWndProc = proc;
    wc.hInstance = instance;
    wc.hbrBackground = (HBRUSH)(COLOR_BTNFACE + 1);
    wc.lpszClassName = L"FlaUiTarget";
    if (!RegisterClassW(&wc))
        return 2;
    HWND top = CreateWindowExW(0, L"FlaUiTarget", L"FlaUI Target", WS_OVERLAPPEDWINDOW | WS_VISIBLE,
                               240, 160, 320, 200, NULL, NULL, instance, NULL);
    if (!top)
        return 3;
    CreateWindowExW(0, L"BUTTON", L"Press", WS_CHILD | WS_VISIBLE | WS_TABSTOP | BS_PUSHBUTTON, 10, 10, 80, 24, top,
                    (HMENU)101, instance, NULL);
    CreateWindowExW(0, L"EDIT", L"", WS_CHILD | WS_VISIBLE | WS_TABSTOP | WS_BORDER | ES_LEFT, 10, 44, 160, 22, top,
                    (HMENU)102, instance, NULL);
    CreateWindowExW(0, L"BUTTON", L"Option", WS_CHILD | WS_VISIBLE | WS_TABSTOP | BS_AUTOCHECKBOX, 10, 76, 150, 20,
                    top, (HMENU)103, instance, NULL);
    INITCOMMONCONTROLSEX icc = {sizeof icc, ICC_LISTVIEW_CLASSES};
    InitCommonControlsEx(&icc);
    HWND lv = CreateWindowExW(0, WC_LISTVIEWW, NULL, WS_CHILD | WS_VISIBLE | WS_BORDER | WS_TABSTOP | LVS_REPORT,
                              180, 10, 120, 110, top, (HMENU)104, instance, NULL);
    if (!lv)
        return 4;
    LVCOLUMNW col = {0};
    col.mask = LVCF_TEXT | LVCF_WIDTH;
    col.cx = 70;
    col.pszText = L"Name";
    SendMessageW(lv, LVM_INSERTCOLUMNW, 0, (LPARAM)&col);
    col.cx = 40;
    col.pszText = L"Size";
    SendMessageW(lv, LVM_INSERTCOLUMNW, 1, (LPARAM)&col);
    const wchar_t *names[3] = {L"alpha.txt", L"beta.txt", L"gamma.txt"};
    const wchar_t *sizes[3] = {L"1", L"22", L"333"};
    for (int i = 0; i < 3; i++) {
        LVITEMW item = {0};
        item.mask = LVIF_TEXT;
        item.iItem = i;
        item.pszText = (wchar_t *)names[i];
        SendMessageW(lv, LVM_INSERTITEMW, 0, (LPARAM)&item);
        item.iSubItem = 1;
        item.pszText = (wchar_t *)sizes[i];
        SendMessageW(lv, LVM_SETITEMTEXTW, i, (LPARAM)&item);
    }
    MSG m;
    while (GetMessageW(&m, NULL, 0, 0) > 0) {
        TranslateMessage(&m);
        DispatchMessageW(&m);
    }
    return (int)m.wParam;
}
