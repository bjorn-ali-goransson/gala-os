/* mlang (Multi Language Support) as real Windows serves it: the class
 * object, the interfaces its object answers, the code-page tables and
 * the font code pages of the font a DC has selected (docs/ROADMAP.md OF).
 * Word's MSO asks CoCreateInstance for CLSID_CMultiLanguage
 * {275C23E2-3747-11D0-9FEA-00AA003F8646} on its way to laying text out,
 * and an unregistered class ends it.
 *
 * The answers here were measured on the pinned Windows 11 24H2 build
 * (26100) by a probe that makes the same calls. Where an answer depends
 * on what the machine has installed -- which fonts cover a script, which
 * code pages are present -- the check is on the shape and not on the
 * value, because gala ships one font and the code pages it converts are
 * its own (ARCHITECTURE "Multi Language Support").
 *
 * What Windows answers, and this checks:
 *
 * - The class object is served in-process only: CoGetClassObject for
 *   IClassFactory is S_OK, CLSCTX_LOCAL_SERVER is REGDB_E_CLASSNOTREG.
 *   The factory answers IUnknown and IClassFactory and nothing else
 *   (IMultiLanguage, IMarshal: E_NOINTERFACE); LockServer either way is
 *   S_OK; CreateInstance makes an object; with a non-NULL outer it makes
 *   one for IUnknown and answers CLASS_E_NOAGGREGATION for any other
 *   interface.
 * - Each creation is its own object (two CoCreateInstance calls are not
 *   the same pointer), and a fresh one has one reference.
 * - IMultiLanguage is the object itself; IMultiLanguage2,
 *   IMultiLanguage3, IMLangCodePages, IMLangFontLink, IMLangFontLink2
 *   and IMLangLineBreakConsole come back as their own pointers, each
 *   with its own one reference. IMLangConvertCharset, IMLangString and
 *   its two string flavours, the three enumerators, IDispatch, IMarshal,
 *   IClassFactory and a CLSID that is nobody: E_NOINTERFACE with the
 *   caller's pointer set to NULL. A NULL output is E_POINTER.
 * - CodePageToCodePages: the FONTSIGNATURE code-page bit of a code page
 *   that has one (1250..1258 are bits 1..8 in that order, 874 Thai, 932
 *   JIS, 936 simplified, 949 Wansung, 950 traditional); E_FAIL and a
 *   zeroed output for one that has none (0, 1, 437, 850, UTF-16, the
 *   ISO and EUC pages, UTF-7, UTF-8, a page that does not exist). A NULL
 *   output is S_OK.
 * - CodePagesToCodePage: the code page of the lowest bit, unless the
 *   default's own bit is in the mask, when the default wins; no bits at
 *   all answers the default, and E_FAIL only when there is neither (a
 *   zero mask with no default, a mask of bits that name no code page --
 *   FS_SYMBOL among them). A NULL output is S_OK.
 * - CodePageToScriptID: the script of a code page (Latin 1, 1250, 1254,
 *   1257 and 437 are all sidAsciiLatin; Greek, Cyrillic, Hebrew, Arabic,
 *   Thai and the four CJK pages their own); E_FAIL for one that has no
 *   script, and the caller's SCRIPT_ID is left as it was. A NULL output
 *   is E_INVALIDARG.
 * - GetFontCodePages answers the code pages of the font handed in, in a
 *   DC it can select it into: a Latin font has the Latin 1 bit. A NULL
 *   DC, a NULL font and a handle that is not a DC are each E_FAIL with a
 *   zeroed output; a NULL output is S_OK.
 * - MapFont maps the font the DC has selected, for a character and the
 *   code pages wanted: code pages that font already covers give a font
 *   back, and not the handle that went in. ReleaseFont takes it, a
 *   second release of it is S_OK, and so is releasing a font mlang never
 *   mapped. A zero mask and a NULL DC are E_FAIL, a NULL output is S_OK,
 *   releasing NULL is E_FAIL, and ResetFontMapping is S_OK.
 *
 * Measured and not checked here, because gala records them instead of
 * answering until a program is seen to need them (ROADMAP OF): the
 * per-character code pages (GetCharCodePages, GetStrCodePages) want the
 * encode tables of all fourteen code pages, which gala's NLS does not
 * have yet; GetFontUnicodeRanges wants a face's coverage, which gdi32
 * does not keep; GetScriptFontInfo wants the installed fonts by script;
 * and IMultiLanguage/2/3's conversions and enumerations want the MIME
 * database. Windows' answers to those, for whoever implements them:
 * GetCharCodePages('A') is every Latin and CJK page at once and an
 * unpaired surrogate is 0 with S_OK; GetStrCodePages walks the count it
 * was given and is E_INVALIDARG for a count of 0 or -1; GetCodePageInfo
 * for 1252 has family 1252, GDI charset 0, web and header charset
 * "Windows-1252" and body charset "iso-8859-1", and for 932 family 932
 * and GDI charset 128; GetCharsetInfo("utf-8") is code page 1200 with
 * internet encoding 65001, and a name that is nobody is E_FAIL;
 * IsConvertible is S_OK or S_FALSE; GetRfc1766FromLcid(0x409) is
 * "en-us"; GetNumberOfScripts is 41.
 */
