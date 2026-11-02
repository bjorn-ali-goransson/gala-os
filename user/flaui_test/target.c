/* The program flaui_test drives (FlaUI, through UI Automation): a window
 * "FlaUI Target" with a push button (control id 101, "Press": clicking it
 * sets the window's title to "FlaUI Pressed"), an edit (102) and a check
 * box (103, "Option"). It ends when its window is closed (WM_CLOSE, which
 * FlaUI's Application.Close posts through Process.CloseMainWindow).
 * Built by build.sh with mingw-w64 into user/bin/flaui_target.exe. */
#include <windows.h>

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
    HWND top = CreateWindowExW(0, L"FlaUiTarget", L"FlaUI Target", WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_VISIBLE,
                               240, 160, 320, 200, NULL, NULL, instance, NULL);
    if (!top)
        return 3;
    CreateWindowExW(0, L"BUTTON", L"Press", WS_CHILD | WS_VISIBLE | WS_TABSTOP | BS_PUSHBUTTON, 10, 10, 80, 24, top,
                    (HMENU)101, instance, NULL);
    CreateWindowExW(0, L"EDIT", L"", WS_CHILD | WS_VISIBLE | WS_TABSTOP | WS_BORDER | ES_LEFT, 10, 44, 160, 22, top,
                    (HMENU)102, instance, NULL);
    CreateWindowExW(0, L"BUTTON", L"Option", WS_CHILD | WS_VISIBLE | WS_TABSTOP | BS_AUTOCHECKBOX, 10, 76, 150, 20,
                    top, (HMENU)103, instance, NULL);
    MSG m;
    while (GetMessageW(&m, NULL, 0, 0) > 0) {
        TranslateMessage(&m);
        DispatchMessageW(&m);
    }
    return (int)m.wParam;
}
