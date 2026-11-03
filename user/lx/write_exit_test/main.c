/* write_exit_test: write(2), writev(2) and exit_group(2) as the kernel does
 * them: counts, partial writes, errors, and the exit status (300 & 0xff:
 * the runner sees 44).
 *
 * A partial write is made deterministic two ways: a non-blocking write
 * larger than an empty pipe's capacity (64 KiB by default) writes what
 * fits; a write whose buffer runs into an unmapped page writes the bytes
 * before it. */
#include "../common/lx.h"

#define O_NONBLOCK 04000
#define F_GETPIPE_SZ 1032
#define PROT_READ 1
#define PROT_WRITE 2
#define MAP_PRIVATE 2
#define MAP_ANONYMOUS 0x20

struct iovec {
	const void *base;
	u64 len;
};

static char big[70000];

int main(void)
{
	int fds[2];
	i64 r;

	say("write(1, \"\", 0)", sys3(NR_write, 1, "", 0));
	say("write(-1)", sys3(NR_write, -1, "x", 1));
	say("write(1000)", sys3(NR_write, 1000, "x", 1));
	say("write(1, NULL, 1)", sys3(NR_write, 1, 0, 1));
	say("write(1, bad pointer, 1)", sys3(NR_write, 1, 0x10, 1));
	say("write(1, kernel address, 1)", sys3(NR_write, 1, 0xffff800000000000UL, 1));

	r = sys2(NR_pipe2, fds, O_NONBLOCK);
	say("pipe2(O_NONBLOCK)", r);
	say("pipe capacity", sys2(NR_fcntl, fds[1], F_GETPIPE_SZ));
	say("write(read end)", sys3(NR_write, fds[0], "x", 1));
	for (unsigned i = 0; i < sizeof big; i++)
		big[i] = (char)i;
	say("write(pipe, 70000) partial", sys3(NR_write, fds[1], big, sizeof big));
	say("write(full pipe)", sys3(NR_write, fds[1], big, 1));
	{
		/* Drain it, and check the bytes came through in order. */
		static char back[65536];
		i64 got = sys3(NR_read, fds[0], back, sizeof back);
		int same = got == 65536;
		for (i64 i = 0; same && i < got; i++)
			same = back[i] == big[i];
		say("read(pipe) back", got);
		say_bool("bytes in order", same);
		say("read(empty pipe)", sys3(NR_read, fds[0], back, 1));
	}

	/* A buffer whose second page is unmapped: the first page's bytes are
	 * written, then the write stops. */
	{
		char *two = (char *)sys6(NR_mmap, 0, 8192, PROT_READ | PROT_WRITE,
					 MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
		for (int i = 0; i < 8192; i++)
			two[i] = 'a';
		sys2(NR_munmap, two + 4096, 4096);
		say("write(pipe, 100 bytes before a hole)",
		    sys3(NR_write, fds[1], two + 4096 - 100, 200));
		say("write(pipe, starts in the hole)", sys3(NR_write, fds[1], two + 4096, 10));
		static char back[256];
		say("read(pipe) back", sys3(NR_read, fds[0], back, sizeof back));
	}

	{
		struct iovec iov[3] = {{"wri", 3}, {"", 0}, {"tev\n", 4}};
		say("writev(1, 3 pieces)", sys3(NR_writev, 1, iov, 3));
		say("writev(1, iovcnt -1)", sys3(NR_writev, 1, iov, -1));
		say("writev(1, iovcnt 1025)", sys3(NR_writev, 1, iov, 1025));
		say("writev(1, iovcnt 0)", sys3(NR_writev, 1, iov, 0));
	}

	say("close(read end)", sys1(NR_close, fds[0]));
	say("close(read end) again", sys1(NR_close, fds[0]));
	say_text("write_exit_test: ok, exit_group(300)");
	exit_group(300);
}
