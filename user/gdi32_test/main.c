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
 *   Segoe UI -12 has Windows' 16/13/3 cell and is proportional ("iiii"
 *   much narrower than "WWWW", TMPF_FIXED_PITCH | TMPF_TRUETYPE,
 *   FF_SWISS); extents add up; GetTextExtentExPointW gives partial
 *   widths and the fit; TextOutW draws only inside its extent;
 *   ExtTextOutW ETO_OPAQUE fills its rectangle; Courier New is fixed
 *   pitch (TMPF_FIXED_PITCH clear, equal advances).
 *   GetObjectType says OBJ_FONT; GetCurrentObject(OBJ_FONT) is the font.
 *
 * - CreateDIBitmap: a DDB of the reference DC's depth; CBM_INIT sets
 *   the DIB's bits as SetDIBits does (bottom-up rows). SetStretchBltMode
 *   returns the previous mode (BLACKONWHITE at first), 0 for an invalid one.
 *
 * - Mapping: MM_TEXT at first (extents 1 x 1, SetWindowExtEx ignored);
 *   LPtoDP/DPtoLP through the origins; MM_ANISOTROPIC scales by viewport
 *   extent / window extent (a zero window extent is refused), PatBlt
 *   included; MM_ISOTROPIC shrinks the viewport extent to equal units;
 *   back to MM_TEXT, the extents are 1 again.
 *
 * - 1-bit DIB sections: 4-byte rows, bottom-up, bit 7 leftmost; pixels
 *   read as their palette colour, drawing picks the nearer entry;
 *   Get/SetDIBColorTable read and change the table (the count done).
 *
 * - CreateBitmapIndirect: CreateBitmap with the BITMAP's fields.
 *
 * - Palettes: CreatePalette keeps the entries (GetObjectType OBJ_PAL,
 *   GetObjectW: the entry count as a WORD, GetPaletteEntries,
 *   GetNearestPaletteIndex); SelectPalette returns the previous one (the
 *   stock DEFAULT_PALETTE first); RealizePalette does not fail;
 *   GetNearestColor on a true-colour DC is the colour.
 * - DIBs: GetDIBits with no buffer and biBitCount 0 fills in the header
 *   (size, 32 bits); with a buffer it copies rows in the format asked
 *   (bottom-up: the last line first; a negative height: top-down);
 *   SetDIBits writes a 24-bit DIB's rows; SetDIBitsToDevice and
 *   StretchDIBits draw a DIB (the latter scaled); GdiFlush succeeds.
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
    // Segoe UI at 96 DPI: a 12-pixel em gives a 16-pixel cell, 13 above
    // the baseline and 3 below; a TrueType, variable-pitch Swiss face
    // (TMPF_FIXED_PITCH set means variable pitch).
    CHECK(tm.tmHeight == 16 && tm.tmAscent == 13 && tm.tmDescent == 3);
    CHECK((tm.tmPitchAndFamily & (TMPF_FIXED_PITCH | TMPF_TRUETYPE)) == (TMPF_FIXED_PITCH | TMPF_TRUETYPE));
    CHECK((tm.tmPitchAndFamily & 0xf0) == FF_SWISS && tm.tmMaxCharWidth > tm.tmAveCharWidth);
    SIZE narrow, wide;
    CHECK(GetTextExtentPoint32W(screen, L"iiii", 4, &narrow) && GetTextExtentPoint32W(screen, L"WWWW", 4, &wide));
    CHECK(narrow.cx * 2 < wide.cx);
    // Extents add up: "iiiiWWWW" is the two together.
    SIZE both;
    CHECK(GetTextExtentPoint32W(screen, L"iiiiWWWW", 8, &both) && both.cx == narrow.cx + wide.cx);
    // GetTextExtentExPointW: the partial widths and how many fit.
    INT fit = 0, dx[8];
    SIZE ex;
    CHECK(GetTextExtentExPointW(screen, L"iiiiWWWW", 8, narrow.cx + 1, &fit, dx, &ex));
    CHECK(fit == 4 && dx[3] == narrow.cx && dx[7] == both.cx && ex.cx == both.cx && ex.cy == tm.tmHeight);

    // Drawing: TextOutW and ExtTextOutW (ETO_OPAQUE fills the rectangle)
    // into a white 32-bit DIB; black pixels only inside the text's box.
    BITMAPINFO bi = {{sizeof(BITMAPINFOHEADER), 64, -32, 1, 32, BI_RGB}};
    DWORD *bits;
    HBITMAP dib = CreateDIBSection(NULL, &bi, DIB_RGB_COLORS, (void **)&bits, NULL, 0);
    HDC mem = CreateCompatibleDC(screen);
    HGDIOBJ oldbmp = SelectObject(mem, dib);
    SelectObject(mem, font);
    for (int i = 0; i < 64 * 32; i++) bits[i] = 0xffffff;
    SetBkMode(mem, TRANSPARENT);
    SIZE hi;
    CHECK(GetTextExtentPoint32W(mem, L"Hi", 2, &hi) && TextOutW(mem, 4, 2, L"Hi", 2));
    int inside = 0, outside = 0;
    for (int y = 0; y < 32; y++)
        for (int x = 0; x < 64; x++)
            if ((bits[y * 64 + x] & 0xffffff) != 0xffffff) {
                if (x >= 4 && x < 4 + hi.cx && y >= 2 && y < 2 + hi.cy) inside++; else outside++;
            }
    CHECK(inside > 10 && outside == 0);
    RECT r = {0, 0, 64, 32};
    SetBkColor(mem, RGB(0, 0, 255));
    CHECK(ExtTextOutW(mem, 4, 2, ETO_OPAQUE, &r, L"Hi", 2, NULL));
    CHECK((bits[31 * 64 + 63] & 0xffffff) == 0x0000ff);
    SelectObject(mem, oldbmp);
    DeleteDC(mem);
    DeleteObject(dib);

    // A fixed-pitch face: every character advances the same.
    LOGFONTW mono = {0};
    mono.lfHeight = -12;
    wcscpy(mono.lfFaceName, L"Courier New");
    HFONT fixed = CreateFontIndirectW(&mono);
    SelectObject(screen, fixed);
    TEXTMETRICW tm3;
    CHECK(GetTextMetricsW(screen, &tm3) && (tm3.tmPitchAndFamily & TMPF_FIXED_PITCH) == 0);
    CHECK(GetTextExtentPoint32W(screen, L"iiii", 4, &narrow) && GetTextExtentPoint32W(screen, L"WWWW", 4, &wide));
    CHECK(narrow.cx == wide.cx && narrow.cx == 4 * tm3.tmAveCharWidth);
    SelectObject(screen, font);
    DeleteObject(fixed);

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

