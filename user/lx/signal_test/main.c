/* signal_test: signals as the raw system calls give them.
 *
 * rt_sigaction (SA_SIGINFO, SA_RESTART, SA_NODEFER, SA_RESETHAND,
 * SA_ONSTACK, sa_mask, sa_restorer as musl sets it: rt_sigreturn),
 * rt_sigprocmask (SIGKILL and SIGSTOP cannot be blocked), pending signals
 * (standard ones do not queue, real-time ones do; the delivery order),
 * rt_sigpending/rt_sigsuspend/rt_sigtimedwait, kill/tgkill/
 * rt_sigqueueinfo and the siginfo each gives, SIGCHLD, sigaltstack, a
 * fault's SIGSEGV (si_addr, si_code, the trap number and error code) with
 * a handler that changes RIP and RAX in the ucontext and returns, and
 * every signal's default action (term/core, stop, ignore, continue; a
 * child per signal).
 *
 * The rt_sigframe is observed, not assumed: a fault probe loads a marker
 * into every register, the handler finds each marker in the ucontext and
 * prints its offset, and the frame's shape (where siginfo lies relative
 * to the ucontext, the restorer address below it) is printed too. The
 * lx_abi crate's host tests check its signal layouts against these
 * lines. */
#define _GNU_SOURCE
#include "../common/lx.h"
#include "../common/more.h"
#include <signal.h>
#include <stddef.h>
#include <sys/mman.h>
#include <sys/resource.h>
#include <sys/wait.h>
#include <ucontext.h>

#define BAD_ADDR 0xdead000UL
#define MARK(i) (0x1111111111111100UL + (i))

/* fault_read(addr): every general register a marker (rcx = addr), then a
 * load from addr at fault_read_insn. A handler resuming at
 * fault_read_resume makes it return the handler's RAX. */
u64 fault_read(u64 addr);
extern char fault_read_insn[], fault_read_resume[];
__asm__(".text\n"
	".globl fault_read, fault_read_insn, fault_read_resume\n"
	"fault_read:\n"
	"	push %rbx\n"
	"	push %rbp\n"
	"	push %r12\n"
	"	push %r13\n"
	"	push %r14\n"
	"	push %r15\n"
	"	mov %rdi, %rcx\n"
	"	movabs $0x1111111111111100, %r8\n"
	"	movabs $0x1111111111111101, %r9\n"
	"	movabs $0x1111111111111102, %r10\n"
	"	movabs $0x1111111111111103, %r11\n"
	"	movabs $0x1111111111111104, %r12\n"
	"	movabs $0x1111111111111105, %r13\n"
	"	movabs $0x1111111111111106, %r14\n"
	"	movabs $0x1111111111111107, %r15\n"
	"	movabs $0x1111111111111108, %rdi\n"
	"	movabs $0x1111111111111109, %rsi\n"
	"	movabs $0x111111111111110a, %rbp\n"
	"	movabs $0x111111111111110b, %rbx\n"
	"	movabs $0x111111111111110c, %rdx\n"
	"	movabs $0x111111111111110d, %rax\n"
	"fault_read_insn:\n"
	"	mov (%rcx), %rax\n"
	"fault_read_resume:\n"
	"	pop %r15\n"
	"	pop %r14\n"
	"	pop %r13\n"
	"	pop %r12\n"
	"	pop %rbp\n"
	"	pop %rbx\n"
	"	ret\n");

/* fault_write(addr): a store to addr; resumes at fault_write_resume. */
void fault_write(u64 addr);
extern char fault_write_insn[], fault_write_resume[];
__asm__(".text\n"
	".globl fault_write, fault_write_insn, fault_write_resume\n"
	"fault_write:\n"
	"fault_write_insn:\n"
	"	movl $1, (%rdi)\n"
	"fault_write_resume:\n"
	"	ret\n");

static const char *const reg_names[] = { "r8", "r9", "r10", "r11", "r12", "r13", "r14", "r15", "rdi", "rsi", "rbp", "rbx", "rdx", "rax" };

