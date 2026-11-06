/* shell32 as real Windows behaves, for what wxWidgets (FileZilla,
 * FileZilla Server's GUI) imports and gala lacked (docs/ROADMAP.md AG):
 * ShellExecuteExW (and ShellExecuteW) running a program, the dropped-file
 * functions on an HDROP, DragAcceptFiles, SHFileOperationW, and the
 * notifications (SHChangeNotify, Shell_NotifyIconW,
 * SHGetIconOverlayIndexW).
 *
 * Expected values are from Microsoft's documentation (learn.microsoft.com:
 * ShellExecuteExW, SHELLEXECUTEINFOW, DragQueryFileW, DragQueryPoint,
 * DROPFILES, DragAcceptFiles, SHFileOperationW, SHFILEOPSTRUCTW); no
 * Windows machine was available to run this one, so it still needs a
 * run-all.ps1 check there. In short:
 *
 * - ShellExecuteExW with SEE_MASK_NOCLOSEPROCESS runs a program and
 *   gives its process handle (hInstApp > 32); a file that is not there
 *   is FALSE with ERROR_FILE_NOT_FOUND and hInstApp SE_ERR_FNF.
 *   ShellExecuteW returns a value over 32 on success.
 * - DragQueryFileW(0xFFFFFFFF) counts the files of a DROPFILES block
 *   (wide or ANSI names, double-NUL-terminated); a NULL buffer gives a
 *   name's length; a short buffer gets a truncated, NUL-terminated name
 *   and the copied length. DragQueryPoint gives pt and TRUE when fNC is
 *   0. DragFinish frees it.
 * - DragAcceptFiles sets and clears WS_EX_ACCEPTFILES.
 * - ExtractIconW: with index -1 the number of icon groups; index n the
 *   n-th group's icon (SM_CXICON sized), a negative index the group with
 *   that resource ID; NULL past the last or for a missing file; 1 for a
 *   file that is not an image. ExtractIconExW: the count, or large
 *   (32) and small (16) icons. SHDefExtractIconW: S_OK, S_FALSE when the
 *   icon is not there.
 * - SHFileOperationW: FO_COPY of a directory to a new name copies the
 *   tree; FO_MOVE and FO_RENAME of a file; FO_DELETE of a wildcard with
 *   FOF_FILESONLY removes only files; FO_DELETE of directories (a
 *   double-NUL list) removes the trees; a source that is not there is
 *   not 0.
 * - What Chromium delay-imports (some by ordinal: SHChangeNotifyRegister
 *   #2, SHChangeNotifyDeregister #4, ILGetSize #152, ILFree #155,
 *   ILCreateFromPathW #190, IsUserAnAdmin #680; shlwapi's
 *   SHCreateMemStream #12): ILCreateFromPathW gives the list of an
 *   existing directory (its path read back by SHGetPathFromIDListW) and
 *   NULL for one that is not there; ILGetSize is the list's size with its
 *   2-byte terminator (2 for the desktop's empty list, 0 for NULL); SHQueryUserNotificationState is
 *   S_OK; ABM_GETTASKBARPOS fills the taskbar's rectangle (within the
 *   screen, not the whole screen) and edge when there is a taskbar; ShellExecuteA of a file that
 *   is not there returns at most 32; SHCreateMemStream holds a copy of
 *   the bytes, read back from its start.
 * - Folders Inno Setup installs to: SHGetFolderPathW(CSIDL_PROGRAMS |
 *   CSIDL_FLAG_CREATE) is %APPDATA%\Microsoft\Windows\Start Menu\Programs
 *   and exists afterwards; SHGetKnownFolderPath(FOLDERID_UserProgramFiles,
 *   KF_FLAG_CREATE) is %LOCALAPPDATA%\Programs and exists afterwards;
 *   CSIDL_COMMON_PROGRAMS is under %ProgramData%.
 *
 * - Shell links (CLSID_ShellLink; Microsoft's IShellLinkW, IPersistFile,
 *   IShellLinkDataList pages, [MS-SHLLINK] for the file, [MS-PROPSTORE]
 *   for its property store, and the checks Wine's shell32/tests/
 *   shelllink.c makes on Windows): a new link has no path (GetPath
 *   S_FALSE, an empty string, a zeroed WIN32_FIND_DATAW), an empty
 *   description, arguments and icon (index 0), SW_SHOWNORMAL, no hot
 *   key, no flags, no Darwin block (CopyDataBlock E_FAIL, NULL);
 *   SetPath(NULL) is E_INVALIDARG; a path that does not exist is kept
 *   without its trailing backslash (the find data: no attributes, the
 *   last part's name); an existing file's find data has its attributes,
 *   size and name; a path with environment variables sets
 *   SLDF_HAS_EXP_SZ and an EXP_SZ_LINK block with the unexpanded path.
 *   IPersistFile: GetCurFile is S_FALSE with NULL before a Save or Load,
 *   S_OK with the file's name after; IsDirty S_OK after a change, S_FALSE
 *   after Save(name, TRUE). The saved file starts with the 76-byte
 *   ShellLinkHeader (HeaderSize 0x4C, CLSID_ShellLink, LinkFlags with
 *   HasName, HasWorkingDir, HasArguments, HasIconLocation and IsUnicode
 *   and the target's place (an IDList or LinkInfo), the target's size,
 *   the icon index, the show command, the hot key, zeroed reserved
 *   fields), holds a PropertyStoreDataBlock for an AppUserModel ID set
 *   through IPropertyStore, and ends with the terminal block; a new link
 *   loaded from it gives every field back, the property's value too. The
 *   specification's own sample (section 3.1, a link to C:\test\a.txt)
 *   loads: its path, working directory, show command and
 *   SLDF_ENABLE_TARGET_METADATA. SetFlags/GetFlags keep SLDF_RUNAS_USER.
 * - SHAddToRecentDocs(SHARD_PATHW): a link named after the file in the
 *   user's Recent folder (CSIDL_RECENT).
 * - kernel32's EnumResourceLanguagesW (Inno Setup's compiler imports it):
 *   one call for this program's icon group 2, with the language
 *   FindResourceExW finds it in; ERROR_RESOURCE_NAME_NOT_FOUND and
 *   ERROR_RESOURCE_TYPE_NOT_FOUND for what is not there.
 * Prints "shell32_test: ok" or exits with the number of the failing
 * check. Run as "shell32_test child" it just exits with 7. */
