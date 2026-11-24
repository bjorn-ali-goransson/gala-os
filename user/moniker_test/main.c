/* Monikers and the running object table as real Windows behaves, for
 * what Word needs (docs/ROADMAP.md OF): Mso20win32client.dll delay-loads
 * ole32!MkParseDisplayName, and because gala had no such export the
 * delay-load raised an exception no handler took. The crash report's
 * list of the thread's last calls puts the kernel's record of the absent
 * export last of all, after four NtProtectVirtualMemory calls patching
 * an import table and before NtTerminateProcess.
 *
 * Every expected value below was measured on Windows 11 itself (the
 * probe this grew out of is in the pull request), and this program is run
 * there as well as under gala. Where the documentation and the machine
 * disagree the machine wins, and three of these are not what the
 * documentation says:
 *
 * - MkParseDisplayName can fail *and* hand back a moniker: a display
 *   name that is a file followed by an extension it cannot parse
 *   ("<a file>!Item") is MK_E_INVALIDEXTENSION with pchEaten set to the
 *   file part and the file's moniker in the caller's pointer.
 * - IMoniker::ParseDisplayName refusing the same extension does *not*
 *   write pchEaten at all.
 * - Composing a file moniker with an anti moniker is S_OK with a NULL
 *   moniker: they annihilate, and S_OK does not promise an object.
 *
 * In short:
 *
 * - MkParseDisplayName: E_INVALIDARG for a NULL bind context, name,
 *   pchEaten or out pointer and for an empty name; a path to a file that
 *   exists is S_OK with the whole name eaten and a file moniker; a file
 *   that is not there, and a directory, are MK_E_CANTOPENFILE with
 *   nothing eaten; a bare word and a URL are MK_E_SYNTAX; a name
 *   beginning "@" is E_NOINTERFACE; "clsid:<guid>:" is a class moniker.
 *   A drive-shaped name counts as a path only while that drive is
 *   mounted: on a drive the machine has not got it is MK_E_SYNTAX, not
 *   MK_E_CANTOPENFILE, and the error GetFileAttributesW leaves behind
 *   plays no part in the difference.
 * - A file moniker keeps its path exactly as given (an empty one and a
 *   relative one too), compares and hashes without regard to case, and
 *   answers IsSystemMoniker MKSYS_FILEMONIKER, CLSID_FileMoniker,
 *   IsDirty S_FALSE, IsRunning S_FALSE and QueryInterface for both
 *   IPersistStream and IROTData.
 * - An item moniker is "<delimiter><name>", also case-insensitive; an
 *   anti moniker is "\..", hashing to 0x80000001; a file composed with
 *   an item is a generic composite whose display name is the two joined.
 * - Two file monikers make no composite at all. The right-hand path
 *   merges into the left as one file moniker, each of its leading ".."
 *   taking a component of the left and leaving the separator behind it
 *   ("C:\a\b.txt" and "..\up.txt" make "C:\a\up.txt"), a leading
 *   "." before a separator dropped, and everything else appended as
 *   written. Climbing past the drive or the UNC share is MK_E_SYNTAX,
 *   and so is an absolute right-hand path -- by its shape alone, an
 *   unmounted drive included -- or an empty left one. The
 *   merge makes no composite, so it happens even when the caller says
 *   it will not take one.
 * - CreateGenericComposite is the left moniker's own ComposeWith with a
 *   generic composite allowed; a NULL on either side hands back the
 *   other moniker with a reference of its own.
 * - Inverse of a file or an item is an anti moniker; Enum on anything
 *   that is not a composite is S_OK with NULL; CommonPrefixWith itself
 *   is MK_S_US, with a sibling it is the common directory including its
 *   trailing separator; RelativePathTo walks up with "..".
 * - BindToObject on a file that is not there is MK_E_CANTOPENFILE and
 *   BindToStorage on it is E_NOINTERFACE.
 * - The running object table is one object per apartment, handed out by
 *   GetRunningObjectTable and by IBindCtx::GetRunningObjectTable alike.
 *   Register holds a reference on the object and on the moniker; the
 *   same moniker again is MK_S_MONIKERALREADYREGISTERED (a success) with
 *   a different key; IsRunning and GetObject match a separate but equal
 *   moniker; Revoke gives the references back, after which IsRunning is
 *   S_FALSE, GetObject is MK_E_UNAVAILABLE and revoking that key again
 *   is E_INVALIDARG.
 *
 * Three calls are deliberately not checked: CreateFileMoniker(path,
 * NULL), GetRunningObjectTable(1, NULL) and CreateGenericComposite(a, b,
 * NULL). Real ole32 does not refuse a NULL out pointer there, it faults
 * (0xC0000005, measured), and this program runs on Windows too. gala
 * refuses them with E_INVALIDARG or E_POINTER instead, which
 * ARCHITECTURE records as a place gala is deliberately kinder.
 *
 * Prints "moniker_test: ok (<n> checks)" or exits with the number of
 * the failing check. */
#define COBJMACROS
#include <windows.h>
#include <ole2.h>
#include <stdio.h>
#include <wctype.h>

