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
 * The test retries its first request while Nginx starts.
 */
#include <windows.h>
#include <wininet.h>
#include <stdio.h>
#include <string.h>

static int check;
#define CHECK(cond) do { check++; if (!(cond)) { printf("wininet_test: check %d (line %d) failed, error %lu\n", check, __LINE__, GetLastError()); fflush(stdout); ExitProcess(check); } } while (0)

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
    /* 3. */
    status = get(s, L"localhost", 8443, L"/", INTERNET_FLAG_SECURE, body, sizeof body, &error);
    CHECK(error == 0 && status == 200 && strstr(body, "Welcome to nginx!") != NULL);
    /* 4. */
    status = get(s, L"127.0.0.1", 8080, L"/nope", 0, body, sizeof body, &error);
    CHECK(error == 0 && status == 404);
    /* 5. */
    status = get(s, L"localhost", 8444, L"/", INTERNET_FLAG_SECURE, body, sizeof body, &error);
    CHECK(error == ERROR_INTERNET_INVALID_CA);
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
    CHECK(InternetCloseHandle(s));
    printf("wininet_test: ok\n");
    return 0;
}
