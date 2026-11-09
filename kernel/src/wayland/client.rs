//! One Wayland client's protocol state (ROADMAP LD step 2; ARCHITECTURE
//! "Linux applications on the desktop (Wayland)"): its object table, the
//! requests it may send and the events they cause, apart from sockets,
//! files and windows, which the `Host` gives it (the kernel's in
//! `wayland`, a fake in the host tests).
//!
//! - **Bytes in, bytes out.** The server puts what it read in `input_room`
//!   (and each descriptor that came with it through `push_fd`), then
//!   `dispatch` handles every whole message: decoded by `wl_abi` against
//!   its signature, each object argument checked to exist with the
//!   interface the protocol names, then acted on. Events are appended to
//!   `output`, which the server writes to the socket.
//! - **Objects** (`Object`): ids the client chose, kept in a table indexed
//!   by id, as libwayland keeps them: a new id is the next unused one or
//!   a free one below it, at most `MAX_OBJECTS`. Destroying one (a
//!   destructor request, or a callback the server is done with) frees its
//!   id and answers `wl_display.delete_id`.
//! - **Errors**: anything the protocol does not allow (a malformed message,
//!   an unknown object or opcode, a request newer than the object's
//!   version, a wrong argument, an interface or request gala lacks) is
//!   answered with `wl_display.error` (object, code, message) and ends the
//!   client (`End`). Nothing here panics on a client's bytes; running out
//!   of memory is a `no_memory` error.
//! - **Globals**: wl_compositor 6, wl_shm 2 (ARGB8888 and XRGB8888),
//!   xdg_wm_base 6, wl_output 4, wl_seat 7 (pointer and keyboard) and
//!   zxdg_decoration_manager_v1 1.
//! - **Input** (`input`): the desktop's input for the client's windows
//!   (`input::Input`, from the window messages the desktop queues) becomes
//!   wl_pointer and wl_keyboard events on every pointer and keyboard the
//!   client made, and xdg_toplevel.close. The seat's state (`Seat`) is the
//!   client's: the surface with the pointer and where, the buttons held
//!   (the pointer stays with its surface while one is: an implicit grab),
//!   the surface with the keyboard focus (the desktop's active window),
//!   the keys down and the modifiers they hold or toggled. A key already
//!   down is not sent again (the client repeats keys itself:
//!   repeat_info). A point outside the surface (a client area larger than
//!   the buffer) is outside: the pointer leaves. A surface unmapped with a
//!   focus gets its leave; one destroyed loses it without one.
//! - **Decorations**: a toplevel whose decoration object asks for
//!   nothing or for server-side wears shasha's frame (mode server_side);
//!   one that asks for client-side gets it, and a frameless window, as
//!   does a toplevel without a decoration object (a client that does not
//!   negotiate decorates itself: xdg-decoration's description; GTK 3). The mode is fixed when the window is made: asked
//!   for again while mapped, the configure says the window's mode; the
//!   next mapping (after a null buffer) follows the new one.
//! - **Surfaces**: `attach`, `damage`/`damage_buffer` and `frame` are
//!   pending until `commit`. An xdg_toplevel's first commit (without a
//!   buffer) is answered with its configure (configure_bounds and
//!   wm_capabilities first where its version has them, then
//!   `xdg_toplevel.configure` 0x0 and `xdg_surface.configure`); once it is
//!   acked, a commit with a buffer maps it as a window
//!   (`Host::window_create`, titled by `set_title`), copies the damaged
//!   rows (all of them for a new or resized window) from the buffer's
//!   pool and releases the buffer at once (the pixels are copied); a
//!   commit with a null buffer unmaps it, and the next one configures it
//!   again. Frame callbacks are done at the refresh after their commit
//!   was applied (`refresh`, which the server calls at 60 Hz while any
//!   are due): the commit is presented at once, and a client drawing on
//!   its frame callbacks draws at the display's rate, not as fast as it
//!   can.
//! - **Popups** (ARCHITECTURE "Popups"; `popup` places them): an
//!   xdg_positioner's rules are copied by get_popup and reposition; the
//!   first commit is answered with xdg_popup.configure (the place relative
//!   to the parent's window geometry, adjusted against the work area) and
//!   xdg_surface.configure; once acked a buffer shows a popup window
//!   (`Host::popup_create`) there. A grab (the last press's serial, the
//!   topmost grabbing popup or a toplevel as parent) joins `grabs`: once
//!   shown, the topmost has the keyboard focus and the desktop's popup
//!   grab (`Host::grab`); the desktop's PopupDone, or the window losing
//!   the activation, dismisses them topmost first (popup_done), and only
//!   the topmost may be destroyed (not_the_topmost_popup).
//! - **Accepted and not acted on** (`Note::Ignored`, ledgered by the
//!   server): regions, buffer scale and transform other than 1 and
//!   normal, surface offsets, a toplevel's window geometry (a popup's
//!   places it), a positioner's set_reactive, set_parent_size and
//!   set_parent_configure, and the toplevel's
//!   requests for move, resize, menus, min and max sizes and fullscreen;
//!   a cursor surface (wl_pointer.set_cursor: the desktop draws its own
//!   pointer). A window message the desktop sends that a Wayland window
//!   does not follow is noted too (`Note::Unhandled`).
//! - **The desktop's sizing** (ARCHITECTURE "Configure from the
//!   desktop"): maximize, restore and place (Snap) set the toplevel's
//!   `Sizing` and send a configure of that surface size and states
//!   (maximized, activated with the keyboard focus, tiled from version
//!   2); the window moves to the desktop's place at the commit after the
//!   client acked it (`XdgSurface::unacked`, `place`). The client's
//!   set_maximized, unset_maximized and set_minimized are the desktop's
//!   actions (`Host::window_act`), answered the same way.

use alloc::collections::VecDeque;
use alloc::vec::Vec;
use core::fmt::{self, Write as _};

use wl_abi::{Arg, Header, Interface, MAX_ARGS, MAX_FDS, MAX_MESSAGE, Message};

use super::input::{self, Input, Modifier};
use super::popup::{self, Rules};
use crate::mm::heap::TryVec;

/// The most objects a client has (and the highest id it may use).
pub const MAX_OBJECTS: u32 = 4096;
/// The most descriptors waiting for the messages that take them.
pub const MAX_QUEUED_FDS: usize = 2 * MAX_FDS;
/// The input buffer: whole messages are handled as soon as they are in.
pub const INPUT: usize = 4 * MAX_MESSAGE;
/// Events not yet taken by the client beyond this end it (libwayland
/// disconnects a client whose buffer is full too).
pub const MAX_OUTPUT: usize = 256 * 1024;
/// The largest shm pool (a pool is a view of the client's own file;
/// this bounds what one buffer can make the server read).
pub const MAX_POOL: u64 = 256 << 20;
/// A title or app id is kept up to this many bytes.
const TITLE_MAX: usize = 256;
/// Rows copied from a buffer at a time.
const ROWS_AT_ONCE: u32 = 64;
/// Ids from here on are the server's (none are made yet).
const SERVER_IDS: u32 = 0xff00_0000;

const fn same(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut i = 0;
    while i < a.len() {
        if a[i] != b[i] {
            return false;
        }
        i += 1;
    }
    true
}

/// The interface called `name` (at compile time: a missing one does not
/// build).
const fn find(name: &str) -> &'static Interface {
    let all = wl_abi::INTERFACES;
    let mut i = 0;
    while i < all.len() {
        if same(all[i].name.as_bytes(), name.as_bytes()) {
            return &all[i];
        }
        i += 1;
    }
    panic!("an interface the protocol XML does not have")
}

pub static WL_DISPLAY: &Interface = find("wl_display");
pub static WL_REGISTRY: &Interface = find("wl_registry");
pub static WL_CALLBACK: &Interface = find("wl_callback");
pub static WL_COMPOSITOR: &Interface = find("wl_compositor");
pub static WL_SHM: &Interface = find("wl_shm");
pub static WL_SHM_POOL: &Interface = find("wl_shm_pool");
pub static WL_BUFFER: &Interface = find("wl_buffer");
pub static WL_SURFACE: &Interface = find("wl_surface");
pub static WL_REGION: &Interface = find("wl_region");
pub static WL_OUTPUT: &Interface = find("wl_output");
pub static XDG_WM_BASE: &Interface = find("xdg_wm_base");
pub static XDG_SURFACE: &Interface = find("xdg_surface");
pub static XDG_TOPLEVEL: &Interface = find("xdg_toplevel");
pub static WL_SEAT: &Interface = find("wl_seat");
pub static WL_POINTER: &Interface = find("wl_pointer");
pub static WL_KEYBOARD: &Interface = find("wl_keyboard");
pub static DECORATION_MANAGER: &Interface = find("zxdg_decoration_manager_v1");
pub static TOPLEVEL_DECORATION: &Interface = find("zxdg_toplevel_decoration_v1");
pub static XDG_POSITIONER: &Interface = find("xdg_positioner");
pub static XDG_POPUP: &Interface = find("xdg_popup");
pub static WL_DATA_DEVICE_MANAGER: &Interface = find("wl_data_device_manager");
pub static WL_DATA_SOURCE: &Interface = find("wl_data_source");
pub static WL_DATA_DEVICE: &Interface = find("wl_data_device");

/// The globals offered, in registry order (name = index + 1), with the
/// version offered.
pub fn globals() -> [(&'static Interface, u32); 7] {
    [
        (WL_COMPOSITOR, 6),
        (WL_SHM, 2),
        (XDG_WM_BASE, 6),
        (WL_OUTPUT, 4),
        // As weston offers it: axis_discrete, not axis_value120.
        (WL_SEAT, 7),
        (DECORATION_MANAGER, 1),
        // GTK 3 adds a seat only once this is there too.
        (WL_DATA_DEVICE_MANAGER, 3),
    ]
}

// Error codes (the XML's enums).
const DISPLAY_INVALID_OBJECT: u32 = 0;
const DISPLAY_INVALID_METHOD: u32 = 1;
const DISPLAY_NO_MEMORY: u32 = 2;
const DISPLAY_IMPLEMENTATION: u32 = 3;
const SHM_INVALID_FORMAT: u32 = 0;
const SHM_INVALID_STRIDE: u32 = 1;
const SHM_INVALID_FD: u32 = 2;
const SURFACE_INVALID_SCALE: u32 = 0;
const SURFACE_INVALID_TRANSFORM: u32 = 1;
const SURFACE_INVALID_OFFSET: u32 = 3;
const WM_BASE_ROLE: u32 = 0;
const WM_BASE_NOT_THE_TOPMOST_POPUP: u32 = 2;
const WM_BASE_INVALID_POPUP_PARENT: u32 = 3;
const WM_BASE_INVALID_SURFACE_STATE: u32 = 4;
const WM_BASE_INVALID_POSITIONER: u32 = 5;
const POSITIONER_INVALID_INPUT: u32 = 0;
const POPUP_INVALID_GRAB: u32 = 0;
const XDG_SURFACE_NOT_CONSTRUCTED: u32 = 1;
const XDG_SURFACE_ALREADY_CONSTRUCTED: u32 = 2;
const XDG_SURFACE_UNCONFIGURED_BUFFER: u32 = 3;
const XDG_SURFACE_INVALID_SERIAL: u32 = 4;
const XDG_SURFACE_INVALID_SIZE: u32 = 5;
const XDG_SURFACE_DEFUNCT_ROLE_OBJECT: u32 = 6;
const SEAT_MISSING_CAPABILITY: u32 = 0;
const POINTER_ROLE: u32 = 0;
const DECORATION_UNCONFIGURED_BUFFER: u32 = 0;
const DECORATION_ALREADY_CONSTRUCTED: u32 = 1;
const DECORATION_ORPHANED: u32 = 2;
const DECORATION_INVALID_MODE: u32 = 3;
const DATA_SOURCE_INVALID_ACTION_MASK: u32 = 0;
const DATA_SOURCE_INVALID_SOURCE: u32 = 1;
const DATA_DEVICE_ROLE: u32 = 0;
const DATA_DEVICE_USED_SOURCE: u32 = 1;
/// wl_data_device_manager's dnd_action bits: copy, move, ask.
const DND_ACTIONS: u32 = 7;

/// xdg_toplevel's states (the configure's array).
pub const STATE_MAXIMIZED: u32 = 1;
pub const STATE_ACTIVATED: u32 = 4;
pub const STATE_TILED_LEFT: u32 = 5;
pub const STATE_TILED_RIGHT: u32 = 6;
pub const STATE_TILED_TOP: u32 = 7;
pub const STATE_TILED_BOTTOM: u32 = 8;
/// xdg_toplevel's wm_capabilities: the desktop maximizes and minimizes.
const CAPABILITY_MAXIMIZE: u32 = 2;
const CAPABILITY_MINIMIZE: u32 = 4;
/// wl_data_source.cancelled and wl_data_device.selection (event opcodes).
const DATA_SOURCE_CANCELLED: u16 = 2;
const DATA_DEVICE_SELECTION: u16 = 5;
/// Configures sent and not acked kept per xdg_surface (an ack names one
/// of them); beyond, the oldest is forgotten.
pub const MAX_CONFIGURES: usize = 16;

/// zxdg_toplevel_decoration_v1's modes.
pub const MODE_CLIENT_SIDE: u32 = 1;
pub const MODE_SERVER_SIDE: u32 = 2;
/// wl_seat's capabilities: pointer and keyboard.
const CAPABILITIES: u32 = 3;
/// The keys down kept per client (more held at once are not sent).
const MAX_KEYS: usize = 32;
/// Descriptors the events hold until written (wl_keyboard.keymap's).
const MAX_OUT_FDS: usize = 2 * MAX_FDS;

// wl_shm formats.
pub const ARGB8888: u32 = 0;
pub const XRGB8888: u32 = 1;

/// What the client's protocol state needs from outside.
pub trait Host {
    /// A file descriptor the client passed (an open file description).
    type Fd;
    /// Whether `fd` is a file whose bytes can be read (a pool's memory).
    fn readable(&mut self, fd: &Self::Fd) -> bool;
    /// Fills `out` from `fd` at `offset`: false if not all of it is there.
    fn read(&mut self, fd: &Self::Fd, offset: u64, out: &mut [u8]) -> bool;
    /// A copy of `fd` (another reference to the same description).
    fn share(&mut self, fd: &Self::Fd) -> Option<Self::Fd>;
    /// The screen's size (wl_output's mode, configure_bounds).
    fn screen(&mut self) -> (u32, u32);
    /// Milliseconds (wl_callback.done of a frame).
    fn now_ms(&mut self) -> u32;
    /// A new window showing a `width` x `height` surface, titled `title`
    /// (UTF-8 as the client gave it): its id, None if it cannot be made.
    /// The keymap (XKB text with its NUL) as a description of a sealed
    /// read-only file, and its size; None without memory.
    fn keymap(&mut self) -> Option<(Self::Fd, u32)>;
    /// `decorated`: with shasha's frame (else frameless: the client draws
    /// its own).
    fn window_create(
        &mut self,
        title: &[u8],
        width: u32,
        height: u32,
        decorated: bool,
    ) -> Option<u32>;
    /// A frameless popup window (above the framed windows, never active,
    /// not on the taskbar: what user32's menus are) at screen (x, y)
    /// showing a `width` x `height` surface: its id, None if it cannot be
    /// made.
    fn popup_create(&mut self, x: i32, y: i32, width: u32, height: u32) -> Option<u32>;
    /// The screen position of the window's client area (where its surface
    /// is).
    fn client_origin(&mut self, id: u32) -> Option<(i32, i32)>;
    /// The desktop's popup grab: window `id`, the topmost grabbing
    /// popup's (a press outside the client's windows ends it: input
    /// PopupDone); None: there is none.
    fn grab(&mut self, id: Option<u32>);
    fn window_resize(&mut self, id: u32, width: u32, height: u32, decorated: bool);
    /// Moves the window's outer rectangle to (x, y) for a `width` x
    /// `height` surface (the desktop's place, from a configure the client
    /// acked).
    fn window_place(&mut self, id: u32, x: i32, y: i32, width: u32, height: u32, decorated: bool);
    /// The window's outer rectangle (x, y, width, height).
    fn window_outer(&mut self, id: u32) -> Option<(i32, i32, u32, u32)>;
    /// The desktop does `act` to the window (the client asked for it:
    /// xdg_toplevel.set_maximized and the like); what follows comes back
    /// as input (SC_MAXIMIZE and the others).
    fn window_act(&mut self, id: u32, act: Act);
    /// The desktop's work area (the screen above the taskbar): a
    /// maximized window's outer size.
    fn work_area(&mut self) -> (u32, u32);
    /// The surface size of a window whose outer size is `width` x
    /// `height` (framed: less shasha's frame).
    fn surface_size(&mut self, width: u32, height: u32, decorated: bool) -> (u32, u32);
    /// Rows `y..y + rows` of the window's pixels (0x00RRGGBB, `width` per
    /// row).
    fn window_present(&mut self, id: u32, y: u32, width: u32, rows: u32, pixels: &[u32]);
    fn window_title(&mut self, id: u32, title: &[u8]);
    fn window_destroy(&mut self, id: u32);
    /// Something observability wants to know (events, log, ledger).
    fn note(&mut self, note: Note<'_>);
    /// Whether every request and event is to be traced.
    fn tracing(&self) -> bool;
    fn trace(&mut self, line: fmt::Arguments<'_>);
}

/// What happened, for the server's events, log and ledger.
#[derive(Debug)]
pub enum Note<'a> {
    /// A surface was given a role (`object` is the role's object).
    Role { surface: u32, role: &'static str, object: u32 },
    /// A commit applied: the size of the buffer it showed (none: no new
    /// buffer), the rows copied into the window (none: nothing damaged or
    /// no window), the window.
    Commit {
        surface: u32,
        buffer: Option<(u32, u32)>,
        rows: Option<(u32, u32)>,
        window: Option<u32>,
        /// Where the window was put (outer position) for the configure
        /// this commit answers.
        placed: Option<(i32, i32)>,
    },
    /// A toplevel's configure sent: the size asked for (0x0: the client's
    /// choice), the states (`STATE_*` bits: 1 << state), the xdg_surface's
    /// serial.
    Configure { surface: u32, width: u32, height: u32, states: u32, serial: u32 },
    /// A window was shown for a surface, or could not be made.
    Mapped { surface: u32, window: Option<u32> },
    /// A popup's life (`what`): "configure" (the place relative to the
    /// parent's window geometry and size, `rect`, the xdg_surface's
    /// serial), "repositioned" (the token as `serial`), "grab", "grab
    /// denied" (`why`), "dismissed" (`why`), "orphaned" (its parent popup
    /// destroyed first).
    Popup { surface: u32, what: &'static str, why: &'static str, rect: Rect, serial: u32 },
    /// A protocol error: wl_display.error sent, the client ends.
    Error { object: u32, interface: &'static str, code: u32, message: &'a str },
    /// A request gala does not implement (answered with an error).
    Missing { interface: &'static str, request: &'static str },
    /// A request accepted but not acted on.
    Ignored { interface: &'static str, request: &'static str },
    /// Input delivered (`what`: "enter", "leave", "button", "axis", "key",
    /// "focus", "unfocus", "close", "maximize", "restore", "place") to
    /// `surface` (window `window`):
    /// `code` the button or key, `state` pressed (1) or released (0).
    Input { what: &'static str, surface: u32, window: u32, code: u32, state: u32 },
    /// Input for a window the client has no surface showing (gone since,
    /// or never its own): dropped.
    InputDropped { window: u32 },
    /// A window message Wayland windows do not follow: ledgered.
    Unhandled { what: &'static str, window: u32 },
}

/// x, y, width, height.
pub type Rect = (i32, i32, i32, i32);

/// What a client asks the desktop to do to its window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Act {
    Maximize,
    /// Back from maximized (or snapped).
    Restore,
    Minimize,
}

/// Why a client ends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum End {
    /// A protocol error (wl_display.error is in the output).
    Error,
    /// More events waiting than MAX_OUTPUT.
    OutputFull,
}

type Result<T = ()> = core::result::Result<T, End>;

/// A short message (protocol errors), cut at `TEXT` bytes on a character
/// boundary.
const TEXT: usize = 160;

pub struct Text {
    buf: [u8; TEXT],
    len: usize,
}

impl Text {
    fn new() -> Text {
        Text { buf: [0; TEXT], len: 0 }
    }

    pub fn as_str(&self) -> &str {
        core::str::from_utf8(&self.buf[..self.len]).unwrap_or("?")
    }
}

impl fmt::Write for Text {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for c in s.chars() {
            let n = c.len_utf8();
            if self.len + n > TEXT {
                break;
            }
            c.encode_utf8(&mut self.buf[self.len..]);
            self.len += n;
        }
        Ok(())
    }
}

/// Bytes a client sent (a name, a title) as text: printable ASCII as is,
/// the rest as `\xNN`.
pub struct Bytes<'a>(pub &'a [u8]);

impl fmt::Display for Bytes<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for &b in self.0 {
            if (0x20..0x7f).contains(&b) && b != b'"' && b != b'\\' {
                f.write_char(b as char)?;
            } else {
                write!(f, "\\x{b:02x}")?;
            }
        }
        Ok(())
    }
}

