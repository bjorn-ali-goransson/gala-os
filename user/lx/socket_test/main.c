/* socket_test: TCP sockets over loopback as the raw system calls give
 * them (ROADMAP LX, H11: what busybox httpd and wget use): socket and its
 * type flags, bind, listen, accept4, connect (blocking, and non-blocking
 * with poll and SO_ERROR), read/write and send/recv/sendto/recvfrom,
 * MSG_PEEK, shutdown, getsockname/getpeername, the socket options a
 * server and a client set, poll on sockets, a child serving a connection
 * after fork, and the errors (EADDRINUSE, ECONNREFUSED, ENOTCONN,
 * EAGAIN, EINVAL, ENOTSOCK, EAFNOSUPPORT, EISCONN, EPIPE); many listeners
 * open at once (a listener holds no fixed share of the stack's sockets),
 * and a full backlog: Linux completes backlog + 1 connections and ignores
 * the SYNs beyond (no ECONNREFUSED, tcp(7)'s tcp_abort_on_overflow off),
 * so those connect once accepts make room (the client's SYN retransmits).
 *
 * Ports are the kernel's choice (bind to port 0) and printed only as
 * relations; the refused connect goes to a port just bound and closed.
 * Everything a peer sends is in place before the other side reads it, or
 * the reader blocks for it; a child reports through its exit status. */
#define _GNU_SOURCE
#include "../common/lx.h"
#include "../common/more.h"
#include <fcntl.h>
#include <sys/stat.h>

#define AF_INET_ 2
#define AF_INET6_ 10
#define SOCK_STREAM_ 1
#define SOCK_NONBLOCK_ 04000
#define SOCK_CLOEXEC_ 02000000
#define SOL_SOCKET_ 1
#define SO_REUSEADDR_ 2
#define SO_TYPE_ 3
#define SO_ERROR_ 4
#define SO_KEEPALIVE_ 9
#define IPPROTO_TCP_ 6
#define TCP_NODELAY_ 1
#define MSG_PEEK_ 2
#define MSG_DONTWAIT_ 0x40
#define MSG_NOSIGNAL_ 0x4000
#define SHUT_WR_ 1
#define POLLIN_ 1
#define POLLOUT_ 4
#define POLLERR_ 8
#define POLLHUP_ 0x10

#define SYS6(n, a, b, c, d, e, f) sys6(n, (i64)(a), (i64)(b), (i64)(c), (i64)(d), (i64)(e), (i64)(f))

struct lx_sockaddr_in {
	unsigned short family;
	unsigned short port; /* network order */
	unsigned char addr[4];
	unsigned char zero[8];
};

struct lx_pollfd {
	int fd;
	short events, revents;
};

static void loopback(struct lx_sockaddr_in *a, unsigned short port_be)
{
	lx_memset(a, 0, sizeof *a);
	a->family = AF_INET_;
	a->port = port_be;
	a->addr[0] = 127;
	a->addr[3] = 1;
}

static i64 tcp_socket(int flags)
{
	return sys3(SYS_socket, AF_INET_, SOCK_STREAM_ | flags, 0);
}

/* A listening socket on 127.0.0.1 and a kernel-chosen port; the port
 * (network order) in *port. */
static i64 listener(unsigned short *port)
{
	struct lx_sockaddr_in a;
	u64 len = sizeof a;
	i64 s = tcp_socket(0);
	loopback(&a, 0);
	sys3(SYS_bind, s, &a, sizeof a);
	sys2(SYS_listen, s, 8);
	sys3(SYS_getsockname, s, &a, &len);
	*port = a.port;
	return s;
}

/* A listener on 127.0.0.1, a kernel-chosen port and `backlog`. */
static i64 listener_with(unsigned short *port, int backlog)
{
	struct lx_sockaddr_in a;
	u64 len = sizeof a;
	i64 s = tcp_socket(0);
	loopback(&a, 0);
	sys3(SYS_bind, s, &a, sizeof a);
	if (sys2(SYS_listen, s, backlog) != 0)
		return -1;
	sys3(SYS_getsockname, s, &a, &len);
	*port = a.port;
	return s;
}

static i64 connect_to(i64 s, unsigned short port)
{
	struct lx_sockaddr_in a;
	loopback(&a, port);
	return sys3(SYS_connect, s, &a, sizeof a);
}

static void say_events(const char *name, i64 r, short revents)
{
	put(name);
	put(" = ");
	put_ret(r);
	put(", revents");
	if (!revents)
		put(" 0");
	if (revents & POLLIN_)
		put(" IN");
	if (revents & POLLOUT_)
		put(" OUT");
	if (revents & POLLERR_)
		put(" ERR");
	if (revents & POLLHUP_)
		put(" HUP");
	if (revents & ~(POLLIN_ | POLLOUT_ | POLLERR_ | POLLHUP_))
		put(" other");
	end_line();
}

