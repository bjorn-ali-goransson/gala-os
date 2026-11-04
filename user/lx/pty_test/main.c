/* pty_test: pseudo-terminals as the raw system calls give them: /dev/ptmx,
 * TIOCGPTN/TIOCSPTLCK (what posix_openpt/grantpt/unlockpt/ptsname do),
 * a fresh pty's termios (TCGETS: the flags and c_cc defaults the lx_abi
 * crate's termios module is checked against), the line discipline:
 * canonical mode (line buffering, ERASE, KILL, WERASE, EOF, ICRNL),
 * echo (ECHO, ECHOE, ECHOCTL) on and off, output processing (OPOST|
 * ONLCR), raw mode (cfmakeraw's flags: byte by byte, VMIN/VTIME), window
 * sizes (TIOCGWINSZ/TIOCSWINSZ, SIGWINCH), and ISIG: ^C, ^\ and ^Z typed
 * at the master reach the foreground process group of a child that made
 * the pty its controlling terminal (^Z is discarded: that group is
 * orphaned).
 *
 * The line discipline runs asynchronously, so echoes are read until
 * what is expected has arrived or a quiet period passes; nothing timed
 * is printed. If /dev/ptmx cannot be opened, one SKIP line is printed. */
#define _GNU_SOURCE
#include "../common/lx.h"
#include "../common/more.h"
#include <fcntl.h>
#include <signal.h>
#include <sys/stat.h>
#include <sys/wait.h>

#define TCGETS 0x5401
#define TCSETS 0x5402
#define TCSETSF 0x5404
#define TIOCSCTTY 0x540E
#define TIOCGPGRP 0x540F
#define TIOCGWINSZ 0x5413
#define TIOCSWINSZ 0x5414
#define FIONREAD 0x541B
#define TIOCGSID 0x5429
#define TIOCGPTN 0x80045430
#define TIOCSPTLCK 0x40045431

/* Linux's struct termios (TCGETS/TCSETS; not musl's larger one). */
struct kterm {
	unsigned iflag, oflag, cflag, lflag;
	unsigned char line;
	unsigned char cc[19];
};

struct winsz {
	unsigned short row, col, xpixel, ypixel;
};

/* c_iflag, c_oflag, c_lflag, c_cflag bits used here (termios(3)). */
#define IGNBRK_ 0000001
#define BRKINT_ 0000002
#define PARMRK_ 0000010
#define ISTRIP_ 0000040
#define INLCR_ 0000100
#define IGNCR_ 0000200
#define ICRNL_ 0000400
#define IXON_ 0002000
#define OPOST_ 0000001
#define CSIZE_ 0000060
#define CS8_ 0000060
#define PARENB_ 0000400
#define ISIG_ 0000001
#define ICANON_ 0000002
#define ECHO_ 0000010
#define ECHONL_ 0000100
#define IEXTEN_ 0100000
#define VTIME_ 5
#define VMIN_ 6

static int master = -1, slave = -1;

static void say_term(const char *name, struct kterm *t)
{
	put(name);
	put(": iflag ");
	put_hex(t->iflag);
	put(", oflag ");
	put_hex(t->oflag);
	put(", cflag ");
	put_hex(t->cflag);
	put(", lflag ");
	put_hex(t->lflag);
	put(", line ");
	put_dec(t->line);
	end_line();
	put(name);
	put(": c_cc");
	for (int i = 0; i < 19; i++) {
		put(" ");
		put_dec(t->cc[i]);
	}
	end_line();
}

static i64 poll_in(int fd, int ms)
{
	struct {
		int fd;
		short events, revents;
	} p = { fd, 1, 0 };
	i64 r = sys3(SYS_poll, &p, 1, ms);
	return r > 0 ? p.revents : r;
}

/* Reads fd until `want` bytes arrived (then 100 ms more, to catch
 * extras) or 2 s of quiet. */
