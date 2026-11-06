/* epoll_test: epoll(7) beyond pipes (poll_test has the pipe basics):
 * TCP sockets over loopback (a listener, both ends of a connection,
 * EPOLLRDHUP, EPOLLONESHOT, EPOLLET as new data arrives, the end of
 * the stream); an epoll fd watched by poll and by another epoll; an fd
 * dup'ed before its close stays in the set (the set holds the open file
 * description); blocking waits: woken by another thread's write,
 * interrupted by a handled signal (EINTR), epoll_pwait's mask keeping a
 * signal pending until it returns, epoll_pwait2's timespec.
 *
 * Whatever a peer sends is waited for with a timeout far longer than it
 * takes (loopback delivery may be asynchronous), so the transcript does
 * not depend on timing. The order between several ready fds is not
 * specified: results are sorted by data. */
#define _GNU_SOURCE
#include "../common/lx.h"
#include "../common/more.h"
#include <fcntl.h>
#include <sched.h>
#include <signal.h>
#include <sys/epoll.h>

#define AF_INET_ 2
#define SOCK_STREAM_ 1
#define SHUT_WR_ 1
#define POLLIN_ 1
#define ITIMER_REAL_ 0

struct lx_sockaddr_in {
	unsigned short family;
	unsigned short port; /* network order */
	unsigned char addr[4];
	unsigned char zero[8];
};

static void loopback(struct lx_sockaddr_in *a, unsigned short port_be)
{
	lx_memset(a, 0, sizeof *a);
	a->family = AF_INET_;
	a->port = port_be;
	a->addr[0] = 127;
	a->addr[3] = 1;
}

static i64 listener(unsigned short *port)
{
	struct lx_sockaddr_in a;
	u64 len = sizeof a;
	i64 s = sys3(SYS_socket, AF_INET_, SOCK_STREAM_, 0);
	loopback(&a, 0);
	sys3(SYS_bind, s, &a, sizeof a);
	sys2(SYS_listen, s, 8);
	sys3(SYS_getsockname, s, &a, &len);
	*port = a.port;
	return s;
}

static void put_events(unsigned ev)
{
	static const struct {
		unsigned bit;
		const char *name;
	} names[] = {
		{ EPOLLIN, "IN" }, { EPOLLPRI, "PRI" }, { EPOLLOUT, "OUT" }, { EPOLLERR, "ERR" }, { EPOLLHUP, "HUP" },
		{ EPOLLRDNORM, "RDNORM" }, { EPOLLWRNORM, "WRNORM" }, { EPOLLRDHUP, "RDHUP" },
	};
	int any = 0;
	for (unsigned i = 0; i < sizeof names / sizeof names[0]; i++)
		if (ev & names[i].bit) {
			if (any)
				put("|");
			put(names[i].name);
			any = 1;
			ev &= ~names[i].bit;
		}
	if (ev) {
		if (any)
			put("|");
		put_hex(ev);
		any = 1;
	}
	if (!any)
		put("0");
}

/* Prints epoll_wait's result, the entries sorted by data. */
static void say_epoll(const char *name, i64 r, struct epoll_event *ev)
{
	for (i64 i = 1; i < r; i++)
		for (i64 j = i; j > 0 && ev[j - 1].data.u64 > ev[j].data.u64; j--) {
			struct epoll_event t = ev[j];
			ev[j] = ev[j - 1];
			ev[j - 1] = t;
		}
	put(name);
	put(" = ");
	put_ret(r);
	for (i64 i = 0; i < r; i++) {
		put(i ? "; " : ": ");
		put("data ");
		put_dec(ev[i].data.u64);
		put(" events ");
		put_events(ev[i].events);
	}
	end_line();
}

static i64 ctl(int ep, int op, int fd, unsigned events, u64 data)
{
	struct epoll_event e;
	e.events = events;
	e.data.u64 = data;
	return sys4(SYS_epoll_ctl, ep, op, fd, &e);
}

