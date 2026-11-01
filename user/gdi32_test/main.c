/* gdi32 as real Windows behaves (documented behaviour only), for what
 * wxWidgets (FileZilla) uses, added as FileZilla's missing-features
 * ledger names it:
 *
 * - The display DC: CreateDCW(L"DISPLAY") gives a DC whose GetDeviceCaps
 *   report the screen (HORZRES/VERTRES as GetSystemMetrics' screen),
 *   96 dpi (LOGPIXELSX/Y), 32 bits per pixel, one plane, a raster display
 *   (TECHNOLOGY DT_RASDISPLAY, RASTERCAPS with RC_BITBLT); DeleteDC.
 *   GetObjectType says OBJ_DC; a memory DC says OBJ_MEMDC.
 * - Memory DCs and bitmaps: CreateCompatibleDC starts with a 1 x 1
 *   monochrome bitmap selected; CreateCompatibleBitmap on the display DC
 *   gives a 32-bit bitmap (GetObjectW: BITMAP with the size, bmBitsPixel
 *   32, bmPlanes 1); selected into the memory DC, SelectObject returns the
 *   old (1 x 1, 1-bit) bitmap; SetPixel/GetPixel and PatBlt draw on it.
 *   CreateDIBSection (32-bit, top-down and bottom-up) returns its bits:
 *   drawing appears in them at the documented place (row 0 of a
 *   bottom-up DIB is the bottom line), and they appear in GetPixel.
 *   BitBlt SRCCOPY copies between memory DCs; SRCINVERT xors;
 *   StretchBlt doubles.
 * - Fonts: CreateFontIndirectW keeps the LOGFONTW (GetObjectW returns it,
 *   92 bytes); selected into a DC, GetTextMetricsW gives a height of at
 *   least |lfHeight|, internal leading tmHeight - |lfHeight|, ascent +
 *   descent = height, average width > 0, and bold weight for FW_BOLD;
 *   GetTextExtentPoint32W grows with the text and has the font's height;
 *   a bigger font measures bigger. GetTextFaceW gives the face name.
 *   GetObjectType says OBJ_FONT; GetCurrentObject(OBJ_FONT) is the font.
 *
 * - kernel32's MulDiv (font sizes): rounded half away from zero, -1 for a
 *   zero divisor or overflow.
 *
 * Prints "gdi32_test: ok" or exits with the number of the failing check. */
#include <windows.h>
#include <stdio.h>
#include <string.h>

static int check;
#define CHECK(cond) do { check++; if (!(cond)) { printf("gdi32_test: check %d (line %d) failed\n", check, __LINE__); fflush(stdout); ExitProcess(check); } } while (0)

static void display_dc(void) {
    HDC dc = CreateDCW(L"DISPLAY", NULL, NULL, NULL);
    CHECK(dc != NULL);
    CHECK(GetObjectType(dc) == OBJ_DC);
    CHECK(GetDeviceCaps(dc, HORZRES) == GetSystemMetrics(SM_CXSCREEN));
    CHECK(GetDeviceCaps(dc, VERTRES) == GetSystemMetrics(SM_CYSCREEN));
    CHECK(GetDeviceCaps(dc, LOGPIXELSX) == 96 && GetDeviceCaps(dc, LOGPIXELSY) == 96);
    CHECK(GetDeviceCaps(dc, BITSPIXEL) == 32 && GetDeviceCaps(dc, PLANES) == 1);
    CHECK(GetDeviceCaps(dc, TECHNOLOGY) == DT_RASDISPLAY);
    CHECK(GetDeviceCaps(dc, RASTERCAPS) & RC_BITBLT);
    CHECK(DeleteDC(dc));
}

