/* fileio_test: the file and directory calls a static busybox makes for
 * its shell and coreutils applets (ROADMAP LX milestone 2), on regular
 * files and directories only: open/openat and their flags, read, write,
 * pread64/pwrite64, lseek, ftruncate, fstat/stat/lstat/newfstatat,
 * getdents64, mkdir/mkdirat, rmdir, unlink/unlinkat, rename/renameat,
 * access/faccessat, chdir/fchdir/getcwd, umask, utimensat (times set
 * and read back), readlink of a file that
 * is not a link, and the errors (ENOENT, EEXIST, ENOTDIR, EISDIR,
 * ENOTEMPTY, EBADF, EINVAL). fs_test is the fuller spec (symlinks, hard
 * links, unlink of an open file, statx, renameat2's flags).
 *
 * Works in a directory of its own, `fileio.dir`, made in the current
 * directory (lx-expect's run directory; in gala the program's own). Prints
 * only what every Linux file system keeping POSIX metadata agrees on:
 * types, modes, sizes of files, link counts of files, directory entries
 * sorted by name; never inode numbers, times or directory sizes. */
#define _GNU_SOURCE
#include "../common/lx.h"
#include "../common/more.h"
#include <fcntl.h>
#include <sys/stat.h>

struct lx_dirent64 {
	u64 d_ino;
	i64 d_off;
	unsigned short d_reclen;
	unsigned char d_type;
	char d_name[];
};

static i64 open_(const char *path, int flags, int mode)
{
	return sys4(SYS_openat, AT_FDCWD, path, flags, mode);
}

static const char *type_name(unsigned mode)
{
	switch (mode & S_IFMT) {
	case S_IFREG: return "regular";
	case S_IFDIR: return "directory";
	case S_IFLNK: return "symlink";
	}
	return "other";
}

/* "name = type, mode 0644, nlink 1, size 6" (nlink and size for files
 * only: a directory's are file-system specific). */
static void say_stat(const char *name, i64 r, struct stat *st)
{
	put(name);
	put(" = ");
	if (r < 0) {
		put_ret(r);
		end_line();
		return;
	}
	put(type_name(st->st_mode));
	put(", mode 0");
	for (int shift = 6; shift >= 0; shift -= 3)
		put_char('0' + ((st->st_mode >> shift) & 7));
	if (!S_ISDIR(st->st_mode)) {
		put(", nlink ");
		put_dec(st->st_nlink);
		put(", size ");
		put_dec(st->st_size);
	}
	end_line();
}

static void stat_path(const char *name, const char *path)
{
	struct stat st;
	i64 r = sys2(SYS_stat, path, &st);
	say_stat(name, r, &st);
}

static void make_file(const char *path, const char *content)
{
	i64 fd = open_(path, O_WRONLY | O_CREAT | O_TRUNC, 0666);
	write_str(fd, content);
	sys1(SYS_close, fd);
}

static void say_content(const char *name, const char *path)
{
	char buf[64];
	i64 fd = open_(path, O_RDONLY, 0);
	if (fd < 0) {
		say_ret(name, fd);
		return;
	}
	i64 n = sys3(SYS_read, fd, buf, sizeof buf);
	sys1(SYS_close, fd);
	say_read(name, n, buf);
}

/* Lists a directory with getdents64 (a small buffer, so it takes several
 * calls): "name type", sorted; then whether every record is as long as
 * Linux makes it and the last call returned 0. */
