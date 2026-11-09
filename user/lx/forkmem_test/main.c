/* forkmem_test: what fork(2) does to memory, as a program sees it.
 * Private memory (anonymous mmap, .data, the stack, the break) is the
 * child's own copy: a write by either side after the fork is not seen by
 * the other, whether it comes from user code, from the kernel (read(2)
 * into the page) or after an mprotect; MADV_DONTNEED drops only the
 * caller's copy. MAP_SHARED anonymous memory (and a MAP_SHARED mapping
 * of /dev/zero) is one memory for both, pages first touched after the
 * fork included, and a futex word in it is one futex for both (a
 * non-private FUTEX_WAIT in the parent woken by the child). A large
 * private region keeps every page's contents across a fork in both, and
 * in a grandchild.
 *
 * Children report through their exit status or a pipe; the parent waits
 * for each before printing on, so the transcript's order is fixed. */
#define _GNU_SOURCE
#include "../common/lx.h"
#include "../common/more.h"
#include <fcntl.h>
#include <sys/mman.h>

#define FUTEX_WAIT 0
#define FUTEX_WAKE 1

#define PAGE 4096UL

static volatile int data_value = 1;

static void *map(u64 len, int prot, int flags, int fd)
{
	return (void *)sys6(SYS_mmap, 0, (i64)len, prot, flags, fd, 0);
}

static void *map_anon(u64 len, int shared)
{
	return map(len, PROT_READ | PROT_WRITE,
		   (shared ? MAP_SHARED : MAP_PRIVATE) | MAP_ANONYMOUS, -1);
}

/* Blocks until the other side writes a byte to `fd`. */
static void await(int fd)
{
	char c;
	sys3(SYS_read, fd, &c, 1);
}

static void signal_fd(int fd)
{
	sys3(SYS_write, fd, "x", 1);
}

/* The child sees `p`'s value from before the fork, then stores `child`
 * there; exit status: what it saw. The parent's value after it ends. */
static void child_writes(const char *what, volatile int *p)
{
	*p = 1;
	i64 pid = lx_fork();
	if (pid == 0) {
		int seen = *p;
		*p = 2;
		lx_exit(seen == 1 && *p == 2 ? 1 : 0);
	}
	put(what);
	put(": child saw 1 and stored 2");
	say_status("", wait_for(pid, 0));
	put("  parent's value");
	say("", *p);
}

/* The parent stores 5 after the fork while the child waits; the child
 * then exits with the value it sees. */
static void parent_writes(const char *what, volatile int *p)
{
	int fds[2];
	sys2(SYS_pipe2, fds, 0);
	*p = 1;
	i64 pid = lx_fork();
	if (pid == 0) {
		await(fds[0]);
		lx_exit(*p);
	}
	*p = 5;
	signal_fd(fds[1]);
	put(what);
	put(": parent stored 5, the child sees");
	say_status("", wait_for(pid, 0));
	sys1(SYS_close, fds[0]);
	sys1(SYS_close, fds[1]);
}

/* Every page of `p` (`pages` of them) starts with its index plus `add`;
 * returns how many do not. */
static u64 check_pages(volatile u64 *p, u64 pages, u64 add)
{
	u64 bad = 0;
	for (u64 i = 0; i < pages; i++)
		bad += p[i * (PAGE / 8)] != i + add;
	return bad;
}

static void fill_pages(volatile u64 *p, u64 pages, u64 add)
{
	for (u64 i = 0; i < pages; i++)
		p[i * (PAGE / 8)] = i + add;
}

