/* msvcrt functions as real Windows' msvcrt.dll behaves (documented
 * behaviour only): sscanf, _vsnprintf measuring and truncating,
 * setjmp/longjmp, _pipe/_read/_write/_close with end of file, _dup and
 * _dup2 on fd 1, strtok, atof, _stricmp/_strnicmp, mbstowcs, _isatty,
 * _getcwd; __argc, _controlfp, _snprintf, _strlwr and remove (TinyCC's
 * startup code and tcc.exe use them); strtod (tcc parses float
 * constants with it); the math functions, strcoll, _difftime64, and
 * printf's %e/%g with msvcrt's three-digit exponents and 17 significant
 * digits (Lua prints numbers with %.14g); a fopen'd FILE's lock (_FILEX);
 * _open taking the lowest free descriptor (0 after closing stdin);
 * what PostgreSQL's mingw-built DLLs (libintl, libiconv) use: wcschr,
 * _ultoa, __mb_cur_max (1 in the C locale), fputwc/fwprintf to a
 * narrow text stream, _beginthreadex/_endthreadex (the exit code) and
 * __dllonexit (appending to a DLL's exit table); what MinGit's git.exe
 * imports: the wide string and path functions (wcscat, wcsncat,
 * wcsncpy, wcsspn, _wcsdup, msvcrt's own swprintf without a count,
 * _wmkdir, _waccess, _wchdir, _wgetcwd, _wopen, _wchmod, _wunlink,
 * _wrmdir, _wmktemp, _wfreopen), __wgetmainargs and __winitenv, _commit,
 * atol, iswctype, rewind, fscanf, _flushall; and shell32's
 * CommandLineToArgvW (quotes and backslashes); what FileZilla's DLLs
 * import: the isw* classes and towupper/towlower, wcstol/wcstoul/_wtoi,
 * wcspbrk, wcscoll/wcsxfrm/strxfrm (the C locale), strncat, wcsftime,
 * _wputenv/_wgetenv, _wrename/_wstat64/_wutime64/_wremove, _wfullpath,
 * _wfindfirst64/_wfindnext64/_findclose, _filelengthi64,
 * _getmaxstdio/_setmaxstdio, fgetpos/fsetpos, the wide stream functions
 * on a narrow text stream (fputws, putwc, fgetwc, getwc, ungetwc),
 * _putws, _aligned_malloc/_aligned_free, _beginthread and
 * _getdrive/_chdrive. Built against msvcrt
 * (not mingw's own stdio) by build.sh.
 * Prints "msvcrt_test: ok" or exits with the number of the failing check. */
#include <stdio.h>
#include <stdarg.h>
#include <stdlib.h>
#include <string.h>
#include <setjmp.h>
#include <io.h>
#include <fcntl.h>
#include <direct.h>
#include <float.h>
#include <errno.h>
#include <math.h>
#include <windows.h>
#include <time.h>
#include <stdint.h>
#include <sys/utime.h>

/* TinyCC's own float.h (the tcc_self scenario compiles this file) does
 * not declare msvcrt's _controlfp. */
#ifndef _MCW_RC
#define _MCW_RC 0x300
#define _RC_NEAR 0
#define _RC_CHOP 0x300
unsigned int __cdecl _controlfp(unsigned int new_value, unsigned int mask);
#endif

#ifndef __TINYC__
#include <process.h>
#include <wchar.h>
#include <wctype.h>
#include <shellapi.h>
#include <sys/stat.h>
typedef struct { int newmode; } startup_info;
int __cdecl __wgetmainargs(int *argc, wchar_t ***argv, wchar_t ***envp, int glob,
                           startup_info *si);
typedef void (__cdecl *exit_fn)(void);
_onexit_t __cdecl __dllonexit(_onexit_t func, exit_fn **begin, exit_fn **end);
static unsigned __stdcall thread_main(void *arg) {
    _endthreadex(*(unsigned *)arg);
    return 99; /* not reached */
}
static int __cdecl at_exit(void) { return 0; }
#endif

