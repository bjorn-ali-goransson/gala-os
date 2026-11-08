/* shlwapi's URL functions, AssocQueryStringW and SHCreateStreamOnFileEx
 * (included by main.c). What SQL Server LocalDB (msxmlsql, sqllang, xe,
 * xmlrw, hkcompile) and wxWidgets import. Expected values: Wine's
 * conformance tests dlls/shlwapi/tests/url.c and path.c (validated on
 * Windows; vectors copied as data) and Microsoft's documentation of
 * UrlIs, UrlEscape, UrlUnescape, UrlCanonicalize, UrlCombine,
 * UrlCreateFromPath, PathCreateFromUrl, AssocQueryString and
 * SHCreateStreamOnFileEx. */

static int url_is(const wchar_t *got, const wchar_t *want) { return wcscmp(got, want) == 0; }

static int escape(const wchar_t *in, DWORD flags, const wchar_t *want) {
    wchar_t out[INTERNET_MAX_URL_LENGTH];
    DWORD len = INTERNET_MAX_URL_LENGTH;
    HRESULT hr = UrlEscapeW(in, out, &len, flags);
    if (hr != S_OK || !url_is(out, want) || len != wcslen(want)) {
        printf("shlwapi_test: UrlEscapeW(%ls, %lx) = %lx %ls\n", in, flags, hr, out);
        return 0;
    }
    return 1;
}

static int unescape(const wchar_t *in, DWORD flags, const wchar_t *want) {
    wchar_t out[INTERNET_MAX_URL_LENGTH];
    DWORD len = INTERNET_MAX_URL_LENGTH;
    HRESULT hr = UrlUnescapeW((wchar_t *)in, out, &len, flags);
    return hr == S_OK && url_is(out, want) && len == wcslen(want);
}

static int from_url(const wchar_t *in, const wchar_t *want) {
    wchar_t out[MAX_PATH];
    DWORD len = MAX_PATH;
    HRESULT hr = PathCreateFromUrlW(in, out, &len, 0);
    if (hr != S_OK || lstrcmpiW(out, want) != 0 || len != wcslen(want)) {
        printf("shlwapi_test: PathCreateFromUrlW(%ls) = %lx %ls\n", in, hr, out);
        return 0;
    }
    return 1;
}

static int to_url(const wchar_t *in, HRESULT ret, const wchar_t *want) {
    wchar_t out[INTERNET_MAX_URL_LENGTH];
    DWORD len = INTERNET_MAX_URL_LENGTH;
    HRESULT hr = UrlCreateFromPathW(in, out, &len, 0);
    if (hr != ret || lstrcmpiW(out, want) != 0 || len != wcslen(want)) {
        printf("shlwapi_test: UrlCreateFromPathW(%ls) = %lx %ls\n", in, hr, out);
        return 0;
    }
    return 1;
}

static int canonical(const wchar_t *in, DWORD flags, const wchar_t *want) {
    wchar_t out[INTERNET_MAX_URL_LENGTH];
    DWORD len = INTERNET_MAX_URL_LENGTH;
    HRESULT hr = UrlCanonicalizeW(in, out, &len, flags);
    if (hr != S_OK || !url_is(out, want) || len != wcslen(want)) {
        printf("shlwapi_test: UrlCanonicalizeW(%ls, %lx) = %lx %ls\n", in, flags, hr, out);
        return 0;
    }
    return 1;
}

static int url_combine(const wchar_t *base, const wchar_t *rel, DWORD flags, const wchar_t *want) {
    wchar_t out[INTERNET_MAX_URL_LENGTH];
    DWORD len = INTERNET_MAX_URL_LENGTH;
    HRESULT hr = UrlCombineW(base, rel, out, &len, flags);
    if (hr != S_OK || !url_is(out, want) || len != wcslen(want)) {
        printf("shlwapi_test: UrlCombineW(%ls, %ls) = %lx %ls\n", base, rel, hr, out);
        return 0;
    }
    return 1;
}

