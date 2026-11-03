/* fs_test: files and directories as the raw system calls give them:
 * open/openat and their flags, getdents64, the stat family and statx,
 * rename/renameat2, unlink of an open file, mkdir/rmdir, symlinks, and
 * the errors (ENOENT, EEXIST, ENOTDIR, EISDIR, ENOTEMPTY, EBADF, ELOOP).
 *
 * Runs in its own fresh directory (lx-expect's run directory). Prints
 * only what is the same on every Linux file system that keeps POSIX
 * metadata (ext4, tmpfs, ...): modes, link counts of files, sizes of
 * files and symlinks, types, directory entries sorted by name (their
 * order is not stable); never inode numbers, times or directory sizes.
 * The struct layouts it relies on are printed (offsets from musl's
 * headers, and how many bytes the kernel actually writes) for the lx_abi
 * crate's host tests. */
#define _GNU_SOURCE
#include "../common/lx.h"
#include "../common/more.h"
#include <fcntl.h>
#include <stddef.h>
#include <sys/stat.h>

#define RENAME_NOREPLACE 1
#define RENAME_EXCHANGE 2
#define STATX_TYPE 1U
#define STATX_MODE 2U
#define STATX_NLINK 4U
#define STATX_SIZE 0x200U
#define STATX_BASIC_STATS 0x7ffU

/* struct statx (statx(2)), as the kernel writes it. */
struct lx_statx {
	unsigned stx_mask, stx_blksize;
	u64 stx_attributes;
	unsigned stx_nlink, stx_uid, stx_gid;
	unsigned short stx_mode, pad1;
	u64 stx_ino, stx_size, stx_blocks, stx_attributes_mask;
	struct {
		i64 sec;
		unsigned nsec;
		int pad;
	} stx_atime, stx_btime, stx_ctime, stx_mtime;
	unsigned stx_rdev_major, stx_rdev_minor, stx_dev_major, stx_dev_minor;
	u64 spare[14];
};

/* struct linux_dirent64 (getdents64(2)): d_name starts at 19. */
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
	case S_IFIFO: return "fifo";
	case S_IFCHR: return "char device";
	}
	return "other";
}

/* "name: type, mode 0644, nlink 1, size 6" (nlink and size for files and
 * symlinks only: a directory's are file-system specific). */
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
	i64 r = sys4(SYS_newfstatat, AT_FDCWD, path, &st, 0);
	say_stat(name, r, &st);
}