// CreateDIBitmap: a DDB in the reference DC's format; with CBM_INIT, the
// DIB's bits set as SetDIBits would (row 0 of a bottom-up DIB is the
// bottom line).
static void dibitmap(void) {
    HDC screen = CreateDCW(L"DISPLAY", NULL, NULL, NULL);
    // The stretch mode: BLACKONWHITE at first; set returns the previous.
    CHECK(GetStretchBltMode(screen) == BLACKONWHITE && SetStretchBltMode(screen, HALFTONE) == BLACKONWHITE);
    CHECK(GetStretchBltMode(screen) == HALFTONE && SetStretchBltMode(screen, 0) == 0);
    BITMAPINFO bi = {{sizeof(BITMAPINFOHEADER), 2, 2, 1, 32, BI_RGB}};
    // Bottom row: blue, green; top row: red, white (BGRX).
    DWORD px[4] = {0x0000ff, 0x00ff00, 0xff0000, 0xffffff};
    HBITMAP b = CreateDIBitmap(screen, &bi.bmiHeader, CBM_INIT, px, &bi, DIB_RGB_COLORS);
    CHECK(b != NULL && GetObjectType(b) == OBJ_BITMAP);
    BITMAP bm;
    CHECK(GetObjectW(b, sizeof bm, &bm) == sizeof bm && bm.bmWidth == 2 && bm.bmHeight == 2 && bm.bmBitsPixel == 32);
    HDC mem = CreateCompatibleDC(screen);
    HGDIOBJ old = SelectObject(mem, b);
    CHECK(GetPixel(mem, 0, 0) == RGB(255, 0, 0) && GetPixel(mem, 1, 0) == RGB(255, 255, 255));
    CHECK(GetPixel(mem, 0, 1) == RGB(0, 0, 255) && GetPixel(mem, 1, 1) == RGB(0, 255, 0));
    SelectObject(mem, old);
    // Without CBM_INIT: the size only.
    HBITMAP e = CreateDIBitmap(screen, &bi.bmiHeader, 0, NULL, NULL, 0);
    CHECK(e != NULL && GetObjectW(e, sizeof bm, &bm) == sizeof bm && bm.bmWidth == 2 && bm.bmHeight == 2);
    CHECK(DeleteObject(b) && DeleteObject(e));
    DeleteDC(mem);
    DeleteDC(screen);
}