#include <windows.h>
#include <shellapi.h>
#include <shlobj.h>
#include <knownfolders.h>
#include <shobjidl.h>
#include <propsys.h>
#include <stdio.h>
#include <string.h>

static int check;
#define CHECK(cond) do { check++; if (!(cond)) { printf("shell32_test: check %d (line %d) failed\n", check, __LINE__); fflush(stdout); ExitProcess(check); } } while (0)

static int exists(const wchar_t *p) { return GetFileAttributesW(p) != INVALID_FILE_ATTRIBUTES; }

static void touch(const wchar_t *p) {
    HANDLE h = CreateFileW(p, GENERIC_WRITE, 0, NULL, CREATE_ALWAYS, 0, NULL);
    CHECK(h != INVALID_HANDLE_VALUE);
    DWORD n;
    WriteFile(h, "data", 4, &n, NULL);
    CloseHandle(h);
}

/* An HDROP with `names` (each NUL-terminated, then a final NUL). */
static HDROP make_drop(const void *names, size_t bytes, BOOL wide, LONG x, LONG y, BOOL nc) {
    HGLOBAL h = GlobalAlloc(GMEM_MOVEABLE | GMEM_ZEROINIT, sizeof(DROPFILES) + bytes);
    DROPFILES *d = GlobalLock(h);
    d->pFiles = sizeof(DROPFILES);
    d->pt.x = x;
    d->pt.y = y;
    d->fNC = nc;
    d->fWide = wide;
    memcpy((char *)d + sizeof(DROPFILES), names, bytes);
    GlobalUnlock(h);
    return (HDROP)h;
}

/* The width of an icon's colour (or mask) bitmap. */
static int icon_width(HICON icon) {
    ICONINFO ii;
    if (!GetIconInfo(icon, &ii))
        return -1;
    BITMAP bm;
    HBITMAP b = ii.hbmColor ? ii.hbmColor : ii.hbmMask;
    int w = GetObjectW(b, sizeof bm, &bm) ? bm.bmWidth : -1;
    if (ii.hbmColor)
        DeleteObject(ii.hbmColor);
    if (ii.hbmMask)
        DeleteObject(ii.hbmMask);
    return w;
}

static int langs_seen;
static WORD lang_seen;
static BOOL CALLBACK count_lang(HMODULE m, LPCWSTR type, LPCWSTR name, WORD lang, LONG_PTR param) {
    if (param == 7 && type == (LPCWSTR)RT_GROUP_ICON && name == MAKEINTRESOURCEW(2)) {
        langs_seen++;
        lang_seen = lang;
    }
    return TRUE;
}

static int fileop(UINT func, const wchar_t *from, const wchar_t *to, FILEOP_FLAGS flags) {
    SHFILEOPSTRUCTW op;
    memset(&op, 0, sizeof op);
    op.wFunc = func;
    op.pFrom = from;
    op.pTo = to;
    op.fFlags = flags | FOF_NOCONFIRMATION | FOF_SILENT | FOF_NOERRORUI;
    return SHFileOperationW(&op);
}

static const GUID clsid_shell_link = {0x00021401, 0, 0, {0xc0, 0, 0, 0, 0, 0, 0, 0x46}};
static const GUID iid_shell_link_w = {0x000214f9, 0, 0, {0xc0, 0, 0, 0, 0, 0, 0, 0x46}};
static const GUID iid_persist_file = {0x0000010b, 0, 0, {0xc0, 0, 0, 0, 0, 0, 0, 0x46}};
static const GUID iid_data_list = {0x45e2b4ae, 0xb1c3, 0x11d0, {0xb9, 0x2f, 0x00, 0xa0, 0xc9, 0x03, 0x12, 0xe1}};
static const GUID iid_property_store = {0x886d8eeb, 0x8cf2, 0x4446, {0x8d, 0x02, 0xcd, 0xba, 0x1d, 0xbd, 0xcf, 0x99}};
static const PROPERTYKEY app_id_key = {{0x9f4c2855, 0x9f79, 0x4b39, {0xa8, 0xd0, 0xe1, 0xd4, 0x2d, 0xe1, 0xd5, 0xf3}}, 5};

