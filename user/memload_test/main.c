/* Memory stays bounded under sustained work, as on Windows, in a machine
 * with less memory than the work moves (ROADMAP OF: the Office
 * Click-to-Run client downloads gigabytes in 256 KiB ranges, a thread
 * per range, onto C: in a 512 MiB VM; the scenario runs this in a 256 MiB
 * VM, whose file cache holds less than what it downloads):
 *
 * 1. Thread churn: 2000 threads made with CreateThread, each touching
 *    32 KiB of its stack, waited for and closed. A thread's stack and
 *    TEB go when it exits (CreateThread's documentation: "the stack is
 *    freed when the thread terminates"), so the process's private bytes
 *    (PROCESS_MEMORY_COUNTERS_EX.PrivateUsage) and its reserved address
 *    space (GlobalMemoryStatusEx's ullTotalVirtual - ullAvailVirtual)
 *    after the 2000 are within 1 MiB and 4 MiB of what they were after
 *    the first 200.
 * 2. Downloads onto a file: a 4 MiB file (C:\memload\blob.bin, served by
 *    Nginx as http://127.0.0.1:8080/memload/blob.bin: the location in
 *    user/schannel_test/nginx.conf) is fetched in 256 KiB ranges
 *    (Range: bytes=..., 206 Partial Content), each range by a new thread
 *    over WinHTTP, and written to C:\memload\out.bin at the next offset,
 *    until the file holds <MiB> MiB (the argument; more than the
 *    machine's file cache can hold). The machine's available memory
 *    (GlobalMemoryStatusEx's ullAvailPhys: free and reclaimable cache)
 *    never falls below an eighth of its total, sampled every 16 MiB.
 * 3. The file is read back through mapped views of 16 MiB, each
 *    unmapped before the next: every range holds the blob's bytes;
 *    available memory stays above an eighth of the total.
 *
 * Expected results are Windows' (documented behaviour; the thresholds
 * are this test's). Prints "memload_test: ok", or "memload_test: check N
 * (line L) failed" and exits with N. Retries its first request while
 * Nginx starts. Run with the size in MiB (default 192).
 */
#include <windows.h>
#include <winhttp.h>
#include <psapi.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static int check;
#define CHECK(cond) do { check++; if (!(cond)) { printf("memload_test: check %d (line %d) failed, error %lu\n", check, __LINE__, GetLastError()); fflush(stdout); ExitProcess(check); } } while (0)

#define BLOB (4u << 20)
#define RANGE (256u << 10)
#define THREADS 2000

static BYTE blob_byte(DWORD at) {
    return (BYTE)(at * 7 + at / 4096 + 1);
}

static DWORD WINAPI touch(void *param) {
    volatile char buf[32768];
    for (unsigned i = 0; i < sizeof buf; i += 4096)
        buf[i] = (char)i;
    (void)param;
    /* 0: every touched byte is (char)i, a multiple of 4096. */
    return (DWORD)(buf[0] + buf[4096]);
}

/* Private bytes and reserved address space of this process. */
static void usage(SIZE_T *priv, ULONGLONG *reserved) {
    PROCESS_MEMORY_COUNTERS_EX pmc;
    CHECK(GetProcessMemoryInfo(GetCurrentProcess(), (PROCESS_MEMORY_COUNTERS *)&pmc, sizeof pmc));
    *priv = pmc.PrivateUsage;
    MEMORYSTATUSEX m = {sizeof m};
    CHECK(GlobalMemoryStatusEx(&m));
    *reserved = m.ullTotalVirtual - m.ullAvailVirtual;
}

static ULONGLONG min_avail = ~0ull, total_phys;

static void sample(void) {
    MEMORYSTATUSEX m = {sizeof m};
    CHECK(GlobalMemoryStatusEx(&m));
    total_phys = m.ullTotalPhys;
    if (m.ullAvailPhys < min_avail)
        min_avail = m.ullAvailPhys;
}

static HINTERNET session;
static HANDLE out;

struct job {
    DWORD index;
    DWORD error;
    DWORD status;
    BYTE *buf;
};