/* What the SIGSEGV handler saw. */
static volatile int segv_count;
static volatile int segv_code, segv_signo, segv_errno;
static volatile u64 segv_addr, segv_trapno, segv_err, segv_cr2, segv_cs;
static volatile i64 info_minus_uc;
static volatile int restorer_below_uc, rip_was_insn, fpstate_ok;
static volatile int reg_offsets[14];
static volatile int addr_offsets[4], n_addr_offsets;
static volatile int mask_offsets[4], n_mask_offsets;
static volatile u64 uc_flags;
static volatile int info_addr_offset = -1;
static volatile int rsp_offset = -1, eflags_offset = -1;
static volatile int rip_offset = -1, cs_offset = -1, err_offset = -1, trapno_offset = -1, fpstate_offset = -1;
static volatile char *resume_to;
static u64 marker_mask;

static void on_segv(int sig, siginfo_t *info, void *ucv)
{
	ucontext_t *uc = ucv;
	u64 *words = (u64 *)uc;
	segv_count++;
	segv_signo = info->si_signo;
	segv_errno = info->si_errno;
	segv_code = info->si_code;
	segv_addr = (u64)info->si_addr;
	segv_trapno = uc->uc_mcontext.gregs[REG_TRAPNO];
	segv_err = uc->uc_mcontext.gregs[REG_ERR];
	segv_cr2 = uc->uc_mcontext.gregs[REG_CR2];
	segv_cs = uc->uc_mcontext.gregs[REG_CSGSFS] & 0xffff;
	uc_flags = uc->uc_flags;
	info_minus_uc = (char *)info - (char *)uc;
	restorer_below_uc = words[-1] == (u64)lx_restorer;
	(void)sig;
	if (resume_to == fault_read_resume) {
		rip_was_insn = (u64)uc->uc_mcontext.gregs[REG_RIP] == (u64)fault_read_insn;
		for (int r = 0; r < 14; r++) {
			reg_offsets[r] = -1;
			for (int w = 0; w < 38; w++)
				if (words[w] == MARK(r)) {
					reg_offsets[r] = w * 8;
					break;
				}
		}
		n_addr_offsets = 0;
		for (int w = 0; w < 38 && n_addr_offsets < 4; w++)
			if (words[w] == BAD_ADDR)
				addr_offsets[n_addr_offsets++] = w * 8;
		n_mask_offsets = 0;
		for (int w = 0; w < 38 && n_mask_offsets < 4; w++)
			if (words[w] == marker_mask)
				mask_offsets[n_mask_offsets++] = w * 8;
		/* After the general registers: rip, eflags, cs/gs/fs/ss, err,
		 * trapno, oldmask, cr2, fpstate. */
		char here;
		for (int w = 15; w < 38; w++) {
			u64 near = (u64)&here;
			if (rsp_offset < 0 && words[w] > near && words[w] - near < 65536)
				rsp_offset = w * 8;
			if (w >= 20 && eflags_offset < 0 && (words[w] & 0x202) == 0x202 && words[w] < 0x400000)
				eflags_offset = w * 8;
		}
		for (int w = 20; w < 38; w++) {
			if (rip_offset < 0 && words[w] == (u64)fault_read_insn)
				rip_offset = w * 8;
			if (cs_offset < 0 && (words[w] & 0xffff) == 0x33)
				cs_offset = w * 8;
			if (err_offset < 0 && words[w] == 4)
				err_offset = w * 8;
			if (trapno_offset < 0 && words[w] == 14)
				trapno_offset = w * 8;
			if (fpstate_offset < 0 && words[w] == (u64)uc->uc_mcontext.fpregs)
				fpstate_offset = w * 8;
		}
		u64 *iw = (u64 *)info;
		for (int w = 0; w < 16; w++)
			if (iw[w] == BAD_ADDR) {
				info_addr_offset = w * 8;
				break;
			}
		u64 fp = (u64)uc->uc_mcontext.fpregs;
		fpstate_ok = fp % 64 == 0 && fp >= (u64)info + 128;
	} else {
		rip_was_insn = (u64)uc->uc_mcontext.gregs[REG_RIP] == (u64)fault_write_insn;
	}
	uc->uc_mcontext.gregs[REG_RIP] = (u64)resume_to;
	uc->uc_mcontext.gregs[REG_RAX] = 0x600d;
}

/* A general handler: counts, records the order and what siginfo said. */
static volatile int calls[65];
static char order[64];
static volatile int order_len;
static volatile int last_code, last_pid_is_me, last_uid_ok, last_value, last_errno;
static volatile int blocked_in_handler, sig_blocked_in_handler;
static i64 my_pid, my_uid;
static int pipe_fds[2];