static int check = 0;
#define CHECK(cond) do { check++; if (!(cond)) { printf("moniker_test: check %d (line %d) failed\n", check, __LINE__); fflush(stdout); ExitProcess(check); } } while (0)

/* The standard moniker class ids. mingw-w64's headers do not declare
   them, and GetClassID below measures them again anyway. */
#define OLE_CLSID(d1)     {d1, 0x0000, 0x0000, {0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x46}}
static const GUID clsid_file = OLE_CLSID(0x00000303);
static const GUID clsid_item = OLE_CLSID(0x00000304);
static const GUID clsid_anti = OLE_CLSID(0x00000305);
static const GUID clsid_composite = OLE_CLSID(0x00000309);
static const GUID clsid_class = OLE_CLSID(0x0000031a);

#define MKSYS_NONE 0
#define MKSYS_GENERICCOMPOSITE 1
#define MKSYS_FILEMONIKER 2
#define MKSYS_ANTIMONIKER 3
#define MKSYS_ITEMMONIKER 4
#define MKSYS_CLASSMONIKER 7

#define MK_S_US_ ((HRESULT)0x000401E6L)
#define MK_S_ALREADY ((HRESULT)0x000401E7L)

/* An object with a reference count this program can read, to see what
   the running object table holds. */
typedef struct {
    IUnknownVtbl *vtable;
    LONG refs;
} Counted;

static HRESULT STDMETHODCALLTYPE counted_qi(IUnknown *this, REFIID iid, void **out) {
    if (!out) {
        return E_POINTER;
    }
    if (IsEqualIID(iid, &IID_IUnknown)) {
        Counted *c = (Counted *)this;
        c->refs++;
        *out = this;
        return S_OK;
    }
    *out = NULL;
    return E_NOINTERFACE;
}

static ULONG STDMETHODCALLTYPE counted_add(IUnknown *this) {
    Counted *c = (Counted *)this;
    return (ULONG)++c->refs;
}

static ULONG STDMETHODCALLTYPE counted_rel(IUnknown *this) {
    Counted *c = (Counted *)this;
    return (ULONG)--c->refs;
}

static IUnknownVtbl counted_vtable = {counted_qi, counted_add, counted_rel};

/* Everything shared by the four kinds of moniker. */
static void common(IMoniker *mk, IBindCtx *bc, DWORD want_sys, const GUID *want_clsid) {
    DWORD sys = 0xdead;
    CLSID id;
    DWORD hash = 0xdead;
    ULARGE_INTEGER size;
    void *p = NULL;
    HRESULT hr;

    hr = IMoniker_IsSystemMoniker(mk, &sys);
    CHECK(hr == S_OK && sys == want_sys);

    memset(&id, 0, sizeof id);
    hr = IMoniker_GetClassID(mk, &id);
    CHECK(hr == S_OK && IsEqualGUID(&id, want_clsid));

    hr = IMoniker_Hash(mk, &hash);
    CHECK(hr == S_OK && hash != 0xdead);

    hr = IMoniker_IsDirty(mk);
    CHECK(hr == S_FALSE);

    size.QuadPart = 0;
    hr = IMoniker_GetSizeMax(mk, &size);
    CHECK(hr == S_OK && size.QuadPart > 0);

    hr = IMoniker_IsRunning(mk, bc, NULL, NULL);
    CHECK(hr == S_FALSE);

    hr = IMoniker_QueryInterface(mk, &IID_IPersistStream, &p);
    CHECK(hr == S_OK && p != NULL);
    if (p) {
        IUnknown_Release((IUnknown *)p);
        p = NULL;
    }
    hr = IMoniker_QueryInterface(mk, &IID_IROTData, &p);
    CHECK(hr == S_OK && p != NULL);
    if (p) {
        IUnknown_Release((IUnknown *)p);
    }
}

/* The display name a moniker gives, compared with what it should be. */
static int named(IMoniker *mk, IBindCtx *bc, const wchar_t *want) {
    LPOLESTR got = NULL;
    HRESULT hr = IMoniker_GetDisplayName(mk, bc, NULL, &got);
    int same;

    if (hr != S_OK || !got) {
        return 0;
    }
    same = wcscmp(got, want) == 0;
    CoTaskMemFree(got);
    return same;
}

/* Two file monikers composed: the right-hand path merged into the left.
   `want` is the display name the one file moniker should have, or NULL
   for MK_E_SYNTAX and no moniker at all. */
static void merged(IBindCtx *bc, const wchar_t *left, const wchar_t *right, const wchar_t *want) {
    IMoniker *a = NULL, *b = NULL, *out = NULL;
    DWORD sys = 0xdead;
    HRESULT hr;

    CHECK(CreateFileMoniker(left, &a) == S_OK && a != NULL);
    CHECK(CreateFileMoniker(right, &b) == S_OK && b != NULL);
    hr = IMoniker_ComposeWith(a, b, FALSE, &out);
    if (!want) {
        CHECK(hr == MK_E_SYNTAX);
        CHECK(out == NULL);
    } else {
        CHECK(hr == S_OK && out != NULL);
        CHECK(IMoniker_IsSystemMoniker(out, &sys) == S_OK && sys == MKSYS_FILEMONIKER);
        CHECK(named(out, bc, want));
    }
    if (out) {
        IMoniker_Release(out);
    }
    IMoniker_Release(b);
    IMoniker_Release(a);
}

