/* proc_test: /proc/self basics as a program reads them: readlink of
 * /proc/self (the pid) and /proc/self/exe (the basename only: the path
 * varies), /proc/self/fd (a sorted listing, readlink of its entries),
 * /proc/self/stat's shape (pid, comm, state, ppid; the field count),
 * /proc/self/status's Name/Pid/PPid lines, /proc/self/maps holding the
 * binary's mappings and [stack] (shape only), /proc/self/cmdline and
 * /proc/self/environ (NUL-separated argv and envp), /proc/self/task.
 *
 * No pid, address or inode is printed: they are compared with what the
 * system calls return. If /proc is not mounted, one SKIP line is
 * printed. */
#define _GNU_SOURCE
#include "../common/lx.h"
#include "../common/more.h"
#include <fcntl.h>

static char buf[65536];

static i64 read_file(const char *path)
{
	i64 fd = sys4(SYS_openat, AT_FDCWD, path, O_RDONLY, 0);
	if (fd < 0)
		return fd;
	i64 n = 0, r;
	while ((r = sys3(SYS_read, fd, buf + n, sizeof buf - 1 - n)) > 0)
		n += r;
	sys1(SYS_close, fd);
	buf[n] = 0;
	return n;
}

static i64 parse_dec(const char *s, const char **end)
{
	i64 v = 0;
	int neg = *s == '-';
	if (neg)
		s++;
	while (*s >= '0' && *s <= '9')
		v = v * 10 + (*s++ - '0');
	if (end)
		*end = s;
	return neg ? -v : v;
}

static const char *base_name(const char *p, i64 n)
{
	const char *b = p;
	for (i64 i = 0; i < n; i++)
		if (p[i] == '/')
			b = p + i + 1;
	return b;
}

static int starts_with(const char *s, const char *pre)
{
	while (*pre)
		if (*s++ != *pre++)
			return 0;
	return 1;
}

/* Finds "name:\t" at a line start in buf; returns what follows. */
static const char *status_field(const char *name)
{
	const char *s = buf;
	while (*s) {
		if (starts_with(s, name) && s[lx_strlen(name)] == ':')
			return s + lx_strlen(name) + 1;
		while (*s && *s != '\n')
			s++;
		if (*s)
			s++;
	}
	return 0;
}

