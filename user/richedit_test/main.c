/* Rich edit controls (msftedit.dll's RICHEDIT50W, riched20.dll's
 * RichEdit20W and RichEdit20A) as Microsoft documents them ("Rich Edit
 * Controls" and the EM_ messages on learn.microsoft.com) and as Wine's
 * conformance tests (run on real Windows) observe them; what wxWidgets'
 * wxTextCtrl with wxTE_RICH2 (FileZilla's message log) uses:
 *
 * - The text keeps paragraphs with a CR each: WM_SETTEXT and
 *   EM_REPLACESEL turn CRLF (and a lone LF) into CR; WM_GETTEXT and
 *   WM_GETTEXTLENGTH give CRLF; EM_GETTEXTEX gives CR, or CRLF with
 *   GT_USECRLF, in Unicode (1200) or the ANSI code page;
 *   EM_GETTEXTLENGTHEX counts CRs (GTL_USECRLF: CRLFs; GTL_NUMBYTES:
 *   bytes); EM_GETTEXTRANGE and EM_GETSELTEXT copy CR text.
 * - Lines: EM_GETLINECOUNT (a text ending in a paragraph mark has an
 *   empty last line), EM_LINEINDEX, EM_LINEFROMCHAR, EM_EXLINEFROMCHAR,
 *   EM_LINELENGTH, EM_GETLINE (the line with its CR, no NUL).
 * - The selection: EM_SETSEL/EM_GETSEL, EM_EXSETSEL/EM_EXGETSEL,
 *   EM_REPLACESEL (the caret after the new text).
 * - Character formats: EM_SETCHARFORMAT with SCF_SELECTION (on an empty
 *   selection: what EM_REPLACESEL inserts next), SCF_ALL and SCF_DEFAULT,
 *   CHARFORMAT2W colours and effects; EM_GETCHARFORMAT reads them back,
 *   its mask leaving out what differs across the selection; a bad cbSize
 *   fails; WM_SETFONT sets the default face and size.
 * - EM_SETBKGNDCOLOR returns the old colour; the control paints its
 *   background and each run in its colour.
 * - Notifications: none until EM_SETEVENTMASK asks; then EN_UPDATE and
 *   EN_CHANGE (WM_COMMAND) for EM_REPLACESEL, typing and WM_SETTEXT
 *   (while EN_CHANGE from WM_SETTEXT is sent, EM_GETEVENTMASK leaves
 *   ENM_CHANGE out), EN_SELCHANGE (WM_NOTIFY) for selection changes,
 *   EN_LINK for a click on an automatically detected URL
 *   (EM_AUTOURLDETECT, which also gives it CFE_LINK).
 * - EM_SETREADONLY and EM_SETOPTIONS (ECO_READONLY is ES_READONLY),
 *   EM_EXLIMITTEXT/EM_LIMITTEXT/EM_GETLIMITTEXT (default 32767, 0: 64K).
 * - Scrolling: EM_GETFIRSTVISIBLELINE, EM_LINESCROLL, EM_SCROLLCARET,
 *   WM_VSCROLL SB_TOP/SB_BOTTOM, the vertical scroll bar's position.
 * - EM_SETPARAFORMAT/EM_GETPARAFORMAT tab stops (twips: a stop at 1440
 *   puts the text after a tab an inch, 96 pixels, from the line's start).
 * - EM_SETTEXTMODE only on an empty control; EM_GETTEXTMODE.
 * - Keys: the arrows, Home/End, Backspace, Delete, Enter.
 *
 * Prints "richedit_test: ok" or exits with the number of the failing
 * check. */
#include <windows.h>
#include <richedit.h>
#include <stdio.h>
#include <string.h>

static int check;
#define CHECK(cond) do { check++; if (!(cond)) { printf("richedit_test: check %d (line %d) failed\n", check, __LINE__); fflush(stdout); ExitProcess(check); } } while (0)

#define ID 77
/* The edit's WM_COMMAND codes and WM_NOTIFY codes, in order. */
static int codes[64], ncodes;
static int notes[64], nnotes;
static CHARRANGE last_sel;
static WORD last_seltyp;
static ENLINK last_link;
/* EM_GETEVENTMASK asked during the last EN_CHANGE. */
static LRESULT mask_during_change = -1;

static LRESULT CALLBACK parent_proc(HWND hwnd, UINT msg, WPARAM wparam, LPARAM lparam) {
    if (msg == WM_COMMAND && LOWORD(wparam) == ID) {
        if (ncodes < 64)
            codes[ncodes++] = HIWORD(wparam);
        if (HIWORD(wparam) == EN_CHANGE)
            mask_during_change = SendMessageW((HWND)lparam, EM_GETEVENTMASK, 0, 0);
    }
    if (msg == WM_NOTIFY && ((NMHDR *)lparam)->idFrom == ID) {
        NMHDR *h = (NMHDR *)lparam;
        if (nnotes < 64)
            notes[nnotes++] = h->code;
        if (h->code == EN_SELCHANGE) {
            last_sel = ((SELCHANGE *)lparam)->chrg;
            last_seltyp = ((SELCHANGE *)lparam)->seltyp;
        }
        if (h->code == EN_LINK)
            last_link = *(ENLINK *)lparam;
    }
    return DefWindowProcW(hwnd, msg, wparam, lparam);
}

