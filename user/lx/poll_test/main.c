/* poll_test: poll/ppoll, select/pselect6 and epoll (epoll_create1,
 * epoll_ctl ADD/MOD/DEL, epoll_wait/epoll_pwait, EPOLLET, EPOLLONESHOT,
 * timeouts) on pipes, as the raw system calls give them: the revents and
 * events reported for empty, readable, full, hung-up and errored pipes,
 * the counts, the errors, and timeouts lasting at least what was asked
 * (printed as booleans). struct epoll_event is packed on x86_64 (12
 * bytes): the test prints the stride it observes in epoll_wait's array. */
#define _GNU_SOURCE
#include "../common/lx.h"
#include "../common/more.h"
#include <fcntl.h>
#include <poll.h>
#include <signal.h>
#include <stddef.h>
#include <sys/epoll.h>
#include <sys/select.h>

static void put_events(unsigned ev)
{
	static const struct {
		unsigned bit;
		const char *name;
	} names[] = {
		{ EPOLLIN, "IN" }, { EPOLLPRI, "PRI" }, { EPOLLOUT, "OUT" }, { EPOLLERR, "ERR" }, { EPOLLHUP, "HUP" },
		{ EPOLLRDNORM, "RDNORM" }, { EPOLLRDBAND, "RDBAND" }, { EPOLLWRNORM, "WRNORM" }, { EPOLLWRBAND, "WRBAND" },
		{ EPOLLRDHUP, "RDHUP" }, { POLLNVAL, "NVAL" },
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

/* poll one fd for `events` with timeout 0: "name = r, revents X". */
static void poll_one(const char *name, int fd, short events)
{
	struct pollfd p = { fd, events, 0x7777 };
	i64 r = sys3(SYS_poll, &p, 1, 0);
	put(name);
	put(" = ");
	put_ret(r);
	put(", revents ");
	put_events((unsigned short)p.revents);
	end_line();
}

static void say_epoll(const char *name, i64 r, struct epoll_event *ev)
{
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

int main(void)
{
	int p[2], q[2];
	char buf[256];
	static char chunk[65536];
	lx_init();

	say_text("-- poll");
	sys2(SYS_pipe2, p, 0); /* 3, 4 */
	poll_one("poll(empty read end, IN)", p[0], POLLIN);
	poll_one("poll(write end, OUT)", p[1], POLLOUT);
	poll_one("poll(write end, IN|OUT)", p[1], POLLIN | POLLOUT);
	write_str(p[1], "x");
	poll_one("poll(read end with data, IN|PRI|OUT)", p[0], POLLIN | POLLPRI | POLLOUT);
	poll_one("poll(read end with data, 0 events)", p[0], 0);
	poll_one("poll(999)", 999, POLLIN);
	struct pollfd neg = { -1, POLLIN, 0x7777 };
	say_ret("poll(fd -1: skipped)", sys3(SYS_poll, &neg, 1, 0));
	say("  revents", neg.revents);
	struct pollfd two[2] = { { p[0], POLLIN, 0 }, { p[1], POLLOUT, 0 } };
	say_ret("poll(both ends)", sys3(SYS_poll, two, 2, 0));
	sys1(SYS_close, p[1]);
	poll_one("poll(read end, data, writer closed)", p[0], POLLIN);
	sys3(SYS_read, p[0], buf, 1);
	poll_one("poll(read end, empty, writer closed)", p[0], POLLIN);
	poll_one("poll(read end, writer closed, 0 events)", p[0], 0);
	sys1(SYS_close, p[0]);
	sys2(SYS_pipe2, p, 0);
	sys1(SYS_close, p[0]);
	poll_one("poll(write end, reader closed, OUT)", p[1], POLLOUT);
	sys1(SYS_close, p[1]);
	sys2(SYS_pipe2, p, O_NONBLOCK);
	while (sys3(SYS_write, p[1], chunk, sizeof chunk) > 0)
		;
	poll_one("poll(full write end, OUT)", p[1], POLLOUT);
	sys3(SYS_read, p[0], chunk, 4096);
	poll_one("poll(write end, a page free, OUT)", p[1], POLLOUT);
	sys1(SYS_close, p[0]);
	sys1(SYS_close, p[1]);

	sys2(SYS_pipe2, p, 0);
	struct pollfd one = { p[0], POLLIN, 0 };
	i64 t0 = now_ns(1);
	i64 r = sys3(SYS_poll, &one, 1, 30);
	i64 el = now_ns(1) - t0;
	say_ret("poll(empty, 30 ms)", r);
	say_bool("  waited >= 30 ms", el >= 30000000);
	struct lx_ts ts = { 0, 30000000 };
	t0 = now_ns(1);
	r = sys5(SYS_ppoll, &one, 1, &ts, 0, 8);
	el = now_ns(1) - t0;
	say_ret("ppoll(empty, 30 ms)", r);
	say_bool("  waited >= 30 ms", el >= 30000000);
	say_bool("  timeout left updated to 0", ts.sec == 0 && ts.nsec == 0);
	struct lx_ts bad = { 0, 1000000000 };
	say_ret("ppoll(tv_nsec = 1e9)", sys5(SYS_ppoll, &one, 1, &bad, 0, 8));
	say_ret("poll(nfds too large)", sys3(SYS_poll, &one, 0x7fffffff, 0));
	say_ret("poll(bad pointer)", sys3(SYS_poll, 8, 1, 0));
	i64 pid = lx_fork();
	if (pid == 0) {
		sleep_ms(30);
		write_str(p[1], "w");
		lx_exit(0);
	}
	one.revents = 0;
	r = sys3(SYS_poll, &one, 1, 5000);
	say_ret("poll(5 s) woken by a child's write", r);
	put("  revents ");
	put_events(one.revents);
	end_line();
	wait_for(pid, 0);
	sys3(SYS_read, p[0], buf, 1);
	say("sizeof(struct pollfd)", sizeof(struct pollfd));
	say("offsetof(pollfd, events)", offsetof(struct pollfd, events));
	say("offsetof(pollfd, revents)", offsetof(struct pollfd, revents));

	say_text("-- select and pselect6");
	fd_set rd, wr;
	FD_ZERO(&rd);
	FD_ZERO(&wr);
	FD_SET(p[0], &rd);
	FD_SET(p[1], &wr);
	struct {
		i64 sec, usec;
	} tv = { 0, 0 };
	int maxfd = (p[0] > p[1] ? p[0] : p[1]) + 1;
	r = sys5(SYS_select, maxfd, &rd, &wr, 0, &tv);
	say_ret("select(read end empty, write end)", r);
	say_bool("  read end set", FD_ISSET(p[0], &rd));
	say_bool("  write end set", FD_ISSET(p[1], &wr));
	write_str(p[1], "y");
	FD_ZERO(&rd);
	FD_SET(p[0], &rd);
	r = sys5(SYS_select, maxfd, &rd, 0, 0, &tv);
	say_ret("select(read end with data)", r);
	say_bool("  read end set", FD_ISSET(p[0], &rd));
	sys3(SYS_read, p[0], buf, 1);
	FD_ZERO(&rd);
	FD_SET(p[0], &rd);
	tv.sec = 0;
	tv.usec = 30000;
	t0 = now_ns(1);
	r = sys5(SYS_select, maxfd, &rd, 0, 0, &tv);
	el = now_ns(1) - t0;
	say_ret("select(empty, 30 ms)", r);
	say_bool("  waited >= 30 ms", el >= 30000000);
	say_bool("  timeval updated to 0", tv.sec == 0 && tv.usec == 0);
	say_bool("  read end cleared", !FD_ISSET(p[0], &rd));
	FD_ZERO(&rd);
	FD_SET(100, &rd);
	tv.usec = 0;
	say_ret("select(fd 100, beyond the descriptor table: ignored)", sys5(SYS_select, 101, &rd, 0, 0, &tv));
	FD_ZERO(&rd);
	FD_SET(20, &rd);
	say_ret("select(fd 20, closed)", sys5(SYS_select, 21, &rd, 0, 0, &tv));
	say_ret("select(nfds -1)", sys5(SYS_select, -1, 0, 0, 0, &tv));
	tv.usec = 1000000;
	say_ret("select(tv_usec = 1e6)", sys5(SYS_select, 0, 0, 0, 0, &tv));
	FD_ZERO(&rd);
	FD_SET(p[0], &rd);
	struct lx_ts pts = { 0, 30000000 };
	/* pselect6's sixth argument: { const sigset_t *ss; size_t ss_len }. */
	u64 usr1 = 1UL << (SIGUSR1 - 1);
	struct {
		u64 *set;
		u64 len;
	} sig = { &usr1, 8 };
	t0 = now_ns(1);
	r = sys6(SYS_pselect6, maxfd, (i64)&rd, 0, 0, (i64)&pts, (i64)&sig);
	el = now_ns(1) - t0;
	say_ret("pselect6(empty, 30 ms, SIGUSR1 blocked meanwhile)", r);
	say_bool("  waited >= 30 ms", el >= 30000000);
	u64 m = 0;
	sys4(SYS_rt_sigprocmask, SIG_BLOCK, 0, &m, 8);
	say_hex("  mask after (restored)", m);
	write_str(p[1], "z");
	FD_ZERO(&rd);
	FD_SET(p[0], &rd);
	pts.nsec = 0;
	r = sys6(SYS_pselect6, maxfd, (i64)&rd, 0, 0, (i64)&pts, 0);
	say_ret("pselect6(read end with data, 0)", r);
	say_bool("  read end set", FD_ISSET(p[0], &rd));
	sys3(SYS_read, p[0], buf, 1);
	sig.len = 4;
	say_ret("pselect6(sigset size 4)", sys6(SYS_pselect6, maxfd, (i64)&rd, 0, 0, (i64)&pts, (i64)&sig));

	say_text("-- epoll");
	i64 ep = sys1(SYS_epoll_create1, EPOLL_CLOEXEC);
	say_ok("epoll_create1(EPOLL_CLOEXEC)", ep);
	say_ret("  F_GETFD", sys2(SYS_fcntl, ep, F_GETFD));
	say_ret("epoll_create1(bad flags)", sys1(SYS_epoll_create1, 1));
	say_ret("epoll_create(0)", sys1(SYS_epoll_create, 0));
	i64 ep2 = sys1(SYS_epoll_create, 1);
	say_ok("epoll_create(1)", ep2);
	sys1(SYS_close, ep2);
	struct epoll_event ev[8];
	say_ret("ADD read end (IN), data 1", ctl(ep, EPOLL_CTL_ADD, p[0], EPOLLIN, 1));
	say_ret("ADD read end again", ctl(ep, EPOLL_CTL_ADD, p[0], EPOLLIN, 1));
	say_ret("MOD write end (not added)", ctl(ep, EPOLL_CTL_MOD, p[1], EPOLLOUT, 2));
	say_ret("DEL write end (not added)", ctl(ep, EPOLL_CTL_DEL, p[1], 0, 0));
	say_ret("ADD 999", ctl(ep, EPOLL_CTL_ADD, 999, EPOLLIN, 9));
	say_ret("ADD the epoll fd to itself", ctl(ep, EPOLL_CTL_ADD, ep, EPOLLIN, 9));
	i64 reg = sys4(SYS_openat, AT_FDCWD, "/", O_RDONLY | O_DIRECTORY, 0);
	say_ret("ADD a directory (no poll support)", ctl(ep, EPOLL_CTL_ADD, reg, EPOLLIN, 9));
	sys1(SYS_close, reg);
	say_ret("op 99", ctl(ep, 99, p[0], EPOLLIN, 1));
	say_ret("epoll_ctl on a pipe as the epoll fd", ctl(p[0], EPOLL_CTL_ADD, p[1], EPOLLOUT, 1));
	say_epoll("epoll_wait(empty, 0)", sys4(SYS_epoll_wait, ep, ev, 8, 0), ev);
	write_str(p[1], "ab");
	say_epoll("epoll_wait(data)", sys4(SYS_epoll_wait, ep, ev, 8, 0), ev);
	say_epoll("epoll_wait again (level-triggered: still)", sys4(SYS_epoll_wait, ep, ev, 8, 0), ev);
	say_ret("ADD write end (OUT), data 2", ctl(ep, EPOLL_CTL_ADD, p[1], EPOLLOUT, 2));
	i64 n = sys4(SYS_epoll_wait, ep, ev, 8, 0);
	/* Order between ready fds is not specified: sort by data. */
	if (n == 2 && ev[0].data.u64 > ev[1].data.u64) {
		struct epoll_event t = ev[0];
		ev[0] = ev[1];
		ev[1] = t;
	}
	say_epoll("epoll_wait(both)", n, ev);
	say_epoll("epoll_wait(both, maxevents 1)", sys4(SYS_epoll_wait, ep, ev, 1, 0), ev);
	say_ret("epoll_wait(maxevents 0)", sys4(SYS_epoll_wait, ep, ev, 0, 0));
	say_ret("epoll_wait(not an epoll fd)", sys4(SYS_epoll_wait, p[0], ev, 8, 0));
	say_ret("epoll_wait(bad pointer)", sys4(SYS_epoll_wait, ep, 8, 8, 0));
	say_ret("MOD read end to IN|ET, data 3", ctl(ep, EPOLL_CTL_MOD, p[0], EPOLLIN | EPOLLET, 3));
	say_ret("DEL write end", ctl(ep, EPOLL_CTL_DEL, p[1], 0, 0));
	say_epoll("EPOLLET: epoll_wait (data was there before)", sys4(SYS_epoll_wait, ep, ev, 8, 0), ev);
	say_epoll("EPOLLET: epoll_wait again (no new data)", sys4(SYS_epoll_wait, ep, ev, 8, 0), ev);
	write_str(p[1], "c");
	say_epoll("EPOLLET: after a new write", sys4(SYS_epoll_wait, ep, ev, 8, 0), ev);
	say_epoll("EPOLLET: again", sys4(SYS_epoll_wait, ep, ev, 8, 0), ev);
	say_read("read everything", sys3(SYS_read, p[0], buf, sizeof buf), buf);
	say_ret("MOD read end to IN|ONESHOT, data 4", ctl(ep, EPOLL_CTL_MOD, p[0], EPOLLIN | EPOLLONESHOT, 4));
	write_str(p[1], "d");
	say_epoll("EPOLLONESHOT: epoll_wait", sys4(SYS_epoll_wait, ep, ev, 8, 0), ev);
	say_epoll("EPOLLONESHOT: again (disabled)", sys4(SYS_epoll_wait, ep, ev, 8, 0), ev);
	write_str(p[1], "e");
	say_epoll("EPOLLONESHOT: after another write (still disabled)", sys4(SYS_epoll_wait, ep, ev, 8, 0), ev);
	say_ret("ADD it again (it is still there)", ctl(ep, EPOLL_CTL_ADD, p[0], EPOLLIN, 5));
	say_ret("MOD re-arms it, data 5", ctl(ep, EPOLL_CTL_MOD, p[0], EPOLLIN | EPOLLONESHOT, 5));
	say_epoll("EPOLLONESHOT: after MOD", sys4(SYS_epoll_wait, ep, ev, 8, 0), ev);
	sys3(SYS_read, p[0], buf, sizeof buf);
	say_ret("MOD read end to IN, data 6", ctl(ep, EPOLL_CTL_MOD, p[0], EPOLLIN, 6));
	t0 = now_ns(1);
	n = sys4(SYS_epoll_wait, ep, ev, 8, 30);
	el = now_ns(1) - t0;
	say_epoll("epoll_wait(empty, 30 ms)", n, ev);
	say_bool("  waited >= 30 ms", el >= 30000000);
	t0 = now_ns(1);
	n = sys6(SYS_epoll_pwait, ep, (i64)ev, 8, 30, (i64)&usr1, 8);
	el = now_ns(1) - t0;
	say_epoll("epoll_pwait(empty, 30 ms, SIGUSR1 blocked meanwhile)", n, ev);
	say_bool("  waited >= 30 ms", el >= 30000000);
	pid = lx_fork();
	if (pid == 0) {
		sleep_ms(30);
		write_str(p[1], "f");
		lx_exit(0);
	}
	say_epoll("epoll_wait(-1) woken by a child's write", sys4(SYS_epoll_wait, ep, ev, 8, -1), ev);
	wait_for(pid, 0);
	sys1(SYS_close, p[1]);
	say_epoll("writer closed, data left", sys4(SYS_epoll_wait, ep, ev, 8, 0), ev);
	sys3(SYS_read, p[0], buf, sizeof buf);
	say_epoll("writer closed, empty", sys4(SYS_epoll_wait, ep, ev, 8, 0), ev);
	sys1(SYS_close, p[0]);
	say_epoll("read end closed: removed from the set", sys4(SYS_epoll_wait, ep, ev, 8, 0), ev);

	/* The packed layout: two ready entries, the second's data 12 bytes
	 * after the first's events. */
	sys2(SYS_pipe2, q, 0);
	write_str(q[1], "g");
	ctl(ep, EPOLL_CTL_ADD, q[0], EPOLLIN, 0x1111);
	ctl(ep, EPOLL_CTL_ADD, q[1], EPOLLOUT, 0x2222);
	static unsigned char raw[64];
	lx_memset(raw, 0xaa, sizeof raw);
	n = sys4(SYS_epoll_wait, ep, raw, 4, 0);
	say_ret("epoll_wait(two ready, raw buffer)", n);
	int pos[2] = { -1, -1 };
	for (int off = 0; off + 8 <= 40; off++) {
		u64 v = 0;
		for (int k = 7; k >= 0; k--)
			v = v << 8 | raw[off + k];
		if (v == 0x1111 || v == 0x2222)
			pos[v == 0x2222] = off;
	}
	int stride = pos[0] > pos[1] ? pos[0] - pos[1] : pos[1] - pos[0];
	say("  data's offset in the first entry", pos[0] < pos[1] ? pos[0] : pos[1]);
	say("  observed stride between entries (bytes)", stride);
	say("  bytes written", written_size(raw, sizeof raw, 0xaa));
	say("sizeof(struct epoll_event)", sizeof(struct epoll_event));
	say("offsetof(epoll_event, data)", offsetof(struct epoll_event, data));
	sys1(SYS_close, ep);
	say_text("poll_test: ok");
	return 0;
}
