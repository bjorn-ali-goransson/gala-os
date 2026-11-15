/* timerfd_test: timerfd_create(2), timerfd_settime(2), timerfd_gettime(2)
 * and reading, polling and epoll on a timerfd (ROADMAP LD: Ubuntu 26.04's
 * glycin, through Rust's async-io, waits with one in epoll; ENOSYS in
 * gala).
 *
 * Only what timing cannot change is printed: a timer never expires early
 * (a blocking read or a poll takes at least the time armed, measured from
 * before the arming call so that what is printed in between does not come
 * off the wait: a loaded machine only makes it later), a one-shot timer
 * counts one expiry, an interval timer at least one per read, an absolute
 * time already past expires with no deadline left to wait for (polled
 * for: the expiry is posted by the timer subsystem, not by the call that
 * arms it),
 * a disarmed timer reads nothing, the remaining time is at most what was
 * armed, and the errors. */
#include "../common/lx.h"
#include "../common/more.h"

#define CLOCK_REALTIME 0
#define CLOCK_MONOTONIC 1
#define CLOCK_BOOTTIME 7
#define TFD_NONBLOCK 04000
#define TFD_CLOEXEC 02000000
#define TFD_TIMER_ABSTIME 1
#define F_GETFD 1
#define F_GETFL 3
#define POLLIN 1

struct ts {
	i64 sec, nsec;
};
struct its {
	struct ts interval, value;
};
struct pfd {
	int fd;
	short events, revents;
};
struct epev {
	unsigned events;
	u64 data;
} __attribute__((packed));

static i64 ms(int clock)
{
	return now_ns(clock) / 1000000;
}

static struct its arm(i64 value_ms, i64 interval_ms)
{
	struct its t = { { interval_ms / 1000, (interval_ms % 1000) * 1000000 },
			 { value_ms / 1000, (value_ms % 1000) * 1000000 } };
	return t;
}

static u64 rd(int fd, i64 *r)
{
	u64 n = 0;
	*r = sys3(SYS_read, fd, &n, 8);
	return n;
}

