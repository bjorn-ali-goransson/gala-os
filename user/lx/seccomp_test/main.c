/* seccomp_test: seccomp(2) and prctl's PR_GET_SECCOMP/PR_SET_SECCOMP
 * (ROADMAP LD: sandboxes install filters: bubblewrap, glycin's loaders,
 * browsers; ENOSYS in gala). The probes, the checks a filter passes
 * before it is installed, and filters at work in children: ERRNO, TRAP
 * (SIGSYS and its siginfo), KILL_PROCESS, the strictest of two filters,
 * a filter kept across fork, and strict mode.
 *
 * Every filter is installed with no_new_privs set, so root and a user
 * see the same. */
#include "../common/lx.h"
#include "../common/more.h"

#define PR_SET_NO_NEW_PRIVS 38
#define PR_GET_SECCOMP 21
#define PR_SET_SECCOMP 22
#define SECCOMP_SET_MODE_STRICT 0
#define SECCOMP_SET_MODE_FILTER 1
#define SECCOMP_GET_ACTION_AVAIL 2
#define SECCOMP_GET_NOTIF_SIZES 3
#define SECCOMP_RET_KILL_PROCESS 0x80000000u
#define SECCOMP_RET_KILL_THREAD 0x00000000u
#define SECCOMP_RET_TRAP 0x00030000u
#define SECCOMP_RET_ERRNO 0x00050000u
#define SECCOMP_RET_USER_NOTIF 0x7fc00000u
#define SECCOMP_RET_TRACE 0x7ff00000u
#define SECCOMP_RET_LOG 0x7ffc0000u
#define SECCOMP_RET_ALLOW 0x7fff0000u
#if defined(__x86_64__)
#define AUDIT_ARCH_X86_64 0xc000003eu
#else
/* The filter's architecture check is aarch64's there (AUDIT_ARCH_AARCH64:
 * EM_AARCH64 with the 64-bit and little-endian bits). */
#define AUDIT_ARCH_X86_64 0xc00000b7u
#endif
#define SIGSYS 31

/* Classic BPF (linux/filter.h). */
#define BPF_LD 0x00
#define BPF_W 0x00
#define BPF_ABS 0x20
#define BPF_JMP 0x05
#define BPF_JEQ 0x10
#define BPF_K 0x00
#define BPF_RET 0x06
#define BPF_ALU 0x04
#define BPF_DIV 0x30

struct sock_filter {
	unsigned short code;
	unsigned char jt, jf;
	unsigned k;
};
struct sock_fprog {
	unsigned short len;
	struct sock_filter *filter;
};

#define STMT(c, k) { (unsigned short)(c), 0, 0, (k) }
#define JUMP(c, k, t, f) { (unsigned short)(c), (t), (f), (k) }
/* seccomp_data: nr at 0, arch at 4. */
#define LOAD_ARCH STMT(BPF_LD | BPF_W | BPF_ABS, 4)
#define LOAD_NR STMT(BPF_LD | BPF_W | BPF_ABS, 0)

static i64 install(struct sock_filter *f, unsigned short n)
{
	struct sock_fprog p = { n, f };
	return sys3(SYS_seccomp, SECCOMP_SET_MODE_FILTER, 0, &p);
}

/* A filter: for syscall `nr`, return `action`; allow the rest (on x86_64;
 * kill another architecture's calls). */
static i64 on(unsigned nr, unsigned action)
{
	struct sock_filter f[] = {
		LOAD_ARCH,
		JUMP(BPF_JMP | BPF_JEQ | BPF_K, AUDIT_ARCH_X86_64, 1, 0),
		STMT(BPF_RET | BPF_K, SECCOMP_RET_KILL_PROCESS),
		LOAD_NR,
		JUMP(BPF_JMP | BPF_JEQ | BPF_K, nr, 0, 1),
		STMT(BPF_RET | BPF_K, action),
		STMT(BPF_RET | BPF_K, SECCOMP_RET_ALLOW),
	};
	return install(f, sizeof f / sizeof *f);
}

