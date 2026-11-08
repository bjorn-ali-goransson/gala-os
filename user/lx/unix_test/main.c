/* unix_test: AF_UNIX sockets as a Wayland compositor and its clients use
 * them (ROADMAP LD step 2), and D-Bus after them: socket and its flags,
 * socketpair, bind to a file-system path (the node appears, unlink
 * removes it), listen, accept and accept4, connect, read/write,
 * sendmsg/recvmsg with several iovecs, shutdown, getsockname and
 * getpeername, SO_PEERCRED, poll and epoll readiness, non-blocking mode,
 * a full backlog, abstract names and autobind, the errors (ENOENT,
 * ECONNREFUSED, EADDRINUSE, EISCONN, ENOTCONN, EPIPE, ...); SCM_RIGHTS
 * from a forked child: the received descriptors refer to the same open
 * files (a memfd both map, a pipe's write end the child reads from), the
 * truncation flags (MSG_CTRUNC), MSG_CMSG_CLOEXEC, and where a stream
 * read stops around a message carrying descriptors; SOCK_SEQPACKET and
 * SOCK_DGRAM socket pairs (message boundaries, MSG_TRUNC).
 *
 * Everything the peer sends is in place before the parent reads it (a
 * pipe tells it so), fd numbers are fixed by lx_init and printed; pids
 * only as relations. The test works in a directory of its own, which it
 * removes (with its socket files) first and last. */
#define _GNU_SOURCE
#include "../common/lx.h"
#include "../common/more.h"

#define AF_UNIX_ 1
#define AF_INET_ 2
#define SOCK_STREAM_ 1
#define SOCK_DGRAM_ 2
#define SOCK_RAW_ 3
#define SOCK_SEQPACKET_ 5
#define SOCK_NONBLOCK_ 04000
#define SOCK_CLOEXEC_ 02000000
#define SOL_SOCKET_ 1
#define SO_TYPE_ 3
#define SO_ERROR_ 4
#define SO_PEERCRED_ 17
#define SO_ACCEPTCONN_ 30
#define SO_DOMAIN_ 39
#define SCM_RIGHTS_ 1
#define MSG_CTRUNC_ 8
#define MSG_TRUNC_ 0x20
#define MSG_DONTWAIT_ 0x40
#define MSG_NOSIGNAL_ 0x4000
#define MSG_CMSG_CLOEXEC_ 0x40000000
#define SHUT_RD_ 0
#define SHUT_WR_ 1
#define POLLIN_ 1
#define POLLOUT_ 4
#define POLLERR_ 8
#define POLLHUP_ 0x10
#define POLLRDHUP_ 0x2000
#define EPOLLIN_ 1
#define EPOLL_CTL_ADD_ 1
#define F_GETFD_ 1
#define F_GETFL_ 3
#define PROT_READ 1
#define PROT_WRITE 2
#define MAP_SHARED 1
#define AT_REMOVEDIR_ 0x200
#define S_IFMT_ 0170000

#define DIR "unix_test.d"
#define PATH DIR "/server.sock"
#define ABSTRACT "gala-unix_test"

struct lx_sockaddr_un {
	unsigned short family;
	char path[108];
};

struct lx_iovec {
	void *base;
	u64 len;
};

struct lx_msghdr {
	void *name;
	unsigned namelen;
	struct lx_iovec *iov;
	u64 iovlen;
	void *control;
	u64 controllen;
	int flags;
};

struct lx_cmsghdr {
	u64 len;
	int level, type;
};

struct lx_ucred {
	int pid;
	unsigned uid, gid;
};

struct lx_pollfd {
	int fd;
	short events, revents;
};

struct lx_stat {
	u64 dev, ino, nlink;
	unsigned mode, uid, gid, pad;
	u64 rdev;
	i64 size, blksize, blocks;
	u64 times[6];
	i64 reserved[3];
};

/* A sockaddr_un for `path` and its length (family + path + NUL). */
static unsigned path_addr(struct lx_sockaddr_un *a, const char *path)
{
	lx_memset(a, 0, sizeof *a);
	a->family = AF_UNIX_;
	u64 n = lx_strlen(path);
	lx_memcpy(a->path, path, n);
	return 2 + n + 1;
}

