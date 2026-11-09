/* run_test: /run as Linux has it, a tmpfs mounted on the root file system
 * (ROADMAP LD; where the Wayland server's runtime directory
 * /run/user/<uid> lives): its own device (st_dev not /'s), `..` out of
 * it back to /, `.` in it itself, / listing it once, getcwd through it,
 * EXDEV for a rename or hard link from one side to the other (checked
 * before anything else about the names, so names that do not exist
 * show it), EEXIST for mkdir of it.
 *
 * Prints nothing that depends on who runs it (the checks are the ones
 * made before permissions) or on the file systems' kinds: only
 * comparisons of device and inode numbers, never the numbers. Recorded
 * with /run a tmpfs of mode 0755 (a systemd system's, and CI's; WORKFLOW
 * "Linux test programs" says how where /run is not a mount). */
#define _GNU_SOURCE
#include "../common/lx.h"
#include "../common/more.h"
#include <fcntl.h>
#include <sys/stat.h>
#include <sys/syscall.h>

/* struct linux_dirent64 (getdents64(2)): d_name starts at 19. */
struct lx_dirent64 {
	u64 d_ino;
	i64 d_off;
	unsigned short d_reclen;
	unsigned char d_type;
	char d_name[];
};

static i64 stat_(const char *path, struct stat *st)
{
	return sys4(SYS_newfstatat, AT_FDCWD, path, st, 0);
}

static void say_same(const char *name, struct stat *a, struct stat *b)
{
	say_bool(name, a->st_dev == b->st_dev && a->st_ino == b->st_ino);
}

static void say_cwd(const char *name)
{
	char buf[256];
	i64 r = sys2(SYS_getcwd, buf, sizeof buf);
	put(name);
	put(" = ");
	if (r < 0)
		put_ret(r);
	else
		put(buf);
	end_line();
}

/* How many entries of / are named "run", and their d_type. */
static void count_run_in_root(void)
{
	static char buf[8192] __attribute__((aligned(8)));
	i64 fd = sys4(SYS_openat, AT_FDCWD, "/", O_RDONLY | O_DIRECTORY, 0);
	int count = 0, type = -1;
	for (;;) {
		i64 n = sys3(SYS_getdents64, fd, buf, sizeof buf);
		if (n <= 0)
			break;
		for (i64 at = 0; at < n;) {
			struct lx_dirent64 *d = (struct lx_dirent64 *)(buf + at);
			if (lx_streq(d->d_name, "run")) {
				count++;
				type = d->d_type;
			}
			at += d->d_reclen;
		}
	}
	sys1(SYS_close, fd);
	say("getdents64(/): entries named run", count);
	say_bool("getdents64(/): run is DT_DIR", type == 4); /* DT_DIR */
}

int main(void)
{
	struct stat root, run, st;
	lx_init();

	say_text("-- the mount");
	say_ret("stat(/)", stat_("/", &root));
	say_ret("stat(/run)", stat_("/run", &run));
	say_bool("/run is a directory", S_ISDIR(run.st_mode));
	say_oct("/run mode", run.st_mode & 07777);
	say_bool("/run's st_dev differs from /'s", run.st_dev != root.st_dev);
	stat_("/run/..", &st);
	say_same("/run/.. is /", &st, &root);
	stat_("/run/.", &st);
	say_same("/run/. is /run", &st, &run);
	stat_("/run/../run", &st);
	say_same("/run/../run is /run", &st, &run);
	stat_("//run/", &st);
	say_same("//run/ is /run", &st, &run);
	count_run_in_root();

	say_text("-- through it");
	i64 fd = sys4(SYS_openat, AT_FDCWD, "/run", O_RDONLY | O_DIRECTORY, 0);
	say_ok("open(/run)", fd);
	sys2(SYS_fstat, fd, &st);
	say_same("fstat(/run fd) is /run", &st, &run);
	sys4(SYS_newfstatat, fd, "..", &st, 0);
	say_same("fstatat(/run fd, ..) is /", &st, &root);
	say_ret("chdir(/run)", sys1(SYS_chdir, "/run"));
	say_cwd("getcwd");
	stat_(".", &st);
	say_same("stat(.) is /run", &st, &run);
	say_ret("chdir(..)", sys1(SYS_chdir, ".."));
	say_cwd("getcwd");
	say_ret("fchdir(/run fd)", sys1(SYS_fchdir, fd));
	say_cwd("getcwd");
	say_ret("chdir(../run/.)", sys1(SYS_chdir, "../run/."));
	say_cwd("getcwd");
	sys1(SYS_close, fd);
	sys1(SYS_chdir, "/");

	say_text("-- across it");
	say_ret("rename(/run/gala-none, /gala-none)",
		sys2(SYS_rename, "/run/gala-run-test-none", "/gala-run-test-none"));
	say_ret("rename(/gala-none, /run/gala-none)",
		sys2(SYS_rename, "/gala-run-test-none", "/run/gala-run-test-none"));
	say_ret("renameat2(/run/gala-none, /gala-none, NOREPLACE)",
		sys5(SYS_renameat2, AT_FDCWD, "/run/gala-run-test-none", AT_FDCWD,
		     "/gala-run-test-none", 1));
	say_ret("link(/run, /gala-none)", sys2(SYS_link, "/run", "/gala-run-test-none"));
	say_ret("mkdir(/run)", sys2(SYS_mkdir, "/run", 0755));
	say_ret("mkdir(/run/)", sys2(SYS_mkdir, "/run/", 0755));
	say_ret("rmdir(/run/.)", sys1(SYS_rmdir, "/run/."));
	return 0;
}
