/* wow64_gui_test: a 32-bit (x86) GUI program under WOW64 (ROADMAP WOW
 * stage 4, ARCHITECTURE "Plan: WOW64"). Built by build.sh with
 * mingw-w64's i686 compiler against user32, gdi32 and kernel32 from
 * SysWOW64. Expected results are those of 32-bit Windows programs on
 * 64-bit Windows: a window class and window, WM_CREATE's CREATESTRUCTW,
 * GetClassInfoW, GetObjectW's DIBSECTION, window longs and class longs, SendMessageW/PostMessageW with 32-bit WPARAM/LPARAM
 * through a message loop, the client rectangle, WM_PAINT painting a
 * rectangle read back with GetPixel, DestroyWindow and WM_QUIT.
 * Prints "wow64_gui_test: ok" or exits with the failing check's line. */
#include <windows.h>
#include <stdio.h>

static int failed;
#define CHECK(c) do { if (!(c)) { printf("wow64_gui_test: check failed at line %d\n", __LINE__); failed = __LINE__; } } while (0)

static int created_cx, created_cy, created_ok, painted, destroyed, posted;
/* WM_SHOWWINDOW: how many, the last wParam/lParam, whether the window
   was already visible then, and whether one came before the first
   WM_WINDOWPOSCHANGING after it was counted from zero. */
static int shows, show_wp = -1, show_lp = -1, show_visible = -1, poschanging, show_first;
/* In WM_DESTROY: IsWindowVisible and ShowWindow(SW_HIDE)'s answer. */
static int destroy_visible = -1, destroy_hide = -1, destroy_shows = -1;
#define COLOR RGB(0x12, 0x34, 0x56)

static LRESULT CALLBACK proc(HWND hwnd, UINT msg, WPARAM wp, LPARAM lp)
{
    switch (msg) {
    case WM_CREATE: {
        CREATESTRUCTW *cs = (CREATESTRUCTW *)lp;
        created_cx = cs->cx;
        created_cy = cs->cy;
        created_ok = cs->lpCreateParams == (void *)0x51 && lstrcmpW(cs->lpszClass, L"Wow64Gui") == 0;
        return 0;
    }
    case WM_USER + 1:
        return (LRESULT)(wp + lp);
    case WM_USER + 2:
        posted = (int)(wp == 7 && lp == -3);
        return 0;
    case WM_PAINT: {
        PAINTSTRUCT ps;
        HDC dc = BeginPaint(hwnd, &ps);
        RECT r = { 0, 0, 40, 30 };
        HBRUSH b = CreateSolidBrush(COLOR);
        FillRect(dc, &r, b);
        DeleteObject(b);
        EndPaint(hwnd, &ps);
        painted++;
        return 0;
    }
    case WM_SHOWWINDOW:
        if (shows++ == 0 && poschanging == 0)
            show_first = 1;
        show_wp = (int)wp;
        show_lp = (int)lp;
        show_visible = IsWindowVisible(hwnd) != 0;
        break;
    case WM_WINDOWPOSCHANGING:
        poschanging++;
        break;
    case WM_DESTROY:
        destroyed = 1;
        /* DestroyWindow hid the window first (no WM_SHOWWINDOW): a hide
           here finds it hidden (setup.exe's splash does this). */
        destroy_visible = IsWindowVisible(hwnd) != 0;
        shows = 0;
        destroy_hide = ShowWindow(hwnd, SW_HIDE);
        destroy_shows = shows;
        PostQuitMessage(42);
        return 0;
    }
    return DefWindowProcW(hwnd, msg, wp, lp);
}

