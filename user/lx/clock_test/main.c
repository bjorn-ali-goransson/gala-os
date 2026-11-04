/* clock_test: clock_gettime/clock_getres/clock_nanosleep/nanosleep/
 * gettimeofday/time as the raw system calls give them (the syscall
 * instruction, not the vDSO). Times vary, so only relations are printed:
 * MONOTONIC never goes back, REALTIME is a plausible date, BOOTTIME is not
 * behind MONOTONIC, the CPU-time clocks advance with work, a sleep lasts
 * at least what was asked; and the errors (EINVAL, EFAULT). */
#define _GNU_SOURCE
#include "../common/lx.h"
#include "../common/more.h"
#include <time.h>

#define CLOCK_REALTIME_ 0
#define CLOCK_MONOTONIC_ 1
#define CLOCK_PROCESS_CPUTIME_ID_ 2
#define CLOCK_THREAD_CPUTIME_ID_ 3
#define CLOCK_MONOTONIC_RAW_ 4
#define CLOCK_BOOTTIME_ 7
#define TIMER_ABSTIME_ 1

static i64 ns(struct lx_ts *t)
{
	return t->sec * 1000000000 + t->nsec;
}

static void say_res(const char *name, int clock)
{
	struct lx_ts t = { -1, -1 };
	i64 r = sys2(SYS_clock_getres, clock, &t);
	put(name);
	put(" = ");
	put_ret(r);
	if (r == 0) {
		put(", ");
		put_dec(t.sec);
		put(" s ");
		put_dec(t.nsec);
		put(" ns");
	}
	end_line();
}

static void busy(void)
{
	volatile u64 x = 0;
	for (u64 i = 0; i < 20000000; i++)
		x += i;
}

