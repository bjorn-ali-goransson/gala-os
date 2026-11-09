/* id_test: who a Linux program is (ROADMAP LX "Linux identity"): the
 * uid and gid it runs as, the user database describing them, and those
 * ids where the kernel reports them: getuid/geteuid/getresuid and the gid
 * calls agree; /etc/passwd has an entry for getuid() whose gid is
 * getgid() and whose home is a directory; /etc/group has an entry for
 * getgid() and for every supplementary group; a file the program makes is
 * owned by it; a socket pair's SO_PEERCRED and a signal's si_uid carry its
 * ids; /etc/passwd and /etc/group are root's 0644 files; becoming root is
 * refused (EPERM) while becoming itself is not.
 *
 * Prints only relations, never ids or names (they differ between the
 * user recording it on Linux and gala's boot user), and nothing a root
 * caller would answer differently is asked unless it says so: run it as
 * an ordinary user. */
#define _GNU_SOURCE
#include "../common/lx.h"
#include "../common/more.h"
#include <fcntl.h>
#include <signal.h>
#include <sys/socket.h>
#include <sys/stat.h>
#include <sys/syscall.h>

static char buf[65536];

/* Reads all of `path` into buf (NUL-terminated); its length or -errno. */
static i64 slurp(const char *path)
{
	i64 fd = sys4(SYS_openat, AT_FDCWD, path, O_RDONLY, 0);
	if (fd < 0)
		return fd;
	i64 n = 0;
	for (;;) {
		i64 r = sys3(SYS_read, fd, buf + n, sizeof buf - 1 - n);
		if (r <= 0)
			break;
		n += r;
	}
	sys1(SYS_close, fd);
	buf[n] = 0;
	return n;
}

static u64 to_u64(const char *s, const char *end)
{
	u64 v = 0;
	for (; s < end && *s >= '0' && *s <= '9'; s++)
		v = v * 10 + (u64)(*s - '0');
	return v;
}

/* Field `k` (0-based, ':'-separated) of the line at `line`: its start;
 * `*end` its end. NULL if the line has fewer fields. */
static const char *field(const char *line, int k, const char **end)
{
	const char *p = line;
	for (int i = 0; i < k; i++) {
		while (*p && *p != ':' && *p != '\n')
			p++;
		if (*p != ':')
			return 0;
		p++;
	}
	const char *e = p;
	while (*e && *e != ':' && *e != '\n')
		e++;
	*end = e;
	return p;
}

/* The line of buf whose field `k` is the number `id`, or NULL. */
static const char *line_with(int k, u64 id)
{
	for (const char *line = buf; *line;) {
		const char *end;
		const char *f = field(line, k, &end);
		if (f && end > f && to_u64(f, end) == id && (end - f) > 0)
			return line;
		while (*line && *line != '\n')
			line++;
		if (*line)
			line++;
	}
	return 0;
}

static volatile int got_uid_ok = -1;

static void on_usr1(int sig, siginfo_t *info, void *ctx)
{
	(void)sig;
	(void)ctx;
	got_uid_ok = info->si_uid == (unsigned)sys0(SYS_getuid);
}

