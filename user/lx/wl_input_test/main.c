/* wl_input_test: input and window control for a Wayland window (ROADMAP
 * LD step 2), as a client of our own over a raw AF_UNIX socket (no
 * libwayland, no xkbcommon: the wire format and the events themselves
 * are what is checked).
 *
 * NOT recorded on Linux: weston's headless backend has no input devices,
 * so it offers no wl_seat and nothing to click or type with. The expected
 * transcript is written from the protocol's specification (wayland.xml:
 * wl_seat, wl_pointer, wl_keyboard; xdg-shell.xml: xdg_toplevel.close;
 * xdg-decoration-unstable-v1.xml), the evdev key codes of a US keyboard
 * (xkeyboard-config's keycodes/evdev: an X keycode is the evdev code + 8)
 * and the input the gala scenario sends (`lx_wl_input`: QEMU's PS/2 mouse
 * and keyboard driven by xtask's screen check, which also checks the
 * windows' pixels). `cargo xtask lx-expect` builds it and does not run it
 * (lx_expect.rs `INPUT_TESTS`).
 *
 * It binds wl_seat (version 7, as weston offers it) and the decoration
 * manager, and checks:
 * - wl_seat: capabilities pointer and keyboard, its name;
 * - wl_keyboard: the keymap (format xkb_v1, a descriptor passed as
 *   SCM_RIGHTS that maps read-only and privately, holding an XKB text
 *   keymap ending with its NUL at size - 1), repeat_info;
 * - a 200x150 toplevel with a decoration object and no mode asked for:
 *   the compositor's choice is server-side (2); once mapped and active
 *   it has the keyboard focus (enter with no keys down, then modifiers);
 * - the pointer, as the scenario moves it: enter, a left click (button
 *   0x110 pressed and released at the surface-local point the scenario
 *   aims at, each in its frame), the wheel turned one notch away from the
 *   user (axis_source wheel, axis_discrete -1, axis -10 on the vertical
 *   axis, in one frame), leave when the pointer moves off the surface;
 * - the keyboard, as the scenario types: evdev codes (a = 30, Left = 105,
 *   an E0 key), Shift's depressed modifier (1) in a modifiers event after
 *   the key that changed it;
 * - xdg_wm_base 6's wm_capabilities: maximize and minimize (gala's desktop
 *   has both; no window menu or fullscreen yet);
 * - the activated state in the toplevel's configure once the window is
 *   the desktop's active one (xdg-shell: "the client window is active");
 * - configure from the desktop (the scenario's Snap keys): Win+Up
 *   maximizes (the work area's size less shasha's frame, states maximized
 *   and activated), Win+Down restores (the size before), Win+Left snaps
 *   to the left half (tiled_left, tiled_top, tiled_bottom: xdg_toplevel
 *   2), Win+Down puts it back; each is acked and answered with a buffer
 *   of the size asked for (the scenario checks the window's pixels where
 *   the desktop placed it). The sizes are those of the scenario's
 *   1280 x 800 screen less its 48-row taskbar;
 * - xdg_toplevel.close when the scenario clicks the frame's close button;
 *   destroying that toplevel, which has the keyboard focus, gives the
 *   keyboard's leave (an unmapped surface loses its focus);
 * - a second toplevel whose decoration asks for client-side: configured
 *   client-side (1), shown frameless (the scenario's pixels), focused;
 *   "q" ends the test.
 *
 * Positions and times are not printed (only the surface-local point of a
 * button, which is where the scenario puts the pointer), nor motion
 * events (how many depends on how the host's mouse packets come). Pings
 * are answered. */
#define _GNU_SOURCE
#include "../common/lx.h"
#include "../common/more.h"

#define AF_UNIX_ 1
#define SOCK_STREAM_ 1
#define SOCK_CLOEXEC_ 02000000
#define SOL_SOCKET_ 1
#define SCM_RIGHTS_ 1
#define MSG_CMSG_CLOEXEC_ 0x40000000
#define PROT_READ 1
#define PROT_WRITE 2
#define MAP_SHARED 1
#define MAP_PRIVATE 2
#define MFD_CLOEXEC 1

