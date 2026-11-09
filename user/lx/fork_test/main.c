/* fork_test: fork/vfork/execve/wait4 as the raw system calls give them:
 * exit statuses (WIFEXITED/WIFSIGNALED/WIFSTOPPED/WIFCONTINUED), copied
 * versus shared memory, execve of itself with a new argv and envp (and
 * what survives it: close-on-exec), its errors, wait4's WNOHANG and
 * rusage, waitid's WNOWAIT, ECHILD, zombies, process groups and
 * sessions, and getpid/getppid relations (printed as relations: pids
 * vary), orphans reparented to a child subreaper.
 *
 * Children report through their exit status or a pipe; the parent waits
 * for each before printing on, so the transcript's order is fixed. Run
 * as `./fork_test exec-child ...` it is the execve'd image. */
#define _GNU_SOURCE
#include "../common/lx.h"
#include "../common/more.h"
#include <fcntl.h>
#include <signal.h>
#include <sys/resource.h>
#include <sys/wait.h>

static volatile int shared_value;

/* vfork(2) whose child stores `value` at `p` and exits with `status`
 * without touching the stack (it is the parent's until the child exits). */
i64 vfork_store(volatile int *p, int value, int status);
__asm__(".text\n"
	".globl vfork_store\n"
	"vfork_store:\n"
	"	mov %rdi, %r8\n"
	"	mov $58, %eax\n" /* vfork */
	"	syscall\n"
	"	test %rax, %rax\n"
	"	jnz 1f\n"
	"	mov %esi, (%r8)\n"
	"	mov %edx, %edi\n"
	"	mov $231, %eax\n" /* exit_group */
	"	syscall\n"
	"	hlt\n"
	"1:	ret\n");

static int exec_child(int argc, char **argv, char **envp)
{
	put("exec child: argc = ");
	put_dec(argc);
	end_line();
	for (int i = 0; i < argc; i++) {
		put("exec child: argv[");
		put_dec(i);
		put("] = ");
		put_quoted(argv[i], lx_strlen(argv[i]));
		end_line();
	}
	int n = 0;
	for (; envp[n]; n++) {
		put("exec child: envp[");
		put_dec(n);
		put("] = ");
		put_quoted(envp[n], lx_strlen(envp[n]));
		end_line();
	}
	say("exec child: envc", n);
	say_ret("exec child: F_GETFD(3) (opened without O_CLOEXEC)", sys2(SYS_fcntl, 3, F_GETFD));
	say_ret("exec child: F_GETFD(4) (opened with O_CLOEXEC)", sys2(SYS_fcntl, 4, F_GETFD));
	/* A caught signal is reset to its default by execve; an ignored one
	 * stays ignored. */
	struct k_sigaction sa;
	sys4(SYS_rt_sigaction, SIGUSR1, 0, &sa, 8);
	say("exec child: SIGUSR1 handler (was caught)", sa.handler);
	sys4(SYS_rt_sigaction, SIGUSR2, 0, &sa, 8);
	say("exec child: SIGUSR2 handler (was ignored)", sa.handler);
	u64 mask = 0;
	sys4(SYS_rt_sigprocmask, SIG_BLOCK, 0, &mask, 8);
	say_bool("exec child: SIGTERM still blocked", (mask & (1UL << (SIGTERM - 1))) != 0);
	return 3;
}

static void on_usr1(int sig)
{
	(void)sig;
}

/* A child that runs `body` and exits with its result. */
#define CHILD(body)                     \
	({                              \
		i64 pid_ = lx_fork();   \
		if (pid_ == 0)          \
			lx_exit(body);  \
		pid_;                   \
	})

