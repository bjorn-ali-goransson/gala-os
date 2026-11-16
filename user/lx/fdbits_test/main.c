/* fdbits_test: a descriptor argument's upper 32 bits are not looked at.
 * Linux's system calls take descriptors as int or unsigned int, and the
 * x86-64 C ABI leaves an int argument's upper register half undefined:
 * code that passes an int fd may leave garbage there (Ubuntu 26.04's
 * glycin's ioctl(FIONBIO) on a socket did: gala answered EBADF, ROADMAP
 * LD). Each call gets its descriptor (and dup2's new one, openat's dirfd)
 * with 0xdeadbeef in the upper half, and works as with the plain value. */
#include "../common/lx.h"
#include "../common/more.h"

#define HI 0xdeadbeef00000000ull
#define AT_FDCWD (-100)
#define FIONBIO 0x5421
#define F_GETFD 1
#define F_GETFL 3

static i64 g(i64 fd)
{
	return (i64)(HI | (u64)(unsigned)fd);
}

int main(void)
{
	lx_init();
	int p[2];
	sys2(SYS_pipe2, p, 0);
	say_ret("write", sys3(SYS_write, g(p[1]), "abc", 3));
	char buf[4];
	say_ret("read", sys3(SYS_read, g(p[0]), buf, 3));
	say_ret("fcntl(F_GETFD)", sys2(SYS_fcntl, g(p[0]), F_GETFD));
	int on = 1;
	say_ret("ioctl(FIONBIO)", sys3(SYS_ioctl, g(p[0]), FIONBIO, &on));
	say_bool("  O_NONBLOCK set", (sys2(SYS_fcntl, p[0], F_GETFL) & 04000) != 0);
	char st[144];
	say_ret("fstat", sys2(SYS_fstat, g(p[0]), st));
	i64 d = sys1(SYS_dup, g(p[0]));
	say_bool("dup: a new descriptor", d > p[1]);
	say_ret("dup2(to 20)", sys2(SYS_dup2, g(p[0]), g(20)));
	say_ret("close(20)", sys1(SYS_close, g(20)));
	say_ret("close(20) again", sys1(SYS_close, 20));
	i64 f = sys4(SYS_openat, g(AT_FDCWD), "fdbits.tmp", 0102 /* O_RDWR|O_CREAT */, 0600);
	say_bool("openat(AT_FDCWD)", f >= 0);
	say_ret("lseek", sys3(SYS_lseek, g(f), 0, 2 /* SEEK_END */));
	say_ret("unlinkat(AT_FDCWD)", sys3(SYS_unlinkat, g(AT_FDCWD), "fdbits.tmp", 0));
	i64 ep = sys1(SYS_epoll_create1, 0);
	struct { unsigned events; u64 data; } LX_EPOLL_PACKED ev = { 1, 0 };
	say_ret("epoll_ctl(both descriptors)", sys4(SYS_epoll_ctl, g(ep), 1, g(p[0]), &ev));
	say_ret("close", sys1(SYS_close, g(d)));
	say_ret("close(the same) again", sys1(SYS_close, d));
	say_text("fdbits_test: ok");
	return 0;
}
