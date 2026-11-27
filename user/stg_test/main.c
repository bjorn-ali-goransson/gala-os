/* ole32's compound files (MS-CFB) as real Windows reads them, for what
 * Word opens on start-up (docs/ROADMAP.md AG): Office's
 * Mso20win32client.dll delay-loads StgOpenStorage and WWLIB.DLL calls
 * it, and a delay load that resolves to nothing ends the process with
 * 0xc06d007f.
 *
 * The program builds a compound file byte by byte -- header, FAT,
 * directory, mini FAT, mini stream -- so every path through the format
 * is covered by something whose bytes the test itself chose: two
 * streams on a FAT chain (5000 bytes over ten sectors, 4096 bytes over
 * eight), two on the mini FAT chain inside the root entry's stream (100
 * bytes over two mini sectors, 50 bytes), an empty stream, and a
 * substorage with a stream of its own. Then it reads them back and
 * checks what the file format says against what Windows 11 itself
 * answers (the program is run on Windows too, which is where the
 * answers below come from). What that turned up, beyond the format's
 * own documentation ([MS-CFB] and learn.microsoft.com StgOpenStorage,
 * IStorage, IStream, IEnumSTATSTG):
 *
 * - Where a stream lives is decided by its size alone against the
 *   header's mini-stream cutoff: 4096 bytes and more follow the FAT,
 *   less follows the mini FAT inside the root entry's stream. Reading a
 *   small stream as if its start were a sector is STG_E_DOCFILECORRUPT.
 * - StgOpenStorage works before CoInitialize.
 * - The mode is checked hard: STGM_READ by itself, STGM_READ with
 *   STGM_SHARE_DENY_NONE, STGM_READWRITE with STGM_SHARE_DENY_WRITE,
 *   STGM_CREATE and an unknown high bit are all STG_E_INVALIDFLAG --
 *   only a deny-write or exclusive share is taken -- while
 *   STGM_DELETEONRELEASE is STG_E_INVALIDFUNCTION.
 * - A file that is not a compound file (or is too short for the header)
 *   is STG_E_FILEALREADYEXISTS, not an obvious error for a read. The
 *   root entry's own type is reported by Stat as STGTY_STORAGE (1), not
 *   the 5 the directory entry holds. A reserved parameter that is not 0
 *   leaves the caller's out pointer untouched where every other
 *   parameter error zeroes it.
 * - Names fold case-insensitively but are not trimmed or prefixed: a
 *   trailing space or a prefix of a name is STG_E_FILENOTFOUND. A
 *   second open of a stream or substorage that is already open is
 *   STG_E_ACCESSDENIED, and OpenStream with a deny-write share (which
 *   StgOpenStorage takes) is STG_E_INVALIDFUNCTION.
 * - A read that spans the end of a stream is S_OK with the partial
 *   count, and so is a read at the end (0 bytes); a seek past the end
 *   is allowed, a seek to a negative offset is STG_E_INVALIDFUNCTION.
 * - Enumeration is in the format's order (name length first, then the
 *   folded name), which is not the order of the entries in the
 *   directory; Next past the end answers S_FALSE with what it got, and
 *   Next for more than one with a NULL count is STG_E_INVALIDPARAMETER.
 * - Writes through a storage opened for reading are all
 *   STG_E_ACCESSDENIED (CreateStream, CreateStorage, DestroyElement,
 *   RenameElement, SetClass, SetStateBits, Commit), while Revert is
 *   S_OK.
 *
 * Measured and not checked here, because gala answers these with a
 * recorded gap rather than an object (docs/ARCHITECTURE.md, the
 * compound files section): a read-only root storage on Windows also
 * answers QueryInterface for IPropertySetStorage, IMarshal and
 * IRootStorage with S_OK and a different pointer. IPersistFile,
 * IPersistStorage, IStream and IDispatch are E_NOINTERFACE, which is
 * checked.
 *
 * Also measured on Windows and deliberately not a contract: with a
 * directory tree whose children are not in the format's order, the
 * enumeration returns nothing at all and only the subtree's root is
 * findable by name -- so the lookup is not a plain in-order walk. What
 * a malformed file answers is not worth copying; the test keeps the
 * malformed image only to show it is not read as a well-formed one.
 */
#include <windows.h>
#include <objbase.h>
#include <stdio.h>
#include <string.h>

static int check;
#define CHECK(cond) do { check++; if (!(cond)) { printf("stg_test: check %d (line %d) failed\n", check, __LINE__); fflush(stdout); ExitProcess(check); } } while (0)

#define SEC 512
#define FREESECT 0xffffffffu
#define ENDOFCHAIN 0xfffffffeu
#define FATSECT 0xfffffffdu
#define NOSTREAM 0xffffffffu

#define FIX L"stg_fix.cfb"
#define ODD L"stg_odd.cfb"
#define PLAIN L"stg_plain.txt"
#define CUT L"stg_cut.cfb"
#define GONE L"stg_none.cfb"

/* The CLSID written into the fixture's root entry, which Stat reports. */
static const GUID root_clsid = { 0x11223344, 0x5566, 0x7788,
                                 { 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff, 0x00 } };

static void put16(unsigned char *p, unsigned v)
{
    p[0] = (unsigned char)(v & 0xff);
    p[1] = (unsigned char)((v >> 8) & 0xff);
}

static void put32(unsigned char *p, unsigned v)
{
    p[0] = (unsigned char)(v & 0xff);
    p[1] = (unsigned char)((v >> 8) & 0xff);
    p[2] = (unsigned char)((v >> 16) & 0xff);
    p[3] = (unsigned char)((v >> 24) & 0xff);
}

/* --- the fixture --- */
#define NSEC 24
static unsigned char img[SEC + NSEC * SEC];

static unsigned char *sector(int n)
{
    return img + SEC + n * SEC;
}