typedef unsigned int u32;
typedef int i32;

struct iovec_ { void *base; u64 len; };
struct msghdr_ {
	void *name; u32 namelen; struct iovec_ *iov; u64 iovlen;
	void *control; u64 controllen; int flags;
};
struct cmsg_fd { u64 len; int level; int type; int fd; int pad; };

/* ---- Objects: what each id is, for the events. ---- */

enum kind { NONE, DISPLAY, REGISTRY, CALLBACK, COMPOSITOR, SHM, WM_BASE, POOL, BUFFER,
	    SURFACE, XDG_SURFACE, TOPLEVEL, SEAT, POINTER, KEYBOARD, DECO_MANAGER, DECORATION };
static enum kind objects[128];
static u32 next_id = 2;

static u32 new_object(enum kind k)
{
	objects[next_id] = k;
	return next_id++;
}

/* ---- Requests ---- */

static int sock;
static u32 out[1024];
static u32 out_words;

static void begin(u32 object, u32 opcode)
{
	out[0] = object;
	out[1] = opcode;
	out_words = 2;
}

static void word(u32 w) { out[out_words++] = w; }

static void string(const char *s)
{
	u32 n = (u32)lx_strlen(s) + 1;
	word(n);
	lx_memset(&out[out_words], 0, (n + 3) & ~3u);
	lx_memcpy(&out[out_words], s, n);
	out_words += (n + 3) / 4;
}

/* Sends the message built since begin(), with `fd` beside it (-1: none). */
static void send(int fd)
{
	out[1] |= (out_words * 4) << 16;
	struct iovec_ iov = { out, out_words * 4 };
	struct cmsg_fd c = { 20, SOL_SOCKET_, SCM_RIGHTS_, fd, 0 };
	struct msghdr_ m = { 0, 0, &iov, 1, fd >= 0 ? &c : 0, fd >= 0 ? 24 : 0, 0 };
	i64 r = sys3(SYS_sendmsg, sock, &m, 0);
	if (r != (i64)(out_words * 4)) {
		say_ret("sendmsg", r);
		lx_exit(1);
	}
}

/* ---- Events ---- */

static unsigned char in[16384];
static u64 in_len;
/* Descriptors that came with the events, oldest first. */
static int fds_in[16];
static int fds_n;

#define WANTED 5
static const char *const wanted[WANTED] = { "wl_compositor", "wl_shm", "xdg_wm_base", "wl_seat",
					    "zxdg_decoration_manager_v1" };
static u32 globals_name[WANTED], globals_version[WANTED];
static u32 done_callback;
static u32 configure_serial;
static u32 seat_caps;
static char seat_name[64];
static int keymap_seen, keymap_format, keymap_fd = -1;
static u32 keymap_size;
static i32 repeat_rate = -1, repeat_delay = -1;
static u32 decoration_mode;
static int closed;

/* The toplevel's last configure: size, states (xdg_toplevel.configure). */
static i32 conf_w, conf_h;
static u32 conf_states[16];
static u32 conf_nstates;
static int conf_activated;
/* wm_capabilities (xdg_toplevel 5). */
static u32 wm_caps[8];
static u32 wm_ncaps;
/* The first window's buffer size, and a configure asking for another. */
static i32 shown_w, shown_h;
static int resize_due;

/* Focus and pointer state. */
static u32 surface1, surface2;
static u32 kb_focus, kb_enter_keys;
static int mods_after_enter;
static u32 ptr_focus;
static i32 ptr_x, ptr_y;
static int live;
static int frame_due;
static const char *frame_what;
static int quit;

static u32 rd(const unsigned char *p) { u32 v; lx_memcpy(&v, p, 4); return v; }

static const char *which(u32 surface)
{
	return surface == surface1 ? "first" : surface == surface2 ? "second" : "other";
}

