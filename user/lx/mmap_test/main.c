/* mmap_test: anonymous mmap(2), mprotect(2) and munmap(2) as the raw
 * system calls give them: results, errors, protections, MAP_FIXED; then
 * file mappings as glibc's ld.so and programs make them (ROADMAP LD):
 * MAP_PRIVATE (copy-on-write) and MAP_SHARED views of a file at an
 * offset, MAP_FIXED over part of a mapping, mprotect and munmap of part
 * of one, the errors; mremap(2) and madvise(2).
 *
 * Accessibility is probed without signals: write(2) from an address fails
 * with EFAULT when it is not readable, read(2) into it when it is not
 * writable. Execute-only mappings are not checked (with protection keys
 * a PROT_EXEC page may be unreadable, depending on the CPU). */
#include "../common/lx.h"
#include "../common/more.h"

#define PROT_NONE 0
#define PROT_READ 1
#define PROT_WRITE 2
#define MAP_SHARED 1
#define MAP_PRIVATE 2
#define MAP_FIXED 0x10
#define MAP_ANONYMOUS 0x20
#define MAP_FIXED_NOREPLACE 0x100000
#define PAGE 4096UL
#define HINT 0x200000000UL /* 8 GiB: free in both the static and the static-pie layout */

static int fds[2];

static int readable(const char *p)
{
	i64 r = sys3(NR_write, fds[1], p, 1);
	if (r == 1) {
		char c;
		sys3(NR_read, fds[0], &c, 1);
	}
	return r == 1;
}

static int writable(char *p)
{
	sys3(NR_write, fds[1], "w", 1);
	i64 r = sys3(NR_read, fds[0], p, 1);
	if (r != 1) {
		char c;
		sys3(NR_read, fds[0], &c, 1);
	}
	return r == 1;
}

static void access_line(const char *name, char *p)
{
	put(name);
	put(": ");
	put(readable(p) ? "r" : "-");
	put(writable(p) ? "w" : "-");
	end_line();
}

static i64 map(u64 addr, u64 len, int prot, int flags)
{
	return sys6(NR_mmap, addr, len, prot, flags, -1, 0);
}

#define O_RDONLY 0
#define O_WRONLY 1
#define O_RDWR 2
#define O_CREAT 0100
#define O_TRUNC 01000
#define O_DIRECTORY 0200000
#define MREMAP_MAYMOVE 1
#define MREMAP_FIXED 2
#define MADV_DONTNEED 4
#define MADV_WILLNEED 3
#define FILE_NAME "mmap_test.file"

static i64 fmap(u64 addr, u64 len, int prot, int flags, int fd, u64 offset)
{
	return sys6(NR_mmap, addr, len, prot, flags, fd, offset);
}

/* The byte the test file holds at `offset`: each page's own letter, then
 * its offset's low bits. */
static char file_byte(u64 offset)
{
	return offset % PAGE == 0 ? (char)('A' + offset / PAGE) : (char)(offset & 0x7f);
}

/* The byte at `offset` read back from the file with pread. */
static int file_at(int fd, u64 offset)
{
	char c = 0;
	i64 r = sys4(SYS_pread64, fd, &c, 1, offset);
	return r == 1 ? c : (int)r;
}

/* A file of three and a half pages, opened read/write. */
static int make_file(void)
{
	int fd = (int)sys4(SYS_openat, -100, FILE_NAME, O_RDWR | O_CREAT | O_TRUNC, 0644);
	static char buf[3 * PAGE + PAGE / 2];
	for (u64 i = 0; i < sizeof buf; i++)
		buf[i] = file_byte(i);
	sys3(NR_write, fd, buf, sizeof buf);
	return fd;
}