int wmain(void)
{
    WNDCLASSEXW wc = { sizeof(wc) };
    wc.style = CS_HREDRAW;
    wc.lpfnWndProc = proc;
    wc.hInstance = GetModuleHandleW(NULL);
    wc.lpszClassName = L"Wow64Gui";
    wc.hbrBackground = (HBRUSH)GetStockObject(WHITE_BRUSH);
    wc.cbWndExtra = 2 * sizeof(LONG_PTR);
    ATOM atom = RegisterClassExW(&wc);
    CHECK(atom != 0);
    /* GetClassInfoW fills the 40-byte x86 WNDCLASSW, nothing after it. */
    struct { WNDCLASSW wc; DWORD canary; } ci;
    memset(&ci, 0xcc, sizeof ci);
    CHECK(GetClassInfoW(wc.hInstance, L"Wow64Gui", &ci.wc));
    CHECK(ci.wc.lpfnWndProc == proc && ci.wc.style == CS_HREDRAW && ci.wc.cbWndExtra == 2 * sizeof(LONG_PTR));
    CHECK(ci.canary == 0xcccccccc);

    HWND hwnd = CreateWindowExW(0, L"Wow64Gui", L"32-bit window", WS_OVERLAPPEDWINDOW,
                                100, 100, 300, 200, NULL, NULL, wc.hInstance, (void *)0x51);
    CHECK(hwnd != NULL);
    CHECK(created_cx == 300 && created_cy == 200 && created_ok);
    if (failed)
        printf("wow64_gui_test: WM_CREATE saw cx %d cy %d, params/class %s\n", created_cx, created_cy,
               created_ok ? "right" : "wrong");
    CHECK(IsWindow(hwnd));

    /* GetObjectW of a DIB section: the x86 BITMAP (24 bytes) and
     * DIBSECTION (84 bytes) with its bits pointer. */
    {
        BITMAPINFO bi = { { sizeof(BITMAPINFOHEADER), 4, 4, 1, 32, BI_RGB } };
        void *bits = NULL;
        HBITMAP dib = CreateDIBSection(NULL, &bi, DIB_RGB_COLORS, &bits, NULL, 0);
        DIBSECTION ds;
        BITMAP bm;
        CHECK(dib != NULL && bits != NULL);
        CHECK(GetObjectW(dib, sizeof bm, &bm) == sizeof bm && bm.bmBits == bits && bm.bmWidth == 4);
        CHECK(GetObjectW(dib, sizeof ds, &ds) == sizeof ds && ds.dsBm.bmBits == bits && ds.dsBmih.biWidth == 4);
        DeleteObject(dib);
    }

    /* Window longs: the extra bytes and the user data, 32-bit wide. */
    SetWindowLongPtrW(hwnd, 0, 0x1234567);
    CHECK(GetWindowLongPtrW(hwnd, 0) == 0x1234567);
    SetWindowLongPtrW(hwnd, GWLP_USERDATA, (LONG_PTR)-5);
    CHECK(GetWindowLongPtrW(hwnd, GWLP_USERDATA) == (LONG_PTR)-5);
    CHECK((WNDPROC)GetWindowLongPtrW(hwnd, GWLP_WNDPROC) == proc);
    /* Class longs, as Delphi's VCL reads them on x86. */
    CHECK(GetClassLongW(hwnd, GCL_STYLE) == CS_HREDRAW);
    CHECK(SetClassLongW(hwnd, GCL_STYLE, CS_HREDRAW | CS_VREDRAW) == CS_HREDRAW);
    CHECK(GetClassLongW(hwnd, GCL_STYLE) == (CS_HREDRAW | CS_VREDRAW));
    CHECK((WNDPROC)GetClassLongW(hwnd, GCL_WNDPROC) == proc);

    /* SendMessageW calls the 32-bit procedure directly. */
    CHECK(SendMessageW(hwnd, WM_USER + 1, 0x1000, 0x234) == 0x1234);

    /* PostMessageW through the queue: a negative LPARAM keeps its sign. */
    CHECK(PostMessageW(hwnd, WM_USER + 2, 7, -3));
    MSG m;
    while (!posted && GetMessageW(&m, NULL, 0, 0) > 0)
        DispatchMessageW(&m);
    CHECK(posted == 1);

    RECT client;
    CHECK(GetClientRect(hwnd, &client));
    CHECK(client.left == 0 && client.top == 0 && client.right > 200 && client.right < 300 &&
          client.bottom > 100 && client.bottom < 200);

    /* Hiding a hidden window: nothing, 0. */
    shows = 0;
    CHECK(ShowWindow(hwnd, SW_HIDE) == 0);
    CHECK(shows == 0);
    /* Showing it: WM_SHOWWINDOW(TRUE, 0) first, while still hidden. */
    poschanging = 0;
    CHECK(ShowWindow(hwnd, SW_SHOWNORMAL) == 0);
    CHECK(shows == 1 && show_wp == 1 && show_lp == 0 && show_visible == 0 && show_first);
    if (failed)
        printf("wow64_gui_test: WM_SHOWWINDOW %d times, wp %d lp %d visible %d first %d\n", shows,
               show_wp, show_lp, show_visible, show_first);
    /* Showing a shown window: nothing, nonzero. */
    shows = 0;
    CHECK(ShowWindow(hwnd, SW_SHOW) != 0);
    CHECK(shows == 0);
    UpdateWindow(hwnd);
    CHECK(painted >= 1);
    HDC dc = GetDC(hwnd);
    CHECK(dc != NULL);
    CHECK(GetPixel(dc, 10, 10) == COLOR);
    CHECK(GetPixel(dc, 100, 50) == RGB(255, 255, 255));
    ReleaseDC(hwnd, dc);

    CHECK(DestroyWindow(hwnd));
    CHECK(destroyed == 1);
    CHECK(destroy_visible == 0 && destroy_hide == 0 && destroy_shows == 0);
    if (failed)
        printf("wow64_gui_test: in WM_DESTROY visible %d, SW_HIDE %d, WM_SHOWWINDOW %d\n",
               destroy_visible, destroy_hide, destroy_shows);
    CHECK(!IsWindow(hwnd));
    int got = 0;
    while (GetMessageW(&m, NULL, 0, 0) > 0)
        DispatchMessageW(&m);
    got = (int)m.wParam;
    CHECK(m.message == WM_QUIT && got == 42);

    if (failed)
        return failed;
    printf("wow64_gui_test: ok\n");
    return 0;
}