static void record(int sig, siginfo_t *info)
{
	calls[sig]++;
	if (order_len < 60) {
		order[order_len++] = ' ';
		order[order_len++] = '0' + sig / 10;
		order[order_len++] = '0' + sig % 10;
	}
	if (info) {
		last_code = info->si_code;
		last_errno = info->si_errno;
		last_pid_is_me = info->si_pid == my_pid;
		last_uid_ok = (i64)info->si_uid == my_uid;
		last_value = info->si_value.sival_int;
	}
	u64 mask = 0;
	sys4(SYS_rt_sigprocmask, SIG_BLOCK, 0, &mask, 8);
	blocked_in_handler = mask != 0;
	sig_blocked_in_handler = (mask >> (sig - 1)) & 1;
}

static void on_sig(int sig, siginfo_t *info, void *uc)
{
	(void)uc;
	record(sig, info);
}

static void on_plain(int sig)
{
	record(sig, 0);
}

/* For SA_RESTART: the handler makes the interrupted read's data. */
static void on_alarm(int sig)
{
	record(sig, 0);
	sys3(SYS_write, pipe_fds[1], "!", 1);
}

/* For sigaltstack. */
static char altstack[65536] __attribute__((aligned(16)));
static volatile int on_alt, alt_flags, uc_stack_offset = -1;
static void on_alt_handler(int sig, siginfo_t *info, void *ucv)
{
	(void)info;
	char local;
	u64 *words = ucv;
	on_alt = &local >= altstack && &local < altstack + sizeof altstack;
	stack_t ss;
	sys2(SYS_sigaltstack, 0, &ss);
	alt_flags = ss.ss_flags;
	for (int w = 0; w < 38; w++)
		if (words[w] == (u64)altstack) {
			uc_stack_offset = w * 8;
			break;
		}
	record(sig, 0);
}

static u64 bit(int sig)
{
	return 1UL << (sig - 1);
}

static void mask_set(int how, u64 m)
{
	sys4(SYS_rt_sigprocmask, how, &m, 0, 8);
}

static u64 mask_now(void)
{
	u64 m = 0;
	sys4(SYS_rt_sigprocmask, SIG_BLOCK, 0, &m, 8);
	return m;
}

static void reset_record(void)
{
	order_len = 0;
	for (int i = 0; i < 65; i++)
		calls[i] = 0;
}

static void say_order(const char *name)
{
	put(name);
	put(":");
	for (int i = 0; i < order_len; i++)
		put_char(order[i]);
	end_line();
}

static void put_action(struct k_sigaction *sa)
{
	put("handler ");
	if (sa->handler == 0)
		put("SIG_DFL");
	else if (sa->handler == 1)
		put("SIG_IGN");
	else
		put("ours");
	put(", flags ");
	put_hex(sa->flags);
	put(", mask ");
	put_hex(sa->mask);
	put(", restorer ");
	put(sa->restorer == 0 ? "none" : sa->restorer == (u64)lx_restorer ? "ours" : "other");
}

static void say_action(const char *name, int sig)
{
	struct k_sigaction sa;
	lx_memset(&sa, 0, sizeof sa);
	i64 r = sys4(SYS_rt_sigaction, sig, 0, &sa, 8);
	put(name);
	put(" = ");
	if (r < 0)
		put_ret(r);
	else
		put_action(&sa);
	end_line();
}