static void file_mappings(void)
{
	int fd = make_file();
	say_bool("file made", fd >= 0);

	/* MAP_PRIVATE of the whole file: its bytes, then zeros to the end
	 * of the last page. */
	i64 p = fmap(0, 4 * PAGE, PROT_READ, MAP_PRIVATE, fd, 0);
	char *c = (char *)p;
	say_bool("MAP_PRIVATE of the file", p > 0 && p % PAGE == 0);
	say("page 1's first byte", c[0]);
	say("page 3's first byte", c[2 * PAGE]);
	say("byte 4097", c[PAGE + 1]);
	say("the last byte of the file", c[3 * PAGE + PAGE / 2 - 1]);
	say("past the end, in its last page", c[3 * PAGE + PAGE / 2]);
	access_line("read-only private page", c);
	say("mprotect(rw)", sys3(NR_mprotect, p, 2 * PAGE, PROT_READ | PROT_WRITE));
	c[0] = 'x';
	c[PAGE + 1] = 'y';
	say("written page 1", c[0]);
	say("the file keeps its byte 0", file_at(fd, 0));
	say("the file keeps its byte 4097", file_at(fd, PAGE + 1));
	/* A page never written shows the file as it changes. */
	char z = 'Z';
	sys4(SYS_pwrite64, fd, &z, 1, 2 * PAGE);
	say("an unwritten private page sees a pwrite", c[2 * PAGE]);
	z = 'C';
	sys4(SYS_pwrite64, fd, &z, 1, 2 * PAGE);
	/* A written one does not. */
	z = 'Q';
	sys4(SYS_pwrite64, fd, &z, 1, 0);
	say("a written private page keeps its copy", c[0]);
	z = 'A';
	sys4(SYS_pwrite64, fd, &z, 1, 0);

	/* MAP_FIXED of the file at an offset over the mapping's second page,
	 * then munmap of its third: the rest stays. */
	i64 f = fmap(p + PAGE, PAGE, PROT_READ, MAP_PRIVATE | MAP_FIXED, fd, 3 * PAGE);
	say_bool("MAP_FIXED of page 4 over page 2", f == p + (i64)PAGE);
	say("page 2 now shows page 4's first byte", c[PAGE]);
	say("page 1 kept its copy", c[0]);
	say("munmap(page 3)", sys2(NR_munmap, p + 2 * PAGE, PAGE));
	access_line("page 3 after munmap", c + 2 * PAGE);
	say("page 4 stays", c[3 * PAGE]);
	say("munmap(all)", sys2(NR_munmap, p, 4 * PAGE));

	/* MAP_SHARED: writes reach the file, pwrites the mapping. */
	i64 s = fmap(0, 2 * PAGE, PROT_READ | PROT_WRITE, MAP_SHARED, fd, PAGE);
	char *sc = (char *)s;
	say_bool("MAP_SHARED at offset 4096", s > 0);
	say("its first byte is page 2's", sc[0]);
	sc[5] = 's';
	say("the file sees the store", file_at(fd, PAGE + 5));
	z = 'P';
	sys4(SYS_pwrite64, fd, &z, 1, 2 * PAGE + 7);
	say("the mapping sees a pwrite", sc[PAGE + 7]);
	say("mprotect(read-only)", sys3(NR_mprotect, s, PAGE, PROT_READ));
	access_line("shared page made read-only", sc);
	say("munmap(shared)", sys2(NR_munmap, s, 2 * PAGE));
	/* A mapping lives on after its fd is closed. */
	i64 k = fmap(0, PAGE, PROT_READ, MAP_PRIVATE, fd, 0);

	/* The errors. */
	int ro = (int)sys4(SYS_openat, -100, FILE_NAME, O_RDONLY, 0);
	int wo = (int)sys4(SYS_openat, -100, FILE_NAME, O_WRONLY, 0);
	int dir = (int)sys4(SYS_openat, -100, ".", O_RDONLY | O_DIRECTORY, 0);
	int pipe_fds[2];
	sys2(NR_pipe2, pipe_fds, 0);
	say_ret("mmap(MAP_SHARED, PROT_WRITE, O_RDONLY)",
		fmap(0, PAGE, PROT_READ | PROT_WRITE, MAP_SHARED, ro, 0));
	i64 r = fmap(0, PAGE, PROT_READ, MAP_SHARED, ro, 0);
	say_bool("mmap(MAP_SHARED, PROT_READ, O_RDONLY) succeeded", r > 0);
	say_ret("mprotect(PROT_WRITE) of it", sys3(NR_mprotect, r, PAGE, PROT_READ | PROT_WRITE));
	sys2(NR_munmap, r, PAGE);
	r = fmap(0, PAGE, PROT_READ | PROT_WRITE, MAP_PRIVATE, ro, 0);
	say_bool("mmap(MAP_PRIVATE, PROT_WRITE, O_RDONLY) succeeded", r > 0);
	sys2(NR_munmap, r, PAGE);
	say_ret("mmap(O_WRONLY)", fmap(0, PAGE, PROT_READ, MAP_PRIVATE, wo, 0));
	say_ret("mmap(a directory)", fmap(0, PAGE, PROT_READ, MAP_PRIVATE, dir, 0));
	say_ret("mmap(a pipe)", fmap(0, PAGE, PROT_READ, MAP_PRIVATE, pipe_fds[0], 0));
	say_ret("mmap(a closed fd)", fmap(0, PAGE, PROT_READ, MAP_PRIVATE, 1000, 0));
	say_ret("mmap(offset not page aligned)", fmap(0, PAGE, PROT_READ, MAP_PRIVATE, fd, 100));
	sys1(NR_close, ro);
	sys1(NR_close, wo);
	sys1(NR_close, dir);
	sys1(NR_close, pipe_fds[0]);
	sys1(NR_close, pipe_fds[1]);
	sys1(NR_close, fd);
	sys1(SYS_unlink, FILE_NAME);
	say("a mapping outlives its fd and name", ((char *)k)[0]);
	sys2(NR_munmap, k, PAGE);
}

