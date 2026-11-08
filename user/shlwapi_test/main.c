/* shlwapi's path and string functions as real Windows behaves, for what
 * programs import and gala lacked (docs/ROADMAP.md AG): SQL Server
 * LocalDB (xe.dll, sqllang, msxmlsql, hkcompile, SqlDumper,
 * SqlUserInstance) and wxWidgets (FileZilla, FileZilla Server's GUI).
 *
 * Expected values are from Microsoft's documentation (learn.microsoft.com,
 * shlwapi.h) and from Wine's conformance tests (dlls/shlwapi/tests/path.c,
 * string.c), which are validated on real Windows; no Windows machine was
 * available to run this one, so it still needs a run-all.ps1 check there.
 * In short:
 *
 * - PathCanonicalizeW: "." components dropped, ".." removes the one
 *   before (never the root); only backslashes separate components ("/"
 *   is part of a name); an empty path becomes "\".
 * - PathCombineW: a file part that is a full path wins, one starting
 *   with "\" goes under the directory's root, else directory "\" file;
 *   the result canonicalized; NULL (and an empty buffer) if it would not
 *   fit in MAX_PATH; it may be used in place.
 * - PathFindExtensionW: the last component's last ".", or the end.
 *   PathRemoveExtensionW removes it. PathRemoveBackslashW: a trailing
 *   backslash removed unless it is the root's; the result points at the
 *   new end (or the last character when nothing was removed).
 * - PathIsRelativeW: no drive and no leading backslash; NULL and "" are
 *   relative. PathIsURLW: a scheme of two or more characters and a colon.
 * - PathMatchSpecW: the whole path against "*"/"?" patterns, case
 *   ignored; several patterns separated by ";" (leading spaces skipped,
 *   trailing ones significant).
 * - PathSearchAndQualifyW: a full path.
 * - StrCmpW/StrCmpIW/StrCmpNIW: <0, 0, >0 (the I forms ignore case).
 *   StrStrW/StrStrIW: the first occurrence, NULL for an empty needle.
 *   StrCmpLogicalW: digit runs compared as numbers, case ignored (the
 *   documented order 2string < 3string < 20string < st2ring < st20ring
 *   < string2 < string20).
 * - SHDeleteKeyW: a key and everything under it.
 * - StrToIntW (Wine's string.c StrToInt_results): an optional '-' and
 *   decimal digits only (no '+', no leading space, no hex), wrapping at
 *   32 bits. StrTrimW: the given characters off both ends, TRUE when any
 *   went. PathIsFileSpecW: TRUE without ':' or '\'.
 * - SHCreateStreamOnFileW (Wine's istream.c): STGM_CONVERT,
 *   STGM_DELETEONRELEASE and STGM_TRANSACTED are E_INVALIDARG; without
 *   STGM_CREATE a missing file is ERROR_FILE_NOT_FOUND; STGM_CREATE makes
 *   it, and it then opens without.
 * - The URL functions, AssocQueryStringW and SHCreateStreamOnFileEx:
 *   url.c (Wine's url.c vectors).
 *
 * Prints "shlwapi_test: ok" or exits with the number of the failing
 * check. */
#define COBJMACROS
#include <windows.h>
#include <shlwapi.h>
#include <stdio.h>

static int check;
#define CHECK(cond) do { check++; if (!(cond)) { printf("shlwapi_test: check %d (%s line %d) failed\n", check, __FILE_NAME__, __LINE__); fflush(stdout); ExitProcess(check); } } while (0)

#ifndef INTERNET_MAX_URL_LENGTH
#define INTERNET_MAX_URL_LENGTH 2084
#endif
#include "url.c"

static int canon(const wchar_t *in, const wchar_t *want) {
    wchar_t out[MAX_PATH] = L"test";
    return PathCanonicalizeW(out, in) && wcscmp(out, want) == 0;
}

static int combine(const wchar_t *dir, const wchar_t *file, const wchar_t *want) {
    wchar_t out[MAX_PATH] = L"control";
    return PathCombineW(out, dir, file) == out && wcscmp(out, want) == 0;
}

static int ext_at(const wchar_t *path, size_t at) {
    return PathFindExtensionW(path) == path + at;
}

