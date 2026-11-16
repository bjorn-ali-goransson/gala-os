/* Shared helpers for gala's Linux test programs (user/lx, ROADMAP LX).
 *
 * Every system call a test checks is made here with the `syscall`
 * instruction (no libc wrapper, no errno), so a transcript shows what the
 * kernel did, not what libc made of it: a failure is the negative errno
 * the kernel returned. musl is linked statically for its start-up code
 * (crt1.o / rcrt1.o: argc, argv, TLS) only.
 *
 * Output: one write(2) per line (a line written in pieces can be split by
 * kernel log lines on a serial console) and no addresses (they vary
 * between runs and builds), so the transcript is the same on every run;
 * `cargo xtask lx-expect` compares it with the test's expected.txt.
 */
#ifndef LX_H
#define LX_H

typedef unsigned long u64;
typedef long i64;

/* struct epoll_event is packed on x86_64 only (Linux's EPOLL_PACKED). */
#if defined(__x86_64__)
#define LX_EPOLL_PACKED __attribute__((packed))
#else
#define LX_EPOLL_PACKED
#endif

#if defined(__x86_64__)
static inline i64 sys6(i64 n, i64 a, i64 b, i64 c, i64 d, i64 e, i64 f)
{
	i64 ret;
	register i64 r10 __asm__("r10") = d;
	register i64 r8 __asm__("r8") = e;
	register i64 r9 __asm__("r9") = f;
	__asm__ volatile("syscall"
			 : "=a"(ret)
			 : "a"(n), "D"(a), "S"(b), "d"(c), "r"(r10), "r"(r8), "r"(r9)
			 : "rcx", "r11", "memory");
	return ret;
}
#elif defined(__aarch64__)
#include <sys/syscall.h>
static inline i64 lx_svc(i64 n, i64 a, i64 b, i64 c, i64 d, i64 e, i64 f)
{
	register i64 x8 __asm__("x8") = n;
	register i64 x0 __asm__("x0") = a;
	register i64 x1 __asm__("x1") = b;
	register i64 x2 __asm__("x2") = c;
	register i64 x3 __asm__("x3") = d;
	register i64 x4 __asm__("x4") = e;
	register i64 x5 __asm__("x5") = f;
	__asm__ volatile("svc #0"
			 : "+r"(x0)
			 : "r"(x8), "r"(x1), "r"(x2), "r"(x3), "r"(x4), "r"(x5)
			 : "memory");
	return x0;
}

/* aarch64 has the generic system-call table, without the calls that
 * have an *at, p* or *2 form. The tests name them as on x86_64; here each
 * such name is a number no call has, which sys6 makes as musl makes it on
 * aarch64 (open is openat(AT_FDCWD, ...), poll is ppoll, ...). */
#define LX_AT_FDCWD (-100)
#define LX_AT_SYMLINK_NOFOLLOW 0x100
#define LX_AT_REMOVEDIR 0x200
#define LX_LEGACY(k) (-1000 - (k))
#ifndef SYS_open
#define SYS_open LX_LEGACY(0)
#define SYS_stat LX_LEGACY(1)
#define SYS_lstat LX_LEGACY(2)
#define SYS_access LX_LEGACY(3)
#define SYS_unlink LX_LEGACY(4)
#define SYS_rmdir LX_LEGACY(5)
#define SYS_mkdir LX_LEGACY(6)
#define SYS_chmod LX_LEGACY(7)
#define SYS_link LX_LEGACY(8)
#define SYS_symlink LX_LEGACY(9)
#define SYS_readlink LX_LEGACY(10)
#define SYS_rename LX_LEGACY(11)
#define SYS_pipe LX_LEGACY(12)
#define SYS_dup2 LX_LEGACY(13)
#define SYS_poll LX_LEGACY(14)
#define SYS_epoll_create LX_LEGACY(15)
#define SYS_epoll_wait LX_LEGACY(16)
#define SYS_eventfd LX_LEGACY(17)
#define SYS_inotify_init LX_LEGACY(18)
#define SYS_alarm LX_LEGACY(19)
#define SYS_pause LX_LEGACY(20)
#define SYS_getpgrp LX_LEGACY(21)
#define SYS_time LX_LEGACY(22)
#define SYS_select LX_LEGACY(23)
#endif

