/* thread_test: threads made with the raw clone(2), as a pthread library
 * makes them (CLONE_VM|CLONE_FS|CLONE_FILES|CLONE_SIGHAND|CLONE_THREAD|
 * CLONE_SYSVSEM|CLONE_SETTLS|CLONE_PARENT_SETTID|CLONE_CHILD_CLEARTID),
 * and futex(2): WAIT/WAKE, a WAIT timing out (ETIMEDOUT), a WAIT on a
 * changed value (EAGAIN), set_tid_address, and joining through the
 * clear-tid wake.
 *
 * A thread runs only code that needs no TLS of musl's (raw system calls,
 * its own stack) and leaves its results in globals; the main thread prints
 * them after joining it. Waits are made deterministic by handshakes, never
 * by timing: a FUTEX_WAKE is retried until it wakes the thread that is
 * known to be about to wait. */
#define _GNU_SOURCE
#include "../common/lx.h"
#include "../common/more.h"
#include <fcntl.h>
#include <sched.h>
#include <signal.h>

#define FUTEX_WAIT 0
#define FUTEX_WAKE 1
#define FUTEX_PRIVATE 128
#define ARCH_GET_FS 0x1003

/* clone_thread(flags, stack_top, ptid, ctid, tls, fn, arg): the new thread
 * calls fn(arg) on stack_top and exits (exit(2), the thread only) with its
 * result. */
i64 clone_thread(u64 flags, void *stack, int *ptid, int *ctid, void *tls, int (*fn)(void *), void *arg);
__asm__(".text\n"
	".globl clone_thread\n"
	"clone_thread:\n"
	"	mov 8(%rsp), %rax\n"
	"	sub $16, %rsi\n"
	"	mov %r9, (%rsi)\n"
	"	mov %rax, 8(%rsi)\n"
	"	mov %rcx, %r10\n"
	"	mov $56, %eax\n" /* clone */
	"	syscall\n"
	"	test %rax, %rax\n"
	"	jnz 1f\n"
	"	xor %ebp, %ebp\n"
	"	pop %rax\n"
	"	pop %rdi\n"
	"	call *%rax\n"
	"	mov %eax, %edi\n"
	"	mov $60, %eax\n" /* exit */
	"	syscall\n"
	"	hlt\n"
	"1:	ret\n");

#define THREAD_FLAGS (CLONE_VM | CLONE_FS | CLONE_FILES | CLONE_SIGHAND | CLONE_THREAD | CLONE_SYSVSEM | \
		      CLONE_SETTLS | CLONE_PARENT_SETTID | CLONE_CHILD_CLEARTID)

static i64 futex(volatile int *addr, int op, int val, struct lx_ts *timeout)
{
	return sys4(SYS_futex, addr, op, val, timeout);
}

static char stacks[4][65536] __attribute__((aligned(16)));
static u64 tls_blocks[4][64] __attribute__((aligned(64)));

/* Joins a thread: waits for the kernel's clear-tid write and wake. */
static void join(volatile int *ctid)
{
	int v;
	while ((v = *ctid) != 0)
		futex(ctid, FUTEX_WAIT, v, 0);
}

/* What the first thread saw. */
static volatile i64 t_pid, t_tid, t_fs, t_fs0, t_settid;
static volatile int t_ran;

static int first(void *arg)
{
	(void)arg;
	t_pid = sys0(SYS_getpid);
	t_tid = sys0(SYS_gettid);
	u64 fs = 0;
	sys2(SYS_arch_prctl, ARCH_GET_FS, &fs);
	t_fs = fs;
	u64 w;
	__asm__ volatile("mov %%fs:0, %0" : "=r"(w));
	t_fs0 = w;
	t_ran = 1;
	return 0;
}

/* CLONE_FILES, CLONE_FS, CLONE_SIGHAND: what one thread changes, the
 * other sees. */
static volatile i64 opened_fd = -1;
static void on_usr1(int sig)
{
	(void)sig;
}

static int changer(void *arg)
{
	(void)arg;
	opened_fd = sys4(SYS_openat, AT_FDCWD, ".", O_RDONLY | O_DIRECTORY, 0);
	sys1(SYS_umask, 077);
	struct k_sigaction sa = { (u64)on_usr1, 0x04000000, (u64)lx_restorer, 0 };
	sys4(SYS_rt_sigaction, SIGUSR1, &sa, 0, 8);
	return 0;
}

/* FUTEX_WAIT/FUTEX_WAKE handshake. */
static volatile int word;
static volatile int about_to_wait;
static volatile i64 wait_result = -99;

static int waiter(void *arg)
{
	(void)arg;
	about_to_wait = 1;
	wait_result = futex(&word, FUTEX_WAIT, 0, 0);
	return 0;
}

/* Four threads add to a counter under a futex lock. */
static volatile int lock_word;
static volatile int counter;

static void lock(void)
{
	while (__sync_val_compare_and_swap(&lock_word, 0, 1) != 0)
		futex(&lock_word, FUTEX_WAIT | FUTEX_PRIVATE, 1, 0);
}

static void unlock(void)
{
	__sync_lock_release(&lock_word);
	futex(&lock_word, FUTEX_WAKE | FUTEX_PRIVATE, 1, 0);
}

static int adder(void *arg)
{
	(void)arg;
	for (int i = 0; i < 10000; i++) {
		lock();
		counter = counter + 1;
		unlock();
	}
	return 0;
}

/* Outlives the main thread, then exits(5). */
static int late(void *arg)
{
	(void)arg;
	sleep_ms(20);
	return 5;
}

/* A thread's exit_group ends the whole process: run in a child. */
static int ender(void *arg)
{
	(void)arg;
	lx_exit(42);
}

