/* urlmon!MkParseDisplayNameEx and the URL moniker it makes, as real
 * Windows behaves, for what Word needs (docs/ROADMAP.md OF).
 *
 * Word delay-loads this export, and a delay load gala cannot answer ends
 * the process: with OleCreateLinkEx answered, `urlmon.dll!MkParseDisplayNameEx`
 * is the one missing row before the end of the run, asked for exactly once.
 *
 * Every expected value below was measured on Windows 11 itself (four
 * probes, in the pull request), and this program is run there as well as
 * under gala.
 *
 * What the function is: ole32's MkParseDisplayName with URLs added in
 * front of it. A display name whose scheme is one urlmon knows becomes a
 * URL moniker (CLSID_StdURLMoniker {79EAC9E0-BAF9-11CE-8C82-00AA004BA90B},
 * MKSYS_URLMONIKER 6) holding the canonical form of the name; every other
 * name is handed to ole32's MkParseDisplayName and its answer -- a file
 * moniker, a class moniker, MK_E_SYNTAX, MK_E_CANTOPENFILE, E_NOINTERFACE
 * -- is returned unchanged. The schemes it knows are a table and not a
 * rule: ftp, http, https, file, mailto, telnet, mk, about, res,
 * javascript, vbscript, local, its and ms-its are taken, while gopher,
 * news, nntp, wais, shell, snews, data, sftp, tel and urn -- schemes
 * Windows' own URL_SCHEME enumeration still lists -- are not.
 *
 * The canonical form is a handful of transformations, each measured on
 * its own:
 *
 * - the scheme is lowercased (but `local:` is left exactly as given);
 * - for a scheme with an authority (`//`), the host is lowercased, an
 *   empty port and the scheme's own default port are removed, an empty
 *   path becomes "/", dot segments are resolved, and backslashes become
 *   forward slashes;
 * - a %XX escape of an unreserved character is decoded (%41 -> A) and
 *   every other escape is kept exactly, upper or lower case;
 * - trailing whitespace is trimmed, and a tab or newline inside the name
 *   is removed, while leading whitespace means the name has no scheme at
 *   all and ole32 answers it (MK_E_SYNTAX);
 * - `file:` is its own shape -- the path is backslashed, "localhost" and
 *   the third slash go, a host becomes a UNC prefix, and dot segments are
 *   left alone;
 * - a scheme without "//" (http:example.com/a, mailto:, about:) keeps
 *   everything after the colon, except `its:`/`ms-its:`, which gain "::/"
 *   when they have no "::" of their own.
 *
 * The moniker answers IUnknown, IMoniker, IPersist, IPersistStream and
 * IAsyncMoniker with itself, and refuses IParseDisplayName, IBindCtx and
 * IClassFactory. (Windows also answers IROTData, IUriContainer and
 * IMarshal with tear-offs, which gala does not have; that limit is named
 * in ARCHITECTURE and is deliberately not checked here.) Everything it
 * can answer without binding is checked below, including the 82 bytes
 * Save writes for one particular URL.
 */

#define COBJMACROS
#define INITGUID
#include <windows.h>
#include <ole2.h>
#include <objidl.h>
#include <urlmon.h>
#include <stdio.h>
#include <string.h>

static int check = 0;
#define CHECK(cond) do { check++; if (!(cond)) { \
    printf("urlmon_test: check %d (line %d) failed\n", check, __LINE__); \
    fflush(stdout); ExitProcess((UINT)check); } } while (0)

#define MK_E_SYNTAX_        ((HRESULT)0x800401E4L)
#define MK_E_NOINVERSE_     ((HRESULT)0x800401ECL)
#define MK_E_NEEDGENERIC_   ((HRESULT)0x800401E2L)
#define MK_E_CANTOPENFILE_  ((HRESULT)0x800401EAL)
#define MK_S_REDUCED_       ((HRESULT)0x000401E2L)
#define MK_S_HIM_           ((HRESULT)0x000401E5L)

/* ---- helpers ---- */

/* Parse one display name. Returns the moniker (or NULL) and the HRESULT. */
static IMoniker *parse_hr(const WCHAR *name, HRESULT *hr_out, ULONG *eaten_out)
{
    IBindCtx *bc = NULL;
    IMoniker *mk = NULL;
    ULONG eaten = 0xdeadbeef;
    HRESULT hr;
    if (CreateBindCtx(0, &bc) != S_OK)
        return NULL;
    hr = MkParseDisplayNameEx(bc, name, &eaten, &mk);
    IBindCtx_Release(bc);
    if (hr_out)
        *hr_out = hr;
    if (eaten_out)
        *eaten_out = eaten;
    return hr == S_OK ? mk : NULL;
}

static IMoniker *parse(const WCHAR *name)
{
    return parse_hr(name, NULL, NULL);
}

