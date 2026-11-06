/* thread_test: threads made with the raw clone(2), as a pthread library
 * makes them (CLONE_VM|CLONE_FS|CLONE_FILES|CLONE_SIGHAND|CLONE_THREAD|
 * CLONE_SYSVSEM|CLONE_SETTLS|CLONE_PARENT_SETTID|CLONE_CHILD_CLEARTID),
 * and futex(2): WAIT/WAKE, a WAIT timing out (ETIMEDOUT), a WAIT on a
 * changed value (EAGAIN), WAIT_BITSET/WAKE_BITSET with absolute
 * deadlines on both clocks, REQUEUE/CMP_REQUEUE, a WAIT interrupted by a
 * signal; set_tid_address, CLONE_CHILD_SETTID, and joining through the
 * clear-tid wake; signals per thread (masks, tgkill/tkill to one thread,
 * kill taken by the thread that does not block it); exit against
 * exit_group; and musl's pthreads on top (create/join, mutexes, condition
 * variables, pthread_kill).
 *
 * A raw thread runs only code that needs no TLS of musl's (raw system
 * calls, its own stack) and leaves its results in globals; the main
 * thread prints them after joining it. Waits are made deterministic by
 * handshakes, never by timing: a FUTEX_WAKE is retried until it wakes the
 * thread that is known to be about to wait. */
#define _GNU_SOURCE
#include "../common/lx.h"
#include "../common/more.h"
#include <fcntl.h>
#include <pthread.h>
#include <sched.h>
#include <signal.h>

#define FUTEX_WAIT 0
#define FUTEX_WAKE 1
#define FUTEX_REQUEUE 3
#define FUTEX_CMP_REQUEUE 4
#define FUTEX_WAIT_BITSET 9
#define FUTEX_WAKE_BITSET 10
#define FUTEX_PRIVATE 128
#define FUTEX_CLOCK_REALTIME 256
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

static char stacks[8][65536] __attribute__((aligned(16)));
static u64 tls_blocks[8][64] __attribute__((aligned(64)));

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

static i64 futex6(volatile int *addr, int op, int val, i64 timeout_or_val2, volatile int *addr2, int val3)
{
	return sys6(SYS_futex, (i64)addr, op, val, timeout_or_val2, (i64)addr2, val3);
}

static void ts_at(struct lx_ts *t, i64 ns)
{
	t->sec = ns / 1000000000;
	t->nsec = ns % 1000000000;
}

/* Two waiters on one word with different bitsets. */
static volatile int bit_word;
static volatile int bits_ready[2];
static volatile i64 bits_result[2] = { -99, -99 };

static int bits_waiter(void *arg)
{
	int i = (int)(i64)arg;
	bits_ready[i] = 1;
	bits_result[i] = futex6(&bit_word, FUTEX_WAIT_BITSET | FUTEX_PRIVATE, 0, 0, 0, 1 << i);
	return 0;
}

/* A waiter moved to another word by CMP_REQUEUE. */
static volatile int rq_from, rq_to;
static volatile int rq_ready;
static volatile i64 rq_result = -99;

static int rq_waiter(void *arg)
{
	(void)arg;
	rq_ready = 1;
	rq_result = futex(&rq_from, FUTEX_WAIT | FUTEX_PRIVATE, 0, 0);
	return 0;
}

/* A FUTEX_WAIT that a handled signal interrupts. */
static volatile int eintr_word;
static volatile int eintr_ready;
static volatile i64 eintr_tid;
static volatile i64 eintr_result = -99;
static volatile int usr1_seen;

static void count_usr1(int sig)
{
	(void)sig;
	usr1_seen++;
}

static int eintr_waiter(void *arg)
{
	(void)arg;
	eintr_tid = sys0(SYS_gettid);
	eintr_ready = 1;
	eintr_result = futex(&eintr_word, FUTEX_WAIT | FUTEX_PRIVATE, 0, 0);
	return 0;
}

/* Signals to one thread. */
static volatile int sig_ready, sig_go;
static volatile i64 sig_tid;
static volatile u64 sig_pending_seen, sig_mask_after;
static volatile i64 sig_waited, sig_waited_code, sig_waited_pid;
static volatile i64 h_tid, h_code, h_pid;
static volatile int h_count;