static void remaps(void)
{
	/* Anonymous memory grows in place when the pages after it are free,
	 * else moves with its contents (MREMAP_MAYMOVE). */
	i64 a = map(HINT, PAGE, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS);
	((char *)a)[1] = 11;
	i64 g = sys4(SYS_mremap, a, PAGE, 3 * PAGE, 0);
	say_bool("mremap grows in place", g == a);
	((char *)a)[2 * PAGE + 1] = 12;
	access_line("its new last page", (char *)a + 2 * PAGE);
	i64 b = map(HINT + 3 * PAGE, PAGE, PROT_READ, MAP_PRIVATE | MAP_ANONYMOUS | MAP_FIXED);
	say_bool("a page after it", b == a + 3 * (i64)PAGE);
	say_ret("mremap past it without MREMAP_MAYMOVE", sys4(SYS_mremap, a, 3 * PAGE, 4 * PAGE, 0));
	i64 m = sys4(SYS_mremap, a, 3 * PAGE, 8 * PAGE, MREMAP_MAYMOVE);
	say_bool("mremap(MREMAP_MAYMOVE) moved it", m > 0 && m != a);
	say("moved: its first page's byte", ((char *)m)[1]);
	say("moved: its third page's byte", ((char *)m)[2 * PAGE + 1]);
	say("moved: a new page is zero", ((char *)m)[5 * PAGE]);
	access_line("the old place", (char *)a);
	i64 sh = sys4(SYS_mremap, m, 8 * PAGE, 2 * PAGE, 0);
	say_bool("mremap shrinks in place", sh == m);
	access_line("past the shrunk end", (char *)m + 2 * PAGE);
	i64 fx = sys5(SYS_mremap, m, 2 * PAGE, 2 * PAGE, MREMAP_MAYMOVE | MREMAP_FIXED, HINT);
	say_bool("mremap(MREMAP_FIXED) to a given address", fx == (i64)HINT);
	say("its byte there", ((char *)HINT)[1]);
	say_ret("mremap(MREMAP_FIXED without MREMAP_MAYMOVE)",
		sys5(SYS_mremap, HINT, PAGE, PAGE, MREMAP_FIXED, HINT + 16 * PAGE));
	say_ret("mremap(unaligned)", sys4(SYS_mremap, HINT + 1, PAGE, PAGE, 0));
	say_ret("mremap(new length 0)", sys4(SYS_mremap, HINT, PAGE, 0, 0));
	say_ret("mremap(bad flags)", sys4(SYS_mremap, HINT, PAGE, PAGE, 0x100));
	say_ret("mremap(not mapped)", sys4(SYS_mremap, HINT + 64 * PAGE, PAGE, 2 * PAGE, MREMAP_MAYMOVE));
	sys2(NR_munmap, HINT, 4 * PAGE);

	/* A file mapping grows with the file's next pages. */
	int fd = make_file();
	i64 f = fmap(HINT, PAGE, PROT_READ, MAP_PRIVATE, fd, PAGE);
	i64 fg = sys4(SYS_mremap, f, PAGE, 2 * PAGE, MREMAP_MAYMOVE);
	say_bool("a file mapping grown", fg > 0);
	say("its second page shows the file's third", ((char *)fg)[PAGE]);
	i64 fb = map(fg + 2 * PAGE, PAGE, PROT_READ, MAP_PRIVATE | MAP_ANONYMOUS | MAP_FIXED);
	i64 fm = sys4(SYS_mremap, fg, 2 * PAGE, 3 * PAGE, MREMAP_MAYMOVE);
	say_bool("a file mapping moved", fm > 0 && fm != fg);
	say("moved: its pages show the file", ((char *)fm)[0] + ((char *)fm)[2 * PAGE] * 256);
	sys2(NR_munmap, fm, 3 * PAGE);
	sys2(NR_munmap, fb, PAGE);
	sys1(NR_close, fd);
	sys1(SYS_unlink, FILE_NAME);
}