// Coordinate mapping: MM_TEXT maps by the origins only; MM_ANISOTROPIC
// scales by viewport extent / window extent; LPtoDP and DPtoLP convert
// both ways; fixed modes ignore SetWindowExtEx; PatBlt in a scaled DC
// fills the scaled rectangle.
static void mapping(void) {
    HDC screen = CreateDCW(L"DISPLAY", NULL, NULL, NULL);
    HDC dc = CreateCompatibleDC(screen);
    HBITMAP bmp = CreateCompatibleBitmap(screen, 32, 32);
    HGDIOBJ old = SelectObject(dc, bmp);
    CHECK(GetMapMode(dc) == MM_TEXT);
    SIZE ext;
    CHECK(GetWindowExtEx(dc, &ext) && ext.cx == 1 && ext.cy == 1);
    CHECK(GetViewportExtEx(dc, &ext) && ext.cx == 1 && ext.cy == 1);
    SetWindowOrgEx(dc, 10, 20, NULL);
    POINT p[2] = {{15, 25}, {10, 20}};
    CHECK(LPtoDP(dc, p, 2) && p[0].x == 5 && p[0].y == 5 && p[1].x == 0 && p[1].y == 0);
    CHECK(DPtoLP(dc, p, 1) && p[0].x == 15 && p[0].y == 25);
    SetWindowOrgEx(dc, 0, 0, NULL);
    // Fixed modes ignore the window extent.
    CHECK(SetWindowExtEx(dc, 100, 100, NULL) && GetWindowExtEx(dc, &ext) && ext.cx == 1);

    CHECK(SetMapMode(dc, MM_ANISOTROPIC) == MM_TEXT && GetMapMode(dc) == MM_ANISOTROPIC);
    SIZE prev;
    CHECK(SetWindowExtEx(dc, 100, 100, &prev) && prev.cx == 1 && prev.cy == 1);
    CHECK(SetViewportExtEx(dc, 200, 50, NULL));
    CHECK(GetWindowExtEx(dc, &ext) && ext.cx == 100 && ext.cy == 100);
    CHECK(GetViewportExtEx(dc, &ext) && ext.cx == 200 && ext.cy == 50);
    POINT q = {10, 10};
    CHECK(LPtoDP(dc, &q, 1) && q.x == 20 && q.y == 5);
    CHECK(DPtoLP(dc, &q, 1) && q.x == 10 && q.y == 10);
    // Window ext 0 is refused.
    CHECK(!SetWindowExtEx(dc, 0, 10, NULL));

    // PatBlt of logical (2, 4, 4 x 8) at 2 x 1/2: device (4, 2) to (12, 6).
    PatBlt(dc, 0, 0, 64, 64, WHITENESS);
    PatBlt(dc, 2, 4, 4, 8, BLACKNESS);
    SetMapMode(dc, MM_TEXT);
    CHECK(GetPixel(dc, 4, 2) == 0 && GetPixel(dc, 11, 5) == 0);
    CHECK(GetPixel(dc, 12, 5) == 0xffffff && GetPixel(dc, 11, 6) == 0xffffff && GetPixel(dc, 3, 2) == 0xffffff);
    // Back in MM_TEXT the extents are 1 again.
    CHECK(GetWindowExtEx(dc, &ext) && ext.cx == 1 && GetViewportExtEx(dc, &ext) && ext.cx == 1);

    // MM_ISOTROPIC keeps both units the same size: the viewport extent
    // shrinks on the axis that would scale more.
    SetMapMode(dc, MM_ISOTROPIC);
    SetWindowExtEx(dc, 100, 100, NULL);
    SetViewportExtEx(dc, 200, 50, NULL);
    CHECK(GetViewportExtEx(dc, &ext) && ext.cx == 50 && ext.cy == 50);

    SelectObject(dc, old);
    DeleteObject(bmp);
    DeleteDC(dc);
    DeleteDC(screen);
}