int main(void)
{
	lx_init();
	i64 fd = sys2(SYS_timerfd_create, CLOCK_MONOTONIC, 0);
	say_bool("timerfd_create(CLOCK_MONOTONIC) >= 0", fd >= 0);
	say("  F_GETFD", sys2(SYS_fcntl, fd, F_GETFD));
	say_bool("  O_NONBLOCK", (sys2(SYS_fcntl, fd, F_GETFL) & TFD_NONBLOCK) != 0);
	struct its cur, old, t;
	lx_memset(&cur, 0x55, sizeof cur);
	say_ret("timerfd_gettime(unarmed)", sys2(SYS_timerfd_gettime, fd, &cur));
	say_bool("  all zero", cur.value.sec == 0 && cur.value.nsec == 0 && cur.interval.sec == 0 && cur.interval.nsec == 0);

	say_text("-- a one-shot timer, a blocking read");
	t = arm(60, 0);
	i64 start = ms(CLOCK_MONOTONIC);
	say_ret("timerfd_settime(60 ms)", sys4(SYS_timerfd_settime, fd, 0, &t, 0));
	sys2(SYS_timerfd_gettime, fd, &cur);
	say_bool("  remaining in (0, 60 ms]", (cur.value.sec > 0 || cur.value.nsec > 0) && cur.value.sec == 0 && cur.value.nsec <= 60000000);
	say_bool("  no interval", cur.interval.sec == 0 && cur.interval.nsec == 0);
	i64 r;
	u64 n = rd(fd, &r);
	i64 took = ms(CLOCK_MONOTONIC) - start;
	say("read", r);
	say("  expirations", n);
	say_bool("  not before 60 ms", took >= 60);
	sys2(SYS_timerfd_gettime, fd, &cur);
	say_bool("expired: disarmed", cur.value.sec == 0 && cur.value.nsec == 0);

	say_text("-- non-blocking, poll and epoll");
	i64 nb = sys2(SYS_timerfd_create, CLOCK_MONOTONIC, TFD_NONBLOCK | TFD_CLOEXEC);
	say("  F_GETFD", sys2(SYS_fcntl, nb, F_GETFD));
	say_bool("  O_NONBLOCK", (sys2(SYS_fcntl, nb, F_GETFL) & TFD_NONBLOCK) != 0);
	rd(nb, &r);
	say_ret("read(unarmed, non-blocking)", r);
	t = arm(30, 0);
	struct pfd p = { (int)nb, POLLIN, 0 };
	/* Before the arming call, as the 60 ms read above: what the lines in
	 * between cost comes off the wait otherwise, and a machine whose
	 * console is slower than the slack polls for less than the 30 ms
	 * (gala's serial console: lx_timerfd). */
	start = ms(CLOCK_MONOTONIC);
	sys4(SYS_timerfd_settime, nb, 0, &t, 0);
	rd(nb, &r);
	say_ret("read before it expires", r);
	say("poll(10 s)", sys3(SYS_poll, &p, 1, 10000));
	say("  revents", p.revents);
	say_bool("  not before 30 ms", ms(CLOCK_MONOTONIC) - start >= 30);
	n = rd(nb, &r);
	say("read", r);
	say("  expirations", n);
	i64 ep = sys1(SYS_epoll_create1, 0);
	struct epev ev = { POLLIN, 7 }, got = { 0, 0 };
	say_ret("epoll_ctl(ADD)", sys4(SYS_epoll_ctl, ep, 1, nb, &ev));
	say("epoll_wait(disarmed, 50 ms)", sys4(SYS_epoll_wait, ep, &got, 1, 50));
	t = arm(20, 0);
	sys4(SYS_timerfd_settime, nb, 0, &t, 0);
	say("epoll_wait(armed, 10 s)", sys4(SYS_epoll_wait, ep, &got, 1, 10000));
	say("  events", got.events);
	say("  data", (i64)got.data);
	rd(nb, &r);
	say("epoll_wait after the read (50 ms)", sys4(SYS_epoll_wait, ep, &got, 1, 50));

	say_text("-- an interval timer");
	t = arm(20, 20);
	sys4(SYS_timerfd_settime, fd, 0, &t, 0);
	sys2(SYS_timerfd_gettime, fd, &cur);
	say_bool("its interval", cur.interval.sec == 0 && cur.interval.nsec == 20000000);
	n = rd(fd, &r);
	say_bool("a read: at least one expiry", r == 8 && n >= 1);
	start = ms(CLOCK_MONOTONIC);
	n = rd(fd, &r);
	say_bool("another read: at least one more", r == 8 && n >= 1);
	sys2(SYS_timerfd_gettime, fd, &cur);
	say_bool("still armed", cur.value.sec > 0 || cur.value.nsec > 0);
	t = arm(0, 0);
	lx_memset(&old, 0x55, sizeof old);
	say_ret("disarmed (0, 0)", sys4(SYS_timerfd_settime, fd, 0, &t, &old));
	say_bool("  the old setting's interval", old.interval.sec == 0 && old.interval.nsec == 20000000);
	sys3(SYS_fcntl, fd, 4 /* F_SETFL */, TFD_NONBLOCK);
	rd(fd, &r);
	say_ret("  read", r);

	say_text("-- absolute times");
	struct its a = { { 0, 0 }, { 0, 0 } };
	i64 now = now_ns(CLOCK_MONOTONIC);
	/* "Now" read before the call: already past when it arms (a time a
	 * second earlier is negative on a machine up less than a second). */
	a.value.sec = now / 1000000000;
	a.value.nsec = now % 1000000000;
	say_ret("TFD_TIMER_ABSTIME, the time just read", sys4(SYS_timerfd_settime, fd, TFD_TIMER_ABSTIME, &a, 0));
	/* It expires with no deadline left to wait for, and poll is what says
	 * so without racing the expiry: fd is non-blocking from here on, and
	 * a read on the very next instruction can reach the timer before the
	 * timer subsystem has posted the expiry (EAGAIN, and 0 expirations
	 * for a time a whole second past). */
	p.fd = (int)fd;
	p.revents = 0;
	say("  poll(10 s)", sys3(SYS_poll, &p, 1, 10000));
	say("  revents", p.revents);
	n = rd(fd, &r);
	say("  read", r);
	say("  expirations", n);
	i64 rt = sys2(SYS_timerfd_create, CLOCK_REALTIME, TFD_NONBLOCK);
	now = now_ns(CLOCK_REALTIME);
	a.value.sec = now / 1000000000 - 1;
	say_ret("CLOCK_REALTIME, TFD_TIMER_ABSTIME, a second ago", sys4(SYS_timerfd_settime, rt, TFD_TIMER_ABSTIME, &a, 0));
	p.fd = (int)rt;
	p.revents = 0;
	say("  poll(10 s)", sys3(SYS_poll, &p, 1, 10000));
	say("  revents", p.revents);
	n = rd(rt, &r);
	say("  expirations", n);
	i64 bt = sys2(SYS_timerfd_create, CLOCK_BOOTTIME, TFD_NONBLOCK);
	say_bool("timerfd_create(CLOCK_BOOTTIME) >= 0", bt >= 0);

	say_text("-- errors");
	say_ret("timerfd_create(clock 99)", sys2(SYS_timerfd_create, 99, 0));
	say_ret("timerfd_create(bad flags)", sys2(SYS_timerfd_create, CLOCK_MONOTONIC, 1));
	t = arm(10, 0);
	t.value.nsec = 1000000000;
	say_ret("settime(nsec 1e9)", sys4(SYS_timerfd_settime, fd, 0, &t, 0));
	t = arm(10, 0);
	t.value.sec = -1;
	say_ret("settime(a negative second)", sys4(SYS_timerfd_settime, fd, 0, &t, 0));
	t = arm(10, 0);
	say_ret("settime(flags 4)", sys4(SYS_timerfd_settime, fd, 4, &t, 0));
	say_ret("settime(bad value pointer)", sys4(SYS_timerfd_settime, fd, 0, 1, 0));
	say_ret("gettime(bad pointer)", sys2(SYS_timerfd_gettime, fd, 1));
	say_ret("settime(not a timerfd)", sys4(SYS_timerfd_settime, ep, 0, &t, 0));
	say_ret("gettime(no descriptor)", sys2(SYS_timerfd_gettime, 99, &cur));
	u64 small;
	say_ret("read(4 bytes)", sys3(SYS_read, nb, &small, 4));
	say_ret("write", sys3(SYS_write, nb, &small, 8));
	say_text("timerfd_test: ok");
	return 0;
}
