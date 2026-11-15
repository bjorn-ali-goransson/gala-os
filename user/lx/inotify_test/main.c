/* inotify_test: inotify(7) (ROADMAP LD: GIO's file monitors use it, and
 * fall back to polling every few seconds without it: GIMP's and GTK's
 * file choosers; ENOSYS in gala).
 *
 * In a directory of its own under the current one: a watch on the
 * directory sees entries made, written, renamed and removed; a watch on a
 * file sees it written; rm_watch gives IN_IGNORED. Only what is the same
 * on any machine is printed: wds as "the first" / "the second", cookies
 * as relations. */
#include "../common/lx.h"
#include "../common/more.h"

#define IN_NONBLOCK 04000
#define IN_CLOEXEC 02000000
#define IN_MODIFY 0x2
#define IN_ATTRIB 0x4
#define IN_CLOSE_WRITE 0x8
#define IN_MOVED_FROM 0x40
#define IN_MOVED_TO 0x80
#define IN_CREATE 0x100
#define IN_DELETE 0x200
#define IN_IGNORED 0x8000
#define IN_ISDIR 0x40000000
#define IN_ONLYDIR 0x01000000
#define IN_MASK_ADD 0x20000000
#define O_RDONLY 0
#define O_WRONLY 1
#define O_CREAT 0100
#define O_TRUNC 01000
#define F_GETFD 1
#define F_GETFL 3
#define FIONREAD 0x541b
#define POLLIN 1

struct pfd {
	int fd;
	short events, revents;
};

static int wd_dir, wd_file;
static unsigned last_cookie;

static void put_mask(unsigned m)
{
	static const struct {
		unsigned bit;
		const char *name;
	} names[] = { { IN_MODIFY, "MODIFY" },	   { IN_ATTRIB, "ATTRIB" },	{ IN_CLOSE_WRITE, "CLOSE_WRITE" },
		      { IN_MOVED_FROM, "MOVED_FROM" }, { IN_MOVED_TO, "MOVED_TO" }, { IN_CREATE, "CREATE" },
		      { IN_DELETE, "DELETE" },	   { IN_IGNORED, "IGNORED" },	{ IN_ISDIR, "ISDIR" } };
	int first = 1;
	for (unsigned i = 0; i < sizeof names / sizeof *names; i++) {
		if (m & names[i].bit) {
			put(first ? "" : "|");
			put(names[i].name);
			first = 0;
			m &= ~names[i].bit;
		}
	}
	if (m) {
		put(first ? "" : "|");
		put_hex(m);
	}
}

/* Reads what is queued (non-blocking fd) and prints each event: the wd,
 * the mask, the name and its padded length, the cookie as a relation. */
static void drain(int fd, const char *label)
{
	static char buf[4096] __attribute__((aligned(8)));
	put(label);
	put(":");
	i64 n = sys3(SYS_read, fd, buf, sizeof buf);
	if (n < 0) {
		put(" ");
		put_ret(n);
		end_line();
		return;
	}
	end_line();
	for (i64 i = 0; i < n;) {
		int wd = *(int *)(buf + i);
		unsigned mask = *(unsigned *)(buf + i + 4);
		unsigned cookie = *(unsigned *)(buf + i + 8);
		unsigned len = *(unsigned *)(buf + i + 12);
		put("  ");
		put(wd == wd_dir ? "dir" : wd == wd_file ? "file" : "?");
		put(" ");
		put_mask(mask);
		if (len) {
			put(" \"");
			put(buf + i + 16);
			put("\" len ");
			put_dec(len);
		}
		if (mask & IN_MOVED_FROM) {
			put(cookie ? ", a cookie" : ", no cookie");
			last_cookie = cookie;
		} else if (mask & IN_MOVED_TO) {
			put(cookie == last_cookie ? ", the same cookie" : ", another cookie");
		} else if (cookie) {
			put(", a cookie");
		}
		end_line();
		i += 16 + len;
	}
}

static void write_file(const char *path, const char *text)
{
	i64 fd = sys3(SYS_open, path, O_WRONLY | O_CREAT | O_TRUNC, 0644);
	sys3(SYS_write, fd, text, lx_strlen(text));
	sys1(SYS_close, fd);
}