static i64 ewait(int ep, struct epoll_event *ev, int max, int ms)
{
	return sys4(SYS_epoll_wait, ep, ev, max, ms);
}

/* A raw thread (as thread_test's): writes to a pipe once told to. */
i64 clone_thread(u64 flags, void *stack, int *ptid, int *ctid, void *tls, int (*fn)(void *), void *arg);
__asm__(".text\n"
	".globl clone_thread\n"
	"clone_thread:\n"
	"	mov 8(%rsp), %rax\n"
	"	sub $16, %rsi\n"
	"	mov %r9, (%rsi)\n"
	"	mov %rax, 8(%rsi)\n"
	"	mov %rcx, %r10\n"
	"	mov $56, %eax\n"
	"	syscall\n"
	"	test %rax, %rax\n"
	"	jnz 1f\n"
	"	xor %ebp, %ebp\n"
	"	pop %rax\n"
	"	pop %rdi\n"
	"	call *%rax\n"
	"	mov %eax, %edi\n"
	"	mov $60, %eax\n"
	"	syscall\n"
	"	hlt\n"
	"1:	ret\n");

#define THREAD_FLAGS (CLONE_VM | CLONE_FS | CLONE_FILES | CLONE_SIGHAND | CLONE_THREAD | CLONE_SYSVSEM | \
		      CLONE_SETTLS | CLONE_PARENT_SETTID | CLONE_CHILD_CLEARTID)

static char stack[65536] __attribute__((aligned(16)));
static u64 tls_block[64] __attribute__((aligned(64)));
static volatile int writer_fd, writer_go;

static int writer(void *arg)
{
	(void)arg;
	while (!writer_go)
		sys0(SYS_sched_yield);
	/* Give the main thread time to block (correct either way). */
	sleep_ms(20);
	write_str(writer_fd, "t");
	return 0;
}

static volatile int alarms;
static void on_alarm(int sig)
{
	(void)sig;
	alarms++;
}

static void arm_ms(int ms)
{
	struct {
		i64 isec, iusec, sec, usec;
	} it = { 0, 0, 0, ms * 1000 };
	sys3(SYS_setitimer, ITIMER_REAL_, &it, 0);
}