static void event(u32 object, u32 opcode, const unsigned char *a, u32 len)
{
	enum kind k = object < 128 ? objects[object] : NONE;
	(void)len;
	if (k == DISPLAY && opcode == 0) {
		put("wl_display.error object ");
		put_dec(rd(a));
		put(" code ");
		put_dec(rd(a + 4));
		end_line();
		lx_exit(1);
	} else if (k == REGISTRY && opcode == 0) {
		u32 name = rd(a), slen = rd(a + 4);
		const char *iface = (const char *)a + 8;
		u32 version = rd(a + 8 + ((slen + 3) & ~3u));
		for (int i = 0; i < WANTED; i++)
			if (lx_streq(iface, wanted[i])) {
				globals_name[i] = name;
				globals_version[i] = version;
			}
	} else if (k == CALLBACK && opcode == 0) {
		done_callback = object;
	} else if (k == WM_BASE && opcode == 0) {
		u32 serial = rd(a);
		begin(object, 3); /* pong */
		word(serial);
		send(-1);
	} else if (k == XDG_SURFACE && opcode == 0) {
		configure_serial = rd(a);
		if (live && conf_w > 0 && (conf_w != shown_w || conf_h != shown_h))
			resize_due = 1;
	} else if (k == TOPLEVEL && opcode == 0) {
		/* configure(width, height, states) */
		u32 n = rd(a + 8) / 4;
		conf_w = (i32)rd(a);
		conf_h = (i32)rd(a + 4);
		conf_nstates = 0;
		conf_activated = 0;
		for (u32 i = 0; i < n && i < 16; i++) {
			conf_states[conf_nstates++] = rd(a + 12 + 4 * i);
			conf_activated |= rd(a + 12 + 4 * i) == 4;
		}
	} else if (k == TOPLEVEL && opcode == 3) {
		/* wm_capabilities(capabilities) */
		u32 n = rd(a) / 4;
		wm_ncaps = 0;
		for (u32 i = 0; i < n && i < 8; i++)
			wm_caps[wm_ncaps++] = rd(a + 4 + 4 * i);
	} else if (k == TOPLEVEL && opcode == 1) {
		closed = 1;
		say_text("toplevel close");
	} else if (k == DECORATION && opcode == 0) {
		decoration_mode = rd(a);
	} else if (k == SEAT && opcode == 0) {
		seat_caps = rd(a);
	} else if (k == SEAT && opcode == 1) {
		u32 n = rd(a);
		if (n > 0 && n < sizeof seat_name)
			lx_memcpy(seat_name, a + 4, n);
	} else if (k == KEYBOARD && opcode == 0) {
		/* keymap(format, fd, size) */
		keymap_seen = 1;
		keymap_format = (int)rd(a);
		keymap_size = rd(a + 4);
		if (fds_n > 0) {
			keymap_fd = fds_in[0];
			lx_memcpy(fds_in, fds_in + 1, sizeof fds_in[0] * (u64)--fds_n);
		}
	} else if (k == KEYBOARD && opcode == 1) {
		/* enter(serial, surface, keys) */
		kb_focus = rd(a + 4);
		kb_enter_keys = rd(a + 8) / 4;
		mods_after_enter = 0;
		if (live) {
			put("keyboard enter ");
			put(which(kb_focus));
			put(", keys down ");
			put_dec(kb_enter_keys);
			end_line();
		}
	} else if (k == KEYBOARD && opcode == 2) {
		if (live) {
			put("keyboard leave ");
			put(which(rd(a + 4)));
			end_line();
		}
		kb_focus = 0;
	} else if (k == KEYBOARD && opcode == 3) {
		/* key(serial, time, key, state) */
		u32 key = rd(a + 8), state = rd(a + 12);
		put("key ");
		put_dec(key);
		put(state ? " pressed" : " released");
		end_line();
		if (key == 16 && state)
			quit = 1;
	} else if (k == KEYBOARD && opcode == 4) {
		/* modifiers(serial, depressed, latched, locked, group) */
		mods_after_enter = 1;
		if (live) {
			put("modifiers ");
			for (int i = 1; i < 5; i++) {
				put_dec(rd(a + 4 * i));
				put(i < 4 ? " " : "");
			}
			end_line();
		}
	} else if (k == KEYBOARD && opcode == 5) {
		repeat_rate = (i32)rd(a);
		repeat_delay = (i32)rd(a + 4);
	} else if (k == POINTER && opcode == 0) {
		/* enter(serial, surface, x, y) */
		ptr_focus = rd(a + 4);
		ptr_x = (i32)rd(a + 8) / 256;
		ptr_y = (i32)rd(a + 12) / 256;
		put("pointer enter ");
		put(which(ptr_focus));
		end_line();
	} else if (k == POINTER && opcode == 1) {
		put("pointer leave ");
		put(which(rd(a + 4)));
		end_line();
		ptr_focus = 0;
	} else if (k == POINTER && opcode == 2) {
		ptr_x = (i32)rd(a + 4) / 256;
		ptr_y = (i32)rd(a + 8) / 256;
	} else if (k == POINTER && opcode == 3) {
		/* button(serial, time, button, state) */
		put("button ");
		put_dec(rd(a + 8));
		put(rd(a + 12) ? " pressed at " : " released at ");
		put_dec(ptr_x);
		put(",");
		put_dec(ptr_y);
		end_line();
		frame_due = 1;
		frame_what = "button frame";
	} else if (k == POINTER && opcode == 4) {
		/* axis(time, axis, value) */
		put("axis ");
		put_dec(rd(a + 4));
		put(" value ");
		put_dec((i32)rd(a + 8) / 256);
		end_line();
		frame_due = 1;
		frame_what = "axis frame";
	} else if (k == POINTER && opcode == 5) {
		if (frame_due)
			say_text(frame_what);
		frame_due = 0;
	} else if (k == POINTER && opcode == 6) {
		say("axis source", rd(a));
	} else if (k == POINTER && opcode == 8) {
		put("axis discrete ");
		put_dec(rd(a));
		put(" ");
		put_dec((i32)rd(a + 4));
		end_line();
	}
}