/* An abstract name: a NUL, then `name` (no NUL counted). */
static unsigned abstract_addr(struct lx_sockaddr_un *a, const char *name)
{
	lx_memset(a, 0, sizeof *a);
	a->family = AF_UNIX_;
	u64 n = lx_strlen(name);
	lx_memcpy(a->path + 1, name, n);
	return 2 + 1 + n;
}

static i64 unix_socket(int type)
{
	return sys3(SYS_socket, AF_UNIX_, type, 0);
}

static void say_events(const char *name, i64 fd, short events)
{
	struct lx_pollfd p = { (int)fd, events, 0 };
	i64 r = sys3(SYS_poll, &p, 1, 0);
	short rev = p.revents;
	put(name);
	put(" = ");
	put_ret(r);
	put(", revents");
	if (!rev)
		put(" 0");
	if (rev & POLLIN_)
		put(" IN");
	if (rev & POLLOUT_)
		put(" OUT");
	if (rev & POLLERR_)
		put(" ERR");
	if (rev & POLLHUP_)
		put(" HUP");
	if (rev & POLLRDHUP_)
		put(" RDHUP");
	if (rev & ~(POLLIN_ | POLLOUT_ | POLLERR_ | POLLHUP_ | POLLRDHUP_))
		put(" other");
	end_line();
}

/* getsockname/getpeername: the length, and the name as it reads. */
static void say_name(const char *name, i64 fd, int peer)
{
	struct lx_sockaddr_un a;
	unsigned len = sizeof a;
	lx_memset(&a, 0x55, sizeof a);
	i64 r = sys3(peer ? SYS_getpeername : SYS_getsockname, fd, &a, &len);
	put(name);
	put(" = ");
	put_ret(r);
	if (r == 0) {
		put(", len ");
		put_dec(len);
		put(", family ");
		put_dec(a.family);
		if (len > 2 && a.path[0]) {
			put(", path ");
			put_quoted(a.path, lx_strlen(a.path));
		} else if (len > 2) {
			put(", abstract, ");
			put_dec(len - 3);
			put(" bytes");
		}
	}
	end_line();
}

static i64 getopt_int(i64 fd, int name, i64 *value)
{
	int v = -1;
	unsigned len = sizeof v;
	i64 r = sys5(SYS_getsockopt, fd, SOL_SOCKET_, name, &v, &len);
	*value = v;
	return r;
}

static void say_opt(const char *name, i64 fd, int opt)
{
	i64 v;
	i64 r = getopt_int(fd, opt, &v);
	if (r < 0)
		say_ret(name, r);
	else
		say(name, v);
}

/* SO_PEERCRED as relations: whose pid, and the caller's ids. */
static void say_peercred(const char *name, i64 fd, i64 pid)
{
	struct lx_ucred c;
	unsigned len = sizeof c;
	lx_memset(&c, 0xff, sizeof c);
	i64 r = sys5(SYS_getsockopt, fd, SOL_SOCKET_, SO_PEERCRED_, &c, &len);
	put(name);
	put(" = ");
	put_ret(r);
	if (r == 0) {
		put(", len ");
		put_dec(len);
		put(c.pid == pid ? ", pid as expected" : ", pid other");
		put(c.uid == (unsigned)sys0(SYS_getuid) ? ", uid ours" : ", uid other");
		put(c.gid == (unsigned)sys0(SYS_getgid) ? ", gid ours" : ", gid other");
	}
	end_line();
}

static void say_mode(const char *name, const char *path)
{
	struct lx_stat st;
	i64 r = sys2(SYS_stat, path, &st);
	if (r < 0)
		say_ret(name, r);
	else
		say_oct(name, st.mode);
}

static void say_recv(const char *name, i64 fd, u64 size, int flags)
{
	char buf[256];
	i64 r = sys6(SYS_recvfrom, fd, (i64)buf, size, flags, 0, 0);
	say_read(name, r, buf);
}

