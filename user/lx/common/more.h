/* More helpers for gala's Linux test programs: the milestone 2 and 3
 * corpus (files, pipes, processes, threads, signals, clocks, polling,
 * ptys, /proc). Include after lx.h.
 *
 * System-call numbers come from musl's <sys/syscall.h> (SYS_*); constants
 * and struct layouts from musl's headers, which are the kernel ABI's for
 * everything used here (the test programs print the offsets they rely on,
 * and the lx_abi crate's host tests check its layouts against them).
 *
 * Still no libc calls for what a test checks: every system call is a
 * `syscall` instruction (lx.h's sys6), results are printed as Linux
 * returns them (-errno, with the errno's name added for readability).
 * Never print what varies between runs: pids, fd numbers unless the test
 * closed every inherited descriptor first (lx_init does), addresses,
 * times, inode numbers. */
#ifndef LX_MORE_H
#define LX_MORE_H

#include <sys/syscall.h>

#define sys5(n, a, b, c, d, e) sys6(n, (i64)(a), (i64)(b), (i64)(c), (i64)(d), (i64)(e), 0)

static const char *errno_name(i64 e)
{
	switch (e) {
	case 1: return "EPERM";
	case 2: return "ENOENT";
	case 3: return "ESRCH";
	case 4: return "EINTR";
	case 5: return "EIO";
	case 6: return "ENXIO";
	case 7: return "E2BIG";
	case 8: return "ENOEXEC";
	case 9: return "EBADF";
	case 10: return "ECHILD";
	case 11: return "EAGAIN";
	case 12: return "ENOMEM";
	case 13: return "EACCES";
	case 14: return "EFAULT";
	case 16: return "EBUSY";
	case 17: return "EEXIST";
	case 18: return "EXDEV";
	case 19: return "ENODEV";
	case 20: return "ENOTDIR";
	case 21: return "EISDIR";
	case 22: return "EINVAL";
	case 23: return "ENFILE";
	case 24: return "EMFILE";
	case 25: return "ENOTTY";
	case 27: return "EFBIG";
	case 28: return "ENOSPC";
	case 29: return "ESPIPE";
	case 30: return "EROFS";
	case 31: return "EMLINK";
	case 32: return "EPIPE";
	case 34: return "ERANGE";
	case 36: return "ENAMETOOLONG";
	case 38: return "ENOSYS";
	case 39: return "ENOTEMPTY";
	case 40: return "ELOOP";
	case 88: return "ENOTSOCK";
	case 95: return "EOPNOTSUPP";
	case 97: return "EAFNOSUPPORT";
	case 98: return "EADDRINUSE";
	case 99: return "EADDRNOTAVAIL";
	case 104: return "ECONNRESET";
	case 106: return "EISCONN";
	case 107: return "ENOTCONN";
	case 111: return "ECONNREFUSED";
	case 114: return "EALREADY";
	case 115: return "EINPROGRESS";
	case 110: return "ETIMEDOUT";
	}
	return 0;
}

static void put_ret(i64 r)
{
	put_dec(r);
	if (r < 0 && r > -4096) {
		const char *n = errno_name(-r);
		if (n) {
			put(" (");
			put(n);
			put(")");
		}
	}
}

/* "name = r", with the errno's name when r is an error. */
static void say_ret(const char *name, i64 r)
{
	put(name);
	put(" = ");
	put_ret(r);
	end_line();
}

/* "name = ok" for a result that varies (a pid, an fd, an address), else
 * the error. */
static void say_ok(const char *name, i64 r)
{
	put(name);
	put(" = ");
	if (r >= 0)
		put("ok");
	else
		put_ret(r);
	end_line();
}

static void say_oct(const char *name, u64 v)
{
	char tmp[24];
	int i = 0;
	put(name);
	put(" = 0");
	do {
		tmp[i++] = '0' + (v & 7);
		v >>= 3;
	} while (v);
	while (i)
		put_char(tmp[--i]);
	end_line();
}

/* Bytes as a C string literal: printable ASCII as is, the rest escaped. */
static void put_quoted(const char *s, i64 n)
{
	put_char('"');
	for (i64 i = 0; i < n; i++) {
		unsigned char c = s[i];
		if (c == '\n')
			put("\\n");
		else if (c == '\r')
			put("\\r");
		else if (c == '\t')
			put("\\t");
		else if (c == '\b')
			put("\\b");
		else if (c == '"' || c == '\\') {
			put_char('\\');
			put_char(c);
		} else if (c >= 32 && c < 127)
			put_char(c);
		else {
			put("\\x");
			put_char("0123456789abcdef"[c >> 4]);
			put_char("0123456789abcdef"[c & 15]);
		}
	}
	put_char('"');
}

/* "name = <r> "bytes"" for a read: the count, then what was read. */
static void say_read(const char *name, i64 r, const char *buf)
{
	put(name);
	put(" = ");
	put_ret(r);
	if (r > 0) {
		put(" ");
		put_quoted(buf, r);
	}
	end_line();
}