#define CINTERFACE
#define COBJMACROS
#include <windows.h>
#include <objbase.h>
#include <mlang.h>
#include <stdio.h>

static int check;
#define CHECK(cond) do { check++; if (!(cond)) { \
        printf("mlang_test: check %d (line %d) failed\n", check, __LINE__); \
        fflush(stdout); ExitProcess(check); } } while (0)

#ifndef REGDB_E_CLASSNOTREG
#define REGDB_E_CLASSNOTREG ((HRESULT)0x80040154L)
#endif

static const GUID gIUnknown = {0x00000000,0x0000,0x0000,{0xC0,0x00,0x00,0x00,0x00,0x00,0x00,0x46}};
static const GUID gIClassFactory = {0x00000001,0x0000,0x0000,{0xC0,0x00,0x00,0x00,0x00,0x00,0x00,0x46}};
static const GUID gIMarshal = {0x00000003,0x0000,0x0000,{0xC0,0x00,0x00,0x00,0x00,0x00,0x00,0x46}};
static const GUID gIDispatch = {0x00020400,0x0000,0x0000,{0xC0,0x00,0x00,0x00,0x00,0x00,0x00,0x46}};
static const GUID gIMultiLanguage = {0x275c23e1,0x3747,0x11d0,{0x9f,0xea,0x00,0xaa,0x00,0x3f,0x86,0x46}};
static const GUID gIMultiLanguage2 = {0xDCCFC164,0x2B38,0x11d2,{0xB7,0xEC,0x00,0xC0,0x4F,0x8F,0x5D,0x9A}};
static const GUID gIMultiLanguage3 = {0x4e5868ab,0xb157,0x4623,{0x9a,0xcc,0x6a,0x1d,0x9c,0xae,0xbe,0x04}};
static const GUID gIMLangCodePages = {0x359F3443,0xBD4A,0x11D0,{0xB1,0x88,0x00,0xAA,0x00,0x38,0xC9,0x69}};
static const GUID gIMLangFontLink = {0x359F3441,0xBD4A,0x11D0,{0xB1,0x88,0x00,0xAA,0x00,0x38,0xC9,0x69}};
static const GUID gIMLangFontLink2 = {0xDCCFC162,0x2B38,0x11d2,{0xB7,0xEC,0x00,0xC0,0x4F,0x8F,0x5D,0x9A}};
static const GUID gIMLangLineBreakConsole = {0xF5BE2EE1,0xBFD7,0x11D0,{0xB1,0x88,0x00,0xAA,0x00,0x38,0xC9,0x69}};
static const GUID gIMLangConvertCharset = {0xd66d6f98,0xcdaa,0x11d0,{0xb8,0x22,0x00,0xc0,0x4f,0xc9,0xb3,0x1f}};
static const GUID gIMLangString = {0xC04D65CE,0xB70D,0x11D0,{0xB1,0x88,0x00,0xAA,0x00,0x38,0xC9,0x69}};
static const GUID gIMLangStringWStr = {0xC04D65D0,0xB70D,0x11D0,{0xB1,0x88,0x00,0xAA,0x00,0x38,0xC9,0x69}};
static const GUID gIMLangStringAStr = {0xC04D65D2,0xB70D,0x11D0,{0xB1,0x88,0x00,0xAA,0x00,0x38,0xC9,0x69}};
static const GUID gIEnumCodePage = {0x275c23e3,0x3747,0x11d0,{0x9f,0xea,0x00,0xaa,0x00,0x3f,0x86,0x46}};
static const GUID gIEnumRfc1766 = {0x3dc39d1d,0xc030,0x11d0,{0xb8,0x1b,0x00,0xc0,0x4f,0xc9,0xb3,0x1f}};
static const GUID gIEnumScript = {0xAE5F1430,0x388B,0x11d2,{0x83,0x80,0x00,0xC0,0x4F,0x8F,0x5D,0xA1}};
static const GUID gBogus = {0x11111111,0x2222,0x3333,{0x44,0x44,0x55,0x55,0x55,0x55,0x55,0x55}};
static const GUID gCLSID = {0x275c23e2,0x3747,0x11d0,{0x9f,0xea,0x00,0xaa,0x00,0x3f,0x86,0x46}};