static int check;
#define CHECK(cond) do { check++; if (!(cond)) { printf("msvcrt_test: check %d (line %d) failed\n", check, __LINE__); fflush(stdout); exit(check); } } while (0)

static int measure(const char *fmt, ...) {
    va_list ap;
    va_start(ap, fmt);
    int r = _vsnprintf(NULL, 0, fmt, ap);
    va_end(ap);
    return r;
}

static int format(char *buf, size_t n, const char *fmt, ...) {
    va_list ap;
    va_start(ap, fmt);
    int r = _vsnprintf(buf, n, fmt, ap);
    va_end(ap);
    return r;
}

static jmp_buf jump;
static void jump_back(int value) { longjmp(jump, value); }

#ifndef __TINYC__
static volatile LONG begun;
static void __cdecl begin_thread(void *arg) { InterlockedExchange(&begun, (LONG)(INT_PTR)arg); }

/* What FileZilla's DLLs (wxWidgets, libfilezilla, GnuTLS) import, as
 * msvcrt.dll documents it. */
static void filezilla_checks(void) {
    CHECK(iswspace(L' ') && iswspace(L'\t') && iswspace(L'\n') && !iswspace(L'a'));
    CHECK(iswalpha(L'a') && iswalpha(L'Z') && iswalpha(0xe9) && !iswalpha(L'1'));
    CHECK(iswdigit(L'7') && !iswdigit(L'a'));
    CHECK(iswalnum(L'z') && iswalnum(L'0') && !iswalnum(L'-'));
    CHECK(iswxdigit(L'F') && iswxdigit(L'9') && !iswxdigit(L'g'));
    CHECK(iswpunct(L'!') && !iswpunct(L'a') && !iswpunct(L' '));
    CHECK(iswprint(L' ') && iswprint(L'x') && !iswprint(L'\n'));
    CHECK(towupper(L'a') == L'A' && towlower(L'Q') == L'q' && towupper(L'1') == L'1');

    wchar_t *end;
    CHECK(wcstol(L"  -123xyz", &end, 10) == -123 && *end == L'x');
    CHECK(wcstol(L"0x1f", NULL, 16) == 31 && wcstol(L"077", NULL, 0) == 63);
    CHECK(wcstol(L"0x1F", NULL, 0) == 31);
    errno = 0;
    CHECK(wcstol(L"99999999999", NULL, 10) == 2147483647L && errno == ERANGE);
    CHECK(wcstoul(L"4294967295", NULL, 10) == 4294967295UL);
    CHECK(wcstol(L"abc", &end, 10) == 0 && *end == L'a');
    CHECK(_wtoi(L" 42abc") == 42 && _wtoi(L"-7") == -7);

    const wchar_t *hello = L"hello";
    CHECK(wcspbrk(hello, L"ol") == hello + 2 && wcspbrk(hello, L"xyz") == NULL);
    CHECK(wcscoll(L"a", L"b") < 0 && wcscoll(L"b", L"a") > 0 && wcscoll(L"a", L"a") == 0);
    wchar_t wx[8];
    CHECK(wcsxfrm(wx, L"abc", 8) == 3 && wcscmp(wx, L"abc") == 0);
    char nx[8];
    CHECK(strxfrm(nx, "abc", 8) == 3 && strcmp(nx, "abc") == 0);
    char cat[16] = "abc";
    CHECK(strncat(cat, "defgh", 2) == cat && strcmp(cat, "abcde") == 0);

    struct tm tm = {0};
    tm.tm_year = 126; tm.tm_mon = 10; tm.tm_mday = 1;
    tm.tm_hour = 12; tm.tm_min = 34; tm.tm_sec = 56;
    wchar_t when[64];
    CHECK(wcsftime(when, 64, L"%Y-%m-%d %H:%M:%S", &tm) == 19);
    CHECK(wcscmp(when, L"2026-11-01 12:34:56") == 0);

    CHECK(_wputenv(L"FZ_TEST=yes") == 0);
    CHECK(_wgetenv(L"FZ_TEST") != NULL && wcscmp(_wgetenv(L"FZ_TEST"), L"yes") == 0);
    CHECK(_wgetenv(L"FZ_NOT_SET") == NULL);

    FILE *f = fopen("fz_a.txt", "w");
    CHECK(f != NULL && fputs("abcdef", f) >= 0);
    fclose(f);
    CHECK(_wrename(L"fz_a.txt", L"fz_b.txt") == 0);
    struct _stat64 st;
    CHECK(_wstat64(L"fz_b.txt", &st) == 0 && st.st_size == 6 && (st.st_mode & _S_IFREG));
    struct __utimbuf64 times = {1000000000, 1000000000};
    CHECK(_wutime64(L"fz_b.txt", &times) == 0);
    CHECK(_wstat64(L"fz_b.txt", &st) == 0 && st.st_mtime == 1000000000);
    int fd = _open("fz_b.txt", _O_RDONLY);
    CHECK(fd >= 0 && _filelengthi64(fd) == 6);
    _close(fd);

    f = fopen("fz_b.txt", "r");
    CHECK(f != NULL && fgetc(f) == 'a' && fgetc(f) == 'b');
    fpos_t pos;
    CHECK(fgetpos(f, &pos) == 0 && fgetc(f) == 'c' && fgetc(f) == 'd');
    CHECK(fsetpos(f, &pos) == 0 && fgetc(f) == 'c');
    fclose(f);
    CHECK(_wremove(L"fz_b.txt") == 0);
    errno = 0;
    CHECK(_wstat64(L"fz_b.txt", &st) == -1 && errno == ENOENT);

    /* Wide characters through a narrow text stream: converted. */
    f = fopen("fz_w.txt", "w");
    CHECK(f != NULL && fputws(L"hi", f) >= 0 && putwc(L'!', f) == L'!');
    fclose(f);
    f = fopen("fz_w.txt", "r");
    CHECK(f != NULL && fgetwc(f) == L'h' && getwc(f) == L'i');
    CHECK(ungetwc(L'i', f) == L'i' && getwc(f) == L'i' && getwc(f) == L'!');
    CHECK(getwc(f) == WEOF);
    fclose(f);
    remove("fz_w.txt");

    wchar_t full[260], cwd[260];
    CHECK(_wfullpath(full, L"x\\..\\y.txt", 260) == full && _wgetcwd(cwd, 260) == cwd);
    size_t n = wcslen(cwd);
    CHECK(wcsncmp(full, cwd, n) == 0 && wcscmp(full + n, L"\\y.txt") == 0);

    CHECK(_wmkdir(L"fz_dir") == 0);
    fclose(fopen("fz_dir\\one.txt", "w"));
    fclose(fopen("fz_dir\\two.txt", "w"));
    struct _wfinddata64_t found;
    intptr_t h = _wfindfirst64(L"fz_dir\\*.txt", &found);
    CHECK(h != -1);
    int files = 1;
    while (_wfindnext64(h, &found) == 0)
        files++;
    CHECK(files == 2 && errno == ENOENT);
    CHECK(_findclose(h) == 0);
    errno = 0;
    CHECK(_wfindfirst64(L"fz_dir\\*.none", &found) == -1 && errno == ENOENT);
    remove("fz_dir\\one.txt");
    remove("fz_dir\\two.txt");
    CHECK(_wrmdir(L"fz_dir") == 0);

    CHECK(_getmaxstdio() == 512);
    CHECK(_setmaxstdio(1024) == 1024 && _getmaxstdio() == 1024);
    CHECK(_setmaxstdio(10) == -1 && _getmaxstdio() == 1024);
    CHECK(_setmaxstdio(512) == 512);

    CHECK(_putws(L"msvcrt_test: _putws") >= 0);
    void *aligned = _aligned_malloc(100, 64);
    CHECK(aligned != NULL && ((uintptr_t)aligned & 63) == 0);
    _aligned_free(aligned);
    CHECK(_beginthread(begin_thread, 0, (void *)5) != (uintptr_t)-1);
    for (int i = 0; i < 1000 && begun == 0; i++)
        Sleep(10);
    CHECK(begun == 5);
    CHECK(_getdrive() == 3 && _chdrive(3) == 0);
}

