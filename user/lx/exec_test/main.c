/* exec_test: execve(2) as a busybox shell uses it (ROADMAP LX milestone
 * 2): its errors (ENOENT, EACCES, ENOEXEC, ENOTDIR, EFAULT), a new argv
 * and envp, what survives it (fds without close-on-exec, ignored
 * signals, the signal mask, the current directory) and what does not
 * (close-on-exec fds, caught signals), a `#!` script run by its
 * interpreter; and the process calls around it: getppid, uname, setuid
 * and setgid to the ids it has.
 *
 * The program path is argv[0] (lx-expect runs `./exec_test`; gala's
 * _pie scenario `./exec_test-pie`): it is printed only as a relation,
 * so both give the same transcript. Children report through their exit
 * status; the parent waits for each before printing on. Run as
 * `<path> exec-child ...` (or by the script as `<path> interp-arg
 * ./script ...`) it is the execve'd image. */
#define _GNU_SOURCE
#include "../common/lx.h"
#include "../common/more.h"
#include <fcntl.h>
#include <signal.h>

static const char *self;

static void say_argv(int argc, char **argv, int from)
{
	for (int i = from; i < argc; i++) {
		put("exec child: argv[");
		put_dec(i);
		put("] = ");
		put_quoted(argv[i], lx_strlen(argv[i]));
		end_line();
	}
}

static int exec_child(int argc, char **argv, char **envp)
{
	say("exec child: argc", argc);
	if (lx_streq(argv[0], "renamed"))
		say_argv(argc, argv, 0);
	else
		say_argv(argc, argv, 1);
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
	char buf[16];
	sys3(SYS_lseek, 3, 0, 0);
	say_read("exec child: read(3)", sys3(SYS_read, 3, buf, sizeof buf), buf);
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
	static char cwd[4096];
	i64 len = sys2(SYS_getcwd, cwd, sizeof cwd);
	say_bool("exec child: still in the directory exec.dir", len >= 10 && lx_memeq(cwd + len - 10, "/exec.dir", 10));
	return 3;
}

/* The script's interpreter: argv[0] is the interpreter's path, then its
 * argument, the script's path and the script's arguments. */
static int script_child(int argc, char **argv)
{
	say("script child: argc", argc);
	say_bool("script child: argv[0] is the #! line's interpreter (../ and the program)",
		 lx_memeq(argv[0], "../", 3));
	say_argv(argc, argv, 1);
	return 4;
}

static void on_usr1(int sig)
{
	(void)sig;
}

