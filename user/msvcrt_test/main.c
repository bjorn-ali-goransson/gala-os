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
 * __dllonexit (appending to a DLL's exit table). Built against msvcrt
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
typedef void (__cdecl *exit_fn)(void);
_onexit_t __cdecl __dllonexit(_onexit_t func, exit_fn **begin, exit_fn **end);
static unsigned __stdcall thread_main(void *arg) {
    _endthreadex(*(unsigned *)arg);
    return 99; /* not reached */
}
static int __cdecl at_exit(void) { return 0; }
#endif

static int check;
#define CHECK(cond) do { check++; if (!(cond)) { printf("msvcrt_test: check %d failed\n", check); fflush(stdout); exit(check); } } while (0)

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
    }
#endif

    printf("msvcrt_test: ok\n");
    return 0;
}