static void reset_codes(void) {
    ncodes = 0;
    nnotes = 0;
}

static int has_code(int code) {
    for (int i = 0; i < ncodes; i++)
        if (codes[i] == code)
            return i + 1;
    return 0;
}

static int has_note(int code) {
    for (int i = 0; i < nnotes; i++)
        if (notes[i] == code)
            return 1;
    return 0;
}

static LRESULT length_ex(HWND e, DWORD flags, UINT codepage) {
    GETTEXTLENGTHEX g = {flags, codepage};
    return SendMessageW(e, EM_GETTEXTLENGTHEX, (WPARAM)&g, 0);
}

static LRESULT text_ex(HWND e, DWORD flags, WCHAR *buf, int chars) {
    GETTEXTEX g = {0};
    g.cb = chars * sizeof(WCHAR);
    g.flags = flags;
    g.codepage = 1200;
    return SendMessageW(e, EM_GETTEXTEX, (WPARAM)&g, (LPARAM)buf);
}

static void set_sel(HWND e, LONG a, LONG b) {
    CHARRANGE r = {a, b};
    SendMessageW(e, EM_EXSETSEL, 0, (LPARAM)&r);
}

static CHARRANGE get_sel(HWND e) {
    CHARRANGE r = {-5, -5};
    SendMessageW(e, EM_EXGETSEL, 0, (LPARAM)&r);
    return r;
}

static DWORD get_format(HWND e, DWORD which, CHARFORMAT2W *cf) {
    memset(cf, 0, sizeof *cf);
    cf->cbSize = sizeof *cf;
    return (DWORD)SendMessageW(e, EM_GETCHARFORMAT, which, (LPARAM)cf);
}

static LRESULT set_color(HWND e, DWORD which, COLORREF color) {
    CHARFORMAT2W cf = {0};
    cf.cbSize = sizeof cf;
    cf.dwMask = CFM_COLOR;
    cf.crTextColor = color;
    return SendMessageW(e, EM_SETCHARFORMAT, which, (LPARAM)&cf);
}

static HWND make(HWND parent, const WCHAR *cls, DWORD style, int h) {
    return CreateWindowExW(0, cls, L"", WS_CHILD | WS_VISIBLE | style, 10, 10, 360, h, parent, (HMENU)ID,
                           GetModuleHandleW(NULL), NULL);
}

static void text_and_lines(HWND e) {
    WCHAR buf[64];
    CHECK(SendMessageW(e, WM_SETTEXT, 0, (LPARAM)L"Hello\r\nworld") == 1);
    CHECK(SendMessageW(e, WM_GETTEXTLENGTH, 0, 0) == 12);
    CHECK(SendMessageW(e, WM_GETTEXT, 64, (LPARAM)buf) == 12 && wcscmp(buf, L"Hello\r\nworld") == 0);
    // A buffer too small: what fits, terminated.
    CHECK(SendMessageW(e, WM_GETTEXT, 4, (LPARAM)buf) == 3 && wcscmp(buf, L"Hel") == 0);
    CHECK(length_ex(e, GTL_DEFAULT, 1200) == 11);
    CHECK(length_ex(e, GTL_USECRLF, 1200) == 12);
    CHECK(length_ex(e, GTL_NUMBYTES, 1200) == 22);
    CHECK(text_ex(e, GT_DEFAULT, buf, 64) == 11 && wcscmp(buf, L"Hello\rworld") == 0);
    CHECK(text_ex(e, GT_USECRLF, buf, 64) == 12 && wcscmp(buf, L"Hello\r\nworld") == 0);
    {
        char a[64];
        GETTEXTEX g = {sizeof a, GT_DEFAULT, CP_ACP, NULL, NULL};
        CHECK(SendMessageW(e, EM_GETTEXTEX, (WPARAM)&g, (LPARAM)a) == 11 && strcmp(a, "Hello\rworld") == 0);
    }
    CHECK(SendMessageW(e, EM_GETLINECOUNT, 0, 0) == 2);
    CHECK(SendMessageW(e, EM_LINEINDEX, 0, 0) == 0);
    CHECK(SendMessageW(e, EM_LINEINDEX, 1, 0) == 6);
    CHECK(SendMessageW(e, EM_LINEINDEX, 2, 0) == -1);
    CHECK(SendMessageW(e, EM_LINEFROMCHAR, 7, 0) == 1);
    CHECK(SendMessageW(e, EM_LINEFROMCHAR, 3, 0) == 0);
    CHECK(SendMessageW(e, EM_EXLINEFROMCHAR, 0, 8) == 1);
    CHECK(SendMessageW(e, EM_LINELENGTH, 6, 0) == 5);
    CHECK(SendMessageW(e, EM_LINELENGTH, 2, 0) == 5);
    // EM_GETLINE: the buffer's size in its first WORD; the line with its
    // paragraph mark, not terminated.
    memset(buf, 0x55, sizeof buf);
    *(WORD *)buf = 20;
    CHECK(SendMessageW(e, EM_GETLINE, 0, (LPARAM)buf) == 6 && memcmp(buf, L"Hello\r", 12) == 0);
    *(WORD *)buf = 20;
    CHECK(SendMessageW(e, EM_GETLINE, 1, (LPARAM)buf) == 5 && memcmp(buf, L"world", 10) == 0);
    *(WORD *)buf = 3;
    CHECK(SendMessageW(e, EM_GETLINE, 0, (LPARAM)buf) == 3 && memcmp(buf, L"Hel", 6) == 0);
    // A lone LF is a paragraph mark too; a final one makes an empty line.
    SendMessageW(e, WM_SETTEXT, 0, (LPARAM)L"a\nb");
    CHECK(length_ex(e, GTL_DEFAULT, 1200) == 3);
    CHECK(SendMessageW(e, EM_GETLINECOUNT, 0, 0) == 2);
    SendMessageW(e, WM_SETTEXT, 0, (LPARAM)L"abc\r");
    CHECK(SendMessageW(e, EM_GETLINECOUNT, 0, 0) == 2);
    CHECK(SendMessageW(e, EM_LINEINDEX, 1, 0) == 4);
    CHECK(SendMessageW(e, WM_GETTEXT, 64, (LPARAM)buf) == 5 && wcscmp(buf, L"abc\r\n") == 0);
    SendMessageW(e, WM_SETTEXT, 0, (LPARAM)L"");
    CHECK(SendMessageW(e, EM_GETLINECOUNT, 0, 0) == 1);
    CHECK(SendMessageW(e, WM_GETTEXTLENGTH, 0, 0) == 0);
}