static i64 collect(int fd, char *buf, i64 size, i64 want)
{
	i64 n = 0;
	while (n < size) {
		int ms = n >= want ? 100 : 2000;
		if (poll_in(fd, ms) <= 0)
			break;
		i64 r = sys3(SYS_read, fd, buf + n, size - n);
		if (r <= 0)
			break;
		n += r;
	}
	return n;
}

static void say_echo(const char *name, i64 want)
{
	char buf[256];
	i64 n = collect(master, buf, sizeof buf, want);
	put(name);
	put(" = ");
	put_quoted(buf, n);
	end_line();
}

static void type(const char *s)
{
	write_str(master, s);
}

static void say_slave_read(const char *name, i64 size)
{
	char buf[256];
	i64 r = sys3(SYS_read, slave, buf, size);
	say_read(name, r, buf);
}

static void set_term(struct kterm *t)
{
	sys3(SYS_ioctl, slave, TCSETSF, t);
}

/* The ISIG child: the pty as its controlling terminal. SIGWINCH is
 * blocked and taken with rt_sigtimedwait, so it cannot arrive before the
 * child waits for it. */
static void isig_child(const char *path, int report)
{
	sys0(SYS_setsid);
	u64 winch = 1UL << (SIGWINCH - 1);
	sys4(SYS_rt_sigprocmask, SIG_BLOCK, &winch, 0, 8);
	int fd = sys4(SYS_openat, AT_FDCWD, path, O_RDWR, 0); /* becomes the controlling tty */
	int sid = -1;
	sys3(SYS_ioctl, fd, TIOCGSID, &sid);
	write_str(report, sid == sys0(SYS_getpid) ? "S" : "s");
	struct lx_ts ten = { 10, 0 };
	write_str(report, sys4(SYS_rt_sigtimedwait, &winch, 0, &ten, 8) == SIGWINCH ? "W" : "T");
	char c[64];
	for (;;) {
		i64 r = sys3(SYS_read, fd, c, sizeof c);
		if (r == 0 || (r < 0 && r != -4))
			lx_exit(9);
	}
}