/* [MS-SHLLINK] 3.1: a shortcut to C:\test\a.txt, as Windows wrote it. */
static const unsigned char spec_sample[] = {
    0x4C,0x00,0x00,0x00,0x01,0x14,0x02,0x00,0x00,0x00,0x00,0x00,0xC0,0x00,0x00,0x00,
    0x00,0x00,0x00,0x46,0x9B,0x00,0x08,0x00,0x20,0x00,0x00,0x00,0xD0,0xE9,0xEE,0xF2,
    0x15,0x15,0xC9,0x01,0xD0,0xE9,0xEE,0xF2,0x15,0x15,0xC9,0x01,0xD0,0xE9,0xEE,0xF2,
    0x15,0x15,0xC9,0x01,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x01,0x00,0x00,0x00,
    0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0xBD,0x00,0x14,0x00,
    0x1F,0x50,0xE0,0x4F,0xD0,0x20,0xEA,0x3A,0x69,0x10,0xA2,0xD8,0x08,0x00,0x2B,0x30,
    0x30,0x9D,0x19,0x00,0x2F,0x43,0x3A,0x5C,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,
    0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x46,0x00,0x31,0x00,0x00,
    0x00,0x00,0x00,0x2C,0x39,0x69,0xA3,0x10,0x00,0x74,0x65,0x73,0x74,0x00,0x00,0x32,
    0x00,0x07,0x00,0x04,0x00,0xEF,0xBE,0x2C,0x39,0x65,0xA3,0x2C,0x39,0x69,0xA3,0x26,
    0x00,0x00,0x00,0x03,0x1E,0x00,0x00,0x00,0x00,0xF5,0x1E,0x00,0x00,0x00,0x00,0x00,
    0x00,0x00,0x00,0x00,0x00,0x74,0x00,0x65,0x00,0x73,0x00,0x74,0x00,0x00,0x00,0x14,
    0x00,0x48,0x00,0x32,0x00,0x00,0x00,0x00,0x00,0x2C,0x39,0x69,0xA3,0x20,0x00,0x61,
    0x2E,0x74,0x78,0x74,0x00,0x34,0x00,0x07,0x00,0x04,0x00,0xEF,0xBE,0x2C,0x39,0x69,
    0xA3,0x2C,0x39,0x69,0xA3,0x26,0x00,0x00,0x00,0x2D,0x6E,0x00,0x00,0x00,0x00,0x96,
    0x01,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x61,0x00,0x2E,0x00,0x74,
    0x00,0x78,0x00,0x74,0x00,0x00,0x00,0x14,0x00,0x00,0x00,0x3C,0x00,0x00,0x00,0x1C,
    0x00,0x00,0x00,0x01,0x00,0x00,0x00,0x1C,0x00,0x00,0x00,0x2D,0x00,0x00,0x00,0x00,
    0x00,0x00,0x00,0x3B,0x00,0x00,0x00,0x11,0x00,0x00,0x00,0x03,0x00,0x00,0x00,0x81,
    0x8A,0x7A,0x30,0x10,0x00,0x00,0x00,0x00,0x43,0x3A,0x5C,0x74,0x65,0x73,0x74,0x5C,
    0x61,0x2E,0x74,0x78,0x74,0x00,0x00,0x07,0x00,0x2E,0x00,0x5C,0x00,0x61,0x00,0x2E,
    0x00,0x74,0x00,0x78,0x00,0x74,0x00,0x07,0x00,0x43,0x00,0x3A,0x00,0x5C,0x00,0x74,
    0x00,0x65,0x00,0x73,0x00,0x74,0x00,0x60,0x00,0x00,0x00,0x03,0x00,0x00,0xA0,0x58,
    0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x63,0x68,0x72,0x69,0x73,0x2D,0x78,0x70,0x73,
    0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x40,0x78,0xC7,0x94,0x47,0xFA,0xC7,0x46,0xB3,
    0x56,0x5C,0x2D,0xC6,0xB6,0xD1,0x15,0xEC,0x46,0xCD,0x7B,0x22,0x7F,0xDD,0x11,0x94,
    0x99,0x00,0x13,0x72,0x16,0x87,0x4A,0x40,0x78,0xC7,0x94,0x47,0xFA,0xC7,0x46,0xB3,
    0x56,0x5C,0x2D,0xC6,0xB6,0xD1,0x15,0xEC,0x46,0xCD,0x7B,0x22,0x7F,0xDD,0x11,0x94,
    0x99,0x00,0x13,0x72,0x16,0x87,0x4A,0x00,0x00,0x00,0x00,
};

static DWORD rd32(const unsigned char *p) { return p[0] | p[1] << 8 | p[2] << 16 | (DWORD)p[3] << 24; }

/* The whole of a small file into buf; its size (0 if not there). */
static DWORD slurp(const wchar_t *path, unsigned char *buf, DWORD size) {
    HANDLE h = CreateFileW(path, GENERIC_READ, FILE_SHARE_READ, NULL, OPEN_EXISTING, 0, NULL);
    if (h == INVALID_HANDLE_VALUE)
        return 0;
    DWORD n = 0;
    ReadFile(h, buf, size, &n, NULL);
    CloseHandle(h);
    return n;
}

/* Whether `hay` holds a StringData of `s`: a 16-bit count, then UTF-16. */
static int has_string_data(const unsigned char *hay, DWORD n, const wchar_t *s) {
    size_t len = wcslen(s);
    for (DWORD i = 0; i + 2 + 2 * len <= n; i++)
        if (hay[i] == (len & 0xff) && hay[i + 1] == len >> 8 && memcmp(hay + i + 2, s, 2 * len) == 0)
            return 1;
    return 0;
}

static IShellLinkW *new_link(void) {
    IShellLinkW *sl = NULL;
    CHECK(CoCreateInstance(&clsid_shell_link, NULL, CLSCTX_INPROC_SERVER, &iid_shell_link_w, (void **)&sl) == S_OK);
    return sl;
}

