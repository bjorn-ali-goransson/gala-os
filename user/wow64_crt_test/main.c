/*
 * A 32-bit (x86) C program on msvcrt (ROADMAP WOW stage 3), built by
 * mingw-w64's i686 compiler with its own startup code (crt2.o: the CRT
 * entry through __getmainargs, _initterm, _onexit/atexit, _fpreset) and
 * msvcrt's stdio (not mingw's). What it checks, with the results 32-bit
 * msvcrt.dll gives under 64-bit Windows' WOW64 (its documented behaviour;
 * the x86 layouts from mingw-w64's headers):
 *
 * - __getmainargs: argc and argv of `one "two three"`; getenv's
 *   PROCESSOR_ARCHITECTURE=x86.
 * - printf's variadic arguments in 4-byte stack slots (doubles and
 *   __int64 in 8): %d %u %x %o %c %s %.2s %* with %I64d/%I64x and doubles
 *   between them, %I (32-bit on x86), %p (8 hex digits), %ls/%S, %e and %g
 *   with three exponent digits; _snprintf's truncation; a va_list of the
 *   program's own passed to vsprintf, _vsnprintf, vfprintf and
 *   _vsnwprintf.
 * - sscanf with %d %lf %s %x %I64d %n %f %hd %p (a 4-byte pointer).
 * - strtod, atof, strtol, strtoul, _strtoi64, atoi; malloc (8-byte
 *   alignment), calloc, realloc, free; qsort and bsearch with cdecl
 *   comparators; string functions.
 * - setjmp (_setjmp3) and longjmp: the documented x86 _JUMP_BUFFER (the
 *   SEH registration, the "VC20" cookie, the stack pointer), longjmp's
 *   value (0 becomes 1), and the SEH frames between unwound (their
 *   handler called with EXCEPTION_UNWINDING, fs:[0] restored).
 * - FILE: 32 bytes, stdin/stdout/stderr the first three of _iob with
 *   _file 0, 1, 2; fopen/fprintf/fputs/fclose, fgets/fscanf/fread,
 *   fseek/ftell/rewind/feof, remove; errno ENOENT for a missing file.
 * - The floating-point control: after mingw's startup (its own _fpreset,
 *   fninit) _controlfp reads 0x8001f; msvcrt's _fpreset gives _CW_DEFAULT
 *   (0x9001f: the x87 control word 0x27f, MXCSR 0x1f80); _controlfp sets
 *   the rounding in both the x87 control word and MXCSR (an x87 division
 *   rounds up), the x87 precision, and leaves the denormal mask alone.
 * - localeconv's lconv (4-byte pointers, then the CHAR_MAX fields); the
 *   32-bit time_t of x86 msvcrt.dll's time, gmtime, localtime, mktime,
 *   difftime and ctime (mingw's i686 headers use it); strftime, clock;
 *   sqrt, pow, floor, fmod (doubles returned in st(0)); _beginthreadex
 *   with an __stdcall routine.
 * - atexit: its handler runs after main returns (`atexit ran`).
 *
 * Prints `wow64_crt_test: ok` and exits with 0, or exits with the number
 * of the failing check (`wow64_crt_test: check N (line L) failed` first).
 * Built by build.sh; the binary is checked in as
 * user/bin/wow64_crt_test.exe.
 */
#include <stdio.h>
#include <stdarg.h>
#include <stdlib.h>
#include <string.h>
#include <setjmp.h>
#include <float.h>
#include <errno.h>
#include <math.h>
#include <time.h>
#include <locale.h>
#include <limits.h>
#include <stdint.h>
#include <wchar.h>
#include <process.h>
#include <windows.h>

static int check;
#define CHECK(cond) do { check++; if (!(cond)) { printf("wow64_crt_test: check %d (line %d) failed\n", check, __LINE__); fflush(stdout); exit(check); } } while (0)
#define STREQ(a, b) (strcmp((a), (b)) == 0)