/* sendmsg of `data` with the `n` descriptors in `fds` as SCM_RIGHTS. */
static i64 send_fds(i64 sock, const char *data, const int *fds, int n)
{
	union {
		struct lx_cmsghdr h;
		char space[64];
	} c;
	struct lx_iovec iov = { (void *)data, lx_strlen(data) };
	struct lx_msghdr m = { 0, 0, &iov, 1, n ? &c : 0, n ? 16 + 8 * ((n * 4 + 7) / 8) : 0, 0 };
	lx_memset(&c, 0, sizeof c);
	c.h.len = 16 + 4 * n;
	c.h.level = SOL_SOCKET_;
	c.h.type = SCM_RIGHTS_;
	lx_memcpy(c.space + 16, fds, 4 * n);
	return sys3(SYS_sendmsg, sock, &m, MSG_NOSIGNAL_);
}

/* recvmsg with `controllen` bytes for control data; prints the bytes,
 * the flags and each control message, and leaves the descriptors
 * received in fds[]; returns how many. */
static int recv_fds(const char *name, i64 sock, u64 controllen, int flags, int *fds)
{
	char data[64];
	union {
		struct lx_cmsghdr h;
		char space[128];
	} c;
	struct lx_iovec iov = { data, sizeof data };
	struct lx_msghdr m = { 0, 0, &iov, 1, controllen ? &c : 0, controllen, -1 };
	lx_memset(&c, 0x55, sizeof c);
	i64 r = sys3(SYS_recvmsg, sock, &m, flags);
	put(name);
	put(" = ");
	put_ret(r);
	int got = 0;
	if (r >= 0) {
		put(" ");
		put_quoted(data, r);
		put(", flags ");
		put_hex(m.flags);
		put(", controllen ");
		put_dec(m.controllen);
		u64 at = 0;
		while (m.control && at + 16 <= m.controllen) {
			struct lx_cmsghdr *h = (struct lx_cmsghdr *)(c.space + at);
			put(", cmsg len ");
			put_dec(h->len);
			put(" level ");
			put_dec(h->level);
			put(" type ");
			put_dec(h->type);
			if (h->level == SOL_SOCKET_ && h->type == SCM_RIGHTS_) {
				int n = (h->len - 16) / 4;
				put(" fds");
				for (int i = 0; i < n; i++) {
					int fd;
					lx_memcpy(&fd, c.space + at + 16 + 4 * i, 4);
					fds[got++] = fd;
					put(" ");
					put_dec(fd);
				}
			}
			if (h->len < 16)
				break;
			at += (h->len + 7) & ~7UL;
		}
	}
	end_line();
	return got;
}

static void cleanup(void)
{
	sys1(SYS_unlink, PATH);
	sys1(SYS_unlink, DIR "/other.sock");
	sys1(SYS_unlink, DIR "/plain");
	sys3(SYS_unlinkat, -100, DIR, AT_REMOVEDIR_);
}

/* The peer: connects to PATH, sends "first", then messages carrying a
 * memfd and a pipe's write end, tells the parent through `sync` that all
 * is sent, then waits on the pipe for the parent's "go" and checks that
 * the parent's store into the memfd shows in its own mapping. */
static void child(int sync)
{
	struct lx_sockaddr_un a;
	unsigned len = path_addr(&a, PATH);
	i64 s = unix_socket(SOCK_STREAM_);
	if (sys3(SYS_connect, s, &a, len) != 0)
		lx_exit(10);
	i64 mfd = sys2(SYS_memfd_create, "unix_test", 0);
	sys2(SYS_ftruncate, mfd, 4096);
	sys4(SYS_pwrite64, mfd, "memfd from child", 16, 0);
	i64 m = sys6(SYS_mmap, 0, 4096, PROT_READ | PROT_WRITE, MAP_SHARED, mfd, 0);
	int p[2];
	sys2(SYS_pipe2, p, 0);
	if (sys3(SYS_write, s, "first", 5) != 5)
		lx_exit(11);
	int two[2] = { (int)mfd, p[1] };
	if (send_fds(s, "second", two, 2) != 6)
		lx_exit(12);
	int same[2] = { (int)mfd, (int)mfd };
	if (send_fds(s, "third", same, 2) != 5)
		lx_exit(13);
	if (send_fds(s, "fourth", same, 1) != 6)
		lx_exit(14);
	if (sys3(SYS_write, s, "fifth", 5) != 5)
		lx_exit(15);
	if (send_fds(s, "sixth", same, 1) != 5)
		lx_exit(19);
	if (sys3(SYS_write, s, "seventh", 7) != 7)
		lx_exit(20);
	sys1(SYS_close, mfd);
	sys1(SYS_close, p[1]);
	sys3(SYS_write, sync, "s", 1);
	char go[8];
	i64 r = sys3(SYS_read, p[0], go, sizeof go);
	if (r != 2 || go[0] != 'g' || go[1] != 'o')
		lx_exit(16);
	/* The pipe's only writer was the received copy: closed, EOF. */
	if (sys3(SYS_read, p[0], go, sizeof go) != 0)
		lx_exit(17);
	if (!lx_memeq((char *)m + 100, "parent wrote this", 17))
		lx_exit(18);
	lx_exit(0);
}

