/* pipe_test: pipe2(2) and its flags, reads and writes at the edges (EOF,
 * EAGAIN, a full pipe, EPIPE and SIGPIPE), dup/dup2/dup3 and fcntl's
 * descriptor commands (F_GETFD/F_SETFD/F_GETFL/F_SETFL/F_DUPFD/
 * F_DUPFD_CLOEXEC, F_GETPIPE_SZ/F_SETPIPE_SZ).
 *
 * lx_init closes every inherited descriptor above 2 first, so descriptor
 * numbers (lowest free first) are deterministic and printed. */
#define _GNU_SOURCE
#include "../common/lx.h"
#include "../common/more.h"
#include <fcntl.h>
#include <signal.h>

static int fds[2];
static volatile int sigpipe_count;
static volatile int sigpipe_code = -99;

static void on_sigpipe(int sig, siginfo_t *info, void *uc)
{
	(void)sig;
	(void)uc;
	sigpipe_count++;
	sigpipe_code = info->si_code;
}

static void say_fds(const char *name, i64 r)
{
	put(name);
	put(" = ");
	put_ret(r);
	if (r == 0) {
		put(", fds ");
		put_dec(fds[0]);
		put(" ");
		put_dec(fds[1]);
	}
	end_line();
}

static void close_pipe(void)
{
	sys1(SYS_close, fds[0]);
	sys1(SYS_close, fds[1]);
}