/* A directory entry: four to a 512-byte sector, the first at sector 1. */
static void entry(int i, const wchar_t *name, int type, unsigned left,
                  unsigned right, unsigned child, unsigned start,
                  unsigned size)
{
    unsigned char *e = sector(1 + i / 4) + (i % 4) * 128;
    int n = 0;
    while (name[n]) {
        put16(e + n * 2, (unsigned)name[n]);
        n++;
    }
    put16(e + n * 2, 0);
    put16(e + 0x40, (unsigned)((n + 1) * 2)); /* name bytes incl. the NUL */
    e[0x42] = (unsigned char)type;            /* 1 storage, 2 stream, 5 root */
    e[0x43] = 1;                              /* colour */
    put32(e + 0x44, left);
    put32(e + 0x48, right);
    put32(e + 0x4c, child);
    put32(e + 0x74, start);
    put32(e + 0x78, size);
}

/* Sectors: 0 FAT, 1-3 directory (12 entries), 4 mini FAT, 5 mini stream,
 * 6-15 Contents (5000 bytes), 16-23 Four (4096 bytes).
 * Mini stream: Small at mini sector 0 (100 bytes, two mini sectors),
 * Inner at mini sector 2 (50 bytes). */
static void build(void)
{
    unsigned char *h = img;
    int i;

    memset(img, 0, sizeof(img));
    memcpy(h, "\xd0\xcf\x11\xe0\xa1\xb1\x1a\xe1", 8);
    put16(h + 0x18, 0x003e);  /* minor version */
    put16(h + 0x1a, 0x0003);  /* major version: 512-byte sectors */
    put16(h + 0x1c, 0xfffe);  /* little endian */
    put16(h + 0x1e, 9);       /* sector shift */
    put16(h + 0x20, 6);       /* mini sector shift */
    put32(h + 0x2c, 1);       /* FAT sectors */
    put32(h + 0x30, 1);       /* first directory sector */
    put32(h + 0x38, 0x1000);  /* mini stream cutoff */
    put32(h + 0x3c, 4);       /* first mini FAT sector */
    put32(h + 0x40, 1);       /* mini FAT sectors */
    put32(h + 0x44, ENDOFCHAIN); /* first DIFAT sector */
    for (i = 0; i < 109; i++)
        put32(h + 0x4c + i * 4, FREESECT);
    put32(h + 0x4c, 0);       /* DIFAT[0]: the FAT is sector 0 */

    for (i = 0; i < 128; i++)
        put32(sector(0) + i * 4, FREESECT);
    put32(sector(0) + 0 * 4, FATSECT);
    put32(sector(0) + 1 * 4, 2);          /* directory: 1 -> 2 -> 3 */
    put32(sector(0) + 2 * 4, 3);
    put32(sector(0) + 3 * 4, ENDOFCHAIN);
    put32(sector(0) + 4 * 4, ENDOFCHAIN); /* mini FAT */
    put32(sector(0) + 5 * 4, ENDOFCHAIN); /* mini stream */
    for (i = 6; i < 15; i++)
        put32(sector(0) + i * 4, (unsigned)(i + 1)); /* Contents */
    put32(sector(0) + 15 * 4, ENDOFCHAIN);
    for (i = 16; i < 23; i++)
        put32(sector(0) + i * 4, (unsigned)(i + 1)); /* Four */
    put32(sector(0) + 23 * 4, ENDOFCHAIN);

    for (i = 0; i < 128; i++)
        put32(sector(4) + i * 4, FREESECT);
    put32(sector(4) + 0 * 4, 1);          /* Small: mini sectors 0 -> 1 */
    put32(sector(4) + 1 * 4, ENDOFCHAIN);
    put32(sector(4) + 2 * 4, ENDOFCHAIN); /* Inner: mini sector 2 */

    for (i = 0; i < 12; i++)
        entry(i, L"", 0, NOSTREAM, NOSTREAM, NOSTREAM, 0, 0);
    /* The root's children in the format's order: Sub(3) Four(4) Empty(5)
     * Small(5) Contents(8) -- shorter names first, then by the folded
     * name. The tree is Empty { left: Four { left: Sub }, right: Small
     * { right: Contents } }, which is that order in-order and is not the
     * order the entries sit in. */
    entry(0, L"Root Entry", 5, NOSTREAM, NOSTREAM, 6, 5, 192);
    entry(1, L"Contents", 2, NOSTREAM, NOSTREAM, NOSTREAM, 6, 5000);
    entry(2, L"Small", 2, NOSTREAM, 1, NOSTREAM, 0, 100);
    entry(3, L"Sub", 1, NOSTREAM, NOSTREAM, 4, 0, 0);
    entry(4, L"Inner", 2, NOSTREAM, NOSTREAM, NOSTREAM, 2, 50);
    entry(5, L"Four", 2, 3, NOSTREAM, NOSTREAM, 16, 4096);
    entry(6, L"Empty", 2, 5, 2, NOSTREAM, ENDOFCHAIN, 0);
    memcpy(sector(1) + 0x50, &root_clsid, sizeof root_clsid);

    for (i = 0; i < 100; i++)
        sector(5)[i] = (unsigned char)(0x80 + i);
    for (i = 0; i < 50; i++)
        sector(5)[128 + i] = (unsigned char)(0x10 + i);
    for (i = 0; i < 5000; i++)
        sector(6)[i] = (unsigned char)(i & 0xff);
    for (i = 0; i < 4096; i++)
        sector(16)[i] = (unsigned char)((i * 3) & 0xff);
}

/* The same bytes, but the tree under Sub is deliberately out of order:
 * children c(1), bb(2), aaa(3) with child = bb, bb.left = aaa,
 * bb.right = c, so an in-order walk gives aaa, bb, c where the format
 * asks for c, bb, aaa. */
static void build_odd_tree(void)
{
    build();
    entry(3, L"Sub", 1, NOSTREAM, NOSTREAM, 8, 0, 0);
    entry(7, L"aaa", 2, NOSTREAM, NOSTREAM, NOSTREAM, 2, 50);
    entry(8, L"bb", 2, 7, 9, NOSTREAM, 2, 50);
    entry(9, L"c", 2, NOSTREAM, NOSTREAM, NOSTREAM, 2, 50);
}