static void url_checks(void) {
    /* UrlIsW (url.c TEST_URLIS_ATTRIBS; the documented flags). */
    CHECK(UrlIsW(L"mailto:", URLIS_OPAQUE) && UrlIsW(L"about:", URLIS_OPAQUE));
    CHECK(UrlIsW(L"javascript:", URLIS_OPAQUE) && UrlIsW(L"shell:", URLIS_OPAQUE));
    CHECK(!UrlIsW(L"http:", URLIS_OPAQUE) && !UrlIsW(L"file:", URLIS_OPAQUE) && !UrlIsW(L"news:", URLIS_OPAQUE));
    CHECK(UrlIsW(L"mailto://www.winehq.org/test.html", URLIS_OPAQUE));
    CHECK(UrlIsW(L"file:", URLIS_FILEURL) && UrlIsW(L"File:partial", URLIS_FILEURL));
    CHECK(UrlIsW(L"file://e:/b/c", URLIS_FILEURL) && !UrlIsW(L"http:partial", URLIS_FILEURL));
    CHECK(UrlIsW(L"http://foo/bar", URLIS_URL) && !UrlIsW(L"c:\\foo\\bar", URLIS_URL));
    CHECK(UrlIsW(L"http://a/b?c=d", URLIS_HASQUERY) && !UrlIsW(L"http://a/b", URLIS_HASQUERY));

    /* UrlEscapeW (TEST_ESCAPE, TEST_ESCAPEW). */
    CHECK(escape(L"http://www.winehq.org/tests0", 0, L"http://www.winehq.org/tests0"));
    CHECK(escape(L"http://www.winehq.org/tests1\n", 0, L"http://www.winehq.org/tests1%0A"));
    CHECK(escape(L"http://www.winehq.org/tests4\r", URL_ESCAPE_SPACES_ONLY, L"http://www.winehq.org/tests4\r"));
    CHECK(escape(L"file:///f oo/b?a r\\baz", 0, L"file:///f%20oo/b?a r\\baz"));
    CHECK(escape(L"file:///foo/b#a r\\baz", 0, L"file:///foo/b%23a%20r/baz"));
    CHECK(escape(L"file:///f o^&`{}|][\"<>\\%o/b#a r\\baz", 0,
                 L"file:///f%20o%5E%26%60%7B%7D%7C%5D%5B%22%3C%3E/%o/b%23a%20r/baz"));
    CHECK(escape(L"file:///f o%o/b?a r\\b%az", URL_ESCAPE_PERCENT, L"file:///f%20o%25o/b?a r\\b%az"));
    CHECK(escape(L"foo/b%ar\\ba?z\\", URL_ESCAPE_SEGMENT_ONLY, L"foo%2Fb%ar%5Cba%3Fz%5C"));
    CHECK(escape(L"foo/bar\\ba?z\\", 0, L"foo/bar%5Cba?z\\"));
    CHECK(escape(L"/foo/bar\\ba#z\\", 0, L"/foo/bar%5Cba#z\\"));
    CHECK(escape(L"/foo/%5C", URL_ESCAPE_PERCENT, L"/foo/%255C"));
    CHECK(escape(L"http://foo/bar\\baz", 0, L"http://foo/bar/baz"));
    CHECK(escape(L"http:foo/bar\\ba?z\\", 0, L"http:foo%2Fbar%2Fba?z\\"));
    CHECK(escape(L"http:\\\\foo/bar\\baz", 0, L"http://foo/bar/baz"));
    CHECK(escape(L"http:/fo ?o/b ar\\baz", 0, L"http:/fo%20?o/b ar\\baz"));
    CHECK(escape(L"https:\\foo/bar\\baz", 0, L"https:/foo/bar/baz"));
    CHECK(escape(L"foo://foo/bar\\baz", 0, L"foo://foo/bar%5Cbaz"));
    CHECK(escape(L"foo:foo/bar\\baz", 0, L"foo:foo%2Fbar%5Cbaz"));
    CHECK(escape(L"foo:\\foo/bar\\baz", 0, L"foo:%5Cfoo%2Fbar%5Cbaz"));
    CHECK(escape(L"mailto:fo/o@b\\%a?\\r.b#\\az", 0, L"mailto:fo%2Fo@b%5C%a%3F%5Cr.b%23%5Caz"));
    CHECK(escape(L"ftp:/fo/o@bar.baz/foo/bar", 0, L"ftp:/fo/o@bar.baz/foo/bar"));
    CHECK(escape(L"ftp://fo/o@bar.baz/fo?o\\bar", 0, L"ftp://fo/o@bar.baz/fo?o\\bar"));
    CHECK(escape(L"ftp\x1f\1end/", 0, L"ftp%1F%01end/"));
    CHECK(escape(L" <>\"", URL_ESCAPE_AS_UTF8, L"%20%3C%3E%22"));
    CHECK(escape(L"&/?#", URL_ESCAPE_AS_UTF8, L"%26/?#"));
    CHECK(escape(L"Ma\xdf", URL_ESCAPE_AS_UTF8, L"Ma%C3%9F"));
    wchar_t small[8];
    DWORD len = 8;
    CHECK(UrlEscapeW(L"http://a/b c", small, &len, 0) == E_POINTER && len == 15);

    /* UrlUnescapeW (TEST_URL_UNESCAPEW). */
    CHECK(unescape(L"file://foo/bar", 0, L"file://foo/bar"));
    CHECK(unescape(L"file://fo%20o%5Ca/bar", 0, L"file://fo o\\a/bar"));
    CHECK(unescape(L"file://%24%25foobar", 0, L"file://$%foobar"));
    CHECK(unescape(L"file:///C:/Program%20Files", URL_UNESCAPE_AS_UTF8, L"file:///C:/Program Files"));
    CHECK(unescape(L"file://foo/%E4%B8%AD%E6%96%87/bar", 0, L"file://foo/\xe4\xb8\xad\xe6\x96\x87/bar"));
    CHECK(unescape(L"file://foo/%E4%B8%AD%E6%96%87/bar", URL_UNESCAPE_AS_UTF8, L"file://foo/\x4e2d\x6587/bar"));
    CHECK(unescape(L"file://foo/%F0%9F%8D%B7/bar", URL_UNESCAPE_AS_UTF8, L"file://foo/\xd83c\xdf77/bar"));
    CHECK(unescape(L"a%20b?c%20d#e%20f", URL_DONT_UNESCAPE_EXTRA_INFO, L"a b?c%20d#e%20f"));
    wchar_t inplace[] = L"x%41y";
    DWORD ulen = 0;
    CHECK(UrlUnescapeW(inplace, NULL, &ulen, URL_UNESCAPE_INPLACE) == S_OK && wcscmp(inplace, L"xAy") == 0);

    /* PathCreateFromUrlW (path.c TEST_PATHFROMURL). */
    CHECK(from_url(L"file:c:/foo/bar", L"c:\\foo\\bar"));
    CHECK(from_url(L"file:c|/foo/bar", L"c:\\foo\\bar"));
    CHECK(from_url(L"file:c:/foo%20ba%2fr", L"c:\\foo ba/r"));
    CHECK(from_url(L"file:foo/bar/", L"foo\\bar\\"));
    CHECK(from_url(L"file:/c:/foo/bar", L"c:\\foo\\bar"));
    CHECK(from_url(L"file:/foo%20ba%2fr", L"\\foo ba/r"));
    CHECK(from_url(L"file://c:/foo/bar", L"c:\\foo\\bar"));
    CHECK(from_url(L"file://c:/foo%20ba%2fr", L"c:\\foo%20ba%2fr"));
    CHECK(from_url(L"file://foo%20ba%2fr", L"\\\\foo ba/r"));
    CHECK(from_url(L"file://localhost/c:/foo/bar", L"c:\\foo\\bar"));
    CHECK(from_url(L"file://host/foo/bar", L"\\\\host\\foo\\bar"));
    CHECK(from_url(L"file:///c:/foo/bar", L"c:\\foo\\bar"));
    CHECK(from_url(L"File:///c:/foo%20ba%2fr", L"c:\\foo ba/r"));
    CHECK(from_url(L"file:///foo/bar/", L"\\foo\\bar\\"));
    CHECK(from_url(L"file:////c:/foo%20ba%2fr", L"c:\\foo%20ba%2fr"));
    CHECK(from_url(L"file://///c:/foo%20ba%2fr", L"\\\\c:\\foo ba/r"));
    wchar_t p[MAX_PATH];
    len = MAX_PATH;
    CHECK(PathCreateFromUrlW(L"http://foo/bar", p, &len, 0) == E_INVALIDARG);
    len = MAX_PATH;
    CHECK(PathCreateFromUrlW(L"c:\\foo\\bar", p, &len, 0) == E_INVALIDARG);

    /* UrlCreateFromPathW (TEST_URLFROMPATH). */
    CHECK(to_url(L"foo", S_OK, L"file:foo"));
    CHECK(to_url(L"foo\\bar", S_OK, L"file:foo/bar"));
    CHECK(to_url(L"\\foo\\bar", S_OK, L"file:///foo/bar"));
    CHECK(to_url(L"c:\\foo\\bar", S_OK, L"file:///c:/foo/bar"));
    CHECK(to_url(L"c:foo\\bar", S_OK, L"file:///c:foo/bar"));
    CHECK(to_url(L"c:\\foo/b a%r", S_OK, L"file:///c:/foo/b%20a%25r"));
    CHECK(to_url(L"file:///c:/foo/bar", S_FALSE, L"file:///c:/foo/bar"));

    /* UrlCanonicalizeW (http_tests, with http: and file:). */
    CHECK(canonical(L"http://a", 0, L"http://a/"));
    CHECK(canonical(L"http://a/b/./c", 0, L"http://a/b/c"));
    CHECK(canonical(L"http://a/b/../c", 0, L"http://a/c"));
    CHECK(canonical(L"http://a/b/..", 0, L"http://a/"));
    CHECK(canonical(L"http://a/b/.", 0, L"http://a/b/"));
    CHECK(canonical(L"http://a/b/../../../c", 0, L"http://a/c"));
    CHECK(canonical(L"http://a/./b/./../", 0, L"http://a/./b/./../"));
    CHECK(canonical(L"http://a/b/..", URL_DONT_SIMPLIFY, L"http://a/b/.."));
    CHECK(canonical(L"http://a/b?c/./d", 0, L"http://a/b?c/./d"));
    CHECK(canonical(L"http://a/b\\./c", 0, L"http://a/b/c"));
    CHECK(canonical(L"http://a//../../..", 0, L"http://a/"));
    CHECK(canonical(L"file://a/b/../c", 0, L"file://a/c"));
    CHECK(canonical(L"https://localhost/b", 0, L"https://localhost/b"));

    /* UrlCombineW (TEST_COMBINE). */
    CHECK(url_combine(L"http://www.winehq.org/tests", L"tests1", 0, L"http://www.winehq.org/tests1"));
    CHECK(url_combine(L"http://www.winehq.org/tests/", L"../tests3", 0, L"http://www.winehq.org/tests3"));
    CHECK(url_combine(L"http://www.winehq.org/tests/test1", L"test2", 0, L"http://www.winehq.org/tests/test2"));
    CHECK(url_combine(L"http://www.winehq.org/tests/../tests", L"tests4", 0, L"http://www.winehq.org/tests4"));
    CHECK(url_combine(L"http://www.winehq.org/tests/../tests/", L"tests5", 0, L"http://www.winehq.org/tests/tests5"));
    CHECK(url_combine(L"http://www.winehq.org/tests/../tests/", L"/tests6/..", 0, L"http://www.winehq.org/"));
    CHECK(url_combine(L"http://www.winehq.org/tests/../tests/", L"\\tests6\\..", 0, L"http://www.winehq.org/"));
    CHECK(url_combine(L"http://www.winehq.org/tests/?query=x&return=y", L"tests8", 0, L"http://www.winehq.org/tests/tests8"));
    CHECK(url_combine(L"http://www.winehq.org/tests/#example", L"tests9", 0, L"http://www.winehq.org/tests/tests9"));
    CHECK(url_combine(L"http://www.winehq.org/tests/../", L"tests11", URL_DONT_SIMPLIFY, L"http://www.winehq.org/tests/../tests11"));
    CHECK(url_combine(L"http://www.winehq.org/test12", L"#", 0, L"http://www.winehq.org/test12#"));
    CHECK(url_combine(L"http://www.winehq.org/test13#aaa", L"#bbb", 0, L"http://www.winehq.org/test13#bbb"));
    CHECK(url_combine(L"file:///C:\\dir\\file.txt", L"test.txt", 0, L"file:///C:/dir/test.txt"));
    CHECK(url_combine(L"file:///C:\\dir\\file.txt#hash\\hash", L"test.txt", 0, L"file:///C:/dir/file.txt#hash/test.txt"));
    CHECK(url_combine(L"file:///C:\\dir\\file.html#hash\\hash", L"test.html", 0, L"file:///C:/dir/test.html"));
    CHECK(url_combine(L"C:\\winehq\\winehq.txt", L"C:\\Test\\test.txt", 0, L"file:///C:/Test/test.txt"));
    CHECK(url_combine(L"http://www.winehq.org/test/", L"test%20file.txt", 0, L"http://www.winehq.org/test/test%20file.txt"));
    CHECK(url_combine(L"foo:today", L"bar:calendar", 0, L"bar:calendar"));
    CHECK(url_combine(L"foo:/today", L"foo:calendar", 0, L"foo:/calendar"));
    CHECK(url_combine(L"Foo:/today/", L"fOo:calendar", 0, L"foo:/today/calendar"));
    CHECK(url_combine(L"xxx:@MSITStore:file.chm/file.html", L"dir/file", 0, L"xxx:dir/file"));
    CHECK(url_combine(L"ftp://xxxxxxxxx/", L"http://wine3/dir", 0, L"http://wine3/dir"));
    CHECK(url_combine(L"c:\\test\\", L"//share/file.txt", 0, L"file://share/file.txt"));
    CHECK(url_combine(L"file:///c:/dir/file.txt", L"index.html?test=c:/abc",
                  URL_ESCAPE_SPACES_ONLY | URL_DONT_ESCAPE_EXTRA_INFO, L"file:///c:/dir/index.html?test=c:/abc"));
}