static void links(const wchar_t *self) {
    CHECK(SUCCEEDED(CoInitialize(NULL)));
    IShellLinkW *sl = new_link();
    wchar_t buf[MAX_PATH], dir[MAX_PATH], link[MAX_PATH];
    WIN32_FIND_DATAW fd;
    int i;
    WORD hk;
    DWORD flags;

    /* A new link. */
    wcscpy(buf, L"garbage");
    memset(&fd, 0xaa, sizeof fd);
    CHECK(sl->lpVtbl->GetPath(sl, buf, MAX_PATH, &fd, SLGP_RAWPATH) == S_FALSE && !buf[0]);
    CHECK(fd.dwFileAttributes == 0 && fd.cFileName[0] == 0);
    wcscpy(buf, L"garbage");
    CHECK(sl->lpVtbl->GetDescription(sl, buf, MAX_PATH) == S_OK && !buf[0]);
    wcscpy(buf, L"garbage");
    CHECK(sl->lpVtbl->GetArguments(sl, buf, MAX_PATH) == S_OK && !buf[0]);
    wcscpy(buf, L"garbage");
    i = 0xdead;
    CHECK(sl->lpVtbl->GetIconLocation(sl, buf, MAX_PATH, &i) == S_OK && !buf[0] && i == 0);
    CHECK(sl->lpVtbl->GetShowCmd(sl, &i) == S_OK && i == SW_SHOWNORMAL);
    hk = 0xbeef;
    CHECK(sl->lpVtbl->GetHotkey(sl, &hk) == S_OK && hk == 0);
    IShellLinkDataList *dl = NULL;
    CHECK(sl->lpVtbl->QueryInterface(sl, &iid_data_list, (void **)&dl) == S_OK);
    flags = 0xdead;
    CHECK(dl->lpVtbl->GetFlags(dl, &flags) == S_OK && flags == 0);
    void *block = (void *)1;
    CHECK(dl->lpVtbl->CopyDataBlock(dl, EXP_DARWIN_ID_SIG, &block) == E_FAIL && block == NULL);
    CHECK(sl->lpVtbl->SetPath(sl, NULL) == E_INVALIDARG);
    CHECK(sl->lpVtbl->SetPath(sl, L"c:\\nonexistent\\file\\") == S_OK);
    memset(&fd, 0xaa, sizeof fd);
    CHECK(sl->lpVtbl->GetPath(sl, buf, MAX_PATH, &fd, SLGP_RAWPATH) == S_OK);
    CHECK(_wcsicmp(buf, L"c:\\nonexistent\\file") == 0);
    CHECK(fd.dwFileAttributes == 0 && _wcsicmp(fd.cFileName, L"file") == 0);
    /* A path with an environment variable. */
    CHECK(sl->lpVtbl->SetPath(sl, L"%TEMP%\\gala-link.txt") == S_OK);
    CHECK(dl->lpVtbl->GetFlags(dl, &flags) == S_OK && (flags & SLDF_HAS_EXP_SZ));
    EXP_SZ_LINK *exp = NULL;
    CHECK(dl->lpVtbl->CopyDataBlock(dl, EXP_SZ_LINK_SIG, (void **)&exp) == S_OK && exp != NULL);
    CHECK(exp->dwSignature == EXP_SZ_LINK_SIG && exp->cbSize == sizeof(EXP_SZ_LINK));
    CHECK(_wcsicmp(exp->swzTarget, L"%TEMP%\\gala-link.txt") == 0);
    LocalFree(exp);

    /* A link to this program, saved. */
    CHECK(GetCurrentDirectoryW(MAX_PATH, dir) > 0);
    wcscpy(link, dir);
    wcscat(link, L"\\sh_link.lnk");
    CHECK(sl->lpVtbl->SetPath(sl, self) == S_OK);
    memset(&fd, 0, sizeof fd);
    CHECK(sl->lpVtbl->GetPath(sl, buf, MAX_PATH, &fd, SLGP_RAWPATH) == S_OK && _wcsicmp(buf, self) == 0);
    WIN32_FILE_ATTRIBUTE_DATA self_info;
    CHECK(GetFileAttributesExW(self, GetFileExInfoStandard, &self_info));
    CHECK(fd.dwFileAttributes == self_info.dwFileAttributes && fd.nFileSizeLow == self_info.nFileSizeLow);
    CHECK(_wcsicmp(fd.cFileName, L"shell32_test.exe") == 0);
    CHECK(sl->lpVtbl->SetArguments(sl, L"/option1 \"Some string\"") == S_OK);
    CHECK(sl->lpVtbl->SetWorkingDirectory(sl, dir) == S_OK);
    CHECK(sl->lpVtbl->SetDescription(sl, L"gala's test link") == S_OK);
    CHECK(sl->lpVtbl->SetIconLocation(sl, self, 1) == S_OK);
    CHECK(sl->lpVtbl->SetShowCmd(sl, SW_SHOWMINNOACTIVE) == S_OK);
    CHECK(sl->lpVtbl->SetHotkey(sl, 0x0241) == S_OK); /* Ctrl+A */
    CHECK(dl->lpVtbl->GetFlags(dl, &flags) == S_OK);
    CHECK(dl->lpVtbl->SetFlags(dl, flags | SLDF_RUNAS_USER) == S_OK);
    CHECK(dl->lpVtbl->GetFlags(dl, &flags) == S_OK && (flags & SLDF_RUNAS_USER));
    IPropertyStore *ps = NULL;
    CHECK(sl->lpVtbl->QueryInterface(sl, &iid_property_store, (void **)&ps) == S_OK);
    PROPVARIANT pv;
    PropVariantInit(&pv);
    pv.vt = VT_LPWSTR;
    pv.pwszVal = L"Gala.Test.Link";
    CHECK(ps->lpVtbl->SetValue(ps, &app_id_key, &pv) == S_OK);
    CHECK(ps->lpVtbl->Commit(ps) == S_OK);
    IPersistFile *pf = NULL;
    CHECK(sl->lpVtbl->QueryInterface(sl, &iid_persist_file, (void **)&pf) == S_OK);
    LPOLESTR cur = (LPOLESTR)1;
    CHECK(pf->lpVtbl->GetCurFile(pf, &cur) == S_FALSE && cur == NULL);
    CHECK(pf->lpVtbl->IsDirty(pf) == S_OK);
    CHECK(pf->lpVtbl->Save(pf, link, TRUE) == S_OK);
    CHECK(pf->lpVtbl->IsDirty(pf) == S_FALSE);
    CHECK(pf->lpVtbl->GetCurFile(pf, &cur) == S_OK && cur != NULL && wcscmp(cur, link) == 0);
    CoTaskMemFree(cur);
    pf->lpVtbl->Release(pf);
    ps->lpVtbl->Release(ps);
    dl->lpVtbl->Release(dl);
    CHECK(sl->lpVtbl->Release(sl) == 0);

    /* The file, as [MS-SHLLINK] lays it out. */
    static unsigned char file[8192];
    DWORD n = slurp(link, file, sizeof file);
    static const unsigned char clsid[16] = {0x01, 0x14, 0x02, 0, 0, 0, 0, 0, 0xc0, 0, 0, 0, 0, 0, 0, 0x46};
    CHECK(n > 0x4c + 4 && rd32(file) == 0x4c && memcmp(file + 4, clsid, 16) == 0);
    DWORD lf = rd32(file + 0x14);
    DWORD want = 0x4 | 0x10 | 0x20 | 0x40 | 0x80; /* HasName, HasWorkingDir, HasArguments, HasIconLocation, IsUnicode */
    CHECK((lf & want) == want && (lf & 0x3) != 0 && (lf & 0x2000));
    CHECK(rd32(file + 0x34) == self_info.nFileSizeLow && rd32(file + 0x38) == 1 && rd32(file + 0x3c) == 7);
    CHECK(file[0x40] == 0x41 && file[0x41] == 0x02 && file[0x42] == 0 && file[0x43] == 0);
    CHECK(rd32(file + 0x44) == 0 && rd32(file + 0x48) == 0);
    CHECK(has_string_data(file, n, L"gala's test link") && has_string_data(file, n, L"/option1 \"Some string\""));
    CHECK(has_string_data(file, n, dir));
    int store = 0;
    for (DWORD k = 0x4c; k + 8 <= n; k++)
        if (rd32(file + k) == 0xa0000009 && rd32(file + k - 4) >= 0xc)
            store = 1;
    CHECK(store && rd32(file + n - 4) == 0);

    /* Loaded back into a new link. */
    sl = new_link();
    CHECK(sl->lpVtbl->QueryInterface(sl, &iid_persist_file, (void **)&pf) == S_OK);
    CHECK(pf->lpVtbl->Load(pf, link, STGM_READ) == S_OK);
    CHECK(pf->lpVtbl->GetCurFile(pf, &cur) == S_OK && wcscmp(cur, link) == 0);
    CoTaskMemFree(cur);
    CHECK(sl->lpVtbl->GetPath(sl, buf, MAX_PATH, NULL, SLGP_RAWPATH) == S_OK && _wcsicmp(buf, self) == 0);
    CHECK(sl->lpVtbl->GetArguments(sl, buf, MAX_PATH) == S_OK && wcscmp(buf, L"/option1 \"Some string\"") == 0);
    CHECK(sl->lpVtbl->GetWorkingDirectory(sl, buf, MAX_PATH) == S_OK && _wcsicmp(buf, dir) == 0);
    CHECK(sl->lpVtbl->GetDescription(sl, buf, MAX_PATH) == S_OK && wcscmp(buf, L"gala's test link") == 0);
    CHECK(sl->lpVtbl->GetIconLocation(sl, buf, MAX_PATH, &i) == S_OK && _wcsicmp(buf, self) == 0 && i == 1);
    CHECK(sl->lpVtbl->GetShowCmd(sl, &i) == S_OK && i == SW_SHOWMINNOACTIVE);
    CHECK(sl->lpVtbl->GetHotkey(sl, &hk) == S_OK && hk == 0x0241);
    CHECK(sl->lpVtbl->QueryInterface(sl, &iid_data_list, (void **)&dl) == S_OK);
    CHECK(dl->lpVtbl->GetFlags(dl, &flags) == S_OK && (flags & SLDF_RUNAS_USER));
    dl->lpVtbl->Release(dl);
    CHECK(sl->lpVtbl->QueryInterface(sl, &iid_property_store, (void **)&ps) == S_OK);
    PropVariantInit(&pv);
    CHECK(ps->lpVtbl->GetValue(ps, &app_id_key, &pv) == S_OK && pv.vt == VT_LPWSTR);
    CHECK(wcscmp(pv.pwszVal, L"Gala.Test.Link") == 0);
    PropVariantClear(&pv);
    ps->lpVtbl->Release(ps);
    pf->lpVtbl->Release(pf);
    sl->lpVtbl->Release(sl);
    CHECK(DeleteFileW(link));

    /* The specification's sample. */
    wcscpy(link, dir);
    wcscat(link, L"\\sh_sample.lnk");
    HANDLE h = CreateFileW(link, GENERIC_WRITE, 0, NULL, CREATE_ALWAYS, 0, NULL);
    DWORD wrote;
    CHECK(h != INVALID_HANDLE_VALUE && WriteFile(h, spec_sample, sizeof spec_sample, &wrote, NULL));
    CloseHandle(h);
    sl = new_link();
    CHECK(sl->lpVtbl->QueryInterface(sl, &iid_persist_file, (void **)&pf) == S_OK);
    CHECK(pf->lpVtbl->Load(pf, link, STGM_READ) == S_OK);
    CHECK(sl->lpVtbl->GetPath(sl, buf, MAX_PATH, NULL, SLGP_RAWPATH) == S_OK);
    CHECK(_wcsicmp(buf, L"C:\\test\\a.txt") == 0);
    CHECK(sl->lpVtbl->GetWorkingDirectory(sl, buf, MAX_PATH) == S_OK && _wcsicmp(buf, L"c:\\test") == 0);
    CHECK(sl->lpVtbl->GetShowCmd(sl, &i) == S_OK && i == SW_SHOWNORMAL);
    CHECK(sl->lpVtbl->QueryInterface(sl, &iid_data_list, (void **)&dl) == S_OK);
    CHECK(dl->lpVtbl->GetFlags(dl, &flags) == S_OK && (flags & SLDF_ENABLE_TARGET_METADATA));
    dl->lpVtbl->Release(dl);
    pf->lpVtbl->Release(pf);
    sl->lpVtbl->Release(sl);
    CHECK(DeleteFileW(link));
    CoUninitialize();
}