/* siginfo_t's head (si_signo, si_errno, si_code). */
struct si {
	int signo, err, code;
};

static volatile int sys_signo, sys_code, sys_errno, sys_nr_ok, sys_arch_ok;

static void on_sigsys(int sig, struct si *si, void *uc)
{
	(void)uc;
	sys_signo = sig;
	sys_code = si->code;
	sys_errno = si->err;
	/* _sigsys: si_call_addr (8 bytes at 16), si_syscall (int at 24),
	 * si_arch (unsigned at 28). */
	int nr = *(int *)((char *)si + 24);
	unsigned arch = *(unsigned *)((char *)si + 28);
	sys_nr_ok = nr == SYS_getuid;
	sys_arch_ok = arch == AUDIT_ARCH_X86_64;
}

static void say_child(const char *name, i64 st)
{
	say_status(name, st);
}

int main(void)
{
	lx_init();
	say_text("-- probes");
	say("prctl(PR_GET_SECCOMP)", sys2(SYS_prctl, PR_GET_SECCOMP, 0));
	unsigned actions[] = { SECCOMP_RET_KILL_PROCESS, SECCOMP_RET_KILL_THREAD, SECCOMP_RET_TRAP, SECCOMP_RET_ERRNO,
			       SECCOMP_RET_USER_NOTIF,	 SECCOMP_RET_TRACE,	  SECCOMP_RET_LOG,  SECCOMP_RET_ALLOW };
	int all = 1;
	for (unsigned i = 0; i < sizeof actions / sizeof *actions; i++)
		if (sys3(SYS_seccomp, SECCOMP_GET_ACTION_AVAIL, 0, &actions[i]) != 0)
			all = 0;
	say_bool("GET_ACTION_AVAIL: every action", all);
	unsigned odd = 0x12340000;
	say_ret("GET_ACTION_AVAIL(0x12340000)", sys3(SYS_seccomp, SECCOMP_GET_ACTION_AVAIL, 0, &odd));
	say_ret("GET_ACTION_AVAIL(flags 1)", sys3(SYS_seccomp, SECCOMP_GET_ACTION_AVAIL, 1, &actions[0]));
	unsigned short sizes[3] = { 0, 0, 0 };
	say_ret("GET_NOTIF_SIZES", sys3(SYS_seccomp, SECCOMP_GET_NOTIF_SIZES, 0, sizes));
	say("  seccomp_notif", sizes[0]);
	say("  seccomp_notif_resp", sizes[1]);
	say("  seccomp_data", sizes[2]);
	say_ret("seccomp(op 99)", sys3(SYS_seccomp, 99, 0, 0));

	say_text("-- installing");
	say_ret("PR_SET_NO_NEW_PRIVS", sys5(SYS_prctl, PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0));
	say_ret("SET_MODE_FILTER(NULL)", sys3(SYS_seccomp, SECCOMP_SET_MODE_FILTER, 0, 0));
	struct sock_filter allow[] = { STMT(BPF_RET | BPF_K, SECCOMP_RET_ALLOW) };
	struct sock_fprog one = { 1, allow };
	say_ret("SET_MODE_FILTER(flags 0x100)", sys3(SYS_seccomp, SECCOMP_SET_MODE_FILTER, 0x100, &one));
	say_ret("a filter of no instructions", install(allow, 0));
	struct sock_filter no_ret[] = { LOAD_NR };
	say_ret("a filter without a final return", install(no_ret, 1));
	struct sock_filter far[] = { JUMP(BPF_JMP | BPF_JEQ | BPF_K, 1, 5, 0), STMT(BPF_RET | BPF_K, SECCOMP_RET_ALLOW) };
	say_ret("a jump out of the filter", install(far, 2));
	struct sock_filter odd_load[] = { STMT(BPF_LD | BPF_W | BPF_ABS, 2), STMT(BPF_RET | BPF_K, SECCOMP_RET_ALLOW) };
	say_ret("a load not on a word", install(odd_load, 2));
	struct sock_filter past[] = { STMT(BPF_LD | BPF_W | BPF_ABS, 64), STMT(BPF_RET | BPF_K, SECCOMP_RET_ALLOW) };
	say_ret("a load past seccomp_data", install(past, 2));
	struct sock_filter div0[] = { STMT(BPF_ALU | BPF_DIV | BPF_K, 0), STMT(BPF_RET | BPF_K, SECCOMP_RET_ALLOW) };
	say_ret("a division by a constant 0", install(div0, 2));

	say_text("-- filters at work (in children)");
	i64 pid = lx_fork();
	if (pid == 0) {
		on(SYS_getppid, SECCOMP_RET_ERRNO | 42);
		i64 r = sys0(SYS_getppid);
		i64 mode = sys2(SYS_prctl, PR_GET_SECCOMP, 0);
		lx_exit(r == -42 && mode == 2 ? 7 : 1);
	}
	say_child("ERRNO(42) on getppid: getppid() is -42, PR_GET_SECCOMP 2 (exit 7)", wait_for(pid, 0));

	pid = lx_fork();
	if (pid == 0) {
		set_action(SIGSYS, (void *)on_sigsys, 4 /* SA_SIGINFO */, 0);
		on(SYS_getuid, SECCOMP_RET_TRAP | 9);
		sys0(SYS_getuid);
		int ok = sys_signo == SIGSYS && sys_code == 1 /* SYS_SECCOMP */ && sys_errno == 9 && sys_nr_ok && sys_arch_ok;
		lx_exit(ok ? 7 : 1);
	}
	say_child("TRAP on getuid: SIGSYS, SYS_SECCOMP, its data, the call and arch (exit 7)", wait_for(pid, 0));

	pid = lx_fork();
	if (pid == 0) {
		on(SYS_getuid, SECCOMP_RET_KILL_PROCESS);
		sys0(SYS_getuid);
		lx_exit(1);
	}
	say_child("KILL_PROCESS on getuid", wait_for(pid, 0));

	pid = lx_fork();
	if (pid == 0) {
		on(SYS_getppid, SECCOMP_RET_ERRNO | 5);
		on(SYS_getppid, SECCOMP_RET_ERRNO | 6);
		on(SYS_getppid, SECCOMP_RET_ALLOW);
		/* The lowest action value wins; ERRNO 5 and 6 tie on the
		 * action: the first in precedence (the newest filter) is 6. */
		lx_exit((int)-sys0(SYS_getppid));
	}
	say_child("three filters on getppid: ERRNO(5), ERRNO(6), ALLOW", wait_for(pid, 0));

	pid = lx_fork();
	if (pid == 0) {
		on(SYS_getppid, SECCOMP_RET_ERRNO | 3);
		i64 g = lx_fork();
		if (g == 0)
			lx_exit((int)-sys0(SYS_getppid));
		lx_exit((int)((wait_for(g, 0) >> 8) & 0xff));
	}
	say_child("a forked child keeps the filter: ERRNO(3)", wait_for(pid, 0));

	pid = lx_fork();
	if (pid == 0) {
		on(SYS_getppid, SECCOMP_RET_TRACE);
		i64 r = sys0(SYS_getppid);
		lx_exit(r == -38 ? 7 : 1);
	}
	say_child("TRACE without a tracer: ENOSYS (exit 7)", wait_for(pid, 0));

	pid = lx_fork();
	if (pid == 0) {
		sys3(SYS_seccomp, SECCOMP_SET_MODE_STRICT, 0, 0);
		sys3(SYS_write, 1, "strict: write works\n", 20);
		sys0(SYS_getppid);
		lx_exit(1);
	}
	say_child("strict mode, then getppid", wait_for(pid, 0));
	say_text("seccomp_test: ok");
	return 0;
}