int main(void)
{
	struct stat st;
	lx_init();
	u64 uid = (u64)sys0(SYS_getuid), gid = (u64)sys0(SYS_getgid);

	say_text("-- the ids");
	say_bool("geteuid() is getuid()", (u64)sys0(SYS_geteuid) == uid);
	say_bool("getegid() is getgid()", (u64)sys0(SYS_getegid) == gid);
	unsigned r[3] = {9, 9, 9};
	say_ret("getresuid", sys3(SYS_getresuid, &r[0], &r[1], &r[2]));
	say_bool("real, effective and saved uid are getuid()",
		 r[0] == uid && r[1] == uid && r[2] == uid);
	say_ret("getresgid", sys3(SYS_getresgid, &r[0], &r[1], &r[2]));
	say_bool("real, effective and saved gid are getgid()",
		 r[0] == gid && r[1] == gid && r[2] == gid);
	say_bool("not root", uid != 0);

	say_text("-- the user database");
	i64 n = slurp("/etc/passwd");
	say_bool("/etc/passwd is readable", n > 0);
	const char *me = n > 0 ? line_with(2, uid) : 0;
	say_bool("/etc/passwd has getuid()", me != 0);
	if (me) {
		const char *end;
		const char *name = field(me, 0, &end);
		say_bool("its name is not empty", name && end > name);
		const char *g = field(me, 3, &end);
		say_bool("its gid is getgid()", g && to_u64(g, end) == gid);
		const char *home = field(me, 5, &end);
		char path[512] = {0};
		if (home && end > home && (u64)(end - home) < sizeof path)
			lx_memcpy(path, home, (u64)(end - home));
		i64 sr = sys4(SYS_newfstatat, AT_FDCWD, path, &st, 0);
		say_bool("its home is a directory", sr == 0 && S_ISDIR(st.st_mode));
		const char *shell = field(me, 6, &end);
		say_bool("its shell is named", shell && end > shell && *shell == '/');
	}
	say_bool("/etc/passwd has root as 0", n > 0 && line_with(2, 0) != 0);
	n = slurp("/etc/group");
	say_bool("/etc/group is readable", n > 0);
	say_bool("/etc/group has getgid()", n > 0 && line_with(2, gid) != 0);
	unsigned groups[64];
	i64 ng = sys2(SYS_getgroups, 64, groups);
	say_bool("getgroups succeeds", ng >= 0);
	int all = 1;
	for (i64 i = 0; i < ng; i++)
		all &= line_with(2, groups[i]) != 0;
	say_bool("/etc/group has every supplementary group", all);
	for (const char *p = "/etc/passwd", *q = "/etc/group"; p; p = q, q = 0) {
		i64 sr = sys4(SYS_newfstatat, AT_FDCWD, p, &st, 0);
		put(p);
		put(" is root's regular file, mode");
		say_oct("", sr == 0 && S_ISREG(st.st_mode) && st.st_uid == 0 ? st.st_mode & 07777
										: 0);
	}

	say_text("-- where the kernel reports them");
	sys2(SYS_mkdir, "w", 0777);
	i64 fd = sys4(SYS_openat, AT_FDCWD, "w/mine", O_WRONLY | O_CREAT | O_TRUNC, 0644);
	sys2(SYS_fstat, fd, &st);
	say_bool("a new file's owner is getuid()", st.st_uid == uid);
	say_bool("a new file's group is getgid()", st.st_gid == gid);
	sys1(SYS_close, fd);
	sys4(SYS_newfstatat, AT_FDCWD, "w", &st, 0);
	say_bool("a new directory's owner is getuid()", st.st_uid == uid);
	int sv[2];
	say_ret("socketpair", sys4(SYS_socketpair, AF_UNIX, SOCK_STREAM, 0, sv));
	struct {
		int pid;
		unsigned uid, gid;
	} cred = {0, 9, 9};
	unsigned len = sizeof cred;
	say_ret("SO_PEERCRED", sys5(SYS_getsockopt, sv[0], SOL_SOCKET, SO_PEERCRED, &cred, &len));
	say_bool("its uid and gid are getuid() and getgid()", cred.uid == uid && cred.gid == gid);
	struct {
		void *handler;
		u64 flags;
		void *restorer;
		u64 mask;
	} sa = {(void *)on_usr1, LX_SA_RESTORER | 4 /* SA_SIGINFO */, (void *)lx_restorer, 0};
	sys4(SYS_rt_sigaction, SIGUSR1, &sa, 0, 8);
	sys2(SYS_kill, sys0(SYS_getpid), SIGUSR1);
	say_bool("a signal from itself carries getuid()", got_uid_ok == 1);

	say_text("-- changing them");
	say_ret("setuid(getuid())", sys1(SYS_setuid, uid));
	say_ret("setgid(getgid())", sys1(SYS_setgid, gid));
	say_ret("setuid(0)", sys1(SYS_setuid, 0));
	say_ret("setgid(0)", sys1(SYS_setgid, 0));
	say_bool("still getuid()", (u64)sys0(SYS_getuid) == uid);
	say_text("id_test: ok");
	return 0;
}