/* What MinGit's git.exe uses (wide strings and paths, startup, a few
 * more), as msvcrt.dll documents them. */
static void wide_checks(void) {
    wchar_t w[64] = L"ab";
    CHECK(wcscat(w, L"cd") == w && wcscmp(w, L"abcd") == 0);
    CHECK(wcsncat(w, L"efgh", 2) == w && wcscmp(w, L"abcdef") == 0);
    wchar_t pad[8];
    wmemset(pad, L'#', 8);
    CHECK(wcsncpy(pad, L"xy", 5) == pad && pad[0] == L'x' && pad[2] == 0 && pad[4] == 0
          && pad[5] == L'#');
    CHECK(wcsspn(L"aabbcz", L"abc") == 5);
    wchar_t *dup = _wcsdup(L"dup");
    CHECK(dup != NULL && wcscmp(dup, L"dup") == 0);
    free(dup);
    /* msvcrt.dll's swprintf takes no count (the old form). */
    HMODULE crt = GetModuleHandleA("msvcrt.dll");
    int (__cdecl *old_swprintf)(wchar_t *, const wchar_t *, ...) =
        (int (__cdecl *)(wchar_t *, const wchar_t *, ...))GetProcAddress(crt, "swprintf");
    CHECK(old_swprintf != NULL);
    CHECK(old_swprintf(w, L"%d:%ls", 42, L"ab") == 5 && wcscmp(w, L"42:ab") == 0);

    CHECK(_wmkdir(L"mtw_dir") == 0);
    CHECK(_waccess(L"mtw_dir", 0) == 0);
    CHECK(_wchdir(L"mtw_dir") == 0);
    wchar_t cwd[260];
    CHECK(_wgetcwd(cwd, 260) == cwd);
    size_t len = wcslen(cwd);
    CHECK(len > 8 && wcscmp(cwd + len - 8, L"\\mtw_dir") == 0);
    wchar_t *heap_cwd = _wgetcwd(NULL, 0);
    CHECK(heap_cwd != NULL && wcscmp(heap_cwd, cwd) == 0);
    free(heap_cwd);
    CHECK(_wchdir(L"..") == 0);
    int fd = _wopen(L"mtw_dir\\f.txt", _O_CREAT | _O_WRONLY, _S_IREAD | _S_IWRITE);
    CHECK(fd >= 0);
    CHECK(_write(fd, "hi", 2) == 2);
    CHECK(_commit(fd) == 0);
    CHECK(_close(fd) == 0);
    CHECK(_wchmod(L"mtw_dir\\f.txt", _S_IREAD) == 0);
    CHECK(_waccess(L"mtw_dir\\f.txt", 2) == -1 && errno == EACCES);
    CHECK(_wchmod(L"mtw_dir\\f.txt", _S_IREAD | _S_IWRITE) == 0);
    CHECK(_waccess(L"mtw_dir\\f.txt", 2) == 0);
    CHECK(_wunlink(L"mtw_dir\\f.txt") == 0);
    CHECK(_wrmdir(L"mtw_dir") == 0);
    CHECK(_waccess(L"mtw_dir", 0) == -1 && errno == ENOENT);
    wchar_t tmpl[] = L"mtwXXXXXX";
    CHECK(_wmktemp(tmpl) == tmpl && wcsncmp(tmpl, L"mtw", 3) == 0 && tmpl[3] != L'X');

    FILE *f = fopen("mtw_a.txt", "w");
    CHECK(f != NULL);
    CHECK(_wfreopen(L"mtw_b.txt", L"w", f) == f);
    CHECK(fputs("re", f) >= 0);
    fclose(f);
    f = fopen("mtw_b.txt", "r");
    char got[8] = {0};
    CHECK(f != NULL && fread(got, 1, 7, f) == 2 && strcmp(got, "re") == 0);
    fclose(f);
    remove("mtw_a.txt");
    remove("mtw_b.txt");

    int argc = 0;
    wchar_t **argv = NULL, **envp = NULL;
    startup_info si = {0};
    CHECK(__wgetmainargs(&argc, &argv, &envp, 0, &si) == 0);
    CHECK(argc >= 1 && argv != NULL && argv[argc] == NULL && envp != NULL);
    CHECK(wcsstr(argv[0], L"msvcrt_test") != NULL);
    wchar_t ***winitenv = (wchar_t ***)GetProcAddress(crt, "__winitenv");
    CHECK(winitenv != NULL && *winitenv == envp);
    CHECK(GetProcAddress(crt, "_wpgmptr") != NULL);

    CHECK(atol("  -123x") == -123);
    CHECK(iswctype(L'a', _ALPHA) && !iswctype(L'1', _ALPHA) && iswctype(L'1', _DIGIT));
    CHECK(iswctype(L' ', _SPACE) && iswctype(L'F', _HEX | _UPPER) && !iswctype(L'g', _HEX));

    f = fopen("mtw_scan.txt", "w+");
    CHECK(f != NULL && fputs("12 abc\n7\n", f) >= 0);
    rewind(f);
    CHECK(fgetc(f) == '1');
    rewind(f);
    int n = 0, m = 0;
    char word[8] = {0};
    CHECK(fscanf(f, "%d %7s", &n, word) == 2 && n == 12 && strcmp(word, "abc") == 0);
    CHECK(fscanf(f, "%d", &m) == 1 && m == 7);
    CHECK(fscanf(f, "%d", &m) == EOF);
    fclose(f);
    remove("mtw_scan.txt");
    CHECK(_flushall() >= 0);

    int count = 0;
    wchar_t **args = CommandLineToArgvW(L"prog.exe a \"b c\" x\\\"y \"\"", &count);
    CHECK(args != NULL && count == 5);
    CHECK(wcscmp(args[0], L"prog.exe") == 0 && wcscmp(args[1], L"a") == 0);
    CHECK(wcscmp(args[2], L"b c") == 0 && wcscmp(args[3], L"x\"y") == 0);
    CHECK(wcscmp(args[4], L"") == 0 && args[5] == NULL);
    LocalFree(args);
}
#endif