static void advice(void)
{
	i64 a = map(0, 2 * PAGE, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS);
	((char *)a)[0] = 5;
	((char *)a)[PAGE] = 6;
	say("madvise(MADV_DONTNEED, page 1)", sys3(SYS_madvise, a, PAGE, MADV_DONTNEED));
	say("anonymous page after MADV_DONTNEED", ((char *)a)[0]);
	say("the other page", ((char *)a)[PAGE]);
	say("madvise(MADV_WILLNEED)", sys3(SYS_madvise, a, 2 * PAGE, MADV_WILLNEED));
	say_ret("madvise(bad advice)", sys3(SYS_madvise, a, PAGE, 9999));
	say_ret("madvise(unaligned)", sys3(SYS_madvise, a + 1, PAGE, MADV_DONTNEED));
	sys2(NR_munmap, a, 2 * PAGE);
	say_ret("madvise(not mapped)", sys3(SYS_madvise, a, PAGE, MADV_DONTNEED));

	int fd = make_file();
	i64 f = fmap(0, PAGE, PROT_READ | PROT_WRITE, MAP_PRIVATE, fd, 0);
	((char *)f)[0] = 'w';
	say("a private file page written", ((char *)f)[0]);
	say("madvise(MADV_DONTNEED) of it", sys3(SYS_madvise, f, PAGE, MADV_DONTNEED));
	say("shows the file again", ((char *)f)[0]);
	sys2(NR_munmap, f, PAGE);
	sys1(NR_close, fd);
	sys1(SYS_unlink, FILE_NAME);
}

