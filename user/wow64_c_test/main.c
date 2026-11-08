/*
 * A plain 32-bit (x86) C program on kernel32 (ROADMAP WOW stage 3), as a C
 * compiler builds one: stdcall imports bound through an import library
 * (`_Name@N` symbols, undecorated names in the DLL), no C runtime. What it
 * checks, with the results 64-bit Windows' WOW64 gives (as documented):
 *
 * - GetStdHandle and WriteFile; GetCommandLineA and GetModuleFileNameA
 *   name this program.
 * - The last error: SetLastError/GetLastError, and TEB32.LastErrorValue
 *   (fs:[0x34]) holds it.
 * - GetSystemInfo of an image that is not large-address-aware: a 4 KiB
 *   page, 64 KiB granularity, the range 0x10000 to 0x7ffeffff,
 *   PROCESSOR_ARCHITECTURE_INTEL; VirtualAlloc gives memory below 2 GiB
 *   and VirtualQuery its 28-byte MEMORY_BASIC_INFORMATION.
 * - The process heap (HeapAlloc, HeapSize, HeapFree), thread-local
 *   storage (TlsAlloc, TlsSetValue, TlsGetValue in another thread), an
 *   interlocked singly linked list (8-byte SLIST_HEADER), a thread
 *   (stdcall start routine, its exit code), Sleep, GetTickCount,
 *   QueryPerformanceCounter, PROCESSOR_ARCHITECTURE=x86.
 *
 * Prints `wow64_c_test: ok` and exits with 0, or exits with the number of
 * the failing check (`wow64_c_test: check N failed, got 0x...` first).
 * Built by build.sh (clang and lld-link for i686-pc-windows-msvc); the
 * binary is checked in as user/bin/wow64_c_test.exe.
 */

typedef unsigned int DWORD;
typedef int BOOL;
typedef void *HANDLE;
typedef unsigned short WORD;
typedef unsigned long long ULONGLONG;
typedef __SIZE_TYPE__ SIZE_T;

#define WINAPI __stdcall
#define IMPORT __declspec(dllimport)

typedef struct {
    WORD wProcessorArchitecture;
    WORD wReserved;
    DWORD dwPageSize;
    void *lpMinimumApplicationAddress;
    void *lpMaximumApplicationAddress;
    SIZE_T dwActiveProcessorMask;
    DWORD dwNumberOfProcessors;
    DWORD dwProcessorType;
    DWORD dwAllocationGranularity;
    WORD wProcessorLevel;
    WORD wProcessorRevision;
} SYSTEM_INFO;

typedef struct {
    void *BaseAddress;
    void *AllocationBase;
    DWORD AllocationProtect;
    SIZE_T RegionSize;
    DWORD State;
    DWORD Protect;
    DWORD Type;
} MEMORY_BASIC_INFORMATION;

typedef struct SLIST_ENTRY {
    struct SLIST_ENTRY *Next;
} SLIST_ENTRY;

typedef union {
    ULONGLONG Alignment;
    struct {
        SLIST_ENTRY Next;
        WORD Depth;
        WORD Sequence;
    } s;
} SLIST_HEADER;

IMPORT HANDLE WINAPI GetStdHandle(DWORD which);
IMPORT BOOL WINAPI WriteFile(HANDLE h, const void *buf, DWORD len, DWORD *written, void *ov);
IMPORT char *WINAPI GetCommandLineA(void);
IMPORT DWORD WINAPI GetModuleFileNameA(HANDLE module, char *buf, DWORD size);
IMPORT void WINAPI SetLastError(DWORD code);
IMPORT DWORD WINAPI GetLastError(void);
IMPORT void WINAPI GetSystemInfo(SYSTEM_INFO *info);
IMPORT void *WINAPI VirtualAlloc(void *addr, SIZE_T size, DWORD type, DWORD protect);
IMPORT SIZE_T WINAPI VirtualQuery(const void *addr, MEMORY_BASIC_INFORMATION *info, SIZE_T len);
IMPORT BOOL WINAPI VirtualFree(void *addr, SIZE_T size, DWORD type);
IMPORT HANDLE WINAPI GetProcessHeap(void);
IMPORT void *WINAPI HeapAlloc(HANDLE heap, DWORD flags, SIZE_T size);
IMPORT SIZE_T WINAPI HeapSize(HANDLE heap, DWORD flags, const void *p);
IMPORT BOOL WINAPI HeapFree(HANDLE heap, DWORD flags, void *p);
IMPORT DWORD WINAPI TlsAlloc(void);
IMPORT BOOL WINAPI TlsSetValue(DWORD index, void *value);
IMPORT void *WINAPI TlsGetValue(DWORD index);
IMPORT BOOL WINAPI TlsFree(DWORD index);
IMPORT void WINAPI InitializeSListHead(SLIST_HEADER *head);
IMPORT SLIST_ENTRY *WINAPI InterlockedPushEntrySList(SLIST_HEADER *head, SLIST_ENTRY *entry);
IMPORT SLIST_ENTRY *WINAPI InterlockedPopEntrySList(SLIST_HEADER *head);
IMPORT WORD WINAPI QueryDepthSList(SLIST_HEADER *head);
IMPORT HANDLE WINAPI CreateThread(void *security, SIZE_T stack, DWORD(WINAPI *start)(void *),
                                  void *param, DWORD flags, DWORD *id);