/* AddRef's answer, without keeping the reference. */
static ULONG refs(IUnknown *p)
{
    ULONG n = IUnknown_AddRef(p);
    IUnknown_Release(p);
    return n;
}

/* The class object: in-process only, a factory and nothing else. */
static void class_object(void)
{
    IClassFactory *fac = NULL;
    IUnknown *p = NULL;
    HRESULT hr;

    hr = CoGetClassObject(&gCLSID, CLSCTX_INPROC_SERVER, NULL, &gIClassFactory, (void **)&fac);
    CHECK(hr == S_OK && fac != NULL);

    CHECK(IClassFactory_QueryInterface(fac, &gIUnknown, (void **)&p) == S_OK && p != NULL);
    IUnknown_Release(p);
    p = NULL;
    CHECK(IClassFactory_QueryInterface(fac, &gIMultiLanguage, (void **)&p) == E_NOINTERFACE);
    CHECK(p == NULL);
    CHECK(IClassFactory_QueryInterface(fac, &gIMarshal, (void **)&p) == E_NOINTERFACE);
    CHECK(p == NULL);

    CHECK(IClassFactory_LockServer(fac, TRUE) == S_OK);
    CHECK(IClassFactory_LockServer(fac, FALSE) == S_OK);

    CHECK(IClassFactory_CreateInstance(fac, NULL, &gIMLangFontLink2, (void **)&p) == S_OK);
    CHECK(p != NULL);
    IUnknown_Release(p);

    /* Aggregation: the inner IUnknown is served, any other interface is not.
       The outer is any IUnknown the factory is not asked to call: its own. */
    {
        IUnknown *outer = (IUnknown *)fac;
        void *inner = NULL;
        hr = IClassFactory_CreateInstance(fac, outer, &gIUnknown, &inner);
        CHECK(hr == S_OK && inner != NULL);
        IUnknown_Release((IUnknown *)inner);
        inner = NULL;
        hr = IClassFactory_CreateInstance(fac, outer, &gIMLangFontLink2, &inner);
        CHECK(hr == CLASS_E_NOAGGREGATION && inner == NULL);
    }
    IClassFactory_Release(fac);

    fac = NULL;
    hr = CoGetClassObject(&gCLSID, CLSCTX_LOCAL_SERVER, NULL, &gIClassFactory, (void **)&fac);
    CHECK(hr == REGDB_E_CLASSNOTREG && fac == NULL);
}