static i64 spawn(int i, int *ptid, int *ctid, int (*fn)(void *))
{
	tls_blocks[i][0] = (u64)&tls_blocks[i][0];
	return clone_thread(THREAD_FLAGS, stacks[i] + sizeof stacks[i], ptid, ctid, &tls_blocks[i][0], fn, 0);
}

int main(void)
{
	lx_init();
	i64 pid = sys0(SYS_getpid), tid = sys0(SYS_gettid);
	int ptid = 0;
	static volatile int ctid;

	say_text("-- set_tid_address");
	static int dummy;
	i64 r = sys1(SYS_set_tid_address, &dummy);
	say_bool("set_tid_address returns gettid()", r == tid);

	say_text("-- a thread");
	ctid = -1;
	r = spawn(0, &ptid, (int *)&ctid, first);
	say_bool("clone returned a tid > 0", r > 0);
	say_bool("CLONE_PARENT_SETTID stored it", ptid == r);
	join(&ctid);
	say("joined: ctid cleared to", ctid);
	say_bool("the thread ran", t_ran);
	say_bool("getpid() in the thread is the process's", t_pid == pid);
	say_bool("gettid() in the thread is clone's result", t_tid == r);
	say_bool("gettid() in the thread differs from the main thread's", t_tid != tid);
	say_bool("CLONE_SETTLS: ARCH_GET_FS is the TLS given", t_fs == (i64)&tls_blocks[0][0]);
	say_bool("  %fs:0 reads the TLS block's first word", t_fs0 == (i64)&tls_blocks[0][0]);
	say_text("-- shared state");
	ctid = -1;
	i64 old_umask = sys1(SYS_umask, 022);
	spawn(1, &ptid, (int *)&ctid, changer);
	join(&ctid);
	say_bool("CLONE_FILES: the thread's fd is open here", opened_fd >= 0 && sys2(SYS_fcntl, opened_fd, F_GETFD) == 0);
	say_oct("CLONE_FS: umask as the thread set it", sys1(SYS_umask, old_umask));
	struct k_sigaction sa;
	sys4(SYS_rt_sigaction, SIGUSR1, 0, &sa, 8);
	say_bool("CLONE_SIGHAND: the thread's SIGUSR1 handler is here", sa.handler == (u64)on_usr1);
	sys1(SYS_close, opened_fd);

	say_text("-- futex");
	word = 0;
	ctid = -1;
	spawn(2, &ptid, (int *)&ctid, waiter);
	while (!about_to_wait)
		sys0(SYS_sched_yield);
	int tries = 0;
	while ((r = futex(&word, FUTEX_WAKE, 1, 0)) == 0 && tries < 100000) {
		tries++;
		sys0(SYS_sched_yield);
	}
	say_ret("FUTEX_WAKE(1) woke", r);
	join(&ctid);
	say_ret("  the waiter's FUTEX_WAIT", wait_result);
	word = 1;
	say_ret("FUTEX_WAKE with no waiter", futex(&word, FUTEX_WAKE, 1, 0));
	say_ret("FUTEX_WAIT on a changed value (1 != 0)", futex(&word, FUTEX_WAIT, 0, 0));
	struct lx_ts timeout = { 0, 50000000 };
	i64 t0 = now_ns(1);
	r = futex(&word, FUTEX_WAIT, 1, &timeout);
	i64 elapsed = now_ns(1) - t0;
	say_ret("FUTEX_WAIT with a 50 ms timeout", r);
	say_bool("  waited at least 50 ms", elapsed >= 50000000);
	say_bool("  and less than 5 s", elapsed < 5000000000L);
	struct lx_ts bad = { 0, 1000000000 };
	say_ret("FUTEX_WAIT with tv_nsec = 1e9", futex(&word, FUTEX_WAIT, 1, &bad));
	say_ret("FUTEX_WAIT on an unaligned address", futex((volatile int *)((char *)&word + 1), FUTEX_WAIT, 1, 0));
	say_ret("FUTEX_WAIT on a bad address", futex((volatile int *)8, FUTEX_WAIT, 1, 0));
	say_ret("futex(unknown op)", futex(&word, 99, 1, 0));

	say_text("-- four threads, a futex lock");
	static volatile int ctids[4];
	for (int i = 0; i < 4; i++) {
		ctids[i] = -1;
		spawn(i, &ptid, (int *)&ctids[i], adder);
	}
	for (int i = 0; i < 4; i++)
		join(&ctids[i]);
	say("counter after 4 x 10000", counter);

	say_text("-- clone's errors and exits");
	char *top = stacks[0] + sizeof stacks[0];
	say_ret("CLONE_THREAD without CLONE_SIGHAND", clone_thread(CLONE_VM | CLONE_THREAD, top, 0, 0, 0, first, 0));
	say_ret("CLONE_SIGHAND without CLONE_VM", clone_thread(CLONE_SIGHAND, top, 0, 0, 0, first, 0));
	say_ret("CLONE_NEWNS|CLONE_FS", clone_thread(CLONE_NEWNS | CLONE_FS, top, 0, 0, 0, first, 0));
	i64 child = lx_fork();
	if (child == 0) {
		static volatile int c2;
		spawn(3, &ptid, (int *)&c2, ender);
		for (;;)
			sys0(SYS_pause);
	}
	say_status("a thread's exit_group ends its process", wait_for(child, 0));
	child = lx_fork();
	if (child == 0) {
		static volatile int c3 = -1;
		spawn(3, &ptid, (int *)&c3, late);
		sys1(SYS_exit, 0); /* the main thread leaves; the process goes on */
	}
	say_status("main thread exit(0), then the last thread exit(5)", wait_for(child, 0));
	say_text("thread_test: ok");
	return 0;
}