IMPORT DWORD WINAPI WaitForSingleObject(HANDLE h, DWORD ms);
IMPORT BOOL WINAPI GetExitCodeThread(HANDLE h, DWORD *code);
IMPORT BOOL WINAPI CloseHandle(HANDLE h);
IMPORT void WINAPI Sleep(DWORD ms);
IMPORT DWORD WINAPI GetTickCount(void);
IMPORT BOOL WINAPI QueryPerformanceCounter(long long *count);
IMPORT DWORD WINAPI GetEnvironmentVariableA(const char *name, char *buf, DWORD size);
IMPORT void WINAPI ExitProcess(DWORD code) __attribute__((noreturn));

#define STD_OUTPUT_HANDLE ((DWORD)-11)
#define MEM_COMMIT 0x1000
#define MEM_RESERVE 0x2000
#define MEM_RELEASE 0x8000
#define PAGE_READWRITE 0x04
#define INFINITE 0xffffffff
#define TLS_OUT_OF_INDEXES 0xffffffff

/* The compiler may call memset for zeroed locals; there is no C runtime. */
void *memset(void *dst, int value, SIZE_T n) {
    volatile unsigned char *d = dst;
    while (n--)
        *d++ = (unsigned char)value;
    return dst;
}

static unsigned len(const char *s) {
    unsigned n = 0;
    while (s[n])
        n++;
    return n;
}

static void print(const char *s) {
    DWORD written;
    WriteFile(GetStdHandle(STD_OUTPUT_HANDLE), s, len(s), &written, 0);
}

static void hex(DWORD v, char *out) {
    for (int i = 0; i < 8; i++)
        out[i] = "0123456789abcdef"[(v >> (28 - 4 * i)) & 0xf];
    out[8] = 0;
}

static DWORD checks;

/* Counts a check; on failure prints it and ends the process with its
   number. */
static void check(int cond, DWORD got) {
    char n[9], g[9];
    checks++;
    if (cond)
        return;
    hex(checks, n);
    hex(got, g);
    print("wow64_c_test: check ");
    print(n);
    print(" failed, got 0x");
    print(g);
    print("\r\n");
    ExitProcess(checks);
}

/* Whether `s` ends with `tail` (ASCII, any case). */
static int ends_with(const char *s, const char *tail) {
    unsigned a = len(s), b = len(tail);
    if (b > a)
        return 0;
    for (unsigned i = 0; i < b; i++)
        if ((s[a - b + i] | 0x20) != (tail[i] | 0x20))
            return 0;
    return 1;
}

static int contains(const char *s, const char *part) {
    unsigned a = len(s), b = len(part);
    for (unsigned i = 0; i + b <= a; i++) {
        unsigned j = 0;
        while (j < b && (s[i + j] | 0x20) == (part[j] | 0x20))
            j++;
        if (j == b)
            return 1;
    }
    return 0;
}

static DWORD tls_index;
static volatile DWORD thread_saw;

static DWORD WINAPI thread(void *param) {
    /* A new thread's slot starts out null. */
    thread_saw = TlsGetValue(tls_index) == 0 ? 1 : 2;
    TlsSetValue(tls_index, param);
    return (DWORD)(SIZE_T)TlsGetValue(tls_index) + 1;
}

/* TEB32.LastErrorValue (fs:[0x34]). */
static DWORD teb_last_error(void) {
    DWORD v;
    __asm__ volatile("movl %%fs:0x34, %0" : "=r"(v));
    return v;
}