/* The object and the interfaces it answers. */
static IUnknown *object_and_interfaces(void)
{
    static const struct { const GUID *iid; int kind; } served[] = {
        /* kind 0: the object itself. 1: its own pointer. 2: E_NOINTERFACE. */
        {&gIUnknown, 0},
        {&gIMultiLanguage, 0},
        {&gIMultiLanguage2, 1},
        {&gIMultiLanguage3, 1},
        {&gIMLangCodePages, 1},
        {&gIMLangFontLink, 1},
        {&gIMLangFontLink2, 1},
        {&gIMLangLineBreakConsole, 1},
        {&gIMLangConvertCharset, 2},
        {&gIMLangString, 2},
        {&gIMLangStringWStr, 2},
        {&gIMLangStringAStr, 2},
        {&gIEnumCodePage, 2},
        {&gIEnumRfc1766, 2},
        {&gIEnumScript, 2},
        {&gIDispatch, 2},
        {&gIMarshal, 2},
        {&gIClassFactory, 2},
        {&gBogus, 2},
    };
    IUnknown *unk = NULL, *other = NULL;
    HRESULT hr;
    unsigned i;

    hr = CoCreateInstance(&gCLSID, NULL, CLSCTX_INPROC_SERVER, &gIUnknown, (void **)&unk);
    CHECK(hr == S_OK && unk != NULL);
    CHECK(refs(unk) == 2);

    hr = CoCreateInstance(&gCLSID, NULL, CLSCTX_INPROC_SERVER, &gIUnknown, (void **)&other);
    CHECK(hr == S_OK && other != NULL && other != unk);
    IUnknown_Release(other);

    for (i = 0; i < sizeof served / sizeof served[0]; i++) {
        IUnknown *p = NULL;
        hr = IUnknown_QueryInterface(unk, served[i].iid, (void **)&p);
        if (served[i].kind == 2) {
            CHECK(hr == E_NOINTERFACE);
            CHECK(p == NULL);
            continue;
        }
        CHECK(hr == S_OK && p != NULL);
        if (served[i].kind == 0) {
            CHECK(p == unk);
            CHECK(refs(p) == 3);
        } else {
            CHECK(p != unk);
            CHECK(refs(p) == 2);
        }
        IUnknown_Release(p);
    }
    CHECK(refs(unk) == 2);
    CHECK(IUnknown_QueryInterface(unk, &gIUnknown, NULL) == E_POINTER);
    return unk;
}

