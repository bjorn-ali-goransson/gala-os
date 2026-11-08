/* wow64_sock_test: Winsock from a 32-bit (x86) program under WOW64 (ROADMAP
 * WOW, ARCHITECTURE "Plan: WOW64"). Built by build.sh with mingw-w64's
 * i686 compiler against ws2_32. Expected results are those of 32-bit
 * programs on 64-bit Windows: WSAStartup's x86 WSADATA (its descriptions
 * before iMaxSockets), getaddrinfo's x86 ADDRINFOA for "localhost",
 * TCP on the loopback address (bind to port 0, getsockname, listen,
 * connect, accept with the peer's address), send/recv both ways and a
 * WSASend/WSARecv of two WSABUFs each (the data may come in parts), select's x86 fd_set (4-byte
 * SOCKETs), a non-blocking recv's WSAEWOULDBLOCK, a graceful close (recv
 * 0), UDP sendto/recvfrom with the sender's address.
 * Prints "wow64_sock_test: ok" or exits with the failing check's line. */
#include <winsock2.h>
#include <ws2tcpip.h>
#include <stdio.h>
#include <string.h>

static int failed;
#define CHECK(c) do { if (!(c)) { printf("wow64_sock_test: check failed at line %d (WSA error %d)\n", __LINE__, WSAGetLastError()); failed = __LINE__; } } while (0)