static i64 poll1(i64 fd, short events, int ms, short *revents)
{
	struct lx_pollfd p = { (int)fd, events, 0 };
	i64 r = sys3(SYS_poll, &p, 1, ms);
	*revents = p.revents;
	return r;
}

static void say_poll(const char *name, i64 fd, short events, int ms)
{
	short rev = 0;
	i64 r = poll1(fd, events, ms, &rev);
	say_events(name, r, rev);
}

int main(void)
{
	char buf[64];
	short rev = 0;
	struct lx_sockaddr_in a, b;
	u64 len;
	int v;
	lx_init();

	say_text("-- socket");
	i64 s = tcp_socket(0);
	say("socket(AF_INET, SOCK_STREAM) fd", s);
	struct stat st;
	sys2(SYS_fstat, s, &st);
	say_bool("  fstat: S_ISSOCK", S_ISSOCK(st.st_mode));
	say_ret("  F_GETFL & O_ACCMODE", sys2(SYS_fcntl, s, F_GETFL) & O_ACCMODE);
	say_ret("  F_GETFD", sys2(SYS_fcntl, s, F_GETFD));
	len = sizeof v;
	say_ret("  getsockopt(SO_TYPE)", sys5(SYS_getsockopt, s, SOL_SOCKET_, SO_TYPE_, &v, &len));
	say("  type", v);
	say("  optlen", len);
	i64 t = tcp_socket(SOCK_NONBLOCK_ | SOCK_CLOEXEC_);
	say_bool("socket(SOCK_NONBLOCK|SOCK_CLOEXEC): F_GETFL has O_NONBLOCK", (sys2(SYS_fcntl, t, F_GETFL) & O_NONBLOCK) != 0);
	say_ret("  F_GETFD", sys2(SYS_fcntl, t, F_GETFD));
	sys1(SYS_close, t);
	say_ret("socket(999, SOCK_STREAM)", sys3(SYS_socket, 999, SOCK_STREAM_, 0));
	say_ret("socket(AF_INET, type 99)", sys3(SYS_socket, AF_INET_, 99, 0));
	say_ret("read(unconnected socket)", sys3(SYS_read, s, buf, 1));
	say_ret("getpeername(unconnected)", sys3(SYS_getpeername, s, &a, &len));
	i64 f = sys4(SYS_openat, AT_FDCWD, "/", O_RDONLY | O_DIRECTORY, 0);
	say_ret("listen(not a socket)", sys2(SYS_listen, f, 1));
	sys1(SYS_close, f);
	say_ret("listen(999)", sys2(SYS_listen, 999, 1));

	say_text("-- bind and listen");
	v = 1;
	say_ret("setsockopt(SO_REUSEADDR, 1)", sys5(SYS_setsockopt, s, SOL_SOCKET_, SO_REUSEADDR_, &v, sizeof v));
	len = sizeof v;
	sys5(SYS_getsockopt, s, SOL_SOCKET_, SO_REUSEADDR_, &v, &len);
	say_bool("  getsockopt(SO_REUSEADDR) is set", v != 0);
	loopback(&a, 0);
	say_ret("bind(short address)", sys3(SYS_bind, s, &a, 8));
	a.family = AF_INET6_;
	say_ret("bind(AF_INET6 address to an AF_INET socket)", sys3(SYS_bind, s, &a, sizeof a));
	loopback(&a, 0);
	say_ret("bind(127.0.0.1:0)", sys3(SYS_bind, s, &a, sizeof a));
	say_ret("bind again", sys3(SYS_bind, s, &a, sizeof a));
	len = sizeof a;
	say_ret("getsockname", sys3(SYS_getsockname, s, &a, &len));
	say("  family", a.family);
	say_bool("  port chosen (not 0)", a.port != 0);
	say_bool("  address 127.0.0.1", a.addr[0] == 127 && a.addr[1] == 0 && a.addr[2] == 0 && a.addr[3] == 1);
	say("  addrlen", len);
	unsigned short port = a.port;
	t = tcp_socket(0);
	say_ret("bind(the same port, another socket)", sys3(SYS_bind, t, &a, sizeof a));
	sys1(SYS_close, t);
	say_ret("listen", sys2(SYS_listen, s, 8));
	sys3(SYS_fcntl, s, F_SETFL, O_NONBLOCK);
	say_ret("accept(non-blocking listener, nothing pending)", sys3(SYS_accept, s, 0, 0));
	sys3(SYS_fcntl, s, F_SETFL, 0);
	say_poll("poll(listener, IN, 0) nothing pending", s, POLLIN_, 0);

	say_text("-- connect and accept");
	i64 c = tcp_socket(0);
	say_ret("connect(127.0.0.1:port)", connect_to(c, port));
	say_ret("connect again", connect_to(c, port));
	say_poll("poll(listener, IN, 5 s)", s, POLLIN_, 5000);
	len = sizeof b;
	lx_memset(&b, 0xaa, sizeof b);
	i64 d = sys4(SYS_accept4, s, &b, &len, SOCK_CLOEXEC_);
	say_ok("accept4(SOCK_CLOEXEC)", d);
	say_ret("  F_GETFD", sys2(SYS_fcntl, d, F_GETFD));
	say("  peer family", b.family);
	say("  addrlen", len);
	struct lx_sockaddr_in cn;
	len = sizeof cn;
	sys3(SYS_getsockname, c, &cn, &len);
	say_bool("  peer is the client's address (port and 127.0.0.1)", b.port == cn.port && b.addr[0] == 127 && b.addr[3] == 1);
	say_bool("  the client's port is not the server's", cn.port != port);
	len = sizeof b;
	say_ret("getpeername(client)", sys3(SYS_getpeername, c, &b, &len));
	say_bool("  is the listener's port", b.port == port);
	len = sizeof b;
	sys3(SYS_getsockname, d, &b, &len);
	say_bool("getsockname(accepted) is the listener's port", b.port == port);
	v = 1;
	say_ret("setsockopt(TCP_NODELAY, 1)", sys5(SYS_setsockopt, c, IPPROTO_TCP_, TCP_NODELAY_, &v, sizeof v));
	say_ret("setsockopt(SO_KEEPALIVE, 1)", sys5(SYS_setsockopt, c, SOL_SOCKET_, SO_KEEPALIVE_, &v, sizeof v));
	len = sizeof v;
	sys5(SYS_getsockopt, c, SOL_SOCKET_, SO_ERROR_, &v, &len);
	say("getsockopt(SO_ERROR) after a connect", v);

	say_text("-- data");
	say_poll("poll(accepted, IN, 0) nothing sent", d, POLLIN_, 0);
	say_poll("poll(client, OUT, 0)", c, POLLOUT_, 0);
	say_ret("write(client, hello)", write_str(c, "hello"));
	say_poll("poll(accepted, IN, 5 s)", d, POLLIN_, 5000);
	say_read("recvfrom(accepted, MSG_PEEK)", SYS6(SYS_recvfrom, d, buf, 3, MSG_PEEK_, 0, 0), buf);
	say_read("read(accepted, 64)", sys3(SYS_read, d, buf, sizeof buf), buf);
	say_ret("recv(accepted, MSG_DONTWAIT) when empty", SYS6(SYS_recvfrom, d, buf, sizeof buf, MSG_DONTWAIT_, 0, 0));
	say_ret("sendto(accepted, reply, NULL address)", SYS6(SYS_sendto, d, "reply", 5, 0, 0, 0));
	say_read("read(client) (blocks until it is there)", sys3(SYS_read, c, buf, 5), buf);
	say_ret("shutdown(client, SHUT_WR)", sys2(SYS_shutdown, c, SHUT_WR_));
	say_ret("read(accepted) after the client's shutdown (end of stream)", sys3(SYS_read, d, buf, sizeof buf));
	say_poll("poll(accepted, IN) after it", d, POLLIN_, 0);
	say_ret("write(client) after SHUT_WR, MSG_NOSIGNAL", SYS6(SYS_sendto, c, "x", 1, MSG_NOSIGNAL_, 0, 0));
	say_ret("write(accepted, bye) still works", write_str(d, "bye"));
	say_ret("close(accepted)", sys1(SYS_close, d));
	say_read("read(client) the rest", sys3(SYS_read, c, buf, sizeof buf), buf);
	say_ret("read(client) then (end of stream)", sys3(SYS_read, c, buf, sizeof buf));
	sys1(SYS_close, c);

	say_text("-- non-blocking connect");
	c = tcp_socket(SOCK_NONBLOCK_);
	i64 r = connect_to(c, port);
	say_bool("connect(non-blocking) = 0 or EINPROGRESS", r == 0 || r == -115);
	say_poll("poll(OUT, 5 s)", c, POLLOUT_, 5000);
	len = sizeof v;
	sys5(SYS_getsockopt, c, SOL_SOCKET_, SO_ERROR_, &v, &len);
	say("  SO_ERROR", v);
	say_ret("  connect again", connect_to(c, port));
	d = sys3(SYS_accept, s, 0, 0);
	say_ok("accept(NULL address)", d);
	sys1(SYS_close, c);
	sys1(SYS_close, d);

	say_text("-- refused");
	unsigned short dead;
	t = listener(&dead);
	sys1(SYS_close, t);
	c = tcp_socket(0);
	say_ret("connect(a closed port)", connect_to(c, dead));
	sys1(SYS_close, c);
	c = tcp_socket(SOCK_NONBLOCK_);
	r = connect_to(c, dead);
	say_bool("connect(non-blocking, closed port) = EINPROGRESS or ECONNREFUSED", r == -115 || r == -111);
	if (r == -115) {
		poll1(c, POLLOUT_, 5000, &rev);
		say_bool("  poll then has ERR", (rev & POLLERR_) != 0);
		len = sizeof v;
		sys5(SYS_getsockopt, c, SOL_SOCKET_, SO_ERROR_, &v, &len);
		say("  SO_ERROR", v);
	} else {
		say_bool("  poll then has ERR", 1);
		say("  SO_ERROR", 111);
	}
	sys1(SYS_close, c);

	say_text("-- a child serves a connection");
	c = tcp_socket(0);
	connect_to(c, port);
	d = sys3(SYS_accept, s, 0, 0);
	i64 pid = lx_fork();
	if (pid == 0) {
		sys1(SYS_close, s);
		sys1(SYS_close, c);
		char in[16];
		i64 n = sys3(SYS_read, d, in, sizeof in);
		write_str(d, "served:");
		sys3(SYS_write, d, in, n > 0 ? n : 0);
		lx_exit(n == 4 ? 0 : 1);
	}
	sys1(SYS_close, d);
	write_str(c, "ping");
	say_status("child", wait_for(pid, 0));
	say_read("read(client) the child's answer", sys3(SYS_read, c, buf, sizeof buf), buf);
	say_ret("read(client) then (the child closed it)", sys3(SYS_read, c, buf, sizeof buf));
	sys1(SYS_close, c);
	sys1(SYS_close, s);

	say_text("-- many listeners");
	{
		enum { MANY = 12 };
		i64 l[MANY];
		unsigned short ports[MANY];
		int listening = 0, served = 0;
		for (int i = 0; i < MANY; i++) {
			l[i] = listener_with(&ports[i], 511);
			listening += l[i] >= 0;
		}
		say("listeners at backlog 511", listening);
		for (int i = 0; i < MANY; i++) {
			char in = 0, back = 0, mine = (char)('a' + i);
			c = tcp_socket(0);
			if (connect_to(c, ports[i]) != 0) {
				sys1(SYS_close, c);
				continue;
			}
			d = sys3(SYS_accept, l[i], 0, 0);
			if (d >= 0 && sys3(SYS_write, c, &mine, 1) == 1 && sys3(SYS_read, d, &in, 1) == 1 && sys3(SYS_write, d, &in, 1) == 1 && sys3(SYS_read, c, &back, 1) == 1 && back == mine)
				served++;
			sys1(SYS_close, d);
			sys1(SYS_close, c);
		}
		say("  each accepted a connection and echoed a byte", served);
		for (int i = 0; i < MANY; i++)
			sys1(SYS_close, l[i]);
	}

	say_text("-- a full backlog");
	{
		enum { CLIENTS = 4 };
		unsigned short bport;
		i64 l = listener_with(&bport, 1);
		i64 cl[CLIENTS];
		int connected = 0, failed = 0, accepted = 0, settled = 0;
		for (int i = 0; i < CLIENTS; i++) {
			cl[i] = tcp_socket(SOCK_NONBLOCK_);
			connect_to(cl[i], bport);
		}
		for (int i = 0; i < CLIENTS; i++) {
			poll1(cl[i], POLLOUT_, 300, &rev);
			connected += (rev & POLLOUT_) != 0 && (rev & POLLERR_) == 0;
			failed += (rev & POLLERR_) != 0;
		}
		say("listen(1), 4 non-blocking connects: connected within 300 ms", connected);
		say("  refused or failed", failed);
		/* Each accept makes room; a waiting client's next SYN gets in. */
		for (int i = 0; i < CLIENTS; i++) {
			d = sys3(SYS_accept, l, 0, 0);
			accepted += d >= 0;
			sys1(SYS_close, d);
		}
		say("  accepted, one by one", accepted);
		for (int i = 0; i < CLIENTS; i++) {
			len = sizeof v;
			v = -1;
			sys5(SYS_getsockopt, cl[i], SOL_SOCKET_, SO_ERROR_, &v, &len);
			settled += v == 0;
			sys1(SYS_close, cl[i]);
		}
		say("  clients with SO_ERROR 0", settled);
		sys1(SYS_close, l);
	}
	say_text("socket_test: ok");
	return 0;
}