/* Reads once (blocking) and handles every whole message; 0 at end of file. */
static i64 dispatch(void)
{
	unsigned char control[256];
	struct iovec_ iov = { in + in_len, sizeof in - in_len };
	struct msghdr_ m = { 0, 0, &iov, 1, control, sizeof control, 0 };
	i64 r = sys3(SYS_recvmsg, sock, &m, MSG_CMSG_CLOEXEC_);
	if (r <= 0)
		return r;
	/* Descriptors the compositor sent: queued for the events taking them. */
	for (u64 at = 0; at + 16 <= m.controllen;) {
		struct cmsg_fd *c = (struct cmsg_fd *)(control + at);
		if (c->level == SOL_SOCKET_ && c->type == SCM_RIGHTS_)
			for (u64 i = 0; 16 + 4 * i < c->len; i++) {
				int fd = ((int *)(control + at + 16))[i];
				if (fds_n < 16)
					fds_in[fds_n++] = fd;
				else
					sys1(NR_close, fd);
			}
		at += (c->len + 7) & ~7ul;
	}
	in_len += (u64)r;
	u64 at = 0;
	while (in_len - at >= 8) {
		u32 object = rd(in + at), second = rd(in + at + 4);
		u32 size = second >> 16;
		if (size < 8 || in_len - at < size)
			break;
		event(object, second & 0xffff, in + at + 8, size - 8);
		at += size;
	}
	lx_memcpy(in, in + at, in_len - at);
	in_len -= at;
	return r;
}

static void more(const char *what)
{
	if (dispatch() <= 0) {
		put(what);
		put(": the connection ended");
		end_line();
		lx_exit(1);
	}
}

/* wl_display.sync, then events until its callback is done. */
static void roundtrip(void)
{
	u32 cb = new_object(CALLBACK);
	begin(1, 0);
	word(cb);
	send(-1);
	done_callback = 0;
	while (done_callback != cb)
		more("roundtrip");
}

