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
 * - The status bar: at the bottom of its parent, full width (and after
 *   WM_SIZE); parts, their texts and rectangles.
 * - Tooltips: tools added, counted, deleted; TTM_SETMAXTIPWIDTH.
 * - The toolbar: buttons added and counted, enabled, checked, side by
 *   side; TB_AUTOSIZE at the top, full width; a click sends WM_COMMAND
 *   with the button's id; TB_DELETEBUTTON; TB_ADDBITMAP with a bitmap
 *   strip returns its first image's index.
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
static LRESULT CALLBACK parent_proc(HWND hwnd, UINT msg, WPARAM wparam, LPARAM lparam) {
    if (msg == WM_COMMAND) {
        last_command = wparam;
        commands++;
    }
    return DefWindowProcW(hwnd, msg, wparam, lparam);
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
    CHECK(ImageList_Add(il, two, NULL) == 0);
    toolbar(parent, il);
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