static void selection(HWND e) {
    WCHAR buf[64];
    SendMessageW(e, WM_SETTEXT, 0, (LPARAM)L"Hello\r\nworld");
    // WM_SETTEXT puts the caret at the start.
    CHARRANGE r = get_sel(e);
    CHECK(r.cpMin == 0 && r.cpMax == 0);
    SendMessageW(e, EM_SETSEL, 1, 3);
    DWORD a = 9, b = 9;
    CHECK(SendMessageW(e, EM_GETSEL, (WPARAM)&a, (LPARAM)&b) == MAKELONG(1, 3) && a == 1 && b == 3);
    r = get_sel(e);
    CHECK(r.cpMin == 1 && r.cpMax == 3);
    // Reversed: the same range.
    set_sel(e, 4, 2);
    r = get_sel(e);
    CHECK(r.cpMin == 2 && r.cpMax == 4);
    CHECK(SendMessageW(e, EM_GETSELTEXT, 0, (LPARAM)buf) == 2 && wcscmp(buf, L"ll") == 0);
    // Past the end: clamped to it.
    set_sel(e, 8, 100);
    r = get_sel(e);
    CHECK(r.cpMin == 8 && r.cpMax >= 11);
    // EM_REPLACESEL: CRLF becomes CR; the caret after the new text.
    set_sel(e, 5, 5);
    SendMessageW(e, EM_REPLACESEL, TRUE, (LPARAM)L",");
    r = get_sel(e);
    CHECK(r.cpMin == 6 && r.cpMax == 6);
    CHECK(text_ex(e, GT_DEFAULT, buf, 64) == 12 && wcscmp(buf, L"Hello,\rworld") == 0);
    set_sel(e, 0, 5);
    SendMessageW(e, EM_REPLACESEL, TRUE, (LPARAM)L"x\r\ny");
    CHECK(text_ex(e, GT_DEFAULT, buf, 64) == 10 && wcscmp(buf, L"x\ry,\rworld") == 0);
    CHECK(SendMessageW(e, EM_GETLINECOUNT, 0, 0) == 3);
    TEXTRANGEW tr = {{2, 4}, buf};
    CHECK(SendMessageW(e, EM_GETTEXTRANGE, 0, (LPARAM)&tr) == 2 && wcscmp(buf, L"y,") == 0);
    tr.chrg.cpMin = 5;
    tr.chrg.cpMax = -1;
    CHECK(SendMessageW(e, EM_GETTEXTRANGE, 0, (LPARAM)&tr) == 5 && wcscmp(buf, L"world") == 0);
}