static void list_dir(const char *title, const char *path, unsigned bufsize)
{
	static char buf[4096] __attribute__((aligned(8)));
	static char names[32][64];
	char *v[32];
	int n = 0, calls = 0;
	i64 fd = open_(path, O_RDONLY | O_DIRECTORY, 0);
	int reclen_ok = 1;
	for (;;) {
		i64 r = sys3(SYS_getdents64, fd, buf, bufsize);
		if (r <= 0) {
			if (r < 0)
				say_ret("getdents64", r);
			break;
		}
		calls++;
		for (i64 pos = 0; pos < r;) {
			struct lx_dirent64 *d = (struct lx_dirent64 *)(buf + pos);
			if ((u64)(19 + lx_strlen(d->d_name) + 1 + 7) / 8 * 8 != d->d_reclen)
				reclen_ok = 0;
			if (d->d_ino == 0)
				reclen_ok = 0;
			if (n < 32) {
				char *o = names[n];
				int k = 0;
				for (const char *s = d->d_name; *s && k < 40; s++)
					o[k++] = *s;
				const char *t = d->d_type == 4 ? " dir" : d->d_type == 8 ? " reg" : " other";
				while (*t)
					o[k++] = *t++;
				o[k] = 0;
				v[n] = o;
				n++;
			}
			pos += d->d_reclen;
		}
	}
	sys1(SYS_close, fd);
	sort_strings(v, n);
	put(title);
	put(":");
	for (int i = 0; i < n; i++) {
		put(" [");
		put(v[i]);
		put("]");
	}
	end_line();
	say_bool("  every d_reclen = align8(19 + name + NUL), d_ino != 0", reclen_ok);
	if (bufsize <= 64)
		say_bool("  took more than one call", calls > 1);
}