struct Pool<F> {
    fd: F,
    size: u64,
}

struct Buffer<F> {
    fd: F,
    offset: u64,
    width: u32,
    height: u32,
    stride: u32,
}

#[derive(Default)]
struct Pending {
    /// None: no attach since the last commit; Some(None): a null buffer.
    buffer: Option<Option<u32>>,
    /// The damaged rows (buffer and surface coordinates are the same at
    /// scale 1 without a transform), from and to (exclusive).
    damage: Option<(i64, i64)>,
    frames: Vec<u32>,
}

#[derive(Default)]
struct Surface {
    pending: Pending,
    /// The window wears shasha's frame (server-side decoration).
    decorated: bool,
    /// Its xdg_surface.
    xdg: Option<u32>,
    /// The window showing it, while mapped.
    window: Option<u32>,
    /// The size of the buffer last shown.
    size: (u32, u32),
    /// The part of it the window shows (x, y, width, height in surface
    /// coordinates): a frameless window its window geometry, clamped to
    /// the buffer (`view_of`), a framed one all of it.
    view: View,
}

/// A window's part of its surface: x, y, width, height.
type View = (i32, i32, u32, u32);

/// What a window shows of a `width` x `height` buffer: a frameless one
/// (client-side decorations) its window geometry, the "visible bounds"
/// (xdg-shell), clamped to the buffer as xdg-shell clamps it: a client's
/// shadows outside it are not shown, since shasha does not compose alpha
/// (a client drawing its own frame draws them translucent, which would
/// show as black). A framed window, or one without a geometry, shows the
/// whole buffer.
fn view_of(geometry: Option<Rect>, decorated: bool, width: u32, height: u32) -> View {
    let whole = (0, 0, width, height);
    let Some((gx, gy, gw, gh)) = geometry.filter(|_| !decorated) else { return whole };
    let (w, h) = (i64::from(width), i64::from(height));
    let (x0, y0) = (i64::from(gx).clamp(0, w), i64::from(gy).clamp(0, h));
    let x1 = (i64::from(gx) + i64::from(gw)).clamp(0, w);
    let y1 = (i64::from(gy) + i64::from(gh)).clamp(0, h);
    if x1 <= x0 || y1 <= y0 {
        return whole;
    }
    (x0 as i32, y0 as i32, (x1 - x0) as u32, (y1 - y0) as u32)
}

struct XdgSurface {
    /// 0 once the wl_surface is gone.
    surface: u32,
    /// The xdg_wm_base that made it (xdg_wm_base's errors about its
    /// popup go there).
    wm_base: u32,
    /// Its xdg_toplevel.
    toplevel: Option<u32>,
    /// Its xdg_popup.
    popup: Option<u32>,
    /// The window geometry (set_window_geometry, applied at commit); None:
    /// the whole surface.
    geometry: Option<Rect>,
    pending_geometry: Option<Rect>,
    /// The serial of the last configure sent.
    sent: Option<u32>,
    /// A configure was acked since the role was given (or since unmapped).
    acked: bool,
    /// The configures sent and not acked yet, oldest first: serial and
    /// the outer position the desktop asked for with it (a popup's: its
    /// place relative to its parent's window geometry) (at most
    /// MAX_CONFIGURES). An ack may name any of them (the client need ack
    /// only the last one it saw); it drops those before it too.
    unacked: Vec<(u32, Option<(i32, i32)>)>,
    /// The position of the configure acked, applied at the next commit
    /// of a buffer (with the buffer's size).
    place: Option<(i32, i32)>,
}

struct Toplevel {
    xdg: u32,
    /// Its zxdg_toplevel_decoration_v1.
    decoration: Option<u32>,
    title: Vec<u8>,
    app_id: Vec<u8>,
    /// What the desktop asks for: the surface size in each configure
    /// (0x0, the client's choice, until the desktop sizes the window), its
    /// states, and where the window goes.
    sizing: Sizing,
}

/// A toplevel's size and states as the desktop set them (maximize, Snap,
/// a place); activated follows the keyboard focus.
#[derive(Clone, Copy, Default, Debug)]
struct Sizing {
    size: (u32, u32),
    maximized: bool,
    /// STATE_TILED_* as bits (1 << state): sent from xdg_toplevel 2.
    tiled: u32,
    /// The outer rectangle a maximized window goes back to.
    restore: Option<(i32, i32, u32, u32)>,
    /// The outer position the desktop put the window at, until a commit
    /// applied it.
    target: Option<(i32, i32)>,
}

/// An xdg_popup (ARCHITECTURE "Popups").
struct Popup {
    xdg: u32,
    /// The parent's xdg_surface (0: none given, or destroyed since: an
    /// orphan keeps its window where it was).
    parent: u32,
    /// The rules it was placed by (the positioner's, copied).
    rules: Rules,
    /// The last configure: place relative to the parent's window geometry
    /// and size.
    configured: Rect,
    /// The place applied (from the configure acked, at the commit after).
    at: Option<(i32, i32)>,
    /// The parent's window geometry's screen position when last placed.
    origin: Option<(i32, i32)>,
    /// Where its window is (screen), while shown.
    shown_at: Option<(i32, i32)>,
    /// It asked for a grab and got it (it is in the client's `grabs`).
    grab: bool,
    /// The compositor dismissed it (popup_done sent): unmapped for good.
    dismissed: bool,
}

enum Kind<F> {
    Display,
    Registry,
    Callback,
    Compositor,
    Shm,
    Pool(Pool<F>),
    Buffer(Buffer<F>),
    Surface(Surface),
    Region,
    Output,
    WmBase,
    XdgSurface(XdgSurface),
    Toplevel(Toplevel),
    Positioner(Rules),
    Popup(Popup),
    Seat,
    Pointer,
    Keyboard,
    DecorationManager,
    Decoration(Decoration),
    DataDeviceManager,
    DataSource(DataSource),
    DataDevice,
}

/// A wl_data_source: what it offers (counted, not kept: nothing reads it
/// yet), whether set_selection or start_drag used it, its drag actions.
#[derive(Default)]
struct DataSource {
    mime_types: u32,
    used: bool,
    actions: Option<u32>,
}

struct Decoration {
    toplevel: u32,
    /// The mode the client asked for (None: no preference).
    wanted: Option<u32>,
}

/// The client's seat: where its pointer and keyboard focus are.
#[derive(Default)]
struct Seat {
    /// The surface the pointer is on, and where (surface coordinates).
    pointer: Option<u32>,
    at: (i32, i32),
    /// Buttons held (1 left, 2 right): the pointer stays with its surface.
    buttons: u32,
    /// The surface with the keyboard focus: the topmost grabbing popup's
    /// while there is one, else the active toplevel's.
    keyboard: Option<u32>,
    /// The surface whose window is the desktop's active one (a toplevel's:
    /// its configures say activated).
    active: Option<u32>,
    /// The serial of the last button or key press (a popup's grab names
    /// it).
    press: Option<u32>,
    /// The keys down (evdev codes), at most MAX_KEYS.
    keys: Vec<u32>,
    /// The modifiers held by those keys, and those toggled (Caps Lock, Num
    /// Lock): wl_keyboard.modifiers' depressed and locked masks.
    depressed: u32,
    locked: u32,
}

struct Object<F> {
    iface: &'static Interface,
    version: u32,
    kind: Kind<F>,
}

/// A client's protocol state (see the module's doc).
pub struct Client<F> {
    objects: Vec<Option<Object<F>>>,
    input: Vec<u8>,
    filled: usize,
    fds: VecDeque<F>,
    output: Vec<u8>,
    /// Descriptors going with the events (wl_keyboard.keymap's), each with
    /// where its event starts in `output`.
    out_fds: VecDeque<(usize, F)>,
    seat: Seat,
    serial: u32,
    /// The client's selection (`wl_data_device.set_selection`'s source):
    /// kept for itself, offered to no other client yet.
    selection: Option<u32>,
    /// The popups that grabbed, bottom (the toplevel's) first: the last
    /// is the topmost, the only one the client may destroy, and the one
    /// with the keyboard focus once shown.
    grabs: Vec<u32>,
    /// Frame callbacks whose commit was applied, waiting for the next
    /// refresh (`refresh`).
    due: Vec<u32>,
    /// Requests handled (the state dump).
    pub requests: u64,
    /// Commits that showed a buffer, and frame callbacks done (the state
    /// dump, the connection's summary).
    pub commits: u64,
    pub frames: u64,
}

/// Arguments as WAYLAND_DEBUG=1 shows them.
struct ShowArgs<'a, F> {
    objects: &'a [Option<Object<F>>],
    sig: &'static Message,
    args: &'a [Arg<'a>],
}

fn fixed(f: &mut fmt::Formatter<'_>, v: i32) -> fmt::Result {
    let sign = if v < 0 { "-" } else { "" };
    let a = v.unsigned_abs();
    write!(f, "{sign}{}.{:03}", a >> 8, (a & 0xff) * 1000 / 256)
}

impl<F> fmt::Display for ShowArgs<'_, F> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, (arg, spec)) in self.args.iter().zip(self.sig.args).enumerate() {
            if i > 0 {
                f.write_str(", ")?;
            }
            match *arg {
                Arg::Int(v) => write!(f, "{v}")?,
                Arg::Uint(v) => write!(f, "{v}")?,
                Arg::Fixed(v) => fixed(f, v)?,
                Arg::String(Some(s)) => write!(f, "\"{}\"", Bytes(s))?,
                Arg::String(None) | Arg::Object(None) => f.write_str("nil")?,
                Arg::Object(Some(id)) => {
                    let name = self
                        .objects
                        .get(id as usize)
                        .and_then(|o| o.as_ref())
                        .map_or("[unknown]", |o| o.iface.name);
                    write!(f, "{name}#{id}")?
                }
                Arg::NewId(id) => {
                    write!(f, "new id {}#{id}", spec.interface.unwrap_or("[unknown]"))?
                }
                Arg::UntypedNewId { interface, version, id } => {
                    let name = Bytes(interface);
                    write!(f, "\"{name}\", {version}, new id {name}#{id}")?
                }
                Arg::Array(a) => write!(f, "array[{}]", a.len())?,
                Arg::Fd => f.write_str("fd")?,
            }
        }
        Ok(())
    }
}

impl<F> Client<F> {
    /// A client with only wl_display (id 1).
    pub fn new() -> core::result::Result<Client<F>, crate::mm::heap::AllocError> {
        let mut input = Vec::new();
        input.try_reserve_exact(INPUT).map_err(|_| crate::mm::heap::AllocError)?;
        input.resize(INPUT, 0);
        let mut objects = Vec::new();
        objects.try_reserve(16).map_err(|_| crate::mm::heap::AllocError)?;
        objects.try_push(None)?;
        objects.try_push(Some(Object { iface: WL_DISPLAY, version: 1, kind: Kind::Display }))?;
        Ok(Client {
            objects,
            input,
            filled: 0,
            fds: VecDeque::new(),
            output: Vec::new(),
            out_fds: VecDeque::new(),
            seat: Seat::default(),
            serial: 0,
            selection: None,
            grabs: Vec::new(),
            due: Vec::new(),
            requests: 0,
            commits: 0,
            frames: 0,
        })
    }

    /// Where the next bytes read go (never empty: a message is at most a
    /// quarter of it, and whole ones are taken out after each read).
    pub fn input_room(&mut self) -> &mut [u8] {
        &mut self.input[self.filled..]
    }

    /// `n` bytes were read into `input_room`.
    pub fn filled(&mut self, n: usize) {
        self.filled = (self.filled + n).min(self.input.len());
    }

    /// The events waiting to be written.
    pub fn output(&self) -> &[u8] {
        &self.output
    }

    /// `n` bytes of the output were written.
    pub fn written(&mut self, n: usize) {
        let n = n.min(self.output.len());
        self.output.copy_within(n.., 0);
        self.output.truncate(self.output.len() - n);
        for (at, _) in self.out_fds.iter_mut() {
            *at = at.saturating_sub(n);
        }
    }

    /// The next descriptor whose event starts in the first `upto` bytes of
    /// the output: it goes with a write of them (rights that come with
    /// bytes before their message are kept by the client until the
    /// message that takes them; never after it).
    pub fn take_fd(&mut self, upto: usize) -> Option<F> {
        if self.out_fds.front().is_some_and(|(at, _)| *at < upto) {
            return self.out_fds.pop_front().map(|(_, f)| f);
        }
        None
    }

    /// How many objects it has.
    pub fn object_count(&self) -> usize {
        self.objects.iter().flatten().count()
    }

    /// A descriptor that came with the bytes read last.
    pub fn push_fd<H: Host<Fd = F>>(&mut self, host: &mut H, fd: F) -> Result {
        if self.fds.len() >= MAX_QUEUED_FDS {
            return Err(self.fail(
                host,
                1,
                DISPLAY_IMPLEMENTATION,
                format_args!("more than {MAX_QUEUED_FDS} file descriptors waiting"),
            ));
        }
        if self.fds.try_reserve(1).is_err() {
            return Err(self.out_of_memory(host));
        }
        self.fds.push_back(fd);
        Ok(())
    }

    /// The client is gone: its windows go (its objects with it).
    pub fn end<H: Host<Fd = F>>(&mut self, host: &mut H) {
        for o in self.objects.iter_mut().flatten() {
            if let Kind::Surface(s) = &mut o.kind
                && let Some(w) = s.window.take()
            {
                host.window_destroy(w);
            }
        }
        self.objects.clear();
        self.fds.clear();
        self.out_fds.clear();
        self.due.clear();
        self.grabs.clear();
        self.selection = None;
    }