int main(void)
{
	struct lx_ts a, b;
	lx_init();

	say_text("-- clock_gettime");
	int ok = 1;
	i64 prev = now_ns(CLOCK_MONOTONIC_);
	for (int i = 0; i < 10000; i++) {
		i64 t = now_ns(CLOCK_MONOTONIC_);
		if (t < prev)
			ok = 0;
		prev = t;
	}
	say_bool("MONOTONIC never goes back (10000 reads)", ok);
	say_ret("clock_gettime(REALTIME)", sys2(SYS_clock_gettime, CLOCK_REALTIME_, &a));
	say_bool("  after 2020-01-01 and before 2100-01-01", a.sec > 1577836800L && a.sec < 4102444800L);
	say_bool("  tv_nsec < 1e9", a.nsec >= 0 && a.nsec < 1000000000);
	i64 mono = now_ns(CLOCK_MONOTONIC_);
	say_ret("clock_gettime(BOOTTIME)", sys2(SYS_clock_gettime, CLOCK_BOOTTIME_, &b));
	say_bool("  BOOTTIME >= MONOTONIC", ns(&b) >= mono);
	say_ret("clock_gettime(MONOTONIC_RAW)", sys2(SYS_clock_gettime, CLOCK_MONOTONIC_RAW_, &b));
	say_bool("  > 0", ns(&b) > 0);
	i64 p0 = now_ns(CLOCK_PROCESS_CPUTIME_ID_), t0 = now_ns(CLOCK_THREAD_CPUTIME_ID_);
	i64 m0 = now_ns(CLOCK_MONOTONIC_);
	busy();
	i64 p1 = now_ns(CLOCK_PROCESS_CPUTIME_ID_), t1 = now_ns(CLOCK_THREAD_CPUTIME_ID_);
	i64 m1 = now_ns(CLOCK_MONOTONIC_);
	say_bool("PROCESS_CPUTIME advanced during a busy loop", p1 > p0);
	say_bool("THREAD_CPUTIME advanced during a busy loop", t1 > t0);
	say_bool("  CPU time used <= wall time passed (+1 ms)", t1 - t0 <= m1 - m0 + 1000000);
	say_ret("clock_gettime(100)", sys2(SYS_clock_gettime, 100, &a));
	say_ret("clock_gettime(-1)", sys2(SYS_clock_gettime, -1, &a));
	say_ret("clock_gettime(MONOTONIC, bad pointer)", sys2(SYS_clock_gettime, CLOCK_MONOTONIC_, 8));
	/* A process's CPU-time clock by pid: (~pid << 3) | 2, as a 32-bit
	 * clockid_t. 0x7ffffff is above any pid_max. */
	i64 self_clock = (int)((~(unsigned)sys0(SYS_getpid) << 3) | 2);
	say_ret("clock_gettime(this process's CPU clock by pid)", sys2(SYS_clock_gettime, self_clock, &a));
	say_ret("clock_gettime(a nonexistent process's CPU clock)", sys2(SYS_clock_gettime, (int)((~0x7ffffffU << 3) | 2), &a));

	say_text("-- clock_getres");
	say_res("clock_getres(REALTIME)", CLOCK_REALTIME_);
	say_res("clock_getres(MONOTONIC)", CLOCK_MONOTONIC_);
	say_res("clock_getres(BOOTTIME)", CLOCK_BOOTTIME_);
	say_res("clock_getres(PROCESS_CPUTIME_ID)", CLOCK_PROCESS_CPUTIME_ID_);
	say_res("clock_getres(THREAD_CPUTIME_ID)", CLOCK_THREAD_CPUTIME_ID_);
	say_res("clock_getres(100)", 100);
	say_ret("clock_getres(MONOTONIC, NULL)", sys2(SYS_clock_getres, CLOCK_MONOTONIC_, 0));

	say_text("-- gettimeofday and time");
	struct {
		i64 sec, usec;
	} tv;
	i64 rt0 = now_ns(CLOCK_REALTIME_);
	say_ret("gettimeofday", sys2(SYS_gettimeofday, &tv, 0));
	i64 rt1 = now_ns(CLOCK_REALTIME_);
	i64 tvns = tv.sec * 1000000000 + tv.usec * 1000;
	say_bool("  between two REALTIME reads (to the microsecond)", tvns >= rt0 / 1000 * 1000 && tvns <= rt1);
	say_bool("  tv_usec < 1e6", tv.usec >= 0 && tv.usec < 1000000);
	i64 t = 0;
	i64 r = sys1(SYS_time, &t);
	say_bool("time() returns what it stores", r == t);
	say_bool("  within 2 s of REALTIME", r >= rt0 / 1000000000 - 2 && r <= rt1 / 1000000000 + 2);

	say_text("-- nanosleep and clock_nanosleep");
	struct lx_ts req = { 0, 20000000 }, rem = { -1, -1 };
	i64 s0 = now_ns(CLOCK_MONOTONIC_);
	r = sys2(SYS_nanosleep, &req, &rem);
	i64 s1 = now_ns(CLOCK_MONOTONIC_);
	say_ret("nanosleep(20 ms)", r);
	say_bool("  slept >= 20 ms", s1 - s0 >= 20000000);
	say_bool("  slept < 5 s", s1 - s0 < 5000000000L);
	say_bool("  rem left alone", rem.sec == -1 && rem.nsec == -1);
	req.sec = 0;
	req.nsec = 0;
	say_ret("nanosleep(0)", sys2(SYS_nanosleep, &req, 0));
	req.nsec = 1000000000;
	say_ret("nanosleep(tv_nsec = 1e9)", sys2(SYS_nanosleep, &req, 0));
	req.nsec = -1;
	say_ret("nanosleep(tv_nsec = -1)", sys2(SYS_nanosleep, &req, 0));
	req.sec = -1;
	req.nsec = 0;
	say_ret("nanosleep(tv_sec = -1)", sys2(SYS_nanosleep, &req, 0));
	say_ret("nanosleep(bad pointer)", sys2(SYS_nanosleep, 8, 0));
	req.sec = 0;
	req.nsec = 20000000;
	s0 = now_ns(CLOCK_MONOTONIC_);
	r = sys4(SYS_clock_nanosleep, CLOCK_MONOTONIC_, 0, &req, 0);
	s1 = now_ns(CLOCK_MONOTONIC_);
	say_ret("clock_nanosleep(MONOTONIC, 20 ms)", r);
	say_bool("  slept >= 20 ms", s1 - s0 >= 20000000);
	i64 deadline = now_ns(CLOCK_MONOTONIC_) + 30000000;
	struct lx_ts abs = { deadline / 1000000000, deadline % 1000000000 };
	r = sys4(SYS_clock_nanosleep, CLOCK_MONOTONIC_, TIMER_ABSTIME_, &abs, 0);
	say_ret("clock_nanosleep(MONOTONIC, TIMER_ABSTIME, now + 30 ms)", r);
	say_bool("  woke at or after the deadline", now_ns(CLOCK_MONOTONIC_) >= deadline);
	abs.sec = 1;
	abs.nsec = 0;
	s0 = now_ns(CLOCK_MONOTONIC_);
	say_ret("clock_nanosleep(TIMER_ABSTIME, in the past)", sys4(SYS_clock_nanosleep, CLOCK_MONOTONIC_, TIMER_ABSTIME_, &abs, 0));
	say_bool("  returned at once (< 1 s)", now_ns(CLOCK_MONOTONIC_) - s0 < 1000000000);
	i64 rdl = now_ns(CLOCK_REALTIME_) + 10000000;
	struct lx_ts rabs = { rdl / 1000000000, rdl % 1000000000 };
	say_ret("clock_nanosleep(REALTIME, TIMER_ABSTIME, now + 10 ms)", sys4(SYS_clock_nanosleep, CLOCK_REALTIME_, TIMER_ABSTIME_, &rabs, 0));
	struct lx_ts ms1 = { 0, 1000000 };
	say_ret("clock_nanosleep(BOOTTIME, 1 ms)", sys4(SYS_clock_nanosleep, CLOCK_BOOTTIME_, 0, &ms1, 0));
	say_ret("clock_nanosleep(THREAD_CPUTIME_ID)", sys4(SYS_clock_nanosleep, CLOCK_THREAD_CPUTIME_ID_, 0, &req, 0));
	say_ret("clock_nanosleep(100)", sys4(SYS_clock_nanosleep, 100, 0, &req, 0));
	req.nsec = 1000000000;
	say_ret("clock_nanosleep(tv_nsec = 1e9)", sys4(SYS_clock_nanosleep, CLOCK_MONOTONIC_, 0, &req, 0));
	say("sizeof(struct timespec)", sizeof(struct timespec));
	static unsigned char buf[64];
	lx_memset(buf, 0xaa, sizeof buf);
	sys2(SYS_clock_gettime, CLOCK_MONOTONIC_, buf);
	say_bool("clock_gettime writes 16 bytes or fewer", written_size(buf, sizeof buf, 0xaa) <= 16 && written_size(buf, sizeof buf, 0xaa) > 8);
	say_text("clock_test: ok");
	return 0;
}
