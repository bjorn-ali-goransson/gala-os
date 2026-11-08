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
 *   A DIB section over a file mapping (Chromium's shared canvas), from
 *   an offset: drawing appears in another view of the mapping, at the
 *   offset; DeleteObject unmaps the bitmap's view only.
 *   Bounds accumulation: SetBoundsRect(DCB_ENABLE | DCB_RESET), then a
 *   PatBlt and a SetPixel: GetBoundsRect gives DCB_SET and
 *   the rectangle around both; with DCB_RESET it is empty afterwards
 *   (DCB_RESET); DCB_DISABLE stops it. SetBoundsRect returns the previous
 *   state with its DCB_ENABLE or DCB_DISABLE bit.
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
 *   pitch (TMPF_FIXED_PITCH clear, equal advances). The ANSI forms
 *   (GetTextExtentPoint32A, GetTextExtentExPointA, GetTextMetricsA,
 *   ExtTextOutA: Scintilla in Inno Setup's IDE imports them) give the
 *   same for ASCII text.
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
 * - Regions: types, combining, testing, moving, clipping (SelectClipRgn,
 *   GetClipBox, ExcludeClipRect, IntersectClipRect), FillRgn,
 *   GetRegionData/ExtCreateRegion.
 *
 * - Drawing: fill and ROP2 modes, Polygon, Polyline, Ellipse, RoundRect,
 *   the brush origin, graphics mode and world transform (identity),
 *   ExtCreatePen, SetLayout, MaskBlt.
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
 * - DC state (what Chromium delay-imports): SaveDC returns the new depth;
 *   RestoreDC(-1) goes back one, RestoreDC(n) to depth n dropping those
 *   after it; a depth not saved, or 0, fails. SetDCBrushColor and
 *   SetDCPenColor return the previous colour (white and black at first;
 *   CLR_INVALID for no DC). CreateFontA / GetTextFaceA keep the ANSI face
 *   name (the count with the NUL). GetCharWidthW gives each character's
 *   advance (as GetTextExtentPoint32W measures it); last < first fails.
 *   GetFontData: the whole file for table 0, a table by its tag ('head'
 *   starts with version 1.0), GDI_ERROR for a table the font lacks.
 *   EnumFontFamiliesExW with an empty name and DEFAULT_CHARSET calls back
 *   at least once and returns the callback's last value (0 stops it); an
 *   unknown family gets no callback. EnumFontsW (Delphi asks it whether
 *   a face is installed): with no face name each family once, with the
 *   callback's last value returned; an unknown face gets no callback.
 * - Older calls Delphi's VCL makes: GetTextExtentPointW measures as
 *   GetTextExtentPoint32W; CreatePenIndirect makes a pen (OBJ_PEN);
 *   UnrealizeObject is TRUE for a brush; RectVisible: TRUE for a
 *   rectangle that meets the clipping region, FALSE for one outside it;
 *   LineDDA reports each point of a line but the last, one per step of
 *   the longer axis.
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

    /* A DIB section in a file mapping, 64 bytes in. */
    HANDLE mapping = CreateFileMappingW(INVALID_HANDLE_VALUE, NULL, PAGE_READWRITE, 0, 65536, NULL);
    CHECK(mapping != NULL);
    DWORD *view = MapViewOfFile(mapping, FILE_MAP_ALL_ACCESS, 0, 0, 0);
    CHECK(view != NULL);
    bi.bmiHeader.biHeight = -4;
    DWORD *shared = NULL;
    HBITMAP dib3 = CreateDIBSection(screen, &bi, DIB_RGB_COLORS, (void **)&shared, mapping, 64);
    CHECK(dib3 != NULL && shared != NULL && shared != view + 16);
    HDC dmem3 = CreateCompatibleDC(screen);
    SelectObject(dmem3, dib3);
    CHECK(SetPixel(dmem3, 2, 1, RGB(7, 8, 9)) == RGB(7, 8, 9));
    CHECK(view[16 + 8 + 2] == 0x00070809); /* seen through the other view */
    view[16] = 0x00445566;
    CHECK(GetPixel(dmem3, 0, 0) == RGB(0x44, 0x55, 0x66));
    CHECK(DeleteDC(dmem3) && DeleteObject(dib3));
    CHECK(view[16 + 8 + 2] == 0x00070809); /* the mapping stays */
    CHECK(UnmapViewOfFile(view) && CloseHandle(mapping));

    /* Bounds accumulation on a DC. */
    {
        RECT b;
        /* SetBoundsRect returns the previous state, accumulation bits
           included (a new DC: DCB_RESET | DCB_DISABLE, 0x9); GetBoundsRect
           only DCB_SET or DCB_RESET (Windows 11 25H2: DCB_RESET alone
           after enabling; the docs say the same). */
        CHECK(SetBoundsRect(dmem, NULL, DCB_ENABLE | DCB_RESET) == (DCB_RESET | DCB_DISABLE));
        CHECK(GetBoundsRect(dmem, &b, 0) == DCB_RESET);
        CHECK(PatBlt(dmem, 2, 1, 3, 2, BLACKNESS));
        CHECK(SetPixel(dmem, 6, 3, RGB(1, 1, 1)) == RGB(1, 1, 1));
        CHECK(GetBoundsRect(dmem, &b, DCB_RESET) == DCB_SET);
        CHECK(b.left == 2 && b.top == 1 && b.right == 7 && b.bottom == 4);
        CHECK(GetBoundsRect(dmem, &b, 0) == DCB_RESET);
        CHECK(SetBoundsRect(dmem, NULL, DCB_DISABLE) == (DCB_RESET | DCB_ENABLE));
        CHECK(SetPixel(dmem, 0, 0, RGB(2, 2, 2)) == RGB(2, 2, 2));
        CHECK(GetBoundsRect(dmem, &b, 0) == DCB_RESET);
    }

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
    // The ANSI forms (what Scintilla, in Inno Setup's IDE, imports) measure
    // the same text the same way.
    SIZE a_both, a_ex;
    INT a_fit = 0, a_dx[8];
    CHECK(GetTextExtentPoint32A(screen, "iiiiWWWW", 8, &a_both) && a_both.cx == both.cx && a_both.cy == both.cy);
    CHECK(GetTextExtentExPointA(screen, "iiiiWWWW", 8, narrow.cx + 1, &a_fit, a_dx, &a_ex));
    CHECK(a_fit == 4 && a_dx[3] == narrow.cx && a_dx[7] == both.cx && a_ex.cx == both.cx);
    TEXTMETRICA tma;
    CHECK(GetTextMetricsA(screen, &tma) && tma.tmHeight == tm.tmHeight && tma.tmAscent == tm.tmAscent);
    CHECK(tma.tmAveCharWidth == tm.tmAveCharWidth && tma.tmPitchAndFamily == tm.tmPitchAndFamily);

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
    SetBkColor(mem, RGB(0, 255, 0));
    CHECK(ExtTextOutA(mem, 4, 2, ETO_OPAQUE, &r, "Hi", 2, NULL));
    CHECK((bits[31 * 64 + 63] & 0xffffff) == 0x00ff00);
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

/* Regions: rectangles and their types (NULLREGION, SIMPLEREGION,
 * COMPLEXREGION), combined (AND, OR, XOR, DIFF, COPY), tested (points,
 * rectangles, equality), moved; as a DC's clip they limit drawing
 * (SelectClipRgn, GetClipBox, ExcludeClipRect, IntersectClipRect);
 * FillRgn paints them; GetRegionData lists the rectangles and
 * ExtCreateRegion makes the region back. */
static void regions(void) {
    HRGN a = CreateRectRgn(0, 0, 10, 10);
    CHECK(a != NULL && GetObjectType(a) == OBJ_REGION);
    RECT box;
    CHECK(GetRgnBox(a, &box) == SIMPLEREGION && box.left == 0 && box.right == 10 && box.bottom == 10);
    RECT r20 = {5, 5, 20, 20};
    HRGN b = CreateRectRgnIndirect(&r20);
    HRGN d = CreateRectRgn(0, 0, 0, 0);
    CHECK(GetRgnBox(d, &box) == NULLREGION);
    CHECK(CombineRgn(d, a, b, RGN_AND) == SIMPLEREGION && GetRgnBox(d, &box) == SIMPLEREGION);
    CHECK(box.left == 5 && box.top == 5 && box.right == 10 && box.bottom == 10);
    CHECK(CombineRgn(d, a, b, RGN_OR) == COMPLEXREGION && GetRgnBox(d, &box) == COMPLEXREGION);
    CHECK(box.left == 0 && box.right == 20 && box.bottom == 20);
    CHECK(PtInRegion(d, 15, 15) && PtInRegion(d, 2, 2) && !PtInRegion(d, 15, 2));
    RECT probe = {12, 0, 14, 4};
    CHECK(!RectInRegion(d, &probe));
    probe.bottom = 6;
    CHECK(RectInRegion(d, &probe));
    CHECK(CombineRgn(d, a, b, RGN_DIFF) == COMPLEXREGION && !PtInRegion(d, 7, 7) && PtInRegion(d, 2, 7));
    CHECK(CombineRgn(d, a, b, RGN_XOR) == COMPLEXREGION && !PtInRegion(d, 7, 7) && PtInRegion(d, 15, 15));
    CHECK(CombineRgn(d, a, NULL, RGN_COPY) == SIMPLEREGION && EqualRgn(d, a));
    /* Two rectangles that make one are one. */
    HRGN left = CreateRectRgn(0, 0, 5, 10), right = CreateRectRgn(5, 0, 10, 10);
    CHECK(CombineRgn(d, left, right, RGN_OR) == SIMPLEREGION && EqualRgn(d, a));
    CHECK(OffsetRgn(d, 3, 4) == SIMPLEREGION && GetRgnBox(d, &box) && box.left == 3 && box.top == 4);
    CHECK(!EqualRgn(d, a));

    /* RGNDATA round trip. */
    CHECK(CombineRgn(d, a, b, RGN_OR) == COMPLEXREGION);
    DWORD size = GetRegionData(d, 0, NULL);
    CHECK(size >= sizeof(RGNDATAHEADER) + 2 * sizeof(RECT));
    RGNDATA *data = (RGNDATA *)HeapAlloc(GetProcessHeap(), 0, size);
    CHECK(GetRegionData(d, size, data) == size && data->rdh.iType == RDH_RECTANGLES && data->rdh.nCount >= 2);
    HRGN e = ExtCreateRegion(NULL, size, data);
    CHECK(e != NULL && EqualRgn(e, d));
    HeapFree(GetProcessHeap(), 0, data);

    /* Clipping a memory DC. */
    HDC screen = CreateDCW(L"DISPLAY", NULL, NULL, NULL);
    HDC mem = CreateCompatibleDC(screen);
    HBITMAP bmp = CreateCompatibleBitmap(screen, 32, 32);
    HGDIOBJ old = SelectObject(mem, bmp);
    PatBlt(mem, 0, 0, 32, 32, WHITENESS);
    CHECK(GetClipBox(mem, &box) == SIMPLEREGION && box.right == 32 && box.bottom == 32);
    CHECK(SelectClipRgn(mem, a) == SIMPLEREGION);
    CHECK(GetClipBox(mem, &box) == SIMPLEREGION && box.right == 10);
    PatBlt(mem, 0, 0, 32, 32, BLACKNESS);
    /* Outside the clip GetPixel answers CLR_INVALID. */
    CHECK(GetPixel(mem, 5, 5) == 0 && GetPixel(mem, 15, 15) == CLR_INVALID);
    SelectClipRgn(mem, NULL);
    CHECK(GetPixel(mem, 15, 15) == RGB(255, 255, 255));
    SelectClipRgn(mem, a);
    CHECK(ExcludeClipRect(mem, 2, 2, 4, 4) == COMPLEXREGION);
    PatBlt(mem, 0, 0, 32, 32, WHITENESS);
    SelectClipRgn(mem, NULL);
    CHECK(GetPixel(mem, 3, 3) == 0 && GetPixel(mem, 8, 8) == RGB(255, 255, 255));
    CHECK(GetClipBox(mem, &box) == SIMPLEREGION && box.right == 32);
    CHECK(IntersectClipRect(mem, 20, 20, 30, 30) == SIMPLEREGION);
    PatBlt(mem, 0, 0, 32, 32, BLACKNESS);
    SelectClipRgn(mem, NULL);
    CHECK(GetPixel(mem, 25, 25) == 0 && GetPixel(mem, 3, 3) == 0 && GetPixel(mem, 8, 8) == RGB(255, 255, 255));
    /* FillRgn paints the region with a brush. */
    PatBlt(mem, 0, 0, 32, 32, WHITENESS);
    HBRUSH red = CreateSolidBrush(RGB(255, 0, 0));
    CHECK(FillRgn(mem, b, red));
    CHECK(GetPixel(mem, 10, 10) == RGB(255, 0, 0) && GetPixel(mem, 2, 2) == RGB(255, 255, 255));
    DeleteObject(red);
    SelectObject(mem, old);
    DeleteObject(bmp);
    DeleteDC(mem);
    DeleteDC(screen);
    CHECK(DeleteObject(a) && DeleteObject(b) && DeleteObject(d) && DeleteObject(e));
    DeleteObject(left);
    DeleteObject(right);
}

/* Drawing primitives on a white 64 x 64 memory DC with a red brush and a
 * blue pen: modes (polygon fill, ROP2: R2_NOT inverts), Polygon,
 * Polyline, Ellipse, RoundRect (insides in the brush, the far corners
 * untouched), SetBrushOrgEx, the graphics mode and world transform
 * (identity), ExtCreatePen, SetLayout; MaskBlt copies the source where
 * the mask is set, keeps the destination where it is not. */
static void shapes(void) {
    HDC screen = CreateDCW(L"DISPLAY", NULL, NULL, NULL);
    HDC dc = CreateCompatibleDC(screen);
    HBITMAP bmp = CreateCompatibleBitmap(screen, 64, 64);
    HGDIOBJ oldb = SelectObject(dc, bmp);
    PatBlt(dc, 0, 0, 64, 64, WHITENESS);
    HBRUSH red = CreateSolidBrush(RGB(255, 0, 0));
    HPEN blue = CreatePen(PS_SOLID, 1, RGB(0, 0, 255));
    HGDIOBJ oldbr = SelectObject(dc, red), oldp = SelectObject(dc, blue);
    CHECK(GetPolyFillMode(dc) == ALTERNATE && SetPolyFillMode(dc, WINDING) == ALTERNATE);
    CHECK(GetPolyFillMode(dc) == WINDING);
    CHECK(GetROP2(dc) == R2_COPYPEN);
    POINT tri[3] = {{2, 2}, {30, 2}, {2, 30}};
    CHECK(Polygon(dc, tri, 3));
    CHECK(GetPixel(dc, 8, 8) == RGB(255, 0, 0) && GetPixel(dc, 28, 28) == RGB(255, 255, 255));
    CHECK(GetPixel(dc, 15, 2) == RGB(0, 0, 255));
    POINT line[3] = {{40, 2}, {60, 2}, {60, 20}};
    CHECK(Polyline(dc, line, 3));
    CHECK(GetPixel(dc, 50, 2) == RGB(0, 0, 255) && GetPixel(dc, 60, 10) == RGB(0, 0, 255));
    CHECK(GetPixel(dc, 50, 10) == RGB(255, 255, 255));
    CHECK(Ellipse(dc, 34, 34, 62, 62));
    CHECK(GetPixel(dc, 48, 48) == RGB(255, 0, 0) && GetPixel(dc, 35, 35) == RGB(255, 255, 255));
    PatBlt(dc, 0, 0, 64, 64, WHITENESS);
    CHECK(RoundRect(dc, 2, 2, 40, 40, 16, 16));
    CHECK(GetPixel(dc, 20, 20) == RGB(255, 0, 0) && GetPixel(dc, 2, 2) == RGB(255, 255, 255));
    CHECK(GetPixel(dc, 20, 2) == RGB(0, 0, 255));
    /* R2_NOT: the pen inverts what it crosses. */
    CHECK(SetROP2(dc, R2_NOT) == R2_COPYPEN);
    MoveToEx(dc, 0, 50, NULL);
    LineTo(dc, 30, 50);
    CHECK(GetPixel(dc, 10, 50) == RGB(0, 0, 0) && GetPixel(dc, 20, 20) == RGB(255, 0, 0));
    SetROP2(dc, R2_COPYPEN);
    POINT org;
    CHECK(SetBrushOrgEx(dc, 3, 4, &org) && org.x == 0 && org.y == 0);
    CHECK(GetGraphicsMode(dc) == GM_COMPATIBLE && SetGraphicsMode(dc, GM_ADVANCED) == GM_COMPATIBLE);
    XFORM xf;
    CHECK(GetWorldTransform(dc, &xf) && xf.eM11 == 1.0f && xf.eM22 == 1.0f && xf.eDx == 0.0f);
    CHECK(ModifyWorldTransform(dc, NULL, MWT_IDENTITY));
    SetGraphicsMode(dc, GM_COMPATIBLE);
    CHECK(SetLayout(dc, 0) == 0);
    LOGBRUSH lb = {BS_SOLID, RGB(0, 255, 0), 0};
    HPEN green = ExtCreatePen(PS_GEOMETRIC | PS_SOLID, 3, &lb, 0, NULL);
    CHECK(green != NULL && GetObjectType(green) == OBJ_EXTPEN);
    SelectObject(dc, green);
    MoveToEx(dc, 0, 60, NULL);
    LineTo(dc, 63, 60);
    CHECK(GetPixel(dc, 30, 60) == RGB(0, 255, 0));
    SelectObject(dc, oldp);
    DeleteObject(green);

    /* MaskBlt: the source where the mask's bit is 1 (SRCCOPY), the
     * destination where it is 0. */
    HDC src = CreateCompatibleDC(screen);
    HBITMAP sb = CreateCompatibleBitmap(screen, 8, 8);
    HGDIOBJ olds = SelectObject(src, sb);
    PatBlt(src, 0, 0, 8, 8, BLACKNESS);
    BYTE bits[16] = {0xf0, 0, 0xf0, 0, 0xf0, 0, 0xf0, 0, 0xf0, 0, 0xf0, 0, 0xf0, 0, 0xf0, 0};
    HBITMAP mask = CreateBitmap(8, 8, 1, 1, bits);
    PatBlt(dc, 0, 0, 64, 64, WHITENESS);
    CHECK(MaskBlt(dc, 0, 0, 8, 8, src, 0, 0, mask, 0, 0, MAKEROP4(SRCCOPY, 0x00AA0029)));
    CHECK(GetPixel(dc, 1, 1) == RGB(0, 0, 0) && GetPixel(dc, 6, 1) == RGB(255, 255, 255));
    SelectObject(src, olds);
    DeleteObject(sb);
    DeleteObject(mask);
    DeleteDC(src);

    SelectObject(dc, oldbr);
    SelectObject(dc, oldb);
    DeleteObject(red);
    DeleteObject(blue);
    DeleteObject(bmp);
    DeleteDC(dc);
    DeleteDC(screen);
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

static int close_to(DWORD got, DWORD want) {
    for (int k = 0; k < 24; k += 8) {
        int d = (int)((got >> k) & 0xff) - (int)((want >> k) & 0xff);
        if (d < -2 || d > 2)
            return 0;
    }
    return 1;
}

/* A top-down 32-bit DIB section of w x h, selected into a new memory DC. */
static HDC dib_dc(int w, int h, DWORD **bits) {
    BITMAPINFO bi = {0};
    bi.bmiHeader.biSize = sizeof bi.bmiHeader;
    bi.bmiHeader.biWidth = w;
    bi.bmiHeader.biHeight = -h;
    bi.bmiHeader.biPlanes = 1;
    bi.bmiHeader.biBitCount = 32;
    HDC dc = CreateCompatibleDC(NULL);
    HBITMAP b = CreateDIBSection(dc, &bi, DIB_RGB_COLORS, (void **)bits, NULL, 0);
    CHECK(dc != NULL && b != NULL);
    SelectObject(dc, b);
    return dc;
}

/* msimg32: AlphaBlend (premultiplied source alpha and a constant alpha),
 * TransparentBlt (a colour key), GradientFill (rectangles). */
static void msimg(void) {
    DWORD *dst, *src;
    HDC d = dib_dc(4, 1, &dst);
    HDC s = dib_dc(4, 1, &src);
    for (int i = 0; i < 4; i++)
        dst[i] = 0x00ffffff;
    src[0] = 0x00000000; /* transparent */
    src[1] = 0xff102030; /* opaque */
    src[2] = 0x80400000; /* half, premultiplied */
    src[3] = 0xff000000; /* opaque black */
    BLENDFUNCTION bf = {AC_SRC_OVER, 0, 255, AC_SRC_ALPHA};
    CHECK(AlphaBlend(d, 0, 0, 4, 1, s, 0, 0, 4, 1, bf));
    CHECK((dst[0] & 0xffffff) == 0xffffff && (dst[1] & 0xffffff) == 0x102030);
    CHECK(close_to(dst[2], 0xbf7f7f) && (dst[3] & 0xffffff) == 0);

    /* No per-pixel alpha: the constant alpha alone (128 of 255). */
    for (int i = 0; i < 4; i++)
        dst[i] = 0x00ffffff;
    BLENDFUNCTION half = {AC_SRC_OVER, 0, 128, 0};
    CHECK(AlphaBlend(d, 0, 0, 4, 1, s, 0, 0, 4, 1, half));
    CHECK(close_to(dst[3], 0x7f7f7f) && close_to(dst[0], 0x7f7f7f));
    CHECK(!AlphaBlend(d, 0, 0, 4, 1, s, 0, 0, 5, 1, bf)); /* outside the source */

    /* A 32-bit device bitmap made from a DIB keeps the alpha byte (wx
     * draws its PNGs so): CreateDIBitmap, GetDIBits, then AlphaBlend. */
    DWORD with_alpha[2] = {0x80400000, 0x00000000};
    BITMAPINFO bi = {0};
    bi.bmiHeader.biSize = sizeof bi.bmiHeader;
    bi.bmiHeader.biWidth = 2;
    bi.bmiHeader.biHeight = -1;
    bi.bmiHeader.biPlanes = 1;
    bi.bmiHeader.biBitCount = 32;
    HDC screen = GetDC(NULL);
    HBITMAP ddb = CreateDIBitmap(screen, &bi.bmiHeader, CBM_INIT, with_alpha, &bi, DIB_RGB_COLORS);
    CHECK(ddb != NULL);
    DWORD back[2] = {0, 1};
    CHECK(GetDIBits(screen, ddb, 0, 1, back, &bi, DIB_RGB_COLORS) == 1 && back[0] == 0x80400000 && back[1] == 0);
    HDC m = CreateCompatibleDC(screen);
    HGDIOBJ was = SelectObject(m, ddb);
    dst[0] = dst[1] = 0x00ffffff;
    CHECK(AlphaBlend(d, 0, 0, 2, 1, m, 0, 0, 2, 1, bf));
    CHECK(close_to(dst[0], 0xbf7f7f) && (dst[1] & 0xffffff) == 0xffffff);
    SelectObject(m, was);
    DeleteObject(ddb);
    DeleteDC(m);
    ReleaseDC(NULL, screen);

    /* BitBlt SRCCOPY between 32-bit bitmaps copies the fourth byte too
     * (wxBitmap::GetSubBitmap cuts icons out of a strip so). */
    HBITMAP piece = CreateBitmap(2, 1, 1, 32, NULL);
    HDC pd = CreateCompatibleDC(NULL);
    HGDIOBJ pwas = SelectObject(pd, piece);
    src[0] = 0x80400000;
    src[1] = 0x00000000;
    CHECK(BitBlt(pd, 0, 0, 2, 1, s, 0, 0, SRCCOPY));
    dst[0] = dst[1] = 0x00ffffff;
    CHECK(AlphaBlend(d, 0, 0, 2, 1, pd, 0, 0, 2, 1, bf));
    CHECK(close_to(dst[0], 0xbf7f7f) && (dst[1] & 0xffffff) == 0xffffff);
    SelectObject(pd, pwas);
    DeleteObject(piece);
    DeleteDC(pd);

    /* TransparentBlt: source pixels of the key colour are left out. */
    src[0] = 0x00ff0000;
    src[1] = 0x0000ff00;
    dst[0] = dst[1] = 0x000000ff;
    CHECK(TransparentBlt(d, 0, 0, 2, 1, s, 0, 0, 2, 1, RGB(255, 0, 0)));
    CHECK((dst[0] & 0xffffff) == 0x0000ff && (dst[1] & 0xffffff) == 0x00ff00);

    /* GradientFill: red to blue across 256 pixels, then top to bottom. */
    DWORD *g;
    HDC gd = dib_dc(256, 2, &g);
    TRIVERTEX v[2] = {{0, 0, 0xff00, 0, 0, 0}, {256, 2, 0, 0, 0xff00, 0}};
    GRADIENT_RECT r = {0, 1};
    CHECK(GradientFill(gd, v, 2, &r, 1, GRADIENT_FILL_RECT_H));
    CHECK((g[0] & 0xffffff) == 0xff0000 && close_to(g[255], 0x0000ff) && close_to(g[128], 0x7f0080));
    CHECK((g[256] & 0xffffff) == 0xff0000);
    v[1].x = 1;
    CHECK(GradientFill(gd, v, 2, &r, 1, GRADIENT_FILL_RECT_V));
    CHECK((g[0] & 0xffffff) == 0xff0000 && close_to(g[256], 0x7f0080) && close_to(g[1], 0x7f0080) == 0);
    DeleteDC(gd);
    DeleteDC(s);
    DeleteDC(d);
}

/* EnumFontFamiliesExW's callback: counts calls; stops when asked. */
static int enum_calls, enum_stop;
static int CALLBACK enum_font(const LOGFONTW *lf, const TEXTMETRICW *tm, DWORD type, LPARAM param) {
    (void)tm;
    enum_calls++;
    if (param != 0x1234 || lf->lfFaceName[0] == 0 || type == 0) return 0;
    return enum_stop ? 0 : 7;
}

static POINT dda[16];
static int dda_n;
static void CALLBACK dda_point(int x, int y, LPARAM data) {
    if (data == 0x55 && dda_n < 16) {
        dda[dda_n].x = x;
        dda[dda_n].y = y;
    }
    dda_n++;
}

/* SaveDC/RestoreDC, the DC brush and pen colours, ANSI fonts, widths,
 * font data and enumeration (what Chromium delay-imports). */
static void dc_state(void) {
    HDC screen = GetDC(NULL);
    HDC dc = CreateCompatibleDC(screen);
    CHECK(SetTextColor(dc, RGB(1, 2, 3)) != CLR_INVALID);
    CHECK(SaveDC(dc) == 1);
    SetTextColor(dc, RGB(4, 5, 6));
    CHECK(SaveDC(dc) == 2);
    SetTextColor(dc, RGB(7, 8, 9));
    CHECK(RestoreDC(dc, -1) && GetTextColor(dc) == RGB(4, 5, 6));
    CHECK(SaveDC(dc) == 2);
    CHECK(RestoreDC(dc, 1) && GetTextColor(dc) == RGB(1, 2, 3));
    CHECK(!RestoreDC(dc, 1) && !RestoreDC(dc, 0) && !RestoreDC(dc, -1));

    CHECK(SetDCBrushColor(dc, RGB(10, 20, 30)) == RGB(255, 255, 255));
    CHECK(SetDCBrushColor(dc, RGB(1, 1, 1)) == RGB(10, 20, 30));
    CHECK(SetDCPenColor(dc, RGB(40, 50, 60)) == RGB(0, 0, 0));
    CHECK(SetDCPenColor(dc, RGB(0, 0, 0)) == RGB(40, 50, 60));
    CHECK(SetDCBrushColor(NULL, 0) == CLR_INVALID);

    HFONT font = CreateFontA(-16, 0, 0, 0, FW_NORMAL, 0, 0, 0, DEFAULT_CHARSET, 0, 0, 0, 0, "Arial");
    CHECK(font != NULL);
    HGDIOBJ old = SelectObject(dc, font);
    char face[32];
    CHECK(GetTextFaceA(dc, sizeof face, face) == 6 && strcmp(face, "Arial") == 0);
    CHECK(GetTextFaceA(dc, 0, NULL) == 6);
    INT widths[2] = {0, 0};
    SIZE one;
    CHECK(GetCharWidthW(dc, 'A', 'B', widths) && widths[0] > 0 && widths[1] > 0);
    CHECK(GetTextExtentPoint32W(dc, L"A", 1, &one) && one.cx == widths[0]);
    CHECK(!GetCharWidthW(dc, 'B', 'A', widths));

    DWORD whole = GetFontData(dc, 0, 0, NULL, 0);
    CHECK(whole != GDI_ERROR && whole > 1000);
    DWORD head = 'h' | 'e' << 8 | 'a' << 16 | (DWORD)'d' << 24;
    BYTE version[4];
    CHECK(GetFontData(dc, head, 0, version, 4) == 4);
    CHECK(version[0] == 0 && version[1] == 1 && version[2] == 0 && version[3] == 0);
    CHECK(GetFontData(dc, head, 0, NULL, 0) >= 54); /* head's size */
    CHECK(GetFontData(dc, 'z' | 'z' << 8 | 'z' << 16 | (DWORD)'z' << 24, 0, NULL, 0) == GDI_ERROR);

    LOGFONTW any = {0};
    any.lfCharSet = DEFAULT_CHARSET;
    CHECK(EnumFontFamiliesExW(dc, &any, enum_font, 0x1234, 0) == 7 && enum_calls >= 1);
    enum_calls = 0;
    enum_stop = 1;
    CHECK(EnumFontFamiliesExW(dc, &any, enum_font, 0x1234, 0) == 0 && enum_calls == 1);
    lstrcpyW(any.lfFaceName, L"No Such Font Family");
    enum_calls = 0;
    EnumFontFamiliesExW(dc, &any, enum_font, 0x1234, 0);
    CHECK(enum_calls == 0);
    enum_calls = 0;
    enum_stop = 0;
    CHECK(EnumFontsW(dc, NULL, (FONTENUMPROCW)enum_font, 0x1234) == 7 && enum_calls >= 1);
    enum_calls = 0;
    EnumFontsW(dc, L"No Such Font Family", (FONTENUMPROCW)enum_font, 0x1234);
    CHECK(enum_calls == 0);

    SIZE old_size, new_size;
    CHECK(GetTextExtentPointW(dc, L"Setup", 5, &old_size) && GetTextExtentPoint32W(dc, L"Setup", 5, &new_size));
    CHECK(old_size.cx == new_size.cx && old_size.cy == new_size.cy && old_size.cx > 0);
    LOGPEN lp = {PS_SOLID, {2, 0}, RGB(1, 2, 3)};
    HPEN pen = CreatePenIndirect(&lp);
    CHECK(pen != NULL && GetObjectType(pen) == OBJ_PEN && DeleteObject(pen));
    HBRUSH brush = CreateSolidBrush(RGB(9, 9, 9));
    CHECK(UnrealizeObject(brush) && DeleteObject(brush));
    HBITMAP canvas = CreateCompatibleBitmap(screen, 64, 64);
    HGDIOBJ old_bitmap = SelectObject(dc, canvas);
    RECT inside = {10, 10, 20, 20}, outside = {100, 100, 120, 120};
    CHECK(RectVisible(dc, &inside) && !RectVisible(dc, &outside));
    SelectObject(dc, old_bitmap);
    DeleteObject(canvas);
    dda_n = 0;
    CHECK(LineDDA(0, 0, 3, 0, dda_point, 0x55) && dda_n == 3);
    CHECK(dda[0].x == 0 && dda[1].x == 1 && dda[2].x == 2 && dda[2].y == 0);
    dda_n = 0;
    CHECK(LineDDA(5, 5, 2, 2, dda_point, 0x55) && dda_n == 3);
    CHECK(dda[0].x == 5 && dda[0].y == 5 && dda[2].x == 3 && dda[2].y == 3);
    dda_n = 0;
    CHECK(LineDDA(0, 0, 0, 4, dda_point, 0x55) && dda_n == 4 && dda[3].x == 0 && dda[3].y == 3);

    SelectObject(dc, old);
    DeleteObject(font);
    DeleteDC(dc);
    ReleaseDC(NULL, screen);
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
    regions();
    shapes();
    palettes();
    dibs();
    msimg();
    dc_state();
    printf("gdi32_test: ok\n");
    return 0;
}