static void bitmaps(void) {
    HDC screen = CreateDCW(L"DISPLAY", NULL, NULL, NULL);
    HDC mem = CreateCompatibleDC(screen);
    CHECK(mem != NULL && GetObjectType(mem) == OBJ_MEMDC);
    HBITMAP bmp = CreateCompatibleBitmap(screen, 20, 10);
    CHECK(bmp != NULL && GetObjectType(bmp) == OBJ_BITMAP);
    BITMAP info;
    CHECK(GetObjectW(bmp, sizeof info, &info) == sizeof info);
    CHECK(info.bmWidth == 20 && info.bmHeight == 10 && info.bmBitsPixel == 32 && info.bmPlanes == 1);
    HBITMAP old = (HBITMAP)SelectObject(mem, bmp);
    CHECK(old != NULL && GetObjectW(old, sizeof info, &info) == sizeof info);
    CHECK(info.bmWidth == 1 && info.bmHeight == 1 && info.bmBitsPixel == 1);
    CHECK(GetCurrentObject(mem, OBJ_BITMAP) == bmp);
    CHECK(SetPixel(mem, 3, 4, RGB(10, 20, 30)) == RGB(10, 20, 30));
    CHECK(GetPixel(mem, 3, 4) == RGB(10, 20, 30));
    CHECK(GetPixel(mem, 30, 4) == CLR_INVALID);
    HBRUSH red = CreateSolidBrush(RGB(255, 0, 0));
    HGDIOBJ old_brush = SelectObject(mem, red);
    CHECK(PatBlt(mem, 0, 0, 5, 5, PATCOPY));
    CHECK(GetPixel(mem, 4, 4) == RGB(255, 0, 0) && GetPixel(mem, 5, 5) != RGB(255, 0, 0));

    /* A top-down and a bottom-up 32-bit DIB section. */
    BITMAPINFO bi = {0};
    bi.bmiHeader.biSize = sizeof bi.bmiHeader;
    bi.bmiHeader.biWidth = 8;
    bi.bmiHeader.biHeight = -4; /* top-down */
    bi.bmiHeader.biPlanes = 1;
    bi.bmiHeader.biBitCount = 32;
    bi.bmiHeader.biCompression = BI_RGB;
    DWORD *top = NULL;
    HBITMAP dib = CreateDIBSection(screen, &bi, DIB_RGB_COLORS, (void **)&top, NULL, 0);
    CHECK(dib != NULL && top != NULL);
    DIBSECTION ds;
    CHECK(GetObjectW(dib, sizeof ds, &ds) == sizeof ds && ds.dsBm.bmBits == top && ds.dsBm.bmWidth == 8);
    HDC dmem = CreateCompatibleDC(screen);
    SelectObject(dmem, dib);
    CHECK(SetPixel(dmem, 1, 0, RGB(1, 2, 3)) == RGB(1, 2, 3));
    CHECK(top[1] == 0x00010203); /* BGRA in memory: 0x00RRGGBB */
    top[8 * 3 + 2] = 0x00a0b0c0;
    CHECK(GetPixel(dmem, 2, 3) == RGB(0xa0, 0xb0, 0xc0));
    bi.bmiHeader.biHeight = 4; /* bottom-up */
    DWORD *bottom = NULL;
    HBITMAP dib2 = CreateDIBSection(screen, &bi, DIB_RGB_COLORS, (void **)&bottom, NULL, 0);
    HDC dmem2 = CreateCompatibleDC(screen);
    SelectObject(dmem2, dib2);
    CHECK(SetPixel(dmem2, 0, 0, RGB(9, 9, 9)) == RGB(9, 9, 9));
    CHECK(bottom[8 * 3] == 0x00090909); /* the top line is the last row */

    /* BitBlt between memory DCs; SRCINVERT; StretchBlt. */
    CHECK(BitBlt(dmem2, 0, 0, 8, 4, dmem, 0, 0, SRCCOPY));
    CHECK(GetPixel(dmem2, 1, 0) == RGB(1, 2, 3) && GetPixel(dmem2, 2, 3) == RGB(0xa0, 0xb0, 0xc0));
    CHECK(BitBlt(dmem2, 1, 0, 1, 1, dmem, 1, 0, SRCINVERT));
    CHECK(GetPixel(dmem2, 1, 0) == RGB(0, 0, 0));
    CHECK(StretchBlt(mem, 10, 0, 4, 2, dmem, 1, 0, 2, 1, SRCCOPY));
    CHECK(GetPixel(mem, 10, 0) == RGB(1, 2, 3) && GetPixel(mem, 11, 1) == RGB(1, 2, 3));

    SelectObject(mem, old_brush);
    SelectObject(mem, old);
    CHECK(DeleteObject(red) && DeleteObject(bmp));
    CHECK(DeleteDC(dmem) && DeleteDC(dmem2) && DeleteObject(dib) && DeleteObject(dib2));
    CHECK(DeleteDC(mem) && DeleteDC(screen));
}

static void fonts(void) {
    LOGFONTW lf = {0};
    lf.lfHeight = -12;
    lf.lfWeight = FW_NORMAL;
    wcscpy(lf.lfFaceName, L"Segoe UI");
    HFONT font = CreateFontIndirectW(&lf);
    CHECK(font != NULL && GetObjectType(font) == OBJ_FONT);
    LOGFONTW back;
    CHECK(GetObjectW(font, sizeof back, &back) == sizeof back);
    CHECK(back.lfHeight == -12 && wcscmp(back.lfFaceName, L"Segoe UI") == 0);
    HDC screen = CreateDCW(L"DISPLAY", NULL, NULL, NULL);
    HGDIOBJ old = SelectObject(screen, font);
    CHECK(old != NULL && GetCurrentObject(screen, OBJ_FONT) == font);
    TEXTMETRICW tm;
    CHECK(GetTextMetricsW(screen, &tm));
    CHECK(tm.tmHeight >= 12 && tm.tmInternalLeading == tm.tmHeight - 12);
    CHECK(tm.tmAscent + tm.tmDescent == tm.tmHeight && tm.tmAveCharWidth > 0);
    CHECK(tm.tmWeight == FW_NORMAL);
    SIZE one, two;
    CHECK(GetTextExtentPoint32W(screen, L"a", 1, &one) && GetTextExtentPoint32W(screen, L"aaaa", 4, &two));
    CHECK(one.cx > 0 && two.cx > one.cx && one.cy == tm.tmHeight);
    WCHAR face[LF_FACESIZE];
    CHECK(GetTextFaceW(screen, LF_FACESIZE, face) > 0);

    lf.lfHeight = -24;
    lf.lfWeight = FW_BOLD;
    HFONT big = CreateFontIndirectW(&lf);
    SelectObject(screen, big);
    TEXTMETRICW tm2;
    SIZE bigger;
    CHECK(GetTextMetricsW(screen, &tm2) && tm2.tmHeight > tm.tmHeight && tm2.tmWeight == FW_BOLD);
    CHECK(GetTextExtentPoint32W(screen, L"aaaa", 4, &bigger) && bigger.cx > two.cx);
    SelectObject(screen, old);
    CHECK(DeleteObject(font) && DeleteObject(big));
    DeleteDC(screen);
}

int main(void) {
    CHECK(MulDiv(10, 3, 4) == 8 && MulDiv(-10, 3, 4) == -8 && MulDiv(9, 96, 72) == 12);
    CHECK(MulDiv(1, 1, 0) == -1 && MulDiv(0x7fffffff, 2, 1) == -1 && MulDiv(7, 1, 2) == 4);
    display_dc();
    bitmaps();
    fonts();
    printf("gdi32_test: ok\n");
    return 0;
}