static void formats(HWND e) {
    CHARFORMAT2W cf;
    SendMessageW(e, WM_SETTEXT, 0, (LPARAM)L"red green blue");
    set_sel(e, 0, 3);
    memset(&cf, 0, sizeof cf);
    cf.cbSize = sizeof cf;
    cf.dwMask = CFM_COLOR | CFM_BOLD;
    cf.dwEffects = CFE_BOLD;
    cf.crTextColor = RGB(255, 0, 0);
    CHECK(SendMessageW(e, EM_SETCHARFORMAT, SCF_SELECTION, (LPARAM)&cf) != 0);
    set_sel(e, 4, 9);
    cf.dwMask = CFM_COLOR | CFM_ITALIC;
    cf.dwEffects = CFE_ITALIC;
    cf.crTextColor = RGB(0, 128, 0);
    CHECK(SendMessageW(e, EM_SETCHARFORMAT, SCF_SELECTION, (LPARAM)&cf) != 0);
    // A bad size fails.
    cf.cbSize = 10;
    CHECK(SendMessageW(e, EM_SETCHARFORMAT, SCF_SELECTION, (LPARAM)&cf) == 0);
    set_sel(e, 0, 3);
    DWORD mask = get_format(e, SCF_SELECTION, &cf);
    CHECK(mask & CFM_COLOR && mask & CFM_BOLD && cf.dwMask == mask);
    CHECK(cf.crTextColor == RGB(255, 0, 0) && !(cf.dwEffects & CFE_AUTOCOLOR));
    CHECK(cf.dwEffects & CFE_BOLD && !(cf.dwEffects & CFE_ITALIC));
    set_sel(e, 5, 7);
    get_format(e, SCF_SELECTION, &cf);
    CHECK(cf.crTextColor == RGB(0, 128, 0) && cf.dwEffects & CFE_ITALIC && !(cf.dwEffects & CFE_BOLD));
    // Across both: colour, bold and italic differ.
    set_sel(e, 0, 9);
    mask = get_format(e, SCF_SELECTION, &cf);
    CHECK(!(mask & CFM_COLOR) && !(mask & CFM_BOLD) && !(mask & CFM_ITALIC) && mask & CFM_FACE);
    // Untouched text: the automatic colour.
    set_sel(e, 10, 14);
    get_format(e, SCF_SELECTION, &cf);
    CHECK(cf.dwEffects & CFE_AUTOCOLOR && !(cf.dwEffects & CFE_BOLD));
    // SCF_ALL: the face and size everywhere, colours kept.
    memset(&cf, 0, sizeof cf);
    cf.cbSize = sizeof cf;
    cf.dwMask = CFM_FACE | CFM_SIZE;
    wcscpy(cf.szFaceName, L"Courier New");
    cf.yHeight = 240;
    CHECK(SendMessageW(e, EM_SETCHARFORMAT, SCF_ALL, (LPARAM)&cf) != 0);
    set_sel(e, 0, 3);
    get_format(e, SCF_SELECTION, &cf);
    CHECK(wcscmp(cf.szFaceName, L"Courier New") == 0 && cf.yHeight == 240 && cf.crTextColor == RGB(255, 0, 0));
    set_sel(e, 11, 12);
    get_format(e, SCF_SELECTION, &cf);
    CHECK(wcscmp(cf.szFaceName, L"Courier New") == 0 && cf.yHeight == 240);
    // SCF_DEFAULT: the default format.
    CHECK(set_color(e, SCF_DEFAULT, RGB(0, 0, 255)) != 0);
    get_format(e, SCF_DEFAULT, &cf);
    CHECK(cf.crTextColor == RGB(0, 0, 255) && !(cf.dwEffects & CFE_AUTOCOLOR));
    // A smaller CHARFORMATW works as well.
    {
        CHARFORMATW small = {0};
        small.cbSize = sizeof small;
        set_sel(e, 0, 3);
        CHECK(SendMessageW(e, EM_GETCHARFORMAT, SCF_SELECTION, (LPARAM)&small) & CFM_COLOR);
        CHECK(small.crTextColor == RGB(255, 0, 0));
    }
    // An empty selection's format is what EM_REPLACESEL inserts next (a
    // log appending coloured lines).
    SendMessageW(e, WM_SETTEXT, 0, (LPARAM)L"");
    set_color(e, SCF_SELECTION, RGB(255, 0, 0));
    SendMessageW(e, EM_REPLACESEL, FALSE, (LPARAM)L"abc\r\n");
    set_sel(e, 4, 4);
    set_color(e, SCF_SELECTION, RGB(0, 0, 255));
    SendMessageW(e, EM_REPLACESEL, FALSE, (LPARAM)L"def");
    set_sel(e, 0, 3);
    get_format(e, SCF_SELECTION, &cf);
    CHECK(cf.crTextColor == RGB(255, 0, 0));
    set_sel(e, 4, 7);
    get_format(e, SCF_SELECTION, &cf);
    CHECK(cf.crTextColor == RGB(0, 0, 255));
    // Typing continues in the format before the caret.
    set_sel(e, 2, 2);
    SendMessageW(e, EM_REPLACESEL, FALSE, (LPARAM)L"Z");
    set_sel(e, 2, 3);
    get_format(e, SCF_SELECTION, &cf);
    CHECK(cf.crTextColor == RGB(255, 0, 0));
    // WM_SETFONT: the default face and size from the font.
    LOGFONTW lf = {0};
    lf.lfHeight = -16;
    lf.lfWeight = FW_NORMAL;
    wcscpy(lf.lfFaceName, L"Arial");
    HFONT font = CreateFontIndirectW(&lf);
    SendMessageW(e, WM_SETFONT, (WPARAM)font, TRUE);
    get_format(e, SCF_DEFAULT, &cf);
    CHECK(wcscmp(cf.szFaceName, L"Arial") == 0 && cf.yHeight == 240);
}

