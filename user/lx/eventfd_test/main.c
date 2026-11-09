/* eventfd_test: eventfd(2) and eventfd2(2), the counter GLib's main loop
 * wakes its threads with (ROADMAP LD step 3: GLib fell back to a pipe in
 * gala, where it was ENOSYS). From eventfd(2): read gives the counter
 * and zeroes it (EFD_SEMAPHORE: gives 1 and takes 1), blocks while it is
 * 0 (EAGAIN with EFD_NONBLOCK); write adds, blocks while the sum would
 * pass 0xfffffffffffffffe (EAGAIN non-blocking), refuses that value
 * plus one; buffers under 8 bytes are EINVAL. poll: readable while the
 * counter is above 0, writable while 1 can be added. A blocked read
 * wakes when another process (a forked child sharing the description)
 * writes, as does an epoll wait. */
#include "../common/lx.h"
#include "../common/more.h"

#define EFD_SEMAPHORE 1
#define EFD_CLOEXEC 02000000
#define EFD_NONBLOCK 04000
#define POLLIN 1
#define POLLOUT 4

static i64 rd(int fd, u64 *v)
{
	*v = 0x5555;
	return sys3(SYS_read, fd, v, 8);
}

static i64 wr(int fd, u64 v)
{
	return sys3(SYS_write, fd, &v, 8);
}

static void say_read_value(const char *label, int fd)
{
	u64 v;
	i64 r = rd(fd, &v);
	put(label);
	put(" = ");
	put_ret(r);
	if (r == 8) {
		put(", value ");
		put_dec((i64)v);
	}
	end_line();
}

static i64 poll_one(int fd, int timeout)
{
	struct {
		int fd;
		short events, revents;
	} p = { fd, POLLIN | POLLOUT, 0 };
	i64 r = sys3(SYS_poll, &p, 1, timeout);
	return r < 0 ? r : p.revents;
}

int main(void)
{
	lx_init();
	say_text("-- counter");
	i64 fd = sys2(SYS_eventfd2, 3, EFD_NONBLOCK);
	say("eventfd2(3, NONBLOCK)", fd);
	say_read_value("read", (int)fd);
	say_read_value("read again", (int)fd);
	say("write 5", wr((int)fd, 5));
	say("write 7", wr((int)fd, 7));
	say("poll after writes (IN|OUT)", poll_one((int)fd, 0));
	say_read_value("read", (int)fd);
	say("poll when 0 (OUT)", poll_one((int)fd, 0));
	u64 small = 0;
	say_ret("read of 4 bytes", sys3(SYS_read, fd, &small, 4));
	say_ret("write of 4 bytes", sys3(SYS_write, fd, &small, 4));
	say_ret("read into a bad pointer", sys3(SYS_read, fd, 1, 8));
	say_ret("write from a bad pointer", sys3(SYS_write, fd, 1, 8));
	/* A read of 16 bytes gives 8. */
	u64 two[2] = { 0, 0 };
	wr((int)fd, 9);
	say_ret("read of 16 bytes", sys3(SYS_read, fd, two, 16));
	say("its value", (i64)two[0]);

	say_text("-- the limit");
	say_ret("write 0xffffffffffffffff", wr((int)fd, ~0UL));
	say_ret("write 0xfffffffffffffffe", wr((int)fd, ~0UL - 1));
	say("poll at the limit (IN)", poll_one((int)fd, 0));
	say_ret("write 1 more (would block)", wr((int)fd, 1));
	say_ret("write 0 at the limit", wr((int)fd, 0));
	u64 v;
	rd((int)fd, &v);
	say_bool("read the limit back", v == ~0UL - 1);
	sys1(SYS_close, fd);

	say_text("-- semaphore");
	fd = sys2(SYS_eventfd2, 2, EFD_SEMAPHORE | EFD_NONBLOCK);
	say("eventfd2(2, SEMAPHORE|NONBLOCK)", fd);
	say_read_value("read", (int)fd);
	say_read_value("read", (int)fd);
	say_read_value("read", (int)fd);
	sys1(SYS_close, fd);

	say_text("-- flags");
	say_ret("eventfd2 with an unknown flag", sys2(SYS_eventfd2, 0, 0x10));
	fd = sys2(SYS_eventfd2, 0, EFD_CLOEXEC);
	say("eventfd2(0, CLOEXEC)", fd);
	say("F_GETFD", sys2(SYS_fcntl, fd, 1));
	say_oct("F_GETFL (O_RDWR)", (u64)sys2(SYS_fcntl, fd, 3) & 3);
	sys1(SYS_close, fd);
	fd = sys1(SYS_eventfd, 4);
	say("eventfd(4)", fd);
	say("F_GETFD", sys2(SYS_fcntl, fd, 1));
	say_oct("F_GETFL has O_NONBLOCK", (u64)sys2(SYS_fcntl, fd, 3) & 04000);
	say_read_value("read", (int)fd);
	sys1(SYS_close, fd);

	say_text("-- blocking");
	fd = sys2(SYS_eventfd2, 0, 0);
	i64 pid = lx_fork();
	if (pid == 0) {
		sleep_ms(50);
		wr((int)fd, 42);
		exit_group(0);
	}
	say_read_value("read waits for the child's write", (int)fd);
	say_status("child", wait_for(pid, 0));
	/* epoll: an eventfd is readable once written. */
	i64 ep = sys1(SYS_epoll_create1, 0);
	struct {
		unsigned events;
		u64 data;
	} __attribute__((packed)) ev = { POLLIN, 77 }, got = { 0, 0 };
	say_ret("epoll_ctl ADD", sys4(SYS_epoll_ctl, ep, 1, fd, &ev));
	say("epoll_wait before a write", sys4(SYS_epoll_wait, ep, &got, 1, 0));
	pid = lx_fork();
	if (pid == 0) {
		sleep_ms(50);
		wr((int)fd, 1);
		exit_group(0);
	}
	i64 n = sys4(SYS_epoll_wait, ep, &got, 1, 5000);
	say("epoll_wait after the child's write", n);
	say("its data", (i64)got.data);
	say_hex("its events", got.events);
	say_status("child", wait_for(pid, 0));
	say_read_value("read", (int)fd);
	say_text("eventfd_test: ok");
	return 0;
}
