/* exec_test: execve(2) as a busybox shell uses it (ROADMAP LX milestone
 * 2): its errors (ENOENT, EACCES, ENOEXEC, ENOTDIR, EFAULT), a new argv
 * and envp, what survives it (fds without close-on-exec, ignored
 * signals, the signal mask, the current directory) and what does not
 * (close-on-exec fds, caught signals), a `#!` script run by its
 * interpreter; and the process calls around it: getppid, uname, setuid
 * and setgid to the ids it has, the resource limits (prlimit64,
 * getrlimit, setrlimit), getrandom, readlink of /proc/self/exe (the
 * program run: glibc's ld.so reads it for $ORIGIN).
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
#include <sys/resource.h>

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
	/* /proc/self/exe is the program execve ran: in exec.dir's parent. */
	static char exe[4096];
	i64 n_exe = sys3(SYS_readlink, "/proc/self/exe", exe, sizeof exe);
	i64 slash = n_exe;
	while (slash > 0 && exe[slash - 1] != '/')
		slash--;
	say_bool("exec child: /proc/self/exe is in exec.dir's parent",
		 n_exe > 0 && len > 10 && slash == len - 9 && lx_memeq(exe, cwd, len - 10));
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

	say_text("-- resource limits");
	{
		u64 a[2] = { 0, 0 }, b[2] = { 1, 1 }, c[2] = { 2, 2 };
		say_ret("getrlimit(RLIMIT_STACK)", sys2(SYS_getrlimit, RLIMIT_STACK, a));
		say_ret("prlimit64(0, RLIMIT_STACK, NULL, old)", sys4(SYS_prlimit64, 0, RLIMIT_STACK, 0, b));
		say_ret("prlimit64(getpid(), RLIMIT_STACK, NULL, old)", sys4(SYS_prlimit64, me, RLIMIT_STACK, 0, c));
		say_bool("  the three agree", a[0] == b[0] && a[1] == b[1] && a[0] == c[0] && a[1] == c[1]);
		say_bool("  soft <= hard", a[0] <= a[1]);
		u64 nofile[2], low[2], back[2];
		sys2(SYS_getrlimit, RLIMIT_NOFILE, nofile);
		low[0] = 64;
		low[1] = nofile[1];
		say_ret("setrlimit(RLIMIT_NOFILE, 64)", sys2(SYS_setrlimit, RLIMIT_NOFILE, low));
		sys2(SYS_getrlimit, RLIMIT_NOFILE, back);
		say("  read back", back[0]);
		say_ret("setrlimit(RLIMIT_NOFILE, as it was)", sys2(SYS_setrlimit, RLIMIT_NOFILE, nofile));
		u64 bad[2] = { 10, 5 };
		say_ret("setrlimit(soft above hard)", sys2(SYS_setrlimit, RLIMIT_NOFILE, bad));
		u64 zero[2] = { 0, 0 }, raise[2] = { 0, 1 }, prev[2] = { 9, 9 };
		say_ret("prlimit64(RLIMIT_CORE, 0/0, old)", sys4(SYS_prlimit64, 0, RLIMIT_CORE, zero, prev));
		say_ret("setrlimit(RLIMIT_CORE, hard raised)", sys2(SYS_setrlimit, RLIMIT_CORE, raise));
		say_ret("getrlimit(resource 99)", sys2(SYS_getrlimit, 99, a));
		say_ret("prlimit64(0, resource 99)", sys4(SYS_prlimit64, 0, 99, 0, a));
		say_ret("getrlimit(bad pointer)", sys2(SYS_getrlimit, RLIMIT_STACK, 8));
	}

	say_text("-- getrandom");
	{
		unsigned char r1[32], r2[32];
		lx_memset(r1, 0, sizeof r1);
		lx_memset(r2, 0, sizeof r2);
		say_ret("getrandom(32)", sys3(SYS_getrandom, r1, sizeof r1, 0));
		say_ret("getrandom(32, GRND_NONBLOCK)", sys3(SYS_getrandom, r2, sizeof r2, 1));
		int zero = 1;
		for (unsigned i = 0; i < sizeof r1; i++)
			zero &= r1[i] == 0;
		say_bool("  not all zero, and the two differ", !zero && !lx_memeq(r1, r2, sizeof r1));
		say_ret("getrandom(0 bytes)", sys3(SYS_getrandom, r1, 0, 0));
		say_ret("getrandom(unknown flag)", sys3(SYS_getrandom, r1, 4, 0x100));
		say_ret("getrandom(GRND_RANDOM|GRND_INSECURE)", sys3(SYS_getrandom, r1, 4, 6));
		say_ret("getrandom(bad pointer)", sys3(SYS_getrandom, 8, 4, 0));
	}

	say_text("-- /proc/self/exe");
	{
		static char exe[4096], cwd[4096];
		i64 n = sys3(SYS_readlink, "/proc/self/exe", exe, sizeof exe);
		i64 len = sys2(SYS_getcwd, cwd, sizeof cwd);
		i64 slash = n;
		while (slash > 0 && exe[slash - 1] != '/')
			slash--;
		say_bool("readlink(/proc/self/exe) is a path in the current directory",
			 n > 0 && exe[0] == '/' && slash == len && lx_memeq(exe, cwd, len - 1));
		char one = 0;
		say_ret("readlink(/proc/self/exe, 1 byte)", sys3(SYS_readlink, "/proc/self/exe", &one, 1));
		say_bool("  its first byte is /", one == '/');
		say_ret("readlinkat(AT_FDCWD, /proc/self/exe)", sys4(SYS_readlinkat, AT_FDCWD, "/proc/self/exe", exe, sizeof exe) == n ? 0 : -1);
		say_ret("readlink(/proc/self/exe, size 0)", sys3(SYS_readlink, "/proc/self/exe", exe, 0));
		say_ret("readlink(/proc/self/no_such_entry)", sys3(SYS_readlink, "/proc/self/no_such_entry", exe, sizeof exe));
	}

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
