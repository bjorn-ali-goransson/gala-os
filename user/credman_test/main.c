/* CredEnumerateW as Windows answers it, measured on Windows 11 24H2, build
 * 26100 (learn.microsoft.com "CredEnumerateW", "CREDENTIALW"; the struct is
 * the SDK's wincred.h). Office's shared runtime imports it from
 * ADVAPI32.dll as a *delay load*, so a gala that lacks it does not get a
 * probe's NULL back -- the delay-load helper raises 0xc06d007f and the
 * process dies (ROADMAP OF).
 *
 * What the machine's own credential set holds is not something a test can
 * know, so this test asserts only what no machine changes -- how the
 * arguments are validated and what a search that finds nothing answers --
 * and prints the rest. It never prints a credential's name or contents.
 *
 * 1. A filter that matches nothing is FALSE with ERROR_NOT_FOUND (1168).
 * 2. Both out parameters are written even then: Count becomes 0 and
 *    Credentials becomes NULL, which sentinels in each say (the function
 *    writes them, it does not leave them). It does not validate them
 *    either -- passing NULL for Count faults inside advapi32 rather than
 *    returning ERROR_INVALID_PARAMETER -- so a caller's bad pointer is a
 *    caller's problem, and that is not asserted here because a test that
 *    asserts it is a test that crashes.
 * 3. CRED_ENUMERATE_ALL_CREDENTIALS (0x1) wants the filter to be NULL and
 *    nothing else: with one it is FALSE with ERROR_INVALID_FLAGS (1004),
 *    and with NULL it succeeds for an ordinary process -- no privilege is
 *    asked for, and it returns more than the filter "*" does (12 against
 *    9 on the machine measured).
 * 4. Any other flag bit is FALSE with ERROR_INVALID_PARAMETER (87) -- 2,
 *    3 and 0xffffffff all are, so an unknown bit beats the known one.
 * 5. An empty filter is not a missing one: "" is ERROR_NOT_FOUND where
 *    NULL enumerates. (A caller that cannot tell them apart measures the
 *    wrong function: a .NET interop layer that turns a null string into
 *    "" makes a NULL-filter enumeration look like ERROR_NOT_FOUND and
 *    CRED_ENUMERATE_ALL_CREDENTIALS look like ERROR_INVALID_FLAGS.)
 * 6. Printed, not asserted, because they are the machine's own state: how
 *    many credentials a NULL filter, the filter "*" and
 *    CRED_ENUMERATE_ALL_CREDENTIALS answer with. On a machine with no
 *    credentials at all (gala today) each is ERROR_NOT_FOUND. Where a set
 *    does come back its shape is checked: every entry present, every
 *    TargetName there and not empty, every Type and Persist one of the
 *    documented ones, and CredFree takes the lot in one call.
 */

#define _WIN32_WINNT 0x0A00
#include <windows.h>
#include <wincred.h>
#include <stdio.h>
#include <string.h>

/* mingw-w64's wincred.h does not carry this one (learn.microsoft.com,
 * "CredEnumerateW": CRED_ENUMERATE_ALL_CREDENTIALS = 0x1). */
#ifndef CRED_ENUMERATE_ALL_CREDENTIALS
#define CRED_ENUMERATE_ALL_CREDENTIALS 0x1
#endif

#ifndef NAME
#define NAME "credman_test"
#endif

static int check;
#define CHECK(cond) do { check++; if (!(cond)) { printf(NAME ": check %d (line %d) failed, error %lu\n", check, __LINE__, GetLastError()); fflush(stdout); ExitProcess(check); } } while (0)

/* Written into both out parameters before every call: what is still there
 * afterwards is what the function did not write (2). */
#define SENTINEL_COUNT 0x5a5a5a5au
#define SENTINEL_PTR ((PCREDENTIALW *)(ULONG_PTR)0x5a5a5a5a)

/* A call that is expected to fail: the error, and that Count and
 * Credentials were both written anyway. */