static void write_file(const wchar_t *path, const void *data, DWORD len)
{
    HANDLE f = CreateFileW(path, GENERIC_WRITE, 0, NULL, CREATE_ALWAYS,
                           FILE_ATTRIBUTE_NORMAL, NULL);
    DWORD done = 0;
    CHECK(f != INVALID_HANDLE_VALUE);
    CHECK(WriteFile(f, data, len, &done, NULL) && done == len);
    CHECK(CloseHandle(f));
}

/* --- what the pieces of a file answer --- */

static IStorage *open_root(DWORD mode)
{
    IStorage *stg = NULL;
    CHECK(StgOpenStorage(FIX, NULL, mode, NULL, 0, &stg) == S_OK);
    CHECK(stg != NULL);
    return stg;
}

static void is_storage_file(void)
{
    CHECK(StgIsStorageFile(FIX) == S_OK);
    CHECK(StgIsStorageFile(PLAIN) == S_FALSE);
    CHECK(StgIsStorageFile(GONE) == STG_E_FILENOTFOUND);
    /* A file holding only the first 256 bytes of a header: the signature
     * is there, so the answer is S_OK even though the file cannot be
     * opened as a storage. */
    CHECK(StgIsStorageFile(CUT) == S_OK);
}

static void open_modes(void)
{
    static const DWORD taken[] = {
        STGM_READ | STGM_SHARE_DENY_WRITE,
        STGM_READ | STGM_SHARE_EXCLUSIVE,
        STGM_READWRITE | STGM_SHARE_EXCLUSIVE,
        STGM_WRITE | STGM_SHARE_EXCLUSIVE,
        STGM_READ | STGM_SHARE_DENY_WRITE | STGM_TRANSACTED,
        STGM_READ | STGM_SHARE_DENY_WRITE | STGM_DIRECT_SWMR,
        STGM_READ | STGM_SHARE_EXCLUSIVE | STGM_TRANSACTED,
    };
    static const DWORD refused[] = {
        STGM_READ,
        STGM_READ | STGM_SHARE_DENY_NONE,
        STGM_READWRITE | STGM_SHARE_DENY_WRITE,
        STGM_READ | STGM_SHARE_DENY_WRITE | STGM_CREATE,
        STGM_READ | STGM_SHARE_DENY_WRITE | 0x80000000u,
    };
    IStorage *stg = NULL;
    size_t i;

    for (i = 0; i < sizeof taken / sizeof taken[0]; i++) {
        stg = NULL;
        CHECK(StgOpenStorage(FIX, NULL, taken[i], NULL, 0, &stg) == S_OK);
        CHECK(stg != NULL);
        CHECK(stg->lpVtbl->Release(stg) == 0);
    }
    for (i = 0; i < sizeof refused / sizeof refused[0]; i++) {
        stg = (IStorage *)(ULONG_PTR)1;
        CHECK(StgOpenStorage(FIX, NULL, refused[i], NULL, 0, &stg)
              == STG_E_INVALIDFLAG);
        CHECK(stg == NULL);
    }
    stg = (IStorage *)(ULONG_PTR)1;
    CHECK(StgOpenStorage(FIX, NULL, STGM_READ | STGM_SHARE_DENY_WRITE
                                        | STGM_DELETEONRELEASE,
                         NULL, 0, &stg) == STG_E_INVALIDFUNCTION);
    CHECK(stg == NULL);
}

static void open_errors(void)
{
    const DWORD read = STGM_READ | STGM_SHARE_DENY_WRITE;
    WCHAR *snb[1] = { NULL };
    WCHAR small[] = L"Small";
    WCHAR *one[2] = { small, NULL };
    IStorage *stg;

    /* A file that is not a compound file, and one too short to hold a
     * header, are both STG_E_FILEALREADYEXISTS. */
    stg = (IStorage *)(ULONG_PTR)1;
    CHECK(StgOpenStorage(PLAIN, NULL, read, NULL, 0, &stg)
          == STG_E_FILEALREADYEXISTS);
    CHECK(stg == NULL);
    stg = (IStorage *)(ULONG_PTR)1;
    CHECK(StgOpenStorage(CUT, NULL, read, NULL, 0, &stg)
          == STG_E_FILEALREADYEXISTS);
    CHECK(stg == NULL);

    stg = (IStorage *)(ULONG_PTR)1;
    CHECK(StgOpenStorage(GONE, NULL, read, NULL, 0, &stg)
          == STG_E_FILENOTFOUND);
    CHECK(stg == NULL);
    /* What the file system says, with the storage facility over it: a
     * directory is an access denied, while a name with a backslash
     * where a file name should be, a missing parent and the empty name
     * are a path that is not there. */
    stg = (IStorage *)(ULONG_PTR)1;
    CHECK(StgOpenStorage(L"C:\\Windows", NULL, read, NULL, 0, &stg)
          == STG_E_ACCESSDENIED);
    CHECK(stg == NULL);
    stg = (IStorage *)(ULONG_PTR)1;
    CHECK(StgOpenStorage(L"C:\\Windows\\", NULL, read, NULL, 0, &stg)
          == STG_E_PATHNOTFOUND);
    CHECK(stg == NULL);
    stg = (IStorage *)(ULONG_PTR)1;
    CHECK(StgOpenStorage(L"C:\\no-such-dir\\stg.cfb", NULL, read, NULL, 0, &stg)
          == STG_E_PATHNOTFOUND);
    CHECK(stg == NULL);
    stg = (IStorage *)(ULONG_PTR)1;
    CHECK(StgOpenStorage(L"", NULL, read, NULL, 0, &stg)
          == STG_E_PATHNOTFOUND);
    CHECK(stg == NULL);
    stg = (IStorage *)(ULONG_PTR)1;
    CHECK(StgOpenStorage(NULL, NULL, read, NULL, 0, &stg)
          == STG_E_INVALIDNAME);
    CHECK(stg == NULL);
    CHECK(StgOpenStorage(FIX, NULL, read, NULL, 0, NULL)
          == STG_E_INVALIDPOINTER);

    /* The reserved parameter is the one error that leaves the caller's
     * pointer as it was. */
    stg = (IStorage *)(ULONG_PTR)1;
    CHECK(StgOpenStorage(FIX, NULL, read, NULL, 1, &stg)
          == STG_E_INVALIDPARAMETER);
    CHECK(stg == (IStorage *)(ULONG_PTR)1);

    /* An exclusion list needs a transacted open: on a direct one it is
     * refused whatever is in it, and with a read-only transacted mode
     * too. (A transacted read-write open with a list is S_OK on
     * Windows, which gala does not do yet: ARCHITECTURE, the compound
     * files section.) */
    stg = (IStorage *)(ULONG_PTR)1;
    CHECK(StgOpenStorage(FIX, NULL, read, (SNB)snb, 0, &stg)
          == STG_E_ACCESSDENIED);
    CHECK(stg == NULL);
    stg = (IStorage *)(ULONG_PTR)1;
    CHECK(StgOpenStorage(FIX, NULL, read, (SNB)one, 0, &stg)
          == STG_E_ACCESSDENIED);
    CHECK(stg == NULL);
    stg = (IStorage *)(ULONG_PTR)1;
    CHECK(StgOpenStorage(FIX, NULL, read | STGM_TRANSACTED, (SNB)snb, 0, &stg)
          == STG_E_ACCESSDENIED);
    CHECK(stg == NULL);
}

