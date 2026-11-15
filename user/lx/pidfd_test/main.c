/* pidfd_test: pidfd_open(2), pidfd_send_signal(2) and waitid(2) (ROADMAP
 * LD: GLib 2.88 on Ubuntu 26.04 watches each child with a pidfd it polls,
 * then reaps it with waitid(P_PIDFD); every GIMP plug-in is such a child;
 * ENOSYS in gala, where GLib fell back to SIGCHLD).
 *
 * Only what is the same on any machine is printed: no pids, the siginfo's
 * fields as relations. */
#include "../common/lx.h"
#include "../common/more.h"

#define P_ALL 0
#define P_PID 1
#define P_PIDFD 3
#define WNOHANG 1
#define WEXITED 4
#define WNOWAIT 0x01000000
#define PIDFD_NONBLOCK 04000
#define POLLIN 1
#define F_GETFD 1
#define F_GETFL 3
#define SIGTERM 15
#define CLD_EXITED 1
#define CLD_KILLED 2

struct pfd {
	int fd;
	short events, revents;
};

/* The fields of siginfo_t a waitid fills (si_signo, si_errno, si_code,
 * then _sigchld: si_pid, si_uid, si_status). */
struct info {
	int signo, err, code, pad;
	int pid;
	unsigned uid;
	int status;
	int rest[25];
};

static int readable(int fd)
{
	struct pfd p = { fd, POLLIN, 0 };
	i64 r = sys3(SYS_poll, &p, 1, 0);
	return r == 1 && (p.revents & POLLIN);
}

static i64 child_exiting(int code, int ms)
{
	i64 pid = lx_fork();
	if (pid == 0) {
		sleep_ms(ms);
		lx_exit(code);
	}
	return pid;
}

static void say_info(const char *name, i64 r, struct info *in, i64 pid)
{
	put(name);
	put(" = ");
	put_ret(r);
	if (r == 0) {
		put(": signo ");
		put_dec(in->signo);
		put(", code ");
		put_dec(in->code);
		put(", status ");
		put_dec(in->status);
		put(", the child's pid ");
		put(in->pid == pid ? "yes" : "no");
		put(", our uid ");
		put(in->uid == (unsigned)sys0(SYS_getuid) ? "yes" : "no");
	}
	end_line();
}

int main(void)
{
	lx_init();
	struct info in;
	char link[128];

	say_text("-- pidfd_open");
	say_ret("pidfd_open(no such pid)", sys2(SYS_pidfd_open, 999999, 0));
	say_ret("pidfd_open(self, flags 1)", sys2(SYS_pidfd_open, sys0(SYS_getpid), 1));
	say_ret("pidfd_open(0)", sys2(SYS_pidfd_open, 0, 0));
	i64 pid = child_exiting(7, 100);
	i64 fd = sys2(SYS_pidfd_open, pid, 0);
	say_bool("pidfd_open(a child)", fd >= 0);
	say("  close-on-exec", sys2(SYS_fcntl, fd, F_GETFD));
	say_bool("  O_NONBLOCK", (sys2(SYS_fcntl, fd, F_GETFL) & 04000) != 0);
	char path[64] = "/proc/self/fd/";
	int k = 14;
	char d[8];
	int n = 0;
	for (i64 v = fd; v > 0 || n == 0; v /= 10)
		d[n++] = '0' + v % 10;
	while (n)
		path[k++] = d[--n];
	path[k] = 0;
	lx_memset(link, 0, sizeof link);
	say_read("  its link", sys3(SYS_readlink, path, link, sizeof link - 1), link);
	say_bool("  readable while it runs", readable(fd));
	lx_memset(&in, 0x55, sizeof in);
	in.pid = 0;
	i64 r = sys5(SYS_waitid, P_PIDFD, fd, &in, WEXITED | WNOHANG, 0);
	say("waitid(P_PIDFD, WNOHANG) while it runs", r);
	say("  si_pid", in.pid);
	sleep_ms(400);
	say_bool("readable once it has exited", readable(fd));
	lx_memset(&in, 0, sizeof in);
	say_info("waitid(P_PIDFD, WEXITED|WNOWAIT)", sys5(SYS_waitid, P_PIDFD, fd, &in, WEXITED | WNOWAIT, 0), &in, pid);
	lx_memset(&in, 0, sizeof in);
	say_info("waitid(P_PIDFD, WEXITED)", sys5(SYS_waitid, P_PIDFD, fd, &in, WEXITED, 0), &in, pid);
	say_bool("  still readable, reaped", readable(fd));
	say_ret("waitid(P_PIDFD) again", sys5(SYS_waitid, P_PIDFD, fd, &in, WEXITED, 0));
	sys1(SYS_close, fd);

	say_text("-- pidfd_send_signal");
	pid = child_exiting(0, 5000);
	fd = sys2(SYS_pidfd_open, pid, PIDFD_NONBLOCK);
	say_bool("pidfd_open(PIDFD_NONBLOCK)", fd >= 0);
	say_bool("  O_NONBLOCK", (sys2(SYS_fcntl, fd, F_GETFL) & 04000) != 0);
	say_ret("waitid(P_PIDFD) on it while it runs", sys5(SYS_waitid, P_PIDFD, fd, &in, WEXITED, 0));
	say_ret("pidfd_send_signal(flags 1)", sys4(SYS_pidfd_send_signal, fd, SIGTERM, 0, 1));
	say_ret("pidfd_send_signal(SIGTERM)", sys4(SYS_pidfd_send_signal, fd, SIGTERM, 0, 0));
	lx_memset(&in, 0, sizeof in);
	say_info("waitid(P_PID, WEXITED)", sys5(SYS_waitid, P_PID, pid, &in, WEXITED, 0), &in, pid);
	say_ret("pidfd_send_signal to a reaped one", sys4(SYS_pidfd_send_signal, fd, SIGTERM, 0, 0));
	sys1(SYS_close, fd);
	say_ret("pidfd_send_signal(a bad fd)", sys4(SYS_pidfd_send_signal, 99, SIGTERM, 0, 0));
	int pp[2];
	sys2(SYS_pipe2, pp, 0);
	say_ret("pidfd_send_signal(a pipe)", sys4(SYS_pidfd_send_signal, pp[0], SIGTERM, 0, 0));
	say_ret("waitid(P_PIDFD, a pipe)", sys5(SYS_waitid, P_PIDFD, pp[0], &in, WEXITED, 0));

	say_text("-- waitid");
	say_ret("waitid(P_ALL, no children)", sys5(SYS_waitid, P_ALL, 0, &in, WEXITED, 0));
	say_ret("waitid(no WEXITED)", sys5(SYS_waitid, P_ALL, 0, &in, 0, 0));
	say_ret("waitid(idtype 9)", sys5(SYS_waitid, 9, 0, &in, WEXITED, 0));
	pid = child_exiting(3, 0);
	lx_memset(&in, 0, sizeof in);
	say_info("waitid(P_ALL, WEXITED)", sys5(SYS_waitid, P_ALL, 0, &in, WEXITED, 0), &in, pid);
	say_text("pidfd_test: ok");
	return 0;
}