int main(int argc, char **argv, char **envp)
{
	if (argc > 1 && lx_streq(argv[1], "exec-child"))
		return exec_child(argc, argv, envp);
	lx_init();
	i64 me = sys0(SYS_getpid);
	int fds[2];
	char buf[64];

	say_text("-- exit statuses");
	i64 pid = CHILD(0);
	say_bool("fork returned a pid > 0", pid > 0);
	say_status("exit(0)", wait_for(pid, 0));
	pid = CHILD(7);
	say_status("exit(7)", wait_for(pid, 0));
	pid = CHILD(255);
	say_status("exit(255)", wait_for(pid, 0));
	pid = CHILD(256);
	say_status("exit(256)", wait_for(pid, 0));
	pid = CHILD(-1);
	say_status("exit(-1)", wait_for(pid, 0));
	pid = lx_fork();
	if (pid == 0) {
		sys1(SYS_exit, 9); /* exit(2), not exit_group: the only thread */
		lx_exit(1);
	}
	say_status("exit (not exit_group) 9", wait_for(pid, 0));
	pid = lx_fork();
	if (pid == 0) {
		sys2(SYS_kill, sys0(SYS_getpid), SIGKILL);
		lx_exit(1);
	}
	say_status("kill(self, SIGKILL)", wait_for(pid, 0));
	pid = lx_fork();
	if (pid == 0) {
		sys2(SYS_kill, sys0(SYS_getpid), SIGTERM);
		lx_exit(1);
	}
	say_status("kill(self, SIGTERM)", wait_for(pid, 0));
	pid = lx_fork();
	if (pid == 0) {
		struct rlimit none = { 0, 0 };
		sys2(SYS_setrlimit, RLIMIT_CORE, &none);
		volatile int *bad = 0;
		__asm__ volatile("" : "+r"(bad));
		bad[2] = 1;
		lx_exit(1);
	}
	say_status("store to address 8", wait_for(pid, 0));
	/* The child exits only once the parent has seen it continued: an
	 * exit before that wait replaces the continued state (exited 5 for
	 * WCONTINUED, then ECHILD), as a busy host showed. */
	int go[2];
	sys2(SYS_pipe2, go, 0);
	pid = lx_fork();
	if (pid == 0) {
		char b;
		sys2(SYS_kill, sys0(SYS_getpid), SIGSTOP);
		sys3(SYS_read, go[0], &b, 1);
		lx_exit(5);
	}
	say_status("kill(self, SIGSTOP), WUNTRACED", wait_for(pid, WUNTRACED));
	say_ret("  kill(child, SIGCONT)", sys2(SYS_kill, pid, SIGCONT));
	say_status("  WCONTINUED", wait_for(pid, WCONTINUED));
	sys3(SYS_write, go[1], "g", 1);
	sys1(SYS_close, go[0]);
	sys1(SYS_close, go[1]);
	say_status("  then", wait_for(pid, 0));

	say_text("-- memory: fork copies, vfork shares");
	shared_value = 1;
	pid = lx_fork();
	if (pid == 0) {
		shared_value = 2;
		lx_exit(shared_value);
	}
	say_status("fork child set the value to 2", wait_for(pid, 0));
	say("  parent's value", shared_value);
	pid = vfork_store(&shared_value, 3, 4);
	say_bool("vfork returned a pid > 0", pid > 0);
	say("  parent's value after the vfork child stored 3", shared_value);
	say_status("  vfork child", wait_for(pid, 0));

	say_text("-- wait4 and waitid");
	say_ret("wait4(-1) with no children", wait_for(-1, 0));
	say_ret("wait4(1) (not a child)", wait_for(1, 0));
	say_ret("wait4(-1, bad options)", wait_for(-1, 0x4000));
	sys2(SYS_pipe2, fds, O_CLOEXEC);
	pid = lx_fork();
	if (pid == 0) {
		sys1(SYS_close, fds[1]);
		/* Report our pid and parent's: the parent compares. */
		sys3(SYS_read, fds[0], buf, 1);
		lx_exit(0);
	}
	say_ret("wait4(child, WNOHANG) while it runs", wait_for(pid, WNOHANG));
	sys1(SYS_close, fds[0]);
	sys1(SYS_close, fds[1]);
	struct rusage ru;
	lx_memset(&ru, 0xff, sizeof ru); /* -1 everywhere: what wait4 leaves alone */
	int status = 0;
	i64 r = sys4(SYS_wait4, pid, &status, 0, &ru);
	say_bool("wait4(child, &ru) returned the child's pid", r == pid);
	say_status("  status", status);
	say_bool("  ru_maxrss written (>= 0)", ru.ru_maxrss >= 0);
	say_bool("  ru_nivcsw written (>= 0)", ru.ru_nivcsw >= 0);
	say_bool("  ru_utime plausible", ru.ru_utime.tv_sec >= 0 && ru.ru_utime.tv_usec >= 0 && ru.ru_utime.tv_usec < 1000000);
	say_bool("  ru_stime plausible", ru.ru_stime.tv_sec >= 0 && ru.ru_stime.tv_usec >= 0 && ru.ru_stime.tv_usec < 1000000);
	/* struct rusage as the kernel writes it: 144 bytes (musl's has 128
	 * reserved bytes more). */
	say("offsetof(rusage, ru_utime)", __builtin_offsetof(struct rusage, ru_utime));
	say("offsetof(rusage, ru_stime)", __builtin_offsetof(struct rusage, ru_stime));
	say("offsetof(rusage, ru_maxrss)", __builtin_offsetof(struct rusage, ru_maxrss));
	say("offsetof(rusage, ru_minflt)", __builtin_offsetof(struct rusage, ru_minflt));
	say("offsetof(rusage, ru_nvcsw)", __builtin_offsetof(struct rusage, ru_nvcsw));
	say("offsetof(rusage, ru_nivcsw)", __builtin_offsetof(struct rusage, ru_nivcsw));
	static unsigned char rbuf[512];
	u64 written = 0;
	for (int fill = 0x55; fill <= 0xaa; fill += 0x55) {
		lx_memset(rbuf, fill, sizeof rbuf);
		sys2(SYS_getrusage, RUSAGE_SELF, rbuf);
		u64 w = written_size(rbuf, sizeof rbuf, fill);
		if (w > written)
			written = w;
	}
	say("getrusage writes bytes", written);
	pid = CHILD(6);
	siginfo_t si;
	lx_memset(&si, 0, sizeof si);
	r = sys5(SYS_waitid, P_PID, pid, &si, WEXITED | WNOWAIT, 0);
	say_ret("waitid(P_PID, WEXITED|WNOWAIT)", r);
	say_bool("  si_pid is the child", si.si_pid == pid);
	say("  si_signo", si.si_signo);
	say("  si_code (CLD_EXITED)", si.si_code);
	say("  si_status", si.si_status);
	say_status("  the zombie is still there: wait4", wait_for(pid, 0));
	say_ret("  then wait4 again", wait_for(pid, 0));
	pid = lx_fork();
	if (pid == 0) {
		sys2(SYS_kill, sys0(SYS_getpid), SIGTERM);
		lx_exit(1);
	}
	lx_memset(&si, 0, sizeof si);
	say_ret("waitid(P_ALL, WEXITED) of a killed child", sys5(SYS_waitid, P_ALL, 0, &si, WEXITED, 0));
	say("  si_code (CLD_KILLED)", si.si_code);
	say("  si_status (the signal)", si.si_status);
	say_ret("waitid(P_ALL, no WEXITED etc.)", sys5(SYS_waitid, P_ALL, 0, &si, 0, 0));

	/* Several zombies, reaped in any order: count them. */
	for (int i = 0; i < 4; i++)
		CHILD(10 + i);
	int sum = 0, reaped = 0;
	while ((r = sys4(SYS_wait4, -1, &status, 0, 0)) > 0) {
		reaped++;
		sum += WEXITSTATUS(status);
	}
	say("four zombies reaped", reaped);
	say("  their statuses' sum", sum);
	say_ret("  then wait4(-1)", r);

	say_text("-- pids, groups, sessions");
	pid = CHILD(sys0(SYS_getppid) == me ? 1 : 2);
	say_status("child: getppid() is the parent (1 = yes)", wait_for(pid, 0));
	sys2(SYS_pipe2, fds, 0);
	pid = lx_fork();
	if (pid == 0) {
		i64 self = sys0(SYS_getpid);
		sys3(SYS_write, fds[1], &self, sizeof self);
		lx_exit(0);
	}
	i64 reported = 0;
	sys3(SYS_read, fds[0], &reported, sizeof reported);
	say_bool("fork's result is the child's getpid()", reported == pid);
	wait_for(pid, 0);
	sys1(SYS_close, fds[0]);
	sys1(SYS_close, fds[1]);
	say_bool("gettid() = getpid() (one thread)", sys0(SYS_gettid) == me);
	i64 pgrp = sys1(SYS_getpgid, 0);
	say_bool("getpgrp() = getpgid(0)", sys0(SYS_getpgrp) == pgrp);
	say_bool("getpgid(self) = getpgid(0)", sys1(SYS_getpgid, me) == pgrp);
	say_ret("getpgid(nonexistent)", sys1(SYS_getpgid, 0x3fffffff));
	say_ret("getsid(nonexistent)", sys1(SYS_getsid, 0x3fffffff));
	pid = CHILD(sys1(SYS_getpgid, 0) == pgrp ? 1 : 2);
	say_status("child: same process group (1 = yes)", wait_for(pid, 0));
	pid = lx_fork();
	if (pid == 0) {
		i64 self = sys0(SYS_getpid);
		int bits = 0;
		bits |= sys2(SYS_setpgid, 0, 0) == 0;
		bits |= (sys1(SYS_getpgid, 0) == self) << 1;
		bits |= (sys0(SYS_setsid) == -1) << 2; /* EPERM: a group leader */
		lx_exit(bits);
	}
	say_status("child: setpgid(0,0)=0 | pgid=pid<<1 | setsid EPERM<<2", wait_for(pid, 0));
	pid = lx_fork();
	if (pid == 0) {
		i64 self = sys0(SYS_getpid);
		int bits = 0;
		bits |= sys0(SYS_setsid) == self;
		bits |= (sys1(SYS_getsid, 0) == self) << 1;
		bits |= (sys1(SYS_getpgid, 0) == self) << 2;
		bits |= (sys0(SYS_setsid) == -1) << 3;
		bits |= (sys2(SYS_setpgid, 0, pgrp) == -1) << 4; /* EPERM: other session */
		lx_exit(bits);
	}
	say_status("child: setsid=pid | sid=pid<<1 | pgid=pid<<2 | again EPERM<<3 | setpgid(old) EPERM<<4", wait_for(pid, 0));
	say_ret("setpgid(nonexistent, 0)", sys2(SYS_setpgid, 0x3fffffff, 0));
	say_ret("setpgid(0, -1)", sys2(SYS_setpgid, 0, -1));

	/* An orphan is reparented to the nearest child subreaper: us. */
	say_ret("prctl(PR_SET_CHILD_SUBREAPER, 1)", sys2(SYS_prctl, 36, 1));
	sys2(SYS_pipe2, fds, 0);
	int back[2];
	sys2(SYS_pipe2, back, 0);
	pid = lx_fork();
	if (pid == 0) {
		i64 parent = sys0(SYS_getpid);
		i64 g = lx_fork();
		if (g == 0) {
			sys1(SYS_close, fds[1]);
			int bits = sys0(SYS_getppid) == parent;
			write_str(back[1], "r"); /* the middle child exits after this */
			sys3(SYS_read, fds[0], buf, 1); /* until EOF: the middle child is gone */
			i64 now = sys0(SYS_getppid);
			bits |= (now == me) << 1;
			sys3(SYS_write, back[1], &now, sizeof now);
			lx_exit(bits);
		}
		sys3(SYS_read, back[0], buf, 1);
		lx_exit(0);
	}
	say_status("middle child", wait_for(pid, 0));
	sys1(SYS_close, fds[0]);
	sys1(SYS_close, fds[1]);
	i64 newparent = 0;
	sys3(SYS_read, back[0], &newparent, sizeof newparent);
	say_bool("orphan's getppid() is now us", newparent == me);
	r = sys4(SYS_wait4, -1, &status, 0, 0);
	say_bool("we reap the orphan", r > 0);
	say_status("  orphan: parent was the middle child | now us<<1", status);
	sys1(SYS_close, back[0]);
	sys1(SYS_close, back[1]);

	say_text("-- execve");
	say_ret("execve(missing)", sys3(SYS_execve, "./missing", 0, 0));
	say_ret("execve(.)", sys3(SYS_execve, ".", 0, 0));
	i64 fd = sys4(SYS_openat, AT_FDCWD, "data", O_WRONLY | O_CREAT, 0644);
	write_str(fd, "just data\n");
	sys1(SYS_close, fd);
	say_ret("execve(not executable)", sys3(SYS_execve, "./data", 0, 0));
	sys2(SYS_chmod, "data", 0755);
	say_ret("execve(executable, not a program)", sys3(SYS_execve, "./data", 0, 0));
	say_ret("execve(f/x)", sys3(SYS_execve, "./data/x", 0, 0));
	say_ret("execve(bad pointer)", sys3(SYS_execve, 8, 0, 0));
	/* Descriptor 3 without close-on-exec, 4 with. */
	sys4(SYS_openat, AT_FDCWD, "data", O_RDONLY, 0);
	sys4(SYS_openat, AT_FDCWD, "data", O_RDONLY | O_CLOEXEC, 0);
	set_action(SIGUSR1, on_usr1, 0, 0);
	set_action(SIGUSR2, (void *)1, 0, 0);
	u64 term = 1UL << (SIGTERM - 1);
	sys4(SYS_rt_sigprocmask, SIG_BLOCK, &term, 0, 8);
	pid = lx_fork();
	if (pid == 0) {
		char *nargv[] = { argv[0], "exec-child", "", "with space", 0 };
		char *nenv[] = { "ONE=1", "EMPTY=", "LAST=the end", 0 };
		i64 e = sys3(SYS_execve, argv[0], nargv, nenv);
		say_ret("execve failed", e);
		lx_exit(100);
	}
	say_status("execve'd child", wait_for(pid, 0));
	pid = lx_fork();
	if (pid == 0) {
		char *nargv[] = { "renamed", "exec-child", 0 };
		char *nenv[] = { 0 };
		sys3(SYS_execve, argv[0], nargv, nenv);
		lx_exit(100);
	}
	say_status("execve'd child with argv[0] renamed and no environment", wait_for(pid, 0));
	sys4(SYS_rt_sigprocmask, SIG_UNBLOCK, &term, 0, 8);
	say_text("fork_test: ok");
	return 0;
}