static u64 lx_strlen(const char *s)
{
	u64 n = 0;
	while (s[n])
		n++;
	return n;
}

static int lx_streq(const char *a, const char *b)
{
	while (*a && *a == *b)
		a++, b++;
	return *a == *b;
}

static int lx_memeq(const void *a, const void *b, u64 n)
{
	const unsigned char *x = a, *y = b;
	for (u64 i = 0; i < n; i++)
		if (x[i] != y[i])
			return 0;
	return 1;
}

static void lx_memset(void *p, int c, u64 n)
{
	volatile unsigned char *q = p;
	for (u64 i = 0; i < n; i++)
		q[i] = c;
}

/* The number of leading bytes of `buf` (filled with `fill` before) that a
 * system call wrote: what it wrote ends with the last changed byte. */
static u64 written_size(const void *buf, u64 n, unsigned char fill)
{
	const unsigned char *b = buf;
	while (n && b[n - 1] == fill)
		n--;
	return n;
}

static i64 write_str(int fd, const char *s)
{
	return sys3(SYS_write, fd, s, lx_strlen(s));
}

/* struct timespec, as the kernel takes it. */
struct lx_ts {
	i64 sec, nsec;
};

static i64 now_ns(int clock)
{
	struct lx_ts t;
	sys2(SYS_clock_gettime, clock, &t);
	return t.sec * 1000000000 + t.nsec;
}

static void sleep_ms(i64 ms)
{
	struct lx_ts t = { ms / 1000, (ms % 1000) * 1000000 };
	while (sys2(SYS_nanosleep, &t, &t) == -4)
		;
}

/* Starts every test from the same state whatever ran it: no descriptors
 * but 0, 1 and 2 (so fd numbers are deterministic), no signal blocked,
 * SIGPIPE and SIGCHLD at their defaults, umask 022. */
#define LX_SIGPIPE 13
#define LX_SIGCHLD 17
struct k_sigaction {
	u64 handler, flags, restorer, mask;
};
#define LX_SA_RESTORER 0x04000000UL

/* sa_restorer as musl's: rt_sigreturn(2) on the frame the handler
 * returned to. */
void lx_restorer(void);
__asm__(".text\n"
	".globl lx_restorer\n"
	".type lx_restorer,@function\n"
	"lx_restorer:\n"
	"	mov $15, %eax\n"
	"	syscall\n"
	"	hlt\n");

/* rt_sigaction(2) with SA_RESTORER and lx_restorer, as musl's sigaction
 * does; `handler` 0 is SIG_DFL, 1 SIG_IGN. */
static i64 set_action(int sig, void *handler, u64 flags, u64 mask)
{
	struct k_sigaction sa = { (u64)handler, flags | LX_SA_RESTORER, (u64)lx_restorer, mask };
	return sys4(SYS_rt_sigaction, sig, &sa, 0, 8);
}

static void lx_init(void)
{
	u64 none = 0;
	sys3(SYS_close_range, 3, ~0U, 0);
	sys4(SYS_rt_sigprocmask, 2 /* SIG_SETMASK */, &none, 0, 8);
	set_action(LX_SIGPIPE, 0, 0, 0);
	set_action(LX_SIGCHLD, 0, 0, 0);
	sys1(SYS_umask, 022);
}

static i64 lx_fork(void)
{
	return sys0(SYS_fork);
}

/* wait4(pid, &status, options, 0): the status, or the error. */
static i64 wait_for(i64 pid, int options)
{
	int status = 0;
	i64 r = sys4(SYS_wait4, pid, &status, options, 0);
	return r < 0 ? r : status;
}

/* How a wait status reads (wait(2)'s macros). The core-dump flag is left
 * out: whether a core is written depends on the machine's limits. */
static void put_status(i64 st)
{
	if (st < 0) {
		put_ret(st);
	} else if ((st & 0x7f) == 0) {
		put("exited ");
		put_dec((st >> 8) & 0xff);
	} else if ((st & 0xff) == 0x7f) {
		put("stopped by ");
		put_dec((st >> 8) & 0xff);
	} else if (st == 0xffff) {
		put("continued");
	} else {
		put("killed by ");
		put_dec(st & 0x7f);
	}
}

static void say_status(const char *name, i64 st)
{
	put(name);
	put(": ");
	put_status(st);
	end_line();
}

/* Sorts n strings in place (insertion sort; the lists are short). */
static void sort_strings(char **v, int n)
{
	for (int i = 1; i < n; i++) {
		char *x = v[i];
		int j = i;
		while (j > 0) {
			const char *a = v[j - 1], *b = x;
			while (*a && *a == *b)
				a++, b++;
			if ((unsigned char)*a <= (unsigned char)*b)
				break;
			v[j] = v[j - 1];
			j--;
		}
		v[j] = x;
	}
}

static __attribute__((noreturn)) void lx_exit(int status)
{
	for (;;)
		sys1(SYS_exit_group, status);
}

#endif
