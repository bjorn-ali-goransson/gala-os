/* msvcrt functions as real Windows' msvcrt.dll behaves (documented
 * behaviour only): sscanf, _vsnprintf measuring and truncating,
 * setjmp/longjmp, _pipe/_read/_write/_close with end of file, _dup and
 * _dup2 on fd 1, strtok, atof, _stricmp/_strnicmp, mbstowcs, _isatty,
 * _getcwd; __argc, _controlfp, _snprintf, _strlwr and remove (TinyCC's
 * startup code and tcc.exe use them); strtod (tcc parses float
 * constants with it). Built against msvcrt (not mingw's own stdio) by build.sh.
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

/* TinyCC's own float.h (the tcc_self scenario compiles this file) does
 * not declare msvcrt's _controlfp. */
#ifndef _MCW_RC
#define _MCW_RC 0x300
#define _RC_NEAR 0
#define _RC_CHOP 0x300
unsigned int __cdecl _controlfp(unsigned int new_value, unsigned int mask);
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
    CHECK(tmp != NULL && fputs("x", tmp) >= 0 && fclose(tmp) == 0);
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

    printf("msvcrt_test: ok\n");
    return 0;
}
