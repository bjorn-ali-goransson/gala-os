/* sysinfo_test: sysinfo(2) and sched_getaffinity(2), which GIMP and GLib
 * call at start (ROADMAP LD step 9: both ENOSYS in gala). glibc's
 * sysconf(_SC_PHYS_PAGES) is sysinfo's totalram, which GIMP sizes its
 * undo memory and tile cache from; GLib's g_get_num_processors counts
 * the affinity mask's CPUs, which GEGL sizes its thread pool from.
 *
 * Memory, load and CPU counts differ between machines: the test prints
 * relations that hold on any of them. sysinfo counts in bytes (mem_unit
 * 1 on a 64-bit kernel), whole pages of RAM, free at most the total,
 * uptime not going back, at least one process; the affinity mask is a
 * whole number of longs at most the buffer, with at least one CPU, the
 * same for pid 0 and our own pid; and the errors. */
#include "../common/lx.h"
#include "../common/more.h"

/* struct sysinfo on x86-64 (sysinfo(2)): 112 bytes. */
struct lx_sysinfo {
	i64 uptime;
	u64 loads[3];
	u64 totalram, freeram, sharedram, bufferram, totalswap, freeswap;
	unsigned short procs, pad;
	u64 totalhigh, freehigh;
	unsigned int mem_unit;
};

static int bits(const unsigned char *m, i64 n)
{
	int c = 0;
	for (i64 i = 0; i < n; i++)
		for (int b = 0; b < 8; b++)
			c += (m[i] >> b) & 1;
	return c;
}

int main(void)
{
	lx_init();
	say("sizeof(struct sysinfo)", sizeof(struct lx_sysinfo));
	struct lx_sysinfo a, b;
	lx_memset(&a, 0x55, sizeof a);
	say_ret("sysinfo", sys1(SYS_sysinfo, &a));
	say("  mem_unit", a.mem_unit);
	say_bool("  totalram > 0", a.totalram > 0);
	say_bool("  totalram is whole pages", a.totalram % 4096 == 0);
	say_bool("  freeram <= totalram", a.freeram <= a.totalram);
	say_bool("  sharedram <= totalram", a.sharedram <= a.totalram);
	say_bool("  bufferram <= totalram", a.bufferram <= a.totalram);
	say_bool("  freeswap <= totalswap", a.freeswap <= a.totalswap);
	say_bool("  no high memory", a.totalhigh == 0 && a.freehigh == 0);
	say_bool("  uptime >= 0", a.uptime >= 0);
	say_bool("  procs >= 1", a.procs >= 1);
	/* pad, then the holes after it (84..88) and after mem_unit (108..112):
	 * Linux clears the whole structure. */
	const unsigned char *raw = (const unsigned char *)&a;
	say_bool("  the padding is zero",
		 a.pad == 0 && bits(raw + 84, 4) == 0 && bits(raw + 108, 4) == 0);
	lx_memset(&b, 0x55, sizeof b);
	sys1(SYS_sysinfo, &b);
	say_bool("a second call: uptime not less", b.uptime >= a.uptime);
	say_bool("a second call: the same totalram", b.totalram == a.totalram);
	say_ret("sysinfo(NULL)", sys1(SYS_sysinfo, 0));
	say_ret("sysinfo(bad pointer)", sys1(SYS_sysinfo, 1));

	say_text("-- sched_getaffinity");
	unsigned char m[128], m2[128];
	lx_memset(m, 0x55, sizeof m);
	i64 n = sys3(SYS_sched_getaffinity, 0, sizeof m, m);
	say_bool("sched_getaffinity(0, 128) > 0", n > 0);
	say_bool("  a whole number of longs", n > 0 && n % 8 == 0);
	say_bool("  at most the buffer", n <= (i64)sizeof m);
	say_bool("  at least one CPU", n > 0 && bits(m, n) >= 1);
	say_bool("  the rest of the buffer untouched", n > 0 && m[n] == 0x55 && m[sizeof m - 1] == 0x55);
	lx_memset(m2, 0, sizeof m2);
	i64 n2 = sys3(SYS_sched_getaffinity, sys0(SYS_getpid), sizeof m2, m2);
	say_bool("our own pid: the same mask", n2 == n && n > 0 && lx_memeq(m, m2, n));
	lx_memset(m2, 0, sizeof m2);
	i64 n3 = sys3(SYS_sched_getaffinity, sys0(SYS_gettid), sizeof m2, m2);
	say_bool("our own tid: the same mask", n3 == n && n > 0 && lx_memeq(m, m2, n));
	say_ret("sched_getaffinity(size 4)", sys3(SYS_sched_getaffinity, 0, 4, m));
	say_ret("sched_getaffinity(size 12)", sys3(SYS_sched_getaffinity, 0, 12, m));
	say_ret("sched_getaffinity(no such pid)", sys3(SYS_sched_getaffinity, 0x3ffffff0, sizeof m, m));
	say_ret("sched_getaffinity(bad pointer)", sys3(SYS_sched_getaffinity, 0, sizeof m, 1));
	say_text("sysinfo_test: ok");
	return 0;
}