/* GET the blob's range for `index` and write it to out.bin. */
static DWORD WINAPI fetch(void *param) {
    struct job *j = param;
    DWORD from = (j->index % (BLOB / RANGE)) * RANGE;
    wchar_t range[64];
    swprintf(range, 64, L"Range: bytes=%lu-%lu", (unsigned long)from, (unsigned long)(from + RANGE - 1));
    HINTERNET c = WinHttpConnect(session, L"127.0.0.1", 8080, 0);
    if (!c) {
        j->error = GetLastError();
        return 0;
    }
    HINTERNET r = WinHttpOpenRequest(c, L"GET", L"/memload/blob.bin", NULL, WINHTTP_NO_REFERER,
                                     WINHTTP_DEFAULT_ACCEPT_TYPES, 0);
    DWORD total = 0;
    if (!r || !WinHttpSendRequest(r, range, (DWORD)-1L, WINHTTP_NO_REQUEST_DATA, 0, 0, 0) ||
        !WinHttpReceiveResponse(r, NULL)) {
        j->error = GetLastError();
    } else {
        DWORD len = sizeof j->status, got;
        WinHttpQueryHeaders(r, WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER, WINHTTP_HEADER_NAME_BY_INDEX,
                            &j->status, &len, WINHTTP_NO_HEADER_INDEX);
        while (total < RANGE && WinHttpReadData(r, j->buf + total, RANGE - total, &got) && got > 0)
            total += got;
        if (total != RANGE)
            j->error = total + 1;
    }
    if (r)
        WinHttpCloseHandle(r);
    WinHttpCloseHandle(c);
    if (j->error == 0) {
        OVERLAPPED o = {0};
        ULONGLONG at = (ULONGLONG)j->index * RANGE;
        o.Offset = (DWORD)at;
        o.OffsetHigh = (DWORD)(at >> 32);
        DWORD wrote = 0;
        if (!WriteFile(out, j->buf, RANGE, &wrote, &o) || wrote != RANGE)
            j->error = GetLastError() | 0x10000000;
    }
    return 0;
}