static int count_near(HWND e, COLORREF want, RECT box) {
    HDC dc = GetDC(e);
    int n = 0;
    for (int y = box.top; y < box.bottom; y++)
        for (int x = box.left; x < box.right; x++) {
            COLORREF c = GetPixel(dc, x, y);
            if (abs(GetRValue(c) - GetRValue(want)) < 60 && abs(GetGValue(c) - GetGValue(want)) < 60 &&
                abs(GetBValue(c) - GetBValue(want)) < 60)
                n++;
        }
    ReleaseDC(e, dc);
    return n;
}

static void painting(HWND e) {
    CHECK(SendMessageW(e, EM_SETBKGNDCOLOR, 0, RGB(0, 0, 96)) == GetSysColor(COLOR_WINDOW));
    SendMessageW(e, WM_SETTEXT, 0, (LPARAM)L"");
    set_color(e, SCF_SELECTION, RGB(255, 64, 64));
    SendMessageW(e, EM_REPLACESEL, FALSE, (LPARAM)L"WWWWWW\r\n");
    set_color(e, SCF_SELECTION, RGB(64, 255, 64));
    SendMessageW(e, EM_REPLACESEL, FALSE, (LPARAM)L"WWWWWW");
    SendMessageW(e, EM_SETSEL, 0, 0);
    RedrawWindow(e, NULL, NULL, RDW_INVALIDATE | RDW_ERASE | RDW_UPDATENOW);
    RECT client;
    GetClientRect(e, &client);
    // The background below the text, the first line red, the second green.
    POINTL p1 = {0, 0}, p2 = {0, 0};
    SendMessageW(e, EM_POSFROMCHAR, (WPARAM)&p1, 0);
    SendMessageW(e, EM_POSFROMCHAR, (WPARAM)&p2, 7);
    CHECK(p2.y > p1.y && p2.x == p1.x);
    RECT bottom = {client.left + 4, client.bottom - 30, client.right - 30, client.bottom - 4};
    CHECK(count_near(e, RGB(0, 0, 96), bottom) == (bottom.right - bottom.left) * (bottom.bottom - bottom.top));
    RECT first = {p1.x, p1.y, p1.x + 60, p2.y};
    RECT second = {p2.x, p2.y, p2.x + 60, p2.y + (p2.y - p1.y)};
    CHECK(count_near(e, RGB(255, 64, 64), first) > 10 && count_near(e, RGB(64, 255, 64), first) == 0);
    CHECK(count_near(e, RGB(64, 255, 64), second) > 10 && count_near(e, RGB(255, 64, 64), second) == 0);
    CHECK(SendMessageW(e, EM_SETBKGNDCOLOR, 1, 0) == RGB(0, 0, 96));
}

static void notifications(HWND e) {
    WCHAR buf[64];
    CHECK(SendMessageW(e, EM_GETEVENTMASK, 0, 0) == 0);
    SendMessageW(e, WM_SETTEXT, 0, (LPARAM)L"abc");
    reset_codes();
    set_sel(e, 3, 3);
    SendMessageW(e, EM_REPLACESEL, TRUE, (LPARAM)L"d");
    CHECK(ncodes == 0 && nnotes == 0);
    CHECK(SendMessageW(e, EM_SETEVENTMASK, 0, ENM_CHANGE | ENM_UPDATE) == 0);
    CHECK(SendMessageW(e, EM_GETEVENTMASK, 0, 0) == (ENM_CHANGE | ENM_UPDATE));
    reset_codes();
    SendMessageW(e, EM_REPLACESEL, TRUE, (LPARAM)L"e");
    CHECK(has_code(EN_CHANGE) && has_code(EN_UPDATE) && has_code(EN_UPDATE) < has_code(EN_CHANGE));
    // While EN_CHANGE from WM_SETTEXT is sent, ENM_CHANGE is masked off.
    reset_codes();
    mask_during_change = -1;
    SendMessageW(e, WM_SETTEXT, 0, (LPARAM)L"new");
    CHECK(has_code(EN_CHANGE) && mask_during_change == ENM_UPDATE);
    CHECK(SendMessageW(e, EM_GETEVENTMASK, 0, 0) == (ENM_CHANGE | ENM_UPDATE));
    // Typing.
    SetFocus(e);
    set_sel(e, 3, 3);
    reset_codes();
    SendMessageW(e, WM_CHAR, 'z', 0);
    CHECK(has_code(EN_CHANGE));
    CHECK(SendMessageW(e, WM_GETTEXT, 64, (LPARAM)buf) == 4 && wcscmp(buf, L"newz") == 0);
    // EN_SELCHANGE.
    CHECK(SendMessageW(e, EM_SETEVENTMASK, 0, ENM_SELCHANGE) == (ENM_CHANGE | ENM_UPDATE));
    reset_codes();
    set_sel(e, 1, 2);
    CHECK(has_note(EN_SELCHANGE) && last_sel.cpMin == 1 && last_sel.cpMax == 2 && last_seltyp == SEL_TEXT);
    CHECK(ncodes == 0);
    reset_codes();
    SendMessageW(e, EM_REPLACESEL, TRUE, (LPARAM)L"q");
    CHECK(ncodes == 0);
    SendMessageW(e, EM_SETEVENTMASK, 0, 0);
}