int main(void) {
    HRESULT hr, init;
    IBindCtx *bc = NULL;
    IMoniker *self_mk = NULL, *item = NULL, *anti = NULL, *comp = NULL;
    wchar_t self[MAX_PATH];
    wchar_t absent_path[MAX_PATH + 16];
    wchar_t deep_absent[MAX_PATH + 32];
    wchar_t gone_back[8] = L"_:\\x";
    wchar_t gone_fwd[8] = L"_:/x";
    wchar_t gone_rel[8] = L"_:x";
    wchar_t gone_bare[4] = L"_:";
    wchar_t gone_two[8] = L"__:\\x";
    wchar_t gone_device[16] = L"\\\\?\\_:\\x";
    wchar_t gone_appended[MAX_PATH + 16];
    wchar_t gone_letter = 0;
    DWORD drives;
    wchar_t upper[MAX_PATH];
    wchar_t with_item[MAX_PATH + 8];
    wchar_t directory[MAX_PATH];
    ULONG eaten;
    DWORD len, i;

    init = CoInitializeEx(NULL, COINIT_APARTMENTTHREADED);
    CHECK(init == S_OK);

    hr = CreateBindCtx(0, &bc);
    CHECK(hr == S_OK && bc != NULL);

    /* This program's own file is the one that certainly exists, here and
       under gala; its directory is the one that certainly is not a file. */
    len = GetModuleFileNameW(NULL, self, MAX_PATH);
    CHECK(len > 0 && len < MAX_PATH);
    wcscpy(directory, self);
    for (i = len; i > 0; i--) {
        if (directory[i - 1] == L'\\') {
            directory[i - 1] = 0;
            break;
        }
    }
    CHECK(directory[0] != 0 && wcslen(directory) < len);
    wcscpy(absent_path, directory);
    wcscat(absent_path, L"\\nothing-is-here.docx");
    CHECK(GetFileAttributesW(absent_path) == INVALID_FILE_ATTRIBUTES);
    wcscpy(deep_absent, directory);
    wcscat(deep_absent, L"\\nothing-is-here\\x.docx");

    /* A drive letter this machine has not got: the last one not in
       GetLogicalDrives, so what follows holds on any machine and under
       gala, which has only C:. */
    drives = GetLogicalDrives();
    for (i = 26; i > 0; i--) {
        if (!(drives & (1u << (i - 1)))) {
            gone_letter = (wchar_t)(L'A' + (i - 1));
            break;
        }
    }
    CHECK(gone_letter != 0);
    gone_back[0] = gone_letter;
    gone_fwd[0] = gone_letter;
    gone_rel[0] = gone_letter;
    gone_bare[0] = gone_letter;
    gone_two[0] = gone_letter;
    gone_two[1] = gone_letter;
    gone_device[4] = gone_letter;
    wcscpy(gone_appended, L"C:\\a\\b.txt\\");
    wcscat(gone_appended, gone_two);
    wcscpy(upper, self);
    for (i = 0; upper[i]; i++) {
        upper[i] = (wchar_t)towupper(upper[i]);
    }
    wcscpy(with_item, self);
    wcscat(with_item, L"!Item");

    /* MkParseDisplayName's refusals: all E_INVALIDARG, and a refused
       call writes neither the count nor the moniker. */
    {
        IMoniker *out = (IMoniker *)(ULONG_PTR)1;
        eaten = 0xffffffff;
        hr = MkParseDisplayName(NULL, self, &eaten, &out);
        CHECK(hr == E_INVALIDARG);
        eaten = 0xffffffff;
        out = (IMoniker *)(ULONG_PTR)1;
        hr = MkParseDisplayName(bc, NULL, &eaten, &out);
        CHECK(hr == E_INVALIDARG);
        out = (IMoniker *)(ULONG_PTR)1;
        hr = MkParseDisplayName(bc, self, NULL, &out);
        CHECK(hr == E_INVALIDARG);
        eaten = 0xffffffff;
        hr = MkParseDisplayName(bc, self, &eaten, NULL);
        CHECK(hr == E_INVALIDARG);
        eaten = 0xffffffff;
        out = NULL;
        hr = MkParseDisplayName(bc, L"", &eaten, &out);
        CHECK(hr == E_INVALIDARG && out == NULL);
    }

    /* A file that is there. */
    eaten = 0;
    hr = MkParseDisplayName(bc, self, &eaten, &self_mk);
    CHECK(hr == S_OK && self_mk != NULL && eaten == (ULONG)wcslen(self));
    common(self_mk, bc, MKSYS_FILEMONIKER, &clsid_file);
    CHECK(named(self_mk, bc, self));

    /* A file that is not, and a directory: both MK_E_CANTOPENFILE with
       nothing eaten and no moniker. */
    {
        IMoniker *out = (IMoniker *)(ULONG_PTR)1;
        eaten = 0xffffffff;
        out = NULL;
        hr = MkParseDisplayName(bc, absent_path, &eaten, &out);
        CHECK(hr == MK_E_CANTOPENFILE && out == NULL && eaten == 0);
        eaten = 0xffffffff;
        out = NULL;
        hr = MkParseDisplayName(bc, directory, &eaten, &out);
        CHECK(hr == MK_E_CANTOPENFILE && out == NULL && eaten == 0);

        /* A bare word and a URL are a syntax error; a name beginning "@"
           is E_NOINTERFACE, which is what a ProgID that names no class
           moniker gives. */
        eaten = 0xffffffff;
        out = NULL;
        hr = MkParseDisplayName(bc, L"zzz", &eaten, &out);
        CHECK(hr == MK_E_SYNTAX && out == NULL && eaten == 0);
        eaten = 0xffffffff;
        out = NULL;
        hr = MkParseDisplayName(bc, L"http://example.com/x", &eaten, &out);
        CHECK(hr == MK_E_SYNTAX && out == NULL && eaten == 0);
        eaten = 0xffffffff;
        out = NULL;
        hr = MkParseDisplayName(bc, L"@Word.Document.12", &eaten, &out);
        CHECK(hr == E_NOINTERFACE && out == NULL);

        /* A drive-shaped name is a path only while the drive is mounted.
           On a drive that is not there every shape of it -- either
           separator, drive-relative, and the drive on its own -- is a
           syntax error, and so is a two-letter prefix, which is no
           drive. Two leading separators name no drive and are a path
           whatever follows, so the same absent drive behind "\\?\" is
           MK_E_CANTOPENFILE instead. */
        eaten = 0xffffffff;
        out = NULL;
        hr = MkParseDisplayName(bc, gone_back, &eaten, &out);
        CHECK(hr == MK_E_SYNTAX && out == NULL && eaten == 0);
        eaten = 0xffffffff;
        out = NULL;
        hr = MkParseDisplayName(bc, gone_fwd, &eaten, &out);
        CHECK(hr == MK_E_SYNTAX && out == NULL && eaten == 0);
        eaten = 0xffffffff;
        out = NULL;
        hr = MkParseDisplayName(bc, gone_rel, &eaten, &out);
        CHECK(hr == MK_E_SYNTAX && out == NULL && eaten == 0);
        eaten = 0xffffffff;
        out = NULL;
        hr = MkParseDisplayName(bc, gone_bare, &eaten, &out);
        CHECK(hr == MK_E_SYNTAX && out == NULL && eaten == 0);
        eaten = 0xffffffff;
        out = NULL;
        hr = MkParseDisplayName(bc, gone_two, &eaten, &out);
        CHECK(hr == MK_E_SYNTAX && out == NULL && eaten == 0);
        eaten = 0xffffffff;
        out = NULL;
        hr = MkParseDisplayName(bc, gone_device, &eaten, &out);
        CHECK(hr == MK_E_CANTOPENFILE && out == NULL && eaten == 0);

        /* And a directory that is not there, on a drive that is: the
           same ERROR_PATH_NOT_FOUND as the absent drive above, and the
           other answer -- which is how the drive, not the error code, is
           shown to be what decides. */
        eaten = 0xffffffff;
        out = NULL;
        hr = MkParseDisplayName(bc, deep_absent, &eaten, &out);
        CHECK(hr == MK_E_CANTOPENFILE && out == NULL && eaten == 0);

        /* A class moniker, which is the one name that is not a file. */
        eaten = 0;
        out = NULL;
        hr = MkParseDisplayName(bc, L"clsid:00000303-0000-0000-C000-000000000046:", &eaten, &out);
        CHECK(hr == S_OK && out != NULL && eaten == 43);
        if (out) {
            DWORD sys = 0;
            CLSID id;
            CHECK(IMoniker_IsSystemMoniker(out, &sys) == S_OK && sys == MKSYS_CLASSMONIKER);
            memset(&id, 0, sizeof id);
            CHECK(IMoniker_GetClassID(out, &id) == S_OK
                  && IsEqualGUID(&id, &clsid_class));
            CHECK(named(out, bc, L"clsid:00000303-0000-0000-C000-000000000046:"));
            /* A class moniker does not answer IsRunning at all. */
            CHECK(IMoniker_IsRunning(out, bc, NULL, NULL) == E_NOTIMPL);
            IMoniker_Release(out);
            out = NULL;
        }

        /* A file followed by an extension it cannot parse: a failure
           that still hands back the moniker for the part it ate. */
        eaten = 0;
        out = NULL;
        hr = MkParseDisplayName(bc, with_item, &eaten, &out);
        CHECK(hr == MK_E_INVALIDEXTENSION);
        CHECK(eaten == (ULONG)wcslen(self));
        CHECK(out != NULL);
        if (out) {
            CHECK(named(out, bc, self));
            CHECK(IMoniker_IsEqual(out, self_mk) == S_OK);
            IMoniker_Release(out);
        }
    }

    /* The maker, and what it keeps. */
    {
        IMoniker *made = NULL, *other = NULL;
        DWORD h1 = 0, h2 = 0;

        hr = CreateFileMoniker(self, &made);
        CHECK(hr == S_OK && made != NULL);
        CHECK(IMoniker_IsEqual(made, self_mk) == S_OK);
        CHECK(named(made, bc, self));
        IMoniker_Release(made);
        made = NULL;

        /* An empty path and a relative one are both kept as given. */
        hr = CreateFileMoniker(L"", &made);
        CHECK(hr == S_OK && made != NULL);
        CHECK(named(made, bc, L""));
        common(made, bc, MKSYS_FILEMONIKER, &clsid_file);
        IMoniker_Release(made);
        made = NULL;

        hr = CreateFileMoniker(L"relative\\path.txt", &made);
        CHECK(hr == S_OK && made != NULL);
        CHECK(named(made, bc, L"relative\\path.txt"));
        CHECK(IMoniker_IsEqual(made, self_mk) == S_FALSE);
        IMoniker_Release(made);
        made = NULL;

        hr = CreateFileMoniker(NULL, &made);
        CHECK(hr == MK_E_SYNTAX);

        /* Case does not matter, to IsEqual or to Hash. */
        hr = CreateFileMoniker(upper, &other);
        CHECK(hr == S_OK && other != NULL);
        CHECK(IMoniker_IsEqual(other, self_mk) == S_OK);
        CHECK(IMoniker_Hash(other, &h1) == S_OK);
        CHECK(IMoniker_Hash(self_mk, &h2) == S_OK);
        CHECK(h1 == h2);

        /* The common prefix of a moniker and itself is itself, said with
           MK_S_US; of two files in one directory, the directory with its
           separator still on. */
        {
            IMoniker *prefix = NULL;
            wchar_t sibling[MAX_PATH + 16];
            wchar_t want[MAX_PATH];

            hr = IMoniker_CommonPrefixWith(other, self_mk, &prefix);
            CHECK(hr == MK_S_US_ && prefix != NULL);
            if (prefix) {
                CHECK(IMoniker_IsEqual(prefix, self_mk) == S_OK);
                IMoniker_Release(prefix);
                prefix = NULL;
            }

            wcscpy(sibling, directory);
            wcscat(sibling, L"\\another-name.txt");
            hr = CreateFileMoniker(sibling, &made);
            CHECK(hr == S_OK && made != NULL);
            hr = IMoniker_CommonPrefixWith(self_mk, made, &prefix);
            CHECK(hr == S_OK && prefix != NULL);
            wcscpy(want, directory);
            wcscat(want, L"\\");
            if (prefix) {
                CHECK(named(prefix, bc, want));
                IMoniker_Release(prefix);
                prefix = NULL;
            }

            /* And the path from one to the other, which steps out of
               the source's own last component before descending: one
               ".." per component of the source past the prefix. */
            {
                IMoniker *rel = NULL;
                hr = IMoniker_RelativePathTo(self_mk, made, &rel);
                CHECK(hr == S_OK && rel != NULL);
                if (rel) {
                    CHECK(named(rel, bc, L"..\\another-name.txt"));
                    IMoniker_Release(rel);
                }
            }
            IMoniker_Release(made);
            made = NULL;
        }

        /* Inverse is an anti moniker, and Enum on anything that is not a
           composite is S_OK with nothing. */
        {
            IMoniker *inv = NULL;
            IEnumMoniker *e = (IEnumMoniker *)(ULONG_PTR)1;
            hr = IMoniker_Inverse(self_mk, &inv);
            CHECK(hr == S_OK && inv != NULL);
            if (inv) {
                common(inv, bc, MKSYS_ANTIMONIKER, &clsid_anti);
                CHECK(named(inv, bc, L"\\.."));
                IMoniker_Release(inv);
            }
            e = NULL;
            hr = IMoniker_Enum(self_mk, TRUE, &e);
            CHECK(hr == S_OK && e == NULL);
        }

        /* Binding a file that is not there cannot start anything. */
        {
            IUnknown *obj = (IUnknown *)(ULONG_PTR)1;
            hr = CreateFileMoniker(absent_path, &made);
            CHECK(hr == S_OK && made != NULL);
            obj = NULL;
            hr = IMoniker_BindToObject(made, bc, NULL, &IID_IUnknown, (void **)&obj);
            CHECK(hr == MK_E_CANTOPENFILE && obj == NULL);
            obj = NULL;
            hr = IMoniker_BindToStorage(made, bc, NULL, &IID_IUnknown, (void **)&obj);
            CHECK(hr == E_NOINTERFACE && obj == NULL);
            IMoniker_Release(made);
            made = NULL;
        }

        /* IMoniker::ParseDisplayName refuses the extension without
           writing the count, where MkParseDisplayName writes it. */
        {
            IMoniker *out = NULL;
            eaten = 0xffffffff;
            hr = IMoniker_ParseDisplayName(self_mk, bc, NULL, L"!Item", &eaten, &out);
            CHECK(hr == MK_E_INVALIDEXTENSION && out == NULL && eaten == 0xffffffff);
        }

        IMoniker_Release(other);
    }

    /* Item monikers, anti monikers and what composing them gives. */
    hr = CreateItemMoniker(L"!", L"Item", &item);
    CHECK(hr == S_OK && item != NULL);
    common(item, bc, MKSYS_ITEMMONIKER, &clsid_item);
    CHECK(named(item, bc, L"!Item"));
    {
        IMoniker *item2 = NULL;
        DWORD h1 = 0, h2 = 0;
        hr = CreateItemMoniker(L"!", L"ITEM", &item2);
        CHECK(hr == S_OK && item2 != NULL);
        CHECK(IMoniker_IsEqual(item2, item) == S_OK);
        CHECK(IMoniker_Hash(item2, &h1) == S_OK);
        CHECK(IMoniker_Hash(item, &h2) == S_OK);
        CHECK(h1 == h2);
        IMoniker_Release(item2);
    }
    {
        IMoniker *inv = NULL;
        hr = IMoniker_Inverse(item, &inv);
        CHECK(hr == S_OK && inv != NULL);
        if (inv) {
            CHECK(named(inv, bc, L"\\.."));
            IMoniker_Release(inv);
        }
    }

    hr = CreateAntiMoniker(&anti);
    CHECK(hr == S_OK && anti != NULL);
    common(anti, bc, MKSYS_ANTIMONIKER, &clsid_anti);
    CHECK(named(anti, bc, L"\\.."));
    {
        DWORD hash = 0;
        CHECK(IMoniker_Hash(anti, &hash) == S_OK && hash == 0x80000001);
    }

    /* A file and an item make a generic composite whose name is the two
       joined; a file and an anti annihilate, which is S_OK and NULL. */
    hr = IMoniker_ComposeWith(self_mk, item, FALSE, &comp);
    CHECK(hr == S_OK && comp != NULL);
    if (comp) {
        wchar_t want[MAX_PATH + 8];
        common(comp, bc, MKSYS_GENERICCOMPOSITE, &clsid_composite);
        wcscpy(want, self);
        wcscat(want, L"!Item");
        CHECK(named(comp, bc, want));
    }
    {
        IMoniker *left = (IMoniker *)(ULONG_PTR)1;
        left = NULL;
        hr = IMoniker_ComposeWith(self_mk, anti, FALSE, &left);
        CHECK(hr == S_OK && left == NULL);
    }

    /* Two file monikers are the exception: no composite at all, the
       right-hand path merged into the left as one file moniker. */
    merged(bc, L"C:\\a\\b.txt", L"c.txt", L"C:\\a\\b.txt\\c.txt");
    merged(bc, L"C:\\a\\b.txt", L"d\\e.txt", L"C:\\a\\b.txt\\d\\e.txt");
    merged(bc, L"c.txt", L"d\\e.txt", L"c.txt\\d\\e.txt");
    merged(bc, L"C:\\a\\", L"x.txt", L"C:\\a\\x.txt");
    merged(bc, L"C:\\a", L"b\\c.txt", L"C:\\a\\b\\c.txt");
    merged(bc, L"C:\\a\\b.txt", L"x\\", L"C:\\a\\b.txt\\x\\");
    merged(bc, L"C:\\a\\b.txt", L"", L"C:\\a\\b.txt");
    merged(bc, L"", L"", L"");
    merged(bc, L"", L"c.txt", NULL);

    /* A leading ".." takes one component of the left and leaves the
       separator behind it; the walk works with either separator. */
    merged(bc, L"C:\\a\\b.txt", L"..\\up.txt", L"C:\\a\\up.txt");
    merged(bc, L"C:\\a\\b\\c.txt", L"..\\..\\up.txt", L"C:\\a\\up.txt");
    merged(bc, L"C:\\a\\b.txt", L"..\\..\\up.txt", L"C:\\up.txt");
    merged(bc, L"C:\\a\\b.txt", L"..", L"C:\\a\\");
    merged(bc, L"C:\\a\\b.txt", L"..\\", L"C:\\a\\");
    merged(bc, L"C:\\a\\b.txt", L"..\\..", L"C:\\");
    merged(bc, L"C:\\a\\", L"..\\x.txt", L"C:\\x.txt");
    merged(bc, L"C:\\a\\b\\", L"..\\..\\x.txt", L"C:\\x.txt");
    merged(bc, L"C:\\a", L"..\\x.txt", L"C:\\x.txt");
    merged(bc, L"c.txt", L"..\\x.txt", L"x.txt");
    merged(bc, L"d\\c.txt", L"..\\x.txt", L"d\\x.txt");
    merged(bc, L"C:\\a\\b.txt", L"../x.txt", L"C:\\a\\x.txt");
    merged(bc, L"\\\\server\\share\\x\\y", L"..\\z", L"\\\\server\\share\\x\\z");

    /* The drive and the UNC share are the floor the walk stops at. */
    merged(bc, L"C:\\a\\b.txt", L"..\\..\\..\\up.txt", NULL);
    merged(bc, L"C:\\a\\b.txt", L"..\\..\\..", NULL);
    merged(bc, L"C:\\a\\b.txt", L"..\\..\\..\\..\\up.txt", NULL);
    merged(bc, L"C:\\", L"..\\x.txt", NULL);
    merged(bc, L"\\\\server\\share\\x", L"..\\..\\z", NULL);
    merged(bc, L"\\\\server\\share\\x", L"..\\..\\..\\z", NULL);

    /* A leading "." before a separator is dropped. Everything else is
       appended as written: an inner ".." or ".", a name that merely
       begins with dots, and a lone "." at the very end of the name. */
    merged(bc, L"C:\\a\\b.txt", L".\\x.txt", L"C:\\a\\b.txt\\x.txt");
    merged(bc, L"C:\\a\\b.txt", L".\\.\\x.txt", L"C:\\a\\b.txt\\x.txt");
    merged(bc, L"C:\\a\\b.txt", L".\\..\\x.txt", L"C:\\a\\x.txt");
    merged(bc, L"C:\\a\\b.txt", L".", L"C:\\a\\b.txt\\.");
    merged(bc, L"C:\\a\\b.txt", L"..x.txt", L"C:\\a\\b.txt\\..x.txt");
    merged(bc, L"C:\\a\\b.txt", L"...\\x.txt", L"C:\\a\\b.txt\\...\\x.txt");
    merged(bc, L"C:\\a\\b.txt", L"d\\..\\x.txt", L"C:\\a\\b.txt\\d\\..\\x.txt");
    merged(bc, L"C:\\a\\b.txt", L"d\\.\\x.txt", L"C:\\a\\b.txt\\d\\.\\x.txt");

    /* An absolute right-hand path is refused whatever the left is, and
       absolute here is the name's shape alone: a drive this machine has
       not got is still absolute, where MkParseDisplayName calls such a
       name no path at all. A drive-relative "<d>:x" is absolute too,
       while two letters and a colon are no drive and are appended. */
    merged(bc, L"C:\\a\\b.txt", gone_back, NULL);
    merged(bc, L"C:\\a\\b.txt", gone_fwd, NULL);
    merged(bc, L"C:\\a\\b.txt", gone_rel, NULL);
    merged(bc, L"C:\\a\\b.txt", gone_two, gone_appended);
    merged(bc, L"C:\\a\\b.txt", L"C:\\c\\d.txt", NULL);
    merged(bc, L"C:\\a\\b.txt", L"\\\\server\\share\\x", NULL);
    merged(bc, L"c.txt", L"C:\\a\\b.txt", NULL);

    /* The merge is no composite, so it happens even when the caller says
       it will not take one -- where an item moniker on the right is
       MK_E_NEEDGENERIC and an anti moniker still annihilates. */
    {
        IMoniker *a = NULL, *b = NULL, *out = NULL;

        CHECK(CreateFileMoniker(L"C:\\a\\b.txt", &a) == S_OK && a != NULL);
        CHECK(CreateFileMoniker(L"c.txt", &b) == S_OK && b != NULL);
        out = NULL;
        CHECK(IMoniker_ComposeWith(a, b, TRUE, &out) == S_OK && out != NULL);
        if (out) {
            CHECK(named(out, bc, L"C:\\a\\b.txt\\c.txt"));
            IMoniker_Release(out);
        }
        out = NULL;
        CHECK(IMoniker_ComposeWith(a, item, TRUE, &out) == MK_E_NEEDGENERIC);
        CHECK(out == NULL);
        out = NULL;
        CHECK(IMoniker_ComposeWith(a, anti, TRUE, &out) == S_OK && out == NULL);
        IMoniker_Release(b);
        IMoniker_Release(a);
    }

    /* CreateGenericComposite: the left moniker's ComposeWith with a
       composite allowed, and a NULL on either side handing back the
       other moniker with a reference of its own. */
    {
        IMoniker *out = NULL;
        wchar_t want[MAX_PATH + 8];

        out = NULL;
        CHECK(CreateGenericComposite(NULL, NULL, &out) == S_OK);
        CHECK(out == NULL);

        out = NULL;
        CHECK(CreateGenericComposite(self_mk, NULL, &out) == S_OK);
        CHECK(out == self_mk);
        if (out) {
            IMoniker_Release(out);
        }

        out = NULL;
        CHECK(CreateGenericComposite(NULL, item, &out) == S_OK);
        CHECK(out == item);
        if (out) {
            IMoniker_Release(out);
        }

        out = NULL;
        CHECK(CreateGenericComposite(self_mk, item, &out) == S_OK && out != NULL);
        if (out) {
            common(out, bc, MKSYS_GENERICCOMPOSITE, &clsid_composite);
            wcscpy(want, self);
            wcscat(want, L"!Item");
            CHECK(named(out, bc, want));
            IMoniker_Release(out);
        }

        out = NULL;
        CHECK(CreateGenericComposite(self_mk, anti, &out) == S_OK);
        CHECK(out == NULL);

        /* And the file-to-file rule again, through the export. */
        {
            IMoniker *a = NULL, *b = NULL;

            CHECK(CreateFileMoniker(L"C:\\a\\b.txt", &a) == S_OK && a != NULL);
            CHECK(CreateFileMoniker(L"..\\up.txt", &b) == S_OK && b != NULL);
            out = NULL;
            CHECK(CreateGenericComposite(a, b, &out) == S_OK && out != NULL);
            if (out) {
                CHECK(named(out, bc, L"C:\\a\\up.txt"));
                IMoniker_Release(out);
            }
            IMoniker_Release(b);
            CHECK(CreateFileMoniker(L"C:\\c\\d.txt", &b) == S_OK && b != NULL);
            out = NULL;
            CHECK(CreateGenericComposite(a, b, &out) == MK_E_SYNTAX);
            CHECK(out == NULL);
            IMoniker_Release(b);
            IMoniker_Release(a);
        }
    }

    /* The running object table. */
    {
        IRunningObjectTable *rot = NULL, *again = NULL, *from_bc = NULL;
        Counted thing;
        DWORD key = 0, key2 = 0;
        IUnknown *back = NULL;
        LONG before;

        thing.vtable = &counted_vtable;
        thing.refs = 1;

        hr = GetRunningObjectTable(0, &rot);
        CHECK(hr == S_OK && rot != NULL);
        hr = GetRunningObjectTable(0, &again);
        CHECK(hr == S_OK && again == rot);
        IRunningObjectTable_Release(again);
        hr = IBindCtx_GetRunningObjectTable(bc, &from_bc);
        CHECK(hr == S_OK && from_bc != NULL);
        IRunningObjectTable_Release(from_bc);

        before = thing.refs;
        hr = IRunningObjectTable_Register(rot, 0, (IUnknown *)&thing, self_mk, &key);
        CHECK(hr == S_OK && key != 0);
        /* The table holds the object while it is registered. */
        CHECK(thing.refs > before);

        CHECK(IRunningObjectTable_IsRunning(rot, self_mk) == S_OK);
        CHECK(IMoniker_IsRunning(self_mk, bc, NULL, NULL) == S_OK);

        /* A separate moniker that is merely equal finds it too. */
        {
            IMoniker *equal = NULL;
            hr = CreateFileMoniker(upper, &equal);
            CHECK(hr == S_OK && equal != NULL);
            CHECK(IRunningObjectTable_IsRunning(rot, equal) == S_OK);
            back = NULL;
            CHECK(IRunningObjectTable_GetObject(rot, equal, &back) == S_OK);
            CHECK(back == (IUnknown *)&thing);
            if (back) {
                IUnknown_Release(back);
                back = NULL;
            }
            IMoniker_Release(equal);
        }

        /* The same moniker registered again: a success of its own, and a
           key of its own. */
        hr = IRunningObjectTable_Register(rot, 0, (IUnknown *)&thing, self_mk, &key2);
        CHECK(hr == MK_S_ALREADY && key2 != 0 && key2 != key);
        CHECK(IRunningObjectTable_Revoke(rot, key2) == S_OK);

        {
            FILETIME ft;
            memset(&ft, 0, sizeof ft);
            CHECK(IRunningObjectTable_GetTimeOfLastChange(rot, self_mk, &ft) == S_OK);
            CHECK(ft.dwHighDateTime != 0 || ft.dwLowDateTime != 0);
            CHECK(IRunningObjectTable_NoteChangeTime(rot, key, &ft) == S_OK);
        }

        /* The enumeration holds a moniker equal to the registered one. */
        {
            IEnumMoniker *e = NULL;
            IMoniker *got = NULL;
            ULONG n = 0;
            int found = 0;
            hr = IRunningObjectTable_EnumRunning(rot, &e);
            CHECK(hr == S_OK && e != NULL);
            while (IEnumMoniker_Next(e, 1, &got, &n) == S_OK && n == 1 && got) {
                if (IMoniker_IsEqual(got, self_mk) == S_OK) {
                    found = 1;
                }
                IMoniker_Release(got);
                got = NULL;
                n = 0;
            }
            CHECK(found);
            IEnumMoniker_Release(e);
        }

        /* Revoked: the references given back, and every look-up refusing
           in its own way. */
        CHECK(IRunningObjectTable_Revoke(rot, key) == S_OK);
        CHECK(thing.refs == before);
        CHECK(IRunningObjectTable_IsRunning(rot, self_mk) == S_FALSE);
        CHECK(IMoniker_IsRunning(self_mk, bc, NULL, NULL) == S_FALSE);
        CHECK(IRunningObjectTable_Revoke(rot, key) == E_INVALIDARG);
        back = NULL;
        CHECK(IRunningObjectTable_GetObject(rot, self_mk, &back) == MK_E_UNAVAILABLE);
        CHECK(back == NULL);

        IRunningObjectTable_Release(rot);
    }

    if (comp) {
        IMoniker_Release(comp);
    }
    if (anti) {
        IMoniker_Release(anti);
    }
    if (item) {
        IMoniker_Release(item);
    }
    IMoniker_Release(self_mk);
    IBindCtx_Release(bc);
    CoUninitialize();

    printf("moniker_test: ok (%d checks)\n", check);
    fflush(stdout);
    return 0;
}
