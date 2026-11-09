/* wl_popup_test: xdg_positioner and xdg_popup (ROADMAP LD step 2), as a
 * client of our own over a raw AF_UNIX socket (no libwayland: the wire
 * format and the events themselves are what is checked).
 *
 * Recorded on Linux against weston's headless backend (lx-expect starts
 * it). Nothing printed depends on where the compositor puts the toplevel
 * or on its output's size: every position is the popup's, relative to its
 * parent's window geometry, and no positioner here asks for a constraint
 * adjustment (weston 13's headless backend adjusts nothing; gala's flip,
 * slide and resize against the work area are checked by wl_input_test,
 * whose window is where the scenario puts it, and by the host tests). A
 * headless weston has no wl_seat either, so nothing here grabs: popups
 * grabbed and dismissed by a click outside, the keyboard focus a grab
 * takes, and the not_the_topmost_popup error of grabbing popups are
 * wl_input_test's (gala only, driven by its scenario's mouse and
 * keyboard). Errors weston answers otherwise than the specification
 * (set_gravity/set_anchor out of their enums: accepted; an incomplete
 * positioner: code 5 on the xdg_surface instead of xdg_wm_base's
 * invalid_positioner; get_popup on a toplevel's xdg_surface: code 0 on it;
 * an xdg_surface destroyed before its popup: accepted) are not here: gala
 * follows the specification there, checked by the host tests.
 *
 * It binds wl_compositor 4, wl_shm 1 and xdg_wm_base 3 (reposition, the
 * version 3 positioner requests), maps a 200x150 toplevel and checks:
 * - xdg_popup.configure: the position from the positioner's anchor
 *   rectangle (20, 30, 40, 10), anchor, gravity and offset, relative to
 *   the parent's window geometry, and the positioner's size: every anchor
 *   with gravity bottom_right (the popup's corner at the anchor point:
 *   a corner of the rectangle, the middle of an edge, its centre), every
 *   gravity with anchor top_left (centred on an axis without gravity), an
 *   offset with odd sizes (halves round down); then xdg_surface.configure,
 *   acked, and a buffer of that size committed (mapped). The positioner is
 *   destroyed at once after get_popup: the popup keeps its rules;
 * - a nested popup (its parent a popup), against that popup's geometry;
 * - xdg_popup.reposition (version 3): repositioned with the token, then
 *   configure and xdg_surface.configure;
 * - the positioner's version 3 requests (set_reactive, set_parent_size,
 *   set_parent_configure) accepted;
 * - delete_id for each popup destroyed (topmost first);
 * - no constraint adjustment (the default): a position far outside any
 *   output is kept;
 * - protocol errors, each on a connection of its own (wl_display.error
 *   names the object, by what it is here, and the code), then the end of
 *   the stream: set_size 0 and set_anchor_rect with a negative size
 *   (xdg_positioner invalid_input);
 * - popups without a grab may be destroyed in any order: a parent popup
 *   destroyed before its child is no error, and the child stays (no
 *   popup_done).
 *
 * Pings are answered and not printed. */
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
	    SURFACE, XDG_SURFACE, TOPLEVEL, POSITIONER, POPUP, OUTPUT };
static const char *const kind_names[] = { "none", "wl_display", "wl_registry", "wl_callback",
					  "wl_compositor", "wl_shm", "xdg_wm_base", "wl_shm_pool",
					  "wl_buffer", "wl_surface", "xdg_surface", "xdg_toplevel",
					  "xdg_positioner", "xdg_popup", "wl_output" };
#define MAX_OBJECTS 256
static enum kind objects[MAX_OBJECTS];
static u32 next_id;

static u32 new_object(enum kind k)
{
	objects[next_id] = k;
	return next_id++;
}

/* ---- Requests ---- */

static int sock;
static u32 out[256];
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

