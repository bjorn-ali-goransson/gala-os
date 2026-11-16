/* procfs_test: the /proc files bubblewrap reads before it makes its
 * namespaces (ROADMAP LD: glycin, GTK 3's image loader on Ubuntu 26.04,
 * sandboxes its loaders with bubblewrap, and falls back to none when
 * bubblewrap reports that namespaces cannot be made; gala had no /proc):
 * /proc/sys/kernel/overflowuid and overflowgid, /proc/sys/user/
 * max_user_namespaces, the links in /proc/self/ns, and /proc/self itself, its exe
 * and auxv. (proc_test has the rest of /proc/self, for later.)
 *
 * What differs between machines is printed as relations: the namespace
 * links' form and that self's and the pid's agree, the limit as a
 * number, unshare(CLONE_NEWUSER) either working or refused (EPERM,
 * ENOSPC), the auxiliary vector the same as PR_GET_AUXV's. */
#include "../common/lx.h"
#include "../common/more.h"

#define O_RDONLY 0
#define O_DIRECTORY 0200000
#define O_PATH 010000000
#define S_IFMT 0170000
#define S_IFDIR 0040000
#define S_IFREG 0100000
#define S_IFLNK 0120000
#define PR_GET_AUXV 0x41555856
#define CLONE_NEWUSER 0x10000000

#if defined(__x86_64__)
struct st {
	u64 dev, ino, nlink;
	unsigned mode, uid, gid, pad;
	u64 rdev;
	i64 size, blksize, blocks;
	u64 times[6];
	i64 unused[3];
};
#else
/* The generic layout (aarch64's: musl's arch/aarch64/bits/stat.h). */
struct st {
	u64 dev, ino;
	unsigned mode, nlink, uid, gid;
	u64 rdev, pad;
	i64 size;
	int blksize, pad2;
	i64 blocks;
	u64 times[6];
	unsigned unused[2];
};
#endif

static unsigned mode_of(const char *path, int follow)
{
	struct st s;
	i64 r = sys4(SYS_newfstatat, -100, path, &s, follow ? 0 : 0x100 /* AT_SYMLINK_NOFOLLOW */);
	return r == 0 ? (s.mode & S_IFMT) : 0;
}

static i64 slurp(const char *path, char *buf, i64 size)
{
	i64 fd = sys2(SYS_open, path, O_RDONLY);
	if (fd < 0)
		return fd;
	i64 n = 0, r;
	while (n < size && (r = sys3(SYS_read, fd, buf + n, size - n)) > 0)
		n += r;
	sys1(SYS_close, fd);
	return n;
}

static i64 link_of(const char *path, char *buf)
{
	lx_memset(buf, 0, 128);
	return sys3(SYS_readlink, path, buf, 127);
}

static int decimal(const char *s, i64 n)
{
	if (n == 0)
		return 0;
	for (i64 i = 0; i < n; i++)
		if (s[i] < '0' || s[i] > '9')
			return 0;
	return 1;
}

/* Writes v in decimal at s, NUL-terminated. */
static void put_dec_to(char *s, i64 v)
{
	char t[24];
	int n = 0;
	do
		t[n++] = '0' + v % 10;
	while ((v /= 10) > 0);
	while (n)
		*s++ = t[--n];
	*s = 0;
}