/* AssocQueryStringW over a class registered for this user. */
static void assoc_checks(void) {
    HKEY key;
    CHECK(RegCreateKeyExW(HKEY_CURRENT_USER, L"Software\\Classes\\.galatest", 0, NULL, 0, KEY_ALL_ACCESS,
                          NULL, &key, NULL) == ERROR_SUCCESS);
    static const wchar_t progid[] = L"GalaTest.File";
    CHECK(RegSetValueExW(key, NULL, 0, REG_SZ, (const BYTE *)progid, sizeof progid) == ERROR_SUCCESS);
    RegCloseKey(key);
    CHECK(RegCreateKeyExW(HKEY_CURRENT_USER, L"Software\\Classes\\GalaTest.File", 0, NULL, 0, KEY_ALL_ACCESS,
                          NULL, &key, NULL) == ERROR_SUCCESS);
    static const wchar_t friendly[] = L"Gala test file";
    CHECK(RegSetValueExW(key, NULL, 0, REG_SZ, (const BYTE *)friendly, sizeof friendly) == ERROR_SUCCESS);
    RegCloseKey(key);
    CHECK(RegCreateKeyExW(HKEY_CURRENT_USER, L"Software\\Classes\\GalaTest.File\\shell\\open\\command", 0,
                          NULL, 0, KEY_ALL_ACCESS, NULL, &key, NULL) == ERROR_SUCCESS);
    static const wchar_t command[] = L"\"C:\\Gala Tools\\viewer.exe\" \"%1\"";
    CHECK(RegSetValueExW(key, NULL, 0, REG_SZ, (const BYTE *)command, sizeof command) == ERROR_SUCCESS);
    RegCloseKey(key);

    wchar_t out[MAX_PATH];
    DWORD len = MAX_PATH;
    CHECK(AssocQueryStringW(0, ASSOCSTR_COMMAND, L".galatest", NULL, out, &len) == S_OK);
    CHECK(wcscmp(out, command) == 0 && len == wcslen(command) + 1);
    len = MAX_PATH;
    /* The command's program does not exist: CO_E_APPNOTFOUND (Windows 11
       25H2); an existing one (this program, below) is S_OK. */
    CHECK(AssocQueryStringW(0, ASSOCSTR_EXECUTABLE, L".galatest", L"open", out, &len)
          == CO_E_APPNOTFOUND);
    len = MAX_PATH;
    CHECK(AssocQueryStringW(0, ASSOCSTR_FRIENDLYDOCNAME, L".galatest", NULL, out, &len) == S_OK);
    CHECK(wcscmp(out, friendly) == 0);
    len = MAX_PATH;
    CHECK(AssocQueryStringW(0, ASSOCSTR_COMMAND, L"GalaTest.File", NULL, out, &len) == S_OK);
    len = 0;
    CHECK(AssocQueryStringW(0, ASSOCSTR_COMMAND, L".galatest", NULL, NULL, &len) == S_FALSE);
    CHECK(len == wcslen(command) + 1);
    /* A short buffer: HRESULT_FROM_WIN32(ERROR_INSUFFICIENT_BUFFER), not
       the documented E_POINTER, the size needed, and the string truncated
       into the buffer (Windows 11 25H2). */
    len = 4;
    CHECK(AssocQueryStringW(0, ASSOCSTR_COMMAND, L".galatest", NULL, out, &len)
          == HRESULT_FROM_WIN32(ERROR_INSUFFICIENT_BUFFER));
    CHECK(len == wcslen(command) + 1);
    CHECK(wcsncmp(out, command, 3) == 0 && out[3] == 0);
    len = MAX_PATH;
    CHECK(AssocQueryStringW(0, ASSOCSTR_COMMAND, L".galanone", NULL, out, &len)
          == HRESULT_FROM_WIN32(ERROR_NO_ASSOCIATION));
    wchar_t me[MAX_PATH], existing[MAX_PATH + 8];
    CHECK(GetModuleFileNameW(NULL, me, MAX_PATH) > 0);
    wcscpy(existing, L"\"");
    wcscat(existing, me);
    wcscat(existing, L"\" \"%1\"");
    CHECK(RegCreateKeyExW(HKEY_CURRENT_USER, L"Software\\Classes\\GalaTest.File\\shell\\open\\command", 0,
                          NULL, 0, KEY_ALL_ACCESS, NULL, &key, NULL) == ERROR_SUCCESS);
    CHECK(RegSetValueExW(key, NULL, 0, REG_SZ, (const BYTE *)existing, (DWORD)((wcslen(existing) + 1) * 2))
          == ERROR_SUCCESS);
    RegCloseKey(key);
    len = MAX_PATH;
    CHECK(AssocQueryStringW(0, ASSOCSTR_EXECUTABLE, L".galatest", L"open", out, &len) == S_OK);
    CHECK(wcscmp(out, me) == 0);
    CHECK(SHDeleteKeyW(HKEY_CURRENT_USER, L"Software\\Classes\\.galatest") == ERROR_SUCCESS);
    CHECK(SHDeleteKeyW(HKEY_CURRENT_USER, L"Software\\Classes\\GalaTest.File") == ERROR_SUCCESS);
}