static void read_only_and_limits(HWND e) {
    WCHAR buf[64];
    SendMessageW(e, WM_SETTEXT, 0, (LPARAM)L"abc");
    CHECK(SendMessageW(e, EM_SETREADONLY, TRUE, 0) != 0);
    CHECK(GetWindowLongW(e, GWL_STYLE) & ES_READONLY);
    CHECK(SendMessageW(e, EM_GETOPTIONS, 0, 0) & ECO_READONLY);
    SetFocus(e);
    set_sel(e, 3, 3);
    SendMessageW(e, WM_CHAR, 'x', 0);
    CHECK(SendMessageW(e, WM_GETTEXTLENGTH, 0, 0) == 3);
    SendMessageW(e, EM_SETREADONLY, FALSE, 0);
    CHECK(!(GetWindowLongW(e, GWL_STYLE) & ES_READONLY));
    LRESULT options = SendMessageW(e, EM_SETOPTIONS, ECOOP_OR, ECO_READONLY);
    CHECK(options & ECO_READONLY && SendMessageW(e, EM_GETOPTIONS, 0, 0) == options);
    CHECK(GetWindowLongW(e, GWL_STYLE) & ES_READONLY);
    options = SendMessageW(e, EM_SETOPTIONS, ECOOP_AND, ~(LPARAM)ECO_READONLY);
    CHECK(!(options & ECO_READONLY) && !(GetWindowLongW(e, GWL_STYLE) & ES_READONLY));
    // Limits.
    CHECK(SendMessageW(e, EM_GETLIMITTEXT, 0, 0) == 32767);
    SendMessageW(e, EM_EXLIMITTEXT, 0, 10);
    CHECK(SendMessageW(e, EM_GETLIMITTEXT, 0, 0) == 10);
    SendMessageW(e, WM_SETTEXT, 0, (LPARAM)L"");
    SendMessageW(e, EM_REPLACESEL, FALSE, (LPARAM)L"0123456789abcdef");
    CHECK(SendMessageW(e, WM_GETTEXT, 64, (LPARAM)buf) == 10 && wcscmp(buf, L"0123456789") == 0);
    SendMessageW(e, EM_LIMITTEXT, 5, 0);
    CHECK(SendMessageW(e, EM_GETLIMITTEXT, 0, 0) == 5);
    SendMessageW(e, EM_EXLIMITTEXT, 0, 0);
    CHECK(SendMessageW(e, EM_GETLIMITTEXT, 0, 0) == 65536);
}

static void scrolling(HWND e) {
    WCHAR text[1024];
    text[0] = 0;
    for (int i = 0; i < 40; i++) {
        WCHAR line[32];
        swprintf(line, 32, L"line %d\r\n", i);
        wcscat(text, line);
    }
    SendMessageW(e, WM_SETTEXT, 0, (LPARAM)text);
    CHECK(SendMessageW(e, EM_GETLINECOUNT, 0, 0) == 41);
    CHECK(SendMessageW(e, EM_GETFIRSTVISIBLELINE, 0, 0) == 0);
    CHECK(GetScrollPos(e, SB_VERT) == 0);
    CHECK(SendMessageW(e, EM_LINESCROLL, 0, 5) != 0);
    CHECK(SendMessageW(e, EM_GETFIRSTVISIBLELINE, 0, 0) == 5);
    CHECK(GetScrollPos(e, SB_VERT) > 0);
    SendMessageW(e, EM_LINESCROLL, 0, -2);
    CHECK(SendMessageW(e, EM_GETFIRSTVISIBLELINE, 0, 0) == 3);
    SendMessageW(e, WM_VSCROLL, SB_TOP, 0);
    CHECK(SendMessageW(e, EM_GETFIRSTVISIBLELINE, 0, 0) == 0);
    // The caret on line 38, scrolled into view.
    LRESULT at = SendMessageW(e, EM_LINEINDEX, 38, 0);
    CHECK(at > 0);
    set_sel(e, at, at);
    SendMessageW(e, EM_SCROLLCARET, 0, 0);
    LRESULT first = SendMessageW(e, EM_GETFIRSTVISIBLELINE, 0, 0);
    CHECK(first > 20 && first <= 38);
    SendMessageW(e, WM_VSCROLL, SB_TOP, 0);
    SendMessageW(e, WM_VSCROLL, SB_BOTTOM, 0);
    CHECK(SendMessageW(e, EM_GETFIRSTVISIBLELINE, 0, 0) > 20);
    SCROLLINFO si = {sizeof si, SIF_ALL};
    CHECK(GetScrollInfo(e, SB_VERT, &si) && si.nPage > 0 && si.nMax > (int)si.nPage && si.nPos > 0);
}