static inline i64 sys6(i64 n, i64 a, i64 b, i64 c, i64 d, i64 e, i64 f)
{
	if (n > LX_LEGACY(0))
		return lx_svc(n, a, b, c, d, e, f);
	switch (n) {
	case SYS_open:
		return lx_svc(SYS_openat, LX_AT_FDCWD, a, b, c, 0, 0);
	case SYS_stat:
		return lx_svc(SYS_newfstatat, LX_AT_FDCWD, a, b, 0, 0, 0);
	case SYS_lstat:
		return lx_svc(SYS_newfstatat, LX_AT_FDCWD, a, b, LX_AT_SYMLINK_NOFOLLOW, 0, 0);
	case SYS_access:
		return lx_svc(SYS_faccessat, LX_AT_FDCWD, a, b, 0, 0, 0);
	case SYS_unlink:
		return lx_svc(SYS_unlinkat, LX_AT_FDCWD, a, 0, 0, 0, 0);
	case SYS_rmdir:
		return lx_svc(SYS_unlinkat, LX_AT_FDCWD, a, LX_AT_REMOVEDIR, 0, 0, 0);
	case SYS_mkdir:
		return lx_svc(SYS_mkdirat, LX_AT_FDCWD, a, b, 0, 0, 0);
	case SYS_chmod:
		return lx_svc(SYS_fchmodat, LX_AT_FDCWD, a, b, 0, 0, 0);
	case SYS_link:
		return lx_svc(SYS_linkat, LX_AT_FDCWD, a, LX_AT_FDCWD, b, 0, 0);
	case SYS_symlink:
		return lx_svc(SYS_symlinkat, a, LX_AT_FDCWD, b, 0, 0, 0);
	case SYS_readlink:
		return lx_svc(SYS_readlinkat, LX_AT_FDCWD, a, b, c, 0, 0);
	case SYS_rename:
		return lx_svc(SYS_renameat, LX_AT_FDCWD, a, LX_AT_FDCWD, b, 0, 0);
	case SYS_pipe:
		return lx_svc(SYS_pipe2, a, 0, 0, 0, 0, 0);
	case SYS_dup2:
		/* dup3 refuses old == new; dup2 then only checks the fd. */
		if (a == b) {
			i64 r = lx_svc(SYS_fcntl, a, 1 /* F_GETFD */, 0, 0, 0, 0);
			return r < 0 ? r : a;
		}
		return lx_svc(SYS_dup3, a, b, 0, 0, 0, 0);
	case SYS_poll: {
		/* poll(fds, n, ms): ppoll with the timeout as a timespec. */
		i64 ts[2] = { c / 1000, (c % 1000) * 1000000 };
		return lx_svc(SYS_ppoll, a, b, c < 0 ? 0 : (i64)ts, 0, 8, 0);
	}
	case SYS_epoll_create:
		return a <= 0 ? -22 /* EINVAL */ : lx_svc(SYS_epoll_create1, 0, 0, 0, 0, 0, 0);
	case SYS_epoll_wait:
		return lx_svc(SYS_epoll_pwait, a, b, c, d, 0, 8);
	case SYS_eventfd:
		return lx_svc(SYS_eventfd2, a, 0, 0, 0, 0, 0);
	case SYS_inotify_init:
		return lx_svc(SYS_inotify_init1, 0, 0, 0, 0, 0, 0);
	case SYS_alarm: {
		/* alarm(s): ITIMER_REAL's setitimer; the old one's seconds,
		 * rounded up as the kernel's alarm does. */
		i64 it[4] = { 0, 0, a, 0 }, old[4] = { 0, 0, 0, 0 };
		i64 r = lx_svc(SYS_setitimer, 0, (i64)it, (i64)old, 0, 0, 0);
		if (r < 0)
			return r;
		return old[2] + ((old[3] >= 500000 || (old[2] == 0 && old[3])) ? 1 : 0);
	}
	case SYS_pause:
		return lx_svc(SYS_ppoll, 0, 0, 0, 0, 0, 0);
	case SYS_getpgrp:
		return lx_svc(SYS_getpgid, 0, 0, 0, 0, 0, 0);
	case SYS_select: {
		/* select(n, r, w, e, tv): pselect6 with the timeval as a
		 * timespec, normalised as Linux's select does (tv_usec of a
		 * second or more carried into tv_sec), and the time left
		 * written back into the timeval, as select does. */
		i64 ts[2];
		i64 *tv = (i64 *)e;
		if (tv) {
			ts[0] = tv[0] + tv[1] / 1000000;
			ts[1] = (tv[1] % 1000000) * 1000;
		}
		i64 r = lx_svc(SYS_pselect6, a, b, c, d, tv ? (i64)ts : 0, 0);
		if (tv && r >= 0) {
			tv[0] = ts[0];
			tv[1] = ts[1] / 1000;
		}
		return r;
	}
	case SYS_time: {
		i64 ts[2] = { 0, 0 };
		lx_svc(SYS_clock_gettime, 0, (i64)ts, 0, 0, 0, 0);
		if (a)
			*(i64 *)a = ts[0];
		return ts[0];
	}
	}
	return -38; /* ENOSYS */
}
#else
#error "lx.h: x86_64 or aarch64 only"
#endif
#define sys0(n) sys6(n, 0, 0, 0, 0, 0, 0)
#define sys1(n, a) sys6(n, (i64)(a), 0, 0, 0, 0, 0)
#define sys2(n, a, b) sys6(n, (i64)(a), (i64)(b), 0, 0, 0, 0)
#define sys3(n, a, b, c) sys6(n, (i64)(a), (i64)(b), (i64)(c), 0, 0, 0)
#define sys4(n, a, b, c, d) sys6(n, (i64)(a), (i64)(b), (i64)(c), (i64)(d), 0, 0)