#define WANTED 3
static const char *const wanted[WANTED] = { "wl_compositor", "wl_shm", "xdg_wm_base" };
static u32 globals_name[WANTED], globals_version[WANTED];
static u32 done_callback;
static u32 configure_serial;
static u32 deleted[MAX_OBJECTS];
/* An error ends the connection: its object's kind and code. */
static int error_seen;
static u32 error_object, error_code;
/* The last popup configure, and on which popup. */
static u32 popup_configured;
static i32 pc_x, pc_y, pc_w, pc_h;
static u32 repositioned_token;
static int repositioned_seen;
static u32 popup_done_seen;

static u32 rd(const unsigned char *p) { u32 v; lx_memcpy(&v, p, 4); return v; }

static void event(u32 object, u32 opcode, const unsigned char *a)
{
	enum kind k = object < MAX_OBJECTS ? objects[object] : NONE;
	if (k == DISPLAY && opcode == 0) {
		error_seen = 1;
		error_object = rd(a);
		error_code = rd(a + 4);
	} else if (k == DISPLAY && opcode == 1) {
		u32 id = rd(a);
		if (id < MAX_OBJECTS)
			deleted[id] = 1;
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
	} else if (k == POPUP && opcode == 0) {
		popup_configured = object;
		pc_x = (i32)rd(a);
		pc_y = (i32)rd(a + 4);
		pc_w = (i32)rd(a + 8);
		pc_h = (i32)rd(a + 12);
	} else if (k == POPUP && opcode == 1) {
		popup_done_seen = object;
	} else if (k == POPUP && opcode == 2) {
		repositioned_seen = 1;
		repositioned_token = rd(a);
	}
}

/* Reads once (blocking) and handles every whole message; 0 at end of file. */
static i64 dispatch(void)
{
	i64 r = sys3(NR_read, sock, in + in_len, sizeof in - in_len);
	if (r <= 0)
		return r;
	in_len += (u64)r;
	u64 at = 0;
	while (in_len - at >= 8) {
		u32 object = rd(in + at), second = rd(in + at + 4);
		u32 size = second >> 16;
		if (size < 8 || in_len - at < size)
			break;
		event(object, second & 0xffff, in + at + 8);
		at += size;
	}
	lx_memcpy(in, in + at, in_len - at);
	in_len -= at;
	return r;
}

static void error_text(void)
{
	put("wl_display.error on ");
	put(error_object < MAX_OBJECTS ? kind_names[objects[error_object]] : "?");
	put(" code ");
	put_dec(error_code);
}