/* CodePageToCodePages and CodePagesToCodePage. */
static void code_page_tables(IMLangFontLink2 *fl)
{
    static const struct { UINT page; HRESULT hr; DWORD cps; } to_pages[] = {
        {0, E_FAIL, 0},          {1, E_FAIL, 0},
        {437, E_FAIL, 0},        {850, E_FAIL, 0},
        {874, S_OK, FS_THAI},    {932, S_OK, FS_JISJAPAN},
        {936, S_OK, FS_CHINESESIMP}, {949, S_OK, FS_WANSUNG},
        {950, S_OK, FS_CHINESETRAD}, {1200, E_FAIL, 0},
        {1201, E_FAIL, 0},       {1250, S_OK, FS_LATIN2},
        {1251, S_OK, FS_CYRILLIC}, {1252, S_OK, FS_LATIN1},
        {1253, S_OK, FS_GREEK},  {1254, S_OK, FS_TURKISH},
        {1255, S_OK, FS_HEBREW}, {1256, S_OK, FS_ARABIC},
        {1257, S_OK, FS_BALTIC}, {1258, S_OK, FS_VIETNAMESE},
        {10000, E_FAIL, 0},      {20127, E_FAIL, 0},
        {28591, E_FAIL, 0},      {28592, E_FAIL, 0},
        {50220, E_FAIL, 0},      {51932, E_FAIL, 0},
        {54936, E_FAIL, 0},      {65000, E_FAIL, 0},
        {65001, E_FAIL, 0},      {12345, E_FAIL, 0},
        {0xffffffff, E_FAIL, 0},
    };
    static const struct { DWORD cps; UINT dflt; HRESULT hr; UINT page; } to_page[] = {
        {FS_LATIN1, 0, S_OK, 1252},
        {FS_LATIN1, 1252, S_OK, 1252},
        {FS_LATIN1, 932, S_OK, 1252},
        {FS_CYRILLIC, 0, S_OK, 1251},
        {FS_CYRILLIC, 1252, S_OK, 1251},
        {FS_LATIN1 | FS_CYRILLIC, 1252, S_OK, 1252},
        {FS_LATIN1 | FS_CYRILLIC, 1251, S_OK, 1251},
        {FS_JISJAPAN, 0, S_OK, 932},
        {FS_JISJAPAN, 1252, S_OK, 932},
        {FS_JISJAPAN | FS_CHINESESIMP, 0, S_OK, 932},
        {0xffffffff, 0, S_OK, 1252},
        {0xffffffff, 1252, S_OK, 1252},
        {0, 0, E_FAIL, 0},
        {0, 1252, S_OK, 1252},
        {FS_SYMBOL, 0, E_FAIL, 0},
        {0x00000800, 0, E_FAIL, 0},
        {FS_VIETNAMESE, 0, S_OK, 1258},
        {FS_THAI, 0, S_OK, 874},
    };
    unsigned i;

    for (i = 0; i < sizeof to_pages / sizeof to_pages[0]; i++) {
        DWORD cps = 0xcccccccc;
        HRESULT hr = IMLangFontLink2_CodePageToCodePages(fl, to_pages[i].page, &cps);
        CHECK(hr == to_pages[i].hr);
        CHECK(cps == to_pages[i].cps);
    }
    CHECK(IMLangFontLink2_CodePageToCodePages(fl, 1252, NULL) == S_OK);

    for (i = 0; i < sizeof to_page / sizeof to_page[0]; i++) {
        UINT page = 0xcccccccc;
        HRESULT hr = IMLangFontLink2_CodePagesToCodePage(fl, to_page[i].cps, to_page[i].dflt,
                                                         &page);
        CHECK(hr == to_page[i].hr);
        CHECK(page == to_page[i].page);
    }
    CHECK(IMLangFontLink2_CodePagesToCodePage(fl, FS_LATIN1, 0, NULL) == S_OK);
}

/* CodePageToScriptID. */
static void script_ids(IMLangFontLink2 *fl)
{
    static const struct { UINT page; HRESULT hr; SCRIPT_ID sid; } cases[] = {
        {0, E_FAIL, 0},
        {437, S_OK, sidAsciiLatin},
        {932, S_OK, sidKana},
        {936, S_OK, sidHan},
        {949, S_OK, sidHangul},
        {950, S_OK, sidBopomofo},
        {874, S_OK, sidThai},
        {1200, E_FAIL, 0},
        {1250, S_OK, sidAsciiLatin},
        {1251, S_OK, sidCyrillic},
        {1252, S_OK, sidAsciiLatin},
        {1253, S_OK, sidGreek},
        {1254, S_OK, sidAsciiLatin},
        {1255, S_OK, sidHebrew},
        {1256, S_OK, sidArabic},
        {1257, S_OK, sidAsciiLatin},
        {1258, S_OK, sidAsciiLatin},
        {65001, E_FAIL, 0},
        {12345, E_FAIL, 0},
    };
    unsigned i;

    for (i = 0; i < sizeof cases / sizeof cases[0]; i++) {
        SCRIPT_ID sid = 0xcc;
        HRESULT hr = IMLangFontLink2_CodePageToScriptID(fl, cases[i].page, &sid);
        CHECK(hr == cases[i].hr);
        /* A failure leaves the caller's SCRIPT_ID as it was. */
        CHECK(sid == (cases[i].hr == S_OK ? cases[i].sid : (SCRIPT_ID)0xcc));
    }
    CHECK(IMLangFontLink2_CodePageToScriptID(fl, 1252, NULL) == E_INVALIDARG);
}