int main(void)
{
	struct epoll_event ev[8];
	char buf[64];
	struct lx_sockaddr_in a;
	unsigned short port;
	lx_init();

	say_text("-- sockets");
	i64 l = listener(&port);
	i64 ep = sys1(SYS_epoll_create1, 0);
	say_ret("ADD listener (IN), data 1", ctl(ep, EPOLL_CTL_ADD, l, EPOLLIN, 1));
	say_epoll("epoll_wait(nothing pending, 0)", ewait(ep, ev, 8, 0), ev);
	i64 c = sys3(SYS_socket, AF_INET_, SOCK_STREAM_, 0);
	loopback(&a, port);
	say_ret("connect", sys3(SYS_connect, c, &a, sizeof a));
	say_epoll("epoll_wait(a connection pending, 5 s)", ewait(ep, ev, 8, 5000), ev);
	i64 s = sys4(SYS_accept4, l, 0, 0, 0);
	say_ok("accept4", s);
	say_epoll("epoll_wait(accepted, 0)", ewait(ep, ev, 8, 0), ev);
	say_ret("ADD server end (IN|RDHUP), data 2", ctl(ep, EPOLL_CTL_ADD, s, EPOLLIN | EPOLLRDHUP, 2));
	say_ret("ADD client end (OUT), data 3", ctl(ep, EPOLL_CTL_ADD, c, EPOLLOUT, 3));
	say_epoll("epoll_wait(nothing sent yet)", ewait(ep, ev, 8, 0), ev);
	say_ret("DEL client end", ctl(ep, EPOLL_CTL_DEL, c, 0, 0));
	say_ret("DEL listener", ctl(ep, EPOLL_CTL_DEL, l, 0, 0));
	write_str(c, "hello");
	say_epoll("epoll_wait(5 bytes sent, 5 s)", ewait(ep, ev, 8, 5000), ev);
	say_epoll("  again (level-triggered)", ewait(ep, ev, 8, 0), ev);
	say_read("read", sys3(SYS_read, s, buf, sizeof buf), buf);
	say_epoll("epoll_wait(all read, 0)", ewait(ep, ev, 8, 0), ev);

	say_ret("MOD server end to IN|ONESHOT, data 4", ctl(ep, EPOLL_CTL_MOD, s, EPOLLIN | EPOLLONESHOT, 4));
	write_str(c, "ab");
	say_epoll("EPOLLONESHOT: epoll_wait(5 s)", ewait(ep, ev, 8, 5000), ev);
	say_epoll("EPOLLONESHOT: again (disabled)", ewait(ep, ev, 8, 0), ev);
	say_ret("MOD re-arms it (IN|ET), data 5", ctl(ep, EPOLL_CTL_MOD, s, EPOLLIN | EPOLLET, 5));
	say_epoll("EPOLLET: epoll_wait (data was there before)", ewait(ep, ev, 8, 0), ev);
	say_epoll("EPOLLET: again (no new data)", ewait(ep, ev, 8, 0), ev);
	write_str(c, "c");
	say_epoll("EPOLLET: after more data (5 s)", ewait(ep, ev, 8, 5000), ev);
	say_epoll("EPOLLET: again", ewait(ep, ev, 8, 0), ev);
	say_read("read", sys3(SYS_read, s, buf, sizeof buf), buf);

	say_ret("MOD server end to IN|RDHUP, data 6", ctl(ep, EPOLL_CTL_MOD, s, EPOLLIN | EPOLLRDHUP, 6));
	say_ret("shutdown(client end, SHUT_WR)", sys2(SYS_shutdown, c, SHUT_WR_));
	say_epoll("epoll_wait(peer shut down writing, 5 s)", ewait(ep, ev, 8, 5000), ev);
	say_read("read", sys3(SYS_read, s, buf, sizeof buf), buf);
	say_ret("ADD client end (IN|OUT|RDHUP), data 7", ctl(ep, EPOLL_CTL_ADD, c, EPOLLIN | EPOLLOUT | EPOLLRDHUP, 7));
	say_epoll("epoll_wait(both ends; the client's shut down writing)", ewait(ep, ev, 8, 0), ev);
	say_ret("DEL server end", ctl(ep, EPOLL_CTL_DEL, s, 0, 0));
	say_ret("MOD client end to IN|RDHUP, data 8", ctl(ep, EPOLL_CTL_MOD, c, EPOLLIN | EPOLLRDHUP, 8));
	sys1(SYS_close, s);
	say_epoll("server end closed: epoll_wait(5 s)", ewait(ep, ev, 8, 5000), ev);
	say_ret("MOD client end to IN|OUT|RDHUP, data 7", ctl(ep, EPOLL_CTL_MOD, c, EPOLLIN | EPOLLOUT | EPOLLRDHUP, 7));
	say_epoll("epoll_wait(0)", ewait(ep, ev, 8, 0), ev);
	sys1(SYS_close, c);
	sys1(SYS_close, l);
	say_epoll("all closed", ewait(ep, ev, 8, 0), ev);
	sys1(SYS_close, ep);

	say_text("-- epoll fds and descriptions");
	int p[2];
	sys2(SYS_pipe2, p, 0);
	i64 inner = sys1(SYS_epoll_create1, 0), outer = sys1(SYS_epoll_create1, 0);
	ctl(inner, EPOLL_CTL_ADD, p[0], EPOLLIN, 1);
	say_ret("ADD an epoll fd to another, data 9", ctl(outer, EPOLL_CTL_ADD, inner, EPOLLIN, 9));
	say_ret("ADD the outer one to the inner one (a loop)", ctl(inner, EPOLL_CTL_ADD, outer, EPOLLIN, 8));
	struct {
		int fd;
		short events, revents;
	} pfd = { (int)inner, POLLIN_, 0 };
	say_ret("poll(an epoll fd, nothing ready)", sys3(SYS_poll, &pfd, 1, 0));
	say_epoll("outer epoll_wait(nothing ready)", ewait(outer, ev, 8, 0), ev);
	write_str(p[1], "x");
	say_ret("poll(an epoll fd, one ready)", sys3(SYS_poll, &pfd, 1, 0));
	say("  revents", pfd.revents);
	say_epoll("outer epoll_wait(one ready inside)", ewait(outer, ev, 8, 0), ev);
	say_epoll("  inner epoll_wait", ewait(inner, ev, 8, 0), ev);
	i64 d = sys1(SYS_dup, p[0]);
	sys1(SYS_close, p[0]);
	say_epoll("read end closed but dup'ed: still in the set", ewait(inner, ev, 8, 0), ev);
	say_ret("DEL by the closed fd number", ctl(inner, EPOLL_CTL_DEL, p[0], 0, 0));
	say_ret("DEL by the dup", ctl(inner, EPOLL_CTL_DEL, d, 0, 0));
	sys3(SYS_read, d, buf, sizeof buf);
	ctl(inner, EPOLL_CTL_ADD, d, EPOLLIN, 10);
	say_epoll("ADD the dup, empty pipe", ewait(inner, ev, 8, 0), ev);

	say_text("-- blocking");
	static volatile int ctid = -1;
	int ptid;
	writer_fd = p[1];
	tls_block[0] = (u64)tls_block;
	clone_thread(THREAD_FLAGS, stack + sizeof stack, &ptid, (int *)&ctid, tls_block, writer, 0);
	writer_go = 1;
	say_epoll("epoll_wait(-1) woken by another thread's write", ewait(inner, ev, 8, -1), ev);
	while (ctid != 0)
		sys4(SYS_futex, &ctid, 0, ctid, 0);
	sys3(SYS_read, d, buf, sizeof buf);
	set_action(SIGALRM, on_alarm, 0, 0);
	arm_ms(20);
	say_epoll("epoll_wait(-1), SIGALRM handled meanwhile", ewait(inner, ev, 8, -1), ev);
	say("  handler runs", alarms);
	u64 alrm = 1UL << (SIGALRM - 1);
	arm_ms(20);
	i64 t0 = now_ns(1);
	i64 r = sys6(SYS_epoll_pwait, inner, (i64)ev, 8, 60, (i64)&alrm, 8);
	i64 el = now_ns(1) - t0;
	say_epoll("epoll_pwait(60 ms, SIGALRM blocked meanwhile, due in 20 ms)", r, ev);
	say_bool("  waited >= 60 ms", el >= 60000000);
	say("  handler runs after it returned", alarms);
	say_ret("epoll_pwait(sigsetsize 4)", sys6(SYS_epoll_pwait, inner, (i64)ev, 8, 0, (i64)&alrm, 4));
	struct lx_ts ts = { 0, 30000000 };
	t0 = now_ns(1);
	r = sys6(SYS_epoll_pwait2, inner, (i64)ev, 8, (i64)&ts, 0, 8);
	el = now_ns(1) - t0;
	say_epoll("epoll_pwait2(30 ms)", r, ev);
	say_bool("  waited >= 30 ms", el >= 30000000);
	struct lx_ts bad = { 0, 1000000000 };
	say_ret("epoll_pwait2(tv_nsec = 1e9)", sys6(SYS_epoll_pwait2, inner, (i64)ev, 8, (i64)&bad, 0, 8));
	write_str(p[1], "y");
	say_epoll("epoll_pwait2(NULL timeout, data there)", sys6(SYS_epoll_pwait2, inner, (i64)ev, 8, 0, 0, 8), ev);
	say_text("epoll_test: ok");
	return 0;
}
