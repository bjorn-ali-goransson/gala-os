/* WinINet's HTTP functions as Windows documents them (learn.microsoft.com,
 * "WinINet functions", "HttpQueryInfo", "Query Info Flags", "WinINet
 * errors"), against Nginx in the guest (user/schannel_test/nginx.conf:
 * plain HTTP on 127.0.0.1:8080, TLS on 8443 with a certificate for
 * localhost from the test CA, which the machine trusts, and on 8444 with
 * one from an untrusted issuer):
 *
 * 1. InternetCrackUrlW splits https://localhost:8443/a/b?c into its scheme
 *    (INTERNET_SCHEME_HTTPS), host, port, path and extra info; a buffer
 *    too small gets ERROR_INSUFFICIENT_BUFFER.
 * 2. GET / over HTTP (InternetOpenW with INTERNET_OPEN_TYPE_DIRECT,
 *    InternetConnectW with INTERNET_SERVICE_HTTP, HttpOpenRequestW with
 *    INTERNET_FLAG_RELOAD, HttpSendRequestW): HttpQueryInfoW gives the
 *    status (200, a number with HTTP_QUERY_FLAG_NUMBER), Content-Length
 *    and, with HTTP_QUERY_CUSTOM (the name in the buffer), Server;
 *    InternetQueryDataAvailable and InternetReadFile (until it reads 0
 *    bytes) read the body, as long as Content-Length, holding "Welcome to
 *    nginx!".
 * 3. The same over HTTPS (INTERNET_FLAG_SECURE) to localhost:8443.
 * 4. GET /nope: status 404, status text "Not Found".
 * 5. HTTPS to 8444 (untrusted issuer): HttpSendRequestW fails with
 *    ERROR_INTERNET_INVALID_CA.
 * 6. A port nobody listens on (8090): HttpSendRequestW fails with
 *    ERROR_INTERNET_CANNOT_CONNECT.
 * 7. InternetOpenUrlW: GET http://127.0.0.1:8080/ in one call; the
 *    handle's status and body as in 2.
 * 8. A redirect (/redirect-rel: 302 to "/") is followed: 200 and the
 *    start page; with INTERNET_FLAG_NO_AUTO_REDIRECT the 302 itself.
 * 9. The ANSI functions (ODT's setup.exe mixes them with the W ones):
 *    InternetCrackUrlA as in 1 (pointers into the caller's URL);
 *    InternetConnectA, HttpOpenRequestA, HttpAddRequestHeadersA and
 *    HttpQueryInfoA, whose text answers are ANSI: the length without the
 *    NUL, or ERROR_INSUFFICIENT_BUFFER with the bytes needed with it.
 * 10. InternetQueryOptionW (Wine's http.c test_user_pass_auth and
 *    internet.c test_InternetQueryOptionA, run on Windows): a string
 *    option too big for the buffer is ERROR_INSUFFICIENT_BUFFER with the
 *    bytes needed with the NUL; one that fits gives its length in
 *    characters without the NUL. INTERNET_OPTION_USER_AGENT of the
 *    session (on a connection: ERROR_INTERNET_INCORRECT_HANDLE_TYPE),
 *    INTERNET_OPTION_URL of a request, INTERNET_OPTION_HANDLE_TYPE.
 * 11. InternetSetStatusCallbackW returns the previous callback (NULL at
 *    first), INTERNET_INVALID_STATUS_CALLBACK for no handle; the children
 *    of a session get its callback; a synchronous request with a context
 *    reports HANDLE_CREATED for the connection and the request (the new
 *    handle's context, the handle as information), then SENDING_REQUEST,
 *    REQUEST_SENT, RECEIVING_RESPONSE and RESPONSE_RECEIVED (a DWORD of
 *    bytes) in that order (Wine's http.c InternetSetFilePointer_test);
 *    a handle whose context is 0 reports nothing.
 * 12. InternetGetCookieExW for a URL nothing has set a cookie for: FALSE,
 *    ERROR_NO_MORE_ITEMS; for about:blank ERROR_INVALID_PARAMETER (Wine's
 *    internet.c test_cookie_url and test_cookie_attrs).
 * The test retries its first request while Nginx starts. Steps 3 and 5,
 * which need the test CA trusted, run last (after 11, before 12) and print
 * the status and error they got, so a machine that cannot trust the CA
 * still runs the rest.
 */