/* SHCreateStreamOnFileEx: a file as an IStream. */
static void stream_checks(void) {
    IStream *s = NULL;
    CHECK(SHCreateStreamOnFileEx(L"shlwapi_stream.tmp", STGM_CREATE | STGM_READWRITE, FILE_ATTRIBUTE_NORMAL,
                                 TRUE, NULL, &s) == S_OK && s != NULL);
    ULONG n = 0;
    CHECK(IStream_Write(s, "hello, stream", 13, &n) == S_OK && n == 13);
    LARGE_INTEGER zero = {0};
    ULARGE_INTEGER pos;
    CHECK(IStream_Seek(s, zero, STREAM_SEEK_CUR, &pos) == S_OK && pos.QuadPart == 13);
    CHECK(IStream_Seek(s, zero, STREAM_SEEK_SET, &pos) == S_OK && pos.QuadPart == 0);
    char buf[32] = {0};
    CHECK(IStream_Read(s, buf, sizeof buf, &n) == S_OK && n == 13 && memcmp(buf, "hello, stream", 13) == 0);
    CHECK(IStream_Read(s, buf, sizeof buf, &n) == S_OK && n == 0);
    STATSTG st;
    CHECK(IStream_Stat(s, &st, STATFLAG_NONAME) == S_OK && st.cbSize.QuadPart == 13 && st.type == STGTY_STREAM);
    ULARGE_INTEGER size;
    size.QuadPart = 5;
    CHECK(IStream_SetSize(s, size) == S_OK);
    CHECK(IStream_Stat(s, &st, STATFLAG_NONAME) == S_OK && st.cbSize.QuadPart == 5);
    CHECK(IStream_Release(s) == 0);
    /* Read-only: the existing file. */
    s = NULL;
    CHECK(SHCreateStreamOnFileEx(L"shlwapi_stream.tmp", STGM_READ, 0, FALSE, NULL, &s) == S_OK);
    CHECK(IStream_Read(s, buf, sizeof buf, &n) == S_OK && n == 5 && memcmp(buf, "hello", 5) == 0);
    CHECK(IStream_Write(s, "x", 1, &n) == STG_E_ACCESSDENIED);
    IStream_Release(s);
    CHECK(DeleteFileW(L"shlwapi_stream.tmp"));
    s = (IStream *)1;
    CHECK(SHCreateStreamOnFileEx(L"shlwapi_none.tmp", STGM_READ, 0, FALSE, NULL, &s)
          == HRESULT_FROM_WIN32(ERROR_FILE_NOT_FOUND) && s == NULL);
}