static void open_sharing(void)
{
    const DWORD read = STGM_READ | STGM_SHARE_DENY_WRITE;
    IStorage *one = NULL, *two = NULL, *writer = (IStorage *)(ULONG_PTR)1;

    CHECK(StgOpenStorage(FIX, NULL, read, NULL, 0, &one) == S_OK);
    CHECK(StgOpenStorage(FIX, NULL, read, NULL, 0, &two) == S_OK);
    CHECK(one != two);
    CHECK(StgOpenStorage(FIX, NULL, STGM_READWRITE | STGM_SHARE_EXCLUSIVE,
                         NULL, 0, &writer) == STG_E_SHAREVIOLATION);
    CHECK(writer == NULL);
    CHECK(two->lpVtbl->Release(two) == 0);
    CHECK(one->lpVtbl->Release(one) == 0);
}

static void root_stat(void)
{
    const DWORD read = STGM_READ | STGM_SHARE_DENY_WRITE;
    IStorage *stg = open_root(read);
    wchar_t full[MAX_PATH];
    IStorage *dotted = NULL;
    STATSTG st;

    CHECK(GetFullPathNameW(FIX, MAX_PATH, full, NULL) > 0);

    memset(&st, 0xcc, sizeof st);
    CHECK(stg->lpVtbl->Stat(stg, &st, STATFLAG_DEFAULT) == S_OK);
    /* The name is the file's full path, whatever shape the name the
     * storage was opened with had. */
    CHECK(st.pwcsName != NULL && wcscmp(st.pwcsName, full) == 0);
    CoTaskMemFree(st.pwcsName);
    /* The directory entry says 5 (root); Stat says STGTY_STORAGE. */
    CHECK(st.type == STGTY_STORAGE);
    CHECK(st.cbSize.QuadPart == 0);
    CHECK(st.grfMode == read);
    CHECK(st.grfLocksSupported == 0);
    CHECK(st.grfStateBits == 0);
    CHECK(IsEqualGUID(&st.clsid, &root_clsid));
    CHECK(st.reserved == 0);
    /* The times are the file's, not the directory entry's (which this
     * fixture leaves zero). */
    CHECK(st.mtime.dwHighDateTime != 0 && st.ctime.dwHighDateTime != 0);

    memset(&st, 0xcc, sizeof st);
    CHECK(stg->lpVtbl->Stat(stg, &st, STATFLAG_NONAME) == S_OK);
    CHECK(st.pwcsName == NULL);
    CHECK(st.type == STGTY_STORAGE && st.grfMode == read);

    CHECK(stg->lpVtbl->Stat(stg, NULL, STATFLAG_DEFAULT)
          == STG_E_INVALIDPOINTER);
    CHECK(stg->lpVtbl->Release(stg) == 0);

    /* A name with a dot in it is the same file and the same reported
     * name. */
    CHECK(StgOpenStorage(L".\\" FIX, NULL, read, NULL, 0, &dotted) == S_OK);
    memset(&st, 0xcc, sizeof st);
    CHECK(dotted->lpVtbl->Stat(dotted, &st, STATFLAG_DEFAULT) == S_OK);
    CHECK(st.pwcsName != NULL && wcscmp(st.pwcsName, full) == 0);
    CoTaskMemFree(st.pwcsName);
    CHECK(dotted->lpVtbl->Release(dotted) == 0);
}