int main(void)
{
    WSADATA wsa;
    memset(&wsa, 0xcc, sizeof wsa);
    CHECK(sizeof wsa == 400);
    CHECK(WSAStartup(MAKEWORD(2, 2), &wsa) == 0);
    CHECK(wsa.wVersion == MAKEWORD(2, 2) && wsa.wHighVersion == MAKEWORD(2, 2));
    CHECK(strcmp(wsa.szDescription, "WinSock 2.0") == 0 && strcmp(wsa.szSystemStatus, "Running") == 0);

    /* getaddrinfo: an x86 ADDRINFOA with a SOCKADDR_IN for 127.0.0.1. */
    struct addrinfo hints = { 0 }, *ai = NULL;
    hints.ai_family = AF_INET;
    hints.ai_socktype = SOCK_STREAM;
    CHECK(getaddrinfo("localhost", "80", &hints, &ai) == 0 && ai != NULL);
    if (ai) {
        struct sockaddr_in *sin = (struct sockaddr_in *)ai->ai_addr;
        CHECK(ai->ai_family == AF_INET && ai->ai_addrlen == sizeof(struct sockaddr_in));
        CHECK(sin->sin_addr.s_addr == htonl(INADDR_LOOPBACK) && sin->sin_port == htons(80));
        freeaddrinfo(ai);
    }

    /* TCP on the loopback address. */
    SOCKET l = socket(AF_INET, SOCK_STREAM, IPPROTO_TCP);
    CHECK(l != INVALID_SOCKET);
    struct sockaddr_in at = { 0 };
    at.sin_family = AF_INET;
    at.sin_addr.s_addr = htonl(INADDR_LOOPBACK);
    CHECK(bind(l, (struct sockaddr *)&at, sizeof at) == 0);
    int len = sizeof at;
    CHECK(getsockname(l, (struct sockaddr *)&at, &len) == 0 && at.sin_port != 0);
    CHECK(listen(l, 4) == 0);
    SOCKET c = socket(AF_INET, SOCK_STREAM, IPPROTO_TCP);
    CHECK(connect(c, (struct sockaddr *)&at, sizeof at) == 0);
    struct sockaddr_in peer;
    len = sizeof peer;
    SOCKET a = accept(l, (struct sockaddr *)&peer, &len);
    CHECK(a != INVALID_SOCKET && len == sizeof peer && peer.sin_addr.s_addr == htonl(INADDR_LOOPBACK));

    char buf[64];
    CHECK(send(c, "ping", 4, 0) == 4);
    CHECK(recv(a, buf, sizeof buf, 0) == 4 && memcmp(buf, "ping", 4) == 0);
    CHECK(send(a, "pong!", 5, 0) == 5);
    CHECK(recv(c, buf, sizeof buf, 0) == 5 && memcmp(buf, "pong!", 5) == 0);

    /* WSASend/WSARecv with two buffers each. */
    WSABUF out[2] = { { 3, "abc" }, { 4, "defg" } };
    DWORD n = 0, flags = 0;
    CHECK(WSASend(c, out, 2, &n, 0, NULL, NULL) == 0 && n == 7);
    char x[2], y[16];
    WSABUF in[2] = { { sizeof x, x }, { sizeof y, y } };
    n = 0;
    CHECK(WSARecv(a, in, 2, &n, &flags, NULL, NULL) == 0 && n >= 1 && n <= 7);
    /* TCP may hand over part of it first (the two buffers can go as two
     * segments, and a busy machine runs the receiver between them): the
     * rest comes with later receives, as on Windows. */
    DWORD total = n;
    char got[7];
    memcpy(got, x, total < 2 ? total : 2);
    if (total > 2)
        memcpy(got + 2, y, total - 2);
    while (total < 7) {
        int r = recv(a, got + total, 7 - total, 0);
        if (r <= 0)
            break;
        total += r;
    }
    if (total != 7)
        printf("wow64_sock_test: WSARecv and recv gave %lu of 7 bytes\n", (unsigned long)total);
    CHECK(total == 7 && memcmp(got, "abcdefg", 7) == 0);

    /* select: the x86 fd_set; readable once data is there. */
    fd_set r;
    struct timeval tv = { 0, 0 };
    FD_ZERO(&r);
    FD_SET(a, &r);
    CHECK(select(0, &r, NULL, NULL, &tv) == 0);
    CHECK(send(c, "z", 1, 0) == 1);
    FD_ZERO(&r);
    FD_SET(a, &r);
    FD_SET(l, &r);
    tv.tv_sec = 5;
    CHECK(select(0, &r, NULL, NULL, &tv) == 1 && FD_ISSET(a, &r) && !FD_ISSET(l, &r));
    CHECK(recv(a, buf, sizeof buf, 0) == 1);

    /* Non-blocking: nothing to read. */
    u_long on = 1;
    CHECK(ioctlsocket(a, FIONBIO, &on) == 0);
    CHECK(recv(a, buf, sizeof buf, 0) == SOCKET_ERROR && WSAGetLastError() == WSAEWOULDBLOCK);

    /* A graceful close: recv returns 0. */
    CHECK(closesocket(c) == 0);
    on = 0;
    CHECK(ioctlsocket(a, FIONBIO, &on) == 0);
    CHECK(recv(a, buf, sizeof buf, 0) == 0);
    closesocket(a);
    closesocket(l);

    /* UDP: sendto/recvfrom with the sender's address. */
    SOCKET u1 = socket(AF_INET, SOCK_DGRAM, IPPROTO_UDP), u2 = socket(AF_INET, SOCK_DGRAM, IPPROTO_UDP);
    struct sockaddr_in a1 = { 0 }, a2 = { 0 }, from;
    a1.sin_family = a2.sin_family = AF_INET;
    a1.sin_addr.s_addr = a2.sin_addr.s_addr = htonl(INADDR_LOOPBACK);
    CHECK(bind(u1, (struct sockaddr *)&a1, sizeof a1) == 0 && bind(u2, (struct sockaddr *)&a2, sizeof a2) == 0);
    len = sizeof a1;
    getsockname(u1, (struct sockaddr *)&a1, &len);
    len = sizeof a2;
    getsockname(u2, (struct sockaddr *)&a2, &len);
    CHECK(sendto(u2, "datagram", 8, 0, (struct sockaddr *)&a1, sizeof a1) == 8);
    len = sizeof from;
    CHECK(recvfrom(u1, buf, sizeof buf, 0, (struct sockaddr *)&from, &len) == 8 && memcmp(buf, "datagram", 8) == 0);
    CHECK(len == sizeof from && from.sin_port == a2.sin_port);
    closesocket(u1);
    closesocket(u2);

    CHECK(WSACleanup() == 0);
    if (!failed)
        printf("wow64_sock_test: ok\n");
    return failed;
}