int main(int argc, char **argv) {
    DWORD mib = argc > 1 ? (DWORD)atoi(argv[1]) : 192;
    CHECK(mib >= 16);
    /* 1. */
    SIZE_T priv0 = 0, priv1;
    ULONGLONG res0 = 0, res1;
    for (DWORD i = 0; i < THREADS; i++) {
        HANDLE h = CreateThread(NULL, 0, touch, (void *)(size_t)i, 0, NULL);
        CHECK(h != NULL);
        CHECK(WaitForSingleObject(h, INFINITE) == WAIT_OBJECT_0);
        CloseHandle(h);
        if (i == 199)
            usage(&priv0, &res0);
    }
    usage(&priv1, &res1);
    printf("memload_test: threads: private %lu -> %lu KiB, reserved %lu -> %lu KiB\n",
           (unsigned long)(priv0 >> 10), (unsigned long)(priv1 >> 10), (unsigned long)(res0 >> 10),
           (unsigned long)(res1 >> 10));
    fflush(stdout);
    CHECK(priv1 <= priv0 + (1 << 20));
    CHECK(res1 <= res0 + (4 << 20));

    /* 2. */
    CHECK(CreateDirectoryW(L"C:\\memload", NULL) || GetLastError() == ERROR_ALREADY_EXISTS);
    BYTE *blob = VirtualAlloc(NULL, BLOB, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE);
    CHECK(blob != NULL);
    for (DWORD i = 0; i < BLOB; i++)
        blob[i] = blob_byte(i);
    HANDLE f = CreateFileW(L"C:\\memload\\blob.bin", GENERIC_WRITE, 0, NULL, CREATE_ALWAYS, 0, NULL);
    CHECK(f != INVALID_HANDLE_VALUE);
    DWORD wrote;
    CHECK(WriteFile(f, blob, BLOB, &wrote, NULL) && wrote == BLOB);
    CloseHandle(f);
    out = CreateFileW(L"C:\\memload\\out.bin", GENERIC_READ | GENERIC_WRITE, FILE_SHARE_READ, NULL, CREATE_ALWAYS, 0,
                      NULL);
    CHECK(out != INVALID_HANDLE_VALUE);
    session = WinHttpOpen(L"gala memload_test", WINHTTP_ACCESS_TYPE_NO_PROXY, WINHTTP_NO_PROXY_NAME,
                          WINHTTP_NO_PROXY_BYPASS, 0);
    CHECK(session != NULL);
    struct job j = {0};
    j.buf = VirtualAlloc(NULL, RANGE, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE);
    CHECK(j.buf != NULL);
    DWORD ranges = mib * (1024 / 256);
    DWORD start = GetTickCount();
    for (DWORD k = 0; k < ranges; k++) {
        for (int attempt = 0;; attempt++) {
            j.index = k;
            j.error = j.status = 0;
            HANDLE h = CreateThread(NULL, 0, fetch, &j, 0, NULL);
            CHECK(h != NULL);
            CHECK(WaitForSingleObject(h, INFINITE) == WAIT_OBJECT_0);
            CloseHandle(h);
            /* Nginx may still be starting. */
            if (k == 0 && j.error == ERROR_WINHTTP_CANNOT_CONNECT && attempt < 100) {
                Sleep(200);
                continue;
            }
            break;
        }
        if (j.error != 0 || j.status != 206)
            printf("memload_test: range %lu: error %lu status %lu\n", (unsigned long)k, (unsigned long)j.error,
                   (unsigned long)j.status);
        CHECK(j.error == 0 && j.status == 206);
        DWORD from = (k % (BLOB / RANGE)) * RANGE;
        CHECK(memcmp(j.buf, blob + from, RANGE) == 0);
        if (k % 64 == 63) {
            sample();
            if (k % 256 == 255) {
                printf("memload_test: %lu MiB in %lu s, available %lu of %lu MiB at least\n",
                       (unsigned long)((k + 1) / 4), (unsigned long)((GetTickCount() - start) / 1000),
                       (unsigned long)(min_avail >> 20), (unsigned long)(total_phys >> 20));
                fflush(stdout);
            }
        }
    }
    WinHttpCloseHandle(session);
    sample();
    printf("memload_test: downloaded %lu MiB in %lu s; available memory at least %lu of %lu MiB\n",
           (unsigned long)mib, (unsigned long)((GetTickCount() - start) / 1000), (unsigned long)(min_avail >> 20),
           (unsigned long)(total_phys >> 20));
    fflush(stdout);
    CHECK(min_avail >= total_phys / 8);

    /* 3. */
    LARGE_INTEGER size;
    CHECK(GetFileSizeEx(out, &size) && size.QuadPart == (LONGLONG)ranges * RANGE);
    HANDLE map = CreateFileMappingW(out, NULL, PAGE_READONLY, 0, 0, NULL);
    CHECK(map != NULL);
    const ULONGLONG view = 16u << 20;
    for (ULONGLONG at = 0; at < (ULONGLONG)size.QuadPart; at += view) {
        const BYTE *p = MapViewOfFile(map, FILE_MAP_READ, (DWORD)(at >> 32), (DWORD)at, (SIZE_T)view);
        CHECK(p != NULL);
        for (ULONGLONG k = 0; k < view; k += RANGE) {
            DWORD from = (DWORD)(((at + k) / RANGE) % (BLOB / RANGE)) * RANGE;
            CHECK(memcmp(p + k, blob + from, RANGE) == 0);
        }
        CHECK(UnmapViewOfFile(p));
        sample();
    }
    CloseHandle(map);
    CloseHandle(out);
    printf("memload_test: read back through views; available memory at least %lu of %lu MiB\n",
           (unsigned long)(min_avail >> 20), (unsigned long)(total_phys >> 20));
    CHECK(min_avail >= total_phys / 8);
    CHECK(DeleteFileW(L"C:\\memload\\out.bin"));
    CHECK(DeleteFileW(L"C:\\memload\\blob.bin"));
    printf("memload_test: ok\n");
    return 0;
}
