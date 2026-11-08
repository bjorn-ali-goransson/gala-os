/* dbghelp and normaliz as real Windows behaves, for what programs import
 * and gala lacked (docs/ROADMAP.md AG): SQL Server LocalDB's
 * SqlDumper.exe (which sqlservr starts to write a crash dump) imports
 * dbghelp's ImageNtHeader, SymSrvGetFileIndexInfoW and MiniDumpWriteDump
 * and did not load; libcurl (in PostgreSQL's bin) imports normaliz.dll's
 * IdnToAscii and IdnToUnicode, a DLL gala did not have.
 *
 * Expected values are from Microsoft's documentation (learn.microsoft.com:
 * ImageNtHeader, SymSrvGetFileIndexInfoW and SYMSRV_INDEX_INFOW, the PE
 * format, IdnToAscii/IdnToUnicode) and RFC 3492's Punycode; no Windows
 * machine was available to run this one, so it still needs a run-all.ps1
 * check there. In short:
 *
 * - ImageNtHeader: the IMAGE_NT_HEADERS of a mapped image ("PE\0\0" at
 *   e_lfanew), NULL for memory that is not one.
 * - SymSrvGetFileIndexInfoW: for an image file with a debug directory
 *   (kernel32.dll), its name, TimeDateStamp, SizeOfImage and whether its
 *   debug information is stripped (IMAGE_FILE_DEBUG_STRIPPED); an image
 *   without one (this program) is ERROR_BAD_EXE_FORMAT, a structure
 *   size other than 1608 ERROR_INVALID_PARAMETER (both measured on
 *   Windows 11 25H2); FALSE with ERROR_FILE_NOT_FOUND for a file that is
 *   not there.
 * - SymInitializeW, SymLoadModuleExW (the base; a module already
 *   loaded is 0 with ERROR_SUCCESS), SymFromAddr (a symbol's name,
 *   address and the displacement into it; an export-table symbol is
 *   SYMFLAG_EXPORT with ModBase 0; SizeOfStruct other than 88 is
 *   ERROR_INVALID_PARAMETER), SymUnloadModule64
 *   (FALSE the second time; SymFromAddr fails after), SymCleanup.
 * - MiniDumpWriteDump is exported (not called: gala does not write dumps
 *   yet, and its stub ends the process).
 * - StackWalk64 over this thread (x64, the current process, with
 *   SymFunctionTableAccess64 and SymGetModuleBase64 as the callbacks):
 *   the first call gives the frame the context describes (AddrPC its
 *   Rip), each later one the caller's, AddrPC its return address; the
 *   stack only grows towards the caller, and the walk ends (FALSE) at
 *   the thread's first frame. Another machine type is FALSE.
 *   SymFunctionTableAccess64 is the RUNTIME_FUNCTION covering an address
 *   (NULL outside images), SymGetModuleBase64 the base of the image
 *   holding it (0 outside images).
 * - normaliz.dll's IdnToAscii/IdnToUnicode are kernel32's ("bücher" is
 *   "xn--bcher-kva").
 *
 * Prints "dbghelp_test: ok" or exits with the number of the failing
 * check. */
#include <windows.h>
#include <dbghelp.h>
#include <stdio.h>
#include <string.h>

static int check;
#define CHECK(cond) do { check++; if (!(cond)) { printf("dbghelp_test: check %d (line %d) failed\n", check, __LINE__); fflush(stdout); ExitProcess(check); } } while (0)

/* SYMSRV_INDEX_INFOW as documented (dbghelp.h). */
typedef struct {
    DWORD sizeofstruct;
    WCHAR file[MAX_PATH + 1];
    BOOL stripped;
    DWORD timestamp;
    DWORD size;
    WCHAR dbgfile[MAX_PATH + 1];
    WCHAR pdbfile[MAX_PATH + 1];
    GUID guid;
    DWORD sig;
    DWORD age;
} INDEX_INFOW;

typedef PIMAGE_NT_HEADERS (WINAPI *image_nt_header_fn)(PVOID);
typedef BOOL (WINAPI *index_info_fn)(PCWSTR, INDEX_INFOW *, DWORD);
typedef int (WINAPI *idn_fn)(DWORD, LPCWSTR, int, LPWSTR, int);