static void layout_and_faults(void)
{
	say_text("-- a fault's SIGSEGV and the rt_sigframe");
	set_action(SIGSEGV, on_segv, SA_SIGINFO, 0);
	/* The interrupted code's mask, saved in uc_sigmask: a marker. */
	marker_mask = bit(SIGWINCH) | bit(SIGURG) | bit(SIGRTMIN + 3);
	mask_set(SIG_SETMASK, marker_mask);
	resume_to = fault_read_resume;
	u64 rax = fault_read(BAD_ADDR);
	u64 after = mask_now();
	mask_set(SIG_SETMASK, 0);
	say("handler calls", segv_count);
	say_hex("fault_read returned the handler's RAX", rax);
	say_bool("the interrupted mask is back after rt_sigreturn", after == marker_mask);
	say("si_signo", segv_signo);
	say("si_errno", segv_errno);
	say("si_code (SEGV_MAPERR)", segv_code);
	say_hex("si_addr", segv_addr);
	say("trapno (page fault)", segv_trapno);
	say("err (user read, not present)", segv_err);
	say_hex("cr2", segv_cr2);
	say_hex("cs", segv_cs);
	say_hex("uc_flags & (UC_SIGCONTEXT_SS|UC_STRICT_RESTORE_SS)", uc_flags & 6);
	say_bool("RIP was the faulting instruction", rip_was_insn);
	say_bool("the word below the ucontext is sa_restorer (pretcode)", restorer_below_uc);
	say("siginfo minus ucontext (bytes)", info_minus_uc);
	say_bool("fpstate 64-aligned, after siginfo", fpstate_ok);
	for (int r = 0; r < 14; r++) {
		put("ucontext offset of ");
		put(reg_names[r]);
		put(" = ");
		put_dec(reg_offsets[r]);
		end_line();
	}
	put("ucontext offsets of the fault address (rcx, cr2) =");
	for (int i = 0; i < n_addr_offsets; i++) {
		put(" ");
		put_dec(addr_offsets[i]);
	}
	end_line();
	put("ucontext offsets of the interrupted mask (oldmask, uc_sigmask) =");
	for (int i = 0; i < n_mask_offsets; i++) {
		put(" ");
		put_dec(mask_offsets[i]);
	}
	end_line();
	say("ucontext offset of rsp (just above the handler's stack)", rsp_offset);
	say("ucontext offset of rip (the faulting instruction)", rip_offset);
	say("ucontext offset of eflags (IF and bit 1 set)", eflags_offset);
	say("ucontext offset of cs (0x33 in the low 16 bits)", cs_offset);
	say("ucontext offset of err (4)", err_offset);
	say("ucontext offset of trapno (14)", trapno_offset);
	say("ucontext offset of the fpstate pointer", fpstate_offset);
	say("siginfo offset of si_addr", info_addr_offset);
	say("musl offsetof(ucontext_t, uc_stack)", offsetof(ucontext_t, uc_stack));
	say("musl offsetof(ucontext_t, uc_mcontext)", offsetof(ucontext_t, uc_mcontext));
	say("musl offsetof(ucontext_t, uc_sigmask)", offsetof(ucontext_t, uc_sigmask));
	say("musl offsetof(siginfo_t, si_code)", offsetof(siginfo_t, si_code));
	say("musl offsetof(siginfo_t, si_addr)", offsetof(siginfo_t, si_addr));
	say("musl offsetof(siginfo_t, si_pid)", offsetof(siginfo_t, si_pid));
	say("musl offsetof(siginfo_t, si_uid)", offsetof(siginfo_t, si_uid));
	say("musl offsetof(siginfo_t, si_status)", offsetof(siginfo_t, si_status));
	say("musl offsetof(siginfo_t, si_value)", offsetof(siginfo_t, si_value));
	say("sizeof(siginfo_t)", sizeof(siginfo_t));
	say("sizeof(stack_t)", sizeof(stack_t));

	/* A store to a read-only page: SEGV_ACCERR. */
	static char page[8192] __attribute__((aligned(4096)));
	*(volatile char *)page = 1; /* present before it is made read-only */
	sys3(SYS_mprotect, page, 4096, PROT_READ);
	segv_count = 0;
	resume_to = fault_write_resume;
	fault_write((u64)page + 24);
	sys3(SYS_mprotect, page, 4096, PROT_READ | PROT_WRITE);
	say("store to a read-only page: handler calls", segv_count);
	say("  si_code (SEGV_ACCERR)", segv_code);
	say_bool("  si_addr is the store's address", segv_addr == (u64)page + 24);
	say("  err (user write, present)", segv_err);
	say_bool("  RIP was the store", rip_was_insn);
	set_action(SIGSEGV, 0, 0, 0);

	/* A fault with SIGSEGV blocked is fatal whatever the handler. */
	i64 pid = lx_fork();
	if (pid == 0) {
		struct rlimit none = { 0, 0 };
		sys2(SYS_setrlimit, RLIMIT_CORE, &none);
		set_action(SIGSEGV, on_segv, SA_SIGINFO, 0);
		mask_set(SIG_BLOCK, bit(SIGSEGV));
		resume_to = fault_read_resume;
		fault_read(BAD_ADDR);
		lx_exit(0);
	}
	say_status("fault with SIGSEGV blocked and a handler", wait_for(pid, 0));
	pid = lx_fork();
	if (pid == 0) {
		struct rlimit none = { 0, 0 };
		sys2(SYS_setrlimit, RLIMIT_CORE, &none);
		char junk[512];
		lx_memset(junk, 0, sizeof junk);
		__asm__ volatile("mov %0, %%rsp\n mov $15, %%eax\n syscall" : : "r"(junk + 256) : "memory");
		lx_exit(0);
	}
	say_status("rt_sigreturn on a zeroed frame", wait_for(pid, 0));
}