static void root_interfaces(void)
{
    IStorage *stg = open_root(STGM_READ | STGM_SHARE_DENY_WRITE);
    IUnknown *unk = NULL;
    void *out;

    CHECK(stg->lpVtbl->QueryInterface(stg, &IID_IUnknown, (void **)&unk) == S_OK);
    CHECK((void *)unk == (void *)stg);
    CHECK(unk->lpVtbl->Release(unk) == 1);
    out = NULL;
    CHECK(stg->lpVtbl->QueryInterface(stg, &IID_IStorage, &out) == S_OK);
    CHECK(out == (void *)stg);
    CHECK(stg->lpVtbl->Release(stg) == 1);

    out = (void *)(ULONG_PTR)1;
    CHECK(stg->lpVtbl->QueryInterface(stg, &IID_IPersistFile, &out)
          == E_NOINTERFACE);
    CHECK(out == NULL);
    out = (void *)(ULONG_PTR)1;
    CHECK(stg->lpVtbl->QueryInterface(stg, &IID_IPersistStorage, &out)
          == E_NOINTERFACE);
    CHECK(out == NULL);
    out = (void *)(ULONG_PTR)1;
    CHECK(stg->lpVtbl->QueryInterface(stg, &IID_IStream, &out)
          == E_NOINTERFACE);
    CHECK(out == NULL);
    out = (void *)(ULONG_PTR)1;
    CHECK(stg->lpVtbl->QueryInterface(stg, &IID_IDispatch, &out)
          == E_NOINTERFACE);
    CHECK(out == NULL);
    CHECK(stg->lpVtbl->QueryInterface(stg, &IID_IStorage, NULL)
          == STG_E_INVALIDPOINTER);
    CHECK(stg->lpVtbl->Release(stg) == 0);
}

static void root_writes(void)
{
    IStorage *stg = open_root(STGM_READ | STGM_SHARE_DENY_WRITE);
    const DWORD write = STGM_CREATE | STGM_WRITE | STGM_SHARE_EXCLUSIVE;
    IStream *stream = (IStream *)(ULONG_PTR)1;
    IStorage *sub = (IStorage *)(ULONG_PTR)1;

    CHECK(stg->lpVtbl->CreateStream(stg, L"New", write, 0, 0, &stream)
          == STG_E_ACCESSDENIED);
    CHECK(stream == NULL);
    CHECK(stg->lpVtbl->CreateStorage(stg, L"NewSub", write, 0, 0, &sub)
          == STG_E_ACCESSDENIED);
    CHECK(sub == NULL);
    CHECK(stg->lpVtbl->DestroyElement(stg, L"Small") == STG_E_ACCESSDENIED);
    CHECK(stg->lpVtbl->RenameElement(stg, L"Small", L"Tiny")
          == STG_E_ACCESSDENIED);
    CHECK(stg->lpVtbl->SetClass(stg, &IID_IStorage) == STG_E_ACCESSDENIED);
    CHECK(stg->lpVtbl->SetStateBits(stg, 1, 1) == STG_E_ACCESSDENIED);
    CHECK(stg->lpVtbl->Commit(stg, STGC_DEFAULT) == STG_E_ACCESSDENIED);
    CHECK(stg->lpVtbl->Revert(stg) == S_OK);
    /* Nothing was written: the names are still what the fixture holds. */
    CHECK(stg->lpVtbl->Release(stg) == 0);
    stg = open_root(STGM_READ | STGM_SHARE_DENY_WRITE);
    stream = NULL;
    CHECK(stg->lpVtbl->OpenStream(stg, L"Small", NULL,
                                  STGM_READ | STGM_SHARE_EXCLUSIVE, 0, &stream)
          == S_OK);
    CHECK(stream->lpVtbl->Release(stream) == 0);
    CHECK(stg->lpVtbl->Release(stg) == 0);
}

/* Opens `name` and checks its bytes are the `len` the fixture wrote. */
static void stream_bytes(IStorage *stg, const wchar_t *name, unsigned len,
                         int first, int step)
{
    const DWORD read = STGM_READ | STGM_SHARE_EXCLUSIVE;
    IStream *s = NULL;
    static unsigned char buf[8192];
    ULONG got = 0xdead;
    unsigned i;
    STATSTG st;

    CHECK(stg->lpVtbl->OpenStream(stg, name, NULL, read, 0, &s) == S_OK);
    memset(&st, 0xcc, sizeof st);
    CHECK(s->lpVtbl->Stat(s, &st, STATFLAG_DEFAULT) == S_OK);
    CHECK(st.pwcsName != NULL && wcscmp(st.pwcsName, name) == 0);
    CoTaskMemFree(st.pwcsName);
    CHECK(st.type == STGTY_STREAM);
    CHECK(st.cbSize.QuadPart == len);
    CHECK(st.grfMode == read);
    CHECK(st.grfLocksSupported == 0 && st.grfStateBits == 0);
    /* A stream's times are the directory entry's, which this fixture
     * leaves zero -- not the file's. */
    CHECK(st.mtime.dwLowDateTime == 0 && st.mtime.dwHighDateTime == 0);
    CHECK(st.ctime.dwLowDateTime == 0 && st.atime.dwLowDateTime == 0);

    memset(buf, 0, sizeof buf);
    CHECK(s->lpVtbl->Read(s, buf, len + 16, &got) == S_OK);
    CHECK(got == len);
    for (i = 0; i < len; i++)
        if (buf[i] != (unsigned char)(first + (int)i * step)) {
            printf("stg_test: %ls byte %u is 0x%02x\n", name, i, buf[i]);
            fflush(stdout);
            ExitProcess(100);
        }
    check++;
    /* At the end: S_OK and nothing read. */
    got = 0xdead;
    CHECK(s->lpVtbl->Read(s, buf, 4, &got) == S_OK && got == 0);
    CHECK(s->lpVtbl->Release(s) == 0);
}