typedef BOOL (WINAPI *walk_fn)(DWORD, HANDLE, HANDLE, STACKFRAME64 *, PVOID,
    PREAD_PROCESS_MEMORY_ROUTINE64, PFUNCTION_TABLE_ACCESS_ROUTINE64,
    PGET_MODULE_BASE_ROUTINE64, PTRANSLATE_ADDRESS_ROUTINE64);
static walk_fn stack_walk;
static PFUNCTION_TABLE_ACCESS_ROUTINE64 table_access;
static PGET_MODULE_BASE_ROUTINE64 module_base;
static DWORD64 walked[64];
static int walked_n, walk_ended;
static DWORD64 first_pc, into_level1, into_walker;
static volatile int sink;

/* Walks this thread's stack from here into walked[]. */
__attribute__((noinline)) static int level2(int x) {
    static CONTEXT ctx;
    into_level1 = (DWORD64)__builtin_return_address(0);
    RtlCaptureContext(&ctx);
    first_pc = ctx.Rip;
    static STACKFRAME64 frame;
    memset(&frame, 0, sizeof frame);
    frame.AddrPC.Offset = ctx.Rip;
    frame.AddrPC.Mode = AddrModeFlat;
    frame.AddrFrame.Offset = ctx.Rbp;
    frame.AddrFrame.Mode = AddrModeFlat;
    frame.AddrStack.Offset = ctx.Rsp;
    frame.AddrStack.Mode = AddrModeFlat;
    DWORD64 last_sp = 0;
    walk_ended = 0;
    while (walked_n < 64) {
        if (!stack_walk(IMAGE_FILE_MACHINE_AMD64, GetCurrentProcess(), GetCurrentThread(), &frame,
                &ctx, NULL, table_access, module_base, NULL)) {
            walk_ended = 1;
            break;
        }
        if (frame.AddrStack.Offset < last_sp)
            return -1;
        last_sp = frame.AddrStack.Offset;
        walked[walked_n++] = frame.AddrPC.Offset;
    }
    return x + walked_n;
}

__attribute__((noinline)) static int level1(int x) {
    into_walker = (DWORD64)__builtin_return_address(0);
    int r = level2(x + 1);
    sink = r;
    return r + 1;
}