static void on_usr1_info(int sig, void *info, void *uc)
{
	(void)sig;
	(void)uc;
	h_tid = sys0(SYS_gettid);
	h_code = *(int *)((char *)info + 8);
	h_pid = *(int *)((char *)info + 16);
	h_count++;
}

static int sig_thread(void *arg)
{
	(void)arg;
	sig_tid = sys0(SYS_gettid);
	sig_ready = 1;
	while (!sig_go)
		sys0(SYS_sched_yield);
	u64 p = 0;
	sys2(SYS_rt_sigpending, &p, 8);
	sig_pending_seen = p;
	u64 usr2 = 1UL << (SIGUSR2 - 1), usr1 = 1UL << (SIGUSR1 - 1);
	unsigned char si[128];
	struct lx_ts zero = { 0, 0 };
	sig_waited = sys4(SYS_rt_sigtimedwait, &usr2, si, &zero, 8);
	sig_waited_code = *(int *)(si + 8);
	sig_waited_pid = *(int *)(si + 16);
	/* Unblocking the SIGUSR1 sent to this thread runs its handler here, on
	 * the way back from rt_sigprocmask. */
	sys4(SYS_rt_sigprocmask, SIG_UNBLOCK, &usr1, 0, 8);
	u64 m = 0;
	sys4(SYS_rt_sigprocmask, SIG_BLOCK, 0, &m, 8);
	sig_mask_after = m;
	return 0;
}

/* SIGTERM sent to its own thread (default action: the process ends). */
static int self_term(void *arg)
{
	(void)arg;
	sys3(SYS_tgkill, sys0(SYS_getpid), sys0(SYS_gettid), SIGTERM);
	for (;;)
		sys0(SYS_pause);
	return 0;
}

/* set_tid_address in a thread: the clear-tid address becomes another. */
static volatile int other_word = -1;
static volatile int set_tid_ok;

static int retid(void *arg)
{
	(void)arg;
	set_tid_ok = sys1(SYS_set_tid_address, &other_word) == sys0(SYS_gettid);
	return 0;
}

/* CLONE_CHILD_SETTID: the tid is at ctid when the thread starts. */
static volatile int settid_word;
static volatile int settid_seen_ok;

static int settid(void *arg)
{
	(void)arg;
	settid_seen_ok = settid_word == sys0(SYS_gettid);
	return 0;
}

/* pthreads (musl's, on clone and futex). */
static pthread_mutex_t m = PTHREAD_MUTEX_INITIALIZER;
static pthread_cond_t cv = PTHREAD_COND_INITIALIZER;
static int p_counter;
static int slot_full, slot_value;
static long consumed_sum;
static int in_wait, released, go_flag;
static volatile i64 pk_tid, pk_handler_tid;

static void *p_value(void *arg)
{
	(void)arg;
	pthread_exit((void *)42);
}

static void *p_add(void *arg)
{
	(void)arg;
	for (int i = 0; i < 10000; i++) {
		pthread_mutex_lock(&m);
		p_counter++;
		pthread_mutex_unlock(&m);
	}
	return 0;
}

static void *p_produce(void *arg)
{
	(void)arg;
	for (int i = 1; i <= 1000; i++) {
		pthread_mutex_lock(&m);
		while (slot_full)
			pthread_cond_wait(&cv, &m);
		slot_value = i;
		slot_full = 1;
		pthread_cond_broadcast(&cv);
		pthread_mutex_unlock(&m);
	}
	return 0;
}

static void *p_waiter(void *arg)
{
	(void)arg;
	pthread_mutex_lock(&m);
	in_wait++;
	while (!go_flag)
		pthread_cond_wait(&cv, &m);
	released++;
	pthread_mutex_unlock(&m);
	return 0;
}

static void on_usr2_tid(int sig)
{
	(void)sig;
	pk_handler_tid = sys0(SYS_gettid);
}