/* GetFontCodePages, MapFont, ReleaseFont, ResetFontMapping. */
static void font_code_pages(IMLangFontLink2 *fl)
{
    HDC dc = CreateCompatibleDC(NULL);
    HFONT gui = (HFONT)GetStockObject(DEFAULT_GUI_FONT);
    HFONT mapped = NULL, old = NULL;
    DWORD cps = 0xcccccccc;

    CHECK(dc != NULL && gui != NULL);

    CHECK(IMLangFontLink2_GetFontCodePages(fl, dc, gui, &cps) == S_OK);
    CHECK((cps & FS_LATIN1) != 0);

    {
        DWORD bad = 0xcccccccc;
        CHECK(IMLangFontLink2_GetFontCodePages(fl, NULL, gui, &bad) == E_FAIL && bad == 0);
        bad = 0xcccccccc;
        CHECK(IMLangFontLink2_GetFontCodePages(fl, dc, NULL, &bad) == E_FAIL && bad == 0);
        bad = 0xcccccccc;
        CHECK(IMLangFontLink2_GetFontCodePages(fl, (HDC)(ULONG_PTR)0x1234, gui, &bad) == E_FAIL
              && bad == 0);
        CHECK(IMLangFontLink2_GetFontCodePages(fl, dc, gui, NULL) == S_OK);
    }

    /* MapFont maps the font the DC has selected, for a character and the
       code pages wanted: the code pages the font already covers give a
       font back, and not the handle that went in. */
    old = (HFONT)SelectObject(dc, gui);
    CHECK(old != NULL);

    CHECK(IMLangFontLink2_MapFont(fl, dc, cps, L'A', &mapped) == S_OK);
    CHECK(mapped != NULL && mapped != gui);
    CHECK(IMLangFontLink2_ReleaseFont(fl, mapped) == S_OK);
    CHECK(IMLangFontLink2_ReleaseFont(fl, mapped) == S_OK);

    mapped = NULL;
    CHECK(IMLangFontLink2_MapFont(fl, dc, 0, L'A', &mapped) == E_FAIL && mapped == NULL);
    CHECK(IMLangFontLink2_MapFont(fl, NULL, FS_LATIN1, L'A', &mapped) == E_FAIL
          && mapped == NULL);
    CHECK(IMLangFontLink2_MapFont(fl, dc, FS_LATIN1, L'A', NULL) == S_OK);
    CHECK(IMLangFontLink2_ReleaseFont(fl, NULL) == E_FAIL);
    CHECK(IMLangFontLink2_ReleaseFont(fl, gui) == S_OK);
    CHECK(IMLangFontLink2_ResetFontMapping(fl) == S_OK);

    /* A mapped font the caller never released: ResetFontMapping takes it. */
    mapped = NULL;
    CHECK(IMLangFontLink2_MapFont(fl, dc, cps, L'A', &mapped) == S_OK && mapped != NULL);
    CHECK(IMLangFontLink2_ResetFontMapping(fl) == S_OK);

    SelectObject(dc, old);
    DeleteDC(dc);
}

int main(void)
{
    IUnknown *unk;
    IMLangFontLink2 *fl = NULL;

    CHECK(CoInitializeEx(NULL, COINIT_APARTMENTTHREADED) == S_OK);

    class_object();
    unk = object_and_interfaces();

    CHECK(IUnknown_QueryInterface(unk, &gIMLangFontLink2, (void **)&fl) == S_OK);
    code_page_tables(fl);
    script_ids(fl);
    font_code_pages(fl);
    IMLangFontLink2_Release(fl);

    CHECK(IUnknown_Release(unk) == 0);
    CoUninitialize();
    printf("mlang_test: ok\n");
    return 0;
}