int main(int argc, char **argv, char **envp)
{
	char link[4096];
	lx_init();
	i64 pid = sys0(SYS_getpid), ppid = sys0(SYS_getppid);

	i64 n = sys3(SYS_readlink, "/proc/self", link, sizeof link - 1);
	if (n < 0) {
		say_ret("SKIP: readlink(/proc/self)", n);
		say_text("proc_test: ok");
		return 0;
	}
	link[n] = 0;
	say_text("-- /proc/self");
	say_bool("readlink(/proc/self) is getpid()", parse_dec(link, 0) == pid);
	n = sys3(SYS_readlink, "/proc/self/exe", link, sizeof link - 1);
	say_ok("readlink(/proc/self/exe)", n);
	if (n > 0) {
		link[n] = 0;
		say_bool("  absolute", link[0] == '/');
		const char *b = base_name(link, n);
		put("  basename = ");
		put(b);
		end_line();
	}
	n = sys3(SYS_readlink, "/proc/self/cwd", link, sizeof link - 1);
	say_bool("readlink(/proc/self/cwd) is getcwd()", n > 0 && sys2(SYS_getcwd, buf, sizeof buf) == n + 1 && lx_memeq(buf, link, n));

	say_text("-- /proc/self/fd");
	int p[2];
	sys2(SYS_pipe2, p, 0); /* 3, 4 */
	i64 f = sys4(SYS_openat, AT_FDCWD, "/", O_RDONLY | O_DIRECTORY, 0); /* 5 */
	i64 dfd = sys4(SYS_openat, AT_FDCWD, "/proc/self/fd", O_RDONLY | O_DIRECTORY, 0); /* 6 */
	static char names[64][16];
	char *v[64];
	int k = 0;
	for (;;) {
		i64 r = sys3(SYS_getdents64, dfd, buf, sizeof buf);
		if (r <= 0)
			break;
		for (i64 pos = 0; pos < r;) {
			unsigned short reclen = *(unsigned short *)(buf + pos + 16);
			const char *name = buf + pos + 19;
			if (k < 64 && lx_strlen(name) < 16) {
				for (int i = 0; (names[k][i] = name[i]); i++)
					;
				v[k] = names[k];
				k++;
			}
			pos += reclen;
		}
	}
	sys1(SYS_close, dfd);
	sort_strings(v, k);
	put("getdents64(/proc/self/fd), sorted:");
	for (int i = 0; i < k; i++) {
		put(" ");
		put(v[i]);
	}
	end_line();
	n = sys3(SYS_readlink, "/proc/self/fd/0", link, sizeof link);
	say_read("readlink(fd/0) (stdin: /dev/null)", n, link);
	n = sys3(SYS_readlink, "/proc/self/fd/3", link, sizeof link);
	say_bool("readlink(fd/3) starts with pipe:[", n > 6 && lx_memeq(link, "pipe:[", 6));
	n = sys3(SYS_readlink, "/proc/self/fd/5", link, sizeof link);
	say_read("readlink(fd/5)", n, link);
	say_ret("readlink(fd/9) (not open)", sys3(SYS_readlink, "/proc/self/fd/9", link, sizeof link));
	i64 again = sys4(SYS_openat, AT_FDCWD, "/proc/self/fd/4", O_WRONLY, 0);
	say_ok("open(/proc/self/fd/4) reopens the pipe's write end", again);
	write_str(again, "via proc");
	char rb[32];
	say_read("  read(pipe)", sys3(SYS_read, p[0], rb, sizeof rb), rb);
	sys1(SYS_close, again);
	sys1(SYS_close, f);

	say_text("-- /proc/self/stat");
	n = read_file("/proc/self/stat");
	say_bool("read", n > 0);
	const char *s = buf, *e;
	say_bool("field 1 is getpid()", parse_dec(s, &e) == pid);
	const char *lp = e + 1, *rp = 0;
	for (const char *q = buf; *q; q++)
		if (*q == ')')
			rp = q;
	put("comm = ");
	if (*lp == '(' && rp)
		put_quoted(lp + 1, rp - lp - 1);
	end_line();
	put("state = ");
	put_char(rp[2]);
	end_line();
	say_bool("field 4 is getppid()", parse_dec(rp + 4, 0) == ppid);
	int fields = 2;
	for (const char *q = rp + 1; *q; q++)
		if (*q == ' ')
			fields++;
	say_bool("at least 52 fields", fields >= 52);
	/* Field 5, the process group; 6, the session; 20, the thread count. */
	const char *q = rp + 2;
	i64 fv[60];
	int nf = 3;
	while (*q && nf < 60) {
		while (*q == ' ')
			q++;
		fv[nf++] = *q == '-' || (*q >= '0' && *q <= '9') ? parse_dec(q, &q) : (q++, 0);
		while (*q && *q != ' ')
			q++;
	}
	say_bool("field 5 is getpgid(0)", fv[5] == sys1(SYS_getpgid, 0));
	say_bool("field 6 is getsid(0)", fv[6] == sys1(SYS_getsid, 0));
	say("field 20 (threads)", fv[20]);

	say_text("-- /proc/self/status");
	read_file("/proc/self/status");
	const char *name = status_field("Name");
	put("Name = ");
	if (name) {
		while (*name == '\t' || *name == ' ')
			name++;
		const char *end = name;
		while (*end && *end != '\n')
			end++;
		put_quoted(name, end - name);
	}
	end_line();
	const char *ps = status_field("Pid");
	say_bool("Pid is getpid()", ps && parse_dec(ps + 1, 0) == pid);
	ps = status_field("PPid");
	say_bool("PPid is getppid()", ps && parse_dec(ps + 1, 0) == ppid);
	ps = status_field("Threads");
	say("Threads", ps ? parse_dec(ps + 1, 0) : -1);

	say_text("-- /proc/self/maps");
	n = read_file("/proc/self/maps");
	int lines = 0, stack = 0, bin_x = 0, bin_any = 0, shape = 1, vdso = 0;
	for (const char *l = buf; *l;) {
		const char *end = l;
		while (*end && *end != '\n')
			end++;
		lines++;
		/* start-end perms offset dev inode [path] */
		const char *c = l;
		int hex = 0;
		while ((*c >= '0' && *c <= '9') || (*c >= 'a' && *c <= 'f'))
			c++, hex++;
		if (*c != '-' || hex < 8)
			shape = 0;
		c++;
		while ((*c >= '0' && *c <= '9') || (*c >= 'a' && *c <= 'f'))
			c++;
		if (*c != ' ')
			shape = 0;
		c++;
		const char *perms = c;
		if (!((perms[0] == 'r' || perms[0] == '-') && (perms[1] == 'w' || perms[1] == '-') && (perms[2] == 'x' || perms[2] == '-') && (perms[3] == 'p' || perms[3] == 's')))
			shape = 0;
		const char *path = end;
		while (path > l && path[-1] != ' ')
			path--;
		i64 plen = end - path;
		if (plen == 7 && lx_memeq(path, "[stack]", 7))
			stack++;
		if (plen == 6 && lx_memeq(path, "[vdso]", 6))
			vdso++;
		const char *b = base_name(path, plen);
		if (end - b == 9 && lx_memeq(b, "proc_test", 9)) {
			bin_any++;
			if (perms[2] == 'x')
				bin_x++;
		}
		l = *end ? end + 1 : end;
	}
	say_bool("more than 3 lines", lines > 3);
	say_bool("every line: start-end perms ...", shape);
	say("[stack] lines", stack);
	say("[vdso] lines", vdso);
	say_bool("the binary is mapped", bin_any > 0);
	say("  executable mappings of it", bin_x);

	say_text("-- /proc/self/cmdline and environ");
	n = read_file("/proc/self/cmdline");
	int i = 0;
	int match = 1;
	for (const char *c = buf; c < buf + n; c += lx_strlen(c) + 1, i++) {
		put("cmdline[");
		put_dec(i);
		put("] = ");
		put_quoted(c, lx_strlen(c));
		end_line();
		if (i >= argc || !lx_streq(c, argv[i]))
			match = 0;
	}
	say_bool("cmdline is argv", match && i == argc);
	n = read_file("/proc/self/environ");
	i = 0;
	match = 1;
	for (const char *c = buf; c < buf + n; c += lx_strlen(c) + 1, i++)
		if (!envp[i] || !lx_streq(c, envp[i]))
			match = 0;
	say_bool("environ is envp", match && !envp[i]);
	say("  entries", i);

	say_text("-- /proc/self/task");
	dfd = sys4(SYS_openat, AT_FDCWD, "/proc/self/task", O_RDONLY | O_DIRECTORY, 0);
	int tasks = 0, tid_found = 0;
	i64 r = sys3(SYS_getdents64, dfd, buf, sizeof buf);
	for (i64 pos = 0; pos < r;) {
		unsigned short reclen = *(unsigned short *)(buf + pos + 16);
		const char *nm = buf + pos + 19;
		if (nm[0] != '.') {
			tasks++;
			if (parse_dec(nm, 0) == sys0(SYS_gettid))
				tid_found = 1;
		}
		pos += reclen;
	}
	sys1(SYS_close, dfd);
	say("task entries", tasks);
	say_bool("  one is gettid()", tid_found);
	say_ret("open(/proc/self/nonexistent)", sys4(SYS_openat, AT_FDCWD, "/proc/self/nonexistent", O_RDONLY, 0));
	say_text("proc_test: ok");
	return 0;
}