int main(void) {
    HMODULE dbghelp = LoadLibraryW(L"dbghelp.dll");
    CHECK(dbghelp != NULL);
    image_nt_header_fn image_nt_header = (image_nt_header_fn)GetProcAddress(dbghelp, "ImageNtHeader");
    index_info_fn index_info = (index_info_fn)GetProcAddress(dbghelp, "SymSrvGetFileIndexInfoW");
    CHECK(image_nt_header != NULL && index_info != NULL);

    /* ImageNtHeader on this program's image, and on plain memory. */
    BYTE *base = (BYTE *)GetModuleHandleW(NULL);
    IMAGE_DOS_HEADER *dos = (IMAGE_DOS_HEADER *)base;
    IMAGE_NT_HEADERS *nt = image_nt_header(base);
    CHECK(nt == (IMAGE_NT_HEADERS *)(base + dos->e_lfanew) && nt->Signature == IMAGE_NT_SIGNATURE);
    static BYTE junk[4096];
    CHECK(image_nt_header(junk) == NULL);
    CHECK(image_nt_header(NULL) == NULL);

    /* SymSrvGetFileIndexInfoW: this program has no debug directory (gcc
       -s), which is ERROR_BAD_EXE_FORMAT; kernel32.dll has one. The
       structure's size must be exactly 1608 (Windows 11 25H2: 1604, 1607,
       1609, 0 and 4096 are ERROR_INVALID_PARAMETER). */
    WCHAR self[MAX_PATH];
    CHECK(GetModuleFileNameW(NULL, self, MAX_PATH) > 0);
    static INDEX_INFOW info;
    CHECK(sizeof info == 1608);
    memset(&info, 0xcc, sizeof info);
    info.sizeofstruct = sizeof info;
    SetLastError(0);
    CHECK(!index_info(self, &info, 0) && GetLastError() == ERROR_BAD_EXE_FORMAT);
    HMODULE k32 = GetModuleHandleW(L"kernel32.dll");
    IMAGE_NT_HEADERS *knt = image_nt_header(k32);
    WCHAR k32_path[MAX_PATH];
    CHECK(knt != NULL && GetModuleFileNameW(k32, k32_path, MAX_PATH) > 0);
    memset(&info, 0xcc, sizeof info);
    info.sizeofstruct = 1607;
    SetLastError(0);
    CHECK(!index_info(k32_path, &info, 0) && GetLastError() == ERROR_INVALID_PARAMETER);
    info.sizeofstruct = sizeof info;
    CHECK(index_info(k32_path, &info, 0));
    CHECK(_wcsicmp(info.file, L"kernel32.dll") == 0);
    CHECK(info.timestamp == knt->FileHeader.TimeDateStamp);
    CHECK(info.size == knt->OptionalHeader.SizeOfImage);
    CHECK(info.stripped == ((knt->FileHeader.Characteristics & IMAGE_FILE_DEBUG_STRIPPED) != 0));
    /* A file that is not there. */
    SetLastError(0);
    CHECK(!index_info(L"C:\\no-such-file.dll", &info, 0) && GetLastError() == ERROR_FILE_NOT_FOUND);
    /* MiniDumpWriteDump is exported: imported here (so the program would
     * not load without it), never called. */
    static volatile int never;
    if (never)
        MiniDumpWriteDump(NULL, 0, NULL, MiniDumpNormal, NULL, NULL, NULL);

    /* The symbol functions over a module's exports: dbghelp's own. */
    typedef BOOL (WINAPI *init_fn)(HANDLE, PCWSTR, BOOL);
    typedef BOOL (WINAPI *cleanup_fn)(HANDLE);
    typedef DWORD64 (WINAPI *load_fn)(HANDLE, HANDLE, PCWSTR, PCWSTR, DWORD64, DWORD, void *, DWORD);
    typedef BOOL (WINAPI *unload_fn)(HANDLE, DWORD64);
    typedef BOOL (WINAPI *from_addr_fn)(HANDLE, DWORD64, DWORD64 *, SYMBOL_INFO *);
    init_fn sym_init = (init_fn)GetProcAddress(dbghelp, "SymInitializeW");
    cleanup_fn sym_cleanup = (cleanup_fn)GetProcAddress(dbghelp, "SymCleanup");
    load_fn sym_load = (load_fn)GetProcAddress(dbghelp, "SymLoadModuleExW");
    unload_fn sym_unload = (unload_fn)GetProcAddress(dbghelp, "SymUnloadModule64");
    from_addr_fn sym_from_addr = (from_addr_fn)GetProcAddress(dbghelp, "SymFromAddr");
    CHECK(sym_init && sym_cleanup && sym_load && sym_unload && sym_from_addr);
    HANDLE proc = GetCurrentProcess();
    CHECK(sym_init(proc, NULL, FALSE));
    WCHAR dll_path[MAX_PATH];
    CHECK(GetModuleFileNameW(dbghelp, dll_path, MAX_PATH) > 0);
    DWORD64 dll_base = (DWORD64)(ULONG_PTR)dbghelp;
    CHECK(sym_load(proc, NULL, dll_path, NULL, dll_base, 0, NULL, 0) == dll_base);
    SetLastError(5);
    CHECK(sym_load(proc, NULL, dll_path, NULL, dll_base, 0, NULL, 0) == 0 && GetLastError() == ERROR_SUCCESS);
    _Static_assert(sizeof(SYMBOL_INFO) == 88, "SYMBOL_INFO is 88 bytes");
    static union { SYMBOL_INFO info; char room[sizeof(SYMBOL_INFO) + 256]; } sym;
    memset(&sym, 0, sizeof sym);
    sym.info.SizeOfStruct = sizeof(SYMBOL_INFO);
    sym.info.MaxNameLen = 256;
    DWORD64 at = (DWORD64)(ULONG_PTR)GetProcAddress(dbghelp, "SymFromAddr");
    DWORD64 disp = 99;
    CHECK(sym_from_addr(proc, at + 1, &disp, &sym.info));
    if (strcmp(sym.info.Name, "SymFromAddr") != 0)
        printf("dbghelp_test: SymFromAddr gave %s\n", sym.info.Name);
    CHECK(strcmp(sym.info.Name, "SymFromAddr") == 0 && disp == 1);
    CHECK(sym.info.Address == at && sym.info.NameLen == 11);
    /* Without a PDB the symbol is the export table's: SYMFLAG_EXPORT and
       no ModBase (Windows 11 25H2, which also gave Size 112: not checked,
       gala does not know a function's size from its exports). */
    if (sym.info.Flags & SYMFLAG_EXPORT)
        CHECK(sym.info.ModBase == 0);
    else
        CHECK(sym.info.ModBase == dll_base);
    CHECK(sym.info.Flags == SYMFLAG_EXPORT);
    /* SizeOfStruct must be sizeof(SYMBOL_INFO), 88: else
       ERROR_INVALID_PARAMETER and the structure untouched. */
    sym.info.SizeOfStruct = sizeof(SYMBOL_INFO) - 1;
    sym.info.NameLen = 77;
    SetLastError(0);
    CHECK(!sym_from_addr(proc, at + 1, &disp, &sym.info) && GetLastError() == ERROR_INVALID_PARAMETER);
    CHECK(sym.info.NameLen == 77);
    sym.info.SizeOfStruct = sizeof(SYMBOL_INFO);
    CHECK(sym_unload(proc, dll_base));
    CHECK(!sym_unload(proc, dll_base));
    CHECK(!sym_from_addr(proc, at + 1, &disp, &sym.info));
    CHECK(sym_cleanup(proc));

    /* StackWalk64 over this thread. */
    stack_walk = (walk_fn)GetProcAddress(dbghelp, "StackWalk64");
    table_access = (PFUNCTION_TABLE_ACCESS_ROUTINE64)GetProcAddress(dbghelp, "SymFunctionTableAccess64");
    module_base = (PGET_MODULE_BASE_ROUTINE64)GetProcAddress(dbghelp, "SymGetModuleBase64");
    CHECK(stack_walk && table_access && module_base);
    CHECK(sym_init(proc, NULL, TRUE));
    DWORD64 exe = (DWORD64)(ULONG_PTR)base;
    CHECK(module_base(proc, (DWORD64)(ULONG_PTR)&level2) == exe);
    BYTE *heap = VirtualAlloc(NULL, 4096, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE);
    CHECK(heap != NULL && module_base(proc, (DWORD64)(ULONG_PTR)heap) == 0);
    RUNTIME_FUNCTION *rf = (RUNTIME_FUNCTION *)table_access(proc, (DWORD64)(ULONG_PTR)&level2 + 4);
    CHECK(rf != NULL && exe + rf->BeginAddress <= (DWORD64)(ULONG_PTR)&level2 + 4
        && (DWORD64)(ULONG_PTR)&level2 + 4 < exe + rf->EndAddress);
    CHECK(table_access(proc, (DWORD64)(ULONG_PTR)heap) == NULL);
    int r = level1(1);
    sink = r;
    CHECK(r != -1);
    CHECK(walk_ended && walked_n >= 3);
    CHECK(walked[0] == first_pc && walked[1] == into_level1 && walked[2] == into_walker);
    /* Another machine type. */
    {
        static STACKFRAME64 frame;
        static CONTEXT ctx;
        memset(&frame, 0, sizeof frame);
        RtlCaptureContext(&ctx);
        CHECK(!stack_walk(IMAGE_FILE_MACHINE_I386, proc, GetCurrentThread(), &frame, &ctx, NULL,
            table_access, module_base, NULL));
    }
    CHECK(sym_cleanup(proc));

    /* normaliz.dll. */
    HMODULE normaliz = LoadLibraryW(L"normaliz.dll");
    CHECK(normaliz != NULL);
    idn_fn to_ascii = (idn_fn)GetProcAddress(normaliz, "IdnToAscii");
    idn_fn to_unicode = (idn_fn)GetProcAddress(normaliz, "IdnToUnicode");
    CHECK(to_ascii != NULL && to_unicode != NULL);
    WCHAR out[64];
    int n = to_ascii(0, L"b\x00fc" L"cher.example", -1, out, 64);
    CHECK(n == 22 && wcscmp(out, L"xn--bcher-kva.example") == 0);
    n = to_unicode(0, L"xn--bcher-kva.example", -1, out, 64);
    CHECK(n == 15 && wcscmp(out, L"b\x00fc" L"cher.example") == 0);

    printf("dbghelp_test: ok\n");
    return 0;
}
