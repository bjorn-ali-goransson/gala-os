/* WinHTTP's synchronous client as Windows documents it, against Nginx in
 * the guest (user/schannel_test/nginx.conf: plain HTTP on 127.0.0.1:8080,
 * TLS on 8443 with a certificate for localhost from the test CA, which the
 * machine trusts, and on 8444 with one from an untrusted issuer):
 *
 * 1. WinHttpCrackUrl splits https://localhost:8443/a/b?c into its scheme
 *    (INTERNET_SCHEME_HTTPS), host, port, path and extra info.
 * 2. GET / over HTTP: WinHttpSendRequest, WinHttpReceiveResponse;
 *    WinHttpQueryHeaders gives the status (200, as a number with
 *    WINHTTP_QUERY_FLAG_NUMBER) and Content-Length; QueryDataAvailable and
 *    ReadData read the body, as long as Content-Length, holding
 *    "Welcome to nginx!".
 * 3. The same over HTTPS (WINHTTP_FLAG_SECURE) to localhost:8443.
 * 4. GET /nope: status 404, status text "Not Found".
 * 5. HTTPS to 8444 (untrusted issuer): WinHttpSendRequest fails with
 *    ERROR_WINHTTP_SECURE_FAILURE.
 * 6. A port nobody listens on (8090): WinHttpSendRequest fails with
 *    ERROR_WINHTTP_CANNOT_CONNECT.
 * The test retries its first request while Nginx starts.
 */
#include <windows.h>
#include <winhttp.h>
#include <stdio.h>
#include <string.h>

static int check;
#define CHECK(cond) do { check++; if (!(cond)) { printf("winhttp_test: check %d (line %d) failed, error %lu\n", check, __LINE__, GetLastError()); fflush(stdout); ExitProcess(check); } } while (0)

/* GET `path` from host:port; the status, and the body into buf (NUL-ended). */
static DWORD get(HINTERNET session, const wchar_t *host, INTERNET_PORT port, const wchar_t *path, DWORD flags,
                 char *buf, DWORD size, DWORD *error) {
    *error = 0;
    HINTERNET c = WinHttpConnect(session, host, port, 0);
    CHECK(c != NULL);
    HINTERNET r = WinHttpOpenRequest(c, L"GET", path, NULL, WINHTTP_NO_REFERER, WINHTTP_DEFAULT_ACCEPT_TYPES, flags);
    CHECK(r != NULL);
    DWORD status = 0;
    if (!WinHttpSendRequest(r, WINHTTP_NO_ADDITIONAL_HEADERS, 0, WINHTTP_NO_REQUEST_DATA, 0, 0, 0)) {
        *error = GetLastError();
    } else {
        CHECK(WinHttpReceiveResponse(r, NULL));
        DWORD len = sizeof status;
        CHECK(WinHttpQueryHeaders(r, WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER, WINHTTP_HEADER_NAME_BY_INDEX,
                                  &status, &len, WINHTTP_NO_HEADER_INDEX));
        DWORD total = 0, avail = 0;
        for (;;) {
            CHECK(WinHttpQueryDataAvailable(r, &avail));
            if (avail == 0)
                break;
            DWORD got = 0;
            if (avail > size - 1 - total)
                avail = size - 1 - total;
            CHECK(avail > 0 && WinHttpReadData(r, buf + total, avail, &got) && got > 0);
            total += got;
        }
        buf[total] = 0;
        DWORD length = 0;
        len = sizeof length;
        if (WinHttpQueryHeaders(r, WINHTTP_QUERY_CONTENT_LENGTH | WINHTTP_QUERY_FLAG_NUMBER,
                                WINHTTP_HEADER_NAME_BY_INDEX, &length, &len, WINHTTP_NO_HEADER_INDEX))
            CHECK(length == total);
        if (status == 404) {
            wchar_t text[64];
            len = sizeof text;
            CHECK(WinHttpQueryHeaders(r, WINHTTP_QUERY_STATUS_TEXT, WINHTTP_HEADER_NAME_BY_INDEX, text, &len,
                                      WINHTTP_NO_HEADER_INDEX));
            CHECK(wcscmp(text, L"Not Found") == 0);
        }
    }
    CHECK(WinHttpCloseHandle(r));
    CHECK(WinHttpCloseHandle(c));
    return status;
}

int main(void) {
    static char body[65536];
    DWORD error;
    /* 1. */
    URL_COMPONENTS u = {0};
    u.dwStructSize = sizeof u;
    u.dwHostNameLength = u.dwUrlPathLength = u.dwExtraInfoLength = (DWORD)-1;
    CHECK(WinHttpCrackUrl(L"https://localhost:8443/a/b?c", 0, 0, &u));
    CHECK(u.nScheme == INTERNET_SCHEME_HTTPS && u.nPort == 8443);
    CHECK(u.dwHostNameLength == 9 && wcsncmp(u.lpszHostName, L"localhost", 9) == 0);
    CHECK(u.dwUrlPathLength == 4 && wcsncmp(u.lpszUrlPath, L"/a/b", 4) == 0);
    CHECK(u.dwExtraInfoLength == 2 && wcsncmp(u.lpszExtraInfo, L"?c", 2) == 0);

    HINTERNET s = WinHttpOpen(L"gala winhttp_test", WINHTTP_ACCESS_TYPE_NO_PROXY, WINHTTP_NO_PROXY_NAME,
                              WINHTTP_NO_PROXY_BYPASS, 0);
    CHECK(s != NULL);
    /* 2, retried while Nginx starts. */
    DWORD status = 0;
    for (int i = 0; i < 100; i++) {
        status = get(s, L"127.0.0.1", 8080, L"/", 0, body, sizeof body, &error);
        if (error != ERROR_WINHTTP_CANNOT_CONNECT)
            break;
        Sleep(200);
    }
    CHECK(error == 0 && status == 200 && strstr(body, "Welcome to nginx!") != NULL);
    /* 3. */
    status = get(s, L"localhost", 8443, L"/", WINHTTP_FLAG_SECURE, body, sizeof body, &error);
    CHECK(error == 0 && status == 200 && strstr(body, "Welcome to nginx!") != NULL);
    /* 4. */
    status = get(s, L"127.0.0.1", 8080, L"/nope", 0, body, sizeof body, &error);
    CHECK(error == 0 && status == 404);
    /* 5. */
    status = get(s, L"localhost", 8444, L"/", WINHTTP_FLAG_SECURE, body, sizeof body, &error);
    CHECK(error == ERROR_WINHTTP_SECURE_FAILURE);
    /* 6. */
    status = get(s, L"127.0.0.1", 8090, L"/", 0, body, sizeof body, &error);
    CHECK(error == ERROR_WINHTTP_CANNOT_CONNECT);
    CHECK(WinHttpCloseHandle(s));
    printf("winhttp_test: ok\n");
    return 0;
}
