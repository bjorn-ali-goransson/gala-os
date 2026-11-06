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
 * Prints "shell32_test: ok" or exits with the number of the failing
 * check. Run as "shell32_test child" it just exits with 7. */
#include <windows.h>
#include <shellapi.h>
#include <shlobj.h>
#include <knownfolders.h>
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

static int fileop(UINT func, const wchar_t *from, const wchar_t *to, FILEOP_FLAGS flags) {
    SHFILEOPSTRUCTW op;
    memset(&op, 0, sizeof op);
    op.wFunc = func;
    op.pFrom = from;
    op.pTo = to;
    op.fFlags = flags | FOF_NOCONFIRMATION | FOF_SILENT | FOF_NOERRORUI;
    return SHFileOperationW(&op);
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

    printf("shell32_test: ok\n");
    return 0;
}