static void refused(const char *what, const wchar_t *filter, DWORD flags, DWORD want_err) {
    DWORD count = SENTINEL_COUNT;
    PCREDENTIALW *creds = SENTINEL_PTR;
    SetLastError(0);
    BOOL ok = CredEnumerateW(filter, flags, &count, &creds);
    DWORD err = GetLastError();
    printf(NAME ": %s: ok %d, error %lu, count %lu, creds %s\n", what, (int)ok, err,
           count, creds == NULL ? "NULL" : (creds == SENTINEL_PTR ? "untouched" : "set"));
    CHECK(!ok);
    CHECK(err == want_err);
    CHECK(count == 0);
    CHECK(creds == NULL);
}

int main(void) {
    /* 1 and 2. */
    refused("filter matching nothing", L"gala_no_such_credential_*", 0, ERROR_NOT_FOUND);

    /* 3. The flag wants a NULL filter; with one it is the flag that is
     * wrong, not the filter. */
    refused("CRED_ENUMERATE_ALL_CREDENTIALS, with a filter", L"x", CRED_ENUMERATE_ALL_CREDENTIALS,
            ERROR_INVALID_FLAGS);
    refused("CRED_ENUMERATE_ALL_CREDENTIALS, filter '*'", L"*", CRED_ENUMERATE_ALL_CREDENTIALS,
            ERROR_INVALID_FLAGS);

    /* 4. */
    refused("flags 2", NULL, 2, ERROR_INVALID_PARAMETER);
    refused("flags 3 (an unknown bit beside the known one)", NULL, 3, ERROR_INVALID_PARAMETER);
    refused("flags 0xffffffff", NULL, 0xffffffff, ERROR_INVALID_PARAMETER);

    /* A filter with a star in the middle or at the front matches nothing
     * rather than being rejected: the grammar is not validated. */
    refused("filter 'a*b'", L"a*b", 0, ERROR_NOT_FOUND);
    refused("filter '*a'", L"*a", 0, ERROR_NOT_FOUND);

    /* 5. An empty filter is not a missing one. */
    refused("filter '' (empty)", L"", 0, ERROR_NOT_FOUND);

    /* 6. The machine's own state: printed, and its shape checked where
     * there is one. Nothing of a credential itself is printed. */
    const wchar_t *filters[3] = { NULL, L"*", NULL };
    const DWORD eflags[3] = { 0, 0, CRED_ENUMERATE_ALL_CREDENTIALS };
    const char *names[3] = { "filter NULL", "filter '*'", "CRED_ENUMERATE_ALL_CREDENTIALS" };
    for (int i = 0; i < 3; i++) {
        DWORD count = SENTINEL_COUNT;
        PCREDENTIALW *creds = SENTINEL_PTR;
        SetLastError(0);
        BOOL ok = CredEnumerateW(filters[i], eflags[i], &count, &creds);
        DWORD err = GetLastError();
        printf(NAME ": %s: ok %d, error %lu, %lu credentials\n", names[i], (int)ok, err,
               ok ? count : 0);
        if (!ok) {
            CHECK(err == ERROR_NOT_FOUND);
            CHECK(count == 0);
            CHECK(creds == NULL);
            continue;
        }
        CHECK(count > 0);
        CHECK(creds != NULL && creds != SENTINEL_PTR);
        for (DWORD j = 0; j < count; j++) {
            CHECK(creds[j] != NULL);
            CHECK(creds[j]->TargetName != NULL);
            CHECK(creds[j]->TargetName[0] != 0);
            CHECK(creds[j]->Type >= CRED_TYPE_GENERIC && creds[j]->Type <= CRED_TYPE_MAXIMUM);
            CHECK(creds[j]->Persist >= CRED_PERSIST_SESSION &&
                  creds[j]->Persist <= CRED_PERSIST_ENTERPRISE);
        }
        /* One call frees the array and everything in it. */
        CredFree(creds);
    }

    printf(NAME ": ok\n");
    return 0;
}