int main(void)
{
	lx_init();
	char buf[4096], a[128], b[128];
	say_bool("/proc is a directory", mode_of("/proc", 1) == S_IFDIR);
	say_bool("/proc/self is a directory (followed)", mode_of("/proc/self", 1) == S_IFDIR);
	i64 n = link_of("/proc/self", a);
	i64 pid = sys0(SYS_getpid);
	i64 v = 0;
	for (i64 i = 0; i < n; i++)
		v = v * 10 + (a[i] - '0');
	say_bool("readlink(/proc/self) is getpid()", n > 0 && decimal(a, n) && v == pid);

	say_text("-- /proc/sys");
	n = slurp("/proc/sys/kernel/overflowuid", buf, sizeof buf);
	say_read("overflowuid", n, buf);
	n = slurp("/proc/sys/kernel/overflowgid", buf, sizeof buf);
	say_read("overflowgid", n, buf);
	say_bool("max_user_namespaces is a regular file", mode_of("/proc/sys/user/max_user_namespaces", 1) == S_IFREG);
	n = slurp("/proc/sys/user/max_user_namespaces", buf, sizeof buf);
	say_bool("  a number and a newline", n > 1 && buf[n - 1] == '\n' && decimal(buf, n - 1));

	say_text("-- /proc/self/ns");
	const char *ns[] = { "cgroup", "ipc", "mnt", "net", "pid", "user", "uts" };
	for (unsigned i = 0; i < sizeof ns / sizeof *ns; i++) {
		char path[160] = "/proc/self/ns/", other[160] = "/proc/";
		lx_memcpy(path + 14, ns[i], lx_strlen(ns[i]) + 1);
		put(ns[i]);
		put(": a link ");
		put(mode_of(path, 0) == S_IFLNK ? "yes" : "no");
		n = link_of(path, a);
		u64 len = lx_strlen(ns[i]);
		int form = n > (i64)len + 3 && lx_memeq(a, ns[i], len) && a[len] == ':' && a[len + 1] == '[' &&
			   a[n - 1] == ']' && decimal(a + len + 2, n - len - 3);
		put(", \"");
		put(ns[i]);
		put(":[N]\" ");
		put(form ? "yes" : "no");
		/* The same through /proc/<pid>. */
		i64 k = link_of("/proc/self", other + 6);
		lx_memcpy(other + 6 + k, "/ns/", 4);
		lx_memcpy(other + 10 + k, ns[i], len + 1);
		i64 m = link_of(other, b);
		put(", /proc/<pid>'s the same ");
		put(m == n && lx_memeq(a, b, n) ? "yes" : "no");
		end_line();
	}
	say_ret("readlink(/proc/self/ns/nothing)", link_of("/proc/self/ns/nothing", a));

	say_text("-- /proc/self/exe and auxv");
	n = link_of("/proc/self/exe", a);
	say_bool("readlink(/proc/self/exe) is absolute", n > 0 && a[0] == '/');
	u64 aux[64];
	i64 size = sys6(SYS_prctl, PR_GET_AUXV, (i64)aux, sizeof aux, 0, 0, 0);
	n = slurp("/proc/self/auxv", buf, sizeof buf);
	/* PR_GET_AUXV gives Linux's whole saved vector (zeros after AT_NULL),
	 * the file the pairs up to AT_NULL's. */
	say_bool("/proc/self/auxv is PR_GET_AUXV's vector up to AT_NULL",
		 n > 0 && n <= size && n % 16 == 0 && lx_memeq(buf, aux, n) && aux[n / 8 - 2] == 0);

	say_text("-- /proc/self/fd links (GLib names an open file by its link)");
	i64 dot = sys2(SYS_open, ".", O_RDONLY | O_DIRECTORY);
	char cwd[128], path[160] = "/proc/self/fd/", other[160] = "/proc/";
	i64 c = sys2(SYS_getcwd, cwd, sizeof cwd);
	put_dec_to(path + 14, dot);
	n = link_of(path, a);
	say_bool("a directory's: getcwd()", c > 1 && n == c - 1 && lx_memeq(a, cwd, n));
	i64 k = link_of("/proc/self", other + 6);
	lx_memcpy(other + 6 + k, "/fd/", 4);
	put_dec_to(other + 10 + k, dot);
	i64 m = link_of(other, b);
	say_bool("  through /proc/<pid>, the same", m == n && lx_memeq(a, b, n));
	int q[2];
	sys2(SYS_pipe2, q, 0);
	put_dec_to(path + 14, q[0]);
	n = link_of(path, a);
	say_bool("a pipe's: pipe:[N]", n > 7 && lx_memeq(a, "pipe:[", 6) && a[n - 1] == ']' && decimal(a + 6, n - 7));
	i64 e = sys2(SYS_eventfd2, 0, 0);
	put_dec_to(path + 14, e);
	n = link_of(path, a);
	say_read("an eventfd's", n, a);
	say_ret("readlink(/proc/self/fd/999) (not open)", link_of("/proc/self/fd/999", a));
	sys1(SYS_close, e);
	sys1(SYS_close, q[0]);
	sys1(SYS_close, q[1]);
	sys1(SYS_close, dot);

	say_text("-- /proc/mounts (GIO's mount monitor reads it)");
	n = link_of("/proc/mounts", a);
	say_read("readlink(/proc/mounts)", n, a);
	n = slurp("/proc/self/mounts", buf, sizeof buf - 1);
	buf[n > 0 ? n : 0] = 0;
	{
		int lines = 0, six = 1, root = 0, proc = 0;
		for (char *l = buf; *l; ) {
			char *e = l;
			while (*e && *e != '\n')
				e++;
			char *f[8];
			int k = 0;
			for (char *q = l; q < e && k < 8;) {
				while (q < e && *q == ' ')
					q++;
				if (q < e)
					f[k++] = q;
				while (q < e && *q != ' ')
					q++;
			}
			lines++;
			if (k != 6)
				six = 0;
			if (k >= 3 && f[1][0] == '/' && f[1][1] == ' ')
				root = 1;
			if (k >= 3 && lx_memeq(f[1], "/proc ", 6) && lx_memeq(f[2], "proc ", 5))
				proc = 1;
			l = *e ? e + 1 : e;
		}
		say_bool("/proc/self/mounts: lines", lines > 1);
		say_bool("  each of 6 fields", six);
		say_bool("  one mounted at /", root);
		say_bool("  /proc of type proc", proc);
	}
	i64 m1 = slurp("/proc/mounts", a, sizeof a);
	say_bool("/proc/mounts reads the same", m1 > 0 && lx_memeq(a, buf, m1 < 128 ? m1 : 128));

	say_text("-- opening /proc");
	i64 d = sys2(SYS_open, "/proc", O_PATH | O_DIRECTORY);
	say_bool("open(/proc, O_PATH|O_DIRECTORY)", d >= 0);
	struct st s;
	say_ret("fstatat(it, sys/kernel/overflowuid)", sys4(SYS_newfstatat, d, "sys/kernel/overflowuid", &s, 0));
	say_ret("open(/proc/0)", sys2(SYS_open, "/proc/0", O_RDONLY));
	say_ret("open(/proc/sys/kernel/nothing)", sys2(SYS_open, "/proc/sys/kernel/nothing", O_RDONLY));

	say_text("-- a user namespace");
	i64 r = sys1(SYS_unshare, CLONE_NEWUSER);
	say_bool("unshare(CLONE_NEWUSER): done, EPERM or ENOSPC", r == 0 || r == -1 || r == -28);
	say_text("procfs_test: ok");
	return 0;
}