int main(int argc, char **argv, char **envp)
{
	self = argv[0];
	if (argc > 1 && lx_streq(argv[1], "exec-child"))
		return exec_child(argc, argv, envp);
	if (argc > 1 && lx_streq(argv[1], "interp-arg"))
		return script_child(argc, argv);
	lx_init();
	i64 me = sys0(SYS_getpid);

	say_text("-- process calls");
	i64 pid = lx_fork();
	if (pid == 0)
		lx_exit(sys0(SYS_getppid) == me);
	say_status("child: getppid() is the parent (1 = yes)", wait_for(pid, 0));
	static char uts[6][65];
	say_ret("uname", sys1(SYS_uname, uts));
	put("  sysname = ");
	put(uts[0]);
	end_line();
	put("  machine = ");
	put(uts[4]);
	end_line();
	say_ret("uname(bad pointer)", sys1(SYS_uname, 8));
	say_ret("setuid(getuid())", sys1(SYS_setuid, sys0(SYS_getuid)));
	say_ret("setgid(getgid())", sys1(SYS_setgid, sys0(SYS_getgid)));

	say_text("-- execve");
	/* The children's path: relative to the directory they run in. */
	static char path[4096];
	path[0] = '.';
	path[1] = '.';
	path[2] = '/';
	u64 k = 3, start = 0;
	if (self[0] == '.' && self[1] == '/')
		start = 2;
	for (u64 i = start; self[i] && k < sizeof path - 1; i++)
		path[k++] = self[i];
	path[k] = 0;
	say_ret("mkdir(exec.dir)", sys2(SYS_mkdir, "exec.dir", 0777));
	say_ret("chdir(exec.dir)", sys1(SYS_chdir, "exec.dir"));
	say_ret("execve(missing)", sys3(SYS_execve, "./missing", 0, 0));
	say_ret("execve(.)", sys3(SYS_execve, ".", 0, 0));
	i64 fd = sys4(SYS_openat, AT_FDCWD, "data", O_WRONLY | O_CREAT, 0644);
	write_str(fd, "just data\n");
	sys1(SYS_close, fd);
	say_ret("execve(not executable)", sys3(SYS_execve, "./data", 0, 0));
	say_ret("chmod(data, 0755)", sys2(SYS_chmod, "data", 0755));
	say_ret("execve(executable, not a program)", sys3(SYS_execve, "./data", 0, 0));
	say_ret("execve(data/x)", sys3(SYS_execve, "./data/x", 0, 0));
	say_ret("execve(bad pointer)", sys3(SYS_execve, 8, 0, 0));
	char *bad_argv[] = { "x", (char *)8, 0 };
	say_ret("execve(argv with a bad pointer)", sys3(SYS_execve, path, bad_argv, 0));
	say_ret("execve(bad argv)", sys3(SYS_execve, path, 8, 0));
	/* Descriptor 3 without close-on-exec, 4 with. */
	sys4(SYS_openat, AT_FDCWD, "data", O_RDONLY, 0);
	sys4(SYS_openat, AT_FDCWD, "data", O_RDONLY | O_CLOEXEC, 0);
	set_action(SIGUSR1, on_usr1, 0, 0);
	set_action(SIGUSR2, (void *)1, 0, 0);
	u64 term = 1UL << (SIGTERM - 1);
	sys4(SYS_rt_sigprocmask, SIG_BLOCK, &term, 0, 8);
	pid = lx_fork();
	if (pid == 0) {
		char *nargv[] = { path, "exec-child", "", "with space", 0 };
		char *nenv[] = { "ONE=1", "EMPTY=", "LAST=the end", 0 };
		i64 e = sys3(SYS_execve, path, nargv, nenv);
		say_ret("execve failed", e);
		lx_exit(100);
	}
	say_status("execve'd child", wait_for(pid, 0));
	pid = lx_fork();
	if (pid == 0) {
		char *nargv[] = { "renamed", "exec-child", 0 };
		char *nenv[] = { 0 };
		sys3(SYS_execve, path, nargv, nenv);
		lx_exit(100);
	}
	say_status("execve'd child with argv[0] renamed and no environment", wait_for(pid, 0));
	sys4(SYS_rt_sigprocmask, SIG_UNBLOCK, &term, 0, 8);

	say_text("-- scripts");
	fd = sys4(SYS_openat, AT_FDCWD, "script", O_WRONLY | O_CREAT, 0755);
	write_str(fd, "#!  ");
	write_str(fd, path);
	write_str(fd, " interp-arg  \nnot read\n");
	sys1(SYS_close, fd);
	pid = lx_fork();
	if (pid == 0) {
		char *nargv[] = { "./script", "a1", "", 0 };
		char *nenv[] = { 0 };
		i64 e = sys3(SYS_execve, "./script", nargv, nenv);
		say_ret("execve(script) failed", e);
		lx_exit(100);
	}
	say_status("execve'd script", wait_for(pid, 0));
	fd = sys4(SYS_openat, AT_FDCWD, "noexec-script", O_WRONLY | O_CREAT, 0644);
	write_str(fd, "#!/nonexistent\n");
	sys1(SYS_close, fd);
	say_ret("execve(script without x)", sys3(SYS_execve, "./noexec-script", 0, 0));
	sys2(SYS_chmod, "noexec-script", 0755);
	say_ret("execve(script, missing interpreter)", sys3(SYS_execve, "./noexec-script", 0, 0));
	say_text("exec_test: ok");
	return 0;
}