static int connect_display(const char *dir, const char *display)
{
	struct { unsigned short family; char path[108]; } addr = { AF_UNIX_, { 0 } };
	u64 n = lx_strlen(dir);
	lx_memcpy(addr.path, dir, n);
	addr.path[n] = '/';
	lx_memcpy(addr.path + n + 1, display, lx_strlen(display));
	int s = (int)sys3(SYS_socket, AF_UNIX_, SOCK_STREAM_ | SOCK_CLOEXEC_, 0);
	i64 r = sys3(SYS_connect, s, &addr, sizeof addr);
	say_ret("connect", r);
	return s;
}

static const char *env(char **envp, const char *name)
{
	u64 n = lx_strlen(name);
	for (; *envp; envp++)
		if (lx_memeq(*envp, name, n) && (*envp)[n] == '=')
			return *envp + n + 1;
	return 0;
}

static int contains(const char *text, u64 n, const char *what)
{
	u64 w = lx_strlen(what);
	for (u64 i = 0; i + w <= n; i++)
		if (lx_memeq(text + i, what, w))
			return 1;
	return 0;
}

/* The bound globals. */
static u32 compositor, shm, wm_base, seat, deco_manager;

/* A w x h XRGB8888 buffer of one colour (`top` above `split`, `bottom`
 * below) in a memfd pool of its own. */
static u32 make_buffer(int w, int h, int split, u32 top, u32 bottom)
{
	int memfd = (int)sys2(SYS_memfd_create, "wl_input_test", MFD_CLOEXEC);
	i64 r = sys2(SYS_ftruncate, memfd, w * h * 4);
	if (r != 0) {
		say_ret("ftruncate", r);
		lx_exit(1);
	}
	u32 *pixels = (u32 *)sys6(NR_mmap, 0, w * h * 4, PROT_READ | PROT_WRITE, MAP_SHARED,
				   memfd, 0);
	for (int y = 0; y < h; y++)
		for (int x = 0; x < w; x++)
			pixels[y * w + x] = y < split ? top : bottom;
	u32 pool = new_object(POOL);
	begin(shm, 0); /* create_pool(id, fd, size) */
	word(pool);
	word((u32)(w * h * 4));
	send(memfd);
	sys1(NR_close, memfd);
	u32 buffer = new_object(BUFFER);
	begin(pool, 0); /* create_buffer(id, offset, width, height, stride, format) */
	word(buffer);
	word(0);
	word((u32)w);
	word((u32)h);
	word((u32)(w * 4));
	word(1); /* XRGB8888 */
	send(-1);
	begin(pool, 1); /* destroy: the buffer keeps the memory */
	send(-1);
	return buffer;
}

struct window {
	u32 surface, xdg, toplevel, decoration, buffer;
};

/* A toplevel titled `title` with a decoration object (`mode` asked for, 0:
 * none), configured (the decoration's mode printed) and mapped with
 * `buffer`. */
static struct window make_window(const char *title, u32 buffer, int w, int h, u32 mode)
{
	struct window win = { 0, 0, 0, 0, buffer };
	win.surface = new_object(SURFACE);
	begin(compositor, 0); /* create_surface */
	word(win.surface);
	send(-1);
	win.xdg = new_object(XDG_SURFACE);
	begin(wm_base, 2); /* get_xdg_surface(id, surface) */
	word(win.xdg);
	word(win.surface);
	send(-1);
	win.toplevel = new_object(TOPLEVEL);
	begin(win.xdg, 1); /* get_toplevel */
	word(win.toplevel);
	send(-1);
	begin(win.toplevel, 2); /* set_title */
	string(title);
	send(-1);
	win.decoration = new_object(DECORATION);
	begin(deco_manager, 1); /* get_toplevel_decoration(id, toplevel) */
	word(win.decoration);
	word(win.toplevel);
	send(-1);
	if (mode) {
		begin(win.decoration, 1); /* set_mode */
		word(mode);
		send(-1);
	}
	configure_serial = 0;
	decoration_mode = 0;
	begin(win.surface, 6); /* commit, without a buffer */
	send(-1);
	while (!configure_serial)
		more("configure");
	say("decoration mode", decoration_mode);
	say_bool("xdg_surface configure", 1);
	put("wm capabilities:");
	for (u32 i = 0; i < wm_ncaps; i++) {
		static const char *const names[] = { "?", "window_menu", "maximize", "fullscreen",
						     "minimize" };
		put(" ");
		put(wm_caps[i] < 5 ? names[wm_caps[i]] : "?");
	}
	end_line();
	begin(win.xdg, 4); /* ack_configure */
	word(configure_serial);
	send(-1);
	begin(win.surface, 1); /* attach(buffer, x, y) */
	word(buffer);
	word(0);
	word(0);
	send(-1);
	begin(win.surface, 9); /* damage_buffer */
	word(0);
	word(0);
	word((u32)w);
	word((u32)h);
	send(-1);
	begin(win.surface, 6); /* commit */
	send(-1);
	return win;
}