#include <windows.h>
#include <wininet.h>
#include <stdio.h>
#include <string.h>

/* The program's name in its output (build.sh: wow64_wininet_test for x86). */
#ifndef NAME
#define NAME "wininet_test"
#endif

static int check;
#define CHECK(cond) do { check++; if (!(cond)) { printf(NAME ": check %d (line %d) failed, error %lu\n", check, __LINE__, GetLastError()); fflush(stdout); ExitProcess(check); } } while (0)

/* The status callback's record (11). */
#define MAX_EVENTS 64
static DWORD events[MAX_EVENTS];
static DWORD_PTR contexts[MAX_EVENTS];
static volatile LONG nevents;

static void CALLBACK status_callback(HINTERNET h, DWORD_PTR context, DWORD status, LPVOID info, DWORD len) {
    (void)h;
    LONG i = InterlockedIncrement(&nevents) - 1;
    if (i < MAX_EVENTS) {
        events[i] = status;
        contexts[i] = context;
    }
    switch (status) {
    case INTERNET_STATUS_HANDLE_CREATED:
    case INTERNET_STATUS_HANDLE_CLOSING:
        CHECK(len == sizeof(HINTERNET) && *(HINTERNET *)info != NULL);
        break;
    case INTERNET_STATUS_REQUEST_SENT:
    case INTERNET_STATUS_RESPONSE_RECEIVED:
        CHECK(len == sizeof(DWORD) && *(DWORD *)info > 0);
        break;
    }
}

/* The statuses `want` (with their contexts) appear in this order among
 * the recorded ones, others between them allowed. */
static int in_order(const DWORD *want, const DWORD_PTR *want_context, int n) {
    int k = 0;
    for (LONG i = 0; i < nevents && i < MAX_EVENTS && k < n; i++)
        if (events[i] == want[k] && contexts[i] == want_context[k])
            k++;
    return k == n;
}

/* The status, Content-Length check and body (NUL-ended) of a sent request. */
static DWORD response(HINTERNET r, char *buf, DWORD size, int use_available) {
    DWORD status = 0, len = sizeof status;
    CHECK(HttpQueryInfoW(r, HTTP_QUERY_STATUS_CODE | HTTP_QUERY_FLAG_NUMBER, &status, &len, NULL));
    CHECK(len == sizeof status);
    DWORD total = 0;
    for (;;) {
        DWORD want = 512;
        if (use_available) {
            DWORD avail = 0;
            CHECK(InternetQueryDataAvailable(r, &avail, 0, 0));
            if (avail == 0)
                break;
            want = avail;
        }
        if (want > size - 1 - total)
            want = size - 1 - total;
        CHECK(want > 0);
        DWORD got = 0;
        CHECK(InternetReadFile(r, buf + total, want, &got));
        if (got == 0) {
            CHECK(!use_available);
            break;
        }
        total += got;
    }
    buf[total] = 0;
    /* At the end, InternetReadFile keeps succeeding with 0 bytes. */
    DWORD got = 1;
    char extra[16];
    CHECK(InternetReadFile(r, extra, sizeof extra, &got) && got == 0);
    DWORD length = 0;
    len = sizeof length;
    if (HttpQueryInfoW(r, HTTP_QUERY_CONTENT_LENGTH | HTTP_QUERY_FLAG_NUMBER, &length, &len, NULL))
        CHECK(length == total);
    else
        CHECK(GetLastError() == ERROR_HTTP_HEADER_NOT_FOUND);
    wchar_t text[64];
    if (status == 404) {
        len = sizeof text;
        CHECK(HttpQueryInfoW(r, HTTP_QUERY_STATUS_TEXT, text, &len, NULL));
        CHECK(wcscmp(text, L"Not Found") == 0 && len == 9 * sizeof(wchar_t));
    }
    /* HTTP_QUERY_CUSTOM: the header's name in the buffer. */
    wcscpy(text, L"Server");
    len = sizeof text;
    CHECK(HttpQueryInfoW(r, HTTP_QUERY_CUSTOM, text, &len, NULL));
    CHECK(wcsncmp(text, L"nginx", 5) == 0);
    /* Too small: ERROR_INSUFFICIENT_BUFFER with the bytes needed (and the NUL). */
    len = 2;
    CHECK(!HttpQueryInfoW(r, HTTP_QUERY_STATUS_CODE, text, &len, NULL));
    CHECK(GetLastError() == ERROR_INSUFFICIENT_BUFFER && len == 4 * sizeof(wchar_t));
    len = sizeof text;
    CHECK(!HttpQueryInfoW(r, HTTP_QUERY_REFRESH, text, &len, NULL));
    CHECK(GetLastError() == ERROR_HTTP_HEADER_NOT_FOUND);
    return status;
}

