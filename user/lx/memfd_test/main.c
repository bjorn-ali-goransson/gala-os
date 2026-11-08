/* memfd_test: memfd_create(2) as Wayland clients and GTK use it for
 * shared-memory buffers (ROADMAP LD step 2): an anonymous file with
 * read, write, lseek, ftruncate and fstat; mmap(MAP_SHARED) whose pages
 * every mapping shares (two mappings in one process, a forked child's
 * old and new mappings), MAP_PRIVATE copies, a mapping at an offset, one
 * grown with mremap after the file grew, a mapping outliving its fd;
 * MFD_CLOEXEC and MFD_ALLOW_SEALING; the seals (fcntl F_ADD_SEALS and
 * F_GET_SEALS: F_SEAL_SHRINK, F_SEAL_GROW, F_SEAL_WRITE, F_SEAL_SEAL) and
 * what each refuses; MFD_NOEXEC_SEAL (F_SEAL_EXEC); the errors.
 *
 * Nothing printed varies: fd numbers are fixed by lx_init, the memfd's
 * name is never read back (its /proc link would show it), contents are
 * compared, not printed with addresses. */
#include "../common/lx.h"
#include "../common/more.h"

#define PROT_READ 1
#define PROT_WRITE 2
#define MAP_SHARED 1
#define MAP_PRIVATE 2
#define MREMAP_MAYMOVE 1
#define MFD_CLOEXEC 1
#define MFD_ALLOW_SEALING 2
#define MFD_NOEXEC_SEAL 8
#define MFD_EXEC 0x10
#define F_GETFD_ 1
#define F_GETFL_ 3
#define F_ADD_SEALS_ 1033
#define F_GET_SEALS_ 1034
#define SEAL_SEAL 1
#define SEAL_SHRINK 2
#define SEAL_GROW 4
#define SEAL_WRITE 8
#define SEEK_SET_ 0
#define SEEK_END_ 2
#define PAGE 4096L

struct lx_stat {
	u64 dev, ino, nlink;
	unsigned mode, uid, gid, pad;
	u64 rdev;
	i64 size, blksize, blocks;
	u64 times[6];
	i64 reserved[3];
};

static i64 map(u64 len, int prot, int flags, i64 fd, i64 off)
{
	return sys6(SYS_mmap, 0, len, prot, flags, fd, off);
}

static int is_map(i64 m)
{
	return m >= 0 || m < -4095;
}

static void say_same(const char *name, const char *p, const char *want)
{
	say_bool(name, lx_memeq(p, want, lx_strlen(want)));
}

static void say_pread(const char *name, i64 fd, i64 at, const char *want)
{
	char buf[64];
	u64 n = lx_strlen(want);
	i64 r = sys4(SYS_pread64, fd, buf, n, at);
	put(name);
	put(" = ");
	if (r == (i64)n && lx_memeq(buf, want, n))
		put("yes");
	else if (r < 0)
		put_ret(r);
	else
		put("no");
	end_line();
}

static void say_stat(const char *name, i64 fd)
{
	struct lx_stat st;
	i64 r = sys2(SYS_fstat, fd, &st);
	if (r < 0) {
		say_ret(name, r);
		return;
	}
	put(name);
	put(": mode 0");
	char tmp[12];
	int i = 0;
	unsigned m = st.mode;
	do {
		tmp[i++] = '0' + (m & 7);
		m >>= 3;
	} while (m);
	while (i)
		put_char(tmp[--i]);
	put(", size ");
	put_dec(st.size);
	put(", nlink ");
	put_dec(st.nlink);
	end_line();
}

static i64 seals(i64 fd)
{
	return sys2(SYS_fcntl, fd, F_GET_SEALS_);
}

static i64 add_seals(i64 fd, i64 s)
{
	return sys3(SYS_fcntl, fd, F_ADD_SEALS_, s);
}