int main(void) {
    /* PathCanonicalizeW (path.c test_PathCanonicalizeA). */
    CHECK(canon(L"", L"\\"));
    CHECK(canon(L"C:\\", L"C:\\"));
    CHECK(canon(L" ", L" "));
    CHECK(canon(L"one\\two", L"one\\two"));
    CHECK(canon(L"C:\\one\\.\\..\\two\\three\\..", L"C:\\two"));
    CHECK(canon(L"C:\\one/two/three\\four/five\\six", L"C:\\one/two/three\\four/five\\six"));
    CHECK(canon(L"C:\\one/.\\two", L"C:\\one/.\\two"));
    CHECK(canon(L"C:\\one/.\\..\\two/three\\..\\four/.five", L"C:\\four/.five"));
    CHECK(canon(L"C:\\..", L"C:\\"));
    CHECK(canon(L"C:\\a\\b\\..\\..\\..\\c", L"C:\\c"));
    wchar_t out[MAX_PATH];
    SetLastError(0xdeadbeef);
    CHECK(!PathCanonicalizeW(out, NULL) && GetLastError() == ERROR_INVALID_PARAMETER);

    /* PathCombineW (test_PathCombineA / test_PathCombineW). */
    CHECK(combine(L"C:\\", NULL, L"C:\\"));
    CHECK(combine(L"C:\\", L"", L"C:\\"));
    CHECK(combine(L"", L"", L"\\"));
    CHECK(combine(NULL, L"one\\two\\three", L"one\\two\\three"));
    CHECK(combine(L"C:", L"one\\two\\three", L"C:\\one\\two\\three"));
    CHECK(combine(L"C:\\", L"one\\two\\three", L"C:\\one\\two\\three"));
    CHECK(combine(L"C:\\", L"\\one\\two\\three", L"C:\\one\\two\\three"));
    CHECK(combine(L"C:\\", L"\\one\\two\\three\\", L"C:\\one\\two\\three\\"));
    CHECK(combine(L"C:\\", L"one/two/three\\", L"C:\\one/two/three\\"));
    CHECK(combine(L"C:\\/\\/", L"\\one\\two\\three\\", L"C:\\one\\two\\three\\"));
    CHECK(combine(L"C:\\", L"one\\..\\two\\.\\three", L"C:\\two\\three"));
    CHECK(combine(L"C:\\", L"..", L"C:\\"));
    CHECK(combine(L"C:\\dir", L"D:\\other", L"D:\\other"));
    CHECK(PathCombineW(NULL, NULL, NULL) == NULL);
    wchar_t two[MAX_PATH] = L"a";
    CHECK(PathCombineW(two, NULL, NULL) == NULL && two[0] == 0);
    /* Too long: NULL and an empty buffer. */
    wchar_t half[MAX_PATH / 2 + 1];
    for (int i = 0; i < MAX_PATH / 2; i++)
        half[i] = L'b';
    half[MAX_PATH / 2] = 0;
    wchar_t big[MAX_PATH + 1];
    big[0] = L'x';
    CHECK(PathCombineW(big, half, half) == NULL && big[0] == 0);
    /* In place. */
    wchar_t place[MAX_PATH] = L"C:\\";
    CHECK(PathCombineW(place, place, L"AA") == place && wcscmp(place, L"C:\\AA") == 0);

    /* PathFindExtensionW, PathRemoveExtensionW (test_PathFindExtensionA). */
    CHECK(PathFindExtensionW(NULL) == NULL);
    CHECK(ext_at(L"", 0));
    CHECK(ext_at(L"file", 4));
    CHECK(ext_at(L"file.txt", 4));
    CHECK(ext_at(L"file.txt.doc", 8));
    CHECK(ext_at(L"C:\\dir.x\\file", 13));
    wchar_t p[MAX_PATH] = L"C:\\dir\\file.txt";
    PathRemoveExtensionW(p);
    CHECK(wcscmp(p, L"C:\\dir\\file") == 0);
    PathRemoveExtensionW(p);
    CHECK(wcscmp(p, L"C:\\dir\\file") == 0);

    /* PathRemoveBackslashW. */
    wcscpy(p, L"C:\\dir\\");
    CHECK(PathRemoveBackslashW(p) == p + 6 && wcscmp(p, L"C:\\dir") == 0);
    CHECK(PathRemoveBackslashW(p) == p + 5 && wcscmp(p, L"C:\\dir") == 0);
    wcscpy(p, L"C:\\");
    CHECK(PathRemoveBackslashW(p) == p + 2 && wcscmp(p, L"C:\\") == 0);

    /* PathIsRelativeW (test_path_is_relative), PathIsURLW (TEST_PATH_IS_URL). */
    CHECK(PathIsRelativeW(NULL) && PathIsRelativeW(L"") && PathIsRelativeW(L"test.txt"));
    CHECK(!PathIsRelativeW(L"\\\\folder\\test.txt") && !PathIsRelativeW(L"C:\\test.txt"));
    CHECK(PathIsRelativeW(L"file://folder/test.txt") && !PathIsRelativeW(L"\\test.txt"));
    CHECK(PathIsURLW(L"http://foo/bar") && PathIsURLW(L"foo://foo/bar") && PathIsURLW(L"bogusscheme:"));
    CHECK(PathIsURLW(L"http:partial") && PathIsURLW(L"about:blank") && PathIsURLW(L"HTTP://www.winehq.org"));
    CHECK(!PathIsURLW(L"c:\\foo\\bar") && !PathIsURLW(L"c:/foo/bar") && !PathIsURLW(L"foo\\bar"));
    CHECK(!PathIsURLW(L"foo.bar") && !PathIsURLW(L"www.winehq.org") && !PathIsURLW(L" http://www.winehq.org"));
    CHECK(!PathIsURLW(L"") && !PathIsURLW(NULL));

    /* PathMatchSpecW (test_PathMatchSpec). */
    const wchar_t *file = L"c:\\foo\\bar\\filename.ext";
    CHECK(!PathMatchSpecW(file, L".ext"));
    CHECK(PathMatchSpecW(file, L"*.ext"));
    CHECK(!PathMatchSpecW(file, L"*.ext "));
    CHECK(PathMatchSpecW(file, L"  *.ext"));
    CHECK(!PathMatchSpecW(file, L"* . ext"));
    CHECK(PathMatchSpecW(file, L"*.e?t"));
    CHECK(!PathMatchSpecW(file, L"filename.ext"));
    CHECK(PathMatchSpecW(file, L"*bar\\filename.ext"));
    CHECK(PathMatchSpecW(file, L" foo; *.ext"));
    CHECK(PathMatchSpecW(file, L"*.ext;*.bar"));
    CHECK(PathMatchSpecW(file, L"*bar*"));
    CHECK(PathMatchSpecW(file, L"*.EXT"));
    CHECK(PathMatchSpecW(L"readme", L"*.*"));

    /* PathSearchAndQualifyW. */
    wchar_t q[MAX_PATH];
    CHECK(PathSearchAndQualifyW(L"C:\\x\\..\\y.txt", q, MAX_PATH) && wcscmp(q, L"C:\\y.txt") == 0);
    wchar_t cwd[MAX_PATH];
    GetCurrentDirectoryW(MAX_PATH, cwd);
    CHECK(PathSearchAndQualifyW(L"nothing-here.txt", q, MAX_PATH));
    CHECK(PathIsRelativeW(q) == FALSE && wcsncmp(q, cwd, wcslen(cwd)) == 0);

    /* StrCmpW, StrCmpIW, StrCmpNIW, StrStrW, StrStrIW (string.c). */
    CHECK(StrCmpW(L"abc", L"abc") == 0 && StrCmpW(L"abc", L"abd") < 0 && StrCmpW(L"b", L"a") > 0);
    CHECK(StrCmpW(L"abc", L"ABC") != 0 && StrCmpIW(L"abc", L"ABC") == 0 && StrCmpIW(L"abc", L"ABD") < 0);
    CHECK(StrCmpNIW(L"abcdef", L"ABCxyz", 3) == 0 && StrCmpNIW(L"abcdef", L"ABCxyz", 4) < 0);
    const wchar_t *hay = L"DeAdBeEf";
    CHECK(StrStrW(hay, L"") == NULL && StrStrW(hay, L"DeAd") == hay && StrStrW(hay, L"dead") == NULL);
    CHECK(StrStrW(hay, L"AdBe") == hay + 2 && StrStrW(hay, L"BeEf") == hay + 4);
    CHECK(StrStrIW(hay, L"") == NULL && StrStrIW(hay, L"dead") == hay && StrStrIW(hay, L"adbe") == hay + 2);
    CHECK(StrStrIW(hay, L"beef") == hay + 4 && StrStrIW(hay, L"cafe") == NULL);

    /* StrCmpLogicalW: the documented order, and equal strings. */
    static const wchar_t *order[] = {L"2string", L"3string", L"20string", L"st2ring", L"st3ring",
                                     L"st20ring", L"string2", L"string3", L"string20"};
    for (int i = 0; i + 1 < (int)(sizeof order / sizeof *order); i++) {
        CHECK(StrCmpLogicalW(order[i], order[i + 1]) == -1);
        CHECK(StrCmpLogicalW(order[i + 1], order[i]) == 1);
    }
    CHECK(StrCmpLogicalW(L"File10", L"file10") == 0 && StrCmpLogicalW(L"a", L"ab") == -1);
    CHECK(StrCmpLogicalW(L"x9", L"x010") == -1);

    /* SHDeleteKeyW: a key with a subkey and a value, all gone. */
    HKEY key;
    CHECK(RegCreateKeyExW(HKEY_CURRENT_USER, L"Software\\GalaShlwapiTest\\a\\b", 0, NULL, 0,
                          KEY_ALL_ACCESS, NULL, &key, NULL) == ERROR_SUCCESS);
    CHECK(RegSetValueExW(key, L"v", 0, REG_DWORD, (const BYTE *)&check, sizeof(DWORD)) == ERROR_SUCCESS);
    RegCloseKey(key);
    CHECK(SHDeleteKeyW(HKEY_CURRENT_USER, L"Software\\GalaShlwapiTest") == ERROR_SUCCESS);
    CHECK(RegOpenKeyExW(HKEY_CURRENT_USER, L"Software\\GalaShlwapiTest", 0, KEY_READ, &key)
          == ERROR_FILE_NOT_FOUND);
    CHECK(SHDeleteKeyW(HKEY_CURRENT_USER, L"Software\\GalaShlwapiTest") == ERROR_FILE_NOT_FOUND);

    /* StrToIntW. */
    CHECK(StrToIntW(L"1099") == 1099 && StrToIntW(L"4294967319") == 23);
    CHECK(StrToIntW(L"+88987") == 0 && StrToIntW(L"012") == 12 && StrToIntW(L"-55") == -55);
    CHECK(StrToIntW(L"0x44ff") == 0 && StrToIntW(L" 1999") == 0 && StrToIntW(L"- 55") == 0);
    CHECK(StrToIntW(L"12ab") == 12 && StrToIntW(L"") == 0);
    /* StrTrimW. */
    {
        wchar_t s[32] = L"  \tgala os \t ";
        CHECK(StrTrimW(s, L" \t") && wcscmp(s, L"gala os") == 0);
        CHECK(!StrTrimW(s, L" \t") && wcscmp(s, L"gala os") == 0);
        wcscpy(s, L"xxxx");
        CHECK(StrTrimW(s, L"x") && s[0] == 0);
    }
    /* PathIsFileSpecW. */
    CHECK(PathIsFileSpecW(L"file.txt") && PathIsFileSpecW(L"a/b") && PathIsFileSpecW(L""));
    CHECK(!PathIsFileSpecW(L"c:file") && !PathIsFileSpecW(L"dir\\file"));
    /* SHCreateStreamOnFileW. */
    {
        wchar_t path[MAX_PATH];
        IStream *st = (IStream *)1;
        GetTempPathW(MAX_PATH, path);
        wcscat(path, L"gala_shlwapi_w.txt");
        DeleteFileW(path);
        CHECK(SHCreateStreamOnFileW(path, STGM_READ | STGM_CONVERT, &st) == E_INVALIDARG && st == NULL);
        st = (IStream *)1;
        CHECK(SHCreateStreamOnFileW(path, STGM_READ | STGM_DELETEONRELEASE, &st) == E_INVALIDARG && st == NULL);
        st = (IStream *)1;
        CHECK(SHCreateStreamOnFileW(path, STGM_READ | STGM_TRANSACTED, &st) == E_INVALIDARG && st == NULL);
        CHECK(SHCreateStreamOnFileW(path, STGM_READ, &st) == HRESULT_FROM_WIN32(ERROR_FILE_NOT_FOUND) && st == NULL);
        CHECK(SHCreateStreamOnFileW(path, STGM_READWRITE | STGM_CREATE, &st) == S_OK && st != NULL);
        ULONG n = 0;
        CHECK(IStream_Write(st, "gala", 4, &n) == S_OK && n == 4);
        CHECK(IStream_Release(st) == 0);
        CHECK(SHCreateStreamOnFileW(path, STGM_READ, &st) == S_OK && st != NULL);
        char buf[8] = {0};
        CHECK(IStream_Read(st, buf, 4, &n) == S_OK && n == 4 && memcmp(buf, "gala", 4) == 0);
        CHECK(IStream_Release(st) == 0);
        CHECK(DeleteFileW(path));
    }

    url_checks();
    assoc_checks();
    stream_checks();

    printf("shlwapi_test: ok\n");
    return 0;
}