// 1-bit DIB sections: rows padded to 4 bytes, bottom-up, bit 7 the
// leftmost pixel; pixels read as their palette colour, and drawing picks
// the nearer of the two.
// CreateBitmapIndirect: CreateBitmap with the BITMAP's fields (1-bit rows
// padded to 16 bits; set bits are white).
static void bitmap_indirect(void) {
    BYTE rows[4] = {0x80, 0, 0x01, 0};
    BITMAP bm = {0, 8, 2, 2, 1, 1, rows};
    HBITMAP b = CreateBitmapIndirect(&bm);
    CHECK(b != NULL);
    BITMAP back;
    CHECK(GetObjectW(b, sizeof back, &back) == sizeof back && back.bmWidth == 8 && back.bmBitsPixel == 1);
    HDC dc = CreateCompatibleDC(NULL);
    HGDIOBJ old = SelectObject(dc, b);
    CHECK(GetPixel(dc, 0, 0) == RGB(255, 255, 255) && GetPixel(dc, 1, 0) == 0);
    CHECK(GetPixel(dc, 7, 1) == RGB(255, 255, 255) && GetPixel(dc, 0, 1) == 0);
    SelectObject(dc, old);
    DeleteDC(dc);
    CHECK(DeleteObject(b));
}

static void mono_dib(void) {
    struct { BITMAPINFOHEADER h; RGBQUAD c[2]; } bi = {
        {sizeof(BITMAPINFOHEADER), 16, 2, 1, 1, BI_RGB}, {{0, 0, 255, 0}, {255, 0, 0, 0}}};
    BYTE *bits = NULL;
    HBITMAP b = CreateDIBSection(NULL, (BITMAPINFO *)&bi, DIB_RGB_COLORS, (void **)&bits, NULL, 0);
    CHECK(b != NULL && bits != NULL);
    BITMAP bm;
    CHECK(GetObjectW(b, sizeof bm, &bm) == sizeof bm && bm.bmBitsPixel == 1 && bm.bmWidthBytes == 4);
    CHECK(bm.bmWidth == 16 && bm.bmHeight == 2 && bm.bmBits == bits);
    HDC dc = CreateCompatibleDC(NULL);
    HGDIOBJ old = SelectObject(dc, b);
    // Palette: 0 red, 1 blue. Memory row 0 is the bottom line (y = 1).
    bits[0] = 0x80;
    CHECK(GetPixel(dc, 0, 1) == RGB(0, 0, 255) && GetPixel(dc, 1, 1) == RGB(255, 0, 0));
    CHECK(GetPixel(dc, 0, 0) == RGB(255, 0, 0));
    // Nearer to blue: bit set; nearer to red: clear.
    SetPixel(dc, 3, 0, RGB(20, 0, 230));
    CHECK(bits[4] == 0x10);
    SetPixel(dc, 0, 1, RGB(200, 30, 0));
    CHECK(bits[0] == 0);
    // The colour table: read, then entry 1 changed to green.
    RGBQUAD q[2];
    CHECK(GetDIBColorTable(dc, 0, 2, q) == 2 && q[0].rgbRed == 255 && q[1].rgbBlue == 255);
    RGBQUAD green = {0, 255, 0, 0};
    CHECK(SetDIBColorTable(dc, 1, 1, &green) == 1);
    CHECK(GetPixel(dc, 3, 0) == RGB(0, 255, 0));
    CHECK(GetDIBColorTable(dc, 1, 4, q) == 1 && q[0].rgbGreen == 255);
    SelectObject(dc, old);
    DeleteDC(dc);
    CHECK(DeleteObject(b));
}

static void palettes(void) {
    struct { WORD version, count; PALETTEENTRY entries[2]; } lp = {0x300, 2, {{255, 0, 0, 0}, {0, 255, 0, 0}}};
    HPALETTE pal = CreatePalette((LOGPALETTE *)&lp);
    CHECK(pal != NULL && GetObjectType(pal) == OBJ_PAL);
    WORD count = 0;
    CHECK(GetObjectW(pal, sizeof count, &count) == sizeof count && count == 2);
    PALETTEENTRY back[2];
    CHECK(GetPaletteEntries(pal, 0, 2, back) == 2 && back[1].peGreen == 255 && back[1].peRed == 0);
    CHECK(GetNearestPaletteIndex(pal, RGB(0, 250, 0)) == 1 && GetNearestPaletteIndex(pal, RGB(240, 5, 5)) == 0);
    HDC dc = CreateDCW(L"DISPLAY", NULL, NULL, NULL);
    HPALETTE old = SelectPalette(dc, pal, FALSE);
    CHECK(old != NULL && old == GetStockObject(DEFAULT_PALETTE));
    CHECK(RealizePalette(dc) != GDI_ERROR);
    CHECK(GetNearestColor(dc, RGB(12, 34, 56)) == RGB(12, 34, 56));
    CHECK(SelectPalette(dc, old, FALSE) == pal);
    CHECK(DeleteObject(pal));
    DeleteDC(dc);
}