int main(void)
{
	char buf[64];
	struct stat st;
	lx_init();

	say_oct("umask(022) returns the old mask", sys1(SYS_umask, 022));
	say_ret("mkdir(fileio.dir)", sys2(SYS_mkdir, "fileio.dir", 0777));
	say_ret("chdir(fileio.dir)", sys1(SYS_chdir, "fileio.dir"));

	say_text("-- open, read, write");
	i64 fd = open_("f", O_WRONLY | O_CREAT | O_EXCL, 0666);
	say("open(f, O_CREAT|O_EXCL) fd", fd);
	say_ret("write(f, hello)", write_str(fd, "hello\n"));
	say_ret("read(write-only fd)", sys3(SYS_read, fd, buf, 1));
	say_ret("fcntl(F_GETFL) & O_ACCMODE", sys2(SYS_fcntl, fd, F_GETFL) & O_ACCMODE);
	sys1(SYS_close, fd);
	stat_path("stat(f) after umask 022", "f");
	say_ret("open(f, O_CREAT|O_EXCL) again", open_("f", O_WRONLY | O_CREAT | O_EXCL, 0666));
	say_ret("open(missing)", open_("missing", O_RDONLY, 0));
	say_ret("open(missing/x)", open_("missing/x", O_RDONLY, 0));
	say_ret("open(f/x)", open_("f/x", O_RDONLY, 0));
	say_ret("open(f, O_DIRECTORY)", open_("f", O_RDONLY | O_DIRECTORY, 0));
	say_ret("open(\"\")", open_("", O_RDONLY, 0));
	say_ret("open(f/) (a file with a slash)", open_("f/", O_RDONLY, 0));
	say_ret("open(bad pointer)", open_((const char *)8, O_RDONLY, 0));
	fd = open_("f", O_RDONLY, 0);
	say_read("read(f)", sys3(SYS_read, fd, buf, sizeof buf), buf);
	say_ret("read(f) at EOF", sys3(SYS_read, fd, buf, sizeof buf));
	say_ret("write(read-only fd)", sys3(SYS_write, fd, "x", 1));
	say_ret("lseek(f, 1, SEEK_SET)", sys3(SYS_lseek, fd, 1, 0));
	say_read("read(f, 3)", sys3(SYS_read, fd, buf, 3), buf);
	say_ret("lseek(f, 0, SEEK_CUR)", sys3(SYS_lseek, fd, 0, 1));
	say_ret("lseek(f, -2, SEEK_END)", sys3(SYS_lseek, fd, -2, 2));
	say_ret("lseek(f, -1, SEEK_SET)", sys3(SYS_lseek, fd, -1, 0));
	say_ret("lseek(f, 0, 7)", sys3(SYS_lseek, fd, 0, 7));
	say_ret("lseek(f, 100, SEEK_SET) (past the end)", sys3(SYS_lseek, fd, 100, 0));
	say_ret("read past the end", sys3(SYS_read, fd, buf, sizeof buf));
	say_read("pread64(f, 2, at 1)", sys4(SYS_pread64, fd, buf, 2, 1), buf);
	say_ret("  the offset after pread64 is still", sys3(SYS_lseek, fd, 0, 1));
	say_ret("pread64(f, at -1)", sys4(SYS_pread64, fd, buf, 2, -1));
	say_ret("fcntl(F_GETFD) without O_CLOEXEC", sys2(SYS_fcntl, fd, F_GETFD));
	i64 dup = sys1(SYS_dup, fd);
	sys3(SYS_lseek, fd, 2, 0);
	say_ret("lseek(dup, 0, SEEK_CUR) (the offset is shared)", sys3(SYS_lseek, dup, 0, 1));
	sys1(SYS_close, dup);
	sys1(SYS_close, fd);
	say_ret("close(f) again", sys1(SYS_close, fd));

	fd = open_("f", O_WRONLY | O_TRUNC, 0);
	say_ret("fstat(O_TRUNC fd)", sys2(SYS_fstat, fd, &st));
	say("  size after O_TRUNC", st.st_size);
	write_str(fd, "ab");
	sys1(SYS_close, fd);
	fd = open_("f", O_WRONLY | O_APPEND, 0);
	say_ret("lseek(O_APPEND fd, 0, SEEK_SET)", sys3(SYS_lseek, fd, 0, 0));
	say_ret("write(O_APPEND fd, cd)", write_str(fd, "cd"));
	say_ret("lseek(O_APPEND fd, 0, SEEK_CUR)", sys3(SYS_lseek, fd, 0, 1));
	say_bool("fcntl(F_GETFL) has O_APPEND", (sys2(SYS_fcntl, fd, F_GETFL) & O_APPEND) != 0);
	sys1(SYS_close, fd);
	say_content("content after O_APPEND", "f");

	fd = open_("f", O_RDWR, 0);
	say_ret("pwrite64(f, XY, at 6) (past the end)", sys4(SYS_pwrite64, fd, "XY", 2, 6));
	say_ret("  the offset after pwrite64 is still", sys3(SYS_lseek, fd, 0, 1));
	say_read("  read(f) (a hole of zeros)", sys3(SYS_read, fd, buf, sizeof buf), buf);
	say_ret("ftruncate(f, 3)", sys2(SYS_ftruncate, fd, 3));
	say_ret("fstat(f)", sys2(SYS_fstat, fd, &st));
	say("  size", st.st_size);
	say_ret("ftruncate(f, 5)", sys2(SYS_ftruncate, fd, 5));
	say_read("  pread64(f) (zero filled)", sys4(SYS_pread64, fd, buf, sizeof buf, 0), buf);
	say_ret("ftruncate(f, -1)", sys2(SYS_ftruncate, fd, -1));
	say_ret("fsync(f)", sys1(SYS_fsync, fd));
	sys1(SYS_close, fd);
	fd = open_("f", O_RDONLY, 0);
	say_ret("ftruncate(read-only fd)", sys2(SYS_ftruncate, fd, 0));
	sys1(SYS_close, fd);
	fd = open_("f", O_RDONLY | O_CLOEXEC, 0);
	say_ret("fcntl(F_GETFD) with O_CLOEXEC", sys2(SYS_fcntl, fd, F_GETFD));
	sys1(SYS_close, fd);
	fd = open_("mode600", O_WRONLY | O_CREAT, 0600);
	sys1(SYS_close, fd);
	stat_path("stat(created with 0600)", "mode600");
	sys1(SYS_umask, 077);
	fd = open_("umask077", O_WRONLY | O_CREAT, 0666);
	sys1(SYS_close, fd);
	say_oct("umask(022) after umask(077) returns", sys1(SYS_umask, 022));
	stat_path("stat(created under umask 077)", "umask077");

	say_text("-- stat");
	say_ret("stat(missing)", sys2(SYS_stat, "missing", &st));
	say_ret("stat(f/x)", sys2(SYS_stat, "f/x", &st));
	say_ret("stat(bad pointer)", sys2(SYS_stat, "f", (void *)8));
	say_ret("lstat(f)", sys2(SYS_lstat, "f", &st));
	say_bool("  S_ISREG", S_ISREG(st.st_mode));
	say_ret("fstat(999)", sys2(SYS_fstat, 999, &st));
	say_ret("newfstatat(AT_FDCWD, f, 0)", sys4(SYS_newfstatat, AT_FDCWD, "f", &st, 0));
	say("  size", st.st_size);
	say_ret("newfstatat(bad flags)", sys4(SYS_newfstatat, AT_FDCWD, "f", &st, 1));
	stat_path("stat(.)", ".");
	stat_path("stat(/)", "/");
	say_ret("readlink(f) (not a link)", sys3(SYS_readlink, "f", buf, sizeof buf));
	say_ret("readlink(missing)", sys3(SYS_readlink, "missing", buf, sizeof buf));
	say_ret("access(f, F_OK)", sys2(SYS_access, "f", F_OK));
	say_ret("access(f, R_OK|W_OK)", sys2(SYS_access, "f", R_OK | W_OK));
	say_ret("access(missing, F_OK)", sys2(SYS_access, "missing", F_OK));
	say_ret("access(f, bad mode)", sys2(SYS_access, "f", 8));
	say_ret("faccessat(AT_FDCWD, f, F_OK, 0)", sys4(SYS_faccessat, AT_FDCWD, "f", F_OK, 0));

	say_text("-- times");
	struct lx_ts ts[2] = { { 2000, 0 }, { 1000000000, 123456700 } };
	say_ret("utimensat(f, atime 2000, mtime 1e9 + 123456700 ns)", sys4(SYS_utimensat, AT_FDCWD, "f", ts, 0));
	sys2(SYS_stat, "f", &st);
	say("  st_atime", st.st_atime);
	say("  st_mtime", st.st_mtime);
	say("  st_mtim.tv_nsec", st.st_mtim.tv_nsec);
	ts[0].nsec = UTIME_OMIT;
	ts[1].sec = 1500000000;
	ts[1].nsec = 0;
	say_ret("utimensat(f, atime UTIME_OMIT, mtime 1.5e9)", sys4(SYS_utimensat, AT_FDCWD, "f", ts, 0));
	sys2(SYS_stat, "f", &st);
	say("  st_atime (kept)", st.st_atime);
	say("  st_mtime", st.st_mtime);
	say_ret("utimensat(f, NULL) (now)", sys4(SYS_utimensat, AT_FDCWD, "f", 0, 0));
	sys2(SYS_stat, "f", &st);
	say_bool("  st_mtime after 2020-01-01", st.st_mtime > 1577836800);
	fd = open_("f", O_RDONLY, 0);
	ts[0].sec = 3000;
	ts[0].nsec = 0;
	ts[1].nsec = UTIME_NOW;
	say_ret("futimens(fd, atime 3000, mtime UTIME_NOW)", sys4(SYS_utimensat, fd, 0, ts, 0));
	sys2(SYS_fstat, fd, &st);
	say("  st_atime", st.st_atime);
	sys1(SYS_close, fd);
	ts[1].nsec = 1000000000;
	say_ret("utimensat(tv_nsec = 1e9)", sys4(SYS_utimensat, AT_FDCWD, "f", ts, 0));
	say_ret("utimensat(missing)", sys4(SYS_utimensat, AT_FDCWD, "missing", 0, 0));

	say_text("-- directories");
	say_ret("mkdir(d)", sys2(SYS_mkdir, "d", 0777));
	say_ret("mkdir(d) again", sys2(SYS_mkdir, "d", 0777));
	say_ret("mkdir(missing/d)", sys2(SYS_mkdir, "missing/d", 0777));
	say_ret("mkdir(f/d)", sys2(SYS_mkdir, "f/d", 0777));
	stat_path("stat(d)", "d");
	say_ret("mkdir(d700, 0700)", sys2(SYS_mkdir, "d700", 0700));
	stat_path("stat(d700)", "d700");
	say_ret("open(d, O_WRONLY)", open_("d", O_WRONLY, 0));
	say_ret("open(d, O_RDWR)", open_("d", O_RDWR, 0));
	say_ret("open(d, O_CREAT)", open_("d", O_RDONLY | O_CREAT, 0666));
	i64 dfd = open_("d", O_RDONLY | O_DIRECTORY, 0);
	say_ok("open(d, O_DIRECTORY)", dfd);
	say_ret("read(directory fd)", sys3(SYS_read, dfd, buf, sizeof buf));
	fd = sys4(SYS_openat, dfd, "inner", O_WRONLY | O_CREAT, 0600);
	say_ok("openat(d, inner, O_CREAT)", fd);
	write_str(fd, "inside");
	sys1(SYS_close, fd);
	say_content("read(d/inner) from AT_FDCWD", "d/inner");
	say_content("read(./d/../d/inner)", "./d/../d/inner");
	fd = open_("f", O_RDONLY, 0);
	say_ret("openat(file fd, x)", sys4(SYS_openat, fd, "x", O_RDONLY, 0));
	sys1(SYS_close, fd);
	say_ret("openat(999, x)", sys4(SYS_openat, 999, "x", O_RDONLY, 0));
	fd = sys4(SYS_openat, 999, "/", O_RDONLY | O_DIRECTORY, 0);
	say_ok("openat(999, /) (absolute: dirfd unused)", fd);
	sys1(SYS_close, fd);
	say_ret("mkdirat(d, sub)", sys3(SYS_mkdirat, dfd, "sub", 0777));
	say_ret("newfstatat(d, inner, 0)", sys4(SYS_newfstatat, dfd, "inner", &st, 0));
	say_oct("  mode", st.st_mode);
	say_ret("newfstatat(d, \"\", AT_EMPTY_PATH)", sys4(SYS_newfstatat, dfd, "", &st, AT_EMPTY_PATH));
	say_bool("  S_ISDIR", S_ISDIR(st.st_mode));
	for (int i = 0; i < 12; i++) {
		char name[] = "d/many-00";
		name[7] = '0' + i / 10;
		name[8] = '0' + i % 10;
		make_file(name, "m");
	}
	list_dir("getdents64(d)", "d", sizeof buf);
	list_dir("getdents64(d/sub)", "d/sub", 4096);
	fd = open_("f", O_RDONLY, 0);
	say_ret("getdents64(file)", sys3(SYS_getdents64, fd, buf, sizeof buf));
	sys1(SYS_close, fd);
	fd = open_("d", O_RDONLY | O_DIRECTORY, 0);
	say_ret("getdents64(d, 8-byte buffer)", sys3(SYS_getdents64, fd, buf, 8));
	sys1(SYS_close, fd);
	say_ret("getdents64(999)", sys3(SYS_getdents64, 999, buf, sizeof buf));

	say_ret("mkdir(cd)", sys2(SYS_mkdir, "cd", 0777));
	say_ret("chdir(cd)", sys1(SYS_chdir, "cd"));
	static char cwd[4096];
	i64 n = sys2(SYS_getcwd, cwd, sizeof cwd);
	say_bool("getcwd ends with /fileio.dir/cd (length with the NUL)",
		 n >= 15 && lx_memeq(cwd + n - 15, "/fileio.dir/cd", 15));
	say_bool("  starts with /", n > 0 && cwd[0] == '/');
	say_ret("getcwd(2-byte buffer)", sys2(SYS_getcwd, cwd, 2));
	say_ret("chdir(..)", sys1(SYS_chdir, ".."));
	say_ret("chdir(f)", sys1(SYS_chdir, "f"));
	say_ret("chdir(missing)", sys1(SYS_chdir, "missing"));
	say_ret("fchdir(d)", sys1(SYS_fchdir, dfd));
	say_content("read(inner) after fchdir", "inner");
	sys1(SYS_chdir, "..");
	n = sys2(SYS_getcwd, cwd, sizeof cwd);
	say_bool("getcwd after chdir(..) ends with /fileio.dir", n >= 12 && lx_memeq(cwd + n - 12, "/fileio.dir", 12));
	fd = open_("f", O_RDONLY, 0);
	say_ret("fchdir(file fd)", sys1(SYS_fchdir, fd));
	sys1(SYS_close, fd);

	say_text("-- rename");
	make_file("a", "A");
	make_file("b", "B");
	say_ret("rename(a, c)", sys2(SYS_rename, "a", "c"));
	say_ret("stat(a)", sys2(SYS_stat, "a", &st));
	say_content("read(c)", "c");
	say_ret("rename(c, b) (replaces b)", sys2(SYS_rename, "c", "b"));
	say_content("read(b)", "b");
	say_ret("rename(nonexistent, x)", sys2(SYS_rename, "nonexistent", "x"));
	say_ret("renameat(d, inner, AT_FDCWD, moved)", sys4(SYS_renameat, dfd, "inner", AT_FDCWD, "moved"));
	say_content("  read(moved)", "moved");
	say_ret("mkdir(e)", sys2(SYS_mkdir, "e", 0777));
	say_ret("mkdir(e/x)", sys2(SYS_mkdir, "e/x", 0777));
	say_ret("rename(d, e) (e not empty)", sys2(SYS_rename, "d", "e"));
	say_ret("rename(d, b) (b a file)", sys2(SYS_rename, "d", "b"));
	say_ret("rename(b, e) (e a directory)", sys2(SYS_rename, "b", "e"));
	say_ret("rename(e, e/x/y) (into itself)", sys2(SYS_rename, "e", "e/x/y"));
	say_ret("mkdir(empty)", sys2(SYS_mkdir, "empty", 0777));
	say_ret("rename(e, empty) (empty replaced)", sys2(SYS_rename, "e", "empty"));
	stat_path("stat(empty/x)", "empty/x");
	say_ret("rename(b, b) (same file: no-op)", sys2(SYS_rename, "b", "b"));
	say_content("read(b)", "b");

	say_text("-- unlink and rmdir");
	say_ret("unlink(b)", sys1(SYS_unlink, "b"));
	say_ret("stat(b)", sys2(SYS_stat, "b", &st));
	say_ret("unlink(b) again", sys1(SYS_unlink, "b"));
	say_ret("unlink(d)", sys1(SYS_unlink, "d"));
	say_ret("unlink(f/x)", sys1(SYS_unlink, "f/x"));
	say_ret("rmdir(empty) (not empty)", sys1(SYS_rmdir, "empty"));
	say_ret("rmdir(f)", sys1(SYS_rmdir, "f"));
	say_ret("rmdir(missing)", sys1(SYS_rmdir, "missing"));
	say_ret("rmdir(.)", sys1(SYS_rmdir, "."));
	say_ret("rmdir(empty/x)", sys1(SYS_rmdir, "empty/x"));
	say_ret("rmdir(empty)", sys1(SYS_rmdir, "empty"));
	say_ret("unlinkat(d, sub, 0)", sys3(SYS_unlinkat, dfd, "sub", 0));
	say_ret("unlinkat(d, sub, AT_REMOVEDIR)", sys3(SYS_unlinkat, dfd, "sub", AT_REMOVEDIR));
	say_ret("unlinkat(d, many-00, AT_REMOVEDIR)", sys3(SYS_unlinkat, dfd, "many-00", AT_REMOVEDIR));
	say_ret("unlinkat(d, many-00, 0)", sys3(SYS_unlinkat, dfd, "many-00", 0));
	say_ret("unlinkat(bad flags)", sys3(SYS_unlinkat, dfd, "many-01", 1));
	for (int i = 1; i < 12; i++) {
		char name[] = "many-00";
		name[5] = '0' + i / 10;
		name[6] = '0' + i % 10;
		sys3(SYS_unlinkat, dfd, name, 0);
	}
	list_dir("getdents64(d) now", "d", 4096);
	sys1(SYS_close, dfd);
	list_dir("getdents64(.)", ".", 4096);
	say_text("fileio_test: ok");
	return 0;
}