    /// Its objects for the state dump, one per line.
    pub fn describe(&self, out: &mut dyn FnMut(fmt::Arguments<'_>)) {
        for (id, o) in self.objects.iter().enumerate() {
            let Some(o) = o else { continue };
            let (name, v) = (o.iface.name, o.version);
            match &o.kind {
                Kind::Surface(s) => out(format_args!(
                    "{name}#{id} v{v}: window {:?}, size {}x{}, view {:?}, xdg {:?}, {} frame callbacks pending",
                    s.window,
                    s.size.0,
                    s.size.1,
                    s.view,
                    s.xdg,
                    s.pending.frames.len()
                )),
                Kind::Pool(p) => out(format_args!("{name}#{id} v{v}: {} bytes", p.size)),
                Kind::Buffer(b) => out(format_args!(
                    "{name}#{id} v{v}: {}x{}, stride {}, offset {}",
                    b.width, b.height, b.stride, b.offset
                )),
                Kind::XdgSurface(x) => out(format_args!(
                    "{name}#{id} v{v}: surface {}, toplevel {:?}, popup {:?}, geometry {:?}, configure sent {:?}, acked {}, unacked {:?}, place {:?}",
                    x.surface, x.toplevel, x.popup, x.geometry, x.sent, x.acked, x.unacked, x.place
                )),
                Kind::Positioner(r) => out(format_args!("{name}#{id} v{v}: {r:?}")),
                Kind::Popup(p) => out(format_args!(
                    "{name}#{id} v{v}: xdg_surface {}, parent {}, {:?}, configured {:?}, at {:?}, parent at {:?}, shown at {:?}, grab {}, dismissed {}",
                    p.xdg,
                    p.parent,
                    p.rules,
                    p.configured,
                    p.at,
                    p.origin,
                    p.shown_at,
                    p.grab,
                    p.dismissed
                )),
                Kind::Toplevel(t) => out(format_args!(
                    "{name}#{id} v{v}: \"{}\" (app id \"{}\"), decoration {:?}, {:?}",
                    Bytes(&t.title),
                    Bytes(&t.app_id),
                    t.decoration,
                    t.sizing
                )),
                Kind::Decoration(d) => out(format_args!(
                    "{name}#{id} v{v}: toplevel {}, mode asked {:?}",
                    d.toplevel, d.wanted
                )),
                Kind::DataSource(d) => out(format_args!(
                    "{name}#{id} v{v}: {} mime types, used {}, actions {:?}, selection {}",
                    d.mime_types,
                    d.used,
                    d.actions,
                    self.selection == Some(id as u32)
                )),
                _ => out(format_args!("{name}#{id} v{v}")),
            }
        }
        let st = &self.seat;
        out(format_args!(
            "seat: pointer on {:?} at {:?}, buttons {:#x}; keyboard on {:?}, keys {:?}, modifiers depressed {:#x} locked {:#x}; active {:?}; last press serial {:?}; popup grabs {:?}",
            st.pointer,
            st.at,
            st.buttons,
            st.keyboard,
            st.keys,
            st.depressed,
            st.locked,
            st.active,
            st.press,
            self.grabs
        ));
    }

    fn next_serial(&mut self) -> u32 {
        self.serial = self.serial.wrapping_add(1);
        self.serial
    }

    fn object(&self, id: u32) -> Option<&Object<F>> {
        self.objects.get(id as usize).and_then(|o| o.as_ref())
    }

    fn object_mut(&mut self, id: u32) -> Option<&mut Object<F>> {
        self.objects.get_mut(id as usize).and_then(|o| o.as_mut())
    }

    fn surface(&mut self, id: u32) -> Option<&mut Surface> {
        match self.object_mut(id) {
            Some(Object { kind: Kind::Surface(s), .. }) => Some(s),
            _ => None,
        }
    }

    fn xdg(&mut self, id: u32) -> Option<&mut XdgSurface> {
        match self.object_mut(id) {
            Some(Object { kind: Kind::XdgSurface(x), .. }) => Some(x),
            _ => None,
        }
    }

    fn toplevel(&mut self, id: u32) -> Option<&mut Toplevel> {
        match self.object_mut(id) {
            Some(Object { kind: Kind::Toplevel(t), .. }) => Some(t),
            _ => None,
        }
    }

    /// A protocol error on `object`: wl_display.error goes out, the client
    /// ends.
    fn fail<H: Host<Fd = F>>(
        &mut self,
        host: &mut H,
        object: u32,
        code: u32,
        message: fmt::Arguments<'_>,
    ) -> End {
        let mut text = Text::new();
        let _ = text.write_fmt(message);
        let interface = self.object(object).map_or("?", |o| o.iface.name);
        host.note(Note::Error { object, interface, code, message: text.as_str() });
        let args = [
            Arg::Object(Some(object)),
            Arg::Uint(code),
            Arg::String(Some(text.as_str().as_bytes())),
        ];
        // The client ends whether or not the error fits.
        let _ = self.event(host, 1, WL_DISPLAY, 0, &args);
        End::Error
    }

    fn out_of_memory<H: Host<Fd = F>>(&mut self, host: &mut H) -> End {
        self.fail(host, 1, DISPLAY_NO_MEMORY, format_args!("no memory"))
    }

    /// Appends event `opcode` of `iface` on `object`.
    fn event<H: Host<Fd = F>>(
        &mut self,
        host: &mut H,
        object: u32,
        iface: &'static Interface,
        opcode: u16,
        args: &[Arg<'_>],
    ) -> Result {
        let Some(sig) = iface.events.get(usize::from(opcode)) else { return Err(End::Error) };
        let mut buf = [0u8; MAX_MESSAGE];
        // Our own events fit their signatures; a title or message too long
        // for a message is cut by the caller.
        let Ok((n, _)) = wl_abi::encode(object, opcode, sig, args, &mut buf) else {
            return Err(End::Error);
        };
        if self.output.len() + n > MAX_OUTPUT {
            return Err(End::OutputFull);
        }
        if self.output.try_extend_from_slice(&buf[..n]).is_err() {
            return Err(End::OutputFull);
        }
        if host.tracing() {
            let shown = ShowArgs { objects: &self.objects, sig, args };
            host.trace(format_args!(" -> {}#{object}.{}({shown})", iface.name, sig.name));
        }
        Ok(())
    }

    /// Appends an event carrying descriptor `fd` (its only fd argument):
    /// the descriptor waits in `out_fds` for the write of its bytes.
    fn event_fd<H: Host<Fd = F>>(
        &mut self,
        host: &mut H,
        object: u32,
        iface: &'static Interface,
        opcode: u16,
        args: &[Arg<'_>],
        fd: F,
    ) -> Result {
        if self.out_fds.len() >= MAX_OUT_FDS {
            return Err(End::OutputFull);
        }
        if self.out_fds.try_reserve(1).is_err() {
            return Err(self.out_of_memory(host));
        }
        let at = self.output.len();
        self.event(host, object, iface, opcode, args)?;
        self.out_fds.push_back((at, fd));
        Ok(())
    }

    /// Sends event `opcode` of `iface` to each of the client's objects of
    /// that interface whose version is in `since..until`.
    fn send_all<H: Host<Fd = F>>(
        &mut self,
        host: &mut H,
        iface: &'static Interface,
        opcode: u16,
        (since, until): (u32, u32),
        args: &[Arg<'_>],
    ) -> Result {
        for i in 1..self.objects.len() {
            let v = match &self.objects[i] {
                Some(o) if core::ptr::eq(o.iface, iface) => o.version,
                _ => continue,
            };
            if v >= since && v < until {
                self.event(host, i as u32, iface, opcode, args)?;
            }
        }
        Ok(())
    }

    /// Frees `id` and tells the client (wl_display.delete_id).
    fn delete<H: Host<Fd = F>>(&mut self, host: &mut H, id: u32) -> Result {
        if let Some(slot) = self.objects.get_mut(id as usize) {
            *slot = None;
        }
        self.event(host, 1, WL_DISPLAY, 1, &[Arg::Uint(id)])
    }

    /// Makes object `id` (a new_id the client chose).
    fn insert<H: Host<Fd = F>>(
        &mut self,
        host: &mut H,
        id: u32,
        iface: &'static Interface,
        version: u32,
        kind: Kind<F>,
    ) -> Result {
        if id >= SERVER_IDS {
            return Err(self.fail(
                host,
                1,
                DISPLAY_INVALID_OBJECT,
                format_args!("invalid new id {id}"),
            ));
        }
        if id > MAX_OBJECTS {
            return Err(self.fail(
                host,
                1,
                DISPLAY_NO_MEMORY,
                format_args!("new id {id}: at most {MAX_OBJECTS} objects"),
            ));
        }
        let object = Some(Object { iface, version, kind });
        let i = id as usize;
        let len = self.objects.len();
        match self.objects.get_mut(i) {
            Some(slot @ None) => *slot = object,
            Some(Some(_)) => {
                return Err(self.fail(
                    host,
                    1,
                    DISPLAY_INVALID_OBJECT,
                    format_args!("new id {id} is in use"),
                ));
            }
            None if i == len => {
                if self.objects.try_push(object).is_err() {
                    return Err(self.out_of_memory(host));
                }
            }
            None => {
                return Err(self.fail(
                    host,
                    1,
                    DISPLAY_INVALID_OBJECT,
                    format_args!("invalid new id {id}"),
                ));
            }
        }
        Ok(())
    }

    /// Handles every whole message in the input.
    pub fn dispatch<H: Host<Fd = F>>(&mut self, host: &mut H) -> Result {
        let mut at = 0;
        let mut msg = [0u8; MAX_MESSAGE];
        let result = loop {
            let rest = &self.input[at..self.filled];
            if rest.len() < wl_abi::HEADER {
                break Ok(());
            }
            let header = match Header::parse(rest) {
                Ok(h) => h,
                Err(e) => {
                    break Err(self.fail(
                        host,
                        1,
                        DISPLAY_INVALID_METHOD,
                        format_args!("malformed message header ({e:?})"),
                    ));
                }
            };
            let size = usize::from(header.size);
            if rest.len() < size {
                break Ok(());
            }
            msg[..size].copy_from_slice(&rest[..size]);
            at += size;
            self.requests += 1;
            if let Err(e) = self.request(host, header, &msg[..size]) {
                break Err(e);
            }
        };
        self.input.copy_within(at..self.filled, 0);
        self.filled -= at;
        result
    }

    /// One request.
    fn request<H: Host<Fd = F>>(&mut self, host: &mut H, header: Header, msg: &[u8]) -> Result {
        let id = header.object;
        let Some(o) = self.object(id) else {
            return Err(self.fail(
                host,
                1,
                DISPLAY_INVALID_OBJECT,
                format_args!("invalid object {id}"),
            ));
        };
        let (iface, version) = (o.iface, o.version);
        let op = header.opcode;
        let sig = match iface.requests.get(usize::from(op)) {
            Some(sig) if sig.since <= version => sig,
            _ => {
                return Err(self.fail(
                    host,
                    1,
                    DISPLAY_INVALID_METHOD,
                    format_args!("invalid method {op}, object {}@{id}", iface.name),
                ));
            }
        };
        let mut args = [Arg::Fd; MAX_ARGS];
        let used = match wl_abi::decode(msg, sig, self.fds.len(), &mut args) {
            Ok((_, used)) => used,
            Err(e) => {
                return Err(self.fail(
                    host,
                    1,
                    DISPLAY_INVALID_METHOD,
                    format_args!("invalid arguments for {}@{id}.{} ({e:?})", iface.name, sig.name),
                ));
            }
        };
        let args = &args[..sig.args.len()];
        if host.tracing() {
            let shown = ShowArgs { objects: &self.objects, sig, args };
            host.trace(format_args!("{}#{id}.{}({shown})", iface.name, sig.name));
        }
        // The descriptors it takes (only wl_shm.create_pool takes one).
        let mut fd = None;
        for _ in 0..used {
            if let Some(f) = self.fds.pop_front()
                && fd.is_none()
            {
                fd = Some(f);
            }
        }
        // Object arguments name live objects of the interface the protocol
        // gives them (libwayland checks the same).
        for (arg, spec) in args.iter().zip(sig.args) {
            if let (Arg::Object(Some(other)), Some(want)) = (*arg, spec.interface) {
                let have = self.object(other).map(|o| o.iface.name);
                if have != Some(want) {
                    return Err(self.fail(
                        host,
                        1,
                        DISPLAY_INVALID_OBJECT,
                        format_args!(
                            "invalid object {other} for {}@{id}.{}: not a {want}",
                            iface.name, sig.name
                        ),
                    ));
                }
            }
        }
        let tag = match self.object(id).map(|o| &o.kind) {
            Some(Kind::Display) => Tag::Display,
            Some(Kind::Registry) => Tag::Registry,
            Some(Kind::Callback) => Tag::Callback,
            Some(Kind::Compositor) => Tag::Compositor,
            Some(Kind::Shm) => Tag::Shm,
            Some(Kind::Pool(_)) => Tag::Pool,
            Some(Kind::Buffer(_)) => Tag::Buffer,
            Some(Kind::Surface(_)) => Tag::Surface,
            Some(Kind::Region) => Tag::Region,
            Some(Kind::Output) => Tag::Output,
            Some(Kind::WmBase) => Tag::WmBase,
            Some(Kind::XdgSurface(_)) => Tag::XdgSurface,
            Some(Kind::Toplevel(_)) => Tag::Toplevel,
            Some(Kind::Positioner(_)) => Tag::Positioner,
            Some(Kind::Popup(_)) => Tag::Popup,
            Some(Kind::Seat) => Tag::Seat,
            Some(Kind::Pointer) => Tag::Pointer,
            Some(Kind::Keyboard) => Tag::Keyboard,
            Some(Kind::DecorationManager) => Tag::DecorationManager,
            Some(Kind::Decoration(_)) => Tag::Decoration,
            Some(Kind::DataDeviceManager) => Tag::DataDeviceManager,
            Some(Kind::DataSource(_)) => Tag::DataSource,
            Some(Kind::DataDevice) => Tag::DataDevice,
            None => return Ok(()),
        };
        let r = Req { id, version, iface, sig, op, args, fd };
        match tag {
            Tag::Display => self.display(host, r),
            Tag::Registry => self.registry(host, r),
            // wl_callback has no requests: refused above.
            Tag::Callback => Ok(()),
            Tag::Compositor => self.compositor(host, r),
            Tag::Shm => self.shm(host, r),
            Tag::Pool => self.pool(host, r),
            Tag::Buffer => self.buffer(host, r),
            Tag::Surface => self.surface_request(host, r),
            Tag::Region | Tag::Output => {
                // Regions are accepted and not used yet; wl_output's only
                // request is release. Both: destroy on their destructor.
                if sig.destructor {
                    self.delete(host, id)
                } else {
                    host.note(Note::Ignored { interface: iface.name, request: sig.name });
                    Ok(())
                }
            }
            Tag::WmBase => self.wm_base(host, r),
            Tag::XdgSurface => self.xdg_surface(host, r),
            Tag::Toplevel => self.toplevel_request(host, r),
            Tag::Positioner => self.positioner_request(host, r),
            Tag::Popup => self.popup_request(host, r),
            Tag::Seat => self.seat_request(host, r),
            Tag::Pointer => self.pointer_request(host, r),
            Tag::Keyboard => {
                // release (version 3) is its only request.
                self.delete(host, r.id)
            }
            Tag::DecorationManager => self.decoration_manager(host, r),
            Tag::Decoration => self.decoration_request(host, r),
            Tag::DataDeviceManager => self.data_device_manager(host, r),
            Tag::DataSource => self.data_source_request(host, r),
            Tag::DataDevice => self.data_device_request(host, r),
        }
    }

    fn display<H: Host<Fd = F>>(&mut self, host: &mut H, r: Req<'_, F>) -> Result {
        match (r.op, r.args) {
            // sync(callback)
            (0, [Arg::NewId(cb)]) => {
                self.insert(host, *cb, WL_CALLBACK, 1, Kind::Callback)?;
                let serial = self.next_serial();
                self.event(host, *cb, WL_CALLBACK, 0, &[Arg::Uint(serial)])?;
                self.delete(host, *cb)
            }
            // get_registry(registry)
            (1, [Arg::NewId(reg)]) => {
                self.insert(host, *reg, WL_REGISTRY, 1, Kind::Registry)?;
                for (i, (g, v)) in globals().into_iter().enumerate() {
                    let args = [
                        Arg::Uint(i as u32 + 1),
                        Arg::String(Some(g.name.as_bytes())),
                        Arg::Uint(v),
                    ];
                    self.event(host, *reg, WL_REGISTRY, 0, &args)?;
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }

    fn registry<H: Host<Fd = F>>(&mut self, host: &mut H, r: Req<'_, F>) -> Result {
        let [Arg::Uint(name), Arg::UntypedNewId { interface, version, id }] = *r.args else {
            return Ok(());
        };
        let global = globals().get((name as usize).wrapping_sub(1)).copied();
        let Some((g, offered)) = global.filter(|(g, _)| g.name.as_bytes() == interface) else {
            return Err(self.fail(
                host,
                r.id,
                DISPLAY_INVALID_OBJECT,
                format_args!("invalid global {} ({name})", Bytes(interface)),
            ));
        };
        if version == 0 || version > offered {
            return Err(self.fail(
                host,
                r.id,
                DISPLAY_INVALID_OBJECT,
                format_args!(
                    "invalid version for global {} ({name}): have {offered}, wanted {version}",
                    g.name
                ),
            ));
        }
        let kind = if core::ptr::eq(g, WL_COMPOSITOR) {
            Kind::Compositor
        } else if core::ptr::eq(g, WL_SHM) {
            Kind::Shm
        } else if core::ptr::eq(g, XDG_WM_BASE) {
            Kind::WmBase
        } else if core::ptr::eq(g, WL_SEAT) {
            Kind::Seat
        } else if core::ptr::eq(g, DECORATION_MANAGER) {
            Kind::DecorationManager
        } else if core::ptr::eq(g, WL_DATA_DEVICE_MANAGER) {
            Kind::DataDeviceManager
        } else {
            Kind::Output
        };
        let output = matches!(kind, Kind::Output);
        let shm = matches!(kind, Kind::Shm);
        let seat = matches!(kind, Kind::Seat);
        self.insert(host, id, g, version, kind)?;
        if seat {
            self.event(host, id, WL_SEAT, 0, &[Arg::Uint(CAPABILITIES)])?;
            if version >= 2 {
                self.event(host, id, WL_SEAT, 1, &[Arg::String(Some(b"seat0"))])?;
            }
        }
        if shm {
            for format in [ARGB8888, XRGB8888] {
                self.event(host, id, WL_SHM, 0, &[Arg::Uint(format)])?;
            }
        }
        if output {
            self.output_bound(host, id, version)?;
        }
        Ok(())
    }

    /// wl_output's description of the screen, on bind.
    fn output_bound<H: Host<Fd = F>>(&mut self, host: &mut H, id: u32, version: u32) -> Result {
        let (w, h) = host.screen();
        let (w, h) = (w as i32, h as i32);
        // geometry(x, y, physical size unknown, subpixel unknown, make,
        // model, transform normal)
        let geometry = [
            Arg::Int(0),
            Arg::Int(0),
            Arg::Int(0),
            Arg::Int(0),
            Arg::Int(0),
            Arg::String(Some(b"gala")),
            Arg::String(Some(b"shasha")),
            Arg::Int(0),
        ];
        self.event(host, id, WL_OUTPUT, 0, &geometry)?;
        // mode(current | preferred, w, h, 60 Hz)
        self.event(
            host,
            id,
            WL_OUTPUT,
            1,
            &[Arg::Uint(3), Arg::Int(w), Arg::Int(h), Arg::Int(60_000)],
        )?;
        if version >= 2 {
            self.event(host, id, WL_OUTPUT, 3, &[Arg::Int(1)])?;
        }
        if version >= 4 {
            self.event(host, id, WL_OUTPUT, 4, &[Arg::String(Some(b"gala-1"))])?;
            self.event(host, id, WL_OUTPUT, 5, &[Arg::String(Some(b"shasha's screen"))])?;
        }
        if version >= 2 {
            self.event(host, id, WL_OUTPUT, 2, &[])?;
        }
        Ok(())
    }

    fn compositor<H: Host<Fd = F>>(&mut self, host: &mut H, r: Req<'_, F>) -> Result {
        match (r.op, r.args) {
            (0, [Arg::NewId(id)]) => {
                self.insert(host, *id, WL_SURFACE, r.version, Kind::Surface(Surface::default()))
            }
            (1, [Arg::NewId(id)]) => self.insert(host, *id, WL_REGION, 1, Kind::Region),
            _ => Ok(()),
        }
    }

    fn shm<H: Host<Fd = F>>(&mut self, host: &mut H, mut r: Req<'_, F>) -> Result {
        match (r.op, r.args) {
            // create_pool(id, fd, size)
            (0, [Arg::NewId(id), Arg::Fd, Arg::Int(size)]) => {
                let (id, size) = (*id, *size);
                let fd = match r.fd.take() {
                    Some(fd) if host.readable(&fd) => fd,
                    _ => {
                        return Err(self.fail(
                            host,
                            r.id,
                            SHM_INVALID_FD,
                            format_args!("the pool's file descriptor is not a readable file"),
                        ));
                    }
                };
                if size <= 0 || size as u64 > MAX_POOL {
                    return Err(self.fail(
                        host,
                        r.id,
                        SHM_INVALID_STRIDE,
                        format_args!("invalid pool size {size} (at most {MAX_POOL})"),
                    ));
                }
                self.insert(host, id, WL_SHM_POOL, 1, Kind::Pool(Pool { fd, size: size as u64 }))
            }
            // release (version 2)
            (1, []) => self.delete(host, r.id),
            _ => Ok(()),
        }
    }

    fn pool<H: Host<Fd = F>>(&mut self, host: &mut H, r: Req<'_, F>) -> Result {
        match (r.op, r.args) {
            // create_buffer(id, offset, width, height, stride, format)
            (
                0,
                [
                    Arg::NewId(id),
                    Arg::Int(offset),
                    Arg::Int(width),
                    Arg::Int(height),
                    Arg::Int(stride),
                    Arg::Uint(format),
                ],
            ) => {
                let (id, offset, width, height, stride, format) =
                    (*id, *offset, *width, *height, *stride, *format);
                if format != ARGB8888 && format != XRGB8888 {
                    return Err(self.fail(
                        host,
                        r.id,
                        SHM_INVALID_FORMAT,
                        format_args!("invalid format {format:#x}"),
                    ));
                }
                let Some(Object { kind: Kind::Pool(pool), .. }) = self.object(r.id) else {
                    return Ok(());
                };
                let size = pool.size;
                let fits = offset >= 0
                    && width > 0
                    && height > 0
                    && i64::from(stride) >= 4 * i64::from(width)
                    && offset as u64 + stride as u64 * height as u64 <= size;
                if !fits {
                    return Err(self.fail(
                        host,
                        r.id,
                        SHM_INVALID_STRIDE,
                        format_args!(
                            "invalid buffer: offset {offset}, {width}x{height}, stride {stride} in a pool of {size} bytes"
                        ),
                    ));
                }
                let Some(fd) = self.object(r.id).and_then(|o| match &o.kind {
                    Kind::Pool(p) => Some(&p.fd),
                    _ => None,
                }) else {
                    return Ok(());
                };
                let Some(fd) = host.share(fd) else { return Err(self.out_of_memory(host)) };
                let buffer = Buffer {
                    fd,
                    offset: offset as u64,
                    width: width as u32,
                    height: height as u32,
                    stride: stride as u32,
                };
                self.insert(host, id, WL_BUFFER, 1, Kind::Buffer(buffer))
            }
            // destroy: the buffers keep the memory.
            (1, []) => self.delete(host, r.id),
            // resize(size): only larger.
            (2, [Arg::Int(size)]) => {
                let size = *size;
                let Some(Object { kind: Kind::Pool(pool), .. }) = self.object_mut(r.id) else {
                    return Ok(());
                };
                if size <= 0 || (size as u64) < pool.size || size as u64 > MAX_POOL {
                    let had = pool.size;
                    return Err(self.fail(
                        host,
                        r.id,
                        SHM_INVALID_STRIDE,
                        format_args!("invalid pool size {size} (was {had}; it may only grow)"),
                    ));
                }
                pool.size = size as u64;
                Ok(())
            }
            _ => Ok(()),
        }
    }

    fn buffer<H: Host<Fd = F>>(&mut self, host: &mut H, r: Req<'_, F>) -> Result {
        if r.op != 0 {
            return Ok(());
        }
        // destroy: a surface's pending attach of it becomes null, as
        // libwayland's destroy listener makes it.
        for o in self.objects.iter_mut().flatten() {
            if let Kind::Surface(s) = &mut o.kind
                && s.pending.buffer == Some(Some(r.id))
            {
                s.pending.buffer = Some(None);
            }
        }
        self.delete(host, r.id)
    }

    fn surface_request<H: Host<Fd = F>>(&mut self, host: &mut H, r: Req<'_, F>) -> Result {
        let sid = r.id;
        match (r.op, r.args) {
            // destroy
            (0, []) => {
                let Some(s) = self.surface(sid) else { return Ok(()) };
                let window = s.window.take();
                let frames = core::mem::take(&mut s.pending.frames);
                let xdg = s.xdg;
                if let Some(w) = window {
                    host.window_destroy(w);
                }
                // Its focus goes with it (a leave could not name it).
                if self.seat.pointer == Some(sid) {
                    self.seat.pointer = None;
                    self.seat.buttons = 0;
                }
                if self.seat.keyboard == Some(sid) {
                    self.seat.keyboard = None;
                    self.seat.keys.clear();
                    self.seat.depressed = 0;
                }
                if self.seat.active == Some(sid) {
                    self.seat.active = None;
                }
                if let Some(x) = xdg.and_then(|x| self.xdg(x)) {
                    x.surface = 0;
                }
                // A grabbing popup's window may have gone: the grab and
                // the focus move to what is left.
                if window.is_some() && !self.grabs.is_empty() {
                    self.grab_changed(host)?;
                }
                for cb in frames {
                    self.delete(host, cb)?;
                }
                self.delete(host, sid)
            }
            // attach(buffer, x, y)
            (1, [Arg::Object(buffer), Arg::Int(x), Arg::Int(y)]) => {
                if r.version >= 5 && (*x != 0 || *y != 0) {
                    return Err(self.fail(
                        host,
                        sid,
                        SURFACE_INVALID_OFFSET,
                        format_args!("attach with an offset ({x}, {y}); use wl_surface.offset"),
                    ));
                }
                if *x != 0 || *y != 0 {
                    host.note(Note::Ignored { interface: r.iface.name, request: r.sig.name });
                }
                if let Some(s) = self.surface(sid) {
                    s.pending.buffer = Some(*buffer);
                }
                Ok(())
            }
            // damage(x, y, w, h), damage_buffer(x, y, w, h)
            (2 | 9, [Arg::Int(_), Arg::Int(y), Arg::Int(_), Arg::Int(h)]) => {
                if *h > 0
                    && let Some(s) = self.surface(sid)
                {
                    let (from, to) = (i64::from(*y), i64::from(*y) + i64::from(*h));
                    s.pending.damage = Some(match s.pending.damage {
                        Some((a, b)) => (a.min(from), b.max(to)),
                        None => (from, to),
                    });
                }
                Ok(())
            }
            // frame(callback)
            (3, [Arg::NewId(cb)]) => {
                self.insert(host, *cb, WL_CALLBACK, 1, Kind::Callback)?;
                let pushed = self.surface(sid).map(|s| s.pending.frames.try_push(*cb));
                if let Some(Err(_)) = pushed {
                    return Err(self.out_of_memory(host));
                }
                Ok(())
            }
            // set_opaque_region, set_input_region: not used yet.
            (4 | 5, _) => {
                host.note(Note::Ignored { interface: r.iface.name, request: r.sig.name });
                Ok(())
            }
            (6, []) => self.commit(host, sid),
            // set_buffer_transform(transform)
            (7, [Arg::Int(t)]) => {
                if !(0..=7).contains(t) {
                    return Err(self.fail(
                        host,
                        sid,
                        SURFACE_INVALID_TRANSFORM,
                        format_args!("invalid transform {t}"),
                    ));
                }
                if *t != 0 {
                    host.note(Note::Ignored { interface: r.iface.name, request: r.sig.name });
                }
                Ok(())
            }
            // set_buffer_scale(scale)
            (8, [Arg::Int(scale)]) => {
                if *scale < 1 {
                    return Err(self.fail(
                        host,
                        sid,
                        SURFACE_INVALID_SCALE,
                        format_args!("invalid scale {scale}"),
                    ));
                }
                if *scale != 1 {
                    host.note(Note::Ignored { interface: r.iface.name, request: r.sig.name });
                }
                Ok(())
            }
            // offset(x, y)
            (10, [Arg::Int(x), Arg::Int(y)]) => {
                if *x != 0 || *y != 0 {
                    host.note(Note::Ignored { interface: r.iface.name, request: r.sig.name });
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }

    /// wl_surface.commit: the pending state applies (see the module's doc).
    fn commit<H: Host<Fd = F>>(&mut self, host: &mut H, sid: u32) -> Result {
        let Some(s) = self.surface(sid) else { return Ok(()) };
        let attach = s.pending.buffer.take();
        let damage = s.pending.damage.take();
        let frames = core::mem::take(&mut s.pending.frames);
        let xdg = s.xdg;
        // The window geometry applies with the commit.
        let roles = xdg.and_then(|x| self.xdg(x)).map(|xs| {
            if let Some(g) = xs.pending_geometry.take() {
                xs.geometry = Some(g);
            }
            (xs.toplevel, xs.popup)
        });
        let (toplevel, popup) = match roles {
            Some((t, p)) => (t, p),
            None => (None, None),
        };
        let mut shown = None;
        let mut placed = None;
        match (xdg, toplevel, popup) {
            (Some(x), None, Some(p)) => {
                shown = self.popup_commit(host, sid, x, p, attach, damage)?;
            }
            (Some(x), Some(t), _) => {
                let (acked, sent) = match self.xdg(x) {
                    Some(xs) => (xs.acked, xs.sent),
                    None => (false, None),
                };
                if !acked {
                    if matches!(attach, Some(Some(_))) {
                        return Err(self.fail(
                            host,
                            x,
                            XDG_SURFACE_UNCONFIGURED_BUFFER,
                            format_args!(
                                "a buffer was attached before the first configure was acked"
                            ),
                        ));
                    }
                    if sent.is_none() {
                        self.configure(host, x, t)?;
                    }
                } else {
                    match attach {
                        Some(Some(b)) => {
                            // The position of an acked configure goes with
                            // this buffer.
                            placed = self.xdg(x).and_then(|xs| xs.place.take());
                            shown = self.show(host, sid, t, b, damage, placed)?;
                            if let Some(top) = self.toplevel(t)
                                && placed.is_some()
                                && top.sizing.target == placed
                            {
                                top.sizing.target = None;
                            }
                            self.release(host, b)?;
                        }
                        Some(None) => self.unmap(host, sid, x)?,
                        None => {}
                    }
                }
            }
            (Some(x), None, None) => {
                return Err(self.fail(
                    host,
                    x,
                    XDG_SURFACE_NOT_CONSTRUCTED,
                    format_args!("commit of an xdg_surface without a role"),
                ));
            }
            (None, _, _) => {
                // No role: nothing shows it; the buffer is free again.
                if let Some(Some(b)) = attach {
                    self.release(host, b)?;
                }
            }
        }
        if shown.is_some() {
            self.commits += 1;
        }
        let window = self.surface(sid).and_then(|s| s.window);
        host.note(Note::Commit {
            surface: sid,
            buffer: shown.map(|(size, _)| size),
            rows: shown.and_then(|(_, rows)| rows),
            window,
            placed: placed.filter(|_| window.is_some()),
        });
        // Done at the next refresh (the commit is presented already).
        if self.due.try_extend_from_slice(&frames).is_err() {
            return Err(self.out_of_memory(host));
        }
        Ok(())
    }

    /// Whether frame callbacks wait for the next refresh.
    pub fn frames_due(&self) -> bool {
        !self.due.is_empty()
    }

    /// The refresh came: every frame callback whose commit was applied is
    /// done, with the time in milliseconds, and deleted.
    pub fn refresh<H: Host<Fd = F>>(&mut self, host: &mut H) -> Result {
        let now = host.now_ms();
        let due = core::mem::take(&mut self.due);
        for &cb in &due {
            if matches!(self.object(cb), Some(Object { kind: Kind::Callback, .. })) {
                self.event(host, cb, WL_CALLBACK, 0, &[Arg::Uint(now)])?;
                self.delete(host, cb)?;
                self.frames += 1;
            }
        }
        // The vector's room is kept for the next ones.
        self.due = due;
        self.due.clear();
        Ok(())
    }

    /// wl_buffer.release (if the client has not destroyed it).
    fn release<H: Host<Fd = F>>(&mut self, host: &mut H, b: u32) -> Result {
        if matches!(self.object(b), Some(Object { kind: Kind::Buffer(_), .. })) {
            self.event(host, b, WL_BUFFER, 0, &[])?;
        }
        Ok(())
    }

    /// The toplevel's configure: bounds and capabilities first where its
    /// version has them, then the size the desktop asks for (0x0: the
    /// client chooses) and the states (maximized, activated while the
    /// surface has the keyboard focus, tiled from version 2), the
    /// decoration's mode, and the xdg_surface's serial, remembered with
    /// the position the desktop asked for until acked.
    fn configure<H: Host<Fd = F>>(&mut self, host: &mut H, x: u32, t: u32) -> Result {
        let version = self.object(t).map_or(1, |o| o.version);
        if version >= 4 {
            let (w, h) = host.screen();
            self.event(host, t, XDG_TOPLEVEL, 2, &[Arg::Int(w as i32), Arg::Int(h as i32)])?;
        }
        if version >= 5 {
            // Neither window_menu nor fullscreen yet.
            let mut caps = [0u8; 8];
            caps[..4].copy_from_slice(&CAPABILITY_MAXIMIZE.to_le_bytes());
            caps[4..].copy_from_slice(&CAPABILITY_MINIMIZE.to_le_bytes());
            self.event(host, t, XDG_TOPLEVEL, 3, &[Arg::Array(&caps)])?;
        }
        let sizing = self.toplevel(t).map(|top| top.sizing).unwrap_or_default();
        let surface = match self.object(x) {
            Some(Object { kind: Kind::XdgSurface(xs), .. }) => xs.surface,
            _ => 0,
        };
        let mut bits = 0u32;
        if sizing.maximized {
            bits |= 1 << STATE_MAXIMIZED;
        }
        if surface != 0 && self.seat.active == Some(surface) {
            bits |= 1 << STATE_ACTIVATED;
        }
        if version >= 2 {
            bits |= sizing.tiled;
        }
        // The states in their numeric order.
        let mut states = [0u8; 4 * 10];
        let mut n = 0;
        for state in 1..10u32 {
            if bits & 1 << state != 0 {
                states[n..n + 4].copy_from_slice(&state.to_le_bytes());
                n += 4;
            }
        }
        let (w, h) = sizing.size;
        let size = [Arg::Int(w as i32), Arg::Int(h as i32), Arg::Array(&states[..n])];
        self.event(host, t, XDG_TOPLEVEL, 0, &size)?;
        if let Some(d) = self.toplevel(t).and_then(|top| top.decoration) {
            let mode = self.mode(t);
            self.event(host, d, TOPLEVEL_DECORATION, 0, &[Arg::Uint(mode)])?;
        }
        let serial = self.next_serial();
        self.event(host, x, XDG_SURFACE, 0, &[Arg::Uint(serial)])?;
        let Some(xs) = self.xdg(x) else { return Ok(()) };
        xs.sent = Some(serial);
        if xs.unacked.len() >= MAX_CONFIGURES {
            xs.unacked.remove(0);
        }
        if xs.unacked.try_push((serial, sizing.target)).is_err() {
            return Err(self.out_of_memory(host));
        }
        host.note(Note::Configure { surface, width: w, height: h, states: bits, serial });
        Ok(())
    }

    /// Sends toplevel `t` a configure again if it was configured (its
    /// state or size changed).
    fn reconfigure<H: Host<Fd = F>>(&mut self, host: &mut H, t: u32) -> Result {
        let x = self.toplevel(t).map_or(0, |top| top.xdg);
        if self.xdg(x).is_some_and(|xs| xs.sent.is_some()) {
            self.configure(host, x, t)?;
        }
        Ok(())
    }

    /// The decoration mode of toplevel `t`: its window's while mapped,
    /// else what its decoration object asked for (server-side when it
    /// asked nothing); without a decoration object client-side: a client
    /// that does not negotiate decorates itself (xdg-decoration's
    /// description), as GTK 3 does, which never binds the manager.
    fn mode(&mut self, t: u32) -> u32 {
        let Some(top) = self.toplevel(t) else { return MODE_SERVER_SIDE };
        let (x, decoration) = (top.xdg, top.decoration);
        let surface = match self.object(x) {
            Some(Object { kind: Kind::XdgSurface(xs), .. }) => xs.surface,
            _ => 0,
        };
        if let Some(s) = self.surface(surface)
            && s.window.is_some()
        {
            return if s.decorated { MODE_SERVER_SIDE } else { MODE_CLIENT_SIDE };
        }
        match decoration.and_then(|d| self.object(d)) {
            Some(Object { kind: Kind::Decoration(d), .. })
                if d.wanted == Some(MODE_CLIENT_SIDE) =>
            {
                MODE_CLIENT_SIDE
            }
            Some(_) => MODE_SERVER_SIDE,
            None => MODE_CLIENT_SIDE,
        }
    }

    /// Shows buffer `b` on surface `sid` (toplevel `t`): maps its window
    /// if it has none, then copies the rows. The buffer's size and the
    /// rows copied.
    fn show<H: Host<Fd = F>>(
        &mut self,
        host: &mut H,
        sid: u32,
        t: u32,
        b: u32,
        damage: Option<(i64, i64)>,
        place: Option<(i32, i32)>,
    ) -> Result<Option<Shown>> {
        let (width, height) = match self.object(b) {
            Some(Object { kind: Kind::Buffer(buf), .. }) => (buf.width, buf.height),
            _ => return Ok(None),
        };
        let mut title = [0u8; TITLE_MAX];
        let n = match self.toplevel(t) {
            Some(top) => {
                title[..top.title.len()].copy_from_slice(&top.title);
                top.title.len()
            }
            None => 0,
        };
        let decorated = self.mode(t) == MODE_SERVER_SIDE;
        let x = self.toplevel(t).map_or(0, |top| top.xdg);
        let geometry = self.xdg(x).and_then(|xs| xs.geometry);
        let Some(s) = self.surface(sid) else { return Ok(None) };
        let decorated = if s.window.is_some() { s.decorated } else { decorated };
        let view = view_of(geometry, decorated, width, height);
        let (had, changed) = (s.window, s.view != view);
        let resized = (s.view.2, s.view.3) != (view.2, view.3);
        s.size = (width, height);
        s.view = view;
        let window = match had {
            Some(w) => {
                if let Some((x, y)) = place {
                    host.window_place(w, x, y, view.2, view.3, s.decorated);
                } else if resized {
                    host.window_resize(w, view.2, view.3, s.decorated);
                }
                Some(w)
            }
            None => {
                s.decorated = decorated;
                let w = host.window_create(&title[..n], view.2, view.3, decorated);
                host.note(Note::Mapped { surface: sid, window: w });
                w
            }
        };
        if let Some(s) = self.surface(sid) {
            s.window = window;
        }
        let rows = self.present(host, b, window, had.is_none() || changed, damage, view)?;
        Ok(Some(((width, height), rows)))
    }

    /// Copies the `view` of buffer `b` into `window`: every row of a new
    /// or changed one (`whole`), else the damaged ones. The rows copied
    /// (buffer rows).
    fn present<H: Host<Fd = F>>(
        &mut self,
        host: &mut H,
        b: u32,
        window: Option<u32>,
        whole: bool,
        damage: Option<(i64, i64)>,
        view: View,
    ) -> Result<Option<(u32, u32)>> {
        let Some(w) = window else { return Ok(None) };
        let (top, bottom) = (i64::from(view.1), i64::from(view.1) + i64::from(view.3));
        let rows = if whole {
            Some((top as u32, bottom as u32))
        } else {
            damage.and_then(|(from, to)| {
                let (from, to) = (from.clamp(top, bottom), to.clamp(top, bottom));
                (from < to).then_some((from as u32, to as u32))
            })
        };
        if let Some((from, to)) = rows {
            self.copy(host, b, w, from, to, view)?;
        }
        Ok(rows)
    }

    /// Copies rows `from..to` of buffer `b`, the columns of `view`, into
    /// window `w` (its row 0 is the view's first).
    fn copy<H: Host<Fd = F>>(
        &mut self,
        host: &mut H,
        b: u32,
        w: u32,
        from: u32,
        to: u32,
        view: View,
    ) -> Result {
        let Some(Object { kind: Kind::Buffer(buf), .. }) = self.object(b) else { return Ok(()) };
        let (stride, offset) = (buf.stride, buf.offset + 4 * view.0 as u64);
        let width = view.2;
        let row_bytes = 4 * width as usize;
        let mut bytes: Vec<u8> = Vec::new();
        let mut pixels: Vec<u32> = Vec::new();
        let chunk = (to - from).min(ROWS_AT_ONCE) as usize;
        if bytes.try_reserve_exact(row_bytes).is_err()
            || pixels.try_reserve_exact(chunk * width as usize).is_err()
        {
            return Err(self.out_of_memory(host));
        }
        bytes.resize(row_bytes, 0);
        let mut y = from;
        while y < to {
            let rows = (to - y).min(ROWS_AT_ONCE);
            pixels.clear();
            for row in y..y + rows {
                let at = offset + u64::from(row) * u64::from(stride);
                let Some(Object { kind: Kind::Buffer(buf), .. }) = self.object(b) else {
                    return Ok(());
                };
                if !host.read(&buf.fd, at, &mut bytes) {
                    return Err(self.fail(
                        host,
                        b,
                        SHM_INVALID_FD,
                        format_args!("error accessing SHM buffer (row {row} past the file's end)"),
                    ));
                }
                // XRGB8888 and ARGB8888 are 0xAARRGGBB little-endian;
                // shasha's pixels are 0x00RRGGBB (alpha not composed yet).
                pixels.extend(
                    bytes.chunks_exact(4).map(|p| u32::from_le_bytes([p[0], p[1], p[2], 0])),
                );
            }
            host.window_present(w, y - view.1 as u32, width, rows, &pixels);
            y += rows;
        }
        Ok(())
    }

    /// A null buffer committed: the window goes and the surface waits for
    /// a new initial commit (and configure).
    fn unmap<H: Host<Fd = F>>(&mut self, host: &mut H, sid: u32, x: u32) -> Result {
        // No longer the active window's surface (the focus does not come
        // back to it), and its popups have nothing to stand on: dismissed.
        if self.seat.active == Some(sid) {
            self.seat.active = None;
        }
        if self.dismiss_children(host, x, "parent unmapped")? {
            self.grab_changed(host)?;
        }
        // An unmapped surface has neither focus.
        if self.seat.pointer == Some(sid) {
            self.seat.buttons = 0;
            self.pointer_leave(host)?;
        }
        if self.seat.keyboard == Some(sid) {
            self.keyboard_leave(host)?;
        }
        if let Some(s) = self.surface(sid) {
            s.size = (0, 0);
            s.view = (0, 0, 0, 0);
            if let Some(w) = s.window.take() {
                host.window_destroy(w);
            }
        }
        let toplevel = match self.xdg(x) {
            Some(xs) => {
                xs.sent = None;
                xs.acked = false;
                xs.unacked.clear();
                xs.place = None;
                xs.toplevel
            }
            None => None,
        };
        // A new window is the client's size again.
        if let Some(top) = toplevel.and_then(|t| self.toplevel(t)) {
            top.sizing = Sizing::default();
        }
        Ok(())
    }

    // ---- Popups (ARCHITECTURE "Popups") -----------------------------------

    fn popup(&mut self, id: u32) -> Option<&mut Popup> {
        match self.object_mut(id) {
            Some(Object { kind: Kind::Popup(p), .. }) => Some(p),
            _ => None,
        }
    }

    /// The xdg_wm_base xdg_surface `x` came from (1, the display, if it is
    /// gone: the error still names an object).
    fn wm_base_of(&mut self, x: u32) -> u32 {
        self.xdg(x).map_or(1, |xs| xs.wm_base)
    }

    /// xdg_surface `x`'s get_popup(id, parent, positioner).
    #[allow(clippy::too_many_arguments)]
    fn get_popup<H: Host<Fd = F>>(
        &mut self,
        host: &mut H,
        x: u32,
        role: Option<u32>,
        id: u32,
        parent: Option<u32>,
        positioner: u32,
        version: u32,
    ) -> Result {
        if role.is_some() {
            return Err(self.fail(
                host,
                x,
                XDG_SURFACE_ALREADY_CONSTRUCTED,
                format_args!("xdg_surface@{x} already has a role"),
            ));
        }
        let rules = match self.object(positioner) {
            Some(Object { kind: Kind::Positioner(r), .. }) => *r,
            _ => return Ok(()),
        };
        let wm_base = self.wm_base_of(x);
        if !rules.complete() {
            return Err(self.fail(
                host,
                wm_base,
                WM_BASE_INVALID_POSITIONER,
                format_args!(
                    "xdg_positioner@{positioner} is incomplete (size {:?}, anchor rectangle {:?})",
                    rules.size, rules.rect
                ),
            ));
        }
        // A parent is an xdg_surface with a role, not this one.
        if let Some(px) = parent {
            let has_role = self.xdg(px).is_some_and(|p| p.toplevel.is_some() || p.popup.is_some());
            if px == x || !has_role {
                return Err(self.fail(
                    host,
                    wm_base,
                    WM_BASE_INVALID_POPUP_PARENT,
                    format_args!("xdg_surface@{px} cannot be a popup's parent (no role yet)"),
                ));
            }
        }
        let p = Popup {
            xdg: x,
            parent: parent.unwrap_or(0),
            rules,
            configured: (0, 0, 0, 0),
            at: None,
            origin: None,
            shown_at: None,
            grab: false,
            dismissed: false,
        };
        self.insert(host, id, XDG_POPUP, version, Kind::Popup(p))?;
        let surface = match self.xdg(x) {
            Some(xs) => {
                xs.popup = Some(id);
                xs.surface
            }
            None => 0,
        };
        host.note(Note::Role { surface, role: "xdg_popup", object: id });
        Ok(())
    }

    /// The screen position of the window geometry of xdg_surface `x`
    /// (None while it shows no window): its window's client origin shows
    /// the view's corner.
    fn geometry_origin<H: Host<Fd = F>>(&mut self, host: &mut H, x: u32) -> Option<(i32, i32)> {
        let (surface, geometry) = self.xdg(x).map(|xs| (xs.surface, xs.geometry))?;
        let (window, view) = self.surface(surface).map(|s| (s.window, s.view))?;
        let (sx, sy) = host.client_origin(window?)?;
        let (gx, gy) = geometry.map_or((0, 0), |g| (g.0, g.1));
        Some((sx.saturating_add(gx - view.0), sy.saturating_add(gy - view.1)))
    }

    /// A point of surface `sid`'s window's client area in surface
    /// coordinates (the view's corner is the client area's).
    fn surface_point(&self, sid: u32, x: i32, y: i32) -> (i32, i32) {
        let (vx, vy) = match self.object(sid) {
            Some(Object { kind: Kind::Surface(s), .. }) => (s.view.0, s.view.1),
            _ => (0, 0),
        };
        (x.saturating_add(vx), y.saturating_add(vy))
    }

    /// Where popup `p`'s parent's window geometry is on the screen: the
    /// parent's while it shows, else (an orphan, or a parent not shown)
    /// where it was when the popup was last placed.
    fn parent_origin<H: Host<Fd = F>>(&mut self, host: &mut H, p: u32) -> Option<(i32, i32)> {
        let (parent, kept) = self.popup(p).map(|pp| (pp.parent, pp.origin))?;
        if parent != 0
            && let Some(o) = self.geometry_origin(host, parent)
        {
            return Some(o);
        }
        kept
    }

    /// Places popup `p` by its rules and sends its configure (and the
    /// xdg_surface's), remembered with the place until acked.
    fn configure_popup<H: Host<Fd = F>>(&mut self, host: &mut H, x: u32, p: u32) -> Result {
        let origin = self.parent_origin(host, p);
        let (work_w, work_h) = host.work_area();
        let Some(rules) = self.popup(p).map(|pp| pp.rules) else { return Ok(()) };
        let rect = popup::place(&rules, origin, (0, 0, work_w, work_h));
        let (px, py, w, h) = rect;
        let args = [Arg::Int(px), Arg::Int(py), Arg::Int(w), Arg::Int(h)];
        self.event(host, p, XDG_POPUP, 0, &args)?;
        let serial = self.next_serial();
        self.event(host, x, XDG_SURFACE, 0, &[Arg::Uint(serial)])?;
        if let Some(pp) = self.popup(p) {
            pp.configured = rect;
            if origin.is_some() {
                pp.origin = origin;
            }
        }
        let Some(xs) = self.xdg(x) else { return Ok(()) };
        xs.sent = Some(serial);
        if xs.unacked.len() >= MAX_CONFIGURES {
            xs.unacked.remove(0);
        }
        if xs.unacked.try_push((serial, Some((px, py)))).is_err() {
            return Err(self.out_of_memory(host));
        }
        let surface = xs.surface;
        host.note(Note::Popup { surface, what: "configure", why: "", rect, serial });
        Ok(())
    }

    /// A commit of popup `p`'s surface `sid` (xdg_surface `x`): its first
    /// is answered with its configure; once acked a buffer maps it (a
    /// popup window at its place) and a null buffer unmaps it. A
    /// dismissed popup shows nothing again (its buffers are released).
    fn popup_commit<H: Host<Fd = F>>(
        &mut self,
        host: &mut H,
        sid: u32,
        x: u32,
        p: u32,
        attach: Option<Option<u32>>,
        damage: Option<(i64, i64)>,
    ) -> Result<Option<Shown>> {
        let Some((parent, dismissed, kept)) =
            self.popup(p).map(|pp| (pp.parent, pp.dismissed, pp.origin))
        else {
            return Ok(None);
        };
        if dismissed {
            if let Some(Some(b)) = attach {
                self.release(host, b)?;
            }
            return Ok(None);
        }
        let (acked, sent) = match self.xdg(x) {
            Some(xs) => (xs.acked, xs.sent),
            None => (false, None),
        };
        if !acked {
            if matches!(attach, Some(Some(_))) {
                return Err(self.fail(
                    host,
                    x,
                    XDG_SURFACE_UNCONFIGURED_BUFFER,
                    format_args!("a buffer was attached before the first configure was acked"),
                ));
            }
            if sent.is_none() {
                if parent == 0 && kept.is_none() {
                    // No protocol here gives a parent another way.
                    let wm_base = self.wm_base_of(x);
                    return Err(self.fail(
                        host,
                        wm_base,
                        WM_BASE_INVALID_POPUP_PARENT,
                        format_args!("xdg_popup@{p} has no parent"),
                    ));
                }
                self.configure_popup(host, x, p)?;
            }
            return Ok(None);
        }
        match attach {
            Some(Some(b)) => {
                let shown = self.show_popup(host, sid, x, p, b, damage)?;
                self.release(host, b)?;
                Ok(shown)
            }
            Some(None) => {
                let grab = self.popup(p).is_some_and(|pp| pp.grab);
                if grab && self.grabs.last() != Some(&p) {
                    return Err(self.not_topmost(host, x, p, "unmapped"));
                }
                self.unmap_popup(host, p)?;
                if grab {
                    self.grab_changed(host)?;
                }
                Ok(None)
            }
            None => Ok(None),
        }
    }

    /// Shows buffer `b` on popup `p`'s surface `sid`: a popup window at
    /// the parent's window geometry plus its place (less its own window
    /// geometry's offset), moved when the place or size changes; a
    /// grabbing popup shown takes the grab and the keyboard focus.
    fn show_popup<H: Host<Fd = F>>(
        &mut self,
        host: &mut H,
        sid: u32,
        x: u32,
        p: u32,
        b: u32,
        damage: Option<(i64, i64)>,
    ) -> Result<Option<Shown>> {
        let (width, height) = match self.object(b) {
            Some(Object { kind: Kind::Buffer(buf), .. }) => (buf.width, buf.height),
            _ => return Ok(None),
        };
        let Some(origin) = self.parent_origin(host, p) else {
            let wm_base = self.wm_base_of(x);
            return Err(self.fail(
                host,
                wm_base,
                WM_BASE_INVALID_POPUP_PARENT,
                format_args!("xdg_popup@{p}'s parent is not mapped"),
            ));
        };
        let (place, geometry) = match self.xdg(x) {
            Some(xs) => (xs.place.take(), xs.geometry),
            None => (None, None),
        };
        let Some(pp) = self.popup(p) else { return Ok(None) };
        if place.is_some() {
            pp.at = place;
        }
        pp.origin = Some(origin);
        let (ax, ay) = pp.at.unwrap_or((pp.configured.0, pp.configured.1));
        let (gx, gy) = geometry.map_or((0, 0), |g| (g.0, g.1));
        // Frameless: the window shows the popup's window geometry, which
        // goes at the place (less what of it the buffer clamps).
        let view = view_of(geometry, false, width, height);
        let at = (
            origin.0.saturating_add(ax).saturating_sub(gx - view.0),
            origin.1.saturating_add(ay).saturating_sub(gy - view.1),
        );
        let moved = pp.shown_at != Some(at);
        pp.shown_at = Some(at);
        let grab = pp.grab;
        let Some(s) = self.surface(sid) else { return Ok(None) };
        let (had, changed) = (s.window, s.view != view);
        let resized = (s.view.2, s.view.3) != (view.2, view.3);
        s.size = (width, height);
        s.view = view;
        s.decorated = false;
        let window = match had {
            Some(w) => {
                if moved || resized {
                    host.window_place(w, at.0, at.1, view.2, view.3, false);
                }
                Some(w)
            }
            None => {
                let w = host.popup_create(at.0, at.1, view.2, view.3);
                host.note(Note::Mapped { surface: sid, window: w });
                w
            }
        };
        if let Some(s) = self.surface(sid) {
            s.window = window;
        }
        let rows = self.present(host, b, window, had.is_none() || changed, damage, view)?;
        if had.is_none() && grab {
            self.grab_changed(host)?;
        }
        Ok(Some(((width, height), rows)))
    }

    /// Popup `p`'s window goes (destroyed or unmapped): the foci it had,
    /// and the grab if it was its; it waits for a new initial commit.
    fn unmap_popup<H: Host<Fd = F>>(&mut self, host: &mut H, p: u32) -> Result {
        let Some((x, grab)) = self.popup(p).map(|pp| (pp.xdg, pp.grab)) else { return Ok(()) };
        if let Some(pp) = self.popup(p) {
            pp.grab = false;
            pp.at = None;
            pp.shown_at = None;
        }
        let sid = match self.xdg(x) {
            Some(xs) => {
                xs.sent = None;
                xs.acked = false;
                xs.unacked.clear();
                xs.place = None;
                xs.surface
            }
            None => 0,
        };
        self.drop_foci(host, sid)?;
        if let Some(s) = self.surface(sid) {
            s.size = (0, 0);
            s.view = (0, 0, 0, 0);
            if let Some(w) = s.window.take() {
                host.window_destroy(w);
            }
        }
        if grab {
            self.grabs.retain(|&g| g != p);
        }
        Ok(())
    }

    /// Surface `sid` no longer shows: the pointer and the keyboard focus
    /// leave it.
    fn drop_foci<H: Host<Fd = F>>(&mut self, host: &mut H, sid: u32) -> Result {
        if sid == 0 {
            return Ok(());
        }
        if self.seat.pointer == Some(sid) {
            self.seat.buttons = 0;
            self.pointer_leave(host)?;
        }
        if self.seat.keyboard == Some(sid) {
            self.keyboard_leave(host)?;
        }
        Ok(())
    }

    /// The compositor dismisses popup `p` (`why`): popup_done, unmapped
    /// for good, and its own popups with it (topmost first).
    fn dismiss<H: Host<Fd = F>>(&mut self, host: &mut H, p: u32, why: &'static str) -> Result {
        let Some(pp) = self.popup(p) else { return Ok(()) };
        if pp.dismissed {
            return Ok(());
        }
        pp.dismissed = true;
        let x = pp.xdg;
        self.dismiss_children(host, x, why)?;
        self.event(host, p, XDG_POPUP, 1, &[])?;
        let surface = self.xdg(x).map_or(0, |xs| xs.surface);
        host.note(Note::Popup { surface, what: "dismissed", why, rect: (0, 0, 0, 0), serial: 0 });
        self.unmap_popup(host, p)
    }

    /// Dismisses the popups whose parent is xdg_surface `x` (each with
    /// its own); whether any was.
    fn dismiss_children<H: Host<Fd = F>>(
        &mut self,
        host: &mut H,
        x: u32,
        why: &'static str,
    ) -> Result<bool> {
        let mut any = false;
        // Newest first: the topmost are the last made.
        let mut i = self.objects.len();
        while i > 1 {
            i -= 1;
            let child = match &self.objects[i] {
                Some(Object { kind: Kind::Popup(c), .. }) if c.parent == x && !c.dismissed => {
                    i as u32
                }
                _ => continue,
            };
            self.dismiss(host, child, why)?;
            any = true;
        }
        Ok(any)
    }

    /// Every grabbing popup is dismissed, topmost first (the desktop
    /// ended the grab, or the window lost the activation); then the
    /// keyboard focus goes back to the active window.
    fn dismiss_grabs<H: Host<Fd = F>>(&mut self, host: &mut H, why: &'static str) -> Result {
        if self.grabs.is_empty() {
            return Ok(());
        }
        while let Some(&p) = self.grabs.last() {
            self.dismiss(host, p, why)?;
            // dismiss takes it off; never loop on one it could not.
            self.grabs.retain(|&g| g != p);
        }
        self.grab_changed(host)
    }

    /// The grab changed: the desktop's follows the topmost shown grabbing
    /// popup, and so does the keyboard focus (without one: the active
    /// window's surface).
    fn grab_changed<H: Host<Fd = F>>(&mut self, host: &mut H) -> Result {
        let mut top = None;
        for &p in self.grabs.iter().rev() {
            let x = match self.object(p) {
                Some(Object { kind: Kind::Popup(pp), .. }) => pp.xdg,
                _ => continue,
            };
            let sid = match self.object(x) {
                Some(Object { kind: Kind::XdgSurface(xs), .. }) => xs.surface,
                _ => continue,
            };
            if let Some(Object { kind: Kind::Surface(s), .. }) = self.object(sid)
                && let Some(w) = s.window
            {
                top = Some((sid, w));
                break;
            }
        }
        host.grab(top.map(|(_, w)| w));
        let focus = top.map(|(sid, _)| sid).or(self.seat.active);
        match focus {
            Some(f) if self.seat.keyboard != Some(f) => self.keyboard_enter(host, f),
            Some(_) => Ok(()),
            None => {
                // Only a popup's focus is taken back here.
                let on_popup = self.seat.keyboard.is_some_and(|k| {
                    matches!(self.object(k), Some(Object { kind: Kind::Surface(s), .. })
                        if s.xdg.is_some_and(|x| matches!(self.object(x),
                            Some(Object { kind: Kind::XdgSurface(xs), .. }) if xs.popup.is_some())))
                });
                if on_popup { self.keyboard_leave(host) } else { Ok(()) }
            }
        }
    }

    /// The not_the_topmost_popup error: popup `p` (xdg_surface `x`) has a
    /// grabbing popup above it.
    fn not_topmost<H: Host<Fd = F>>(&mut self, host: &mut H, x: u32, p: u32, what: &str) -> End {
        let wm_base = self.wm_base_of(x);
        let top = self.grabs.last().copied().unwrap_or(0);
        self.fail(
            host,
            wm_base,
            WM_BASE_NOT_THE_TOPMOST_POPUP,
            format_args!("xdg_popup@{p} {what} while xdg_popup@{top} grabs above it"),
        )
    }

    fn positioner_request<H: Host<Fd = F>>(&mut self, host: &mut H, r: Req<'_, F>) -> Result {
        let id = r.id;
        // The rules are a copy, stored back at the end.
        let mut rules = match self.object(id) {
            Some(Object { kind: Kind::Positioner(rules), .. }) => *rules,
            _ => return Ok(()),
        };
        let invalid = |what: fmt::Arguments<'_>, this: &mut Self, host: &mut H| -> Result {
            Err(this.fail(host, id, POSITIONER_INVALID_INPUT, what))
        };
        match (r.op, r.args) {
            (0, []) => return self.delete(host, id),
            // set_size(width, height)
            (1, [Arg::Int(w), Arg::Int(h)]) => {
                if *w <= 0 || *h <= 0 {
                    return invalid(format_args!("invalid size {w}x{h}"), self, host);
                }
                rules.size = (*w, *h);
            }
            // set_anchor_rect(x, y, width, height)
            (2, [Arg::Int(x), Arg::Int(y), Arg::Int(w), Arg::Int(h)]) => {
                if *w < 0 || *h < 0 {
                    return invalid(
                        format_args!("invalid anchor rectangle size {w}x{h}"),
                        self,
                        host,
                    );
                }
                rules.rect = Some((*x, *y, *w, *h));
            }
            // set_anchor(anchor), set_gravity(gravity)
            (3 | 4, [Arg::Uint(v)]) => {
                if !popup::valid_direction(*v) {
                    let what = if r.op == 3 { "anchor" } else { "gravity" };
                    return invalid(format_args!("invalid {what} {v}"), self, host);
                }
                if r.op == 3 {
                    rules.anchor = *v;
                } else {
                    rules.gravity = *v;
                }
            }
            // set_constraint_adjustment(bits): unknown bits are ignored.
            (5, [Arg::Uint(bits)]) => rules.adjust = bits & popup::ADJUST_ALL,
            // set_offset(x, y)
            (6, [Arg::Int(x), Arg::Int(y)]) => rules.offset = (*x, *y),
            // set_reactive (3): kept; a popup is not reconstrained when its
            // parent moves yet.
            (7, []) => {
                rules.reactive = true;
                host.note(Note::Ignored { interface: r.iface.name, request: r.sig.name });
            }
            // set_parent_size, set_parent_configure (3): the parent's
            // future state is not used (popups are placed against its
            // present one).
            (8 | 9, _) => {
                host.note(Note::Ignored { interface: r.iface.name, request: r.sig.name });
            }
            _ => {}
        }
        if let Some(Object { kind: Kind::Positioner(stored), .. }) = self.object_mut(id) {
            *stored = rules;
        }
        Ok(())
    }

    fn popup_request<H: Host<Fd = F>>(&mut self, host: &mut H, r: Req<'_, F>) -> Result {
        let p = r.id;
        let Some((x, grab, dismissed)) = self.popup(p).map(|pp| (pp.xdg, pp.grab, pp.dismissed))
        else {
            return Ok(());
        };
        match (r.op, r.args) {
            // destroy: unmapped; the topmost grabbing popup only. Its own
            // popups stay where they are (orphans: weston's behaviour).
            (0, []) => {
                if grab && !dismissed && self.grabs.last() != Some(&p) {
                    return Err(self.not_topmost(host, x, p, "destroyed"));
                }
                self.unmap_popup(host, p)?;
                for i in 1..self.objects.len() {
                    let child_xdg = match &mut self.objects[i] {
                        Some(Object { kind: Kind::Popup(c), .. }) if c.parent == x => {
                            c.parent = 0;
                            c.xdg
                        }
                        _ => continue,
                    };
                    {
                        let surface = self.xdg(child_xdg).map_or(0, |xs| xs.surface);
                        host.note(Note::Popup {
                            surface,
                            what: "orphaned",
                            why: "its parent popup was destroyed first",
                            rect: (0, 0, 0, 0),
                            serial: 0,
                        });
                    }
                }
                if let Some(xs) = self.xdg(x) {
                    xs.popup = None;
                }
                if grab {
                    self.grab_changed(host)?;
                }
                self.delete(host, p)
            }
            // grab(seat, serial)
            (1, [Arg::Object(Some(_)), Arg::Uint(serial)]) => self.grab(host, x, p, *serial),
            // reposition(positioner, token) (3)
            (2, [Arg::Object(Some(pos)), Arg::Uint(token)]) => {
                let rules = match self.object(*pos) {
                    Some(Object { kind: Kind::Positioner(rules), .. }) => *rules,
                    _ => return Ok(()),
                };
                if !rules.complete() {
                    let wm_base = self.wm_base_of(x);
                    return Err(self.fail(
                        host,
                        wm_base,
                        WM_BASE_INVALID_POSITIONER,
                        format_args!("xdg_positioner@{pos} is incomplete"),
                    ));
                }
                if let Some(pp) = self.popup(p) {
                    pp.rules = rules;
                }
                // Before the first configure, that one uses the rules; a
                // dismissed popup is placed no more.
                let configured = self.xdg(x).is_some_and(|xs| xs.sent.is_some());
                if dismissed || !configured {
                    return Ok(());
                }
                self.event(host, p, XDG_POPUP, 2, &[Arg::Uint(*token)])?;
                let surface = self.xdg(x).map_or(0, |xs| xs.surface);
                let rect = (0, 0, 0, 0);
                host.note(Note::Popup {
                    surface,
                    what: "repositioned",
                    why: "",
                    rect,
                    serial: *token,
                });
                self.configure_popup(host, x, p)
            }
            _ => Ok(()),
        }
    }

    /// xdg_popup.grab: allowed only before its first commit, with the
    /// topmost grabbing popup (or, without one, a toplevel) as its parent;
    /// granted for the serial of the client's last button or key press
    /// (else the popup is dismissed at once, as the protocol says of a
    /// grab the compositor denies), and so for a parent dismissed already.
    fn grab<H: Host<Fd = F>>(&mut self, host: &mut H, x: u32, p: u32, serial: u32) -> Result {
        let (committed, surface) = match self.xdg(x) {
            Some(xs) => (xs.sent.is_some() || xs.acked, xs.surface),
            None => (false, 0),
        };
        let mapped = self.surface(surface).is_some_and(|s| s.window.is_some());
        let Some((parent, dismissed)) = self.popup(p).map(|pp| (pp.parent, pp.dismissed)) else {
            return Ok(());
        };
        if committed || mapped {
            return Err(self.fail(
                host,
                p,
                POPUP_INVALID_GRAB,
                format_args!("xdg_popup@{p} grabs after its first commit"),
            ));
        }
        if dismissed {
            return Ok(());
        }
        let parent_popup = self.xdg(parent).and_then(|px| px.popup);
        let parent_dismissed =
            parent_popup.and_then(|pp| self.popup(pp)).is_some_and(|pp| pp.dismissed);
        let rect = (0, 0, 0, 0);
        if parent_dismissed {
            self.event(host, p, XDG_POPUP, 1, &[])?;
            if let Some(pp) = self.popup(p) {
                pp.dismissed = true;
            }
            let why = "its parent was dismissed";
            host.note(Note::Popup { surface, what: "grab denied", why, rect, serial });
            return Ok(());
        }
        let top = self.grabs.last().copied();
        let parent_toplevel = self.xdg(parent).is_some_and(|px| px.toplevel.is_some());
        let valid = match top {
            None => parent_toplevel,
            Some(t) => parent_popup == Some(t),
        };
        if !valid {
            return Err(self.fail(
                host,
                p,
                POPUP_INVALID_GRAB,
                format_args!(
                    "xdg_popup@{p}'s parent is neither the topmost grabbing popup ({top:?}) nor, without one, a toplevel"
                ),
            ));
        }
        if self.seat.press != Some(serial) {
            self.event(host, p, XDG_POPUP, 1, &[])?;
            if let Some(pp) = self.popup(p) {
                pp.dismissed = true;
            }
            let why = "not the serial of the last button or key press";
            host.note(Note::Popup { surface, what: "grab denied", why, rect, serial });
            return Ok(());
        }
        if self.grabs.try_push(p).is_err() {
            return Err(self.out_of_memory(host));
        }
        if let Some(pp) = self.popup(p) {
            pp.grab = true;
        }
        host.note(Note::Popup { surface, what: "grab", why: "", rect, serial });
        Ok(())
    }

    fn wm_base<H: Host<Fd = F>>(&mut self, host: &mut H, r: Req<'_, F>) -> Result {
        match (r.op, r.args) {
            (0, []) => self.delete(host, r.id),
            // create_positioner(id)
            (1, [Arg::NewId(id)]) => self.insert(
                host,
                *id,
                XDG_POSITIONER,
                r.version,
                Kind::Positioner(Rules::default()),
            ),
            // get_xdg_surface(id, surface)
            (2, [Arg::NewId(id), Arg::Object(Some(sid))]) => {
                let (id, sid) = (*id, *sid);
                let Some(s) = self.surface(sid) else { return Ok(()) };
                if s.xdg.is_some() {
                    return Err(self.fail(
                        host,
                        r.id,
                        WM_BASE_ROLE,
                        format_args!("wl_surface@{sid} already has an xdg_surface"),
                    ));
                }
                if matches!(s.pending.buffer, Some(Some(_))) || s.window.is_some() {
                    return Err(self.fail(
                        host,
                        r.id,
                        WM_BASE_INVALID_SURFACE_STATE,
                        format_args!("wl_surface@{sid} has a buffer attached"),
                    ));
                }
                let x = XdgSurface {
                    surface: sid,
                    wm_base: r.id,
                    toplevel: None,
                    popup: None,
                    geometry: None,
                    pending_geometry: None,
                    sent: None,
                    acked: false,
                    unacked: Vec::new(),
                    place: None,
                };
                self.insert(host, id, XDG_SURFACE, r.version, Kind::XdgSurface(x))?;
                if let Some(s) = self.surface(sid) {
                    s.xdg = Some(id);
                }
                Ok(())
            }
            // pong(serial): no pings are sent yet.
            (3, _) => Ok(()),
            _ => Ok(()),
        }
    }

    fn xdg_surface<H: Host<Fd = F>>(&mut self, host: &mut H, r: Req<'_, F>) -> Result {
        let x = r.id;
        let Some(xs) = self.xdg(x) else { return Ok(()) };
        let (surface, toplevel, popup, sent) = (xs.surface, xs.toplevel, xs.popup, xs.sent);
        let role = toplevel.or(popup);
        match (r.op, r.args) {
            (0, []) => {
                if role.is_some() {
                    let what = if toplevel.is_some() { "xdg_toplevel" } else { "xdg_popup" };
                    return Err(self.fail(
                        host,
                        x,
                        XDG_SURFACE_DEFUNCT_ROLE_OBJECT,
                        format_args!("xdg_surface destroyed before its {what}"),
                    ));
                }
                if let Some(s) = self.surface(surface) {
                    s.xdg = None;
                }
                self.delete(host, x)
            }
            // get_toplevel(id)
            (1, [Arg::NewId(id)]) => {
                if role.is_some() {
                    return Err(self.fail(
                        host,
                        x,
                        XDG_SURFACE_ALREADY_CONSTRUCTED,
                        format_args!("xdg_surface@{x} already has a role"),
                    ));
                }
                let t = Toplevel {
                    xdg: x,
                    decoration: None,
                    title: Vec::new(),
                    app_id: Vec::new(),
                    sizing: Sizing::default(),
                };
                self.insert(host, *id, XDG_TOPLEVEL, r.version, Kind::Toplevel(t))?;
                if let Some(xs) = self.xdg(x) {
                    xs.toplevel = Some(*id);
                }
                host.note(Note::Role { surface, role: "xdg_toplevel", object: *id });
                Ok(())
            }
            // get_popup(id, parent, positioner)
            (2, [Arg::NewId(id), Arg::Object(parent), Arg::Object(Some(pos))]) => {
                self.get_popup(host, x, role, *id, *parent, *pos, r.version)
            }
            // set_window_geometry(x, y, width, height): applied at commit;
            // a popup's places it (and its children), a toplevel's is not
            // followed yet.
            (3, [Arg::Int(gx), Arg::Int(gy), Arg::Int(w), Arg::Int(h)]) => {
                if *w <= 0 || *h <= 0 {
                    return Err(self.fail(
                        host,
                        x,
                        XDG_SURFACE_INVALID_SIZE,
                        format_args!("invalid window geometry {w}x{h}"),
                    ));
                }
                if toplevel.is_some() {
                    host.note(Note::Ignored { interface: r.iface.name, request: r.sig.name });
                }
                if let Some(xs) = self.xdg(x) {
                    xs.pending_geometry = Some((*gx, *gy, *w, *h));
                }
                Ok(())
            }
            // ack_configure(serial)
            (4, [Arg::Uint(serial)]) => {
                if role.is_none() {
                    return Err(self.fail(
                        host,
                        x,
                        XDG_SURFACE_NOT_CONSTRUCTED,
                        format_args!("ack_configure without a role"),
                    ));
                }
                let serial = *serial;
                let Some(xs) = self.xdg(x) else { return Ok(()) };
                let Some(i) = xs.unacked.iter().position(|(s, _)| *s == serial) else {
                    return Err(self.fail(
                        host,
                        x,
                        XDG_SURFACE_INVALID_SERIAL,
                        format_args!(
                            "ack_configure of serial {serial}, not one sent and not acked (last sent {sent:?})"
                        ),
                    ));
                };
                // Acking one acks those before it.
                let at = xs.unacked[i].1;
                xs.unacked.drain(..=i);
                if at.is_some() {
                    xs.place = at;
                }
                xs.acked = true;
                Ok(())
            }
            _ => Ok(()),
        }
    }

    fn toplevel_request<H: Host<Fd = F>>(&mut self, host: &mut H, r: Req<'_, F>) -> Result {
        let t = r.id;
        let Some(top) = self.toplevel(t) else { return Ok(()) };
        let x = top.xdg;
        match (r.op, r.args) {
            // destroy: unmapped, the xdg_surface without a role again. Its
            // decoration object must be gone first.
            (0, []) => {
                if let Some(d) = self.toplevel(t).and_then(|top| top.decoration) {
                    return Err(self.fail(
                        host,
                        d,
                        DECORATION_ORPHANED,
                        format_args!("xdg_toplevel@{t} destroyed before its decoration"),
                    ));
                }
                let surface = match self.xdg(x) {
                    Some(xs) => {
                        xs.toplevel = None;
                        xs.surface
                    }
                    None => 0,
                };
                self.unmap(host, surface, x)?;
                self.delete(host, t)
            }
            // set_title(title), set_app_id(app_id)
            (2 | 3, [Arg::String(Some(text))]) => {
                // Cut to TITLE_MAX bytes on a character boundary.
                let mut n = text.len().min(TITLE_MAX);
                while n > 0 && n < text.len() && text[n] & 0xc0 == 0x80 {
                    n -= 1;
                }
                let mut kept = Vec::new();
                if kept.try_extend_from_slice(&text[..n]).is_err() {
                    return Err(self.out_of_memory(host));
                }
                let Some(top) = self.toplevel(t) else { return Ok(()) };
                if r.op == 2 {
                    top.title = kept;
                } else {
                    top.app_id = kept;
                    return Ok(());
                }
                let surface = match self.object(x) {
                    Some(Object { kind: Kind::XdgSurface(xs), .. }) => xs.surface,
                    _ => 0,
                };
                let window = self.surface(surface).and_then(|s| s.window);
                if let (Some(w), Some(top)) = (window, self.toplevel(t)) {
                    let title = core::mem::take(&mut top.title);
                    host.window_title(w, &title);
                    if let Some(top) = self.toplevel(t) {
                        top.title = title;
                    }
                }
                Ok(())
            }
            // set_maximized, unset_maximized, set_minimized: the desktop's
            // action on its window (as its caption buttons), which comes
            // back as a configure; before the window is shown, nothing to
            // act on yet.
            (9 | 10 | 13, []) => {
                let act = match r.op {
                    9 => Act::Maximize,
                    10 => Act::Restore,
                    _ => Act::Minimize,
                };
                let surface = match self.object(x) {
                    Some(Object { kind: Kind::XdgSurface(xs), .. }) => xs.surface,
                    _ => 0,
                };
                match self.surface(surface).and_then(|s| s.window) {
                    Some(w) => host.window_act(w, act),
                    None => {
                        host.note(Note::Ignored { interface: r.iface.name, request: r.sig.name })
                    }
                }
                Ok(())
            }
            // set_parent, show_window_menu, move, resize, set_max_size,
            // set_min_size, set_fullscreen, unset_fullscreen: accepted, not
            // acted on yet.
            _ => {
                host.note(Note::Ignored { interface: r.iface.name, request: r.sig.name });
                Ok(())
            }
        }
    }

    fn seat_request<H: Host<Fd = F>>(&mut self, host: &mut H, r: Req<'_, F>) -> Result {
        match (r.op, r.args) {
            // get_pointer(id): told where the pointer is if on a surface.
            (0, [Arg::NewId(id)]) => {
                self.insert(host, *id, WL_POINTER, r.version, Kind::Pointer)?;
                if let Some(sid) = self.seat.pointer {
                    let (x, y) = self.seat.at;
                    self.pointer_enter_one(host, *id, r.version, sid, x, y)?;
                }
                Ok(())
            }
            // get_keyboard(id): the keymap and repeat rate, then the focus
            // if the client has it.
            (1, [Arg::NewId(id)]) => {
                let id = *id;
                self.insert(host, id, WL_KEYBOARD, r.version, Kind::Keyboard)?;
                let Some((fd, size)) = host.keymap() else { return Err(self.out_of_memory(host)) };
                let args = [Arg::Uint(input::KEYMAP_XKB_V1), Arg::Fd, Arg::Uint(size)];
                self.event_fd(host, id, WL_KEYBOARD, 0, &args, fd)?;
                if r.version >= 4 {
                    let rate = [Arg::Int(input::REPEAT_RATE), Arg::Int(input::REPEAT_DELAY)];
                    self.event(host, id, WL_KEYBOARD, 5, &rate)?;
                }
                if let Some(sid) = self.seat.keyboard {
                    self.keyboard_enter_one(host, id, sid)?;
                }
                Ok(())
            }
            // get_touch: the seat has never had touch.
            (2, _) => Err(self.fail(
                host,
                r.id,
                SEAT_MISSING_CAPABILITY,
                format_args!("wl_seat@{} has no touch capability", r.id),
            )),
            // release (version 5)
            (3, []) => self.delete(host, r.id),
            _ => Ok(()),
        }
    }

    fn pointer_request<H: Host<Fd = F>>(&mut self, host: &mut H, r: Req<'_, F>) -> Result {
        match (r.op, r.args) {
            // set_cursor(serial, surface, x, y): the desktop draws its own
            // pointer; a surface with another role is an error.
            (0, [_, Arg::Object(surface), _, _]) => {
                if let Some(sid) = *surface
                    && self.surface(sid).is_some_and(|s| s.xdg.is_some() || s.window.is_some())
                {
                    return Err(self.fail(
                        host,
                        r.id,
                        POINTER_ROLE,
                        format_args!("wl_surface@{sid} has another role than cursor"),
                    ));
                }
                if surface.is_some() {
                    host.note(Note::Ignored { interface: r.iface.name, request: r.sig.name });
                }
                Ok(())
            }
            // release (version 3)
            (1, []) => self.delete(host, r.id),
            _ => Ok(()),
        }
    }

    fn decoration_manager<H: Host<Fd = F>>(&mut self, host: &mut H, r: Req<'_, F>) -> Result {
        match (r.op, r.args) {
            (0, []) => self.delete(host, r.id),
            // get_toplevel_decoration(id, toplevel)
            (1, [Arg::NewId(id), Arg::Object(Some(t))]) => {
                let (id, t) = (*id, *t);
                let Some(top) = self.toplevel(t) else { return Ok(()) };
                let (x, had) = (top.xdg, top.decoration);
                if had.is_some() {
                    return Err(self.fail(
                        host,
                        r.id,
                        DECORATION_ALREADY_CONSTRUCTED,
                        format_args!("xdg_toplevel@{t} already has a decoration object"),
                    ));
                }
                let surface = match self.object(x) {
                    Some(Object { kind: Kind::XdgSurface(xs), .. }) => xs.surface,
                    _ => 0,
                };
                if self.surface(surface).is_some_and(|s| {
                    s.window.is_some() || matches!(s.pending.buffer, Some(Some(_)))
                }) {
                    return Err(self.fail(
                        host,
                        r.id,
                        DECORATION_UNCONFIGURED_BUFFER,
                        format_args!("xdg_toplevel@{t} has a buffer attached or committed"),
                    ));
                }
                let d = Decoration { toplevel: t, wanted: None };
                self.insert(host, id, TOPLEVEL_DECORATION, r.version, Kind::Decoration(d))?;
                if let Some(top) = self.toplevel(t) {
                    top.decoration = Some(id);
                }
                // Made after the first configure: a configure tells it the
                // mode (before it, the first one does).
                if self.xdg(x).is_some_and(|xs| xs.sent.is_some()) {
                    self.configure(host, x, t)?;
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }

    fn data_device_manager<H: Host<Fd = F>>(&mut self, host: &mut H, r: Req<'_, F>) -> Result {
        match (r.op, r.args) {
            // create_data_source(id)
            (0, [Arg::NewId(id)]) => {
                let kind = Kind::DataSource(DataSource::default());
                self.insert(host, *id, WL_DATA_SOURCE, r.version, kind)
            }
            // get_data_device(id, seat): the client's selection, none, is
            // told before each keyboard focus (`keyboard_enter`).
            (1, [Arg::NewId(id), Arg::Object(Some(_))]) => {
                self.insert(host, *id, WL_DATA_DEVICE, r.version, Kind::DataDevice)
            }
            _ => Ok(()),
        }
    }

    fn data_source_request<H: Host<Fd = F>>(&mut self, host: &mut H, r: Req<'_, F>) -> Result {
        let id = r.id;
        let Some(Object { kind: Kind::DataSource(d), .. }) = self.object_mut(id) else {
            return Ok(());
        };
        match (r.op, r.args) {
            // offer(mime_type)
            (0, [Arg::String(_)]) => {
                d.mime_types = d.mime_types.saturating_add(1);
                Ok(())
            }
            // destroy: the selection it was goes with it.
            (1, []) => {
                if self.selection == Some(id) {
                    self.selection = None;
                }
                self.delete(host, id)
            }
            // set_actions(dnd_actions): once, only known actions, and only
            // for a source not used yet (for a drag: not as a selection).
            (2, [Arg::Uint(actions)]) => {
                let (bad, again, used) = (actions & !DND_ACTIONS != 0, d.actions.is_some(), d.used);
                if bad || again {
                    let why = if bad { "invalid action mask" } else { "actions already set" };
                    return Err(self.fail(
                        host,
                        id,
                        DATA_SOURCE_INVALID_ACTION_MASK,
                        format_args!("wl_data_source@{id}: {why} ({actions:#x})"),
                    ));
                }
                if used {
                    return Err(self.fail(
                        host,
                        id,
                        DATA_SOURCE_INVALID_SOURCE,
                        format_args!("wl_data_source@{id} was used already"),
                    ));
                }
                d.actions = Some(*actions);
                Ok(())
            }
            _ => Ok(()),
        }
    }

    /// Takes source `sid` for set_selection or start_drag: once only
    /// (`used_source` on the device), and a drag's source (one with
    /// actions) is no selection (`invalid_source` on the source).
    fn use_source<H: Host<Fd = F>>(
        &mut self,
        host: &mut H,
        device: u32,
        sid: u32,
        selection: bool,
    ) -> Result {
        let Some(Object { kind: Kind::DataSource(d), .. }) = self.object_mut(sid) else {
            return Ok(());
        };
        let (used, drag) = (d.used, d.actions.is_some());
        d.used = true;
        if used {
            return Err(self.fail(
                host,
                device,
                DATA_DEVICE_USED_SOURCE,
                format_args!("wl_data_source@{sid} has already been used"),
            ));
        }
        if selection && drag {
            return Err(self.fail(
                host,
                sid,
                DATA_SOURCE_INVALID_SOURCE,
                format_args!("wl_data_source@{sid} has drag-and-drop actions: not a selection"),
            ));
        }
        Ok(())
    }

    fn data_device_request<H: Host<Fd = F>>(&mut self, host: &mut H, r: Req<'_, F>) -> Result {
        match (r.op, r.args) {
            // start_drag(source, origin, icon, serial): drag-and-drop is not
            // done yet: the source, if any, is cancelled at once, as for a
            // drag the compositor ended.
            (0, [Arg::Object(source), Arg::Object(Some(_)), Arg::Object(icon), _]) => {
                if let Some(icon) = *icon
                    && self.surface(icon).is_some_and(|s| s.xdg.is_some() || s.window.is_some())
                {
                    return Err(self.fail(
                        host,
                        r.id,
                        DATA_DEVICE_ROLE,
                        format_args!("wl_surface@{icon} has another role than a drag icon"),
                    ));
                }
                host.note(Note::Ignored { interface: r.iface.name, request: r.sig.name });
                if let Some(sid) = *source {
                    self.use_source(host, r.id, sid, false)?;
                    self.event(host, sid, WL_DATA_SOURCE, DATA_SOURCE_CANCELLED, &[])?;
                }
                Ok(())
            }
            // set_selection(source, serial): the client's selection; the
            // one it replaces is cancelled. Offered to no other client (nor
            // to the Windows clipboard) yet.
            (1, [Arg::Object(source), _]) => {
                if let Some(sid) = *source {
                    self.use_source(host, r.id, sid, true)?;
                }
                let old = core::mem::replace(&mut self.selection, *source);
                if let Some(old) = old.filter(|&o| Some(o) != *source) {
                    self.event(host, old, WL_DATA_SOURCE, DATA_SOURCE_CANCELLED, &[])?;
                }
                if source.is_some() {
                    host.note(Note::Ignored { interface: r.iface.name, request: r.sig.name });
                }
                Ok(())
            }
            // release (version 2)
            (2, []) => self.delete(host, r.id),
            _ => Ok(()),
        }
    }

    /// wl_data_device.selection before a keyboard focus: no selection
    /// (nil) unless the client holds its own, which it keeps knowing.
    fn selection_before_focus<H: Host<Fd = F>>(&mut self, host: &mut H) -> Result {
        if self.selection.is_some() {
            return Ok(());
        }
        for i in 1..self.objects.len() {
            if matches!(&self.objects[i], Some(o) if matches!(o.kind, Kind::DataDevice)) {
                self.event(
                    host,
                    i as u32,
                    WL_DATA_DEVICE,
                    DATA_DEVICE_SELECTION,
                    &[Arg::Object(None)],
                )?;
            }
        }
        Ok(())
    }

    fn decoration_request<H: Host<Fd = F>>(&mut self, host: &mut H, r: Req<'_, F>) -> Result {
        let d = r.id;
        let t = match self.object(d) {
            Some(Object { kind: Kind::Decoration(dec), .. }) => dec.toplevel,
            _ => return Ok(()),
        };
        let wanted = match (r.op, r.args) {
            // destroy: back to no server-side decoration at the next
            // mapping (a mapped window keeps its frame).
            (0, []) => {
                if let Some(top) = self.toplevel(t) {
                    top.decoration = None;
                }
                return self.delete(host, d);
            }
            // set_mode(mode)
            (1, [Arg::Uint(mode)]) => {
                if *mode != MODE_CLIENT_SIDE && *mode != MODE_SERVER_SIDE {
                    return Err(self.fail(
                        host,
                        d,
                        DECORATION_INVALID_MODE,
                        format_args!("invalid decoration mode {mode}"),
                    ));
                }
                Some(*mode)
            }
            // unset_mode
            (2, []) => None,
            _ => return Ok(()),
        };
        if let Some(Object { kind: Kind::Decoration(dec), .. }) = self.object_mut(d) {
            dec.wanted = wanted;
        }
        // Answered by a configure, once the first one has gone (until then
        // the first one carries it).
        let x = self.toplevel(t).map_or(0, |top| top.xdg);
        if self.xdg(x).is_some_and(|xs| xs.sent.is_some()) {
            self.configure(host, x, t)?;
        }
        Ok(())
    }

    // ---- Input ------------------------------------------------------------

    /// The client's surface that window `window` shows.
    fn surface_of(&self, window: u32) -> Option<u32> {
        self.objects.iter().enumerate().find_map(|(i, o)| match o {
            Some(Object { kind: Kind::Surface(s), .. }) if s.window == Some(window) => {
                Some(i as u32)
            }
            _ => None,
        })
    }

    /// The window showing surface `sid` (0: none), for the notes.
    fn window_of(&self, sid: u32) -> u32 {
        match self.object(sid) {
            Some(Object { kind: Kind::Surface(s), .. }) => s.window.unwrap_or(0),
            _ => 0,
        }
    }

    /// The desktop's input for one of the client's windows (see the
    /// module's doc).
    pub fn input<H: Host<Fd = F>>(&mut self, host: &mut H, input: Input) -> Result {
        let window = match input {
            Input::Focus { window, .. }
            | Input::Motion { window, .. }
            | Input::Button { window, .. }
            | Input::Wheel { window, .. }
            | Input::Key { window, .. }
            | Input::Left { window }
            | Input::Close { window }
            | Input::Maximize { window }
            | Input::Restore { window }
            | Input::Place { window, .. }
            | Input::PopupDone { window }
            | Input::Unhandled { window, .. } => window,
        };
        // The desktop ended the grab: its popups go, whether or not the
        // window it named is still there (destroyed meanwhile).
        if let Input::PopupDone { .. } = input
            && (!self.grabs.is_empty() || self.surface_of(window).is_some())
        {
            return self.dismiss_grabs(host, "a press outside the client's windows");
        }
        let Some(sid) = self.surface_of(window) else {
            host.note(Note::InputDropped { window });
            return Ok(());
        };
        let note = |what, code, state| Note::Input { what, surface: sid, window, code, state };
        match input {
            // The activated state, and the keyboard focus with it (the
            // topmost grabbing popup keeps it while there is one).
            Input::Focus { active: true, .. } => {
                let old = self.seat.active;
                let focus = self.grab_focus().unwrap_or(sid);
                if old != Some(sid) || self.seat.keyboard != Some(focus) {
                    self.seat.active = Some(sid);
                    if self.seat.keyboard != Some(focus) {
                        self.keyboard_enter(host, focus)?;
                    }
                    if old != Some(sid) {
                        if let Some(o) = old.and_then(|o| self.toplevel_of(o)) {
                            self.reconfigure(host, o)?;
                        }
                        if let Some(t) = self.toplevel_of(sid) {
                            self.reconfigure(host, t)?;
                        }
                    }
                }
            }
            // Another window is active: the client's popup grab ends (the
            // user went elsewhere), then its keyboard focus.
            Input::Focus { active: false, .. } => {
                if self.seat.active == Some(sid) || self.seat.keyboard == Some(sid) {
                    let was = self.seat.active.take();
                    self.dismiss_grabs(host, "its window is no longer the active one")?;
                    if self.seat.keyboard.is_some() {
                        self.keyboard_leave(host)?;
                    }
                    if let Some(t) = was.and_then(|w| self.toplevel_of(w)) {
                        self.reconfigure(host, t)?;
                    }
                }
            }
            Input::Maximize { .. } | Input::Restore { .. } | Input::Place { .. } => {
                self.desktop_sizing(host, sid, window, input)?;
            }
            Input::Motion { x, y, .. } => {
                let (x, y) = self.surface_point(sid, x, y);
                self.pointer_at(host, sid, x, y)?;
            }
            Input::Button { x, y, button, pressed, .. } => {
                let (x, y) = self.surface_point(sid, x, y);
                let bit = if button == input::BTN_LEFT { 1 } else { 2 };
                if !pressed && self.seat.buttons & bit == 0 {
                    // Pressed elsewhere: not this client's.
                    return Ok(());
                }
                if !self.pointer_at(host, sid, x, y)? {
                    return Ok(());
                }
                if pressed {
                    self.seat.buttons |= bit;
                } else {
                    self.seat.buttons &= !bit;
                }
                let (serial, time) = (self.next_serial(), host.now_ms());
                if pressed {
                    self.seat.press = Some(serial);
                }
                let args = [
                    Arg::Uint(serial),
                    Arg::Uint(time),
                    Arg::Uint(button),
                    Arg::Uint(pressed.into()),
                ];
                self.send_all(host, WL_POINTER, 3, (1, u32::MAX), &args)?;
                self.send_all(host, WL_POINTER, 5, (5, u32::MAX), &[])?;
                host.note(note("button", button, pressed.into()));
            }
            Input::Wheel { x, y, delta, .. } => {
                let (x, y) = self.surface_point(sid, x, y);
                if delta == 0 || !self.pointer_at(host, sid, x, y)? {
                    return Ok(());
                }
                let time = host.now_ms();
                // Away from the user scrolls up: negative on the vertical
                // axis (0).
                let value = -delta * input::AXIS_STEP * 256 / 120;
                // axis_source wheel, then axis_discrete (5 to 7) or
                // axis_value120 (8 on), then axis, in one frame.
                self.send_all(host, WL_POINTER, 6, (5, u32::MAX), &[Arg::Uint(0)])?;
                let steps = [Arg::Uint(0), Arg::Int(-delta / 120)];
                if delta % 120 == 0 {
                    self.send_all(host, WL_POINTER, 8, (5, 8), &steps)?;
                }
                self.send_all(
                    host,
                    WL_POINTER,
                    9,
                    (8, u32::MAX),
                    &[Arg::Uint(0), Arg::Int(-delta)],
                )?;
                let axis = [Arg::Uint(time), Arg::Uint(0), Arg::Fixed(value)];
                self.send_all(host, WL_POINTER, 4, (1, u32::MAX), &axis)?;
                self.send_all(host, WL_POINTER, 5, (5, u32::MAX), &[])?;
                host.note(note("axis", 0, delta as u32));
            }
            // To the topmost grabbing popup while there is one.
            Input::Key { key, pressed, .. } => {
                let target = self.grab_focus().unwrap_or(sid);
                self.key(host, target, key, pressed)?
            }
            Input::Left { .. } => {
                if self.seat.pointer == Some(sid) && self.seat.buttons == 0 {
                    self.pointer_leave(host)?;
                }
            }
            Input::Close { .. } => {
                let toplevel =
                    self.surface(sid).and_then(|s| s.xdg).and_then(|x| match self.object(x) {
                        Some(Object { kind: Kind::XdgSurface(xs), .. }) => xs.toplevel,
                        _ => None,
                    });
                if let Some(t) = toplevel {
                    self.event(host, t, XDG_TOPLEVEL, 1, &[])?;
                    host.note(note("close", 0, 0));
                }
            }
            Input::Unhandled { what, .. } => host.note(Note::Unhandled { what, window }),
            // Handled above (no grab: nothing to end).
            Input::PopupDone { .. } => {}
        }
        Ok(())
    }

    /// The surface of the topmost shown grabbing popup (it has the
    /// keyboard focus).
    fn grab_focus(&self) -> Option<u32> {
        self.grabs.iter().rev().find_map(|&p| {
            let x = match self.object(p) {
                Some(Object { kind: Kind::Popup(pp), .. }) => pp.xdg,
                _ => return None,
            };
            let sid = match self.object(x) {
                Some(Object { kind: Kind::XdgSurface(xs), .. }) => xs.surface,
                _ => return None,
            };
            match self.object(sid) {
                Some(Object { kind: Kind::Surface(s), .. }) if s.window.is_some() => Some(sid),
                _ => None,
            }
        })
    }

    /// The toplevel surface `sid` has.
    fn toplevel_of(&self, sid: u32) -> Option<u32> {
        let x = match self.object(sid) {
            Some(Object { kind: Kind::Surface(s), .. }) => s.xdg?,
            _ => return None,
        };
        match self.object(x) {
            Some(Object { kind: Kind::XdgSurface(xs), .. }) => xs.toplevel,
            _ => None,
        }
    }

    /// The desktop sizes window `window` (surface `sid`): maximized (the
    /// work area; the window's rectangle kept for restoring), restored
    /// (to that rectangle), or placed (Snap: tiled against the work area's
    /// left or right edge and its top and bottom). The toplevel gets a
    /// configure of that surface size; the window moves and takes the
    /// buffer's size at the commit after the client acked it.
    fn desktop_sizing<H: Host<Fd = F>>(
        &mut self,
        host: &mut H,
        sid: u32,
        window: u32,
        input: Input,
    ) -> Result {
        let Some(t) = self.toplevel_of(sid) else { return Ok(()) };
        let decorated = self.surface(sid).is_some_and(|s| s.decorated);
        let (work_w, work_h) = host.work_area();
        let outer = host.window_outer(window);
        let Some(top) = self.toplevel(t) else { return Ok(()) };
        let mut z = top.sizing;
        let what = match input {
            Input::Maximize { .. } => {
                if !z.maximized {
                    z.restore = outer;
                }
                z.maximized = true;
                z.tiled = 0;
                z.size = host.surface_size(work_w, work_h, decorated);
                z.target = Some((0, 0));
                "maximize"
            }
            Input::Restore { .. } => {
                if !z.maximized {
                    // Not maximized here (the desktop restores a
                    // minimized window itself).
                    return Ok(());
                }
                z.maximized = false;
                z.tiled = 0;
                match z.restore.take() {
                    Some((x, y, w, h)) => {
                        z.size = host.surface_size(w, h, decorated);
                        z.target = Some((x, y));
                    }
                    // Maximized by a place: the client chooses again.
                    None => {
                        z.size = (0, 0);
                        z.target = None;
                    }
                }
                "restore"
            }
            Input::Place { x, y, w, h, .. } => {
                let fills = (x, y) == (0, 0) && (w, h) == (work_w, work_h);
                if fills && !z.maximized {
                    z.restore = outer;
                } else if !fills {
                    z.restore = None;
                }
                z.maximized = fills;
                // Against the work area's left or right edge, its height:
                // tiled on those sides (as the desktop marks it snapped).
                let (left, right) = (x == 0, i64::from(x) + i64::from(w) == i64::from(work_w));
                z.tiled = 0;
                if !fills && y == 0 && h == work_h && (left || right) {
                    z.tiled = 1 << STATE_TILED_TOP | 1 << STATE_TILED_BOTTOM;
                    if left {
                        z.tiled |= 1 << STATE_TILED_LEFT;
                    }
                    if right {
                        z.tiled |= 1 << STATE_TILED_RIGHT;
                    }
                }
                z.size = host.surface_size(w, h, decorated);
                z.target = Some((x, y));
                "place"
            }
            _ => return Ok(()),
        };
        if let Some(top) = self.toplevel(t) {
            top.sizing = z;
        }
        host.note(Note::Input { what, surface: sid, window, code: 0, state: 0 });
        self.reconfigure(host, t)
    }

    /// The pointer at (x, y) on surface `sid` (from the desktop's window):
    /// enter (leaving another surface), or motion if it moved. Outside the
    /// surface it leaves, unless a button holds it there. Whether the
    /// pointer is on `sid` now.
    fn pointer_at<H: Host<Fd = F>>(
        &mut self,
        host: &mut H,
        sid: u32,
        x: i32,
        y: i32,
    ) -> Result<bool> {
        let (w, h) = self.surface(sid).map_or((0, 0), |s| s.size);
        let inside = x >= 0 && y >= 0 && (x as u32) < w && (y as u32) < h;
        let held = self.seat.buttons != 0 && self.seat.pointer == Some(sid);
        if !inside && !held {
            if self.seat.pointer == Some(sid) {
                self.pointer_leave(host)?;
            }
            return Ok(false);
        }
        if self.seat.pointer != Some(sid) {
            if self.seat.pointer.is_some() {
                self.pointer_leave(host)?;
            }
            self.seat.pointer = Some(sid);
            self.seat.at = (x, y);
            for i in 1..self.objects.len() {
                let v = match &self.objects[i] {
                    Some(o) if matches!(o.kind, Kind::Pointer) => o.version,
                    _ => continue,
                };
                self.pointer_enter_one(host, i as u32, v, sid, x, y)?;
            }
            host.note(Note::Input {
                what: "enter",
                surface: sid,
                window: self.window_of(sid),
                code: 0,
                state: 0,
            });
        } else if self.seat.at != (x, y) {
            self.seat.at = (x, y);
            let time = host.now_ms();
            let args = [
                Arg::Uint(time),
                Arg::Fixed(wl_abi::fixed_from_int(x)),
                Arg::Fixed(wl_abi::fixed_from_int(y)),
            ];
            self.send_all(host, WL_POINTER, 2, (1, u32::MAX), &args)?;
            self.send_all(host, WL_POINTER, 5, (5, u32::MAX), &[])?;
        }
        Ok(true)
    }

    /// wl_pointer.enter (and its frame) on pointer `p`.
    fn pointer_enter_one<H: Host<Fd = F>>(
        &mut self,
        host: &mut H,
        p: u32,
        version: u32,
        sid: u32,
        x: i32,
        y: i32,
    ) -> Result {
        let serial = self.next_serial();
        let args = [
            Arg::Uint(serial),
            Arg::Object(Some(sid)),
            Arg::Fixed(wl_abi::fixed_from_int(x)),
            Arg::Fixed(wl_abi::fixed_from_int(y)),
        ];
        self.event(host, p, WL_POINTER, 0, &args)?;
        if version >= 5 {
            self.event(host, p, WL_POINTER, 5, &[])?;
        }
        Ok(())
    }

    /// The pointer leaves its surface.
    fn pointer_leave<H: Host<Fd = F>>(&mut self, host: &mut H) -> Result {
        let Some(sid) = self.seat.pointer.take() else { return Ok(()) };
        let serial = self.next_serial();
        self.send_all(
            host,
            WL_POINTER,
            1,
            (1, u32::MAX),
            &[Arg::Uint(serial), Arg::Object(Some(sid))],
        )?;
        self.send_all(host, WL_POINTER, 5, (5, u32::MAX), &[])?;
        host.note(Note::Input {
            what: "leave",
            surface: sid,
            window: self.window_of(sid),
            code: 0,
            state: 0,
        });
        Ok(())
    }

    /// The keyboard focus moves to `sid` (leaving another surface): enter
    /// with no keys down, then the modifiers.
    fn keyboard_enter<H: Host<Fd = F>>(&mut self, host: &mut H, sid: u32) -> Result {
        if self.seat.keyboard.is_some() {
            self.keyboard_leave(host)?;
        }
        self.seat.keyboard = Some(sid);
        self.seat.keys.clear();
        self.seat.depressed = 0;
        self.selection_before_focus(host)?;
        for i in 1..self.objects.len() {
            if matches!(&self.objects[i], Some(o) if matches!(o.kind, Kind::Keyboard)) {
                self.keyboard_enter_one(host, i as u32, sid)?;
            }
        }
        host.note(Note::Input {
            what: "focus",
            surface: sid,
            window: self.window_of(sid),
            code: 0,
            state: 1,
        });
        Ok(())
    }

    /// wl_keyboard.enter and modifiers on keyboard `k`.
    fn keyboard_enter_one<H: Host<Fd = F>>(&mut self, host: &mut H, k: u32, sid: u32) -> Result {
        let serial = self.next_serial();
        let args = [Arg::Uint(serial), Arg::Object(Some(sid)), Arg::Array(&[])];
        self.event(host, k, WL_KEYBOARD, 1, &args)?;
        let serial = self.next_serial();
        let mods = [
            Arg::Uint(serial),
            Arg::Uint(self.seat.depressed),
            Arg::Uint(0),
            Arg::Uint(self.seat.locked),
            Arg::Uint(0),
        ];
        self.event(host, k, WL_KEYBOARD, 4, &mods)
    }

    /// The keyboard focus leaves its surface (the keys down are forgotten:
    /// their releases go to the next focus).
    fn keyboard_leave<H: Host<Fd = F>>(&mut self, host: &mut H) -> Result {
        let Some(sid) = self.seat.keyboard.take() else { return Ok(()) };
        self.seat.keys.clear();
        self.seat.depressed = 0;
        let serial = self.next_serial();
        self.send_all(
            host,
            WL_KEYBOARD,
            2,
            (1, u32::MAX),
            &[Arg::Uint(serial), Arg::Object(Some(sid))],
        )?;
        host.note(Note::Input {
            what: "unfocus",
            surface: sid,
            window: self.window_of(sid),
            code: 0,
            state: 0,
        });
        Ok(())
    }

    /// A key on surface `sid`: wl_keyboard.key, then the modifiers if it
    /// changed them. A key already down (the device's repeat) or released
    /// without having been pressed here is not sent.
    fn key<H: Host<Fd = F>>(&mut self, host: &mut H, sid: u32, key: u32, pressed: bool) -> Result {
        if self.seat.keyboard != Some(sid) {
            self.keyboard_enter(host, sid)?;
        }
        let down = self.seat.keys.iter().position(|&k| k == key);
        match (pressed, down) {
            (true, Some(_)) | (false, None) => return Ok(()),
            (true, None) => {
                if self.seat.keys.len() >= MAX_KEYS || self.seat.keys.try_push(key).is_err() {
                    return Ok(());
                }
            }
            (false, Some(i)) => {
                self.seat.keys.swap_remove(i);
            }
        }
        let (serial, time) = (self.next_serial(), host.now_ms());
        if pressed {
            self.seat.press = Some(serial);
        }
        let args = [Arg::Uint(serial), Arg::Uint(time), Arg::Uint(key), Arg::Uint(pressed.into())];
        self.send_all(host, WL_KEYBOARD, 3, (1, u32::MAX), &args)?;
        host.note(Note::Input {
            what: "key",
            surface: sid,
            window: self.window_of(sid),
            code: key,
            state: pressed.into(),
        });
        let mut depressed = 0;
        for &k in &self.seat.keys {
            if let Some(Modifier::Held(m)) = input::modifier(k) {
                depressed |= m;
            }
        }
        let mut locked = self.seat.locked;
        if pressed && let Some(Modifier::Toggled(m)) = input::modifier(key) {
            locked ^= m;
        }
        if (depressed, locked) != (self.seat.depressed, self.seat.locked) {
            (self.seat.depressed, self.seat.locked) = (depressed, locked);
            let serial = self.next_serial();
            let mods = [
                Arg::Uint(serial),
                Arg::Uint(depressed),
                Arg::Uint(0),
                Arg::Uint(locked),
                Arg::Uint(0),
            ];
            self.send_all(host, WL_KEYBOARD, 4, (1, u32::MAX), &mods)?;
        }
        Ok(())
    }
}

/// What a commit showed: the buffer's size and the rows copied.
type Shown = ((u32, u32), Option<(u32, u32)>);

/// Which handler an object's requests go to.
#[derive(Clone, Copy)]
enum Tag {
    Display,
    Registry,
    Callback,
    Compositor,
    Shm,
    Pool,
    Buffer,
    Surface,
    Region,
    Output,
    WmBase,
    XdgSurface,
    Toplevel,
    Positioner,
    Popup,
    Seat,
    Pointer,
    Keyboard,
    DecorationManager,
    Decoration,
    DataDeviceManager,
    DataSource,
    DataDevice,
}

/// A decoded request.
struct Req<'a, F> {
    id: u32,
    version: u32,
    iface: &'static Interface,
    sig: &'static Message,
    op: u16,
    args: &'a [Arg<'a>],
    /// The descriptor it took.
    fd: Option<F>,
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