static void *p_killed(void *arg)
{
	(void)arg;
	pk_tid = sys0(SYS_gettid);
	pthread_mutex_lock(&m);
	in_wait++;
	while (!go_flag)
		pthread_cond_wait(&cv, &m);
	pthread_mutex_unlock(&m);
	return (void *)(long)(pk_handler_tid == pk_tid);
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

static i64 spawn_with(int i, u64 flags, int *ptid, int *ctid, int (*fn)(void *), void *arg)
{
	tls_blocks[i][0] = (u64)&tls_blocks[i][0];
	return clone_thread(flags, stacks[i] + sizeof stacks[i], ptid, ctid, &tls_blocks[i][0], fn, arg);
}

static i64 spawn(int i, int *ptid, int *ctid, int (*fn)(void *))
{
	return spawn_with(i, THREAD_FLAGS, ptid, ctid, fn, 0);
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


	say_text("-- futex: bitsets, absolute deadlines, requeue");
	static volatile int w2;
	word = 1;
	say_ret("WAIT_BITSET with bitset 0", futex6(&word, FUTEX_WAIT_BITSET, 1, 0, 0, 0));
	say_ret("WAKE_BITSET with bitset 0", futex6(&word, FUTEX_WAKE_BITSET, 1, 0, 0, 0));
	struct lx_ts at;
	ts_at(&at, now_ns(1) - 1000000);
	say_ret("WAIT_BITSET, MONOTONIC deadline passed", futex6(&word, FUTEX_WAIT_BITSET, 1, (i64)&at, 0, -1));
	t0 = now_ns(1);
	ts_at(&at, t0 + 50000000);
	r = futex6(&word, FUTEX_WAIT_BITSET | FUTEX_PRIVATE, 1, (i64)&at, 0, -1);
	say_ret("WAIT_BITSET, MONOTONIC deadline in 50 ms", r);
	say_bool("  woke at or after the deadline", now_ns(1) >= t0 + 50000000);
	t0 = now_ns(0);
	ts_at(&at, t0 + 30000000);
	r = futex6(&word, FUTEX_WAIT_BITSET | FUTEX_CLOCK_REALTIME, 1, (i64)&at, 0, -1);
	say_ret("WAIT_BITSET|CLOCK_REALTIME, deadline in 30 ms", r);
	say_bool("  woke at or after the deadline", now_ns(0) >= t0 + 30000000);
	struct lx_ts one_ms = { 0, 1000000 };
	say_ret("WAIT|CLOCK_REALTIME, 1 ms", futex6(&word, FUTEX_WAIT | FUTEX_CLOCK_REALTIME, 1, (i64)&one_ms, 0, 0));
	say_ret("WAKE|CLOCK_REALTIME", futex6(&word, FUTEX_WAKE | FUTEX_CLOCK_REALTIME, 1, 0, 0, 0));
	say_ret("WAIT_BITSET on a changed value", futex6(&word, FUTEX_WAIT_BITSET, 0, 0, 0, -1));
	say_ret("REQUEUE with no waiter", futex6(&word, FUTEX_REQUEUE, 1, 1, &w2, 0));
	say_ret("CMP_REQUEUE with no waiter", futex6(&word, FUTEX_CMP_REQUEUE, 1, 1, &w2, 1));
	say_ret("CMP_REQUEUE, *uaddr != val3", futex6(&word, FUTEX_CMP_REQUEUE, 1, 1, &w2, 0));
	say_ret("CMP_REQUEUE, nr_requeue -1", futex6(&word, FUTEX_CMP_REQUEUE, 1, -1, &w2, 1));

	bit_word = 0;
	static volatile int bctid[2];
	for (int i = 0; i < 2; i++) {
		bctid[i] = -1;
		spawn_with(4 + i, THREAD_FLAGS, &ptid, (int *)&bctid[i], bits_waiter, (void *)(i64)i);
	}
	while (!bits_ready[0] || !bits_ready[1])
		sys0(SYS_sched_yield);
	tries = 0;
	while ((r = futex6(&bit_word, FUTEX_WAKE_BITSET | FUTEX_PRIVATE, 1, 0, 0, 2)) == 0 && tries < 100000) {
		tries++;
		sys0(SYS_sched_yield);
	}
	say_ret("WAKE_BITSET(0x2) woke", r);
	join(&bctid[1]);
	say_ret("  the 0x2 waiter's WAIT_BITSET", bits_result[1]);
	say_bool("  the 0x1 waiter still waits", bits_result[0] == -99);
	tries = 0;
	while ((r = futex6(&bit_word, FUTEX_WAKE_BITSET | FUTEX_PRIVATE, 5, 0, 0, -1)) == 0 && tries < 100000) {
		tries++;
		sys0(SYS_sched_yield);
	}
	say_ret("WAKE_BITSET(all bits) woke", r);
	join(&bctid[0]);
	say_ret("  the 0x1 waiter's WAIT_BITSET", bits_result[0]);

	rq_from = 0;
	rq_to = 0;
	ctid = -1;
	spawn(4, &ptid, (int *)&ctid, rq_waiter);
	while (!rq_ready)
		sys0(SYS_sched_yield);
	tries = 0;
	while ((r = futex6(&rq_from, FUTEX_CMP_REQUEUE | FUTEX_PRIVATE, 0, 1, &rq_to, 0)) == 0 && tries < 100000) {
		tries++;
		sys0(SYS_sched_yield);
	}
	say_ret("CMP_REQUEUE(wake 0, requeue 1) moved", r);
	say_ret("  FUTEX_WAKE on the first word", futex(&rq_from, FUTEX_WAKE | FUTEX_PRIVATE, 1, 0));
	say_ret("  FUTEX_WAKE on the second word", futex(&rq_to, FUTEX_WAKE | FUTEX_PRIVATE, 1, 0));
	join(&ctid);
	say_ret("  the requeued waiter's FUTEX_WAIT", rq_result);

	set_action(SIGUSR1, count_usr1, 0, 0);
	eintr_word = 0;
	ctid = -1;
	spawn(4, &ptid, (int *)&ctid, eintr_waiter);
	while (!eintr_ready)
		sys0(SYS_sched_yield);
	while (eintr_result == -99) {
		sys3(SYS_tgkill, pid, eintr_tid, SIGUSR1);
		sys0(SYS_sched_yield);
	}
	join(&ctid);
	say_ret("FUTEX_WAIT interrupted by a handled signal", eintr_result);
	say_bool("  the handler ran", usr1_seen > 0);
	set_action(SIGUSR1, 0, 0, 0);

	say_text("-- signals and threads");
	u64 both = (1UL << (SIGUSR1 - 1)) | (1UL << (SIGUSR2 - 1)), none = 0, pend = 0;
	sys4(SYS_rt_sigprocmask, SIG_BLOCK, &both, 0, 8);
	set_action(SIGUSR1, on_usr1_info, 4 /* SA_SIGINFO */, 0);
	ctid = -1;
	spawn(4, &ptid, (int *)&ctid, sig_thread);
	while (!sig_ready)
		sys0(SYS_sched_yield);
	say_ret("tgkill(the thread, SIGUSR1)", sys3(SYS_tgkill, pid, sig_tid, SIGUSR1));
	sys2(SYS_rt_sigpending, &pend, 8);
	say_hex("  pending in the main thread", pend);
	say_ret("kill(the process, SIGUSR2)", sys2(SYS_kill, pid, SIGUSR2));
	sys2(SYS_rt_sigpending, &pend, 8);
	say_hex("  pending in the main thread", pend);
	sig_go = 1;
	join(&ctid);
	say_hex("the thread saw pending", sig_pending_seen);
	say_ret("  its rt_sigtimedwait(SIGUSR2)", sig_waited);
	say("  si_code", sig_waited_code);
	say_bool("  si_pid is the sender", sig_waited_pid == pid);
	say_bool("its SIGUSR1 handler ran in it", h_count == 1 && h_tid == sig_tid);
	say("  si_code", h_code);
	say_bool("  si_pid is the sender", h_pid == pid);
	say_hex("  its mask after unblocking SIGUSR1", sig_mask_after);
	sys2(SYS_rt_sigpending, &pend, 8);
	say_hex("pending in the main thread now", pend);
	sys4(SYS_rt_sigprocmask, SIG_SETMASK, &none, 0, 8);
	say_bool("unblocked in the main thread: no handler ran here", h_count == 1);
	set_action(SIGUSR1, 0, 0, 0);
	say_ret("tgkill(pid, no such tid)", sys3(SYS_tgkill, pid, 0x3fffffff, SIGUSR1));
	say_ret("tgkill(pid, tid, signal 65)", sys3(SYS_tgkill, pid, tid, 65));
	say_ret("tgkill(0, tid)", sys3(SYS_tgkill, 0, tid, SIGUSR1));
	say_ret("tgkill(pid, tid, 0)", sys3(SYS_tgkill, pid, tid, 0));
	say_ret("tkill(tid, 0)", sys2(SYS_tkill, tid, 0));
	say_ret("tkill(-1, 0)", sys2(SYS_tkill, -1, 0));
	i64 child = lx_fork();
	if (child == 0) {
		static volatile int c1;
		spawn(4, &ptid, (int *)&c1, self_term);
		for (;;)
			sys0(SYS_pause);
	}
	say_status("a thread's tgkill(SIGTERM) to itself ends its process", wait_for(child, 0));

	say_text("-- set_tid_address and CLONE_CHILD_SETTID in a thread");
	ctid = -1;
	spawn(4, &ptid, (int *)&ctid, retid);
	join(&other_word);
	say_bool("set_tid_address in the thread returned its tid", set_tid_ok);
	say("  the new address cleared at exit to", other_word);
	say("  the clone's ctid (no longer the clear address)", ctid);
	settid_word = -1;
	r = spawn_with(5, THREAD_FLAGS | CLONE_CHILD_SETTID, &ptid, (int *)&settid_word, settid, 0);
	join(&settid_word);
	say_bool("CLONE_CHILD_SETTID: the thread found its tid there", settid_seen_ok);
	say("  cleared at exit to", settid_word);

	say_text("-- clone's errors and exits");
	char *top = stacks[0] + sizeof stacks[0];
	say_ret("CLONE_THREAD without CLONE_SIGHAND", clone_thread(CLONE_VM | CLONE_THREAD, top, 0, 0, 0, first, 0));
	say_ret("CLONE_SIGHAND without CLONE_VM", clone_thread(CLONE_SIGHAND, top, 0, 0, 0, first, 0));
	say_ret("CLONE_NEWNS|CLONE_FS", clone_thread(CLONE_NEWNS | CLONE_FS, top, 0, 0, 0, first, 0));
	child = lx_fork();
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

	say_text("-- pthreads");
	pthread_t pt[4];
	void *ret = 0;
	say_ret("pthread_create", pthread_create(&pt[0], 0, p_value, 0));
	say_ret("pthread_join", pthread_join(pt[0], &ret));
	say("  its pthread_exit value", (i64)ret);
	for (int i = 0; i < 4; i++)
		pthread_create(&pt[i], 0, p_add, 0);
	for (int i = 0; i < 4; i++)
		pthread_join(pt[i], 0);
	say("mutex: counter after 4 x 10000", p_counter);
	pthread_create(&pt[0], 0, p_produce, 0);
	for (int n = 0; n < 1000; n++) {
		pthread_mutex_lock(&m);
		while (!slot_full)
			pthread_cond_wait(&cv, &m);
		consumed_sum += slot_value;
		slot_full = 0;
		pthread_cond_broadcast(&cv);
		pthread_mutex_unlock(&m);
	}
	pthread_join(pt[0], 0);
	say("condition variable: the sum of 1..1000 handed over", consumed_sum);
	in_wait = released = go_flag = 0;
	for (int i = 0; i < 3; i++)
		pthread_create(&pt[i], 0, p_waiter, 0);
	for (;;) {
		pthread_mutex_lock(&m);
		int n = in_wait;
		pthread_mutex_unlock(&m);
		if (n == 3)
			break;
		sys0(SYS_sched_yield);
	}
	pthread_mutex_lock(&m);
	go_flag = 1;
	pthread_cond_broadcast(&cv);
	pthread_mutex_unlock(&m);
	for (int i = 0; i < 3; i++)
		pthread_join(pt[i], 0);
	say("pthread_cond_broadcast released waiters", released);
	in_wait = go_flag = 0;
	set_action(SIGUSR2, on_usr2_tid, 0, 0);
	pthread_create(&pt[0], 0, p_killed, 0);
	for (;;) {
		pthread_mutex_lock(&m);
		int n = in_wait;
		pthread_mutex_unlock(&m);
		if (n == 1)
			break;
		sys0(SYS_sched_yield);
	}
	say_ret("pthread_kill(SIGUSR2)", pthread_kill(pt[0], SIGUSR2));
	while (!pk_handler_tid)
		sys0(SYS_sched_yield);
	pthread_mutex_lock(&m);
	go_flag = 1;
	pthread_cond_broadcast(&cv);
	pthread_mutex_unlock(&m);
	pthread_join(pt[0], &ret);
	say_bool("  its handler ran in that thread", ret != 0);
	set_action(SIGUSR2, 0, 0, 0);
	say_text("thread_test: ok");
	return 0;
}