static void streams(void)
{
    IStorage *stg = open_root(STGM_READ | STGM_SHARE_DENY_WRITE);
    const DWORD read = STGM_READ | STGM_SHARE_EXCLUSIVE;
    IStream *s = NULL, *clone = NULL;
    IStorage *sub = NULL;
    static unsigned char buf[8192];
    ULARGE_INTEGER pos;
    LARGE_INTEGER move;
    ULONG got;
    void *out;

    /* Both chain kinds and an empty stream. */
    stream_bytes(stg, L"Contents", 5000, 0, 1);
    stream_bytes(stg, L"Four", 4096, 0, 3);
    stream_bytes(stg, L"Small", 100, 0x80, 1);
    stream_bytes(stg, L"Empty", 0, 0, 0);

    /* Names fold, but are neither trimmed nor matched by a prefix. */
    s = NULL;
    CHECK(stg->lpVtbl->OpenStream(stg, L"CONTENTS", NULL, read, 0, &s) == S_OK);
    CHECK(s->lpVtbl->Release(s) == 0);
    s = NULL;
    CHECK(stg->lpVtbl->OpenStream(stg, L"small", NULL, read, 0, &s) == S_OK);
    CHECK(s->lpVtbl->Release(s) == 0);
    s = (IStream *)(ULONG_PTR)1;
    CHECK(stg->lpVtbl->OpenStream(stg, L"Contents ", NULL, read, 0, &s)
          == STG_E_FILENOTFOUND);
    CHECK(s == NULL);
    s = (IStream *)(ULONG_PTR)1;
    CHECK(stg->lpVtbl->OpenStream(stg, L"Cont", NULL, read, 0, &s)
          == STG_E_FILENOTFOUND);
    CHECK(s == NULL);
    s = (IStream *)(ULONG_PTR)1;
    CHECK(stg->lpVtbl->OpenStream(stg, L"Nothing", NULL, read, 0, &s)
          == STG_E_FILENOTFOUND);
    CHECK(s == NULL);
    /* A substorage is not a stream and the other way round. */
    s = (IStream *)(ULONG_PTR)1;
    CHECK(stg->lpVtbl->OpenStream(stg, L"Sub", NULL, read, 0, &s)
          == STG_E_FILENOTFOUND);
    CHECK(s == NULL);
    sub = (IStorage *)(ULONG_PTR)1;
    CHECK(stg->lpVtbl->OpenStorage(stg, L"Small", NULL, read, NULL, 0, &sub)
          == STG_E_FILENOTFOUND);
    CHECK(sub == NULL);

    /* The mode OpenStream takes is narrower than StgOpenStorage's. */
    s = (IStream *)(ULONG_PTR)1;
    CHECK(stg->lpVtbl->OpenStream(stg, L"Small", NULL, STGM_READ, 0, &s)
          == STG_E_INVALIDFLAG);
    CHECK(s == NULL);
    s = (IStream *)(ULONG_PTR)1;
    CHECK(stg->lpVtbl->OpenStream(stg, L"Small", NULL,
                                  STGM_READWRITE | STGM_SHARE_EXCLUSIVE, 0, &s)
          == STG_E_INVALIDFLAG);
    CHECK(s == NULL);
    s = (IStream *)(ULONG_PTR)1;
    CHECK(stg->lpVtbl->OpenStream(stg, L"Small", NULL,
                                  STGM_READ | STGM_SHARE_DENY_WRITE, 0, &s)
          == STG_E_INVALIDFUNCTION);
    CHECK(s == NULL);
    s = (IStream *)(ULONG_PTR)1;
    CHECK(stg->lpVtbl->OpenStream(stg, NULL, NULL, read, 0, &s)
          == STG_E_INVALIDNAME);
    CHECK(s == NULL);
    s = (IStream *)(ULONG_PTR)1;
    CHECK(stg->lpVtbl->OpenStream(stg, L"Small", (void *)buf, read, 0, &s)
          == STG_E_INVALIDPARAMETER);
    CHECK(s == NULL);
    s = (IStream *)(ULONG_PTR)1;
    CHECK(stg->lpVtbl->OpenStream(stg, L"Small", NULL, read, 1, &s)
          == STG_E_INVALIDPARAMETER);
    CHECK(s == NULL);
    CHECK(stg->lpVtbl->OpenStream(stg, L"Small", NULL, read, 0, NULL)
          == STG_E_INVALIDPOINTER);

    /* One open at a time per element. */
    s = NULL;
    CHECK(stg->lpVtbl->OpenStream(stg, L"Contents", NULL, read, 0, &s) == S_OK);
    clone = (IStream *)(ULONG_PTR)1;
    CHECK(stg->lpVtbl->OpenStream(stg, L"contents", NULL, read, 0, &clone)
          == STG_E_ACCESSDENIED);
    CHECK(clone == NULL);

    /* Reads across and past the end, and the pointer check. */
    move.QuadPart = 4900;
    CHECK(s->lpVtbl->Seek(s, move, STREAM_SEEK_SET, &pos) == S_OK);
    CHECK(pos.QuadPart == 4900);
    got = 0xdead;
    CHECK(s->lpVtbl->Read(s, buf, 500, &got) == S_OK && got == 100);
    CHECK(buf[0] == (unsigned char)(4900 & 0xff));
    got = 0xdead;
    CHECK(s->lpVtbl->Read(s, buf, 0, &got) == S_OK && got == 0);
    CHECK(s->lpVtbl->Read(s, NULL, 4, &got) == STG_E_INVALIDPOINTER);

    /* Seeks: past the end is allowed, negative is not. */
    move.QuadPart = 5010;
    CHECK(s->lpVtbl->Seek(s, move, STREAM_SEEK_SET, &pos) == S_OK);
    CHECK(pos.QuadPart == 5010);
    got = 0xdead;
    CHECK(s->lpVtbl->Read(s, buf, 4, &got) == S_OK && got == 0);
    move.QuadPart = -1;
    CHECK(s->lpVtbl->Seek(s, move, STREAM_SEEK_SET, &pos)
          == STG_E_INVALIDFUNCTION);
    move.QuadPart = -10;
    CHECK(s->lpVtbl->Seek(s, move, STREAM_SEEK_END, &pos) == S_OK);
    CHECK(pos.QuadPart == 4990);
    move.QuadPart = 0;
    CHECK(s->lpVtbl->Seek(s, move, STREAM_SEEK_CUR, NULL) == S_OK);

    /* A clone keeps the position and moves on its own. */
    clone = NULL;
    CHECK(s->lpVtbl->Clone(s, &clone) == S_OK && clone != NULL && clone != s);
    move.QuadPart = 0;
    CHECK(clone->lpVtbl->Seek(clone, move, STREAM_SEEK_CUR, &pos) == S_OK);
    CHECK(pos.QuadPart == 4990);
    move.QuadPart = 0;
    CHECK(clone->lpVtbl->Seek(clone, move, STREAM_SEEK_SET, &pos) == S_OK);
    move.QuadPart = 0;
    CHECK(s->lpVtbl->Seek(s, move, STREAM_SEEK_CUR, &pos) == S_OK);
    CHECK(pos.QuadPart == 4990);
    CHECK(clone->lpVtbl->Release(clone) == 0);

    /* A stream of a storage opened for reading takes no writes. */
    pos.QuadPart = 10;
    CHECK(s->lpVtbl->SetSize(s, pos) == STG_E_ACCESSDENIED);
    got = 0xdead;
    CHECK(s->lpVtbl->Write(s, "gala", 4, &got) == STG_E_ACCESSDENIED);
    CHECK(s->lpVtbl->Commit(s, STGC_DEFAULT) == S_OK);
    CHECK(s->lpVtbl->Revert(s) == S_OK);

    out = NULL;
    CHECK(s->lpVtbl->QueryInterface(s, &IID_IStream, &out) == S_OK);
    CHECK(out == (void *)s);
    CHECK(s->lpVtbl->Release(s) == 1);
    out = NULL;
    CHECK(s->lpVtbl->QueryInterface(s, &IID_IUnknown, &out) == S_OK);
    CHECK(out == (void *)s);
    CHECK(s->lpVtbl->Release(s) == 1);
    /* A compound file's stream is not an ISequentialStream (a memory
     * stream is). */
    out = (void *)(ULONG_PTR)1;
    CHECK(s->lpVtbl->QueryInterface(s, &IID_ISequentialStream, &out)
          == E_NOINTERFACE);
    CHECK(out == NULL);
    out = (void *)(ULONG_PTR)1;
    CHECK(s->lpVtbl->QueryInterface(s, &IID_IStorage, &out) == E_NOINTERFACE);
    CHECK(out == NULL);
    CHECK(s->lpVtbl->Release(s) == 0);

    /* The stream is open again once the first one is gone. */
    s = NULL;
    CHECK(stg->lpVtbl->OpenStream(stg, L"Contents", NULL, read, 0, &s) == S_OK);
    CHECK(s->lpVtbl->Release(s) == 0);
    CHECK(stg->lpVtbl->Release(stg) == 0);
}