static void text_mode_and_urls(HWND e) {
    CHECK(SendMessageW(e, EM_GETTEXTMODE, 0, 0) == (TM_RICHTEXT | TM_MULTILEVELUNDO | TM_MULTICODEPAGE));
    SendMessageW(e, WM_SETTEXT, 0, (LPARAM)L"text");
    CHECK(SendMessageW(e, EM_SETTEXTMODE, TM_PLAINTEXT, 0) != 0);
    // URLs.
    CHECK(SendMessageW(e, EM_AUTOURLDETECT, TRUE, 0) == 0);
    CHECK(SendMessageW(e, EM_GETAUTOURLDETECT, 0, 0) != 0);
    SendMessageW(e, EM_SETEVENTMASK, 0, ENM_LINK);
    SendMessageW(e, WM_SETTEXT, 0, (LPARAM)L"see http://example.com/x now");
    CHARFORMAT2W cf;
    set_sel(e, 6, 8);
    get_format(e, SCF_SELECTION, &cf);
    CHECK(cf.dwEffects & CFE_LINK);
    set_sel(e, 0, 2);
    get_format(e, SCF_SELECTION, &cf);
    CHECK(!(cf.dwEffects & CFE_LINK));
    POINTL p = {0, 0};
    SendMessageW(e, EM_POSFROMCHAR, (WPARAM)&p, 8);
    reset_codes();
    SendMessageW(e, WM_LBUTTONDOWN, MK_LBUTTON, MAKELPARAM(p.x + 2, p.y + 3));
    SendMessageW(e, WM_LBUTTONUP, 0, MAKELPARAM(p.x + 2, p.y + 3));
    CHECK(has_note(EN_LINK));
    CHECK(last_link.chrg.cpMin == 4 && last_link.chrg.cpMax == 24 && last_link.msg == WM_LBUTTONUP);
    SendMessageW(e, EM_SETEVENTMASK, 0, 0);
    SendMessageW(e, EM_AUTOURLDETECT, FALSE, 0);
    // An empty control takes another text mode.
    SendMessageW(e, WM_SETTEXT, 0, (LPARAM)L"");
    CHECK(SendMessageW(e, EM_SETTEXTMODE, TM_PLAINTEXT, 0) == 0);
    LRESULT mode = SendMessageW(e, EM_GETTEXTMODE, 0, 0);
    CHECK(mode & TM_PLAINTEXT && !(mode & TM_RICHTEXT));
}

static void paragraphs(HWND e) {
    SendMessageW(e, WM_SETTEXT, 0, (LPARAM)L"a\tb");
    PARAFORMAT pf = {0};
    pf.cbSize = sizeof pf;
    pf.dwMask = PFM_TABSTOPS;
    pf.cTabCount = 2;
    pf.rgxTabs[0] = 1440;
    pf.rgxTabs[1] = 2880;
    CHECK(SendMessageW(e, EM_SETPARAFORMAT, 0, (LPARAM)&pf) != 0);
    PARAFORMAT got = {0};
    got.cbSize = sizeof got;
    CHECK(SendMessageW(e, EM_GETPARAFORMAT, 0, (LPARAM)&got) & PFM_TABSTOPS);
    CHECK(got.cTabCount == 2 && got.rgxTabs[0] == 1440 && got.rgxTabs[1] == 2880);
    // The tab stop at an inch: 96 pixels at 96 dpi.
    POINTL p0 = {0, 0}, p2 = {0, 0};
    SendMessageW(e, EM_POSFROMCHAR, (WPARAM)&p0, 0);
    SendMessageW(e, EM_POSFROMCHAR, (WPARAM)&p2, 2);
    CHECK(p2.x - p0.x == 96 && p2.y == p0.y);
    // A bad size fails.
    pf.cbSize = 10;
    CHECK(SendMessageW(e, EM_SETPARAFORMAT, 0, (LPARAM)&pf) == 0);
}

