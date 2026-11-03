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
 * - SymSrvGetFileIndexInfoW: for an image file, its name, TimeDateStamp,
 *   SizeOfImage and whether its debug information is stripped
 *   (IMAGE_FILE_DEBUG_STRIPPED); a CodeView (RSDS) record's PDB name,
 *   GUID and age when there is one (none here); FALSE with
 *   ERROR_FILE_NOT_FOUND for a file that is not there.
 * - MiniDumpWriteDump is exported (not called: gala does not write dumps
 *   yet, and its stub ends the process).
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

    /* SymSrvGetFileIndexInfoW on this program's file. */
    WCHAR self[MAX_PATH];
    CHECK(GetModuleFileNameW(NULL, self, MAX_PATH) > 0);
    static INDEX_INFOW info;
    memset(&info, 0xcc, sizeof info);
    info.sizeofstruct = sizeof info;
    CHECK(index_info(self, &info, 0));
    CHECK(wcscmp(info.file, L"dbghelp_test.exe") == 0);
    CHECK(info.timestamp == nt->FileHeader.TimeDateStamp);
    CHECK(info.size == nt->OptionalHeader.SizeOfImage);
    CHECK(info.stripped == ((nt->FileHeader.Characteristics & IMAGE_FILE_DEBUG_STRIPPED) != 0));
    CHECK(info.pdbfile[0] == 0 && info.dbgfile[0] == 0 && info.age == 0 && info.sig == 0);
    static const GUID zero;
    CHECK(memcmp(&info.guid, &zero, sizeof zero) == 0);
    /* A file that is not there. */
    SetLastError(0);
    CHECK(!index_info(L"C:\\no-such-file.dll", &info, 0) && GetLastError() == ERROR_FILE_NOT_FOUND);
    /* MiniDumpWriteDump is exported: imported here (so the program would
     * not load without it), never called. */
    static volatile int never;
    if (never)
        MiniDumpWriteDump(NULL, 0, NULL, MiniDumpNormal, NULL, NULL, NULL);

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