static void lstat_path(const char *name, const char *path)
{
	struct stat st;
	i64 r = sys4(SYS_newfstatat, AT_FDCWD, path, &st, AT_SYMLINK_NOFOLLOW);
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

/* Lists a directory with getdents64: "name type reclen", sorted. */
static void list_dir(const char *title, const char *path)
{
	static char buf[4096] __attribute__((aligned(8)));
	static char names[32][64];
	char *v[32];
	int n = 0;
	i64 fd = open_(path, O_RDONLY | O_DIRECTORY, 0);
	int aligned = 1, offset_ok = 1;
	for (;;) {
		i64 r = sys3(SYS_getdents64, fd, buf, sizeof buf);
		if (r <= 0) {
			if (r < 0)
				say_ret("getdents64", r);
			break;
		}
		for (i64 pos = 0; pos < r;) {
			struct lx_dirent64 *d = (struct lx_dirent64 *)(buf + pos);
			if (d->d_reclen % 8)
				aligned = 0;
			if ((u64)(19 + lx_strlen(d->d_name) + 1 + 7) / 8 * 8 != d->d_reclen)
				offset_ok = 0;
			if (n < 32) {
				char *o = names[n];
				int k = 0;
				for (const char *s = d->d_name; *s && k < 40; s++)
					o[k++] = *s;
				const char *t = d->d_type == 4 ? " dir" : d->d_type == 8 ? " reg" : d->d_type == 10 ? " lnk" : " other";
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
	say_bool("  every d_reclen 8-aligned", aligned);
	say_bool("  every d_reclen = align8(19 + name + NUL)", offset_ok);
}

static void layouts(void)
{
	say_text("-- layouts");
	say("sizeof(struct stat)", sizeof(struct stat));
	say("offsetof(stat, st_dev)", offsetof(struct stat, st_dev));
	say("offsetof(stat, st_ino)", offsetof(struct stat, st_ino));
	say("offsetof(stat, st_nlink)", offsetof(struct stat, st_nlink));
	say("offsetof(stat, st_mode)", offsetof(struct stat, st_mode));
	say("offsetof(stat, st_uid)", offsetof(struct stat, st_uid));
	say("offsetof(stat, st_gid)", offsetof(struct stat, st_gid));
	say("offsetof(stat, st_rdev)", offsetof(struct stat, st_rdev));
	say("offsetof(stat, st_size)", offsetof(struct stat, st_size));
	say("offsetof(stat, st_blksize)", offsetof(struct stat, st_blksize));
	say("offsetof(stat, st_blocks)", offsetof(struct stat, st_blocks));
	say("offsetof(stat, st_atim)", offsetof(struct stat, st_atim));
	say("offsetof(stat, st_mtim)", offsetof(struct stat, st_mtim));
	say("offsetof(stat, st_ctim)", offsetof(struct stat, st_ctim));

	/* What fstat writes: the whole struct (its padding zeroed). */
	static unsigned char buf[512];
	i64 fd = open_("layout", O_RDWR | O_CREAT, 0666);
	sys3(SYS_write, fd, "0123456789", 10);
	lx_memset(buf, 0xaa, sizeof buf);
	sys2(SYS_fstat, fd, buf);
	say("fstat writes bytes", written_size(buf, sizeof buf, 0xaa));
	struct stat *st = (struct stat *)buf;
	say_bool("st_size at its offset is 10", st->st_size == 10);
	say_bool("st_mode at its offset is 0100644", st->st_mode == 0100644);
	say_bool("st_nlink at its offset is 1", st->st_nlink == 1);

	say("sizeof(struct statx)", sizeof(struct lx_statx));
	say("offsetof(statx, stx_mask)", offsetof(struct lx_statx, stx_mask));
	say("offsetof(statx, stx_nlink)", offsetof(struct lx_statx, stx_nlink));
	say("offsetof(statx, stx_mode)", offsetof(struct lx_statx, stx_mode));
	say("offsetof(statx, stx_ino)", offsetof(struct lx_statx, stx_ino));
	say("offsetof(statx, stx_size)", offsetof(struct lx_statx, stx_size));
	say("offsetof(statx, stx_atime)", offsetof(struct lx_statx, stx_atime));
	say("offsetof(statx, stx_mtime)", offsetof(struct lx_statx, stx_mtime));
	say("offsetof(statx, stx_rdev_major)", offsetof(struct lx_statx, stx_rdev_major));
	lx_memset(buf, 0xaa, sizeof buf);
	sys5(SYS_statx, fd, "", AT_EMPTY_PATH, STATX_BASIC_STATS, buf);
	say("statx writes bytes", written_size(buf, sizeof buf, 0xaa));
	struct lx_statx *sx = (struct lx_statx *)buf;
	say_bool("stx_size at its offset is 10", sx->stx_size == 10);
	say_bool("stx_mode at its offset is 0100644", sx->stx_mode == 0100644);
	sys1(SYS_close, fd);

	say("offsetof(linux_dirent64, d_reclen)", offsetof(struct lx_dirent64, d_reclen));
	say("offsetof(linux_dirent64, d_type)", offsetof(struct lx_dirent64, d_type));
	say("offsetof(linux_dirent64, d_name)", offsetof(struct lx_dirent64, d_name));
}

int main(void)
{
	char buf[64];
	struct stat st;
	lx_init();

	say_text("-- open and openat");
	i64 fd = open_("f", O_WRONLY | O_CREAT | O_EXCL, 0666);
	say("open(f, O_CREAT|O_EXCL) fd", fd);
	say_ret("write(f, hello)", write_str(fd, "hello\n"));
	say_ret("read(write-only fd)", sys3(SYS_read, fd, buf, 1));
	sys1(SYS_close, fd);
	stat_path("stat(f) after umask 022", "f");
	say_ret("open(f, O_CREAT|O_EXCL) again", open_("f", O_WRONLY | O_CREAT | O_EXCL, 0666));
	say_ret("open(missing)", open_("missing", O_RDONLY, 0));
	say_ret("open(missing/x)", open_("missing/x", O_RDONLY, 0));
	say_ret("open(f/x)", open_("f/x", O_RDONLY, 0));
	say_ret("open(f, O_DIRECTORY)", open_("f", O_RDONLY | O_DIRECTORY, 0));
	say_ret("open(\"\")", open_("", O_RDONLY, 0));
	fd = open_("f", O_RDONLY, 0);
	say_read("read(f)", sys3(SYS_read, fd, buf, sizeof buf), buf);
	say_ret("read(f) at EOF", sys3(SYS_read, fd, buf, sizeof buf));
	say_ret("write(read-only fd)", sys3(SYS_write, fd, "x", 1));
	say_ret("lseek(f, 1, SEEK_SET)", sys3(SYS_lseek, fd, 1, 0));
	say_read("read(f, 3)", sys3(SYS_read, fd, buf, 3), buf);
	say_ret("lseek(f, 0, SEEK_END)", sys3(SYS_lseek, fd, 0, 2));
	say_ret("lseek(f, -1, SEEK_SET)", sys3(SYS_lseek, fd, -1, 0));
	say_ret("fcntl(F_GETFD) without O_CLOEXEC", sys2(SYS_fcntl, fd, F_GETFD));
	sys1(SYS_close, fd);
	say_ret("close(f) again", sys1(SYS_close, fd));
	say_ret("close(999)", sys1(SYS_close, 999));

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

	fd = open_("f", O_RDONLY | O_CLOEXEC, 0);
	say_ret("fcntl(F_GETFD) with O_CLOEXEC", sys2(SYS_fcntl, fd, F_GETFD));
	sys1(SYS_close, fd);

	fd = open_("f", O_PATH, 0);
	say_ok("open(f, O_PATH)", fd);
	say_ret("read(O_PATH fd)", sys3(SYS_read, fd, buf, 1));
	say_ret("fstat(O_PATH fd)", sys2(SYS_fstat, fd, &st));
	say("  size", st.st_size);
	say_bool("fcntl(F_GETFL) has O_PATH", (sys2(SYS_fcntl, fd, F_GETFL) & O_PATH) != 0);
	sys1(SYS_close, fd);

	say_text("-- directories");
	say_ret("mkdir(d)", sys2(SYS_mkdir, "d", 0777));
	say_ret("mkdir(d) again", sys2(SYS_mkdir, "d", 0777));
	say_ret("mkdir(missing/d)", sys2(SYS_mkdir, "missing/d", 0777));
	say_ret("mkdir(f/d)", sys2(SYS_mkdir, "f/d", 0777));
	stat_path("stat(d)", "d");
	say_ret("open(d, O_WRONLY)", open_("d", O_WRONLY, 0));
	say_ret("open(d, O_RDWR)", open_("d", O_RDWR, 0));
	i64 dfd = open_("d", O_RDONLY | O_DIRECTORY, 0);
	say_ok("open(d, O_DIRECTORY)", dfd);
	say_ret("read(directory fd)", sys3(SYS_read, dfd, buf, sizeof buf));
	fd = sys4(SYS_openat, dfd, "inner", O_WRONLY | O_CREAT, 0600);
	say_ok("openat(d, inner, O_CREAT)", fd);
	write_str(fd, "inside");
	sys1(SYS_close, fd);
	say_content("read(d/inner) from AT_FDCWD", "d/inner");
	fd = open_("f", O_RDONLY, 0);
	say_ret("openat(file fd, x)", sys4(SYS_openat, fd, "x", O_RDONLY, 0));
	sys1(SYS_close, fd);
	say_ret("openat(999, x)", sys4(SYS_openat, 999, "x", O_RDONLY, 0));
	fd = sys4(SYS_openat, 999, "/", O_RDONLY | O_DIRECTORY, 0);
	say_ok("openat(999, /) (absolute: dirfd unused)", fd);
	sys1(SYS_close, fd);
	say_ret("mkdirat(d, sub)", sys3(SYS_mkdirat, dfd, "sub", 0700));
	say_ret("symlinkat(../f, d, ln)", sys3(SYS_symlinkat, "../f", dfd, "ln"));
	list_dir("getdents64(d)", "d");
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
	say_bool("getcwd ends with /cd (length with the NUL)", n >= 4 && lx_memeq(cwd + n - 4, "/cd", 4));
	say_ret("getcwd(2-byte buffer)", sys2(SYS_getcwd, cwd, 2));
	say_ret("chdir(..)", sys1(SYS_chdir, ".."));
	say_ret("chdir(f)", sys1(SYS_chdir, "f"));
	say_ret("fchdir(d)", sys1(SYS_fchdir, dfd));
	say_content("read(inner) after fchdir", "inner");
	sys1(SYS_chdir, "..");

	say_text("-- stat family");
	say_ret("link(f, f2)", sys2(SYS_link, "f", "f2"));
	stat_path("stat(f) with two links", "f");
	say_ret("link(f, f2) again", sys2(SYS_link, "f", "f2"));
	say_ret("link(d, d2)", sys2(SYS_link, "d", "d2"));
	say_ret("symlink(f, lnk)", sys2(SYS_symlink, "f", "lnk"));
	say_ret("symlink(missing, dangling)", sys2(SYS_symlink, "missing", "dangling"));
	say_ret("symlink(x, f)", sys2(SYS_symlink, "x", "f"));
	stat_path("stat(lnk)", "lnk");
	lstat_path("lstat(lnk)", "lnk");
	stat_path("stat(dangling)", "dangling");
	lstat_path("lstat(dangling)", "dangling");
	say_ret("lstat syscall(lnk)", sys2(SYS_lstat, "lnk", &st));
	say_bool("  S_ISLNK", S_ISLNK(st.st_mode));
	say_ret("stat syscall(lnk)", sys2(SYS_stat, "lnk", &st));
	say_bool("  S_ISREG", S_ISREG(st.st_mode));
	say_ret("stat syscall(missing)", sys2(SYS_stat, "missing", &st));
	say_ret("fstat(999)", sys2(SYS_fstat, 999, &st));
	say_ret("newfstatat(d, inner)", sys4(SYS_newfstatat, dfd, "inner", &st, 0));
	say_oct("  mode", st.st_mode);
	say("  size", st.st_size);
	say_ret("newfstatat(d, \"\", AT_EMPTY_PATH)", sys4(SYS_newfstatat, dfd, "", &st, AT_EMPTY_PATH));
	say_bool("  S_ISDIR", S_ISDIR(st.st_mode));
	say_ret("newfstatat(d, \"\", 0)", sys4(SYS_newfstatat, dfd, "", &st, 0));
	say_ret("newfstatat(AT_FDCWD, d/ln, AT_SYMLINK_NOFOLLOW)", sys4(SYS_newfstatat, AT_FDCWD, "d/ln", &st, AT_SYMLINK_NOFOLLOW));
	say_oct("  mode", st.st_mode);
	say("  size", st.st_size);
	say_ret("newfstatat(bad flags)", sys4(SYS_newfstatat, AT_FDCWD, "f", &st, 0x8000000));

	struct lx_statx sx;
	say_ret("statx(f, STATX_BASIC_STATS)", sys5(SYS_statx, AT_FDCWD, "f", 0, STATX_BASIC_STATS, &sx));
	unsigned want = STATX_TYPE | STATX_MODE | STATX_NLINK | STATX_SIZE;
	say_bool("  stx_mask has TYPE|MODE|NLINK|SIZE", (sx.stx_mask & want) == want);
	say_oct("  stx_mode", sx.stx_mode);
	say("  stx_nlink", sx.stx_nlink);
	say("  stx_size", sx.stx_size);
	say_ret("statx(lnk, AT_SYMLINK_NOFOLLOW)", sys5(SYS_statx, AT_FDCWD, "lnk", AT_SYMLINK_NOFOLLOW, STATX_BASIC_STATS, &sx));
	say_oct("  stx_mode", sx.stx_mode);
	say_ret("statx(d)", sys5(SYS_statx, AT_FDCWD, "d", 0, STATX_BASIC_STATS, &sx));
	say_bool("  S_ISDIR(stx_mode)", S_ISDIR(sx.stx_mode));
	say_ret("statx(missing)", sys5(SYS_statx, AT_FDCWD, "missing", 0, STATX_BASIC_STATS, &sx));
	say_ret("statx(reserved mask bit)", sys5(SYS_statx, AT_FDCWD, "f", 0, 0x80000000U, &sx));

	say_text("-- symlinks");
	n = sys3(SYS_readlink, "lnk", buf, sizeof buf);
	say_read("readlink(lnk)", n, buf);
	n = sys4(SYS_readlinkat, dfd, "ln", buf, sizeof buf);
	say_read("readlinkat(d, ln)", n, buf);
	sys2(SYS_symlink, "abcdef", "long");
	n = sys3(SYS_readlink, "long", buf, 3);
	say_read("readlink(long, 3-byte buffer)", n, buf);
	say_ret("readlink(f)", sys3(SYS_readlink, "f", buf, sizeof buf));
	say_ret("readlink(missing)", sys3(SYS_readlink, "missing", buf, sizeof buf));
	say_ret("open(lnk, O_NOFOLLOW)", open_("lnk", O_RDONLY | O_NOFOLLOW, 0));
	fd = open_("lnk", O_PATH | O_NOFOLLOW, 0);
	say_ok("open(lnk, O_PATH|O_NOFOLLOW)", fd);
	sys2(SYS_fstat, fd, &st);
	say_bool("  fstat: S_ISLNK", S_ISLNK(st.st_mode));
	sys1(SYS_close, fd);
	fd = open_("lnk", O_RDONLY, 0);
	say_read("read(open(lnk))", sys3(SYS_read, fd, buf, sizeof buf), buf);
	sys1(SYS_close, fd);
	say_ret("open(dangling)", open_("dangling", O_RDONLY, 0));
	sys2(SYS_symlink, "loop1", "loop2");
	sys2(SYS_symlink, "loop2", "loop1");
	say_ret("open(loop1)", open_("loop1", O_RDONLY, 0));
	fd = open_("dangling", O_WRONLY | O_CREAT, 0666);
	say_ok("open(dangling, O_CREAT) creates the target", fd);
	sys1(SYS_close, fd);
	stat_path("stat(missing) now", "missing");
	say_ret("open(lnk, O_CREAT|O_EXCL)", open_("lnk", O_WRONLY | O_CREAT | O_EXCL, 0666));

	say_text("-- rename");
	make_file("a", "A");
	make_file("b", "B");
	say_ret("rename(a, c)", sys2(SYS_rename, "a", "c"));
	stat_path("stat(a)", "a");
	say_content("read(c)", "c");
	say_ret("rename(c, b) (replaces b)", sys2(SYS_rename, "c", "b"));
	say_content("read(b)", "b");
	say_ret("rename(nonexistent, x)", sys2(SYS_rename, "nonexistent", "x"));
	make_file("a", "N");
	say_ret("renameat2(a, b, RENAME_NOREPLACE)", sys5(SYS_renameat2, AT_FDCWD, "a", AT_FDCWD, "b", RENAME_NOREPLACE));
	say_ret("renameat2(a, new, RENAME_NOREPLACE)", sys5(SYS_renameat2, AT_FDCWD, "a", AT_FDCWD, "new", RENAME_NOREPLACE));
	say_ret("renameat2(new, b, RENAME_EXCHANGE)", sys5(SYS_renameat2, AT_FDCWD, "new", AT_FDCWD, "b", RENAME_EXCHANGE));
	say_content("  read(new)", "new");
	say_content("  read(b)", "b");
	say_ret("renameat2(NOREPLACE|EXCHANGE)", sys5(SYS_renameat2, AT_FDCWD, "new", AT_FDCWD, "b", RENAME_NOREPLACE | RENAME_EXCHANGE));
	say_ret("renameat2(new, missing, RENAME_EXCHANGE)", sys5(SYS_renameat2, AT_FDCWD, "new", AT_FDCWD, "missing2", RENAME_EXCHANGE));
	say_ret("renameat(d, inner, AT_FDCWD, moved)", sys4(SYS_renameat, dfd, "inner", AT_FDCWD, "moved"));
	say_content("  read(moved)", "moved");
	sys2(SYS_mkdir, "e", 0777);
	sys2(SYS_mkdir, "e/x", 0777);
	say_ret("rename(d, e) (e not empty)", sys2(SYS_rename, "d", "e"));
	say_ret("rename(d, f) (f a file)", sys2(SYS_rename, "d", "f"));
	say_ret("rename(f, e) (e a directory)", sys2(SYS_rename, "f", "e"));
	say_ret("rename(e, e/x/y) (into itself)", sys2(SYS_rename, "e", "e/x/y"));
	sys2(SYS_mkdir, "empty", 0777);
	say_ret("rename(e, empty) (empty replaced)", sys2(SYS_rename, "e", "empty"));
	stat_path("stat(empty/x)", "empty/x");
	say_ret("rename(f, f2) (same file: no-op)", sys2(SYS_rename, "f", "f2"));
	stat_path("stat(f)", "f");

	say_text("-- unlink and rmdir");
	fd = open_("u", O_RDWR | O_CREAT, 0666);
	write_str(fd, "still here");
	say_ret("unlink(u) while open", sys1(SYS_unlink, "u"));
	stat_path("stat(u)", "u");
	sys3(SYS_lseek, fd, 0, 0);
	say_read("read(fd of unlinked u)", sys3(SYS_read, fd, buf, sizeof buf), buf);
	sys2(SYS_fstat, fd, &st);
	say("  fstat nlink", st.st_nlink);
	say("  fstat size", st.st_size);
	say_ret("  write after unlink", write_str(fd, "!"));
	sys1(SYS_close, fd);
	say_ret("unlink(u) again", sys1(SYS_unlink, "u"));
	say_ret("unlink(d)", sys1(SYS_unlink, "d"));
	say_ret("unlink(lnk) (the link, not f)", sys1(SYS_unlink, "lnk"));
	stat_path("stat(f)", "f");
	say_ret("unlink(f/x)", sys1(SYS_unlink, "f/x"));
	say_ret("rmdir(empty) (not empty)", sys1(SYS_rmdir, "empty"));
	say_ret("rmdir(f)", sys1(SYS_rmdir, "f"));
	say_ret("rmdir(missing2)", sys1(SYS_rmdir, "missing2"));
	say_ret("rmdir(.)", sys1(SYS_rmdir, "."));
	say_ret("rmdir(empty/x)", sys1(SYS_rmdir, "empty/x"));
	say_ret("rmdir(empty)", sys1(SYS_rmdir, "empty"));
	say_ret("unlinkat(d, sub, 0)", sys3(SYS_unlinkat, dfd, "sub", 0));
	say_ret("unlinkat(d, sub, AT_REMOVEDIR)", sys3(SYS_unlinkat, dfd, "sub", AT_REMOVEDIR));
	say_ret("unlinkat(d, ln, AT_REMOVEDIR)", sys3(SYS_unlinkat, dfd, "ln", AT_REMOVEDIR));
	say_ret("unlinkat(d, ln, 0)", sys3(SYS_unlinkat, dfd, "ln", 0));
	say_ret("unlinkat(bad flags)", sys3(SYS_unlinkat, dfd, "x", 0x100));
	list_dir("getdents64(d) now", "d");
	sys1(SYS_close, dfd);
	list_dir("getdents64(.)", ".");

	layouts();
	say_text("fs_test: ok");
	return 0;
}