int main(void)
{
	lx_init();
	int fds[2];

	say_text("-- private memory: each side's writes are its own");
	volatile int *anon = map_anon(PAGE, 0);
	child_writes("anonymous mmap", anon);
	parent_writes("anonymous mmap", anon);
	child_writes(".data", &data_value);
	parent_writes(".data", &data_value);
	volatile int stack_value = 0;
	child_writes("stack", &stack_value);
	parent_writes("stack", &stack_value);
	i64 brk0 = sys1(SYS_brk, 0);
	i64 brk1 = sys1(SYS_brk, brk0 + (i64)PAGE);
	say_bool("brk grew a page", brk1 == brk0 + (i64)PAGE);
	child_writes("break", (volatile int *)brk0);
	parent_writes("break", (volatile int *)brk0);

	/* A page never touched before the fork: demand-zero in both. */
	volatile int *fresh = map_anon(2 * PAGE, 0);
	i64 pid = lx_fork();
	if (pid == 0) {
		fresh[0] = 7;
		lx_exit(fresh[PAGE / sizeof(int)] == 0 ? 1 : 0);
	}
	say_status("untouched private page: the child stored 7, saw a zero beside it", wait_for(pid, 0));
	say("  parent's value", fresh[0]);

	/* The kernel's writes into a copy are the child's alone. */
	sys2(SYS_pipe2, fds, 0);
	anon[0] = 1;
	pid = lx_fork();
	if (pid == 0) {
		int v = 9;
		sys3(SYS_write, fds[1], &v, sizeof v);
		i64 r = sys3(SYS_read, fds[0], (void *)anon, sizeof v);
		lx_exit(r == sizeof v ? anon[0] : 0);
	}
	say_status("read(2) into a private page in the child, which then sees", wait_for(pid, 0));
	say("  parent's value", anon[0]);
	sys1(SYS_close, fds[0]);
	sys1(SYS_close, fds[1]);

	/* Read-only at the fork, made writable afterwards. */
	volatile int *ro = map_anon(PAGE, 0);
	ro[0] = 1;
	sys3(SYS_mprotect, (void *)ro, PAGE, PROT_READ);
	pid = lx_fork();
	if (pid == 0) {
		if (sys3(SYS_mprotect, (void *)ro, PAGE, PROT_READ | PROT_WRITE) != 0)
			lx_exit(0);
		ro[0] = 3;
		lx_exit(ro[0]);
	}
	say_status("mprotect(PROT_WRITE) after the fork, the child stores 3", wait_for(pid, 0));
	say("  parent's value", ro[0]);

	/* MADV_DONTNEED: the child's copy goes, the parent's stays. */
	anon[0] = 4;
	pid = lx_fork();
	if (pid == 0) {
		i64 r = sys3(SYS_madvise, (void *)anon, PAGE, MADV_DONTNEED);
		lx_exit(r == 0 && anon[0] == 0 ? 1 : 0);
	}
	say_status("MADV_DONTNEED in the child reads zero (1 = yes)", wait_for(pid, 0));
	say("  parent's value", anon[0]);

	/* A child's child sees what its parent had at its fork. */
	anon[0] = 1;
	pid = lx_fork();
	if (pid == 0) {
		anon[0] = 2;
		i64 grandchild = lx_fork();
		if (grandchild == 0) {
			int seen = anon[0];
			anon[0] = 3;
			lx_exit(seen);
		}
		i64 st = wait_for(grandchild, 0);
		lx_exit(((st >> 8) & 0xff) * 10 + anon[0]);
	}
	say_status("grandchild saw * 10 + the child's value after it", wait_for(pid, 0));
	say("  parent's value", anon[0]);

	say_text("-- shared memory: one memory for both");
	volatile int *shared = map_anon(2 * PAGE, 1);
	shared[0] = 1;
	pid = lx_fork();
	if (pid == 0) {
		int seen = shared[0];
		shared[0] = 2;
		/* A page neither touched before the fork. */
		shared[PAGE / sizeof(int)] = 6;
		lx_exit(seen);
	}
	say_status("MAP_SHARED|MAP_ANONYMOUS: the child saw", wait_for(pid, 0));
	say("  parent's value after the child stored 2", shared[0]);
	say("  parent's value in a page first touched by the child", shared[PAGE / sizeof(int)]);
	sys2(SYS_pipe2, fds, 0);
	pid = lx_fork();
	if (pid == 0) {
		await(fds[0]);
		lx_exit(shared[0]);
	}
	shared[0] = 8;
	signal_fd(fds[1]);
	say_status("  parent stored 8 after the fork, the child sees", wait_for(pid, 0));
	sys1(SYS_close, fds[0]);
	sys1(SYS_close, fds[1]);
	int zero = sys2(SYS_open, "/dev/zero", O_RDWR);
	volatile int *z = map(PAGE, PROT_READ | PROT_WRITE, MAP_SHARED, zero);
	sys1(SYS_close, zero);
	say("/dev/zero MAP_SHARED reads", z[0]);
	pid = lx_fork();
	if (pid == 0) {
		z[0] = 4;
		lx_exit(0);
	}
	wait_for(pid, 0);
	say("  parent's value after the child stored 4", z[0]);

	/* The child wakes the parent's FUTEX_WAIT on a shared word (until
	 * a wake finds it: the parent is about to wait). */
	volatile int *word = shared + 64;
	*word = 0;
	pid = lx_fork();
	if (pid == 0) {
		for (;;) {
			i64 n = sys6(SYS_futex, (i64)word, FUTEX_WAKE, 1, 0, 0, 0);
			if (n != 0)
				lx_exit(n);
			sleep_ms(1);
		}
	}
	i64 r = sys6(SYS_futex, (i64)word, FUTEX_WAIT, 0, 0, 0, 0);
	say_ret("FUTEX_WAIT on a shared word, woken by the child", r);
	say_status("  the child's FUTEX_WAKE woke", wait_for(pid, 0));

	say_text("-- a large private region");
	u64 pages = 4096; /* 16 MiB */
	volatile u64 *big = map_anon(pages * PAGE, 0);
	fill_pages(big, pages, 0);
	int up[2], down[2];
	sys2(SYS_pipe2, up, 0);
	sys2(SYS_pipe2, down, 0);
	pid = lx_fork();
	if (pid == 0) {
		u64 bad = check_pages(big, pages, 0);
		fill_pages(big, pages, 1);
		bad += check_pages(big, pages, 1);
		i64 grandchild = lx_fork();
		if (grandchild == 0)
			lx_exit(check_pages(big, pages, 1) == 0 ? 1 : 0);
		i64 st = wait_for(grandchild, 0);
		/* The parent rewrites its pages meanwhile; we look again. */
		signal_fd(up[1]);
		await(down[0]);
		bad += check_pages(big, pages, 1);
		lx_exit(bad == 0 && ((st >> 8) & 0xff) == 1 ? 1 : 0);
	}
	await(up[0]);
	fill_pages(big, pages, 2);
	signal_fd(down[1]);
	say_status("16 MiB: the child saw every page, rewrote them, its child saw its copy (1 = yes)",
		   wait_for(pid, 0));
	say("  pages the parent sees changed", check_pages(big, pages, 2));

	say_text("forkmem_test: ok");
	return 0;
}