#if defined(__x86_64__)
/* Linux x86_64 system-call numbers the tests use. */
#define NR_read 0
#define NR_write 1
#define NR_close 3
#define NR_mmap 9
#define NR_mprotect 10
#define NR_munmap 11
#define NR_brk 12
#define NR_writev 20
#define NR_fcntl 72
#define NR_getuid 102
#define NR_getgid 104
#define NR_geteuid 107
#define NR_getegid 108
#define NR_arch_prctl 158
#define NR_exit_group 231
#define NR_pipe2 293
#else
/* Linux aarch64 system-call numbers the tests use (the generic table,
 * asm-generic/unistd.h; aarch64 has no arch_prctl). */
#define NR_read 63
#define NR_write 64
#define NR_close 57
#define NR_mmap 222
#define NR_mprotect 226
#define NR_munmap 215
#define NR_brk 214
#define NR_writev 66
#define NR_fcntl 25
#define NR_getuid 174
#define NR_getgid 176
#define NR_geteuid 175
#define NR_getegid 177
#define NR_exit_group 94
#define NR_pipe2 59
#endif

/* One output line, built in place and written with a single write(2). */
static char lx_line[1024];
static unsigned lx_len;

static void put_char(char c)
{
	if (lx_len < sizeof lx_line - 1)
		lx_line[lx_len++] = c;
}

static void put(const char *s)
{
	while (*s)
		put_char(*s++);
}

static void put_dec(i64 v)
{
	char tmp[24];
	int i = 0;
	u64 u = v < 0 ? -(u64)v : (u64)v;
	if (v < 0)
		put_char('-');
	do {
		tmp[i++] = '0' + u % 10;
		u /= 10;
	} while (u);
	while (i)
		put_char(tmp[--i]);
}

static void put_hex(u64 v)
{
	char tmp[17];
	int i = 0;
	put("0x");
	do {
		tmp[i++] = "0123456789abcdef"[v & 15];
		v >>= 4;
	} while (v);
	while (i)
		put_char(tmp[--i]);
}

static void end_line(void)
{
	lx_line[lx_len++] = '\n';
	sys3(NR_write, 1, lx_line, lx_len);
	lx_len = 0;
}

/* "name = value" lines. */
static void say(const char *name, i64 value)
{
	put(name);
	put(" = ");
	put_dec(value);
	end_line();
}

static void say_hex(const char *name, u64 value)
{
	put(name);
	put(" = ");
	put_hex(value);
	end_line();
}

static void say_bool(const char *name, int value)
{
	put(name);
	put(value ? " = yes" : " = no");
	end_line();
}

static void say_text(const char *text)
{
	put(text);
	end_line();
}

static __attribute__((noreturn)) void exit_group(int status)
{
	for (;;)
		sys1(NR_exit_group, status);
}

#endif
