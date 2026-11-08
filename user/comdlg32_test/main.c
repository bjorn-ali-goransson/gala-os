/* comdlg32 as real Windows behaves, for what Inno Setup's IDE (ISIDE.exe)
 * imports (ROADMAP PK stage 3: its "/assoc" run did not load without
 * them): GetFileTitleW, and the parameter checks of PrintDlgW,
 * FindTextW, ReplaceTextW and ChooseColorW with CommDlgExtendedError.
 *
 * Expected values are from Microsoft's documentation (GetFileTitleW,
 * PRINTDLGW, FINDREPLACEW, CHOOSECOLORW, CommDlgExtendedError) and the
 * checks Wine's dlls/comdlg32/tests (printdlg.c, finddlg.c) make on
 * Windows; no Windows machine was available to run this one, so it still
 * needs a run-all.ps1 check there. In short:
 *
 * - GetFileTitleW: 0 and the file's name (no directory); with no buffer,
 *   or one too small, the size the name needs (NUL included). A name
 *   with an extension comes back with or without it (the user's
 *   preference for showing extensions).
 * - PrintDlgW: NULL is CDERR_INITIALIZATION, a wrong lStructSize
 *   CDERR_STRUCTSIZE; PD_RETURNDEFAULT gives the default printer, or
 *   fails with PDERR_NODEFAULTPRN on a machine without printers.
 * - FindTextW/ReplaceTextW: NULL is CDERR_INITIALIZATION, lStructSize 0
 *   CDERR_STRUCTSIZE, no owner window CDERR_DIALOGFAILURE, no search
 *   buffer (or, for ReplaceText, no replacement buffer)
 *   FRERR_BUFFERLENGTHZERO, FR_ENABLEHOOK without a hook CDERR_NOHOOK,
 *   FR_ENABLETEMPLATEHANDLE without hInstance CDERR_NOHINSTANCE,
 *   FR_ENABLETEMPLATE without one CDERR_FINDRESFAILURE. A valid call
 *   makes the modeless dialog, or (where no dialog can be shown) returns
 *   NULL with CDERR_DIALOGFAILURE.
 * - ChooseColorW with CC_ENABLETEMPLATE naming a template the module
 *   lacks: FALSE, CDERR_FINDRESFAILURE.
 *
 * Prints "comdlg32_test: ok" or exits with the number of the failing
 * check. */
#include <windows.h>
#include <commdlg.h>
#include <cderr.h>
#include <stdio.h>
#include <string.h>

static int check;
#define CHECK(cond) do { check++; if (!(cond)) { printf("comdlg32_test: check %d (line %d) failed (ext %#lx)\n", check, __LINE__, CommDlgExtendedError()); fflush(stdout); ExitProcess(check); } } while (0)

typedef HWND (WINAPI *find_fn)(FINDREPLACEW *);

/* FindTextW or ReplaceTextW fails with `code`. */
static void fails(find_fn f, FINDREPLACEW *fr, DWORD code) {
    HWND h = f(fr);
    CHECK(h == NULL && CommDlgExtendedError() == code);
}

/* A valid call: the dialog, or none with CDERR_DIALOGFAILURE. */
static void works(find_fn f, FINDREPLACEW *fr) {
    HWND h = f(fr);
    if (h != NULL)
        DestroyWindow(h);
    else
        CHECK(CommDlgExtendedError() == CDERR_DIALOGFAILURE);
}