int main(void)
{
	char buf[128];
	static char chunk[4096];
	lx_init();

	say_text("-- pipe2");
	say_fds("pipe2(0)", sys2(SYS_pipe2, fds, 0));
	say_ret("  F_GETFD read end", sys2(SYS_fcntl, fds[0], F_GETFD));
	say_ret("  F_GETFD write end", sys2(SYS_fcntl, fds[1], F_GETFD));
	say_ret("  F_GETFL read end", sys2(SYS_fcntl, fds[0], F_GETFL));
	say_ret("  F_GETFL write end", sys2(SYS_fcntl, fds[1], F_GETFL));
	say_ret("  F_GETPIPE_SZ", sys2(SYS_fcntl, fds[0], F_GETPIPE_SZ));
	say_ret("  write(abc)", write_str(fds[1], "abc"));
	say_read("  read(2)", sys3(SYS_read, fds[0], buf, 2), buf);
	say_read("  read(128)", sys3(SYS_read, fds[0], buf, sizeof buf), buf);
	say_ret("  read(read end, 0 bytes)", sys3(SYS_read, fds[0], buf, 0));
	say_ret("  write(read end)", write_str(fds[0], "x"));
	say_ret("  read(write end)", sys3(SYS_read, fds[1], buf, 1));
	say_ret("  lseek(read end)", sys3(SYS_lseek, fds[0], 0, 1));
	write_str(fds[1], "tail");
	sys1(SYS_close, fds[1]);
	say_read("  read after the writer closed", sys3(SYS_read, fds[0], buf, sizeof buf), buf);
	say_ret("  read at EOF", sys3(SYS_read, fds[0], buf, sizeof buf));
	say_ret("  read at EOF again", sys3(SYS_read, fds[0], buf, sizeof buf));
	sys1(SYS_close, fds[0]);

	say_fds("pipe2(O_CLOEXEC)", sys2(SYS_pipe2, fds, O_CLOEXEC));
	say_ret("  F_GETFD read end", sys2(SYS_fcntl, fds[0], F_GETFD));
	say_ret("  F_GETFD write end", sys2(SYS_fcntl, fds[1], F_GETFD));
	close_pipe();

	say_fds("pipe2(O_NONBLOCK)", sys2(SYS_pipe2, fds, O_NONBLOCK));
	say_bool("  F_GETFL has O_NONBLOCK", (sys2(SYS_fcntl, fds[0], F_GETFL) & O_NONBLOCK) != 0);
	say_ret("  read(empty)", sys3(SYS_read, fds[0], buf, sizeof buf));
	i64 total = 0, r;
	while ((r = sys3(SYS_write, fds[1], chunk, sizeof chunk)) > 0)
		total += r;
	say("  bytes written until full", total);
	say_ret("  write(full)", r);
	say_ret("  write(full, 1 byte)", sys3(SYS_write, fds[1], "x", 1));
	say_ret("  read(100)", sys3(SYS_read, fds[0], chunk, 100));
	say_ret("  write(100) after reading 100 (less than a page free)", sys3(SYS_write, fds[1], chunk, 100));
	say_ret("  read(4096)", sys3(SYS_read, fds[0], chunk, sizeof chunk));
	say_ret("  write(100) with a page free", sys3(SYS_write, fds[1], chunk, 100));
	close_pipe();

	say_ret("pipe2(O_APPEND)", sys2(SYS_pipe2, fds, O_APPEND));
	say_ret("pipe2(bad pointer)", sys2(SYS_pipe2, 8, 0));
	say_fds("pipe (the old call)", sys1(SYS_pipe, fds));
	say_ret("  F_SETPIPE_SZ(4096)", sys3(SYS_fcntl, fds[1], F_SETPIPE_SZ, 4096));
	say_ret("  F_GETPIPE_SZ", sys2(SYS_fcntl, fds[0], F_GETPIPE_SZ));
	say_ret("  F_SETPIPE_SZ(5000) (rounded up)", sys3(SYS_fcntl, fds[1], F_SETPIPE_SZ, 5000));
	say_ret("  F_SETPIPE_SZ(0) (at least a page)", sys3(SYS_fcntl, fds[1], F_SETPIPE_SZ, 0));
	close_pipe();

	say_text("-- EPIPE and SIGPIPE");
	u64 mask = 1UL << (SIGPIPE - 1), old = 0, pending = 0;
	sys2(SYS_pipe2, fds, 0);
	sys1(SYS_close, fds[0]);
	sys4(SYS_rt_sigprocmask, SIG_BLOCK, &mask, &old, 8);
	say_ret("write(no reader) with SIGPIPE blocked", write_str(fds[1], "x"));
	sys2(SYS_rt_sigpending, &pending, 8);
	say_bool("  SIGPIPE pending", (pending & mask) != 0);
	siginfo_t si;
	struct lx_ts zero = { 0, 0 };
	say_ret("  rt_sigtimedwait takes it", sys4(SYS_rt_sigtimedwait, &mask, &si, &zero, 8));
	say_ret("  si_code", si.si_code);
	sys4(SYS_rt_sigprocmask, SIG_SETMASK, &old, 0, 8);
	set_action(SIGPIPE, on_sigpipe, SA_SIGINFO, 0);
	say_ret("write(no reader) with a handler", write_str(fds[1], "x"));
	say("  handler calls", sigpipe_count);
	say_ret("  handler's si_code", sigpipe_code);
	set_action(SIGPIPE, (void *)1, 0, 0);
	say_ret("write(no reader) with SIGPIPE ignored", write_str(fds[1], "x"));
	say("  handler calls", sigpipe_count);
	set_action(SIGPIPE, 0, 0, 0);
	i64 pid = lx_fork();
	if (pid == 0) {
		write_str(fds[1], "x");
		lx_exit(0);
	}
	say_status("child writing with no reader, SIGPIPE default", wait_for(pid, 0));
	sys1(SYS_close, fds[1]);

	sys2(SYS_pipe2, fds, 0);
	pid = lx_fork();
	if (pid == 0) {
		sys1(SYS_close, fds[0]);
		write_str(fds[1], "from the child");
		lx_exit(0);
	}
	sys1(SYS_close, fds[1]);
	say_status("child writer", wait_for(pid, 0));
	say_read("read from the child", sys3(SYS_read, fds[0], buf, sizeof buf), buf);
	say_ret("read at EOF (child gone)", sys3(SYS_read, fds[0], buf, sizeof buf));
	sys1(SYS_close, fds[0]);

	say_text("-- dup, dup2, dup3");
	sys2(SYS_pipe2, fds, 0); /* 3 (read) and 4 (write) */
	say_ret("dup(3)", sys1(SYS_dup, 3));
	say_ret("close(5)", sys1(SYS_close, 5));
	say_ret("dup(4) takes the lowest free", sys1(SYS_dup, 4));
	say_ret("dup(999)", sys1(SYS_dup, 999));
	say_ret("dup2(4, 4)", sys2(SYS_dup2, 4, 4));
	say_ret("dup2(999, 999)", sys2(SYS_dup2, 999, 999));
	say_ret("dup2(5, 100)", sys2(SYS_dup2, 5, 100));
	say_ret("  F_GETFD", sys2(SYS_fcntl, 100, F_GETFD));
	say_ret("dup2(999, 101)", sys2(SYS_dup2, 999, 101));
	say_ret("dup2(5, -1)", sys2(SYS_dup2, 5, -1));
	say_ret("dup3(5, 5, 0)", sys3(SYS_dup3, 5, 5, 0));
	say_ret("dup3(5, 101, O_CLOEXEC)", sys3(SYS_dup3, 5, 101, O_CLOEXEC));
	say_ret("  F_GETFD", sys2(SYS_fcntl, 101, F_GETFD));
	say_ret("dup3(5, 102, O_NONBLOCK)", sys3(SYS_dup3, 5, 102, O_NONBLOCK));
	say_ret("dup2(5, 101) over a cloexec fd", sys2(SYS_dup2, 5, 101));
	say_ret("  F_GETFD (cleared)", sys2(SYS_fcntl, 101, F_GETFD));
	say_ret("write(100, via)", write_str(100, "via"));
	say_ret("write(101, \" dup\")", write_str(101, " dup"));
	say_read("read(3)", sys3(SYS_read, 3, buf, sizeof buf), buf);
	say_ret("F_SETFL(100, O_NONBLOCK)", sys3(SYS_fcntl, 100, F_SETFL, O_NONBLOCK));
	say_bool("  shared by its dups: F_GETFL(4) has O_NONBLOCK", (sys2(SYS_fcntl, 4, F_GETFL) & O_NONBLOCK) != 0);
	say_ret("dup(3)", sys1(SYS_dup, 3));
	say_ret("F_SETFL(6, O_NONBLOCK)", sys3(SYS_fcntl, 6, F_SETFL, O_NONBLOCK));
	say_ret("  read(3, empty)", sys3(SYS_read, 3, buf, sizeof buf));
	say_ret("F_SETFL(6, O_RDWR) (the access mode is ignored)", sys3(SYS_fcntl, 6, F_SETFL, O_RDWR));
	say_ret("  F_GETFL(6)", sys2(SYS_fcntl, 6, F_GETFL));
	say_ret("close(100)", sys1(SYS_close, 100));
	say_ret("close(101)", sys1(SYS_close, 101));
	sys1(SYS_close, 4);
	sys1(SYS_close, 5);
	say_ret("read(3) with every write end closed", sys3(SYS_read, 3, buf, sizeof buf));
	sys1(SYS_close, 3);
	sys1(SYS_close, 6);

	say_text("-- fcntl");
	sys2(SYS_pipe2, fds, 0); /* 3 and 4 */
	say_ret("F_DUPFD(3, 10)", sys3(SYS_fcntl, 3, F_DUPFD, 10));
	say_ret("  F_GETFD", sys2(SYS_fcntl, 10, F_GETFD));
	say_ret("F_DUPFD(3, 10) again", sys3(SYS_fcntl, 3, F_DUPFD, 10));
	say_ret("F_DUPFD_CLOEXEC(3, 10)", sys3(SYS_fcntl, 3, F_DUPFD_CLOEXEC, 10));
	say_ret("  F_GETFD", sys2(SYS_fcntl, 12, F_GETFD));
	say_ret("F_DUPFD(3, 0)", sys3(SYS_fcntl, 3, F_DUPFD, 0));
	say_ret("F_DUPFD(3, -1)", sys3(SYS_fcntl, 3, F_DUPFD, -1));
	say_ret("F_SETFD(3, FD_CLOEXEC)", sys3(SYS_fcntl, 3, F_SETFD, FD_CLOEXEC));
	say_ret("  F_GETFD", sys2(SYS_fcntl, 3, F_GETFD));
	say_ret("F_SETFD(3, 0xff) (only FD_CLOEXEC kept)", sys3(SYS_fcntl, 3, F_SETFD, 0xff));
	say_ret("  F_GETFD", sys2(SYS_fcntl, 3, F_GETFD));
	say_ret("F_SETFD(3, 0)", sys3(SYS_fcntl, 3, F_SETFD, 0));
	say_ret("  F_GETFD", sys2(SYS_fcntl, 3, F_GETFD));
	say_ret("F_GETFD(999)", sys2(SYS_fcntl, 999, F_GETFD));
	say_ret("fcntl(3, unknown command 9999)", sys2(SYS_fcntl, 3, 9999));
	say_text("pipe_test: ok");
	return 0;
}
