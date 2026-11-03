/* mmap_test: anonymous mmap(2), mprotect(2) and munmap(2) as the raw
 * system calls give them: results, errors, protections, MAP_FIXED.
 *
 * Accessibility is probed without signals: write(2) from an address fails
 * with EFAULT when it is not readable, read(2) into it when it is not
 * writable. Execute-only mappings are not checked (with protection keys
 * a PROT_EXEC page may be unreadable, depending on the CPU). */
#include "../common/lx.h"

#define PROT_NONE 0
#define PROT_READ 1
#define PROT_WRITE 2
#define MAP_SHARED 1
#define MAP_PRIVATE 2
#define MAP_FIXED 0x10
#define MAP_ANONYMOUS 0x20
#define MAP_FIXED_NOREPLACE 0x100000
#define PAGE 4096UL
#define HINT 0x200000000UL /* 8 GiB: free in both the static and the static-pie layout */

static int fds[2];

static int readable(const char *p)
{
	i64 r = sys3(NR_write, fds[1], p, 1);
	if (r == 1) {
		char c;
		sys3(NR_read, fds[0], &c, 1);
	}
	return r == 1;
}

static int writable(char *p)
{
	sys3(NR_write, fds[1], "w", 1);
	i64 r = sys3(NR_read, fds[0], p, 1);
	if (r != 1) {
		char c;
		sys3(NR_read, fds[0], &c, 1);
	}
	return r == 1;
}

static void access_line(const char *name, char *p)
{
	put(name);
	put(": ");
	put(readable(p) ? "r" : "-");
	put(writable(p) ? "w" : "-");
	end_line();
}

static i64 map(u64 addr, u64 len, int prot, int flags)
{
	return sys6(NR_mmap, addr, len, prot, flags, -1, 0);
}

int main(void)
{
	sys2(NR_pipe2, fds, 0);

	i64 a = map(0, 3 * PAGE, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS);
	say_bool("mmap(3 pages) succeeded", a > 0);
	say_bool("page aligned", a % PAGE == 0);
	{
		char *p = (char *)a;
		int zero = 1;
		for (u64 i = 0; i < 3 * PAGE; i++)
			zero &= p[i] == 0;
		say_bool("zero filled", zero);
		p[0] = 1;
		p[3 * PAGE - 1] = 2;
		access_line("rw page", p);
	}

	say("mprotect(page 2, PROT_READ)", sys3(NR_mprotect, a + PAGE, PAGE, PROT_READ));
	access_line("page 1", (char *)a);
	access_line("page 2", (char *)a + PAGE);
	say("mprotect(page 3, PROT_NONE)", sys3(NR_mprotect, a + 2 * PAGE, PAGE, PROT_NONE));
	access_line("page 3", (char *)a + 2 * PAGE);
	say("mprotect(page 3, rw)", sys3(NR_mprotect, a + 2 * PAGE, PAGE, PROT_READ | PROT_WRITE));
	say("page 3 kept its data", ((char *)a)[3 * PAGE - 1]);
	say("mprotect(unaligned)", sys3(NR_mprotect, a + 1, PAGE, PROT_READ));
	say("mprotect(bad prot)", sys3(NR_mprotect, a, PAGE, 0x100));

	say("munmap(page 2)", sys2(NR_munmap, a + PAGE, PAGE));
	access_line("page 2 after munmap", (char *)a + PAGE);
	access_line("page 1 after munmap of 2", (char *)a);
	say("munmap(page 2) again", sys2(NR_munmap, a + PAGE, PAGE));
	say("munmap(unaligned)", sys2(NR_munmap, a + 1, PAGE));
	say("munmap(length 0)", sys2(NR_munmap, a, 0));
	say("munmap(unaligned length)", sys2(NR_munmap, a + 2 * PAGE, 100));
	access_line("page 3 after munmap(len 100)", (char *)a + 2 * PAGE);

	/* A hint is used when it is free; MAP_FIXED replaces what is there. */
	i64 h = map(HINT, 2 * PAGE, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS);
	say_bool("hint used", h == (i64)HINT);
	((char *)h)[10] = 7;
	((char *)h)[PAGE + 10] = 8;
	i64 f = map(HINT + PAGE, PAGE, PROT_READ, MAP_PRIVATE | MAP_ANONYMOUS | MAP_FIXED);
	say_bool("MAP_FIXED at the hint's second page", f == (i64)(HINT + PAGE));
	say("first page kept", ((char *)h)[10]);
	say("second page replaced (zero)", ((char *)h)[PAGE + 10]);
	access_line("replaced page", (char *)h + PAGE);
	say("MAP_FIXED_NOREPLACE over it",
	    map(HINT, PAGE, PROT_READ, MAP_PRIVATE | MAP_ANONYMOUS | MAP_FIXED_NOREPLACE));
	i64 nr = map(HINT + 2 * PAGE, PAGE, PROT_READ, MAP_PRIVATE | MAP_ANONYMOUS | MAP_FIXED_NOREPLACE);
	say_bool("MAP_FIXED_NOREPLACE next to it", nr == (i64)(HINT + 2 * PAGE));
	say("munmap(the whole range, 3 pages)", sys2(NR_munmap, HINT, 3 * PAGE));
	access_line("hint after munmap", (char *)HINT);

	/* A hint that is not page aligned is rounded (not MAP_FIXED). */
	i64 u = map(HINT + 123, PAGE, PROT_READ, MAP_PRIVATE | MAP_ANONYMOUS);
	say_bool("unaligned hint rounded down", u == (i64)HINT);
	sys2(NR_munmap, u, PAGE);

	/* Unaligned length: rounded up to a page. */
	i64 l = map(0, 100, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS);
	access_line("length 100, last byte of its page", (char *)l + PAGE - 1);
	say("munmap(length 100)", sys2(NR_munmap, l, 100));

	say("mmap(length 0)", map(0, 0, PROT_READ, MAP_PRIVATE | MAP_ANONYMOUS));
	say("mmap(MAP_FIXED, unaligned)", map(HINT + 1, PAGE, PROT_READ, MAP_PRIVATE | MAP_ANONYMOUS | MAP_FIXED));
	say("mmap(neither shared nor private)", map(0, PAGE, PROT_READ, MAP_ANONYMOUS));
	say("mmap(no MAP_ANONYMOUS, fd -1)", sys6(NR_mmap, 0, PAGE, PROT_READ, MAP_PRIVATE, -1, 0));
	say("mmap(anonymous, offset 1)", sys6(NR_mmap, 0, PAGE, PROT_READ, MAP_PRIVATE | MAP_ANONYMOUS, -1, 1));
	say("mmap(huge length)", map(0, 1UL << 62, PROT_READ, MAP_PRIVATE | MAP_ANONYMOUS));
	say("mmap(MAP_FIXED, kernel address)",
	    map(0xffff800000000000UL, PAGE, PROT_READ, MAP_PRIVATE | MAP_ANONYMOUS | MAP_FIXED));
	{
		i64 s = map(0, PAGE, PROT_READ | PROT_WRITE, MAP_SHARED | MAP_ANONYMOUS);
		say_bool("MAP_SHARED|MAP_ANONYMOUS succeeded", s > 0 && s % PAGE == 0);
		access_line("shared page", (char *)s);
		say("munmap(shared)", sys2(NR_munmap, s, PAGE));
	}

	say_text("mmap_test: ok");
	exit_group(0);
}