static void substorage(void)
{
    IStorage *stg = open_root(STGM_READ | STGM_SHARE_DENY_WRITE);
    const DWORD read = STGM_READ | STGM_SHARE_EXCLUSIVE;
    WCHAR *snb[1] = { NULL };
    IStorage *sub = NULL, *again = (IStorage *)(ULONG_PTR)1;
    STATSTG st;

    CHECK(stg->lpVtbl->OpenStorage(stg, L"Sub", NULL, read, NULL, 0, &sub)
          == S_OK);
    memset(&st, 0xcc, sizeof st);
    CHECK(sub->lpVtbl->Stat(sub, &st, STATFLAG_DEFAULT) == S_OK);
    CHECK(st.pwcsName != NULL && wcscmp(st.pwcsName, L"Sub") == 0);
    CoTaskMemFree(st.pwcsName);
    CHECK(st.type == STGTY_STORAGE && st.cbSize.QuadPart == 0);
    CHECK(st.grfMode == read);

    /* Its stream is on the mini FAT chain too. */
    stream_bytes(sub, L"Inner", 50, 0x10, 1);

    CHECK(stg->lpVtbl->OpenStorage(stg, L"SUB", NULL, read, NULL, 0, &again)
          == STG_E_ACCESSDENIED);
    CHECK(again == NULL);
    again = (IStorage *)(ULONG_PTR)1;
    CHECK(stg->lpVtbl->OpenStorage(stg, L"Sub", NULL, read, (SNB)snb, 0, &again)
          == STG_E_INVALIDPARAMETER);
    CHECK(again == NULL);
    again = (IStorage *)(ULONG_PTR)1;
    CHECK(stg->lpVtbl->OpenStorage(stg, L"Sub", NULL, read, NULL, 1, &again)
          == STG_E_INVALIDPARAMETER);
    CHECK(again == NULL);
    CHECK(stg->lpVtbl->OpenStorage(stg, L"Sub", NULL, read, NULL, 0, NULL)
          == STG_E_INVALIDPOINTER);
    CHECK(stg->lpVtbl->OpenStorage(stg, NULL, NULL, read, NULL, 0, &again)
          == STG_E_INVALIDNAME);

    CHECK(sub->lpVtbl->Release(sub) == 0);
    CHECK(stg->lpVtbl->Release(stg) == 0);
}

/* The format's order: the name's length first, then the folded name. */
static const wchar_t *const order[] = { L"Sub", L"Four", L"Empty", L"Small",
                                        L"Contents" };

static void next_is(IEnumSTATSTG *e, int i)
{
    STATSTG st;
    ULONG got = 0xdead;

    memset(&st, 0xcc, sizeof st);
    CHECK(e->lpVtbl->Next(e, 1, &st, &got) == S_OK && got == 1);
    CHECK(st.pwcsName != NULL && wcscmp(st.pwcsName, order[i]) == 0);
    CoTaskMemFree(st.pwcsName);
    CHECK(st.type == (i == 0 ? STGTY_STORAGE : STGTY_STREAM));
}