int main(void) {
    /* sscanf: conversions, %p round trip, scan sets. */
    int d = 0; unsigned x = 0; char word[16] = ""; double f = 0;
    CHECK(sscanf("12 0x1f abc 3.5", "%d %x %15s %lf", &d, &x, word, &f) == 4);
    CHECK(d == 12 && x == 31 && strcmp(word, "abc") == 0 && f == 3.5);
    void *p = (void *)0x12345678abcd, *q = NULL;
    char text[32];
    sprintf(text, "%p", p);
    CHECK(sscanf(text, "%p", &q) == 1 && q == p);
    char set[16] = "";
    CHECK(sscanf("key=value", "%15[^=]=", set) == 1 && strcmp(set, "key") == 0);
    CHECK(sscanf("", "%d", &d) == EOF);

    /* _vsnprintf: NULL, 0 measures; too small a buffer gives -1. */
    CHECK(measure("%d-%s", 12345, "abc") == 9);
    char small[4];
    CHECK(format(small, sizeof small, "%s", "12345") == -1 && memcmp(small, "1234", 4) == 0);
    CHECK(format(text, sizeof text, "%I64d|%5.2f|%x", (long long)-7, 2.5, 255) == 11
          && strcmp(text, "-7| 2.50|ff") == 0);

    /* setjmp/longjmp: the value, and 0 becomes 1. */
    volatile int round = 0;
    int got = setjmp(jump);
    if (got == 0) { round = 1; jump_back(7); }
    if (round == 1) { CHECK(got == 7); round = 2; jump_back(0); }
    CHECK(round == 2 && got == 1);

    /* _pipe: write, read, end of file once the write end is closed. */
    int fds[2];
    char buf[8];
    CHECK(_pipe(fds, 256, _O_BINARY) == 0);
    CHECK(_write(fds[1], "abc", 3) == 3);
    CHECK(_read(fds[0], buf, sizeof buf) == 3 && memcmp(buf, "abc", 3) == 0);
    CHECK(_isatty(fds[0]) == 0);

    /* _dup2 onto fd 1 sends standard output into the pipe; back after. */
    fflush(stdout);
    int saved = _dup(1);
    CHECK(saved > 2);
    CHECK(_dup2(fds[1], 1) == 0);
    CHECK(_write(1, "xyz", 3) == 3);
    CHECK(_dup2(saved, 1) == 0);
    _close(saved);
    CHECK(_read(fds[0], buf, sizeof buf) == 3 && memcmp(buf, "xyz", 3) == 0);
    CHECK(_close(fds[1]) == 0);
    CHECK(_read(fds[0], buf, sizeof buf) == 0);
    CHECK(_close(fds[0]) == 0);

    /* The lowest free descriptor: after closing fd 0, _open returns 0 (a
     * shell's background job reopens stdin from the null device so). */
    int saved_in = _dup(0);
    CHECK(saved_in > 2);
    CHECK(_close(0) == 0);
    CHECK(_open("nul", _O_RDONLY) == 0);
    CHECK(_read(0, buf, sizeof buf) == 0);
    CHECK(_dup2(saved_in, 0) == 0);
    _close(saved_in);

    /* Strings and numbers. */
    char list[] = "a,b,,c";
    char *t1 = strtok(list, ","), *t2 = strtok(NULL, ","), *t3 = strtok(NULL, ",");
    CHECK(strcmp(t1, "a") == 0 && strcmp(t2, "b") == 0 && strcmp(t3, "c") == 0 && strtok(NULL, ",") == NULL);
    CHECK(atof("  -2.5e2") == -250.0);
    CHECK(_stricmp("Hello", "hELLO") == 0 && _strnicmp("abcX", "ABCy", 3) == 0 && _stricmp("a", "b") < 0);
    CHECK(mbstowcs(NULL, "h\xc3\xa9llo", 0) == 5 || mbstowcs(NULL, "h\xc3\xa9llo", 0) == 6);
    char cwd[260];
    CHECK(_getcwd(cwd, sizeof cwd) != NULL && cwd[1] == ':');

    /* What TinyCC's startup code and tcc.exe use. */
    CHECK(__argc == 1);
    /* x64: every exception masked, round to nearest (MXCSR 0x1f80). */
    CHECK(_controlfp(0, 0) == 0x8001f);
    CHECK(_controlfp(_RC_CHOP, _MCW_RC) == 0x8031f);
    CHECK((_controlfp(0, 0) & _MCW_RC) == _RC_CHOP);
    CHECK(_controlfp(_RC_NEAR, _MCW_RC) == 0x8001f);
    CHECK(_snprintf(small, sizeof small, "%d", 42) == 2 && strcmp(small, "42") == 0);
    CHECK(_snprintf(small, sizeof small, "%d", 123456) == -1);
    char mixed[] = "MiXeD 1";
    CHECK(_strlwr(mixed) == mixed && strcmp(mixed, "mixed 1") == 0);
    FILE *tmp = fopen("msvcrt_test.tmp", "w");
    CHECK(tmp != NULL && fputs("x", tmp) >= 0);
#ifndef __TINYC__
    /* mingw's _lock_file (its own: msvcrt.dll does not export one) enters
     * the CRITICAL_SECTION that follows a fopen'd FILE in msvcrt.dll
     * (_FILEX): it must be a real, free lock. TinyCC has no _lock_file. */
    _lock_file(tmp);
    _lock_file(tmp);
    CHECK(fputs("y", tmp) >= 0);
    _unlock_file(tmp);
    _unlock_file(tmp);
#endif
    CHECK(fclose(tmp) == 0);
    CHECK(remove("msvcrt_test.tmp") == 0 && remove("msvcrt_test.tmp") == -1);

    /* msvcrt's own strtod (mingw's headers send strtod to libmingwex):
     * the end, overflow, no number, values. */
    typedef double (*strtod_fn)(const char *, char **);
    strtod_fn msvcrt_strtod = (strtod_fn)GetProcAddress(GetModuleHandleA("msvcrt.dll"), "strtod");
    CHECK(msvcrt_strtod != NULL);
    char *end = NULL;
    CHECK(msvcrt_strtod(" 0.1x", &end) == 0.1 && *end == 'x');
    errno = 0;
    CHECK(msvcrt_strtod("1e400;", &end) == HUGE_VAL && errno == ERANGE && *end == ';');
    const char *none = "abc";
    CHECK(msvcrt_strtod(none, &end) == 0.0 && end == none);
    CHECK(msvcrt_strtod("1.7976931348623157e308", NULL) == DBL_MAX);
    CHECK(msvcrt_strtod("-2.5e-3", NULL) == -0.0025);

    /* msvcrt's math functions (looked up: gcc may inline or use
     * libmingwex's), exact results or within an ulp of them. */
    typedef double (*f1)(double);
    typedef double (*f2)(double, double);
    typedef int (*i1)(int);
    HMODULE crt = GetModuleHandleA("msvcrt.dll");
    f2 m_pow = (f2)GetProcAddress(crt, "pow"), m_fmod = (f2)GetProcAddress(crt, "fmod");
    f2 m_atan2 = (f2)GetProcAddress(crt, "atan2");
    f1 m_floor = (f1)GetProcAddress(crt, "floor"), m_ceil = (f1)GetProcAddress(crt, "ceil");
    f1 m_sqrt = (f1)GetProcAddress(crt, "sqrt"), m_sin = (f1)GetProcAddress(crt, "sin");
    f1 m_cos = (f1)GetProcAddress(crt, "cos"), m_exp = (f1)GetProcAddress(crt, "exp");
    f1 m_log = (f1)GetProcAddress(crt, "log"), m_log10 = (f1)GetProcAddress(crt, "log10");
    f1 m_tanh = (f1)GetProcAddress(crt, "tanh");
    i1 m_abs = (i1)GetProcAddress(crt, "abs");
    CHECK(m_pow && m_fmod && m_atan2 && m_floor && m_ceil && m_sqrt && m_sin && m_cos && m_exp
          && m_log && m_log10 && m_tanh && m_abs);
    CHECK(m_pow(2, 10) == 1024 && m_pow(9, 0.5) == 3 && m_pow(2, -1) == 0.5);
    CHECK(m_floor(-3.5) == -4 && m_ceil(3.2) == 4 && m_fmod(7.5, 2) == 1.5 && m_fmod(-7.5, 2) == -1.5);
    CHECK(m_sqrt(16) == 4 && m_sin(0) == 0 && m_cos(0) == 1 && m_exp(0) == 1 && m_log(1) == 0);
    CHECK(fabs(m_log10(1000) - 3) < 1e-15 && fabs(m_atan2(1, 1) * 4 - 3.141592653589793) < 1e-15);
    CHECK(fabs(m_sin(3.141592653589793 / 6) - 0.5) < 1e-15 && m_tanh(0) == 0 && m_abs(-4) == 4);
    CHECK(strcoll("apple", "banana") < 0 && strcoll("x", "x") == 0);
    CHECK(_difftime64(10, 4) == 6.0);

    /* printf's floating-point conversions as msvcrt.dll prints them. */
    static const struct { const char *fmt; double v; const char *want; } floats[] = {
        {"%.14g", 1.0 / 3, "0.33333333333333"},
        {"%.14g", 0.30000000000000004, "0.3"},
        {"%.14g", 1e100, "1e+100"},
        {"%.14g", 2.5e-5, "2.5e-005"},
        {"%.14g", 1024.0, "1024"},
        {"%e", 12345.678, "1.234568e+004"},
        {"%E", 0.000123, "1.230000E-004"},
        {"%g", 0.0001, "0.0001"},
        {"%g", 100000.0, "100000"},
        {"%g", 1e6, "1e+006"},
        {"%g", 123456789.0, "1.23457e+008"},
        {"%.3g", 99.96, "100"},
        {"%f", 1e20, "100000000000000000000.000000"},
        {"%.20f", 0.1, "0.10000000000000001000"},
        {"%08.3f", -3.14159, "-003.142"},
        {"%+.2e", 0.0, "+0.00e+000"},
        {"%.0f", 0.6, "1"},
        {"%#.0f", 2.0, "2."},
        {"%.1f", 0.96, "1.0"},
        {"%10.4f|", 3.14159265, "    3.1416|"},
    };
    for (size_t i = 0; i < sizeof floats / sizeof floats[0]; i++) {
        char got[64];
        sprintf(got, floats[i].fmt, floats[i].v);
        if (strcmp(got, floats[i].want) != 0) {
            printf("msvcrt_test: %s of %.17g gave \"%s\"\n", floats[i].fmt, floats[i].v, got);
        }
        CHECK(strcmp(got, floats[i].want) == 0);
    }

#ifndef __TINYC__
    {
        const wchar_t *w = L"abc";
        CHECK(wcschr(w, L'b') == w + 1 && wcschr(w, L'z') == NULL && wcschr(w, 0) == w + 3);
        char num[40];
        CHECK(_ultoa(4000000000UL, num, 16) == num && strcmp(num, "ee6b2800") == 0);
        CHECK(__mb_cur_max == 1);
        FILE *wf = fopen("msvcrt_test_wide.txt", "w");
        CHECK(wf != NULL);
        CHECK(fwprintf(wf, L"%d-%ls", 5, L"x") == 3);
        CHECK(fputwc(L'z', wf) == L'z');
        fclose(wf);
        wf = fopen("msvcrt_test_wide.txt", "r");
        char back[16] = {0};
        CHECK(wf != NULL && fread(back, 1, sizeof back - 1, wf) == 4 && strcmp(back, "5-xz") == 0);
        fclose(wf);
        remove("msvcrt_test_wide.txt");
        unsigned code = 7, tid = 0;
        HANDLE t = (HANDLE)_beginthreadex(NULL, 0, thread_main, &code, 0, &tid);
        CHECK(t != NULL && tid != 0);
        CHECK(WaitForSingleObject(t, 10000) == WAIT_OBJECT_0);
        DWORD exit_code = 0;
        CHECK(GetExitCodeThread(t, &exit_code) && exit_code == 7);
        CloseHandle(t);
        exit_fn *table = malloc(32 * sizeof *table), *begin = table, *end = table;
        CHECK(__dllonexit(at_exit, &begin, &end) == at_exit);
        CHECK(end - begin == 1 && (void *)begin[0] == (void *)at_exit);
        free(begin);
        wide_checks();
        filezilla_checks();
    }
#endif

    printf("msvcrt_test: ok\n");
    return 0;
}