/* The desktop asked the first window for another size (resize_due): the
 * configure printed, acked, and a buffer of that size committed (the
 * first buffer again for its own size), then the step's line for the
 * scenario. */
static void resize(struct window *w)
{
	static const char *const state_names[] = { "?", "maximized", "fullscreen", "resizing",
						   "activated", "tiled_left", "tiled_right",
						   "tiled_top", "tiled_bottom", "suspended" };
	static const char *const steps[] = { "maximized", "restored", "snapped", "unsnapped" };
	static const u32 colours[] = { 0x002e8b57, 0, 0x008b4513, 0 };
	static int step;
	resize_due = 0;
	put("configure ");
	put_dec((u32)conf_w);
	put(" ");
	put_dec((u32)conf_h);
	put(":");
	for (u32 i = 0; i < conf_nstates; i++) {
		put(" ");
		put(conf_states[i] < 10 ? state_names[conf_states[i]] : "?");
	}
	end_line();
	begin(w->xdg, 4); /* ack_configure */
	word(configure_serial);
	send(-1);
	u32 buffer = w->buffer;
	if (conf_w != 200 || conf_h != 150)
		buffer = make_buffer(conf_w, conf_h, conf_h, colours[step & 3], colours[step & 3]);
	begin(w->surface, 1); /* attach(buffer, x, y) */
	word(buffer);
	word(0);
	word(0);
	send(-1);
	begin(w->surface, 9); /* damage_buffer */
	word(0);
	word(0);
	word((u32)conf_w);
	word((u32)conf_h);
	send(-1);
	begin(w->surface, 6); /* commit */
	send(-1);
	roundtrip();
	shown_w = conf_w;
	shown_h = conf_h;
	put("wl_input_test: ");
	put(step < 4 ? steps[step] : "resized");
	end_line();
	step++;
}