int main(void)
{
	lx_init();
	char name[300];

	/* Creation, flags and errors. */
	i64 fd = sys2(SYS_memfd_create, "gala-pool", MFD_CLOEXEC | MFD_ALLOW_SEALING);
	say_ret("memfd_create(CLOEXEC|ALLOW_SEALING)", fd);
	say_ret("F_GETFD", sys2(SYS_fcntl, fd, F_GETFD_));
	say_oct("F_GETFL", sys2(SYS_fcntl, fd, F_GETFL_));
	say_stat("fstat new", fd);
	say_ret("F_GET_SEALS new", seals(fd));
	i64 plain = sys2(SYS_memfd_create, "plain", 0);
	say_ret("memfd_create(0)", plain);
	say_ret("F_GETFD plain", sys2(SYS_fcntl, plain, F_GETFD_));
	say_ret("F_GET_SEALS plain", seals(plain));
	say_ret("F_ADD_SEALS plain SHRINK", add_seals(plain, SEAL_SHRINK));
	say_ret("close plain", sys1(SYS_close, plain));
	say_ret("memfd_create bad flags", sys2(SYS_memfd_create, "x", 0x100));
	lx_memset(name, 'n', sizeof name);
	name[249] = 0;
	i64 longest = sys2(SYS_memfd_create, name, 0);
	say_ret("memfd_create 249-byte name", longest);
	sys1(SYS_close, longest);
	name[249] = 'n';
	name[250] = 0;
	say_ret("memfd_create 250-byte name", sys2(SYS_memfd_create, name, 0));
	say_ret("memfd_create NULL name", sys2(SYS_memfd_create, 0, 0));
	say_ret("memfd_create same name again", sys2(SYS_memfd_create, "gala-pool", 0));
	sys1(SYS_close, 4);
	/* MFD_NOEXEC_SEAL (libwayland tries it first): mode 0666, F_SEAL_EXEC. */
	i64 nx = sys2(SYS_memfd_create, "noexec", MFD_CLOEXEC | MFD_NOEXEC_SEAL);
	say_ret("memfd_create(NOEXEC_SEAL)", nx);
	say_stat("fstat noexec", nx);
	say_ret("F_GET_SEALS noexec", seals(nx));
	say_ret("fchmod 0755 (SEAL_EXEC)", sys2(SYS_fchmod, nx, 0755));
	say_ret("fchmod 0644", sys2(SYS_fchmod, nx, 0644));
	say_ret("F_ADD_SEALS noexec SHRINK", add_seals(nx, SEAL_SHRINK));
	sys1(SYS_close, nx);
	say_ret("memfd_create(EXEC|NOEXEC_SEAL)",
		sys2(SYS_memfd_create, "x", MFD_EXEC | MFD_NOEXEC_SEAL));

	/* An empty file, then bytes through the fd. */
	say_ret("lseek END empty", sys3(SYS_lseek, fd, 0, SEEK_END_));
	char buf[64];
	say_ret("read empty", sys3(SYS_read, fd, buf, sizeof buf));
	say_ret("write", sys3(SYS_write, fd, "hello, memfd", 12));
	say_ret("lseek SET 0", sys3(SYS_lseek, fd, 0, SEEK_SET_));
	i64 r = sys3(SYS_read, fd, buf, sizeof buf);
	say_read("read back", r, buf);
	say_pread("pread at 7 is memfd", fd, 7, "memfd");
	say_ret("ftruncate 8192", sys2(SYS_ftruncate, fd, 2 * PAGE));
	say_stat("fstat after ftruncate", fd);
	say_ret("lseek END", sys3(SYS_lseek, fd, 0, SEEK_END_));
	say_ret("read at the end", sys3(SYS_read, fd, buf, sizeof buf));

	/* Shared mappings. */
	i64 m1 = map(2 * PAGE, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
	say_ok("mmap SHARED", m1);
	char *a = (char *)m1;
	say_same("map shows the bytes written", a, "hello, memfd");
	say_bool("map past them is zero", a[12] == 0 && a[PAGE + 5] == 0);
	lx_memcpy(a + PAGE, "from the map", 12);
	say_pread("pread sees a store to the map", fd, PAGE, "from the map");
	sys4(SYS_pwrite64, fd, "via pwrite", 10, 100);
	say_same("map sees pwrite", a + 100, "via pwrite");
	i64 m2 = map(2 * PAGE, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
	say_ok("second mmap SHARED", m2);
	char *b = (char *)m2;
	a[200] = 'x';
	say_bool("second map sees a store to the first", b[200] == 'x');
	i64 off = map(PAGE, PROT_READ, MAP_SHARED, fd, PAGE);
	say_ok("mmap SHARED at offset 4096", off);
	say_same("offset map shows the second page", (char *)off, "from the map");
	i64 priv = map(2 * PAGE, PROT_READ | PROT_WRITE, MAP_PRIVATE, fd, 0);
	say_ok("mmap PRIVATE", priv);
	char *p = (char *)priv;
	say_same("private map shows the file", p, "hello, memfd");
	lx_memcpy(p, "PRIVATE", 7);
	say_pread("file unchanged by a private store", fd, 0, "hello, memfd");
	say_same("shared map unchanged by a private store", a, "hello, memfd");
	say_ret("munmap private", sys2(SYS_munmap, priv, 2 * PAGE));

	/* A forked child: its copy of the mapping and a new one are shared. */
	i64 pid = lx_fork();
	if (pid == 0) {
		lx_memcpy(a + 300, "child was here", 14);
		i64 m = map(2 * PAGE, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
		if (!is_map(m))
			lx_exit(2);
		lx_memcpy((char *)m + 400, "child map", 9);
		lx_exit(0);
	}
	say_status("child", wait_for(pid, 0));
	say_same("parent sees the child's store", a + 300, "child was here");
	say_same("parent sees the child's own mapping", a + 400, "child map");
	say_pread("pread sees both", fd, 400, "child map");

	/* Grown: the file, then the mapping with mremap. */
	say_ret("ftruncate 16384", sys2(SYS_ftruncate, fd, 4 * PAGE));
	sys4(SYS_pwrite64, fd, "fourth page", 11, 3 * PAGE);
	i64 grown = sys6(SYS_mremap, m1, 2 * PAGE, 4 * PAGE, MREMAP_MAYMOVE, 0, 0);
	say_ok("mremap to 16384", grown);
	a = (char *)grown;
	say_same("grown map shows the fourth page", a + 3 * PAGE, "fourth page");
	say_same("grown map kept the first", a, "hello, memfd");

	/* Seals. */
	int pfd[2];
	sys2(SYS_pipe2, pfd, 0);
	say_ret("F_ADD_SEALS on a pipe's read end", add_seals(pfd[0], SEAL_SHRINK));
	say_ret("F_ADD_SEALS on a pipe's write end", add_seals(pfd[1], SEAL_SHRINK));
	say_ret("F_GET_SEALS on a pipe", seals(pfd[0]));
	sys1(SYS_close, pfd[0]);
	sys1(SYS_close, pfd[1]);
	say_ret("F_ADD_SEALS unknown bit", add_seals(fd, 0x100));
	say_ret("F_ADD_SEALS SHRINK", add_seals(fd, SEAL_SHRINK));
	say_ret("F_GET_SEALS", seals(fd));
	say_ret("ftruncate smaller (SHRINK)", sys2(SYS_ftruncate, fd, PAGE));
	say_ret("ftruncate larger (SHRINK)", sys2(SYS_ftruncate, fd, 5 * PAGE));
	say_ret("F_ADD_SEALS GROW", add_seals(fd, SEAL_GROW));
	say_ret("F_GET_SEALS", seals(fd));
	say_ret("ftruncate larger (GROW)", sys2(SYS_ftruncate, fd, 6 * PAGE));
	say_ret("ftruncate same size (GROW)", sys2(SYS_ftruncate, fd, 5 * PAGE));
	say_ret("pwrite past the end (GROW)", sys4(SYS_pwrite64, fd, "end", 3, 5 * PAGE));
	say_ret("pwrite inside (GROW)", sys4(SYS_pwrite64, fd, "in", 2, 5 * PAGE - 2));
	say_ret("lseek END", sys3(SYS_lseek, fd, 0, SEEK_END_));
	say_ret("write at the end (GROW)", sys3(SYS_write, fd, "end", 3));
	say_ret("F_ADD_SEALS WRITE while mapped writable", add_seals(fd, SEAL_WRITE));
	say_ret("munmap grown", sys2(SYS_munmap, grown, 4 * PAGE));
	say_ret("F_ADD_SEALS WRITE while mapped writable once more", add_seals(fd, SEAL_WRITE));
	say_ret("munmap second", sys2(SYS_munmap, m2, 2 * PAGE));
	say_ret("F_ADD_SEALS WRITE with a read-only shared map", add_seals(fd, SEAL_WRITE));
	say_ret("munmap read-only", sys2(SYS_munmap, off, PAGE));
	say_ret("F_ADD_SEALS WRITE", add_seals(fd, SEAL_WRITE));
	say_ret("F_GET_SEALS", seals(fd));
	say_ret("pwrite (WRITE)", sys4(SYS_pwrite64, fd, "no", 2, 0));
	say_ret("lseek SET 0", sys3(SYS_lseek, fd, 0, SEEK_SET_));
	say_ret("write (WRITE)", sys3(SYS_write, fd, "no", 2));
	say_ok("mmap SHARED writable (WRITE)",
		map(PAGE, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0));
	i64 ro = map(PAGE, PROT_READ, MAP_SHARED, fd, 0);
	say_ok("mmap SHARED read-only (WRITE)", ro);
	say_ret("mprotect it writable (WRITE)", sys3(SYS_mprotect, ro, PAGE, PROT_READ | PROT_WRITE));
	i64 cow = map(PAGE, PROT_READ | PROT_WRITE, MAP_PRIVATE, fd, 0);
	say_ok("mmap PRIVATE writable (WRITE)", cow);
	if (is_map(cow))
		lx_memcpy((char *)cow, "copy", 4);
	say_pread("file unchanged (WRITE)", fd, 0, "hello, memfd");
	say_ret("ftruncate same size (WRITE)", sys2(SYS_ftruncate, fd, 5 * PAGE));
	say_ret("F_ADD_SEALS SEAL", add_seals(fd, SEAL_SEAL));
	say_ret("F_GET_SEALS", seals(fd));
	say_ret("F_ADD_SEALS after SEAL", add_seals(fd, SEAL_GROW));
	say_ret("F_ADD_SEALS nothing after SEAL", add_seals(fd, 0));

	/* The mapping outlives the fd. */
	say_ret("close", sys1(SYS_close, fd));
	say_same("read-only map after close", (char *)ro, "hello, memfd");
	say_same("private map after close", (char *)cow, "copy");
	say_text("memfd_test: ok");
	return 0;
}