/* GET `path` from host:port; the status, and the body into buf. */
static DWORD get(HINTERNET session, const wchar_t *host, INTERNET_PORT port, const wchar_t *path, DWORD flags,
                 char *buf, DWORD size, DWORD *error) {
    *error = 0;
    HINTERNET c = InternetConnectW(session, host, port, NULL, NULL, INTERNET_SERVICE_HTTP, 0, 0);
    CHECK(c != NULL);
    HINTERNET r = HttpOpenRequestW(c, L"GET", path, NULL, NULL, NULL, flags | INTERNET_FLAG_RELOAD, 0);
    CHECK(r != NULL);
    CHECK(HttpAddRequestHeadersW(r, L"X-Gala-Test: wininet\r\n", (DWORD)-1, HTTP_ADDREQ_FLAG_ADD));
    DWORD status = 0;
    if (!HttpSendRequestW(r, NULL, 0, NULL, 0))
        *error = GetLastError();
    else
        status = response(r, buf, size, 1);
    CHECK(InternetCloseHandle(r));
    CHECK(InternetCloseHandle(c));
    return status;
}

int main(void) {
    static char body[65536];
    DWORD error;
    /* 1. */
    URL_COMPONENTSW u = {0};
    u.dwStructSize = sizeof u;
    u.dwSchemeLength = u.dwHostNameLength = u.dwUrlPathLength = u.dwExtraInfoLength = (DWORD)-1;
    CHECK(InternetCrackUrlW(L"https://localhost:8443/a/b?c", 0, 0, &u));
    CHECK(u.nScheme == INTERNET_SCHEME_HTTPS && u.nPort == 8443);
    CHECK(u.dwSchemeLength == 5 && wcsncmp(u.lpszScheme, L"https", 5) == 0);
    CHECK(u.dwHostNameLength == 9 && wcsncmp(u.lpszHostName, L"localhost", 9) == 0);
    CHECK(u.dwUrlPathLength == 4 && wcsncmp(u.lpszUrlPath, L"/a/b", 4) == 0);
    CHECK(u.dwExtraInfoLength == 2 && wcsncmp(u.lpszExtraInfo, L"?c", 2) == 0);
    wchar_t hostbuf[16];
    URL_COMPONENTSW v = {0};
    v.dwStructSize = sizeof v;
    v.lpszHostName = hostbuf;
    v.dwHostNameLength = 4;
    CHECK(!InternetCrackUrlW(L"http://127.0.0.1/", 0, 0, &v));
    CHECK(GetLastError() == ERROR_INSUFFICIENT_BUFFER);
    v.dwHostNameLength = sizeof hostbuf / sizeof hostbuf[0];
    CHECK(InternetCrackUrlW(L"http://127.0.0.1/", 0, 0, &v));
    CHECK(v.nScheme == INTERNET_SCHEME_HTTP && v.nPort == INTERNET_DEFAULT_HTTP_PORT);
    CHECK(v.dwHostNameLength == 9 && wcscmp(hostbuf, L"127.0.0.1") == 0);

    HINTERNET s = InternetOpenW(L"gala wininet_test", INTERNET_OPEN_TYPE_DIRECT, NULL, NULL, 0);
    CHECK(s != NULL);
    /* 2, retried while Nginx starts. */
    DWORD status = 0;
    for (int i = 0; i < 100; i++) {
        status = get(s, L"127.0.0.1", 8080, L"/", 0, body, sizeof body, &error);
        if (error != ERROR_INTERNET_CANNOT_CONNECT)
            break;
        Sleep(200);
    }
    CHECK(error == 0 && status == 200 && strstr(body, "Welcome to nginx!") != NULL);
    /* 4. */
    status = get(s, L"127.0.0.1", 8080, L"/nope", 0, body, sizeof body, &error);
    CHECK(error == 0 && status == 404);
    /* 6. */
    status = get(s, L"127.0.0.1", 8090, L"/", 0, body, sizeof body, &error);
    CHECK(error == ERROR_INTERNET_CANNOT_CONNECT);
    /* 7. */
    HINTERNET r = InternetOpenUrlW(s, L"http://127.0.0.1:8080/", NULL, 0, INTERNET_FLAG_RELOAD, 0);
    CHECK(r != NULL);
    status = response(r, body, sizeof body, 0);
    CHECK(status == 200 && strstr(body, "Welcome to nginx!") != NULL);
    CHECK(InternetCloseHandle(r));
    /* 8. */
    r = InternetOpenUrlW(s, L"http://127.0.0.1:8080/redirect-rel", NULL, 0, INTERNET_FLAG_RELOAD, 0);
    CHECK(r != NULL);
    status = response(r, body, sizeof body, 0);
    CHECK(status == 200 && strstr(body, "Welcome to nginx!") != NULL);
    CHECK(InternetCloseHandle(r));
    r = InternetOpenUrlW(s, L"http://127.0.0.1:8080/redirect-rel", NULL, 0,
                         INTERNET_FLAG_RELOAD | INTERNET_FLAG_NO_AUTO_REDIRECT, 0);
    CHECK(r != NULL);
    status = response(r, body, sizeof body, 0);
    CHECK(status == 302);
    CHECK(InternetCloseHandle(r));

    /* 9. */
    URL_COMPONENTSA ua = {0};
    static char url_a[] = "https://localhost:8443/a/b?c";
    ua.dwStructSize = sizeof ua;
    ua.dwSchemeLength = ua.dwHostNameLength = ua.dwUrlPathLength = ua.dwExtraInfoLength = (DWORD)-1;
    CHECK(InternetCrackUrlA(url_a, 0, 0, &ua));
    CHECK(ua.nScheme == INTERNET_SCHEME_HTTPS && ua.nPort == 8443);
    CHECK(ua.lpszScheme == url_a && ua.dwSchemeLength == 5);
    CHECK(ua.lpszHostName == url_a + 8 && ua.dwHostNameLength == 9);
    CHECK(ua.lpszUrlPath == url_a + 22 && ua.dwUrlPathLength == 4);
    CHECK(ua.lpszExtraInfo == url_a + 26 && ua.dwExtraInfoLength == 2);
    char hosta[16];
    URL_COMPONENTSA va = {0};
    va.dwStructSize = sizeof va;
    va.lpszHostName = hosta;
    va.dwHostNameLength = 4;
    CHECK(!InternetCrackUrlA("http://127.0.0.1/", 0, 0, &va));
    CHECK(GetLastError() == ERROR_INSUFFICIENT_BUFFER);
    va.dwHostNameLength = sizeof hosta;
    CHECK(InternetCrackUrlA("http://127.0.0.1/", 0, 0, &va));
    CHECK(va.nScheme == INTERNET_SCHEME_HTTP && va.dwHostNameLength == 9 && strcmp(hosta, "127.0.0.1") == 0);
    HINTERNET c = InternetConnectA(s, "127.0.0.1", 8080, NULL, NULL, INTERNET_SERVICE_HTTP, 0, 0);
    CHECK(c != NULL);
    r = HttpOpenRequestA(c, "GET", "/nope", NULL, NULL, NULL, INTERNET_FLAG_RELOAD, 0);
    CHECK(r != NULL);
    CHECK(HttpAddRequestHeadersA(r, "X-Gala-Test: wininet-a\r\n", (DWORD)-1, HTTP_ADDREQ_FLAG_ADD));
    CHECK(HttpSendRequestW(r, NULL, 0, NULL, 0));
    char texta[64];
    DWORD len = sizeof texta;
    CHECK(HttpQueryInfoA(r, HTTP_QUERY_STATUS_CODE, texta, &len, NULL));
    CHECK(strcmp(texta, "404") == 0 && len == 3);
    len = 3;
    CHECK(!HttpQueryInfoA(r, HTTP_QUERY_STATUS_TEXT, texta, &len, NULL));
    CHECK(GetLastError() == ERROR_INSUFFICIENT_BUFFER && len == 10);
    len = sizeof texta;
    CHECK(HttpQueryInfoA(r, HTTP_QUERY_STATUS_TEXT, texta, &len, NULL));
    CHECK(strcmp(texta, "Not Found") == 0 && len == 9);
    DWORD number = 0;
    len = sizeof number;
    CHECK(HttpQueryInfoA(r, HTTP_QUERY_STATUS_CODE | HTTP_QUERY_FLAG_NUMBER, &number, &len, NULL));
    CHECK(number == 404 && len == sizeof number);
    strcpy(texta, "Server");
    len = sizeof texta;
    CHECK(HttpQueryInfoA(r, HTTP_QUERY_CUSTOM, texta, &len, NULL));
    CHECK(strncmp(texta, "nginx", 5) == 0 && len == strlen(texta));
    strcpy(texta, "X-Gala-Test");
    len = sizeof texta;
    CHECK(HttpQueryInfoA(r, HTTP_QUERY_CUSTOM | HTTP_QUERY_FLAG_REQUEST_HEADERS, texta, &len, NULL));
    CHECK(strcmp(texta, "wininet-a") == 0 && len == 9);
    CHECK(InternetCloseHandle(r));

    /* 10. */
    static const wchar_t agent[] = L"gala wininet_test";
    wchar_t option[64];
    DWORD size = 0;
    CHECK(!InternetQueryOptionW(s, INTERNET_OPTION_USER_AGENT, option, &size));
    CHECK(GetLastError() == ERROR_INSUFFICIENT_BUFFER && size == (wcslen(agent) + 1) * sizeof(wchar_t));
    size = sizeof option;
    CHECK(InternetQueryOptionW(s, INTERNET_OPTION_USER_AGENT, option, &size));
    CHECK(wcscmp(option, agent) == 0 && size == wcslen(agent));
    size = sizeof option;
    CHECK(!InternetQueryOptionW(c, INTERNET_OPTION_USER_AGENT, option, &size));
    CHECK(GetLastError() == ERROR_INTERNET_INCORRECT_HANDLE_TYPE);
    DWORD type = 0;
    size = sizeof type;
    CHECK(InternetQueryOptionW(s, INTERNET_OPTION_HANDLE_TYPE, &type, &size));
    CHECK(type == INTERNET_HANDLE_TYPE_INTERNET && size == sizeof type);
    size = sizeof type;
    CHECK(InternetQueryOptionW(c, INTERNET_OPTION_HANDLE_TYPE, &type, &size));
    CHECK(type == INTERNET_HANDLE_TYPE_CONNECT_HTTP);
    r = HttpOpenRequestW(c, L"GET", L"/a?b", NULL, NULL, NULL, INTERNET_FLAG_RELOAD, 0);
    CHECK(r != NULL);
    size = sizeof type;
    CHECK(InternetQueryOptionW(r, INTERNET_OPTION_HANDLE_TYPE, &type, &size));
    CHECK(type == INTERNET_HANDLE_TYPE_HTTP_REQUEST);
    static const wchar_t url[] = L"http://127.0.0.1:8080/a?b";
    size = 0;
    CHECK(!InternetQueryOptionW(r, INTERNET_OPTION_URL, option, &size));
    CHECK(GetLastError() == ERROR_INSUFFICIENT_BUFFER && size == (wcslen(url) + 1) * sizeof(wchar_t));
    size = sizeof option;
    CHECK(InternetQueryOptionW(r, INTERNET_OPTION_URL, option, &size));
    CHECK(wcscmp(option, url) == 0 && size == wcslen(url));
    CHECK(InternetCloseHandle(r));
    CHECK(InternetCloseHandle(c));

    /* 11. */
    CHECK(InternetSetStatusCallbackW(s, status_callback) == NULL);
    CHECK(InternetSetStatusCallbackW(s, status_callback) == status_callback);
    CHECK(InternetSetStatusCallbackW(NULL, status_callback) == INTERNET_INVALID_STATUS_CALLBACK);
    nevents = 0;
    c = InternetConnectW(s, L"127.0.0.1", 8080, NULL, NULL, INTERNET_SERVICE_HTTP, 0, 0x11);
    CHECK(c != NULL);
    r = HttpOpenRequestW(c, L"GET", L"/", NULL, NULL, NULL, INTERNET_FLAG_RELOAD, 0x22);
    CHECK(r != NULL);
    CHECK(HttpSendRequestW(r, NULL, 0, NULL, 0));
    status = response(r, body, sizeof body, 1);
    CHECK(status == 200);
    static const DWORD want[] = {
        INTERNET_STATUS_HANDLE_CREATED, INTERNET_STATUS_HANDLE_CREATED, INTERNET_STATUS_SENDING_REQUEST,
        INTERNET_STATUS_REQUEST_SENT, INTERNET_STATUS_RECEIVING_RESPONSE, INTERNET_STATUS_RESPONSE_RECEIVED,
    };
    static const DWORD_PTR want_context[] = { 0x11, 0x22, 0x22, 0x22, 0x22, 0x22 };
    CHECK(in_order(want, want_context, 6));
    CHECK(InternetCloseHandle(r));
    CHECK(InternetCloseHandle(c));
    /* A context of 0: no notifications. */
    LONG before = nevents;
    c = InternetConnectW(s, L"127.0.0.1", 8080, NULL, NULL, INTERNET_SERVICE_HTTP, 0, 0);
    CHECK(c != NULL);
    r = HttpOpenRequestW(c, L"GET", L"/", NULL, NULL, NULL, INTERNET_FLAG_RELOAD, 0);
    CHECK(r != NULL);
    CHECK(HttpSendRequestW(r, NULL, 0, NULL, 0));
    for (LONG i = before; i < nevents && i < MAX_EVENTS; i++)
        CHECK(events[i] == INTERNET_STATUS_HANDLE_CLOSING);
    CHECK(InternetCloseHandle(r));
    CHECK(InternetCloseHandle(c));
    CHECK(InternetSetStatusCallbackW(s, NULL) == status_callback);
    /* 3 and 5, the steps that need the test CA trusted, last: a machine
       that cannot trust it still runs the others. Each prints what it got. */
    status = get(s, L"localhost", 8443, L"/", INTERNET_FLAG_SECURE, body, sizeof body, &error);
    printf(NAME ": step 3: status %lu, error %lu\n", status, error);
    CHECK(error == 0 && status == 200 && strstr(body, "Welcome to nginx!") != NULL);
    status = get(s, L"localhost", 8444, L"/", INTERNET_FLAG_SECURE, body, sizeof body, &error);
    printf(NAME ": step 5: status %lu, error %lu\n", status, error);
    CHECK(error == ERROR_INTERNET_INVALID_CA);
    CHECK(InternetCloseHandle(s));

    /* 12. */
    wchar_t cookie[256];
    len = sizeof cookie / sizeof cookie[0];
    CHECK(!InternetGetCookieExW(L"http://cookie.gala.test/", NULL, cookie, &len, 0, NULL));
    CHECK(GetLastError() == ERROR_NO_MORE_ITEMS);
    len = sizeof cookie / sizeof cookie[0];
    CHECK(!InternetGetCookieExW(L"about:blank", NULL, cookie, &len, 0, NULL));
    CHECK(GetLastError() == ERROR_INVALID_PARAMETER);
    printf(NAME ": ok\n");
    return 0;
}