int main(void)
{
	lx_init();
	sys2(SYS_mkdir, "inotify-dir", 0755);
	sys1(SYS_chdir, "inotify-dir");

	say_text("-- inotify_init1");
	say_ret("inotify_init1(flags 1)", sys1(SYS_inotify_init1, 1));
	i64 fd = sys1(SYS_inotify_init1, IN_NONBLOCK | IN_CLOEXEC);
	say_bool("inotify_init1(IN_NONBLOCK|IN_CLOEXEC)", fd >= 0);
	say("  close-on-exec", sys2(SYS_fcntl, fd, F_GETFD));
	say_bool("  O_NONBLOCK", (sys2(SYS_fcntl, fd, F_GETFL) & 04000) != 0);
	i64 plain = sys0(SYS_inotify_init);
	say_bool("inotify_init()", plain >= 0);
	say("  close-on-exec", sys2(SYS_fcntl, plain, F_GETFD));
	sys1(SYS_close, plain);

	say_text("-- inotify_add_watch");
	say_ret("mask 0", sys3(SYS_inotify_add_watch, fd, ".", 0));
	say_ret("no such path", sys3(SYS_inotify_add_watch, fd, "nothing", IN_CREATE));
	say_ret("a bad fd", sys3(SYS_inotify_add_watch, 99, ".", IN_CREATE));
	int p[2];
	sys2(SYS_pipe2, p, 0);
	say_ret("a pipe", sys3(SYS_inotify_add_watch, p[0], ".", IN_CREATE));
	unsigned all = IN_MODIFY | IN_MOVED_FROM | IN_MOVED_TO | IN_CREATE | IN_DELETE;
	wd_dir = sys3(SYS_inotify_add_watch, fd, ".", all);
	say_bool("the directory", wd_dir > 0);
	say_bool("  again: the same wd", sys3(SYS_inotify_add_watch, fd, ".", all) == wd_dir);
	say_bool("  through another path: the same wd",
		 sys3(SYS_inotify_add_watch, fd, "../inotify-dir", all | IN_MASK_ADD) == wd_dir);
	drain(fd, "read, nothing yet");
	struct pfd pf = { fd, POLLIN, 0 };
	say("poll, nothing yet", sys3(SYS_poll, &pf, 1, 0));

	say_text("-- a directory's entries");
	write_file("a", "1");
	int avail = -1;
	sys3(SYS_ioctl, fd, FIONREAD, &avail);
	say_bool("FIONREAD after a file is made", avail > 0);
	pf.revents = 0;
	say("poll", sys3(SYS_poll, &pf, 1, 0));
	char small[8];
	say_ret("read(8 bytes)", sys3(SYS_read, fd, small, sizeof small));
	drain(fd, "a made and written");
	write_file("a", "22");
	drain(fd, "a written again");
	sys2(SYS_rename, "a", "b");
	drain(fd, "a renamed b");
	sys2(SYS_mkdir, "sub", 0755);
	sys1(SYS_rmdir, "sub");
	drain(fd, "sub made and removed");
	sys1(SYS_unlink, "b");
	drain(fd, "b removed");

	say_text("-- a file");
	write_file("c", "x");
	drain(fd, "c made");
	wd_file = sys3(SYS_inotify_add_watch, fd, "c", IN_MODIFY);
	say_bool("watch c", wd_file > 0 && wd_file != wd_dir);
	say_ret("IN_ONLYDIR on c", sys3(SYS_inotify_add_watch, fd, "c", IN_MODIFY | IN_ONLYDIR));
	i64 cf = sys2(SYS_open, "c", O_WRONLY);
	sys3(SYS_write, cf, "y", 1);
	sys1(SYS_close, cf);
	drain(fd, "c written");

	say_text("-- inotify_rm_watch");
	say_ret("rm_watch(the file's)", sys2(SYS_inotify_rm_watch, fd, wd_file));
	say_ret("rm_watch(it again)", sys2(SYS_inotify_rm_watch, fd, wd_file));
	drain(fd, "after rm_watch");
	sys1(SYS_unlink, "c");
	drain(fd, "c removed");
	sys1(SYS_close, fd);
	sys1(SYS_chdir, "..");
	sys1(SYS_rmdir, "inotify-dir");
	say_text("inotify_test: ok");
	return 0;
}