int main(void)
{
	lx_init();
	cleanup();

	/* socket(2) and its flags; the errors. */
	i64 s = unix_socket(SOCK_STREAM_ | SOCK_NONBLOCK_ | SOCK_CLOEXEC_);
	say_ret("socket(AF_UNIX, STREAM|NONBLOCK|CLOEXEC)", s);
	say_ret("F_GETFD", sys2(SYS_fcntl, s, F_GETFD_));
	say_oct("F_GETFL", sys2(SYS_fcntl, s, F_GETFL_));
	say_opt("SO_TYPE", s, SO_TYPE_);
	say_opt("SO_DOMAIN", s, SO_DOMAIN_);
	say_opt("SO_ACCEPTCONN", s, SO_ACCEPTCONN_);
	struct lx_stat st;
	sys2(SYS_fstat, s, &st);
	say_oct("fstat mode", st.mode & S_IFMT_);
	i64 raw = unix_socket(SOCK_RAW_);
	say_ret("socket(AF_UNIX, RAW)", raw);
	say_opt("SO_TYPE raw", raw, SO_TYPE_);
	sys1(SYS_close, raw);
	say_ret("socket(AF_UNIX, RDM)", unix_socket(4));
	i64 one = sys3(SYS_socket, AF_UNIX_, SOCK_STREAM_, 1);
	say_ret("socket(AF_UNIX, STREAM, PF_UNIX)", one);
	sys1(SYS_close, one);
	say_ret("socket(AF_UNIX, STREAM, 2)", sys3(SYS_socket, AF_UNIX_, SOCK_STREAM_, 2));
	say_name("getsockname unbound", s, 0);
	say_name("getpeername unconnected", s, 1);
	char buf[256];
	say_ret("read unconnected", sys3(SYS_read, s, buf, 1));
	say_ret("write unconnected", sys6(SYS_sendto, s, (i64)"x", 1, MSG_NOSIGNAL_, 0, 0));
	say_ret("shutdown unconnected", sys2(SYS_shutdown, s, SHUT_WR_));
	say_ret("listen unbound", sys2(SYS_listen, s, 1));
	say_ret("accept not listening", sys4(SYS_accept4, s, 0, 0, 0));
	say_events("poll fresh", s, POLLIN_ | POLLOUT_);
	sys1(SYS_close, s);

	/* A socket pair. */
	int sv[2];
	say_ret("socketpair(STREAM)", sys4(SYS_socketpair, AF_UNIX_, SOCK_STREAM_, 0, sv));
	say("sv[0]", sv[0]);
	say("sv[1]", sv[1]);
	say_name("getsockname pair", sv[0], 0);
	say_name("getpeername pair", sv[0], 1);
	say_peercred("SO_PEERCRED pair", sv[0], sys0(SYS_getpid));
	say_events("poll pair empty", sv[0], POLLIN_ | POLLOUT_ | POLLRDHUP_);
	say_ret("write 0->1", sys3(SYS_write, sv[0], "ping", 4));
	say_events("poll pair with bytes", sv[1], POLLIN_ | POLLOUT_ | POLLRDHUP_);
	say_recv("recv MSG_PEEK", sv[1], sizeof buf, 2);
	say_recv("read 1", sv[1], sizeof buf, 0);
	say_recv("recv empty DONTWAIT", sv[1], sizeof buf, MSG_DONTWAIT_);
	say_ret("write 1->0", sys3(SYS_write, sv[1], "pong", 4));
	say_ret("write 1->0 again", sys3(SYS_write, sv[1], "!", 1));
	say_recv("read 0 (two writes)", sv[0], sizeof buf, 0);
	{
		char x[3], y[8];
		struct lx_iovec iov[2] = { { "abc", 3 }, { "defgh", 5 } };
		struct lx_msghdr m = { 0, 0, iov, 2, 0, 0, 0 };
		say_ret("sendmsg two iovecs", sys3(SYS_sendmsg, sv[0], &m, 0));
		struct lx_iovec in[2] = { { x, 3 }, { y, 8 } };
		struct lx_msghdr r = { 0, 0, in, 2, 0, 0, -1 };
		i64 n = sys3(SYS_recvmsg, sv[1], &r, 0);
		say_ret("recvmsg two iovecs", n);
		say_bool("scattered", lx_memeq(x, "abc", 3) && lx_memeq(y, "defgh", 5));
		say_hex("recvmsg flags", r.flags);
	}
	say_ret("listen on a pair", sys2(SYS_listen, sv[0], 1));
	say_ret("shutdown SHUT_WR", sys2(SYS_shutdown, sv[0], SHUT_WR_));
	say_events("poll peer after SHUT_WR", sv[1], POLLIN_ | POLLOUT_ | POLLRDHUP_);
	say_recv("read after SHUT_WR", sv[1], sizeof buf, 0);
	say_ret("write after SHUT_WR", sys6(SYS_sendto, sv[0], (i64)"x", 1, MSG_NOSIGNAL_, 0, 0));
	say_events("poll self after SHUT_WR", sv[0], POLLIN_ | POLLOUT_ | POLLRDHUP_);
	say_ret("write the other way", sys3(SYS_write, sv[1], "back", 4));
	say_recv("read the other way", sv[0], sizeof buf, 0);
	say_ret("close 0", sys1(SYS_close, sv[0]));
	say_events("poll after peer closed", sv[1], POLLIN_ | POLLOUT_ | POLLRDHUP_);
	say_recv("read after peer closed", sv[1], sizeof buf, 0);
	say_ret("write after peer closed", sys6(SYS_sendto, sv[1], (i64)"x", 1, MSG_NOSIGNAL_, 0, 0));
	sys1(SYS_close, sv[1]);

	/* Non-blocking: fill a pair until EAGAIN, then read it all back. */
	sys4(SYS_socketpair, AF_UNIX_, SOCK_STREAM_ | SOCK_NONBLOCK_, 0, sv);
	{
		static char chunk[4096];
		i64 total = 0, r;
		while ((r = sys3(SYS_write, sv[0], chunk, sizeof chunk)) > 0)
			total += r;
		say_ret("write until full", r);
		say_bool("something was written", total > 0);
		say_events("poll full", sv[0], POLLOUT_);
		i64 back = 0;
		while ((r = sys3(SYS_read, sv[1], chunk, sizeof chunk)) > 0)
			back += r;
		say_ret("read until empty", r);
		say_bool("read what was written", back == total);
		say_events("poll emptied", sv[0], POLLOUT_);
	}
	sys1(SYS_close, sv[0]);
	sys1(SYS_close, sv[1]);

	/* Binding to a path. */
	say_ret("mkdir", sys2(SYS_mkdir, DIR, 0755));
	struct lx_sockaddr_un a;
	unsigned len = path_addr(&a, PATH);
	i64 l = unix_socket(SOCK_STREAM_);
	say_ret("socket listener", l);
	say_ret("bind", sys3(SYS_bind, l, &a, len));
	say_mode("stat socket file", PATH);
	{
		struct lx_stat ls;
		sys2(SYS_lstat, PATH, &ls);
		say_oct("lstat mode", ls.mode);
		say("st_size", ls.size);
	}
	say_name("getsockname bound", l, 0);
	say_ret("bind again", sys3(SYS_bind, l, &a, len));
	i64 other = unix_socket(SOCK_STREAM_);
	say_ret("bind the same path", sys3(SYS_bind, other, &a, len));
	{
		struct lx_sockaddr_un in = a;
		in.family = AF_INET_;
		say_ret("bind AF_INET family", sys3(SYS_bind, other, &in, len));
		say_ret("bind too long", sys3(SYS_bind, other, &a, sizeof a + 1));
		struct lx_sockaddr_un missing;
		unsigned ml = path_addr(&missing, DIR "/no/such.sock");
		say_ret("bind in a missing directory", sys3(SYS_bind, other, &missing, ml));
	}
	i64 c = unix_socket(SOCK_STREAM_);
	say_ret("connect not listening", sys3(SYS_connect, c, &a, len));
	say_ret("listen", sys2(SYS_listen, l, 5));
	say_opt("SO_ACCEPTCONN listening", l, SO_ACCEPTCONN_);
	say_events("poll listener idle", l, POLLIN_ | POLLOUT_);
	{
		struct lx_sockaddr_un n;
		unsigned nl = path_addr(&n, DIR "/missing.sock");
		say_ret("connect to a missing path", sys3(SYS_connect, c, &n, nl));
		i64 f = sys3(SYS_open, DIR "/plain", 0101 /* O_WRONLY|O_CREAT */, 0644);
		sys1(SYS_close, f);
		nl = path_addr(&n, DIR "/plain");
		say_ret("connect to a regular file", sys3(SYS_connect, c, &n, nl));
		i64 o = sys3(SYS_open, PATH, 0, 0);
		say_ret("open a socket file", o);
	}
	say_ret("connect", sys3(SYS_connect, c, &a, len));
	say_ret("connect again", sys3(SYS_connect, c, &a, len));
	say_events("poll listener with a connection", l, POLLIN_ | POLLOUT_);
	say_name("getsockname client", c, 0);
	say_name("getpeername client", c, 1);
	say_peercred("SO_PEERCRED client", c, sys0(SYS_getpid));
	struct lx_sockaddr_un from;
	unsigned fromlen = sizeof from;
	i64 acc = sys4(SYS_accept4, l, &from, &fromlen, SOCK_CLOEXEC_);
	say_ret("accept4(CLOEXEC)", acc);
	say("accept4 address length", fromlen);
	say_ret("accepted F_GETFD", sys2(SYS_fcntl, acc, F_GETFD_));
	say_name("getsockname accepted", acc, 0);
	say_name("getpeername accepted", acc, 1);
	say_peercred("SO_PEERCRED accepted", acc, sys0(SYS_getpid));
	say_ret("client writes", sys3(SYS_write, c, "hello", 5));
	say_recv("server reads", acc, sizeof buf, 0);
	sys1(SYS_close, acc);
	sys1(SYS_close, c);
	i64 nb = unix_socket(SOCK_STREAM_ | SOCK_NONBLOCK_);
	{
		i64 nl = unix_socket(SOCK_STREAM_ | SOCK_NONBLOCK_);
		struct lx_sockaddr_un o;
		unsigned ol = path_addr(&o, DIR "/other.sock");
		sys3(SYS_bind, nl, &o, ol);
		sys2(SYS_listen, nl, 1);
		say_ret("accept with none waiting", sys4(SYS_accept4, nl, 0, 0, 0));
		/* listen(1): Linux queues backlog + 1 before refusing. */
		int done = 0;
		i64 r = 0;
		for (int i = 0; i < 5; i++) {
			i64 k = unix_socket(SOCK_STREAM_ | SOCK_NONBLOCK_);
			r = sys3(SYS_connect, k, &o, ol);
			sys1(SYS_close, k);
			if (r != 0)
				break;
			done++;
		}
		say("connected before the backlog was full", done);
		say_ret("then connect", r);
		sys1(SYS_close, nl);
		say_ret("connect after the listener closed", sys3(SYS_connect, nb, &o, ol));
	}
	say_ret("unlink socket file", sys1(SYS_unlink, PATH));
	sys1(SYS_close, nb);
	nb = unix_socket(SOCK_STREAM_);
	say_ret("connect after unlink", sys3(SYS_connect, nb, &a, len));
	sys1(SYS_close, nb);
	say_name("getsockname after unlink", l, 0);
	say_ret("close listener", sys1(SYS_close, l));
	say_ret("bind other to the freed path", sys3(SYS_bind, other, &a, path_addr(&a, PATH)));
	sys1(SYS_close, other);
	say_mode("stat after its socket closed", PATH);
	say_ret("unlink it", sys1(SYS_unlink, PATH));

	/* Abstract names and autobind. */
	{
		struct lx_sockaddr_un b;
		unsigned bl = abstract_addr(&b, ABSTRACT);
		i64 al = unix_socket(SOCK_STREAM_);
		say_ret("bind abstract", sys3(SYS_bind, al, &b, bl));
		say_name("getsockname abstract", al, 0);
		i64 dup = unix_socket(SOCK_STREAM_);
		say_ret("bind the same abstract name", sys3(SYS_bind, dup, &b, bl));
		say_ret("listen abstract", sys2(SYS_listen, al, 1));
		i64 k = unix_socket(SOCK_STREAM_);
		say_ret("connect abstract", sys3(SYS_connect, k, &b, bl));
		say_name("getpeername abstract", k, 1);
		struct lx_sockaddr_un shorter;
		unsigned sl = abstract_addr(&shorter, "gala-unix");
		i64 k2 = unix_socket(SOCK_STREAM_);
		say_ret("connect to a prefix of the name", sys3(SYS_connect, k2, &shorter, sl));
		sys1(SYS_close, al);
		say_ret("connect after close", sys3(SYS_connect, k2, &b, bl));
		say_ret("bind the freed abstract name", sys3(SYS_bind, dup, &b, bl));
		sys1(SYS_close, dup);
		sys1(SYS_close, k);
		i64 auto_ = unix_socket(SOCK_STREAM_);
		say_ret("bind with only the family (autobind)", sys3(SYS_bind, auto_, &b, 2));
		say_name("getsockname autobound", auto_, 0);
		sys1(SYS_close, auto_);
		sys1(SYS_close, k2);
	}

	/* A forked peer passing descriptors. */
	{
		len = path_addr(&a, PATH);
		l = unix_socket(SOCK_STREAM_);
		sys3(SYS_bind, l, &a, len);
		sys2(SYS_listen, l, 1);
		int sync[2];
		sys2(SYS_pipe2, sync, 0);
		i64 pid = lx_fork();
		if (pid == 0) {
			sys1(SYS_close, sync[0]);
			child(sync[1]);
		}
		sys1(SYS_close, sync[1]);
		i64 conn = sys4(SYS_accept4, l, 0, 0, 0);
		say_ret("accept the child", conn);
		say_peercred("SO_PEERCRED of the child", conn, pid);
		i64 ep = sys1(SYS_epoll_create1, 0);
		struct {
			unsigned events;
			unsigned long data;
		} __attribute__((packed)) ev = { EPOLLIN_, 7 }, out[2];
		sys4(SYS_epoll_ctl, ep, EPOLL_CTL_ADD_, conn, &ev);
		i64 n = sys4(SYS_epoll_wait, ep, out, 2, -1);
		say_ret("epoll_wait", n);
		say_bool("epoll reports IN for the connection",
			 n == 1 && out[0].events == EPOLLIN_ && out[0].data == 7);
		say_ret("sync", sys3(SYS_read, sync[0], buf, 1));
		say_recv("recv 5", conn, 5, 0);
		int fds[4];
		int got = recv_fds("recvmsg with room", conn, 64, 0, fds);
		say("descriptors", got);
		if (got == 2) {
			struct lx_stat ms;
			sys2(SYS_fstat, fds[0], &ms);
			say("memfd size", ms.size);
			say_ret("memfd F_GETFD", sys2(SYS_fcntl, fds[0], F_GETFD_));
			char data[32];
			i64 r = sys4(SYS_pread64, fds[0], data, 16, 0);
			say_read("memfd reads", r, data);
			i64 m = sys6(SYS_mmap, 0, 4096, PROT_READ | PROT_WRITE, MAP_SHARED, fds[0], 0);
			if (m >= 0 || m < -4095)
				lx_memcpy((char *)m + 100, "parent wrote this", 17);
		}
		int more[4];
		int k = recv_fds("recvmsg room for one (CMSG_CLOEXEC)", conn, 20, MSG_CMSG_CLOEXEC_, more);
		say("descriptors", k);
		if (k == 1)
			say_ret("received F_GETFD", sys2(SYS_fcntl, more[0], F_GETFD_));
		say_ret("the dropped descriptor is not open", sys2(SYS_fcntl, more[0] + 1, F_GETFD_));
		k = recv_fds("recvmsg without room", conn, 0, 0, more + 1);
		say("descriptors", k);
		say_recv("read (takes a message with descriptors, then stops)", conn, sizeof buf, 0);
		say_ret("its descriptors are not open", sys2(SYS_fcntl, more[0] + 1, F_GETFD_));
		say_recv("read the rest", conn, sizeof buf, 0);
		if (got == 2) {
			say_ret("write go to the child's pipe", sys3(SYS_write, fds[1], "go", 2));
			sys1(SYS_close, fds[1]);
		}
		say_status("child", wait_for(pid, 0));
		say_recv("read after the child ended", conn, sizeof buf, 0);
		say_ret("sendmsg a bad descriptor", send_fds(conn, "x", (int[]){ 99 }, 1));
		sys1(SYS_close, conn);
		sys1(SYS_close, l);
		sys1(SYS_close, ep);
		sys1(SYS_close, sync[0]);
		if (got == 2)
			sys1(SYS_close, fds[0]);
		if (k >= 0)
			sys1(SYS_close, more[0]);
		cleanup();
	}

	/* Message sockets. */
	say_ret("socketpair(SEQPACKET)", sys4(SYS_socketpair, AF_UNIX_, SOCK_SEQPACKET_, 0, sv));
	say_opt("SO_TYPE seqpacket", sv[0], SO_TYPE_);
	sys3(SYS_write, sv[0], "abc", 3);
	sys3(SYS_write, sv[0], "defgh", 5);
	sys3(SYS_write, sv[0], "", 0);
	{
		int none[1];
		say_recv("seqpacket read 2 of 3", sv[1], 2, 0);
		recv_fds("seqpacket recvmsg", sv[1], 0, 0, none);
		say_recv("seqpacket empty message", sv[1], sizeof buf, MSG_DONTWAIT_);
		say_recv("seqpacket nothing more", sv[1], sizeof buf, MSG_DONTWAIT_);
		sys3(SYS_write, sv[0], "truncated", 9);
		char small[4];
		struct lx_iovec iov = { small, 4 };
		struct lx_msghdr m = { 0, 0, &iov, 1, 0, 0, -1 };
		i64 r = sys3(SYS_recvmsg, sv[1], &m, 0);
		say_ret("seqpacket recvmsg 4 of 9", r);
		say_hex("its flags", m.flags);
	}
	sys1(SYS_close, sv[0]);
	say_events("poll seqpacket after peer closed", sv[1], POLLIN_ | POLLOUT_ | POLLRDHUP_);
	say_recv("seqpacket read after peer closed", sv[1], sizeof buf, 0);
	sys1(SYS_close, sv[1]);

	say_ret("socketpair(DGRAM)", sys4(SYS_socketpair, AF_UNIX_, SOCK_DGRAM_, 0, sv));
	say_opt("SO_TYPE dgram", sv[0], SO_TYPE_);
	sys3(SYS_write, sv[0], "one", 3);
	sys3(SYS_write, sv[0], "two!", 4);
	{
		char small[2];
		struct lx_iovec iov = { small, 2 };
		struct lx_msghdr m = { 0, 0, &iov, 1, 0, 0, -1 };
		i64 r = sys3(SYS_recvmsg, sv[1], &m, 0);
		say_ret("dgram recvmsg 2 of 3", r);
		say_hex("its flags", m.flags);
		say_recv("dgram next", sv[1], sizeof buf, 0);
		say_recv("dgram MSG_TRUNC", sv[1], 0, MSG_DONTWAIT_ | MSG_TRUNC_);
	}
	sys1(SYS_close, sv[0]);
	say_events("poll dgram after peer closed", sv[1], POLLIN_ | POLLOUT_);
	say_recv("dgram read after peer closed", sv[1], sizeof buf, MSG_DONTWAIT_);
	say_ret("dgram write after peer closed", sys6(SYS_sendto, sv[1], (i64)"x", 1, MSG_NOSIGNAL_, 0, 0));
	sys1(SYS_close, sv[1]);

	say_text("unix_test: ok");
	return 0;
}