static void dibs(void) {
    HDC screen = CreateDCW(L"DISPLAY", NULL, NULL, NULL);
    HDC mem = CreateCompatibleDC(screen);
    HBITMAP bmp = CreateCompatibleBitmap(screen, 4, 2);
    HGDIOBJ old = SelectObject(mem, bmp);
    SetPixel(mem, 0, 0, RGB(10, 0, 0));
    SetPixel(mem, 0, 1, RGB(0, 20, 0));
    SelectObject(mem, old);

    BITMAPINFO bi = {0};
    bi.bmiHeader.biSize = sizeof bi.bmiHeader;
    CHECK(GetDIBits(mem, bmp, 0, 2, NULL, &bi, DIB_RGB_COLORS) != 0);
    CHECK(bi.bmiHeader.biWidth == 4 && bi.bmiHeader.biHeight == 2 && bi.bmiHeader.biBitCount == 32);
    DWORD px[8];
    bi.bmiHeader.biCompression = BI_RGB;
    CHECK(GetDIBits(mem, bmp, 0, 2, px, &bi, DIB_RGB_COLORS) == 2);
    CHECK((px[0] & 0xffffff) == 0x001400 && (px[4] & 0xffffff) == 0x0a0000); /* bottom line first */
    bi.bmiHeader.biHeight = -2;
    CHECK(GetDIBits(mem, bmp, 0, 2, px, &bi, DIB_RGB_COLORS) == 2);
    CHECK((px[0] & 0xffffff) == 0x0a0000);

    /* A 24-bit bottom-up DIB: rows padded to 4 bytes (4 * 3 = 12). */
    BYTE rgb[24] = {0};
    rgb[0] = 0xff;                      /* bottom-left: blue */
    rgb[12 + 3 + 2] = 0xff;             /* top, second pixel: red */
    bi.bmiHeader.biHeight = 2;
    bi.bmiHeader.biBitCount = 24;
    CHECK(SetDIBits(mem, bmp, 0, 2, rgb, &bi, DIB_RGB_COLORS) == 2);
    SelectObject(mem, bmp);
    CHECK(GetPixel(mem, 0, 1) == RGB(0, 0, 255) && GetPixel(mem, 1, 0) == RGB(255, 0, 0));

    /* Drawing a DIB: 2 x 1, 32 bits. */
    DWORD two[2] = {0x00112233, 0x00445566};
    BITMAPINFO b2 = {0};
    b2.bmiHeader.biSize = sizeof b2.bmiHeader;
    b2.bmiHeader.biWidth = 2;
    b2.bmiHeader.biHeight = 1;
    b2.bmiHeader.biPlanes = 1;
    b2.bmiHeader.biBitCount = 32;
    CHECK(SetDIBitsToDevice(mem, 2, 1, 2, 1, 0, 0, 0, 1, two, &b2, DIB_RGB_COLORS) == 1);
    CHECK(GetPixel(mem, 2, 1) == RGB(0x11, 0x22, 0x33) && GetPixel(mem, 3, 1) == RGB(0x44, 0x55, 0x66));
    CHECK(StretchDIBits(mem, 0, 0, 4, 2, 0, 0, 2, 1, two, &b2, DIB_RGB_COLORS, SRCCOPY) != 0);
    CHECK(GetPixel(mem, 1, 1) == RGB(0x11, 0x22, 0x33) && GetPixel(mem, 3, 0) == RGB(0x44, 0x55, 0x66));
    CHECK(GdiFlush());
    SelectObject(mem, old);
    DeleteObject(bmp);
    DeleteDC(mem);
    DeleteDC(screen);
}

int main(void) {
    CHECK(MulDiv(10, 3, 4) == 8 && MulDiv(-10, 3, 4) == -8 && MulDiv(9, 96, 72) == 12);
    CHECK(MulDiv(1, 1, 0) == -1 && MulDiv(0x7fffffff, 2, 1) == -1 && MulDiv(7, 1, 2) == 4);
    display_dc();
    bitmaps();
    fonts();
    dibitmap();
    mapping();
    mono_dib();
    bitmap_indirect();
    palettes();
    dibs();
    printf("gdi32_test: ok\n");
    return 0;
}