static void enumeration(void)
{
    IStorage *stg = open_root(STGM_READ | STGM_SHARE_DENY_WRITE);
    IEnumSTATSTG *e = NULL, *clone = NULL;
    STATSTG four[4];
    ULONG got;
    int i;

    CHECK(stg->lpVtbl->EnumElements(stg, 0, NULL, 0, &e) == S_OK && e != NULL);
    for (i = 0; i < 5; i++)
        next_is(e, i);
    got = 0xdead;
    CHECK(e->lpVtbl->Next(e, 1, four, &got) == S_FALSE && got == 0);

    CHECK(e->lpVtbl->Reset(e) == S_OK);
    CHECK(e->lpVtbl->Skip(e, 2) == S_OK);
    next_is(e, 2);
    /* Asking for more than is left gives what is left. */
    got = 0xdead;
    memset(four, 0xcc, sizeof four);
    CHECK(e->lpVtbl->Next(e, 4, four, &got) == S_FALSE && got == 2);
    for (i = 0; i < 2; i++) {
        CHECK(wcscmp(four[i].pwcsName, order[3 + i]) == 0);
        CoTaskMemFree(four[i].pwcsName);
    }
    CHECK(e->lpVtbl->Skip(e, 1) == S_FALSE);
    CHECK(e->lpVtbl->Reset(e) == S_OK);
    CHECK(e->lpVtbl->Skip(e, 99) == S_FALSE);
    got = 0xdead;
    CHECK(e->lpVtbl->Next(e, 1, four, &got) == S_FALSE && got == 0);

    /* The count may be left out for one, but not for more. */
    CHECK(e->lpVtbl->Reset(e) == S_OK);
    memset(four, 0xcc, sizeof four);
    CHECK(e->lpVtbl->Next(e, 1, four, NULL) == S_OK);
    CHECK(wcscmp(four[0].pwcsName, order[0]) == 0);
    CoTaskMemFree(four[0].pwcsName);
    CHECK(e->lpVtbl->Next(e, 2, four, NULL) == STG_E_INVALIDPARAMETER);
    got = 0xdead;
    CHECK(e->lpVtbl->Next(e, 0, four, &got) == S_OK && got == 0);
    CHECK(e->lpVtbl->Next(e, 1, NULL, &got) == STG_E_INVALIDPOINTER);

    /* A clone carries the position and then moves on its own. */
    CHECK(e->lpVtbl->Clone(e, &clone) == S_OK && clone != NULL && clone != e);
    next_is(clone, 1);
    next_is(clone, 2);
    next_is(e, 1);
    CHECK(clone->lpVtbl->Release(clone) == 0);
    CHECK(e->lpVtbl->Release(e) == 0);

    e = (IEnumSTATSTG *)(ULONG_PTR)1;
    CHECK(stg->lpVtbl->EnumElements(stg, 1, NULL, 0, &e)
          == STG_E_INVALIDPARAMETER);
    CHECK(e == NULL);
    e = (IEnumSTATSTG *)(ULONG_PTR)1;
    CHECK(stg->lpVtbl->EnumElements(stg, 0, NULL, 1, &e)
          == STG_E_INVALIDPARAMETER);
    CHECK(e == NULL);
    CHECK(stg->lpVtbl->EnumElements(stg, 0, NULL, 0, NULL)
          == STG_E_INVALIDPOINTER);

    /* A substorage enumerates its own children. */
    {
        IStorage *sub = NULL;
        STATSTG st;
        CHECK(stg->lpVtbl->OpenStorage(stg, L"Sub", NULL,
                                       STGM_READ | STGM_SHARE_EXCLUSIVE,
                                       NULL, 0, &sub) == S_OK);
        CHECK(sub->lpVtbl->EnumElements(sub, 0, NULL, 0, &e) == S_OK);
        got = 0xdead;
        CHECK(e->lpVtbl->Next(e, 1, &st, &got) == S_OK && got == 1);
        CHECK(wcscmp(st.pwcsName, L"Inner") == 0);
        CoTaskMemFree(st.pwcsName);
        got = 0xdead;
        CHECK(e->lpVtbl->Next(e, 1, &st, &got) == S_FALSE && got == 0);
        CHECK(e->lpVtbl->Release(e) == 0);
        CHECK(sub->lpVtbl->Release(sub) == 0);
    }
    CHECK(stg->lpVtbl->Release(stg) == 0);
}

/* The malformed image: its root still reads, and the subtree that is out
 * of order is not walked as if it were in order. What Windows answers
 * for its children is not a contract (see the head of this file); what
 * the test holds to is that the file opens and the well-formed part of
 * it reads. */
static void odd_tree(void)
{
    IStorage *stg = NULL;
    IStorage *sub = NULL;

    CHECK(StgOpenStorage(ODD, NULL, STGM_READ | STGM_SHARE_DENY_WRITE, NULL, 0,
                         &stg) == S_OK);
    stream_bytes(stg, L"Contents", 5000, 0, 1);
    CHECK(stg->lpVtbl->OpenStorage(stg, L"Sub", NULL,
                                   STGM_READ | STGM_SHARE_EXCLUSIVE, NULL, 0,
                                   &sub) == S_OK);
    CHECK(sub->lpVtbl->Release(sub) == 0);
    CHECK(stg->lpVtbl->Release(stg) == 0);
}

int main(void)
{
    build();
    write_file(FIX, img, sizeof img);
    build_odd_tree();
    write_file(ODD, img, sizeof img);
    build();
    write_file(CUT, img, 256);
    write_file(PLAIN, "not a compound file at all, but long enough\n", 43);
    DeleteFileW(GONE);

    /* Before CoInitialize: the file opens and reads all the same. */
    {
        IStorage *stg = open_root(STGM_READ | STGM_SHARE_DENY_WRITE);
        stream_bytes(stg, L"Small", 100, 0x80, 1);
        CHECK(stg->lpVtbl->Release(stg) == 0);
    }

    CHECK(CoInitializeEx(NULL, COINIT_APARTMENTTHREADED) == S_OK);
    is_storage_file();
    open_modes();
    open_errors();
    open_sharing();
    root_stat();
    root_interfaces();
    root_writes();
    streams();
    substorage();
    enumeration();
    odd_tree();
    CoUninitialize();

    DeleteFileW(FIX);
    DeleteFileW(ODD);
    DeleteFileW(CUT);
    DeleteFileW(PLAIN);
    printf("stg_test: ok (%d checks)\n", check);
    return 0;
}