/* Every signal's default action, observed: a child per signal. */
static void default_actions(void)
{
	say_text("-- default actions (a child per signal; core files off)");
	static const int sigs[] = { 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31, 34, 64 };
	for (unsigned i = 0; i < sizeof sigs / sizeof sigs[0]; i++) {
		int sig = sigs[i];
		int ready[2], gate[2];
		char c;
		sys2(SYS_pipe2, ready, 0);
		sys2(SYS_pipe2, gate, 0);
		i64 pid = lx_fork();
		if (pid == 0) {
			struct rlimit none = { 0, 0 };
			sys2(SYS_setrlimit, RLIMIT_CORE, &none);
			sys2(SYS_setpgid, 0, 0); /* not orphaned: stop signals stop */
			sys1(SYS_close, gate[1]);
			sys3(SYS_write, ready[1], "r", 1);
			while (sys3(SYS_read, gate[0], &c, 1) != 0)
				;
			lx_exit(0);
		}
		sys1(SYS_close, gate[0]);
		sys3(SYS_read, ready[0], &c, 1);
		sys2(SYS_kill, pid, sig);
		sys1(SYS_close, gate[1]);
		i64 st = wait_for(pid, WUNTRACED);
		put("signal ");
		put_dec(sig);
		put(": ");
		if (st >= 0 && (st & 0x7f) == 0 && ((st >> 8) & 0xff) == 0)
			put("ignored (the child exited 0)");
		else
			put_status(st);
		end_line();
		if (st >= 0 && (st & 0xff) == 0x7f) {
			sys2(SYS_kill, pid, SIGKILL);
			wait_for(pid, 0);
		}
		sys1(SYS_close, ready[0]);
		sys1(SYS_close, ready[1]);
	}
}