void mainCRTStartup(void) {
    print("wow64_c_test: hello\r\n");

    /* The program's command line and file. */
    check(contains(GetCommandLineA(), "wow64_c_test"), 0);
    char path[260];
    DWORD n = GetModuleFileNameA(0, path, sizeof path);
    check(n > 0 && n < sizeof path && ends_with(path, "\\wow64_c_test.exe"), n);

    /* The last error, where the TEB32 keeps it. */
    SetLastError(1234);
    check(GetLastError() == 1234, GetLastError());
    check(teb_last_error() == 1234, teb_last_error());

    /* The system as a 32-bit program without /LARGEADDRESSAWARE sees it. */
    SYSTEM_INFO si;
    GetSystemInfo(&si);
    check(si.wProcessorArchitecture == 0, si.wProcessorArchitecture);
    check(si.dwPageSize == 4096, si.dwPageSize);
    check(si.dwAllocationGranularity == 0x10000, si.dwAllocationGranularity);
    check(si.lpMinimumApplicationAddress == (void *)0x10000,
          (DWORD)(SIZE_T)si.lpMinimumApplicationAddress);
    check(si.lpMaximumApplicationAddress == (void *)0x7ffeffff,
          (DWORD)(SIZE_T)si.lpMaximumApplicationAddress);
    check(si.dwNumberOfProcessors >= 1, si.dwNumberOfProcessors);

    /* Memory below 2 GiB, and its description. */
    char *p = VirtualAlloc(0, 0x20000, MEM_RESERVE | MEM_COMMIT, PAGE_READWRITE);
    check(p != 0 && (SIZE_T)p < 0x80000000u, (DWORD)(SIZE_T)p);
    p[0x1ffff] = 7;
    MEMORY_BASIC_INFORMATION mbi;
    check(VirtualQuery(p + 0x1000, &mbi, sizeof mbi) == 28, 0);
    check(mbi.BaseAddress == p + 0x1000 && mbi.AllocationBase == p,
          (DWORD)(SIZE_T)mbi.BaseAddress);
    check(mbi.RegionSize == 0x1f000, (DWORD)mbi.RegionSize);
    check(mbi.State == MEM_COMMIT && mbi.Protect == PAGE_READWRITE, mbi.State);
    check(VirtualFree(p, 0, MEM_RELEASE) == 1, GetLastError());

    /* The process heap. */
    HANDLE heap = GetProcessHeap();
    char *block = HeapAlloc(heap, 0, 100);
    check(block != 0, 0);
    block[99] = 1;
    check(HeapSize(heap, 0, block) == 100, (DWORD)HeapSize(heap, 0, block));
    check(HeapFree(heap, 0, block) == 1, GetLastError());

    /* Thread-local storage and a thread. */
    tls_index = TlsAlloc();
    check(tls_index != TLS_OUT_OF_INDEXES, tls_index);
    check(TlsSetValue(tls_index, (void *)0x1234) == 1, GetLastError());
    DWORD id = 0;
    HANDLE t = CreateThread(0, 0, thread, (void *)0x41, 0, &id);
    check(t != 0 && id != 0, GetLastError());
    check(WaitForSingleObject(t, INFINITE) == 0, GetLastError());
    DWORD code = 0;
    check(GetExitCodeThread(t, &code) == 1 && code == 0x42, code);
    check(thread_saw == 1, thread_saw);
    check(TlsGetValue(tls_index) == (void *)0x1234, (DWORD)(SIZE_T)TlsGetValue(tls_index));
    check(CloseHandle(t) == 1, GetLastError());
    check(TlsFree(tls_index) == 1, GetLastError());

    /* An interlocked singly linked list. */
    static SLIST_HEADER head __attribute__((aligned(8)));
    static SLIST_ENTRY entries[3];
    check(sizeof head == 8, sizeof head);
    InitializeSListHead(&head);
    check(QueryDepthSList(&head) == 0, QueryDepthSList(&head));
    for (int i = 0; i < 3; i++)
        InterlockedPushEntrySList(&head, &entries[i]);
    check(QueryDepthSList(&head) == 3 && head.s.Depth == 3, QueryDepthSList(&head));
    check(head.s.Next.Next == &entries[2], (DWORD)(SIZE_T)head.s.Next.Next);
    check(InterlockedPopEntrySList(&head) == &entries[2], 0);
    check(QueryDepthSList(&head) == 2, QueryDepthSList(&head));

    /* Time. */
    DWORD t0 = GetTickCount();
    long long c0, c1;
    check(QueryPerformanceCounter(&c0) == 1, 0);
    Sleep(20);
    check(QueryPerformanceCounter(&c1) == 1 && c1 > c0, (DWORD)(c1 - c0));
    check(GetTickCount() - t0 >= 10, GetTickCount() - t0);

    /* WOW64's environment. */
    char arch[16];
    check(GetEnvironmentVariableA("PROCESSOR_ARCHITECTURE", arch, sizeof arch) == 3, 0);
    check(arch[0] == 'x' && arch[1] == '8' && arch[2] == '6' && arch[3] == 0, arch[0]);

    print("wow64_c_test: ok\r\n");
    ExitProcess(0);
}
