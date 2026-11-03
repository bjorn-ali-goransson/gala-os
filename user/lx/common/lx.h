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
#define sys0(n) sys6(n, 0, 0, 0, 0, 0, 0)
#define sys1(n, a) sys6(n, (i64)(a), 0, 0, 0, 0, 0)
#define sys2(n, a, b) sys6(n, (i64)(a), (i64)(b), 0, 0, 0, 0)
#define sys3(n, a, b, c) sys6(n, (i64)(a), (i64)(b), (i64)(c), 0, 0, 0)
#define sys4(n, a, b, c, d) sys6(n, (i64)(a), (i64)(b), (i64)(c), (i64)(d), 0, 0)

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