/* SHAddToRecentDocs: a link in the Recent folder named after the file. */
static void recent_docs(void) {
    wchar_t doc[MAX_PATH], recent[MAX_PATH];
    CHECK(GetCurrentDirectoryW(MAX_PATH, doc) > 0);
    wcscat(doc, L"\\sh_recent.txt");
    touch(doc);
    CHECK(SHGetFolderPathW(NULL, CSIDL_RECENT, NULL, 0, recent) == S_OK);
    wcscat(recent, L"\\sh_recent.txt.lnk");
    DeleteFileW(recent);
    SHAddToRecentDocs(SHARD_PATHW, doc);
    /* (Windows may add it from another thread: give it a while.) */
    for (int waited = 0; !exists(recent) && waited < 100; waited++)
        Sleep(100);
    CHECK(exists(recent));
    DeleteFileW(recent);
    DeleteFileW(doc);
}

int main(int argc, char **argv) {
    if (argc > 1 && strcmp(argv[1], "child") == 0)
        return 7;

    /* Start menu and per-user program folders. */
    wchar_t folder[MAX_PATH], want[MAX_PATH];
    CHECK(SHGetFolderPathW(NULL, CSIDL_PROGRAMS | CSIDL_FLAG_CREATE, NULL, 0, folder) == S_OK);
    CHECK(GetEnvironmentVariableW(L"APPDATA", want, MAX_PATH) > 0);
    wcscat(want, L"\\Microsoft\\Windows\\Start Menu\\Programs");
    CHECK(_wcsicmp(folder, want) == 0 && exists(folder));
    PWSTR known = NULL;
    CHECK(SHGetKnownFolderPath(&FOLDERID_UserProgramFiles, KF_FLAG_CREATE, NULL, &known) == S_OK);
    CHECK(GetEnvironmentVariableW(L"LOCALAPPDATA", want, MAX_PATH) > 0);
    wcscat(want, L"\\Programs");
    CHECK(_wcsicmp(known, want) == 0 && exists(known));
    CoTaskMemFree(known);
    CHECK(SHGetFolderPathW(NULL, CSIDL_COMMON_PROGRAMS, NULL, 0, folder) == S_OK);
    CHECK(GetEnvironmentVariableW(L"ProgramData", want, MAX_PATH) > 0);
    CHECK(_wcsnicmp(folder, want, wcslen(want)) == 0 && wcsstr(folder, L"\\Start Menu\\Programs") != NULL);

    /* ShellExecuteExW and ShellExecuteW running this program. */
    wchar_t self[MAX_PATH];
    CHECK(GetModuleFileNameW(NULL, self, MAX_PATH) > 0);
    SHELLEXECUTEINFOW info;
    memset(&info, 0, sizeof info);
    info.cbSize = sizeof info;
    info.fMask = SEE_MASK_NOCLOSEPROCESS;
    info.lpVerb = L"open";
    info.lpFile = self;
    info.lpParameters = L"child";
    info.nShow = SW_SHOWNORMAL;
    CHECK(ShellExecuteExW(&info) && (INT_PTR)info.hInstApp > 32 && info.hProcess != NULL);
    CHECK(WaitForSingleObject(info.hProcess, 30000) == WAIT_OBJECT_0);
    DWORD code = 0;
    CHECK(GetExitCodeProcess(info.hProcess, &code) && code == 7);
    CloseHandle(info.hProcess);
    memset(&info, 0, sizeof info);
    info.cbSize = sizeof info;
    info.fMask = SEE_MASK_NOCLOSEPROCESS | SEE_MASK_FLAG_NO_UI;
    info.lpFile = L"C:\\no-such-dir\\no-such-program.exe";
    SetLastError(0);
    CHECK(!ShellExecuteExW(&info) && GetLastError() == ERROR_FILE_NOT_FOUND);
    CHECK((INT_PTR)info.hInstApp == SE_ERR_FNF && info.hProcess == NULL);
    CHECK((INT_PTR)ShellExecuteW(NULL, NULL, self, L"child", NULL, SW_HIDE) > 32);

    /* Dropped files: wide names. */
    static const wchar_t wide[] = L"C:\\one.txt\0C:\\dir\\two\0";
    HDROP drop = make_drop(wide, sizeof wide, TRUE, 12, 34, FALSE);
    wchar_t name[MAX_PATH];
    CHECK(DragQueryFileW(drop, 0xFFFFFFFF, NULL, 0) == 2);
    CHECK(DragQueryFileW(drop, 0, NULL, 0) == 10);
    CHECK(DragQueryFileW(drop, 1, name, MAX_PATH) == 10 && wcscmp(name, L"C:\\dir\\two") == 0);
    CHECK(DragQueryFileW(drop, 0, name, 4) == 3 && wcscmp(name, L"C:\\") == 0);
    CHECK(DragQueryFileW(drop, 2, name, MAX_PATH) == 0);
    POINT pt = {0, 0};
    CHECK(DragQueryPoint(drop, &pt) && pt.x == 12 && pt.y == 34);
    DragFinish(drop);
    /* ANSI names, dropped in the non-client area. */
    static const char ansi[] = "C:\\a.txt\0";
    drop = make_drop(ansi, sizeof ansi, FALSE, 5, 6, TRUE);
    CHECK(DragQueryFileW(drop, 0xFFFFFFFF, NULL, 0) == 1);
    CHECK(DragQueryFileW(drop, 0, name, MAX_PATH) == 8 && wcscmp(name, L"C:\\a.txt") == 0);
    CHECK(!DragQueryPoint(drop, &pt) && pt.x == 5 && pt.y == 6);
    DragFinish(drop);

    /* DragAcceptFiles. */
    HWND w = CreateWindowExW(0, L"STATIC", L"drop", WS_POPUP, 0, 0, 50, 50, NULL, NULL, NULL, NULL);
    CHECK(w != NULL);
    CHECK((GetWindowLongW(w, GWL_EXSTYLE) & WS_EX_ACCEPTFILES) == 0);
    DragAcceptFiles(w, TRUE);
    CHECK((GetWindowLongW(w, GWL_EXSTYLE) & WS_EX_ACCEPTFILES) != 0);
    DragAcceptFiles(w, FALSE);
    CHECK((GetWindowLongW(w, GWL_EXSTYLE) & WS_EX_ACCEPTFILES) == 0);
    DestroyWindow(w);

    /* SHFileOperationW. */
    CHECK(CreateDirectoryW(L"sh_a", NULL) && CreateDirectoryW(L"sh_a\\b", NULL));
    touch(L"sh_a\\b\\c.txt");
    touch(L"sh_a\\d.txt");
    touch(L"sh_a\\e.txt");
    CHECK(fileop(FO_COPY, L"sh_a\0", L"sh_copy\0", 0) == 0);
    CHECK(exists(L"sh_copy\\b\\c.txt") && exists(L"sh_copy\\d.txt") && exists(L"sh_a\\d.txt"));
    CHECK(fileop(FO_MOVE, L"sh_copy\\d.txt\0", L"sh_moved.txt\0", 0) == 0);
    CHECK(exists(L"sh_moved.txt") && !exists(L"sh_copy\\d.txt"));
    CHECK(fileop(FO_RENAME, L"sh_moved.txt\0", L"sh_renamed.txt\0", 0) == 0);
    CHECK(exists(L"sh_renamed.txt") && !exists(L"sh_moved.txt"));
    CHECK(fileop(FO_COPY, L"sh_renamed.txt\0", L"sh_a\0", 0) == 0 && exists(L"sh_a\\sh_renamed.txt"));
    CHECK(fileop(FO_DELETE, L"sh_a\\*.txt\0", NULL, FOF_FILESONLY) == 0);
    CHECK(!exists(L"sh_a\\d.txt") && !exists(L"sh_a\\e.txt") && exists(L"sh_a\\b\\c.txt"));
    CHECK(fileop(FO_DELETE, L"sh_a\0sh_copy\0sh_renamed.txt\0", NULL, 0) == 0);
    CHECK(!exists(L"sh_a") && !exists(L"sh_copy") && !exists(L"sh_renamed.txt"));
    CHECK(fileop(FO_DELETE, L"sh_not_there\0", NULL, 0) != 0);

    /* Notifications nothing listens to. */
    SHChangeNotify(SHCNE_UPDATEDIR, SHCNF_PATHW, L"C:\\", NULL);
    CHECK(SHGetIconOverlayIndexW(L"C:\\nothing.ico", 0) == -1);

    /* Icons: this program has two icon groups (build.sh: one with 16 and
     * 32 pixel images, one with 32 only). */
    CHECK((UINT_PTR)ExtractIconW(NULL, self, (UINT)-1) == 2);
    HICON icon = ExtractIconW(NULL, self, 0);
    CHECK(icon != NULL && icon_width(icon) == GetSystemMetrics(SM_CXICON));
    DestroyIcon(icon);
    icon = ExtractIconW(NULL, self, 1);
    CHECK(icon != NULL);
    DestroyIcon(icon);
    CHECK(ExtractIconW(NULL, self, 2) == NULL);
    icon = ExtractIconW(NULL, self, (UINT)-2); /* resource ID 2 */
    CHECK(icon != NULL);
    DestroyIcon(icon);
    CHECK(ExtractIconW(NULL, L"C:\\no-such-file.exe", 0) == NULL);
    HANDLE text = CreateFileW(L"sh_text.txt", GENERIC_WRITE, 0, NULL, CREATE_ALWAYS, 0, NULL);
    DWORD wrote;
    WriteFile(text, "not an image", 12, &wrote, NULL);
    CloseHandle(text);
    CHECK((UINT_PTR)ExtractIconW(NULL, L"sh_text.txt", 0) == 1);
    DeleteFileW(L"sh_text.txt");
    CHECK(ExtractIconExW(self, -1, NULL, NULL, 0) == 2);
    HICON large[2] = {0}, small[2] = {0};
    CHECK(ExtractIconExW(self, 0, large, small, 2) == 2);
    CHECK(large[0] && large[1] && small[0] && small[1]);
    CHECK(icon_width(large[0]) == 32 && icon_width(small[0]) == 16);
    for (int i = 0; i < 2; i++) {
        DestroyIcon(large[i]);
        DestroyIcon(small[i]);
    }
    HICON l = NULL, sm = NULL;
    CHECK(SHDefExtractIconW(self, 0, 0, &l, &sm, MAKELONG(32, 16)) == S_OK && l && sm);
    CHECK(icon_width(l) == 32 && icon_width(sm) == 16);
    DestroyIcon(l);
    DestroyIcon(sm);
    CHECK(SHDefExtractIconW(self, 5, 0, &l, NULL, 32) == S_FALSE);

    /* kernel32's EnumResourceLanguagesW (Inno Setup's compiler imports it):
     * the icon group's one language, which FindResourceExW finds; no such
     * name or type fails. */
    langs_seen = 0;
    CHECK(EnumResourceLanguagesW(NULL, (LPCWSTR)RT_GROUP_ICON, MAKEINTRESOURCEW(2), count_lang, 7));
    CHECK(langs_seen == 1 && FindResourceExW(NULL, (LPCWSTR)RT_GROUP_ICON, MAKEINTRESOURCEW(2), lang_seen) != NULL);
    SetLastError(0);
    CHECK(!EnumResourceLanguagesW(NULL, (LPCWSTR)RT_GROUP_ICON, MAKEINTRESOURCEW(99), count_lang, 7));
    CHECK(GetLastError() == ERROR_RESOURCE_NAME_NOT_FOUND);
    SetLastError(0);
    CHECK(!EnumResourceLanguagesW(NULL, (LPCWSTR)RT_DIALOG, MAKEINTRESOURCEW(2), count_lang, 7));
    CHECK(GetLastError() == ERROR_RESOURCE_TYPE_NOT_FOUND);

    /* What Chromium delay-imports. */
    HMODULE shell = GetModuleHandleA("shell32.dll");
    UINT (WINAPI *il_size)(LPCITEMIDLIST) = (void *)GetProcAddress(shell, MAKEINTRESOURCEA(152));
    LPITEMIDLIST (WINAPI *il_from_path)(LPCWSTR) = (void *)GetProcAddress(shell, MAKEINTRESOURCEA(190));
    ULONG (WINAPI *notify_register)(HWND, int, LONG, UINT, int, const void *) =
        (void *)GetProcAddress(shell, MAKEINTRESOURCEA(2));
    BOOL (WINAPI *notify_deregister)(ULONG) = (void *)GetProcAddress(shell, MAKEINTRESOURCEA(4));
    CHECK(il_from_path && notify_register && notify_deregister);
    CHECK(il_from_path(L"C:\\no\\such\\dir") == NULL);
    LPITEMIDLIST windows = il_from_path(L"C:\\Windows");
    WCHAR back_path[MAX_PATH];
    CHECK(windows != NULL && SHGetPathFromIDListW(windows, back_path) && lstrcmpiW(back_path, L"C:\\Windows") == 0);
    CHECK(il_size(windows) > 2);
    void (WINAPI *il_free)(LPITEMIDLIST) = (void *)GetProcAddress(shell, MAKEINTRESOURCEA(155));
    BOOL (WINAPI *admin)(void) = (void *)GetProcAddress(shell, MAKEINTRESOURCEA(680));
    CHECK(il_size && il_free && admin);
    LPITEMIDLIST desktop = NULL;
    CHECK(SHGetSpecialFolderLocation(NULL, CSIDL_DESKTOP, &desktop) == S_OK && desktop);
    CHECK(il_size(desktop) == 2 && il_size(NULL) == 0);
    il_free(desktop);
    il_free(windows);
    BOOL is_admin = admin();
    CHECK(is_admin == TRUE || is_admin == FALSE);
    QUERY_USER_NOTIFICATION_STATE state = 0;
    CHECK(SHQueryUserNotificationState(&state) == S_OK && state >= 1 && state <= 7);
    APPBARDATA bar = { sizeof bar };
    int sw = GetSystemMetrics(SM_CXSCREEN), sh = GetSystemMetrics(SM_CYSCREEN);
    /* (gala without a screen has no taskbar: then the call fails.) */
    if (SHAppBarMessage(ABM_GETTASKBARPOS, &bar) != 0) {
        CHECK(bar.uEdge <= ABE_BOTTOM);
        CHECK(bar.rc.left >= 0 && bar.rc.top >= 0 && bar.rc.right <= sw && bar.rc.bottom <= sh);
        CHECK(bar.rc.right > bar.rc.left && bar.rc.bottom > bar.rc.top);
        CHECK((bar.rc.right - bar.rc.left) * (bar.rc.bottom - bar.rc.top) < sw * sh);
    }
    CHECK((INT_PTR)ShellExecuteA(NULL, "open", "C:\\no\\such\\file.exe", NULL, NULL, SW_HIDE) <= 32);
    IStream *(WINAPI *mem_stream)(const BYTE *, UINT) =
        (void *)GetProcAddress(LoadLibraryA("shlwapi.dll"), MAKEINTRESOURCEA(12));
    CHECK(mem_stream != NULL);
    IStream *stream = mem_stream((const BYTE *)"gala", 4);
    CHECK(stream != NULL);
    char back[8] = {0};
    ULONG got = 0;
    CHECK(stream->lpVtbl->Read(stream, back, 8, &got) == S_OK && got == 4 && memcmp(back, "gala", 4) == 0);
    stream->lpVtbl->Release(stream);

    links(self);
    recent_docs();

    printf("shell32_test: ok\n");
    return 0;
}