static void more(const char *what)
{
	if (dispatch() <= 0 || error_seen) {
		put(what);
		put(": the connection ended");
		end_line();
		if (error_seen) {
			error_text();
			end_line();
		}
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
	if (r != 0) {
		say_ret("connect", r);
		lx_exit(1);
	}
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

/* The bound globals. */
static u32 compositor, shm, wm_base;
static const char *runtime_dir, *display_name;

/* A connection with the globals bound (versions printed on the first). */
static void open_connection(int first)
{
	for (u32 i = 0; i < MAX_OBJECTS; i++) {
		objects[i] = NONE;
		deleted[i] = 0;
	}
	next_id = 2;
	in_len = 0;
	error_seen = 0;
	sock = connect_display(runtime_dir, display_name);
	objects[1] = DISPLAY;
	u32 registry = new_object(REGISTRY);
	begin(1, 1);
	word(registry);
	send(-1);
	roundtrip();
	static const u32 want_version[WANTED] = { 4, 1, 3 };
	static const enum kind kinds[WANTED] = { COMPOSITOR, SHM, WM_BASE };
	u32 bound[WANTED];
	for (int i = 0; i < WANTED; i++) {
		if (!globals_name[i] || globals_version[i] < want_version[i]) {
			put("global ");
			put(wanted[i]);
			put(": missing or too old");
			end_line();
			lx_exit(1);
		}
		bound[i] = new_object(kinds[i]);
		begin(registry, 0);
		word(globals_name[i]);
		string(wanted[i]);
		word(want_version[i]);
		word(bound[i]);
		send(-1);
		if (first) {
			put("bind ");
			put(wanted[i]);
			put(" version ");
			put_dec(want_version[i]);
			end_line();
		}
	}
	compositor = bound[0];
	shm = bound[1];
	wm_base = bound[2];
}

/* A w x h XRGB8888 buffer of one colour in a memfd pool of its own. */
static u32 make_buffer(int w, int h, u32 colour)
{
	int memfd = (int)sys2(SYS_memfd_create, "wl_popup_test", MFD_CLOEXEC);
	i64 r = sys2(SYS_ftruncate, memfd, w * h * 4);
	if (r != 0) {
		say_ret("ftruncate", r);
		lx_exit(1);
	}
	u32 *pixels = (u32 *)sys6(NR_mmap, 0, w * h * 4, PROT_READ | PROT_WRITE, MAP_SHARED,
				   memfd, 0);
	for (int i = 0; i < w * h; i++)
		pixels[i] = colour;
	sys2(NR_munmap, pixels, w * h * 4);
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

static u32 make_surface(void)
{
	u32 s = new_object(SURFACE);
	begin(compositor, 0); /* create_surface */
	word(s);
	send(-1);
	return s;
}

static u32 make_xdg(u32 surface)
{
	u32 x = new_object(XDG_SURFACE);
	begin(wm_base, 2); /* get_xdg_surface(id, surface) */
	word(x);
	word(surface);
	send(-1);
	return x;
}

static void commit(u32 surface)
{
	begin(surface, 6);
	send(-1);
}

static void attach_commit(u32 surface, u32 buffer)
{
	begin(surface, 1); /* attach(buffer, 0, 0) */
	word(buffer);
	word(0);
	word(0);
	send(-1);
	begin(surface, 2); /* damage(0, 0, big) */
	word(0);
	word(0);
	word(0x7fffffff);
	word(0x7fffffff);
	send(-1);
	commit(surface);
}

static void ack(u32 xdg)
{
	begin(xdg, 4); /* ack_configure */
	word(configure_serial);
	send(-1);
}

/* xdg_positioner: what set_* takes. */
struct pos {
	i32 w, h;
	i32 ax, ay, aw, ah;
	u32 anchor, gravity, adjust;
	i32 ox, oy;
	int v3; /* the version 3 requests too */
};

static u32 make_positioner(const struct pos *p)
{
	u32 id = new_object(POSITIONER);
	begin(wm_base, 1); /* create_positioner */
	word(id);
	send(-1);
	begin(id, 1); /* set_size */
	word((u32)p->w);
	word((u32)p->h);
	send(-1);
	begin(id, 2); /* set_anchor_rect */
	word((u32)p->ax);
	word((u32)p->ay);
	word((u32)p->aw);
	word((u32)p->ah);
	send(-1);
	if (p->anchor) {
		begin(id, 3);
		word(p->anchor);
		send(-1);
	}
	if (p->gravity) {
		begin(id, 4);
		word(p->gravity);
		send(-1);
	}
	if (p->adjust) {
		begin(id, 5);
		word(p->adjust);
		send(-1);
	}
	if (p->ox || p->oy) {
		begin(id, 6);
		word((u32)p->ox);
		word((u32)p->oy);
		send(-1);
	}
	if (p->v3) {
		begin(id, 7); /* set_reactive */
		send(-1);
		begin(id, 8); /* set_parent_size */
		word(200);
		word(150);
		send(-1);
		begin(id, 9); /* set_parent_configure */
		word(configure_serial);
		send(-1);
	}
	return id;
}

static void destroy(u32 id)
{
	begin(id, 0);
	send(-1);
}

struct popup {
	u32 surface, xdg, popup;
};

/* A popup of `parent` (an xdg_surface) placed by `p`: its configure read
 * and printed (`what`), acked, and mapped with a buffer of its size. */
static struct popup make_popup(u32 parent, const struct pos *p, const char *what, u32 colour)
{
	struct popup pp;
	pp.surface = make_surface();
	pp.xdg = make_xdg(pp.surface);
	u32 positioner = make_positioner(p);
	pp.popup = new_object(POPUP);
	begin(pp.xdg, 2); /* get_popup(id, parent, positioner) */
	word(pp.popup);
	word(parent);
	word(positioner);
	send(-1);
	destroy(positioner);
	configure_serial = 0;
	popup_configured = 0;
	commit(pp.surface);
	while (!configure_serial)
		more("popup configure");
	put(what);
	put(": configure ");
	if (popup_configured != pp.popup) {
		put("missing");
	} else {
		put_dec(pc_x);
		put(" ");
		put_dec(pc_y);
		put(" ");
		put_dec(pc_w);
		put(" ");
		put_dec(pc_h);
	}
	end_line();
	ack(pp.xdg);
	attach_commit(pp.surface, make_buffer(pc_w, pc_h, colour));
	roundtrip();
	return pp;
}

static void destroy_popup(struct popup *pp, const char *what)
{
	destroy(pp->popup);
	destroy(pp->xdg);
	destroy(pp->surface);
	roundtrip();
	put("delete_id ");
	put(what);
	put(deleted[pp->popup] && deleted[pp->xdg] && deleted[pp->surface] ? ": yes" : ": no");
	end_line();
}

/* Repositions popup `pp` with `p` (token `token`): repositioned, then
 * configure and xdg_surface.configure, acked and committed. */
static void reposition(struct popup *pp, const struct pos *p, u32 token, const char *what)
{
	u32 positioner = make_positioner(p);
	begin(pp->popup, 2); /* reposition(positioner, token) */
	word(positioner);
	word(token);
	send(-1);
	destroy(positioner);
	configure_serial = 0;
	popup_configured = 0;
	repositioned_seen = 0;
	while (!configure_serial)
		more("reposition");
	put(what);
	put(": repositioned ");
	if (repositioned_seen)
		put_dec(repositioned_token);
	else
		put("missing");
	put(", configure ");
	put_dec(pc_x);
	put(" ");
	put_dec(pc_y);
	put(" ");
	put_dec(pc_w);
	put(" ");
	put_dec(pc_h);
	end_line();
	ack(pp->xdg);
	attach_commit(pp->surface, make_buffer(pc_w, pc_h, 0x00ffcc00));
	roundtrip();
}

/* The toplevel: 200x150, configured and mapped. */
static u32 toplevel_xdg;

static void make_toplevel(void)
{
	u32 surface = make_surface();
	toplevel_xdg = make_xdg(surface);
	u32 toplevel = new_object(TOPLEVEL);
	begin(toplevel_xdg, 1); /* get_toplevel */
	word(toplevel);
	send(-1);
	begin(toplevel, 2); /* set_title */
	string("wl_popup_test");
	send(-1);
	configure_serial = 0;
	commit(surface);
	while (!configure_serial)
		more("toplevel configure");
	ack(toplevel_xdg);
	attach_commit(surface, make_buffer(200, 150, 0x00336699));
	roundtrip();
}

/* Anchors and gravities (xdg_positioner's enums). */
enum { NO = 0, TOP = 1, BOTTOM = 2, LEFT = 3, RIGHT = 4, TOP_LEFT = 5, BOTTOM_LEFT = 6,
       TOP_RIGHT = 7, BOTTOM_RIGHT = 8 };
static const char *const dir_names[] = { "none", "top", "bottom", "left", "right", "top_left",
					 "bottom_left", "top_right", "bottom_right" };

/* Reads until the connection ends: the error it got printed. */
static void expect_error(const char *what)
{
	while (dispatch() > 0)
		;
	put(what);
	put(": ");
	if (error_seen)
		error_text();
	else
		put("no error");
	put(", then the end of the stream");
	end_line();
	sys1(NR_close, sock);
}

int main(int argc, char **argv, char **envp)
{
	(void)argc;
	(void)argv;
	lx_init();
	runtime_dir = env(envp, "XDG_RUNTIME_DIR");
	display_name = env(envp, "WAYLAND_DISPLAY");
	if (!runtime_dir || !display_name) {
		say_text("XDG_RUNTIME_DIR or WAYLAND_DISPLAY is not set");
		lx_exit(1);
	}
	open_connection(1);
	make_toplevel();

	/* Every anchor with gravity bottom_right, on an anchor rectangle
	 * (20, 30, 40, 10) of the toplevel: the popup's top left corner at
	 * the anchor point. */
	struct pos p = { 50, 40, 20, 30, 40, 10, NO, BOTTOM_RIGHT, 0, 0, 0, 0 };
	struct popup first;
	for (u32 a = NO; a <= BOTTOM_RIGHT; a++) {
		p.anchor = a;
		char what[64] = "anchor ";
		lx_memcpy(what + 7, dir_names[a], lx_strlen(dir_names[a]) + 1);
		struct popup pp = make_popup(toplevel_xdg, &p, what, 0x00ff8800);
		if (a == BOTTOM_RIGHT)
			first = pp;
		else
			destroy_popup(&pp, what);
	}
	/* Every gravity, anchor top_left. */
	p.anchor = TOP_LEFT;
	for (u32 g = NO; g <= BOTTOM_RIGHT; g++) {
		p.gravity = g;
		char what[64] = "gravity ";
		lx_memcpy(what + 8, dir_names[g], lx_strlen(dir_names[g]) + 1);
		struct popup pp = make_popup(toplevel_xdg, &p, what, 0x00ff8800);
		destroy_popup(&pp, what);
	}
	/* An offset (an odd size: centring rounds). */
	struct pos q = { 51, 41, 20, 30, 41, 11, NO, NO, 0, 7, -9, 1 };
	struct popup pq = make_popup(toplevel_xdg, &q, "offset, odd sizes", 0x00ff8800);
	destroy_popup(&pq, "offset");

	/* A popup of the popup: against its parent's geometry (50x40). */
	struct pos n = { 30, 20, 0, 0, 50, 40, RIGHT, RIGHT, 0, 0, 0, 0 };
	struct popup nested = make_popup(first.xdg, &n, "nested", 0x0000aa00);

	/* Reposition the nested one (version 3). */
	struct pos r = { 60, 30, 10, 10, 20, 10, BOTTOM, BOTTOM, 0, 2, 3, 0 };
	reposition(&nested, &r, 42, "reposition");

	destroy_popup(&nested, "nested");
	destroy_popup(&first, "first");

	/* No constraint adjustment (the default): the position is the
	 * positioner's, however far outside any output it is. */
	struct pos c = { 10, 10, 0, 0, 1, 1, TOP_LEFT, BOTTOM_RIGHT, 0, -100000, -100000, 0 };
	struct popup far = make_popup(toplevel_xdg, &c, "far away, no adjustment", 0x00ff0000);
	destroy_popup(&far, "far away");
	sys1(NR_close, sock);

	/* Protocol errors. */
	open_connection(0);
	{
		u32 id = new_object(POSITIONER);
		begin(wm_base, 1);
		word(id);
		send(-1);
		begin(id, 1); /* set_size(0, 10) */
		word(0);
		word(10);
		send(-1);
		expect_error("set_size 0");
	}
	open_connection(0);
	{
		u32 id = new_object(POSITIONER);
		begin(wm_base, 1);
		word(id);
		send(-1);
		begin(id, 2); /* set_anchor_rect(0, 0, -1, 5) */
		word(0);
		word(0);
		word((u32)-1);
		word(5);
		send(-1);
		expect_error("set_anchor_rect negative");
	}
	open_connection(0);
	{
		make_toplevel();
		struct pos e = { 10, 10, 0, 0, 1, 1, NO, NO, 0, 0, 0, 0 };
		struct popup a = make_popup(toplevel_xdg, &e, "parent", 0x00ff0000);
		struct popup b = make_popup(a.xdg, &e, "child", 0x00ff0000);
		popup_done_seen = 0;
		destroy(a.popup);
		roundtrip();
		say_text("parent popup destroyed first: no error");
		say_bool("its child dismissed", popup_done_seen == b.popup);
		sys1(NR_close, sock);
	}
	say_text("wl_popup_test: ok");
	lx_exit(0);
}