int main(int argc, char **argv, char **envp)
{
	(void)argc;
	(void)argv;
	lx_init();
	const char *dir = env(envp, "XDG_RUNTIME_DIR"), *display = env(envp, "WAYLAND_DISPLAY");
	if (!dir || !display) {
		say_text("XDG_RUNTIME_DIR or WAYLAND_DISPLAY is not set");
		lx_exit(1);
	}
	sock = connect_display(dir, display);
	objects[1] = DISPLAY;

	/* The registry and its globals. */
	u32 registry = new_object(REGISTRY);
	begin(1, 1);
	word(registry);
	send(-1);
	roundtrip();
	int missing = 0;
	for (int i = 0; i < WANTED; i++) {
		put("global ");
		put(wanted[i]);
		put(globals_name[i] ? ": yes" : ": no");
		end_line();
		missing |= !globals_name[i];
	}
	if (missing)
		lx_exit(1);

	/* Bind wl_compositor 4, wl_shm 1, xdg_wm_base 6 (tiled states,
	 * wm_capabilities), wl_seat 7 and the decoration manager 1 (at most
	 * what is offered). */
	static const u32 want_version[WANTED] = { 4, 1, 6, 7, 1 };
	static const enum kind kinds[WANTED] = { COMPOSITOR, SHM, WM_BASE, SEAT, DECO_MANAGER };
	u32 bound[WANTED];
	for (int i = 0; i < WANTED; i++) {
		u32 v = globals_version[i] < want_version[i] ? globals_version[i] : want_version[i];
		bound[i] = new_object(kinds[i]);
		begin(registry, 0);
		word(globals_name[i]);
		string(wanted[i]);
		word(v);
		word(bound[i]);
		send(-1);
		if (kinds[i] == SEAT) {
			put("bind wl_seat version ");
			put_dec(v);
			end_line();
		}
	}
	compositor = bound[0];
	shm = bound[1];
	wm_base = bound[2];
	seat = bound[3];
	deco_manager = bound[4];
	roundtrip();
	say("seat capabilities", seat_caps);
	put("seat name: ");
	put(seat_name);
	end_line();

	/* The pointer and the keyboard: the keymap comes at once. */
	u32 pointer = new_object(POINTER);
	begin(seat, 0); /* get_pointer */
	word(pointer);
	send(-1);
	u32 keyboard = new_object(KEYBOARD);
	begin(seat, 1); /* get_keyboard */
	word(keyboard);
	send(-1);
	roundtrip();
	say_bool("keymap", keymap_seen);
	say("keymap format", keymap_format);
	say_bool("keymap fd", keymap_fd >= 0);
	/* Since wl_keyboard 7 the client must map it MAP_PRIVATE. */
	const char *map = (const char *)sys6(NR_mmap, 0, keymap_size, PROT_READ, MAP_PRIVATE,
					      keymap_fd, 0);
	int mapped = (i64)map > 0 || (i64)map < -4096;
	say_bool("keymap mapped", mapped);
	if (!mapped)
		lx_exit(1);
	say_bool("keymap text",
		 keymap_size > 13 && lx_memeq(map, "xkb_keymap {", 12) && map[keymap_size - 1] == 0 &&
			 lx_strlen(map) == keymap_size - 1);
	say_bool("keymap us",
		 contains(map, keymap_size, "xkb_symbols") && contains(map, keymap_size, "<AC01>") &&
			 contains(map, keymap_size, "<LFSH>"));
	sys2(NR_munmap, map, keymap_size);
	sys1(NR_close, keymap_fd);
	say("repeat rate", repeat_rate);
	say("repeat delay", repeat_delay);

	/* The first window: server-side decorations, mapped, focused. */
	u32 buffer1 = make_buffer(200, 150, 75, 0x00336699, 0x00ff8800);
	struct window w1 = make_window("wl_input_test", buffer1, 200, 150, 0);
	surface1 = w1.surface;
	shown_w = 200;
	shown_h = 150;
	while (kb_focus != surface1 || !mods_after_enter)
		more("keyboard enter");
	say_bool("keyboard enter first", 1);
	say("keyboard enter keys down", kb_enter_keys);
	/* Active: a configure with the activated state follows. */
	while (!conf_activated)
		more("activated");
	say_bool("configure activated first", 1);
	live = 1;
	say_text("wl_input_test: mapped");

	/* The scenario's clicks, wheel, keys, pointer moves and Snap keys,
	 * until the frame's close button closes it. */
	while (!closed) {
		more("input");
		if (resize_due)
			resize(&w1);
	}
	begin(w1.decoration, 0); /* destroy: before its toplevel */
	send(-1);
	begin(w1.toplevel, 0);
	send(-1);
	begin(w1.xdg, 0);
	send(-1);
	begin(w1.surface, 0);
	send(-1);
	begin(buffer1, 0);
	send(-1);

	/* The second: client-side decorations asked for and given, so the
	 * desktop shows it frameless. */
	u32 buffer2 = make_buffer(100, 80, 80, 0x008a2be2, 0x008a2be2);
	struct window w2 = make_window("wl_input_test 2", buffer2, 100, 80, 1);
	surface2 = w2.surface;
	while (kb_focus != surface2 || !mods_after_enter)
		more("keyboard enter");
	say_text("wl_input_test: frameless mapped");
	while (!quit)
		more("the last key");
	sys1(NR_close, sock);
	say_text("wl_input_test: ok");
	lx_exit(0);
}