static void keys(HWND e) {
    WCHAR buf[64];
    SetFocus(e);
    SendMessageW(e, WM_SETTEXT, 0, (LPARAM)L"abc");
    set_sel(e, 3, 3);
    SendMessageW(e, WM_KEYDOWN, VK_LEFT, 0);
    CHARRANGE r = get_sel(e);
    CHECK(r.cpMin == 2 && r.cpMax == 2);
    SendMessageW(e, WM_KEYDOWN, VK_HOME, 0);
    r = get_sel(e);
    CHECK(r.cpMin == 0 && r.cpMax == 0);
    SendMessageW(e, WM_KEYDOWN, VK_DELETE, 0);
    CHECK(SendMessageW(e, WM_GETTEXT, 64, (LPARAM)buf) == 2 && wcscmp(buf, L"bc") == 0);
    SendMessageW(e, WM_KEYDOWN, VK_END, 0);
    r = get_sel(e);
    CHECK(r.cpMin == 2 && r.cpMax == 2);
    SendMessageW(e, WM_CHAR, '\b', 0);
    SendMessageW(e, WM_CHAR, '\r', 0);
    SendMessageW(e, WM_CHAR, 'd', 0);
    CHECK(SendMessageW(e, WM_GETTEXT, 64, (LPARAM)buf) == 4 && wcscmp(buf, L"b\r\nd") == 0);
    CHECK(SendMessageW(e, EM_GETLINECOUNT, 0, 0) == 2);
    // Up and Down between lines.
    SendMessageW(e, WM_KEYDOWN, VK_UP, 0);
    CHECK(SendMessageW(e, EM_LINEFROMCHAR, (WPARAM)-1, 0) == 0);
    SendMessageW(e, WM_KEYDOWN, VK_DOWN, 0);
    CHECK(SendMessageW(e, EM_LINEFROMCHAR, (WPARAM)-1, 0) == 1);
}

static void riched20(HWND parent) {
    WCHAR buf[64];
    CHECK(LoadLibraryW(L"riched20.dll") != NULL);
    HWND e = make(parent, L"RichEdit20W", ES_MULTILINE, 100);
    CHECK(e != NULL);
    CHECK(GetClassNameW(e, buf, 64) == 11 && wcscmp(buf, L"RichEdit20W") == 0);
    SendMessageW(e, WM_SETTEXT, 0, (LPARAM)L"one\r\ntwo");
    CHECK(SendMessageW(e, WM_GETTEXT, 64, (LPARAM)buf) == 8 && wcscmp(buf, L"one\r\ntwo") == 0);
    CHECK(length_ex(e, GTL_DEFAULT, 1200) == 7);
    CHECK(SendMessageW(e, EM_GETLINECOUNT, 0, 0) == 2);
    DestroyWindow(e);
    // The ANSI class (an ANSI window whichever function made it) takes
    // and gives ANSI text.
    e = CreateWindowExW(0, L"RichEdit20A", L"", WS_CHILD | WS_VISIBLE | ES_MULTILINE, 10, 10, 300, 100, parent,
                        (HMENU)ID, GetModuleHandleW(NULL), NULL);
    CHECK(e != NULL);
    char a[64];
    SendMessageA(e, WM_SETTEXT, 0, (LPARAM) "abc\r\ndef");
    CHECK(SendMessageA(e, WM_GETTEXT, 64, (LPARAM)a) == 8 && strcmp(a, "abc\r\ndef") == 0);
    CHECK(SendMessageA(e, WM_GETTEXTLENGTH, 0, 0) == 8);
    DestroyWindow(e);
}

int main(void) {
    WNDCLASSW wc = {0};
    wc.lpfnWndProc = parent_proc;
    wc.hInstance = GetModuleHandleW(NULL);
    wc.lpszClassName = L"RichEditTestParent";
    wc.hbrBackground = (HBRUSH)(COLOR_WINDOW + 1);
    CHECK(RegisterClassW(&wc) != 0);
    HWND parent = CreateWindowExW(0, L"RichEditTestParent", L"richedit", WS_OVERLAPPEDWINDOW | WS_VISIBLE, 50, 50,
                                  420, 320, NULL, NULL, GetModuleHandleW(NULL), NULL);
    CHECK(parent != NULL);
    // No class before the DLL is loaded.
    CHECK(CreateWindowExW(0, MSFTEDIT_CLASS, L"", WS_CHILD, 0, 0, 10, 10, parent, NULL, NULL, NULL) == NULL);
    CHECK(LoadLibraryW(L"msftedit.dll") != NULL);
    HWND e = make(parent, MSFTEDIT_CLASS, ES_MULTILINE | WS_VSCROLL | ES_AUTOVSCROLL, 200);
    CHECK(e != NULL);
    WCHAR name[32];
    CHECK(GetClassNameW(e, name, 32) == 11 && wcscmp(name, L"RICHEDIT50W") == 0);
    text_and_lines(e);
    selection(e);
    formats(e);
    painting(e);
    notifications(e);
    read_only_and_limits(e);
    scrolling(e);
    paragraphs(e);
    keys(e);
    text_mode_and_urls(e);
    DestroyWindow(e);
    riched20(parent);
    DestroyWindow(parent);
    printf("richedit_test: ok\n");
    return 0;
}