int main(void)
{
	struct kterm t, def;
	struct stat st;
	char path[32];
	char buf[256];
	lx_init();
	sys1(SYS_alarm, 15); /* a hang ends as "killed by 14", taking the children with it */

	say_text("-- /dev/ptmx");
	master = sys4(SYS_openat, AT_FDCWD, "/dev/ptmx", O_RDWR | O_NOCTTY, 0);
	if (master < 0) {
		say_ret("SKIP: open(/dev/ptmx)", master);
		say_text("pty_test: ok");
		return 0;
	}
	say_ok("open(/dev/ptmx, O_RDWR|O_NOCTTY)", master);
	int ptn = -1;
	say_ret("TIOCGPTN", sys3(SYS_ioctl, master, TIOCGPTN, &ptn));
	say_bool("  pty number >= 0", ptn >= 0);
	/* ptsname: "/dev/pts/<n>" */
	{
		int k = 0, d = 0;
		char tmp[12];
		const char *pre = "/dev/pts/";
		while (*pre)
			path[k++] = *pre++;
		int v = ptn;
		do {
			tmp[d++] = '0' + v % 10;
			v /= 10;
		} while (v);
		while (d)
			path[k++] = tmp[--d];
		path[k] = 0;
	}
	say_ret("open(slave) while locked", sys4(SYS_openat, AT_FDCWD, path, O_RDWR | O_NOCTTY, 0));
	int unlock = 0;
	say_ret("TIOCSPTLCK(0) (unlockpt)", sys3(SYS_ioctl, master, TIOCSPTLCK, &unlock));
	slave = sys4(SYS_openat, AT_FDCWD, path, O_RDWR | O_NOCTTY, 0);
	say_ok("open(slave)", slave);
	sys2(SYS_fstat, slave, &st);
	say_bool("  slave is a character device", S_ISCHR(st.st_mode));
	say_ret("TIOCGPTN on the slave", sys3(SYS_ioctl, slave, TIOCGPTN, &ptn));
	int pfd[2];
	sys2(SYS_pipe2, pfd, 0);
	say_ret("TCGETS on a pipe", sys3(SYS_ioctl, pfd[0], TCGETS, &t));
	sys1(SYS_close, pfd[0]);
	sys1(SYS_close, pfd[1]);

	say_text("-- a fresh pty's termios");
	static unsigned char raw[128];
	u64 written = 0;
	for (int fill = 0x55; fill <= 0xaa; fill += 0x55) {
		lx_memset(raw, fill, sizeof raw);
		sys3(SYS_ioctl, slave, TCGETS, raw);
		u64 w = written_size(raw, sizeof raw, fill);
		if (w > written)
			written = w;
	}
	say("TCGETS writes bytes", written);
	sys3(SYS_ioctl, slave, TCGETS, &def);
	say_term("slave", &def);
	sys3(SYS_ioctl, master, TCGETS, &t);
	say_term("master", &t);
	struct winsz ws = { 7, 7, 7, 7 };
	say_ret("TIOCGWINSZ", sys3(SYS_ioctl, slave, TIOCGWINSZ, &ws));
	say("  rows", ws.row);
	say("  cols", ws.col);
	ws.row = 24;
	ws.col = 80;
	ws.xpixel = 640;
	ws.ypixel = 480;
	say_ret("TIOCSWINSZ(24x80) on the slave", sys3(SYS_ioctl, slave, TIOCSWINSZ, &ws));
	struct winsz got = { 0, 0, 0, 0 };
	sys3(SYS_ioctl, master, TIOCGWINSZ, &got);
	say_bool("  the master reads it back, pixels too", got.row == 24 && got.col == 80 && got.xpixel == 640 && got.ypixel == 480);
	say("sizeof(struct winsize)", sizeof(struct winsz));

	say_text("-- canonical mode");
	type("partial");
	sleep_ms(50);
	say_bool("a line without its newline: slave not readable", poll_in(slave, 100) == 0);
	type("\n");
	say_slave_read("then the newline: read", sizeof buf);
	say_echo("  echo", 9);
	type("ab\x7f" "c\n");
	say_slave_read("ab DEL c NL: read", sizeof buf);
	say_echo("  echo (ECHOE: backspace, space, backspace)", 9);
	type("xyz\x15ok\n");
	say_slave_read("xyz ^U ok NL: read", sizeof buf);
	say_echo("  echo", 16);
	type("one two\x17three\n");
	say_slave_read("one two ^W three NL: read", sizeof buf);
	say_echo("  echo", 23);
	type("cr\r");
	say_slave_read("cr CR (ICRNL): read", sizeof buf);
	say_echo("  echo", 4);
	type("abc\x04");
	say_slave_read("abc ^D: read", sizeof buf);
	type("\x04");
	say_slave_read("^D alone: read (EOF)", sizeof buf);
	say_echo("  echo of both", 3);
	type("hello\n");
	say_slave_read("hello NL: read(2)", 2);
	say_slave_read("  read again", sizeof buf);
	say_echo("  echo", 7);
	type("ctl\x01\n");
	say_slave_read("ctl ^A NL: read", sizeof buf);
	say_echo("  echo (ECHOCTL)", 7);
	write_str(slave, "out\nput");
	say_echo("slave writes out NL put: master reads (ONLCR)", 8);

	say_text("-- echo off");
	t = def;
	t.lflag &= ~ECHO_;
	set_term(&t);
	type("secret\n");
	say_slave_read("ECHO off: read", sizeof buf);
	say_echo("  echo", 0);

	say_text("-- raw mode (cfmakeraw)");
	t = def;
	t.iflag &= ~(IGNBRK_ | BRKINT_ | PARMRK_ | ISTRIP_ | INLCR_ | IGNCR_ | ICRNL_ | IXON_);
	t.oflag &= ~OPOST_;
	t.lflag &= ~(ECHO_ | ECHONL_ | ICANON_ | ISIG_ | IEXTEN_);
	t.cflag &= ~(CSIZE_ | PARENB_);
	t.cflag |= CS8_;
	t.cc[VMIN_] = 1;
	t.cc[VTIME_] = 0;
	say_ret("TCSETSF(raw)", sys3(SYS_ioctl, slave, TCSETSF, &t));
	struct kterm back;
	sys3(SYS_ioctl, slave, TCGETS, &back);
	say_term("raw", &back);
	type("x\ry\x7f\x03");
	for (int i = 0; i < 5; i++) {
		put("read(1) #");
		put_dec(i + 1);
		say_slave_read("", 1);
	}
	say_echo("  echo", 0);
	int avail = -1;
	type("abc");
	collect(slave, buf, 0, 0);
	poll_in(slave, 2000);
	sleep_ms(20);
	sys3(SYS_ioctl, slave, FIONREAD, &avail);
	say("FIONREAD after abc", avail);
	say_slave_read("read", sizeof buf);
	write_str(slave, "a\nb");
	say_echo("slave writes a NL b, OPOST off: master reads", 3);
	t.cc[VMIN_] = 0;
	t.cc[VTIME_] = 0;
	set_term(&t);
	say_slave_read("VMIN 0, VTIME 0, nothing typed: read", sizeof buf);
	t.cc[VTIME_] = 2;
	set_term(&t);
	i64 t0 = now_ns(1);
	i64 r = sys3(SYS_read, slave, buf, sizeof buf);
	i64 el = now_ns(1) - t0;
	say_ret("VMIN 0, VTIME 2 (0.2 s), nothing typed: read", r);
	say_bool("  waited >= 0.15 s", el >= 150000000);
	set_term(&def);

	say_text("-- ISIG: a child with the pty as its controlling terminal");
	int rep[2];
	sys2(SYS_pipe2, rep, 0);
	i64 pid = lx_fork();
	if (pid == 0) {
		sys1(SYS_close, rep[0]);
		sys1(SYS_close, master);
		isig_child(path, rep[1]);
	}
	sys1(SYS_close, rep[1]);
	char c = 0;
	sys3(SYS_read, rep[0], &c, 1);
	say_bool("child: setsid, open(slave) without O_NOCTTY: TIOCGSID is its pid", c == 'S');
	int fg = -1;
	sys3(SYS_ioctl, master, TIOCGPGRP, &fg);
	say_bool("TIOCGPGRP on the master: the child's group", fg == pid);
	ws.row = 50;
	ws.col = 132;
	sys3(SYS_ioctl, master, TIOCSWINSZ, &ws);
	c = 0;
	sys3(SYS_read, rep[0], &c, 1);
	say_bool("TIOCSWINSZ(50x132): SIGWINCH reached the child", c == 'W');
	type("\x1a");
	say_echo("^Z: echo", 2);
	sleep_ms(50);
	say_ret("  wait4(WNOHANG|WUNTRACED) (not stopped: orphaned group)", wait_for(pid, WNOHANG | WUNTRACED));
	type("\x03");
	say_echo("^C: echo", 2);
	say_status("  child", wait_for(pid, 0));
	sys1(SYS_close, rep[0]);

	pid = lx_fork();
	if (pid == 0) {
		sys1(SYS_close, master);
		sys0(SYS_setsid);
		int fd = sys4(SYS_openat, AT_FDCWD, path, O_RDWR, 0);
		sys3(SYS_ioctl, fd, TIOCSCTTY, 0);
		char x[8];
		for (;;)
			sys3(SYS_read, fd, x, sizeof x);
	}
	sleep_ms(50);
	for (int i = 0; i < 200 && sys3(SYS_ioctl, master, TIOCGPGRP, &fg) == 0 && fg != pid; i++)
		sleep_ms(10);
	type("\x1c");
	say_echo("^\\: echo", 2);
	say_status("  child", wait_for(pid, 0));

	say_text("-- hang-up");
	sys1(SYS_close, slave);
	collect(master, buf, sizeof buf, 0);
	say_ret("every slave fd closed: read(master)", sys3(SYS_read, master, buf, sizeof buf));
	sys1(SYS_close, master);
	say_text("pty_test: ok");
	return 0;
}
