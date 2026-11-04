/* brk_test: the program break as brk(2) gives it (the raw system call,
 * which returns the new break, or the old one when it refuses; not
 * libc's brk/sbrk). Addresses are printed relative to the initial break.
 *
 * Accessibility is probed without signals: write(2) from an address fails
 * with EFAULT when it is not readable, read(2) into it when it is not
 * writable. */
#include "../common/lx.h"

extern char end[]; /* the linker's end of .bss */

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

int main(void)
{
	sys2(NR_pipe2, fds, 0);
	u64 start = sys1(NR_brk, 0);
	u64 bss_end = (u64)end;

	say_bool("initial break page aligned", start % 4096 == 0);
	/* Where the break starts is not checked: above .bss at a random
	 * offset for ET_EXEC, elsewhere for a static-pie (observed, 6.18). */
	(void)bss_end;
	say("brk(0) again, relative", sys1(NR_brk, 0) - start);
	say_bool("the initial break is not mapped", !readable((char *)start));

	u64 r = sys1(NR_brk, start + 3 * 4096);
	say("brk(+3 pages), relative", r - start);
	{
		char *p = (char *)start;
		int zero = 1;
		for (int i = 0; i < 3 * 4096; i++)
			zero &= p[i] == 0;
		say_bool("new pages zero", zero);
		say_bool("new pages writable", writable(p) && writable(p + 3 * 4096 - 1));
		say_bool("page after the break mapped", readable(p + 3 * 4096));
		p[5000] = 42;
	}

	r = sys1(NR_brk, start + 3 * 4096 + 100);
	say("brk(+3 pages + 100), relative", r - start);
	say_bool("the partial page is mapped", readable((char *)start + 3 * 4096 + 99));
	say_bool("the rest of that page is mapped", readable((char *)start + 4 * 4096 - 1));

	r = sys1(NR_brk, start + 4096 + 1);
	say("brk(shrink to +1 page + 1), relative", r - start);
	say_bool("kept page still holds its data", ((char *)start)[5000] == 42);
	say_bool("page 3 unmapped", !readable((char *)start + 2 * 4096));

	r = sys1(NR_brk, start + 3 * 4096);
	say("brk(+3 pages again), relative", r - start);
	say("page 3 zero again", ((char *)start)[2 * 4096 + 7]);
	say("byte 5000 kept", ((char *)start)[5000]);

	r = sys1(NR_brk, start - 4096);
	say("brk(below the initial break), relative", (i64)(r - start));
	r = sys1(NR_brk, start);
	say("brk(initial), relative", r - start);
	say_bool("first page unmapped", !readable((char *)start));
	r = sys1(NR_brk, 0x7fffffffffffUL);
	say("brk(top of user space), relative", (i64)(r - start));
	r = sys1(NR_brk, 0xffff800000000000UL);
	say("brk(kernel address), relative", (i64)(r - start));
	r = sys1(NR_brk, start + (256UL << 20));
	say("brk(+256 MiB), relative", (i64)(r - start));
	r = sys1(NR_brk, start);
	say("brk(initial) again, relative", (i64)(r - start));

	say_text("brk_test: ok");
	exit_group(0);
}