int main(void)
{
	sys2(NR_pipe2, fds, 0);

	i64 a = map(0, 3 * PAGE, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS);
	say_bool("mmap(3 pages) succeeded", a > 0);
	say_bool("page aligned", a % PAGE == 0);
	{
		char *p = (char *)a;
		int zero = 1;
		for (u64 i = 0; i < 3 * PAGE; i++)
			zero &= p[i] == 0;
		say_bool("zero filled", zero);
		p[0] = 1;
		p[3 * PAGE - 1] = 2;
		access_line("rw page", p);
	}

	say("mprotect(page 2, PROT_READ)", sys3(NR_mprotect, a + PAGE, PAGE, PROT_READ));
	access_line("page 1", (char *)a);
	access_line("page 2", (char *)a + PAGE);
	say("mprotect(page 3, PROT_NONE)", sys3(NR_mprotect, a + 2 * PAGE, PAGE, PROT_NONE));
	access_line("page 3", (char *)a + 2 * PAGE);
	say("mprotect(page 3, rw)", sys3(NR_mprotect, a + 2 * PAGE, PAGE, PROT_READ | PROT_WRITE));
	say("page 3 kept its data", ((char *)a)[3 * PAGE - 1]);
	say("mprotect(unaligned)", sys3(NR_mprotect, a + 1, PAGE, PROT_READ));
	say("mprotect(bad prot)", sys3(NR_mprotect, a, PAGE, 0x100));

	say("munmap(page 2)", sys2(NR_munmap, a + PAGE, PAGE));
	access_line("page 2 after munmap", (char *)a + PAGE);
	access_line("page 1 after munmap of 2", (char *)a);
	say("munmap(page 2) again", sys2(NR_munmap, a + PAGE, PAGE));
	say("munmap(unaligned)", sys2(NR_munmap, a + 1, PAGE));
	say("munmap(length 0)", sys2(NR_munmap, a, 0));
	say("munmap(unaligned length)", sys2(NR_munmap, a + 2 * PAGE, 100));
	access_line("page 3 after munmap(len 100)", (char *)a + 2 * PAGE);

	/* A hint is used when it is free; MAP_FIXED replaces what is there. */
	i64 h = map(HINT, 2 * PAGE, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS);
	say_bool("hint used", h == (i64)HINT);
	((char *)h)[10] = 7;
	((char *)h)[PAGE + 10] = 8;
	i64 f = map(HINT + PAGE, PAGE, PROT_READ, MAP_PRIVATE | MAP_ANONYMOUS | MAP_FIXED);
	say_bool("MAP_FIXED at the hint's second page", f == (i64)(HINT + PAGE));
	say("first page kept", ((char *)h)[10]);
	say("second page replaced (zero)", ((char *)h)[PAGE + 10]);
	access_line("replaced page", (char *)h + PAGE);
	say("MAP_FIXED_NOREPLACE over it",
	    map(HINT, PAGE, PROT_READ, MAP_PRIVATE | MAP_ANONYMOUS | MAP_FIXED_NOREPLACE));
	i64 nr = map(HINT + 2 * PAGE, PAGE, PROT_READ, MAP_PRIVATE | MAP_ANONYMOUS | MAP_FIXED_NOREPLACE);
	say_bool("MAP_FIXED_NOREPLACE next to it", nr == (i64)(HINT + 2 * PAGE));
	say("munmap(the whole range, 3 pages)", sys2(NR_munmap, HINT, 3 * PAGE));
	access_line("hint after munmap", (char *)HINT);

	/* A hint that is not page aligned is rounded (not MAP_FIXED). */
	i64 u = map(HINT + 123, PAGE, PROT_READ, MAP_PRIVATE | MAP_ANONYMOUS);
	say_bool("unaligned hint rounded down", u == (i64)HINT);
	sys2(NR_munmap, u, PAGE);

	/* Unaligned length: rounded up to a page. */
	i64 l = map(0, 100, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS);
	access_line("length 100, last byte of its page", (char *)l + PAGE - 1);
	say("munmap(length 100)", sys2(NR_munmap, l, 100));

	say("mmap(length 0)", map(0, 0, PROT_READ, MAP_PRIVATE | MAP_ANONYMOUS));
	say("mmap(MAP_FIXED, unaligned)", map(HINT + 1, PAGE, PROT_READ, MAP_PRIVATE | MAP_ANONYMOUS | MAP_FIXED));
	say("mmap(neither shared nor private)", map(0, PAGE, PROT_READ, MAP_ANONYMOUS));
	say("mmap(no MAP_ANONYMOUS, fd -1)", sys6(NR_mmap, 0, PAGE, PROT_READ, MAP_PRIVATE, -1, 0));
	say("mmap(anonymous, offset 1)", sys6(NR_mmap, 0, PAGE, PROT_READ, MAP_PRIVATE | MAP_ANONYMOUS, -1, 1));
	say("mmap(huge length)", map(0, 1UL << 62, PROT_READ, MAP_PRIVATE | MAP_ANONYMOUS));
	say("mmap(MAP_FIXED, kernel address)",
	    map(0xffff800000000000UL, PAGE, PROT_READ, MAP_PRIVATE | MAP_ANONYMOUS | MAP_FIXED));
	{
		i64 s = map(0, PAGE, PROT_READ | PROT_WRITE, MAP_SHARED | MAP_ANONYMOUS);
		say_bool("MAP_SHARED|MAP_ANONYMOUS succeeded", s > 0 && s % PAGE == 0);
		access_line("shared page", (char *)s);
		say("munmap(shared)", sys2(NR_munmap, s, PAGE));
	}

	file_mappings();
	remaps();
	advice();

	say_text("mmap_test: ok");
	exit_group(0);
}