_Static_assert(sizeof(FILE) == 32, "x86 FILE");
_Static_assert(sizeof(jmp_buf) == 64, "x86 jmp_buf");
_Static_assert(sizeof(time_t) == 4, "msvcrt.dll's x86 time_t");

static void bye(void) {
    printf("wow64_crt_test: atexit ran\n");
}

/* The program's own variadic functions: a va_list of 4-byte slots. */
static int fmt(char *buf, const char *format, ...) {
    va_list ap;
    va_start(ap, format);
    int n = vsprintf(buf, format, ap);
    va_end(ap);
    return n;
}

static int nfmt(char *buf, size_t size, const char *format, ...) {
    va_list ap;
    va_start(ap, format);
    int n = _vsnprintf(buf, size, format, ap);
    va_end(ap);
    return n;
}

static int ffmt(FILE *f, const char *format, ...) {
    va_list ap;
    va_start(ap, format);
    int n = vfprintf(f, format, ap);
    va_end(ap);
    return n;
}

static int wfmt(wchar_t *buf, size_t size, const wchar_t *format, ...) {
    va_list ap;
    va_start(ap, format);
    int n = _vsnwprintf(buf, size, format, ap);
    va_end(ap);
    return n;
}

static int __cdecl compare_int(const void *a, const void *b) {
    return *(const int *)a - *(const int *)b;
}

static int __cdecl compare_str(const void *a, const void *b) {
    return strcmp(*(const char *const *)a, *(const char *const *)b);
}

/* SEH: the head of this thread's exception registration chain. */
static void *seh_head(void) {
    void *p;
    __asm__ volatile("movl %%fs:0, %0" : "=r"(p));
    return p;
}

static void seh_set(void *p) {
    __asm__ volatile("movl %0, %%fs:0" : : "r"(p) : "memory");
}

static jmp_buf jump;
static volatile int unwound, unwind_flags;

static EXCEPTION_DISPOSITION __cdecl unwind_handler(EXCEPTION_RECORD *rec, void *frame,
                                                    CONTEXT *context, void *dispatcher) {
    (void)frame, (void)context, (void)dispatcher;
    unwound++;
    unwind_flags = rec->ExceptionFlags;
    return ExceptionContinueSearch;
}

struct registration {
    struct registration *prev;
    void *handler;
};

/* Registers an SEH frame, then longjmps over it. */
static void __attribute__((noinline)) jump_over_frame(int value) {
    struct registration reg;
    reg.prev = seh_head();
    reg.handler = (void *)unwind_handler;
    seh_set(&reg);
    longjmp(jump, value);
}

static void __attribute__((noinline)) jump_plain(int value) {
    longjmp(jump, value);
}

static unsigned __stdcall thread_main(void *arg) {
    char buf[64];
    sprintf(buf, "%d-%.1f", *(int *)arg, 0.5);
    return STREQ(buf, "7-0.5") ? 70 : 1;
}

static unsigned short fpu_control(void) {
    unsigned short cw;
    __asm__ volatile("fnstcw %0" : "=m"(cw));
    return cw;
}

static unsigned mxcsr(void) {
    unsigned v;
    __asm__ volatile("stmxcsr %0" : "=m"(v));
    return v;
}