int main(void) {
    /* GetFileTitleW. */
    wchar_t title[MAX_PATH];
    CHECK(GetFileTitleW(L"C:\\dir\\readme", NULL, 0) == 7);
    CHECK(GetFileTitleW(L"C:\\dir\\readme", title, MAX_PATH) == 0 && wcscmp(title, L"readme") == 0);
    CHECK(GetFileTitleW(L"C:\\dir\\readme", title, 3) == 7);
    CHECK(GetFileTitleW(L"notes", title, MAX_PATH) == 0 && wcscmp(title, L"notes") == 0);
    CHECK(GetFileTitleW(L"C:\\dir\\file.txt", title, MAX_PATH) == 0);
    CHECK(wcscmp(title, L"file.txt") == 0 || wcscmp(title, L"file") == 0);

    /* PrintDlgW. */
    CHECK(!PrintDlgW(NULL) && CommDlgExtendedError() == CDERR_INITIALIZATION);
    PRINTDLGW pd;
    memset(&pd, 0, sizeof pd);
    pd.lStructSize = sizeof pd - 1;
    CHECK(!PrintDlgW(&pd) && CommDlgExtendedError() == CDERR_STRUCTSIZE);
    memset(&pd, 0, sizeof pd);
    pd.lStructSize = sizeof pd;
    pd.Flags = PD_RETURNDEFAULT;
    BOOL got = PrintDlgW(&pd);
    CHECK(got || CommDlgExtendedError() == PDERR_NODEFAULTPRN);
    if (got) {
        GlobalFree(pd.hDevMode);
        GlobalFree(pd.hDevNames);
    }

    /* FindTextW and ReplaceTextW. */
    WNDCLASSW wc;
    memset(&wc, 0, sizeof wc);
    wc.lpfnWndProc = DefWindowProcW;
    wc.lpszClassName = L"comdlg32_test";
    CHECK(RegisterClassW(&wc) != 0);
    find_fn both[2] = {FindTextW, ReplaceTextW};
    wchar_t what[64] = L"abc", with[64] = L"def";
    FINDREPLACEW fr;
    for (int i = 0; i < 2; i++)
        fails(both[i], NULL, CDERR_INITIALIZATION);
    memset(&fr, 0, sizeof fr);
    for (int i = 0; i < 2; i++)
        fails(both[i], &fr, CDERR_STRUCTSIZE);
    fr.lStructSize = sizeof fr;
    for (int i = 0; i < 2; i++)
        fails(both[i], &fr, CDERR_DIALOGFAILURE);
    fr.hwndOwner = CreateWindowW(wc.lpszClassName, NULL, WS_VISIBLE, 0, 0, 200, 100, NULL, NULL,
                                 GetModuleHandleW(NULL), NULL);
    CHECK(fr.hwndOwner != NULL);
    for (int i = 0; i < 2; i++)
        fails(both[i], &fr, FRERR_BUFFERLENGTHZERO);
    fr.wFindWhatLen = 64;
    for (int i = 0; i < 2; i++)
        fails(both[i], &fr, FRERR_BUFFERLENGTHZERO);
    fr.lpstrFindWhat = what;
    works(FindTextW, &fr);
    fails(ReplaceTextW, &fr, FRERR_BUFFERLENGTHZERO);
    fr.lpstrReplaceWith = with;
    works(FindTextW, &fr);
    works(ReplaceTextW, &fr);
    fr.wReplaceWithLen = 64;
    fr.Flags = FR_ENABLEHOOK;
    for (int i = 0; i < 2; i++)
        fails(both[i], &fr, CDERR_NOHOOK);
    fr.Flags = FR_ENABLETEMPLATE;
    for (int i = 0; i < 2; i++)
        fails(both[i], &fr, CDERR_FINDRESFAILURE);
    fr.Flags = FR_ENABLETEMPLATEHANDLE;
    for (int i = 0; i < 2; i++)
        fails(both[i], &fr, CDERR_NOHINSTANCE);
    fr.Flags = 0;
    works(FindTextW, &fr);
    works(ReplaceTextW, &fr);
    DestroyWindow(fr.hwndOwner);

    /* ChooseColorW with a template the module lacks. */
    static COLORREF custom[16];
    CHOOSECOLORW cc;
    memset(&cc, 0, sizeof cc);
    cc.lStructSize = sizeof cc;
    cc.hInstance = (HWND)GetModuleHandleW(NULL);
    cc.lpCustColors = custom;
    cc.Flags = CC_ENABLETEMPLATE;
    cc.lpTemplateName = L"NO_SUCH_TEMPLATE";
    CHECK(!ChooseColorW(&cc) && CommDlgExtendedError() == CDERR_FINDRESFAILURE);

    printf("comdlg32_test: ok\n");
    return 0;
}
