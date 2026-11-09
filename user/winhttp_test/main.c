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
 * 7.-12. Automatic redirects (WINHTTP_OPTION_REDIRECT_POLICY, default
 *    WINHTTP_OPTION_REDIRECT_POLICY_DISALLOW_HTTPS_TO_HTTP; expected
 *    values from the documentation and Wine's conformance tests, which
 *    run on Windows: test_redirect, test_max_http_automatic_redirects,
 *    test_WinHttpQueryOption):
 * 7. GET /redirect-rel (302, Location "/"): WinHttpReceiveResponse
 *    follows it: 200 and the start page; WINHTTP_OPTION_URL is the
 *    request's URL before, the redirect's target after.
 * 8. The same with WINHTTP_OPTION_REDIRECT_POLICY_NEVER: the 302 itself,
 *    its Location header "/".
 * 9. http to https (/to-https, 301 to https://localhost:8443/): followed,
 *    the page comes over TLS.
 * 10. https to http (/to-http on 8443): the default policy refuses it,
 *    WinHttpReceiveResponse fails with ERROR_WINHTTP_REDIRECT_FAILED;
 *    with WINHTTP_OPTION_REDIRECT_POLICY_ALWAYS it is followed.
 * 11. A redirect loop with WINHTTP_OPTION_MAX_HTTP_AUTOMATIC_REDIRECTS 2:
 *    ERROR_WINHTTP_REDIRECT_FAILED once two were followed.
 * 12. A POST redirected by a 302 becomes a GET (200); by a 307 it stays a
 *    POST with its data (Nginx answers a POST to a static page with 405).
 * 13. The policy option: DISALLOW_HTTPS_TO_HTTP by default; a value of
 *    the wrong size is ERROR_INSUFFICIENT_BUFFER.
 * 14. Connection reuse (keep-alive: HTTP/1.1's persistent connections,
 *    which WinHTTP keeps for the next request to the same server once a
 *    response has been read to its end). Nginx's /conn answers its
 *    connection's serial number and how many requests it has carried:
 *    a second request on the same connect handle, and one on another
 *    connect handle of the session to the same server, go over the same
 *    connection (its count one higher each time), over HTTP and HTTPS;
 *    a request that says "Connection: close" ends its connection, so
 *    the next one is on a new connection.
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

/* A request with its redirect options (policy and max: -1 leaves the
 * default), data for a POST, and WINHTTP_OPTION_URL after the response
 * into `url`; the status, or 0 with the failing call's error. */
static DWORD request(HINTERNET session, const wchar_t *host, INTERNET_PORT port, const wchar_t *verb,
                     const wchar_t *path, DWORD flags, int policy, int max, const char *data, wchar_t *url,
                     DWORD url_size, DWORD *error) {
    *error = 0;
    HINTERNET c = WinHttpConnect(session, host, port, 0);
    CHECK(c != NULL);
    HINTERNET r = WinHttpOpenRequest(c, verb, path, NULL, WINHTTP_NO_REFERER, WINHTTP_DEFAULT_ACCEPT_TYPES, flags);
    CHECK(r != NULL);
    if (policy >= 0) {
        DWORD v = (DWORD)policy;
        CHECK(WinHttpSetOption(r, WINHTTP_OPTION_REDIRECT_POLICY, &v, sizeof v));
    }
    if (max >= 0) {
        DWORD v = (DWORD)max;
        CHECK(WinHttpSetOption(r, WINHTTP_OPTION_MAX_HTTP_AUTOMATIC_REDIRECTS, &v, sizeof v));
    }
    DWORD status = 0, len;
    DWORD data_len = data ? (DWORD)strlen(data) : 0;
    if (!WinHttpSendRequest(r, WINHTTP_NO_ADDITIONAL_HEADERS, 0, (void *)data, data_len, data_len, 0)) {
        *error = GetLastError();
    } else if (!WinHttpReceiveResponse(r, NULL)) {
        *error = GetLastError();
    } else {
        len = sizeof status;
        CHECK(WinHttpQueryHeaders(r, WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER, WINHTTP_HEADER_NAME_BY_INDEX,
                                  &status, &len, WINHTTP_NO_HEADER_INDEX));
    }
    url[0] = 0;
    len = url_size;
    CHECK(WinHttpQueryOption(r, WINHTTP_OPTION_URL, url, &len));
    CHECK(WinHttpCloseHandle(r));
    CHECK(WinHttpCloseHandle(c));
    return status;
}

/* GET /conn on `c` (with `headers`, or none), read to its end: the
 * status, and Nginx's connection serial number and request count. */
static DWORD conn_get(HINTERNET c, DWORD flags, const wchar_t *headers, unsigned long *id, unsigned long *n) {
    char text[128];
    HINTERNET r = WinHttpOpenRequest(c, L"GET", L"/conn", NULL, WINHTTP_NO_REFERER, WINHTTP_DEFAULT_ACCEPT_TYPES, flags);
    CHECK(r != NULL);
    CHECK(WinHttpSendRequest(r, headers ? headers : WINHTTP_NO_ADDITIONAL_HEADERS, headers ? (DWORD)-1L : 0,
                             WINHTTP_NO_REQUEST_DATA, 0, 0, 0));
    CHECK(WinHttpReceiveResponse(r, NULL));
    DWORD status = 0, len = sizeof status;
    CHECK(WinHttpQueryHeaders(r, WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER, WINHTTP_HEADER_NAME_BY_INDEX,
                              &status, &len, WINHTTP_NO_HEADER_INDEX));
    DWORD got = 0, total = 0;
    while (total < sizeof text - 1 && WinHttpReadData(r, text + total, sizeof text - 1 - total, &got) && got > 0)
        total += got;
    text[total] = 0;
    /* The end: a read that gives nothing. */
    CHECK(WinHttpReadData(r, text + total, sizeof text - 1 - total, &got) && got == 0);
    CHECK(sscanf(text, "%lu %lu", id, n) == 2);
    CHECK(WinHttpCloseHandle(r));
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

    wchar_t url[256];
    DWORD len;
    /* 7. */
    {
        HINTERNET c = WinHttpConnect(s, L"127.0.0.1", 8080, 0);
        CHECK(c != NULL);
        HINTERNET r = WinHttpOpenRequest(c, L"GET", L"/redirect-rel", NULL, WINHTTP_NO_REFERER,
                                         WINHTTP_DEFAULT_ACCEPT_TYPES, 0);
        CHECK(r != NULL);
        len = sizeof url;
        CHECK(WinHttpQueryOption(r, WINHTTP_OPTION_URL, url, &len));
        CHECK(wcscmp(url, L"http://127.0.0.1:8080/redirect-rel") == 0);
        CHECK(len == wcslen(url) * sizeof(wchar_t));
        CHECK(WinHttpSendRequest(r, WINHTTP_NO_ADDITIONAL_HEADERS, 0, WINHTTP_NO_REQUEST_DATA, 0, 0, 0));
        CHECK(WinHttpReceiveResponse(r, NULL));
        DWORD status = 0;
        len = sizeof status;
        CHECK(WinHttpQueryHeaders(r, WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER, WINHTTP_HEADER_NAME_BY_INDEX,
                                  &status, &len, WINHTTP_NO_HEADER_INDEX));
        CHECK(status == 200);
        DWORD got = 0, total = 0;
        while (WinHttpReadData(r, body + total, sizeof body - 1 - total, &got) && got > 0)
            total += got;
        body[total] = 0;
        CHECK(strstr(body, "Welcome to nginx!") != NULL);
        len = sizeof url;
        CHECK(WinHttpQueryOption(r, WINHTTP_OPTION_URL, url, &len));
        CHECK(wcscmp(url, L"http://127.0.0.1:8080/") == 0);
        CHECK(WinHttpCloseHandle(r));
        CHECK(WinHttpCloseHandle(c));
    }
    /* 8. */
    {
        HINTERNET c = WinHttpConnect(s, L"127.0.0.1", 8080, 0);
        CHECK(c != NULL);
        HINTERNET r = WinHttpOpenRequest(c, L"GET", L"/redirect-rel", NULL, WINHTTP_NO_REFERER,
                                         WINHTTP_DEFAULT_ACCEPT_TYPES, 0);
        CHECK(r != NULL);
        DWORD never = WINHTTP_OPTION_REDIRECT_POLICY_NEVER;
        CHECK(WinHttpSetOption(r, WINHTTP_OPTION_REDIRECT_POLICY, &never, sizeof never));
        CHECK(WinHttpSendRequest(r, WINHTTP_NO_ADDITIONAL_HEADERS, 0, WINHTTP_NO_REQUEST_DATA, 0, 0, 0));
        CHECK(WinHttpReceiveResponse(r, NULL));
        DWORD status = 0;
        len = sizeof status;
        CHECK(WinHttpQueryHeaders(r, WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER, WINHTTP_HEADER_NAME_BY_INDEX,
                                  &status, &len, WINHTTP_NO_HEADER_INDEX));
        CHECK(status == 302);
        len = sizeof url;
        CHECK(WinHttpQueryHeaders(r, WINHTTP_QUERY_LOCATION, WINHTTP_HEADER_NAME_BY_INDEX, url, &len,
                                  WINHTTP_NO_HEADER_INDEX));
        CHECK(wcscmp(url, L"/") == 0);
        len = sizeof url;
        CHECK(WinHttpQueryOption(r, WINHTTP_OPTION_URL, url, &len));
        CHECK(wcscmp(url, L"http://127.0.0.1:8080/redirect-rel") == 0);
        CHECK(WinHttpCloseHandle(r));
        CHECK(WinHttpCloseHandle(c));
    }
    /* 9. */
    status = request(s, L"127.0.0.1", 8080, L"GET", L"/to-https", 0, -1, -1, NULL, url, sizeof url, &error);
    CHECK(error == 0 && status == 200 && wcscmp(url, L"https://localhost:8443/") == 0);
    /* 10. */
    status = request(s, L"localhost", 8443, L"GET", L"/to-http", WINHTTP_FLAG_SECURE, -1, -1, NULL, url, sizeof url,
                     &error);
    CHECK(error == ERROR_WINHTTP_REDIRECT_FAILED);
    status = request(s, L"localhost", 8443, L"GET", L"/to-http", WINHTTP_FLAG_SECURE,
                     WINHTTP_OPTION_REDIRECT_POLICY_ALWAYS, -1, NULL, url, sizeof url, &error);
    CHECK(error == 0 && status == 200 && wcscmp(url, L"http://127.0.0.1:8080/") == 0);
    /* 11. */
    status = request(s, L"127.0.0.1", 8080, L"GET", L"/loop", 0, -1, 2, NULL, url, sizeof url, &error);
    CHECK(error == ERROR_WINHTTP_REDIRECT_FAILED && wcscmp(url, L"http://127.0.0.1:8080/loop") == 0);
    /* 12. */
    status = request(s, L"127.0.0.1", 8080, L"POST", L"/redirect-rel", 0, -1, -1, "data", url, sizeof url, &error);
    CHECK(error == 0 && status == 200);
    status = request(s, L"127.0.0.1", 8080, L"POST", L"/post-307", 0, -1, -1, "data", url, sizeof url, &error);
    CHECK(error == 0 && status == 405 && wcscmp(url, L"http://127.0.0.1:8080/") == 0);
    /* 13. */
    DWORD policy = 0;
    len = sizeof policy;
    CHECK(WinHttpQueryOption(s, WINHTTP_OPTION_REDIRECT_POLICY, &policy, &len));
    CHECK(len == sizeof policy && policy == WINHTTP_OPTION_REDIRECT_POLICY_DISALLOW_HTTPS_TO_HTTP);
    policy = WINHTTP_OPTION_REDIRECT_POLICY_ALWAYS;
    CHECK(!WinHttpSetOption(s, WINHTTP_OPTION_REDIRECT_POLICY, &policy, sizeof policy - 1));
    CHECK(GetLastError() == ERROR_INSUFFICIENT_BUFFER);
    CHECK(!WinHttpSetOption(s, WINHTTP_OPTION_REDIRECT_POLICY, &policy, sizeof policy + 1));
    CHECK(GetLastError() == ERROR_INSUFFICIENT_BUFFER);
    /* 14. */
    {
        unsigned long id[4], n[4];
        HINTERNET c = WinHttpConnect(s, L"127.0.0.1", 8080, 0);
        CHECK(c != NULL);
        for (int i = 0; i < 2; i++) {
            CHECK(conn_get(c, 0, NULL, &id[i], &n[i]) == 200);
        }
        CHECK(id[1] == id[0] && n[1] == n[0] + 1);
        HINTERNET c2 = WinHttpConnect(s, L"127.0.0.1", 8080, 0);
        CHECK(c2 != NULL);
        CHECK(conn_get(c2, 0, NULL, &id[2], &n[2]) == 200);
        CHECK(id[2] == id[0] && n[2] == n[0] + 2);
        /* Connection: close ends it: the next request has a new one. */
        CHECK(conn_get(c2, 0, L"Connection: close", &id[3], &n[3]) == 200);
        CHECK(id[3] == id[0] && n[3] == n[0] + 3);
        CHECK(conn_get(c, 0, NULL, &id[0], &n[0]) == 200);
        CHECK(id[0] != id[3] && n[0] == 1);
        CHECK(WinHttpCloseHandle(c2));
        CHECK(WinHttpCloseHandle(c));
        /* Over TLS. */
        c = WinHttpConnect(s, L"localhost", 8443, 0);
        CHECK(c != NULL);
        CHECK(conn_get(c, WINHTTP_FLAG_SECURE, NULL, &id[0], &n[0]) == 200);
        CHECK(conn_get(c, WINHTTP_FLAG_SECURE, NULL, &id[1], &n[1]) == 200);
        CHECK(id[1] == id[0] && n[1] == n[0] + 1);
        CHECK(WinHttpCloseHandle(c));
    }
    CHECK(WinHttpCloseHandle(s));
    printf("winhttp_test: ok\n");
    return 0;
}