/* The moniker's display name, into the caller's buffer. */
static int name_of(IMoniker *mk, WCHAR *out, size_t cch)
{
    IBindCtx *bc = NULL;
    LPOLESTR s = NULL;
    HRESULT hr;
    out[0] = 0;
    if (!mk || CreateBindCtx(0, &bc) != S_OK)
        return 0;
    hr = IMoniker_GetDisplayName(mk, bc, NULL, &s);
    IBindCtx_Release(bc);
    if (hr != S_OK || !s)
        return 0;
    wcsncpy(out, s, cch - 1);
    out[cch - 1] = 0;
    CoTaskMemFree(s);
    return 1;
}

static DWORD kind_of(IMoniker *mk)
{
    DWORD k = 0xffffffff;
    if (IMoniker_IsSystemMoniker(mk, &k) != S_OK)
        return 0xfffffffe;
    return k;
}

/* A display name that must become a URL moniker whose canonical form is
 * `want` (NULL: unchanged), eating the whole name. */
static void canon(const WCHAR *given, const WCHAR *want)
{
    HRESULT hr = 0;
    ULONG eaten = 0;
    IMoniker *mk = parse_hr(given, &hr, &eaten);
    WCHAR got[1024];
    CHECK(hr == S_OK);
    CHECK(mk != NULL);
    CHECK(eaten == (ULONG)wcslen(given));
    CHECK(kind_of(mk) == MKSYS_URLMONIKER);
    CHECK(name_of(mk, got, 1024));
    CHECK(wcscmp(got, want ? want : given) == 0);
    IMoniker_Release(mk);
}

/* A display name urlmon does not know, whose answer is ole32's. */
static void refused(const WCHAR *given, HRESULT want)
{
    HRESULT hr = 0;
    ULONG eaten = 0xdeadbeef;
    IMoniker *mk = parse_hr(given, &hr, &eaten);
    CHECK(hr == want);
    CHECK(mk == NULL);
    CHECK(eaten == 0);
}

static void answers(IMoniker *mk, const GUID *iid)
{
    void *p = NULL;
    CHECK(IMoniker_QueryInterface(mk, iid, &p) == S_OK);
    CHECK(p == (void *)mk);
    if (p)
        IUnknown_Release((IUnknown *)p);
}

static void refuses(IMoniker *mk, const GUID *iid)
{
    void *p = (void *)(ULONG_PTR)0xdead;
    CHECK(IMoniker_QueryInterface(mk, iid, &p) == E_NOINTERFACE);
    CHECK(p == NULL);
}

DEFINE_GUID(IID_IAsyncMoniker_, 0x79eac9d3, 0xbaf9, 0x11ce, 0x8c, 0x82, 0x00, 0xaa, 0x00, 0x4b,
            0xa9, 0x0b);