int main(void)
{
	lx_init();
	my_pid = sys0(SYS_getpid);
	my_uid = sys0(SYS_getuid);
	struct k_sigaction sa;

	say_text("-- rt_sigaction");
	say_action("SIGUSR1 at start", SIGUSR1);
	say_ret("set SIGUSR1 (SA_SIGINFO|SA_RESTART, mask SIGUSR2)", set_action(SIGUSR1, on_sig, SA_SIGINFO | SA_RESTART, bit(SIGUSR2)));
	say_action("SIGUSR1 now", SIGUSR1);
	say_ret("set SIGUSR1 with SIGKILL|SIGSTOP in the mask", set_action(SIGUSR1, on_sig, SA_SIGINFO, bit(SIGKILL) | bit(SIGSTOP) | bit(SIGUSR2)));
	say_action("  SIGUSR1 (mask)", SIGUSR1);
	say_ret("set SIGKILL", set_action(SIGKILL, on_sig, 0, 0));
	say_ret("set SIGSTOP to SIG_IGN", set_action(SIGSTOP, (void *)1, 0, 0));
	say_action("query SIGKILL", SIGKILL);
	say_ret("set signal 0", set_action(0, on_sig, 0, 0));
	say_ret("set signal 65", set_action(65, on_sig, 0, 0));
	sa.handler = 0;
	sa.flags = 0;
	sa.restorer = 0;
	sa.mask = 0;
	say_ret("rt_sigaction(sigsetsize 4)", sys4(SYS_rt_sigaction, SIGUSR1, &sa, 0, 4));
	say_ret("rt_sigaction(bad pointer)", sys4(SYS_rt_sigaction, SIGUSR1, 8, 0, 8));
	say_action("SIGCHLD", SIGCHLD);

	say_text("-- rt_sigprocmask");
	u64 all = ~0UL, got = 0;
	say_ret("SIG_SETMASK(all)", sys4(SYS_rt_sigprocmask, SIG_SETMASK, &all, &got, 8));
	say_hex("  previous", got);
	say_hex("  now (SIGKILL and SIGSTOP stay unblocked)", mask_now());
	mask_set(SIG_UNBLOCK, bit(SIGUSR1) | bit(SIGINT));
	say_hex("SIG_UNBLOCK(SIGUSR1|SIGINT)", mask_now());
	mask_set(SIG_SETMASK, 0);
	mask_set(SIG_BLOCK, bit(SIGTERM));
	say_hex("SIG_SETMASK(0), SIG_BLOCK(SIGTERM)", mask_now());
	say_ret("how = 3", sys4(SYS_rt_sigprocmask, 3, &all, 0, 8));
	say_ret("sigsetsize 4", sys4(SYS_rt_sigprocmask, SIG_BLOCK, &all, 0, 4));
	say_ret("how = 3 with no new set (ignored)", sys4(SYS_rt_sigprocmask, 3, 0, &got, 8));
	mask_set(SIG_SETMASK, 0);

	say_text("-- kill, tgkill, rt_sigqueueinfo and their siginfo");
	reset_record();
	say_ret("kill(self, SIGUSR1)", sys2(SYS_kill, my_pid, SIGUSR1));
	say("  handler calls", calls[SIGUSR1]);
	say("  si_code (SI_USER)", last_code);
	say_bool("  si_pid is us", last_pid_is_me);
	say_bool("  si_uid is getuid()", last_uid_ok);
	say_bool("  in the handler: SIGUSR1 blocked", sig_blocked_in_handler);
	say_hex("  mask after the handler", mask_now());
	say_ret("tgkill(self, self, SIGUSR1)", sys3(SYS_tgkill, my_pid, sys0(SYS_gettid), SIGUSR1));
	say("  si_code (SI_TKILL)", last_code);
	say_bool("  si_pid is us", last_pid_is_me);
	siginfo_t qi;
	lx_memset(&qi, 0, sizeof qi);
	qi.si_signo = SIGUSR1;
	qi.si_code = SI_QUEUE;
	qi.si_pid = my_pid;
	qi.si_uid = my_uid;
	qi.si_value.sival_int = 0x1234567;
	say_ret("rt_sigqueueinfo(self, SIGUSR1, SI_QUEUE)", sys3(SYS_rt_sigqueueinfo, my_pid, SIGUSR1, &qi));
	say("  si_code (SI_QUEUE)", last_code);
	say_hex("  si_value", last_value);
	say_ret("kill(self, 0)", sys2(SYS_kill, my_pid, 0));
	say_ret("kill(self, 65)", sys2(SYS_kill, my_pid, 65));
	say_ret("kill(nonexistent, SIGUSR1)", sys2(SYS_kill, 0x3fffffff, SIGUSR1));
	say_ret("tgkill(self, nonexistent tid)", sys3(SYS_tgkill, my_pid, 0x3fffffff, SIGUSR1));
	say_ret("tgkill(0, self)", sys3(SYS_tgkill, 0, my_pid, SIGUSR1));
	say("handler calls in all", calls[SIGUSR1]);

	say_text("-- pending signals");
	reset_record();
	set_action(SIGUSR2, on_sig, SA_SIGINFO, 0);
	set_action(SIGRTMIN, on_sig, SA_SIGINFO, 0);
	mask_set(SIG_BLOCK, bit(SIGUSR1) | bit(SIGUSR2) | bit(SIGRTMIN));
	sys2(SYS_kill, my_pid, SIGUSR2);
	sys2(SYS_kill, my_pid, SIGUSR1);
	sys2(SYS_kill, my_pid, SIGUSR1);
	sys2(SYS_kill, my_pid, SIGRTMIN);
	sys2(SYS_kill, my_pid, SIGRTMIN);
	u64 pending = 0;
	sys2(SYS_rt_sigpending, &pending, 8);
	say_hex("blocked, sent USR2, USR1 x2, RTMIN x2: rt_sigpending", pending);
	say("  handler calls so far", order_len / 3);
	mask_set(SIG_UNBLOCK, bit(SIGUSR1) | bit(SIGUSR2) | bit(SIGRTMIN));
	/* SIGUSR1's frame is set up first (its sa_mask blocks SIGUSR2), the
	 * real-time ones nest on it before it runs; SIGUSR2 comes when
	 * SIGUSR1's handler returns. */
	say_order("  unblocked: the handlers ran in this order (RTMIN queued twice)");
	sys2(SYS_rt_sigpending, &pending, 8);
	say_hex("  rt_sigpending now", pending);
	say_ret("rt_sigpending(sigsetsize 16)", sys2(SYS_rt_sigpending, &pending, 16));
	set_action(SIGRTMIN, 0, 0, 0);

	reset_record();
	mask_set(SIG_BLOCK, bit(SIGUSR1));
	sys2(SYS_kill, my_pid, SIGUSR1);
	u64 none = 0;
	say_ret("rt_sigsuspend(empty) with SIGUSR1 pending", sys2(SYS_rt_sigsuspend, &none, 8));
	say("  handler calls", calls[SIGUSR1]);
	say_hex("  mask after (restored)", mask_now());
	siginfo_t wi;
	struct lx_ts ts = { 0, 10000000 };
	u64 usr1 = bit(SIGUSR1);
	say_ret("rt_sigtimedwait(SIGUSR1, 10 ms) with none pending", sys4(SYS_rt_sigtimedwait, &usr1, &wi, &ts, 8));
	sys3(SYS_tgkill, my_pid, sys0(SYS_gettid), SIGUSR1);
	lx_memset(&wi, 0, sizeof wi);
	say_ret("rt_sigtimedwait(SIGUSR1) after tgkill", sys4(SYS_rt_sigtimedwait, &usr1, &wi, &ts, 8));
	say("  si_signo", wi.si_signo);
	say("  si_code", wi.si_code);
	say("  handler calls (none: taken, not delivered)", calls[SIGUSR1]);
	mask_set(SIG_SETMASK, 0);

	say_text("-- SA_NODEFER, SA_RESETHAND, sa_mask");
	reset_record();
	set_action(SIGUSR2, on_sig, SA_SIGINFO, bit(SIGTERM));
	sys2(SYS_kill, my_pid, SIGUSR2);
	say_bool("plain: SIGUSR2 blocked in its handler", sig_blocked_in_handler);
	set_action(SIGUSR2, on_sig, SA_SIGINFO | SA_NODEFER, bit(SIGTERM));
	sys2(SYS_kill, my_pid, SIGUSR2);
	say_bool("SA_NODEFER: SIGUSR2 blocked in its handler", sig_blocked_in_handler);
	say_bool("  sa_mask's SIGTERM blocked in it", blocked_in_handler);
	set_action(SIGUSR2, on_sig, SA_SIGINFO | SA_RESETHAND, 0);
	sys2(SYS_kill, my_pid, SIGUSR2);
	say_bool("SA_RESETHAND: SIGUSR2 blocked in its handler", sig_blocked_in_handler);
	say_action("  SIGUSR2 after", SIGUSR2);
	say("  handler calls", calls[SIGUSR2]);
	set_action(SIGUSR2, (void *)1, 0, 0);
	say_ret("SIG_IGN: kill(self, SIGUSR2)", sys2(SYS_kill, my_pid, SIGUSR2));
	mask_set(SIG_BLOCK, bit(SIGUSR2));
	sys2(SYS_kill, my_pid, SIGUSR2);
	sys2(SYS_rt_sigpending, &pending, 8);
	say_bool("  blocked and ignored: left pending", (pending & bit(SIGUSR2)) != 0);
	mask_set(SIG_SETMASK, 0);
	sys2(SYS_rt_sigpending, &pending, 8);
	say_bool("  unblocked: discarded", (pending & bit(SIGUSR2)) == 0);
	set_action(SIGUSR2, 0, 0, 0);

	say_text("-- SA_RESTART");
	sys2(SYS_pipe2, pipe_fds, 0);
	struct {
		i64 isec, iusec, sec, usec;
	} it = { 0, 0, 0, 30000 };
	char c = 0;
	reset_record();
	set_action(SIGALRM, on_alarm, 0, 0);
	sys3(SYS_setitimer, 0, &it, 0);
	say_ret("read(empty pipe), SIGALRM without SA_RESTART", sys3(SYS_read, pipe_fds[0], &c, 1));
	say_read("  then read", sys3(SYS_read, pipe_fds[0], &c, 1), &c);
	set_action(SIGALRM, on_alarm, SA_RESTART, 0);
	sys3(SYS_setitimer, 0, &it, 0);
	say_read("read(empty pipe), SIGALRM with SA_RESTART", sys3(SYS_read, pipe_fds[0], &c, 1), &c);
	sys3(SYS_setitimer, 0, &it, 0);
	struct lx_ts long_sleep = { 5, 0 }, rem = { 0, 0 };
	say_ret("nanosleep(5 s), SIGALRM with SA_RESTART (never restarted)", sys2(SYS_nanosleep, &long_sleep, &rem));
	say_bool("  remaining time written, about 5 s", rem.sec == 4 && rem.nsec > 0);
	say("  SIGALRM handler calls", calls[SIGALRM]);
	sys3(SYS_read, pipe_fds[0], &c, 1);
	set_action(SIGALRM, 0, 0, 0);

	say_text("-- sigaltstack");
	stack_t ss = { 0 }, old;
	say_ret("sigaltstack(query)", sys2(SYS_sigaltstack, 0, &old));
	say("  ss_flags (SS_DISABLE)", old.ss_flags);
	ss.ss_sp = altstack;
	ss.ss_size = 1024;
	say_ret("sigaltstack(1 KiB)", sys2(SYS_sigaltstack, &ss, 0));
	ss.ss_size = sizeof altstack;
	ss.ss_flags = 0x10;
	say_ret("sigaltstack(bad flags)", sys2(SYS_sigaltstack, &ss, 0));
	ss.ss_flags = 0;
	say_ret("sigaltstack(64 KiB)", sys2(SYS_sigaltstack, &ss, 0));
	set_action(SIGUSR1, on_alt_handler, SA_SIGINFO | SA_ONSTACK, 0);
	sys2(SYS_kill, my_pid, SIGUSR1);
	say_bool("SA_ONSTACK handler ran on the alternate stack", on_alt);
	say("  sigaltstack's ss_flags in it (SS_ONSTACK)", alt_flags);
	say("  ucontext offset of uc_stack.ss_sp", uc_stack_offset);
	sys2(SYS_sigaltstack, 0, &old);
	say("  ss_flags after", old.ss_flags);
	set_action(SIGUSR1, on_alt_handler, SA_SIGINFO, 0);
	sys2(SYS_kill, my_pid, SIGUSR1);
	say_bool("without SA_ONSTACK: on the alternate stack", on_alt);
	ss.ss_flags = SS_DISABLE;
	say_ret("sigaltstack(SS_DISABLE)", sys2(SYS_sigaltstack, &ss, 0));
	set_action(SIGUSR1, 0, 0, 0);

	say_text("-- SIGCHLD");
	mask_set(SIG_BLOCK, bit(SIGCHLD));
	i64 pid = lx_fork();
	if (pid == 0)
		lx_exit(7);
	u64 chld = bit(SIGCHLD);
	struct lx_ts five = { 5, 0 };
	lx_memset(&wi, 0, sizeof wi);
	say_ret("rt_sigtimedwait(SIGCHLD) after a child's exit(7)", sys4(SYS_rt_sigtimedwait, &chld, &wi, &five, 8));
	say("  si_code (CLD_EXITED)", wi.si_code);
	say_bool("  si_pid is the child", wi.si_pid == pid);
	say("  si_status", wi.si_status);
	int *iw = (int *)&wi;
	int pid_off = -1, status_off = -1;
	for (int w = 3; w < 32; w++) {
		if (pid_off < 0 && iw[w] == pid)
			pid_off = w * 4;
		else if (status_off < 0 && iw[w] == 7)
			status_off = w * 4;
	}
	say("  siginfo offset of the child's pid", pid_off);
	say("  siginfo offset of the exit status", status_off);
	say_status("  wait4", wait_for(pid, 0));
	mask_set(SIG_SETMASK, 0);
	reset_record();
	set_action(SIGCHLD, on_sig, SA_SIGINFO, 0);
	pid = lx_fork();
	if (pid == 0)
		lx_exit(8);
	say_status("handler: a child's exit(8)", wait_for(pid, 0));
	sleep_ms(1);
	say("  SIGCHLD handler calls", calls[SIGCHLD]);
	say("  si_code", last_code);
	set_action(SIGCHLD, (void *)1, 0, 0);
	pid = lx_fork();
	if (pid == 0)
		lx_exit(9);
	say_ret("SIGCHLD ignored: wait4 (no zombie is left)", wait_for(pid, 0));
	set_action(SIGCHLD, 0, 0, 0);

	layout_and_faults();
	default_actions();
	(void)on_plain;
	say_text("signal_test: ok");
	return 0;
}