int main(int argc, char **argv) {
    char buf[256];
    printf("wow64_crt_test: hello %d %s %.2f\n", 32, "bit", 1.5);
    atexit(bye);

    /* 1: arguments and environment. */
    CHECK(argc == 3);
    CHECK(STREQ(argv[1], "one"));
    CHECK(STREQ(argv[2], "two three"));
    CHECK(argv[3] == NULL);
    const char *arch = getenv("PROCESSOR_ARCHITECTURE");
    CHECK(arch != NULL && STREQ(arch, "x86"));

    /* 2: printf's argument walking and conversions. */
    sprintf(buf, "%d|%5d|%-5d|%05d|%u|%x|%X|%o|%c|%s|%.2s|%%", -42, 42, 42, 42, 3000000000u,
            255, 255, 8, 'A', "str", "abc");
    CHECK(STREQ(buf, "-42|   42|42   |00042|3000000000|ff|FF|10|A|str|ab|%"));
    sprintf(buf, "%I64d %d %.3f %I64x %s %g", -1234567890123LL, 7, 2.5, 0x123456789abcULL, "end",
            0.5);
    CHECK(STREQ(buf, "-1234567890123 7 2.500 123456789abc end 0.5"));
    sprintf(buf, "%Iu %d %Ix", (size_t)4000000000u, 5, (size_t)0xdeadbeef);
    CHECK(STREQ(buf, "4000000000 5 deadbeef"));
    sprintf(buf, "%p|%d", (void *)0x1234abcd, 9);
    CHECK(STREQ(buf, "1234ABCD|9"));
    sprintf(buf, "%*d|%-*.*f|%d", 6, 42, 8, 2, 1.25, 3);
    CHECK(STREQ(buf, "    42|1.25    |3"));
    sprintf(buf, "%e|%.3e|%g|%g|%8.3f", 12345.678, 0.000123, 1e-5, 123456789.0, 3.14159);
    CHECK(STREQ(buf, "1.234568e+004|1.230e-004|1e-005|1.23457e+008|   3.142"));
    sprintf(buf, "%ls|%S|%d", L"wide", L"W2", 4);
    CHECK(STREQ(buf, "wide|W2|4"));
    memset(buf, 'x', 16);
    CHECK(_snprintf(buf, 5, "%s", "toolong") == -1);
    CHECK(memcmp(buf, "toolox", 6) == 0);
    CHECK(_snprintf(buf, 10, "%d", 12) == 2 && STREQ(buf, "12"));
    CHECK(fmt(buf, "%d %I64d %f %s %c", 1, (long long)1 << 40, 0.25, "x", 'y') == 28);
    CHECK(STREQ(buf, "1 1099511627776 0.250000 x y"));
    CHECK(nfmt(buf, sizeof buf, "%.1f|%I64u|%d", -2.25, 18446744073709551615ULL, -1) == 28);
    CHECK(STREQ(buf, "-2.3|18446744073709551615|-1"));
    wchar_t wbuf[64];
    CHECK(wfmt(wbuf, 64, L"%d|%s|%hs|%.1f", 5, L"wide", "narrow", 0.5) == 17);
    CHECK(wcscmp(wbuf, L"5|wide|narrow|0.5") == 0);

    /* 3: sscanf. */
    {
        const char *in = "  12 -3 4.5 abc 1f 1234567890123 xyz";
        int a = 0, b = 0, x = 0, n = 0;
        double d = 0;
        long long ll = 0;
        char s[16];
        CHECK(sscanf(in, "%d %d %lf %s %x %I64d%n", &a, &b, &d, s, &x, &ll, &n) == 6);
        CHECK(a == 12 && b == -3 && d == 4.5 && STREQ(s, "abc") && x == 31);
        CHECK(ll == 1234567890123LL && n == (int)(strstr(in, " xyz") - in));
        float f = 0;
        short sh = 0;
        void *p = NULL;
        int guard = 0x5a5a5a5a;
        CHECK(sscanf("3.25 7 0012FF7C", "%f %hd %p", &f, &sh, &p) == 3);
        CHECK(f == 3.25f && sh == 7 && p == (void *)0x12ff7c && guard == 0x5a5a5a5a);
    }

    /* 4: numbers from strings. */
    {
        char *end;
        CHECK(strtod("1.5e3xyz", &end) == 1500.0 && STREQ(end, "xyz"));
        CHECK(atof("-2.25") == -2.25);
        CHECK(strtol("0x1F", &end, 16) == 31 && *end == 0);
        CHECK(strtol("-123abc", &end, 10) == -123 && STREQ(end, "abc"));
        CHECK(strtoul("4294967295", NULL, 10) == 4294967295u);
        CHECK(_strtoi64("-9000000000", NULL, 10) == -9000000000LL);
        CHECK(atoi("  42z") == 42);
    }

    /* 5: the heap. */
    {
        char *p = malloc(100);
        CHECK(p != NULL && ((uintptr_t)p & 7) == 0);
        memset(p, 'q', 100);
        p = realloc(p, 10000);
        CHECK(p != NULL && p[0] == 'q' && p[99] == 'q');
        free(p);
        unsigned char *z = calloc(10, 10);
        CHECK(z != NULL);
        int zero = 1;
        for (int i = 0; i < 100; i++)
            zero &= z[i] == 0;
        CHECK(zero);
        free(z);
    }

    /* 6: qsort, bsearch, strings. */
    {
        int v[] = {5, 3, 9, 1, 7};
        qsort(v, 5, sizeof v[0], compare_int);
        CHECK(v[0] == 1 && v[1] == 3 && v[2] == 5 && v[3] == 7 && v[4] == 9);
        int key = 7;
        int *hit = bsearch(&key, v, 5, sizeof v[0], compare_int);
        CHECK(hit == &v[3]);
        const char *names[] = {"pear", "apple", "fig"};
        qsort(names, 3, sizeof names[0], compare_str);
        CHECK(STREQ(names[0], "apple") && STREQ(names[1], "fig") && STREQ(names[2], "pear"));
        strcpy(buf, "ab");
        strcat(buf, "cd");
        CHECK(STREQ(buf, "abcd") && strlen(buf) == 4 && strrchr(buf, 'c') == buf + 2);
        CHECK(_stricmp("HeLLo", "hello") == 0 && strstr(buf, "bc") == buf + 1);
    }

    /* 7: setjmp/longjmp and the x86 jump buffer. */
    {
        volatile int count = 0;
        void *head = seh_head();
        int r = setjmp(jump);
        if (r == 0) {
            _JUMP_BUFFER *j = (_JUMP_BUFFER *)jump;
            CHECK(j->Registration == (unsigned long)head);
            CHECK(j->Cookie == 0x56433230); /* "VC20": _setjmp3's */
            CHECK(j->Eip != 0);
            CHECK(j->Esp - (unsigned long)&count < 4096 ||
                  (unsigned long)&count - j->Esp < 4096);
            count = 1;
            jump_plain(0);
        }
        CHECK(r == 1 && count == 1);
        r = setjmp(jump);
        if (r == 0) {
            count = 2;
            jump_over_frame(5);
        }
        CHECK(r == 5 && count == 2);
        CHECK(unwound == 1 && (unwind_flags & 2)); /* EXCEPTION_UNWINDING */
        CHECK(seh_head() == head);
    }

    /* 8: files. */
    {
        FILE *in = stdin, *out = stdout, *err = stderr;
        CHECK((char *)out - (char *)in == 32 && (char *)err - (char *)in == 64);
        CHECK(in->_file == 0 && out->_file == 1 && err->_file == 2);
        CHECK(_fileno(stdout) == 1);
        errno = 0;
        CHECK(fopen("C:\\no such dir\\x.txt", "rb") == NULL && errno == ENOENT);
        FILE *f = fopen("crt32.txt", "wb");
        CHECK(f != NULL);
        CHECK(fprintf(f, "line1 %d\nline2 %.1f\n", 1, 2.5) == 18);
        CHECK(ffmt(f, "%s %I64d\n", "big", 1LL << 33) == 15);
        CHECK(fputs("tail", f) >= 0);
        CHECK(fclose(f) == 0);
        f = fopen("crt32.txt", "rb");
        CHECK(f != NULL);
        CHECK(fgets(buf, sizeof buf, f) == buf && STREQ(buf, "line1 1\n"));
        double d = 0;
        CHECK(fscanf(f, "line2 %lf", &d) == 1 && d == 2.5);
        CHECK(fseek(f, 0, SEEK_END) == 0 && ftell(f) == 37);
        rewind(f);
        size_t got = fread(buf, 1, sizeof buf, f);
        CHECK(got == 37 && feof(f));
        buf[got] = 0;
        CHECK(STREQ(buf, "line1 1\nline2 2.5\nbig 8589934592\ntail"));
        CHECK(fclose(f) == 0);
        CHECK(remove("crt32.txt") == 0);
        CHECK(fopen("crt32.txt", "rb") == NULL);
    }

    /* 9: the floating-point control (x87 and SSE). */
    {
        /* mingw's startup ran its own _fpreset (fninit: 64-bit precision). */
        CHECK(fpu_control() == 0x37f && _controlfp(0, 0) == 0x8001f);
        void(__cdecl * crt_fpreset)(void) =
            (void(__cdecl *)(void))GetProcAddress(GetModuleHandleA("msvcrt.dll"), "_fpreset");
        CHECK(crt_fpreset != NULL);
        crt_fpreset();
        CHECK(_controlfp(0, 0) == 0x9001f); /* _CW_DEFAULT */
        CHECK(fpu_control() == 0x27f);
        CHECK((mxcsr() & 0xffc0) == 0x1f80);
        CHECK(_controlfp(_RC_UP, _MCW_RC) == 0x9021f);
        CHECK(fpu_control() == 0xa7f);
        CHECK(((mxcsr() >> 13) & 3) == 2);
        volatile double one = 1.0, three = 3.0;
        double third = one / three; /* x87 */
        uint64_t bits;
        memcpy(&bits, &third, 8);
        CHECK(bits == 0x3fd5555555555556ULL);
        CHECK(_controlfp(_RC_NEAR, _MCW_RC) == 0x9001f);
        third = one / three;
        memcpy(&bits, &third, 8);
        CHECK(bits == 0x3fd5555555555555ULL);
        CHECK(_controlfp(_PC_24, _MCW_PC) == 0xa001f);
        CHECK((fpu_control() & 0x300) == 0);
        CHECK(_controlfp(_PC_53, _MCW_PC) == 0x9001f);
        /* _controlfp leaves the denormal mask (_control87 changes it). */
        CHECK(_controlfp(0, _EM_DENORMAL) == 0x9001f);
        CHECK(fpu_control() & 2);
        crt_fpreset();
        CHECK(fpu_control() == 0x27f);
    }

    /* 10: locale, time, math, threads. */
    {
        struct lconv *lc = localeconv();
        CHECK(STREQ(lc->decimal_point, ".") && STREQ(lc->thousands_sep, ""));
        CHECK(STREQ(lc->currency_symbol, "") && lc->int_frac_digits == CHAR_MAX);
        CHECK(lc->n_sign_posn == CHAR_MAX);
        time_t now = time(NULL);
        CHECK(now > 1500000000);
        time_t t = 1000000000;
        struct tm *g = gmtime(&t);
        CHECK(g->tm_year == 101 && g->tm_mon == 8 && g->tm_mday == 9 && g->tm_hour == 1);
        CHECK(g->tm_min == 46 && g->tm_sec == 40 && g->tm_wday == 0 && g->tm_yday == 251);
        CHECK(strftime(buf, sizeof buf, "%Y-%m-%d %H:%M:%S %a %b %j", g) == 31);
        CHECK(STREQ(buf, "2001-09-09 01:46:40 Sun Sep 252"));
        struct tm local = *localtime(&t);
        CHECK(mktime(&local) == t);
        CHECK(difftime(t + 10, t) == 10.0);
        CHECK(strlen(ctime(&t)) == 25);
        CHECK(clock() >= 0);
        volatile double two = 2.0;
        CHECK(fabs(sqrt(two) * sqrt(two) - 2.0) < 1e-15);
        CHECK(pow(two, 10.0) == 1024.0 && floor(-1.5) == -2.0 && fmod(7.5, two) == 1.5);
        int arg = 7;
        unsigned id;
        HANDLE h = (HANDLE)_beginthreadex(NULL, 0, thread_main, &arg, 0, &id);
        CHECK(h != NULL);
        CHECK(WaitForSingleObject(h, 10000) == WAIT_OBJECT_0);
        DWORD code = 0;
        CHECK(GetExitCodeThread(h, &code) && code == 70);
        CloseHandle(h);
    }

    printf("wow64_crt_test: ok\n");
    return 0;
}