int main(void)
{
    WCHAR tmp[MAX_PATH];
    WCHAR afile[MAX_PATH + 64], adir[MAX_PATH + 64], absent[MAX_PATH + 64];
    HANDLE h;
    IMoniker *url = NULL;

    OleInitialize(NULL);

    /* ---- the arguments ---- */
    {
        IBindCtx *bc = NULL;
        IMoniker *mk;
        ULONG eaten;
        CHECK(CreateBindCtx(0, &bc) == S_OK);

        /* Each of the four is E_INVALIDARG, and neither out parameter is
         * touched -- not even zeroed. */
        mk = (IMoniker *)(ULONG_PTR)0xdead;
        eaten = 0xdeadbeef;
        CHECK(MkParseDisplayNameEx(NULL, L"http://example.com/x", &eaten, &mk) == E_INVALIDARG);
        CHECK(mk == (IMoniker *)(ULONG_PTR)0xdead);
        CHECK(eaten == 0xdeadbeef);

        mk = (IMoniker *)(ULONG_PTR)0xdead;
        eaten = 0xdeadbeef;
        CHECK(MkParseDisplayNameEx(bc, NULL, &eaten, &mk) == E_INVALIDARG);
        CHECK(mk == (IMoniker *)(ULONG_PTR)0xdead);
        CHECK(eaten == 0xdeadbeef);

        mk = (IMoniker *)(ULONG_PTR)0xdead;
        CHECK(MkParseDisplayNameEx(bc, L"http://example.com/x", NULL, &mk) == E_INVALIDARG);
        CHECK(mk == (IMoniker *)(ULONG_PTR)0xdead);

        eaten = 0xdeadbeef;
        CHECK(MkParseDisplayNameEx(bc, L"http://example.com/x", &eaten, NULL) == E_INVALIDARG);
        CHECK(eaten == 0xdeadbeef);

        /* The empty name is E_INVALIDARG as well, and it too leaves both
         * alone -- where every other refusal zeroes them. */
        mk = (IMoniker *)(ULONG_PTR)0xdead;
        eaten = 0xdeadbeef;
        CHECK(MkParseDisplayNameEx(bc, L"", &eaten, &mk) == E_INVALIDARG);
        CHECK(mk == (IMoniker *)(ULONG_PTR)0xdead);
        CHECK(eaten == 0xdeadbeef);

        IBindCtx_Release(bc);
    }

    /* ---- the schemes it knows ---- */
    canon(L"http://h/p", NULL);
    canon(L"https://h/p", NULL);
    canon(L"ftp://h/p", NULL);
    canon(L"file:///C:/x", L"file://C:\\x");
    canon(L"mailto:a@b", NULL);
    canon(L"telnet://h", L"telnet://h/");
    canon(L"mk:@MSITStore:x.chm::/y.htm", NULL);
    canon(L"about:x", NULL);
    canon(L"res://x/1", NULL);
    canon(L"javascript:0", NULL);
    canon(L"vbscript:0", NULL);
    canon(L"local:x", NULL);
    canon(L"its:x", L"its:x::/");
    canon(L"ms-its:x", L"ms-its:x::/");

    /* ---- and the ones it does not, which are ole32's answers ---- */
    refused(L"gopher://h/p", MK_E_SYNTAX_);
    refused(L"news:group", MK_E_SYNTAX_);
    refused(L"nntp://h/g", MK_E_SYNTAX_);
    refused(L"wais://h/p", MK_E_SYNTAX_);
    refused(L"shell:Desktop", MK_E_SYNTAX_);
    refused(L"snews://h/g", MK_E_SYNTAX_);
    refused(L"data:text/plain,x", MK_E_SYNTAX_);
    refused(L"sftp://h/p", MK_E_SYNTAX_);
    refused(L"tel:123", MK_E_SYNTAX_);
    refused(L"urn:x:y", MK_E_SYNTAX_);
    refused(L"msshellrooted:x", MK_E_SYNTAX_);
    refused(L"ms-help://x", MK_E_SYNTAX_);
    refused(L"zzz://h/p", MK_E_SYNTAX_);
    /* a prefix of a known scheme is not that scheme */
    refused(L"htt://h/p", MK_E_SYNTAX_);
    refused(L"httpx://h/p", MK_E_SYNTAX_);
    refused(L"http2://h/p", MK_E_SYNTAX_);
    /* A drive letter is not a one-letter scheme. The letter is one this
     * machine has not got, because a name on a drive that *is* mounted
     * is a path ole32 tries to open (MK_E_CANTOPENFILE, further down). */
    {
        WCHAR gone[8] = L"_:/x";
        DWORD drives = GetLogicalDrives();
        WCHAR letter = 0;
        int i;
        for (i = 26; i > 0; i--) {
            if (!(drives & (1u << (i - 1)))) {
                letter = (WCHAR)(L'A' + (i - 1));
                break;
            }
        }
        CHECK(letter != 0);
        gone[0] = letter;
        refused(gone, MK_E_SYNTAX_);
        gone[2] = L'\\';
        refused(gone, MK_E_SYNTAX_);
    }

    /* the scheme is matched without regard to case, and lowercased --
     * except `local:`, which is kept exactly as it was given */
    canon(L"HTTP://h/p", L"http://h/p");
    canon(L"HTTPS://h/p", L"https://h/p");
    canon(L"FTP://h/p", L"ftp://h/p");
    canon(L"FILE:///C:/x", L"file://C:\\x");
    canon(L"MAILTO:a@b", L"mailto:a@b");
    canon(L"ABOUT:x", L"about:x");
    canon(L"RES://x/1", L"res://x/1");
    canon(L"MK:@x", L"mk:@x");
    canon(L"JAVASCRIPT:0", L"javascript:0");
    canon(L"VBSCRIPT:0", L"vbscript:0");
    canon(L"TELNET://h", L"telnet://h/");
    canon(L"LOCAL:x", NULL);

    /* ---- the canonical form, one transformation at a time ---- */

    /* an empty path becomes a slash, and a query or fragment implies one */
    canon(L"http://example.com", L"http://example.com/");
    canon(L"http://example.com/", NULL);
    canon(L"http://example.com?q", L"http://example.com/?q");
    canon(L"http://example.com#f", L"http://example.com/#f");
    canon(L"http://", L"http:///");

    /* the host is lowercased; the path, query and fragment are not */
    canon(L"http://EXAMPLE.COM/a", L"http://example.com/a");
    canon(L"http://example.com/A/B", NULL);
    canon(L"HtTp://Example.COM:80/A?Q=1#F", L"http://example.com/A?Q=1#F");

    /* the scheme's own default port goes, and only that one */
    canon(L"http://example.com:80/a", L"http://example.com/a");
    canon(L"http://example.com:8080/a", NULL);
    canon(L"https://example.com:443/a", L"https://example.com/a");
    canon(L"https://example.com:80/a", NULL);
    canon(L"http://example.com:/a", L"http://example.com/a");
    canon(L"http://example.com:0/a", NULL);

    /* dot segments are resolved, and a trailing one leaves the slash */
    canon(L"http://example.com/a/./b", L"http://example.com/a/b");
    canon(L"http://example.com/a/../b", L"http://example.com/b");
    canon(L"http://example.com/../a", L"http://example.com/a");
    canon(L"http://example.com/a/b/..", L"http://example.com/a/");
    canon(L"http://example.com/a/b/.", L"http://example.com/a/b/");
    canon(L"http://example.com/.", L"http://example.com/");
    canon(L"http://example.com/..", L"http://example.com/");
    /* an empty segment is not a dot segment */
    canon(L"http://example.com/a//b", NULL);

    /* backslashes in a hierarchical path are slashes */
    canon(L"http://example.com\\a\\b", L"http://example.com/a/b");

    /* escapes: an unreserved character is decoded, everything else kept
     * exactly, in the case it was written */
    canon(L"http://example.com/%41", L"http://example.com/A");
    canon(L"http://example.com/%2F", NULL);
    canon(L"http://example.com/%2f", NULL);
    canon(L"http://example.com/%25", NULL);
    canon(L"http://example.com/%20", NULL);

    /* a space is left alone, and so is a plus */
    canon(L"http://example.com/a b", NULL);
    canon(L"http://example.com/a+b", NULL);
    canon(L"http://example.com/a?q=1 2", NULL);

    /* trailing whitespace is trimmed; a tab or a newline inside is
     * removed; leading whitespace hides the scheme altogether */
    canon(L"http://example.com/a  ", L"http://example.com/a");
    canon(L"http://example.com/a\tb", L"http://example.com/ab");
    canon(L"http://example.com/a\nb", L"http://example.com/ab");
    refused(L"  http://example.com/a", MK_E_SYNTAX_);

    /* the user information is kept */
    canon(L"http://u:p@example.com/a", NULL);

    /* a hierarchical scheme without "//" keeps everything after the colon */
    canon(L"http:example.com/a", NULL);
    canon(L"http:/example.com/a", NULL);
    canon(L"http:", NULL);

    /* an address is a host like any other */
    canon(L"http://127.0.0.1/a", NULL);
    canon(L"http://[::1]/a", NULL);

    /* file: is its own shape */
    canon(L"file://C:/a/b.txt", L"file://C:\\a\\b.txt");
    canon(L"file:///C:/a/b.txt", L"file://C:\\a\\b.txt");
    canon(L"file://C:\\a\\b.txt", NULL);
    canon(L"file://localhost/C:/a.txt", L"file://C:\\a.txt");
    canon(L"file://server/share/x", L"file://\\\\server\\share\\x");
    canon(L"file://\\\\server\\share\\x", NULL);
    canon(L"file:C:/a/b.txt", NULL);
    /* dot segments are left alone in a file: path, and so are spaces */
    canon(L"file:///C:/a/../b.txt", L"file://C:\\a\\..\\b.txt");
    canon(L"file:///C:/a b.txt", L"file://C:\\a b.txt");

    /* the escapes: an unreserved character is decoded wherever it is in
     * the path or the host, and a decoded dot then takes part in the dot
     * segments like any other; a query or a fragment is not decoded at
     * all, and neither is an escape that is not of an unreserved
     * character, nor one that is not an escape */
    canon(L"http://example.com/%61", L"http://example.com/a");
    canon(L"http://example.com/%4a", L"http://example.com/J");
    canon(L"http://example.com/%39", L"http://example.com/9");
    canon(L"http://example.com/%2D", L"http://example.com/-");
    canon(L"http://example.com/%5F", L"http://example.com/_");
    canon(L"http://example.com/%7E", L"http://example.com/~");
    canon(L"http://example.com/%2E", L"http://example.com/");
    canon(L"http://example.com/%2B", NULL);
    canon(L"http://example.com/%3A", NULL);
    canon(L"http://example.com/%40", NULL);
    canon(L"http://example.com/%3F", NULL);
    canon(L"http://example.com/%23", NULL);
    canon(L"http://example.com/%00", NULL);
    canon(L"http://example.com/%7F", NULL);
    canon(L"http://example.com/%GG", NULL);
    canon(L"http://example.com/%4", NULL);
    canon(L"http://example.com/%", NULL);
    canon(L"http://example.com/%2541", NULL);
    canon(L"http://example.com/a%41b?q=%41#%41", L"http://example.com/aAb?q=%41#%41");
    canon(L"http://%41%42/a", L"http://ab/a");

    /* every scheme's own default port, and only its own */
    canon(L"ftp://example.com:21/a", L"ftp://example.com/a");
    canon(L"ftp://example.com:80/a", NULL);
    canon(L"telnet://example.com:23/", L"telnet://example.com/");
    canon(L"telnet://example.com:80/", NULL);
    canon(L"http://example.com:80", L"http://example.com/");
    canon(L"ftp://example.com:21", L"ftp://example.com/");

    /* the host is lowercased and the user information is not */
    canon(L"http://User:PASS@Example.COM/a", L"http://User:PASS@example.com/a");
    canon(L"ftp://Anonymous@FTP.Example.COM/a", L"ftp://Anonymous@ftp.example.com/a");

    /* an opaque scheme keeps everything after its colon, in the case it
     * was written */
    canon(L"MAILTO:Someone@EXAMPLE.COM", L"mailto:Someone@EXAMPLE.COM");
    canon(L"res://SHELL32.DLL/1", NULL);
    canon(L"about:BLANK", NULL);
    canon(L"mk:@MSITStore:X.CHM::/Y.HTM", NULL);
    canon(L"javascript:ALERT(1)", NULL);
    canon(L"vbscript:MsgBox 1", NULL);
    canon(L"LOCAL:X", NULL);
    canon(L"mailto:", NULL);
    canon(L"about:", NULL);
    /* its: and ms-its: gain "::/" only where they have no "::" of their own */
    canon(L"its:X.CHM::/Y.HTM", NULL);
    canon(L"ms-its:X.CHM::/Y.HTM", NULL);
    canon(L"its:x::/", NULL);

    /* file:: an empty authority and "localhost" are dropped, a drive
     * written with a bar becomes a colon, the host is lowercased while
     * the path is not, and the dot segments stay */
    canon(L"file://", NULL);
    canon(L"file:///", L"file://");
    canon(L"file://localhost/", L"file://");
    canon(L"file:///C:/a/./b.txt", L"file://C:\\a\\.\\b.txt");
    canon(L"file://C:/A/B.TXT", L"file://C:\\A\\B.TXT");
    canon(L"file://LOCALHOST/C:/a.txt", L"file://C:\\a.txt");
    canon(L"file://SERVER/Share/X", L"file://\\\\server\\Share\\X");
    canon(L"file://c|/x", L"file://c:\\x");
    canon(L"file:", NULL);

    /* a carriage return goes the way of a tab, inside the name or at
     * the end of it, and a tab inside the host goes too */
    canon(L"http://example.com/a\rb", L"http://example.com/ab");
    canon(L"http://example.com/a\r\nb", L"http://example.com/ab");
    canon(L"http://exam\tple.com/a", L"http://example.com/a");
    canon(L"http://example.com/a\t", L"http://example.com/a");
    canon(L"http://example.com/a\n", L"http://example.com/a");

    /* ---- what ole32 answers, unchanged ---- */
    {
        HRESULT hr = 0;
        ULONG eaten = 0;
        IMoniker *mk;
        WCHAR got[MAX_PATH + 64];

        CHECK(GetTempPathW(MAX_PATH, tmp) != 0);
        wcscpy(afile, tmp);
        wcscat(afile, L"urlmon_test_file.txt");
        wcscpy(adir, tmp);
        wcscat(adir, L"urlmon_test_dir");
        wcscpy(absent, tmp);
        wcscat(absent, L"urlmon_test_absent.txt");
        h = CreateFileW(afile, GENERIC_WRITE, 0, NULL, CREATE_ALWAYS, 0, NULL);
        CHECK(h != INVALID_HANDLE_VALUE);
        CloseHandle(h);
        CreateDirectoryW(adir, NULL);
        DeleteFileW(absent);

        /* a file that exists is a file moniker, the whole name eaten */
        mk = parse_hr(afile, &hr, &eaten);
        CHECK(hr == S_OK);
        CHECK(mk != NULL);
        CHECK(eaten == (ULONG)wcslen(afile));
        CHECK(kind_of(mk) == MKSYS_FILEMONIKER);
        CHECK(name_of(mk, got, MAX_PATH + 64));
        CHECK(wcscmp(got, afile) == 0);
        IMoniker_Release(mk);

        refused(absent, MK_E_CANTOPENFILE_);
        refused(adir, MK_E_CANTOPENFILE_);
        refused(L"\\\\server\\share\\x.txt", MK_E_CANTOPENFILE_);
        refused(L"urlmon_test_file.txt", MK_E_SYNTAX_);
        refused(L"hello", MK_E_SYNTAX_);
        refused(L" ", MK_E_SYNTAX_);
        refused(L"@Word.Document.12", E_NOINTERFACE);

        /* a class moniker is ole32's too */
        mk = parse_hr(L"clsid:00000303-0000-0000-C000-000000000046:", &hr, &eaten);
        CHECK(hr == S_OK);
        CHECK(mk != NULL);
        CHECK(eaten == 43);
        CHECK(kind_of(mk) == MKSYS_CLASSMONIKER);
        IMoniker_Release(mk);

        DeleteFileW(afile);
        RemoveDirectoryW(adir);
    }

    /* ---- the moniker it makes ---- */
    url = parse(L"http://example.com/a/b.htm");
    CHECK(url != NULL);

    answers(url, &IID_IUnknown);
    answers(url, &IID_IMoniker);
    answers(url, &IID_IPersist);
    answers(url, &IID_IPersistStream);
    answers(url, &IID_IAsyncMoniker_);
    refuses(url, &IID_IParseDisplayName);
    refuses(url, &IID_IBindCtx);
    refuses(url, &IID_IClassFactory);

    {
        CLSID cls;
        ULARGE_INTEGER size;
        DWORD h1 = 0, h2 = 0;
        IMoniker *same = parse(L"http://example.com/a/b.htm");
        IMoniker *other = parse(L"http://example.com/a/c.htm");
        IMoniker *upper = parse(L"http://example.com/A/B.HTM");

        CHECK(IMoniker_GetClassID(url, &cls) == S_OK);
        CHECK(cls.Data1 == 0x79eac9e0);
        CHECK(cls.Data2 == 0xbaf9);
        CHECK(cls.Data3 == 0x11ce);
        CHECK(cls.Data4[0] == 0x8c && cls.Data4[7] == 0x0b);

        /* it reports itself dirty, always */
        CHECK(IMoniker_IsDirty(url) == S_OK);

        CHECK(IMoniker_GetSizeMax(url, &size) == S_OK);
        CHECK(size.QuadPart == 82);

        CHECK(IMoniker_Hash(url, &h1) == S_OK);
        CHECK(same != NULL);
        CHECK(IMoniker_Hash(same, &h2) == S_OK);
        CHECK(h1 == h2);
        CHECK(IMoniker_IsEqual(url, same) == S_OK);
        CHECK(other != NULL);
        CHECK(IMoniker_IsEqual(url, other) == S_FALSE);
        /* the comparison is of the canonical name, case and all */
        CHECK(upper != NULL);
        CHECK(IMoniker_IsEqual(url, upper) == S_FALSE);

        if (same) IMoniker_Release(same);
        if (upper) IMoniker_Release(upper);

        /* the methods that need no binding */
        {
            IBindCtx *bc = NULL;
            IMoniker *out = (IMoniker *)(ULONG_PTR)0xdead;
            IEnumMoniker *en = (IEnumMoniker *)(ULONG_PTR)0xdead;
            IMoniker *anti = NULL, *item = NULL;
            WCHAR got[512];
            ULONG eaten;

            CHECK(CreateBindCtx(0, &bc) == S_OK);
            CHECK(CreateAntiMoniker(&anti) == S_OK);
            CHECK(CreateItemMoniker(L"!", L"x", &item) == S_OK);

            /* a URL has no inverse */
            out = NULL;
            CHECK(IMoniker_Inverse(url, &out) == MK_E_NOINVERSE_);
            CHECK(out == NULL);

            /* composing it with itself is reduction to the right-hand
             * one, absolute or not, host and scheme and all */
            out = NULL;
            CHECK(IMoniker_ComposeWith(url, other, FALSE, &out) == S_OK);
            CHECK(out != NULL);
            CHECK(kind_of(out) == MKSYS_URLMONIKER);
            CHECK(name_of(out, got, 512));
            CHECK(wcscmp(got, L"http://example.com/a/c.htm") == 0);
            IMoniker_Release(out);

            /* with anything else it is a generic composite, and only
             * when it is allowed to make one */
            out = NULL;
            CHECK(IMoniker_ComposeWith(url, item, FALSE, &out) == S_OK);
            CHECK(out != NULL);
            CHECK(kind_of(out) == MKSYS_GENERICCOMPOSITE);
            CHECK(name_of(out, got, 512));
            CHECK(wcscmp(got, L"http://example.com/a/b.htm!x") == 0);
            IMoniker_Release(out);

            out = (IMoniker *)(ULONG_PTR)0xdead;
            CHECK(IMoniker_ComposeWith(url, item, TRUE, &out) == MK_E_NEEDGENERIC_);
            CHECK(out == NULL);

            /* an anti moniker does not annihilate a URL: it composes */
            out = NULL;
            CHECK(IMoniker_ComposeWith(url, anti, FALSE, &out) == S_OK);
            CHECK(out != NULL);
            CHECK(kind_of(out) == MKSYS_GENERICCOMPOSITE);
            IMoniker_Release(out);

            /* it enumerates nothing, and says so with S_OK and no
             * enumerator rather than an empty one */
            en = (IEnumMoniker *)(ULONG_PTR)0xdead;
            CHECK(IMoniker_Enum(url, TRUE, &en) == S_OK);
            CHECK(en == NULL);
            en = (IEnumMoniker *)(ULONG_PTR)0xdead;
            CHECK(IMoniker_Enum(url, FALSE, &en) == S_OK);
            CHECK(en == NULL);

            /* a relative path where there is one, the other moniker
             * where there is not */
            out = NULL;
            CHECK(IMoniker_RelativePathTo(url, other, &out) == S_OK);
            CHECK(out != NULL);
            CHECK(name_of(out, got, 512));
            CHECK(wcscmp(got, L"c.htm") == 0);
            IMoniker_Release(out);
            {
                IMoniker *up = parse(L"http://example.com/c.htm");
                IMoniker *across = parse(L"http://other.test/c.htm");
                CHECK(up != NULL);
                out = NULL;
                CHECK(IMoniker_RelativePathTo(url, up, &out) == S_OK);
                CHECK(out != NULL);
                CHECK(name_of(out, got, 512));
                CHECK(wcscmp(got, L"../c.htm") == 0);
                IMoniker_Release(out);
                CHECK(across != NULL);
                out = NULL;
                CHECK(IMoniker_RelativePathTo(url, across, &out) == MK_S_HIM_);
                CHECK(out != NULL);
                CHECK(name_of(out, got, 512));
                CHECK(wcscmp(got, L"http://other.test/c.htm") == 0);
                IMoniker_Release(out);
                IMoniker_Release(up);
                IMoniker_Release(across);
            }

            /* no common prefix, ever */
            out = (IMoniker *)(ULONG_PTR)0xdead;
            CHECK(IMoniker_CommonPrefixWith(url, other, &out) == E_NOTIMPL);

            /* it reduces to itself, with a reference of its own */
            out = NULL;
            CHECK(IMoniker_Reduce(url, bc, MKRREDUCE_ALL, NULL, &out) == MK_S_REDUCED_);
            CHECK(out == url);
            if (out) IMoniker_Release(out);

            /* nothing is running under it */
            CHECK(IMoniker_IsRunning(url, bc, NULL, NULL) == S_FALSE);

            /* parsing a name after it resolves against it */
            out = NULL;
            eaten = 0xdeadbeef;
            CHECK(IMoniker_ParseDisplayName(url, bc, NULL, (LPOLESTR)L"c.htm", &eaten, &out)
                  == S_OK);
            CHECK(eaten == 5);
            CHECK(out != NULL);
            CHECK(kind_of(out) == MKSYS_URLMONIKER);
            CHECK(name_of(out, got, 512));
            CHECK(wcscmp(got, L"http://example.com/a/c.htm") == 0);
            IMoniker_Release(out);

            out = NULL;
            eaten = 0xdeadbeef;
            CHECK(IMoniker_ParseDisplayName(url, bc, NULL, (LPOLESTR)L"http://other/x", &eaten,
                                            &out) == S_OK);
            CHECK(eaten == 14);
            CHECK(out != NULL);
            CHECK(name_of(out, got, 512));
            CHECK(wcscmp(got, L"http://other/x") == 0);
            IMoniker_Release(out);

            IMoniker_Release(anti);
            IMoniker_Release(item);
            IBindCtx_Release(bc);
        }

        if (other) IMoniker_Release(other);
    }

    /* ---- the bytes Save writes ---- */
    {
        IPersistStream *ps = NULL;
        IStream *s = NULL;
        LARGE_INTEGER zero;
        ULARGE_INTEGER pos;
        unsigned char buf[128];
        ULONG got = 0;
        /* the same 16 bytes follow every URL, whatever it is */
        static const unsigned char tail[24] = {
            0x79, 0x58, 0x81, 0xf4, 0x3b, 0x1d, 0x7f, 0x48,
            0xaf, 0x2c, 0x82, 0x5d, 0xc4, 0x85, 0x27, 0x63,
            0x00, 0x00, 0x00, 0x00, 0xa5, 0xab, 0x00, 0x03,
        };
        static const WCHAR saved[] = L"http://example.com/a/b.htm";

        zero.QuadPart = 0;
        CHECK(IMoniker_QueryInterface(url, &IID_IPersistStream, (void **)&ps) == S_OK);
        CHECK(CreateStreamOnHGlobal(NULL, TRUE, &s) == S_OK);
        CHECK(IPersistStream_Save(ps, s, TRUE) == S_OK);
        CHECK(IStream_Seek(s, zero, STREAM_SEEK_CUR, &pos) == S_OK);
        CHECK(pos.QuadPart == 82);
        CHECK(IStream_Seek(s, zero, STREAM_SEEK_SET, NULL) == S_OK);
        CHECK(IStream_Read(s, buf, sizeof(buf), &got) == S_OK);
        CHECK(got == 82);
        /* a length that counts everything after itself */
        CHECK(buf[0] == 78 && buf[1] == 0 && buf[2] == 0 && buf[3] == 0);
        /* the display name, UTF-16, with its NUL */
        CHECK(memcmp(buf + 4, saved, sizeof(saved)) == 0);
        CHECK(memcmp(buf + 4 + sizeof(saved), tail, sizeof(tail)) == 0);

        /* and it loads back into another URL moniker */
        {
            IMoniker *fresh = NULL;
            IPersistStream *fps = NULL;
            WCHAR back[512];
            CHECK(CreateURLMoniker(NULL, L"http://nothing.test/", &fresh) == S_OK);
            CHECK(fresh != NULL);
            CHECK(IMoniker_QueryInterface(fresh, &IID_IPersistStream, (void **)&fps) == S_OK);
            CHECK(IStream_Seek(s, zero, STREAM_SEEK_SET, NULL) == S_OK);
            CHECK(IPersistStream_Load(fps, s) == S_OK);
            CHECK(name_of(fresh, back, 512));
            CHECK(wcscmp(back, saved) == 0);
            CHECK(IMoniker_IsEqual(fresh, url) == S_OK);
            IPersistStream_Release(fps);
            IMoniker_Release(fresh);
        }

        IStream_Release(s);
        IPersistStream_Release(ps);
    }

    /* ---- CreateURLMoniker and CreateURLMonikerEx ---- */
    {
        IMoniker *m = NULL;
        WCHAR got[512];

        /* no base: the name is taken as it stands, canonical or not */
        CHECK(CreateURLMoniker(NULL, L"http://example.com", &m) == S_OK);
        CHECK(name_of(m, got, 512));
        CHECK(wcscmp(got, L"http://example.com/") == 0);
        IMoniker_Release(m);
        m = NULL;

        /* a name with no scheme is kept as it is, and is still a URL
         * moniker */
        CHECK(CreateURLMoniker(NULL, L"c.htm", &m) == S_OK);
        CHECK(kind_of(m) == MKSYS_URLMONIKER);
        CHECK(name_of(m, got, 512));
        CHECK(wcscmp(got, L"c.htm") == 0);
        IMoniker_Release(m);
        m = NULL;

        CHECK(CreateURLMoniker(NULL, L"not a url", &m) == S_OK);
        CHECK(name_of(m, got, 512));
        CHECK(wcscmp(got, L"not a url") == 0);
        IMoniker_Release(m);
        m = NULL;

        /* with a base it resolves against it */
        CHECK(CreateURLMoniker(url, L"c.htm", &m) == S_OK);
        CHECK(name_of(m, got, 512));
        CHECK(wcscmp(got, L"http://example.com/a/c.htm") == 0);
        IMoniker_Release(m);
        m = NULL;

        CHECK(CreateURLMoniker(url, L"/c.htm", &m) == S_OK);
        CHECK(name_of(m, got, 512));
        CHECK(wcscmp(got, L"http://example.com/c.htm") == 0);
        IMoniker_Release(m);
        m = NULL;

        CHECK(CreateURLMoniker(url, L"", &m) == S_OK);
        CHECK(name_of(m, got, 512));
        CHECK(wcscmp(got, L"http://example.com/a/") == 0);
        IMoniker_Release(m);
        m = NULL;

        CHECK(CreateURLMoniker(NULL, NULL, &m) == E_INVALIDARG);
        CHECK(CreateURLMoniker(NULL, L"http://example.com/", NULL) == E_INVALIDARG);

        /* the Ex form takes two flags and refuses the rest */
        CHECK(CreateURLMonikerEx(NULL, L"http://example.com", &m, URL_MK_LEGACY) == S_OK);
        CHECK(name_of(m, got, 512));
        CHECK(wcscmp(got, L"http://example.com/") == 0);
        IMoniker_Release(m);
        m = NULL;
        CHECK(CreateURLMonikerEx(NULL, L"http://example.com", &m, URL_MK_UNIFORM) == S_OK);
        CHECK(name_of(m, got, 512));
        CHECK(wcscmp(got, L"http://example.com/") == 0);
        IMoniker_Release(m);
        m = NULL;
        CHECK(CreateURLMonikerEx(NULL, L"http://example.com", &m, 0xffff) == E_INVALIDARG);
    }

    IMoniker_Release(url);

    printf("urlmon_test: ok (%d checks)\n", check);
    fflush(stdout);
    OleUninitialize();
    return 0;
}
