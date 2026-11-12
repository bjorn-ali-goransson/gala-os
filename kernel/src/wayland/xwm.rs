//! The X window manager of rootless Xwayland (ROADMAP LD step 5;
//! ARCHITECTURE "X11 programs through Xwayland"): an X11 client of
//! Xwayland's, on the connection Xwayland makes for its window manager
//! (`-wm <fd>`), that lets each top-level X window be a window of
//! shasha's desktop. Sources: the X Window System protocol, ICCCM and
//! EWMH specifications (X.Org, freedesktop.org), read as specifications;
//! Xwayland's `WL_SURFACE_ID` message as Xwayland documents it.
//!
//! - **What it does**: it holds SubstructureRedirect on the root, so a
//!   client's MapRequest and ConfigureRequest come here; it reads the
//!   window's title (`_NET_WM_NAME`, else `WM_NAME`), protocols
//!   (`WM_DELETE_WINDOW`, `WM_TAKE_FOCUS`) and Motif decoration hint,
//!   sets `WM_STATE` and maps it. Xwayland then gives the window a
//!   wl_surface and tells the root's redirecting client its id with a
//!   `WL_SURFACE_ID` ClientMessage; the window manager hands that surface
//!   to the Wayland server (`Shell::show`), which shows it as a desktop
//!   window (framed, titled) at its first buffer. Override-redirect
//!   windows (menus, tooltips) are never redirected: their surface is
//!   shown frameless at their own root position.
//! - **Coordinates**: rootless Xwayland's root is the screen, so an X
//!   window's root position is its window's client-area position on the
//!   desktop; when the desktop places a managed window, the window
//!   manager moves the X window there (`placed`), so pointer coordinates
//!   and override-redirect menus line up.
//! - **Focus and close**: the desktop's activation becomes SetInputFocus
//!   (and `WM_TAKE_FOCUS` where the window asks for it) and
//!   `_NET_ACTIVE_WINDOW`; the frame's close button `WM_DELETE_WINDOW`,
//!   or KillClient for a window that does not take it.
//! - **Limits**: the X selections (the clipboard) are not bridged, no
//!   window is reparented (no X-side frame), `_NET_WM_STATE` requests
//!   (maximize, full screen) are not acted on (ledgered by the server),
//!   and `xwayland_shell_v1` is not offered (Xwayland falls back to
//!   `WL_SURFACE_ID`, whose surface ids a client may reuse: an id is
//!   taken when it names a surface of the Xwayland connection with no
//!   role).
//!
//! The state is pure: bytes in, bytes out and calls on `Shell`, so host
//! tests drive it (`tests`).

use alloc::collections::VecDeque;
use alloc::vec::Vec;
use core::fmt;

use crate::mm::heap::{AllocError, TryVec};

/// Bytes waiting to be written beyond which the connection is given up
/// (Xwayland stopped reading).
const MAX_OUTPUT: usize = 256 * 1024;
/// The longest message read (a reply's 32 bytes and its data: the
/// largest property asked for is TITLE_LONGS).
const MAX_MESSAGE: usize = 32 + 4 * 1024;
/// The longest title kept, in 4-byte units asked of GetProperty.
const TITLE_LONGS: u32 = 256;
/// X windows tracked at most (Xwayland's top-level windows).
const MAX_WINDOWS: usize = 4096;

// Predefined atoms (the protocol's appendix B).
const ATOM_ATOM: u32 = 4;
const ATOM_CARDINAL: u32 = 6;
const ATOM_STRING: u32 = 31;
const ATOM_WINDOW: u32 = 33;
const ATOM_WM_NAME: u32 = 39;
const ATOM_WM_TRANSIENT_FOR: u32 = 68;

/// The atoms interned at the start, in this order.
const ATOM_NAMES: [&str; 13] = [
    "WM_PROTOCOLS",
    "WM_DELETE_WINDOW",
    "WM_TAKE_FOCUS",
    "WM_STATE",
    "UTF8_STRING",
    "_NET_WM_NAME",
    "WL_SURFACE_ID",
    "_NET_SUPPORTING_WM_CHECK",
    "_NET_SUPPORTED",
    "_NET_ACTIVE_WINDOW",
    "_MOTIF_WM_HINTS",
    "WM_S0",
    "_NET_WM_STATE",
];
const WM_PROTOCOLS: usize = 0;
const WM_DELETE_WINDOW: usize = 1;
const WM_TAKE_FOCUS: usize = 2;
const WM_STATE: usize = 3;
const UTF8_STRING: usize = 4;
const NET_WM_NAME: usize = 5;
const WL_SURFACE_ID: usize = 6;
const NET_SUPPORTING_WM_CHECK: usize = 7;
const NET_SUPPORTED: usize = 8;
const NET_ACTIVE_WINDOW: usize = 9;
const MOTIF_WM_HINTS: usize = 10;
const WM_S0: usize = 11;
const NET_WM_STATE: usize = 12;

// Requests (major opcodes).
const CREATE_WINDOW: u8 = 1;
const CHANGE_WINDOW_ATTRIBUTES: u8 = 2;
const MAP_WINDOW: u8 = 8;
const CONFIGURE_WINDOW: u8 = 12;
const INTERN_ATOM: u8 = 16;
const QUERY_EXTENSION: u8 = 98;
const CHANGE_PROPERTY: u8 = 18;
const GET_PROPERTY: u8 = 20;
const SET_SELECTION_OWNER: u8 = 22;
const SEND_EVENT: u8 = 25;
const SET_INPUT_FOCUS: u8 = 42;
const KILL_CLIENT: u8 = 113;

// Events.
const CREATE_NOTIFY: u8 = 16;
const DESTROY_NOTIFY: u8 = 17;
const UNMAP_NOTIFY: u8 = 18;
const MAP_NOTIFY: u8 = 19;
const MAP_REQUEST: u8 = 20;
const CONFIGURE_NOTIFY: u8 = 22;
const CONFIGURE_REQUEST: u8 = 23;
const PROPERTY_NOTIFY: u8 = 28;
const CLIENT_MESSAGE: u8 = 33;

// Event masks.
const PROPERTY_CHANGE_MASK: u32 = 1 << 22;
const SUBSTRUCTURE_NOTIFY_MASK: u32 = 1 << 19;
const SUBSTRUCTURE_REDIRECT_MASK: u32 = 1 << 20;
// Window attribute value-mask bits.
const CW_OVERRIDE_REDIRECT: u32 = 1 << 9;
const CW_EVENT_MASK: u32 = 1 << 11;
// ConfigureWindow value-mask bits.
const CONFIG_X: u16 = 1;
const CONFIG_Y: u16 = 2;
const CONFIG_WIDTH: u16 = 4;
const CONFIG_HEIGHT: u16 = 8;
const CONFIG_BORDER: u16 = 16;
const CONFIG_SIBLING: u16 = 32;
const CONFIG_STACK: u16 = 64;

/// ICCCM's WM_STATE values.
const NORMAL_STATE: u32 = 1;
const WITHDRAWN_STATE: u32 = 0;

/// What the window manager asks of the Wayland server.
pub trait Shell {
    /// Lets Xwayland's wl_surface `surface` show X window `info.window`:
    /// false if the connection has no such surface free for it (not made
    /// yet: asked again at the next round).
    fn show(&mut self, surface: u32, info: &Info<'_>) -> bool;
    /// The window's title changed.
    fn title(&mut self, surface: u32, title: &[u8]);
    /// The X window is unmapped: the surface shows nothing any more.
    fn hide(&mut self, surface: u32);
    /// Something observability wants to know (`events=wayland`'s `xwm`).
    fn note(&mut self, what: &'static str, window: u32, detail: fmt::Arguments<'_>);
}

/// An X window as the server shows it.
pub struct Info<'a> {
    pub window: u32,
    /// Not managed (a menu or tooltip): frameless, at its root position.
    pub override_redirect: bool,
    /// Its root position (an override-redirect window's place).
    pub at: (i32, i32),
    pub title: &'a [u8],
    /// With shasha's frame (a managed window unless its Motif hint asks
    /// for none).
    pub decorated: bool,
}

/// Why the connection ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum XwmError {
    NoMemory,
    /// Xwayland refused the connection setup.
    Refused,
    /// A message the protocol does not allow (a length out of range).
    Malformed,
    /// More requests waiting than MAX_OUTPUT.
    OutputFull,
}

impl From<AllocError> for XwmError {
    fn from(_: AllocError) -> XwmError {
        XwmError::NoMemory
    }
}

type Result<T = ()> = core::result::Result<T, XwmError>;

/// A reply the window manager waits for, by its request's sequence
/// number.
#[derive(Clone, Copy, Debug)]
enum Wait {
    /// InternAtom of ATOM_NAMES[i].
    Atom(usize),
    /// GetProperty of a window's property.
    Prop(u32, Prop),
    /// QueryExtension("Composite").
    Composite,
    /// CompositeQueryVersion (its answer is not needed).
    CompositeVersion,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Prop {
    WmName,
    NetWmName,
    Protocols,
    Motif,
    TransientFor,
}

/// A top-level X window.
#[derive(Default)]
struct Window {
    id: u32,
    at: (i32, i32),
    size: (u32, u32),
    override_redirect: bool,
    /// Mapped (MapNotify seen, UnmapNotify not since).
    mapped: bool,
    /// The wl_surface Xwayland gave it (WL_SURFACE_ID).
    surface: Option<u32>,
    /// The server shows it through that surface.
    shown: bool,
    title: Vec<u8>,
    /// The title came from _NET_WM_NAME (WM_NAME no longer replaces it).
    net_title: bool,
    delete: bool,
    take_focus: bool,
    /// The Motif hint asks for no decorations.
    undecorated: bool,
    transient_for: u32,
}

enum Phase {
    /// The setup request is sent; its reply is awaited.
    Setup,
    Running,
}

/// The window manager's connection (see the module's doc).
pub struct Xwm {
    phase: Phase,
    input: Vec<u8>,
    filled: usize,
    output: Vec<u8>,
    /// Requests sent (the low 16 bits are the sequence number).
    sent: u32,
    waits: VecDeque<(u16, Wait)>,
    atoms: [u32; ATOM_NAMES.len()],
    root: u32,
    /// Resource ids: base, mask, next.
    ids: (u32, u32, u32),
    /// Our window that names the window manager (_NET_SUPPORTING_WM_CHECK).
    check: u32,
    windows: Vec<Window>,
    /// The window with the input focus (set by the window manager).
    focus: u32,
    /// Messages handled (the dump).
    pub events: u64,
    /// Every message received is noted (`wayland=trace`).
    pub trace: bool,
    /// The Composite extension's answer came (the root's children are
    /// redirected, or it is missing).
    composite: bool,
}

fn le16(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

fn le32(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

fn pad4(n: usize) -> usize {
    (n + 3) & !3
}

impl Xwm {
    /// The window manager's side of a new connection: the setup request
    /// is in its output.
    pub fn new() -> Result<Xwm> {
        let mut input = Vec::new();
        input.try_reserve_exact(MAX_MESSAGE).map_err(|_| XwmError::NoMemory)?;
        input.resize(MAX_MESSAGE, 0);
        let mut x = Xwm {
            phase: Phase::Setup,
            input,
            filled: 0,
            output: Vec::new(),
            sent: 0,
            waits: VecDeque::new(),
            atoms: [0; ATOM_NAMES.len()],
            root: 0,
            ids: (0, 0, 0),
            check: 0,
            windows: Vec::new(),
            focus: 0,
            events: 0,
            trace: false,
            composite: false,
        };
        // Little-endian, protocol 11.0, no authorization.
        x.put(&[b'l', 0, 11, 0, 0, 0, 0, 0, 0, 0, 0, 0])?;
        Ok(x)
    }

    pub fn input_room(&mut self) -> &mut [u8] {
        &mut self.input[self.filled..]
    }

    pub fn filled(&mut self, n: usize) {
        self.filled = (self.filled + n).min(self.input.len());
    }

    pub fn output(&self) -> &[u8] {
        &self.output
    }

    pub fn written(&mut self, n: usize) {
        self.output.drain(..n.min(self.output.len()));
    }

    fn put(&mut self, bytes: &[u8]) -> Result {
        if self.output.len() + bytes.len() > MAX_OUTPUT {
            return Err(XwmError::OutputFull);
        }
        self.output.try_extend_from_slice(bytes)?;
        Ok(())
    }

    /// Starts a request of `words` 4-byte units (header included); the
    /// caller puts the rest. Returns its sequence number.
    fn request(&mut self, opcode: u8, data: u8, words: usize) -> Result<u16> {
        let len = u16::try_from(words).map_err(|_| XwmError::Malformed)?;
        let l = len.to_le_bytes();
        self.put(&[opcode, data, l[0], l[1]])?;
        self.sent = self.sent.wrapping_add(1);
        Ok(self.sent as u16)
    }

    fn put32(&mut self, v: &[u32]) -> Result {
        for w in v {
            self.put(&w.to_le_bytes())?;
        }
        Ok(())
    }

    fn wait(&mut self, seq: u16, w: Wait) -> Result {
        self.waits.try_reserve(1).map_err(|_| XwmError::NoMemory)?;
        self.waits.push_back((seq, w));
        Ok(())
    }

    fn atom(&self, i: usize) -> u32 {
        self.atoms[i]
    }

    fn new_id(&mut self) -> u32 {
        let (base, mask, next) = self.ids;
        self.ids.2 = next.wrapping_add(1);
        base | (next & mask)
    }

    fn window(&mut self, id: u32) -> Option<&mut Window> {
        self.windows.iter_mut().find(|w| w.id == id)
    }

    // ---- Requests ------------------------------------------------------

    fn intern(&mut self, i: usize) -> Result {
        let name = ATOM_NAMES[i].as_bytes();
        let seq = self.request(INTERN_ATOM, 0, 2 + pad4(name.len()) / 4)?;
        self.put(&(name.len() as u16).to_le_bytes())?;
        self.put(&[0, 0])?;
        self.put(name)?;
        self.put(&[0; 3][..pad4(name.len()) - name.len()])?;
        self.wait(seq, Wait::Atom(i))
    }

    fn change_attributes(&mut self, window: u32, mask: u32, values: &[u32]) -> Result {
        self.request(CHANGE_WINDOW_ATTRIBUTES, 0, 3 + values.len())?;
        self.put32(&[window, mask])?;
        self.put32(values)
    }

    fn get_property(&mut self, window: u32, prop: Prop) -> Result {
        let (atom, kind, longs) = match prop {
            Prop::WmName => (ATOM_WM_NAME, 0, TITLE_LONGS),
            Prop::NetWmName => (self.atom(NET_WM_NAME), self.atom(UTF8_STRING), TITLE_LONGS),
            Prop::Protocols => (self.atom(WM_PROTOCOLS), ATOM_ATOM, 32),
            Prop::Motif => (self.atom(MOTIF_WM_HINTS), 0, 5),
            Prop::TransientFor => (ATOM_WM_TRANSIENT_FOR, ATOM_WINDOW, 1),
        };
        if atom == 0 {
            return Ok(());
        }
        let seq = self.request(GET_PROPERTY, 0, 6)?;
        self.put32(&[window, atom, kind, 0, longs])?;
        self.wait(seq, Wait::Prop(window, prop))
    }

    /// ChangeProperty (Replace) of 32-bit values.
    fn set_property32(&mut self, window: u32, prop: u32, kind: u32, values: &[u32]) -> Result {
        self.request(CHANGE_PROPERTY, 0, 6 + values.len())?;
        self.put32(&[window, prop, kind])?;
        self.put(&[32, 0, 0, 0])?;
        self.put32(&[values.len() as u32])?;
        self.put32(values)
    }

    fn set_property8(&mut self, window: u32, prop: u32, kind: u32, bytes: &[u8]) -> Result {
        self.request(CHANGE_PROPERTY, 0, 6 + pad4(bytes.len()) / 4)?;
        self.put32(&[window, prop, kind])?;
        self.put(&[8, 0, 0, 0])?;
        self.put32(&[bytes.len() as u32])?;
        self.put(bytes)?;
        self.put(&[0; 3][..pad4(bytes.len()) - bytes.len()])
    }

    fn configure(&mut self, window: u32, mask: u16, values: &[u32]) -> Result {
        self.request(CONFIGURE_WINDOW, 0, 3 + values.len())?;
        self.put32(&[window])?;
        self.put(&mask.to_le_bytes())?;
        self.put(&[0, 0])?;
        self.put32(values)
    }

    /// A ClientMessage of format 32 to `window` (no propagation, no mask:
    /// to its owner).
    fn client_message(&mut self, window: u32, kind: u32, data: [u32; 5]) -> Result {
        self.request(SEND_EVENT, 0, 11)?;
        self.put32(&[window, 0])?;
        self.put(&[CLIENT_MESSAGE, 32, 0, 0])?;
        self.put32(&[window, kind])?;
        self.put32(&data)
    }

    // ---- Messages ------------------------------------------------------

    /// Handles every whole message received.
    pub fn process<S: Shell>(&mut self, shell: &mut S) -> Result {
        loop {
            let need = match self.phase {
                Phase::Setup => {
                    if self.filled < 8 {
                        return Ok(());
                    }
                    8 + 4 * usize::from(le16(&self.input, 6))
                }
                Phase::Running => {
                    if self.filled < 32 {
                        return Ok(());
                    }
                    if self.input[0] == 1 { 32 + 4 * le32(&self.input, 4) as usize } else { 32 }
                }
            };
            if need > self.input.len() {
                return Err(XwmError::Malformed);
            }
            if self.filled < need {
                return Ok(());
            }
            let mut msg = [0u8; 32];
            match self.phase {
                Phase::Setup => self.setup(shell, need)?,
                Phase::Running => {
                    msg.copy_from_slice(&self.input[..32]);
                    if msg[0] == 1 {
                        self.reply(shell, need)?;
                    } else {
                        self.message(shell, &msg)?;
                    }
                }
            }
            self.events += 1;
            self.input.copy_within(need..self.filled, 0);
            self.filled -= need;
        }
    }

    fn setup<S: Shell>(&mut self, shell: &mut S, len: usize) -> Result {
        let b = &self.input[..len];
        if b[0] != 1 || len < 40 {
            shell.note("refused", 0, format_args!("setup status {}", b[0]));
            return Err(XwmError::Refused);
        }
        let (base, mask) = (le32(b, 12), le32(b, 16));
        let vendor = usize::from(le16(b, 24));
        let formats = usize::from(b[29]);
        let screen = 40 + pad4(vendor) + 8 * formats;
        if screen + 4 > len {
            return Err(XwmError::Malformed);
        }
        self.root = le32(b, screen);
        self.ids = (base, mask, 1);
        self.phase = Phase::Running;
        shell.note("connected", self.root, format_args!("root window"));
        for i in 0..ATOM_NAMES.len() {
            self.intern(i)?;
        }
        // Rootless Xwayland gives a wl_surface only to a window whose
        // drawing is redirected manually: the window manager redirects the
        // root's children (Composite's RedirectSubwindows).
        let name = b"Composite";
        let seq = self.request(QUERY_EXTENSION, 0, 2 + pad4(name.len()) / 4)?;
        self.put(&(name.len() as u16).to_le_bytes())?;
        self.put(&[0, 0])?;
        self.put(name)?;
        self.put(&[0; 3][..pad4(name.len()) - name.len()])?;
        self.wait(seq, Wait::Composite)
    }

    /// Composite's answer: its version asked (the protocol wants that
    /// first), the root's children redirected manually; then, once the
    /// atoms are there too, `start`.
    fn composite<S: Shell>(&mut self, shell: &mut S, present: bool, major: u8) -> Result {
        self.composite = true;
        if present {
            let seq = self.request(major, 0, 3)?;
            self.put32(&[0, 4])?;
            self.wait(seq, Wait::CompositeVersion)?;
            // RedirectSubwindows(root, Manual).
            self.request(major, 2, 3)?;
            self.put32(&[self.root])?;
            self.put(&[1, 0, 0, 0])?;
            shell.note("composite", self.root, format_args!("root's children redirected"));
        } else {
            shell.note("error", self.root, format_args!("no Composite extension: no window shows"));
        }
        if !self.waits.iter().any(|(_, w)| matches!(w, Wait::Atom(_))) {
            shell.note("ready", self.root, format_args!("atoms interned"));
            self.start()?;
        }
        Ok(())
    }

    /// Once every atom is known: the root redirected to us, our check
    /// window and the WM_S0 selection.
    fn start(&mut self) -> Result {
        let root = self.root;
        let mask = SUBSTRUCTURE_REDIRECT_MASK | SUBSTRUCTURE_NOTIFY_MASK | PROPERTY_CHANGE_MASK;
        self.change_attributes(root, CW_EVENT_MASK, &[mask])?;
        // An InputOnly 1x1 window, override-redirect (never managed).
        let check = self.new_id();
        self.check = check;
        self.request(CREATE_WINDOW, 0, 9)?;
        self.put32(&[check, root])?;
        self.put(&(-1i16).to_le_bytes())?;
        self.put(&(-1i16).to_le_bytes())?;
        self.put(&1u16.to_le_bytes())?;
        self.put(&1u16.to_le_bytes())?;
        self.put(&0u16.to_le_bytes())?;
        self.put(&2u16.to_le_bytes())?; // InputOnly
        self.put32(&[0, CW_OVERRIDE_REDIRECT, 1])?;
        let wm_check = self.atom(NET_SUPPORTING_WM_CHECK);
        self.set_property32(root, wm_check, ATOM_WINDOW, &[check])?;
        self.set_property32(check, wm_check, ATOM_WINDOW, &[check])?;
        let (name, utf8) = (self.atom(NET_WM_NAME), self.atom(UTF8_STRING));
        self.set_property8(check, name, utf8, b"gala")?;
        let supported = [
            self.atom(NET_SUPPORTING_WM_CHECK),
            self.atom(NET_WM_NAME),
            self.atom(NET_ACTIVE_WINDOW),
        ];
        let net_supported = self.atom(NET_SUPPORTED);
        self.set_property32(root, net_supported, ATOM_ATOM, &supported)?;
        let s0 = self.atom(WM_S0);
        self.request(SET_SELECTION_OWNER, 0, 4)?;
        self.put32(&[check, s0, 0])
    }

    fn reply<S: Shell>(&mut self, shell: &mut S, len: usize) -> Result {
        let seq = le16(&self.input, 2);
        let Some(i) = self.waits.iter().position(|(s, _)| *s == seq) else { return Ok(()) };
        let Some((_, wait)) = self.waits.remove(i) else { return Ok(()) };
        match wait {
            Wait::Atom(i) => {
                self.atoms[i] = le32(&self.input, 8);
                if self.composite && self.waits.iter().all(|(_, w)| !matches!(w, Wait::Atom(_))) {
                    shell.note("ready", self.root, format_args!("atoms interned"));
                    self.start()?;
                }
            }
            Wait::Composite => {
                let (present, major) = (self.input[8] != 0, self.input[9]);
                self.composite(shell, present, major)?;
            }
            Wait::CompositeVersion => {}
            Wait::Prop(window, prop) => {
                let b = &self.input[..len];
                let (format, kind, count) = (b[1], le32(b, 8), le32(b, 16) as usize);
                let bytes = count * usize::from(format / 8);
                if 32 + bytes > len {
                    return Err(XwmError::Malformed);
                }
                let mut data = [0u8; 4 * TITLE_LONGS as usize];
                let n = bytes.min(data.len());
                data[..n].copy_from_slice(&b[32..32 + n]);
                self.property(shell, window, prop, format, kind, &data[..n])?;
            }
        }
        Ok(())
    }

    /// A property's value arrived (format 0: the window has none).
    fn property<S: Shell>(
        &mut self,
        shell: &mut S,
        window: u32,
        prop: Prop,
        format: u8,
        kind: u32,
        data: &[u8],
    ) -> Result {
        let longs = |i: usize| (format == 32 && data.len() >= 4 * i + 4).then(|| le32(data, 4 * i));
        let (delete, take_focus) = (self.atom(WM_DELETE_WINDOW), self.atom(WM_TAKE_FOCUS));
        let Some(w) = self.window(window) else { return Ok(()) };
        let mut retitled = false;
        match prop {
            Prop::WmName => {
                if !w.net_title && format == 8 && (kind == ATOM_STRING || kind != 0) {
                    w.title.clear();
                    w.title.try_extend_from_slice(data)?;
                    retitled = true;
                }
            }
            Prop::NetWmName => {
                if format == 8 {
                    w.title.clear();
                    w.title.try_extend_from_slice(data)?;
                    w.net_title = true;
                    retitled = true;
                }
            }
            Prop::Protocols => {
                w.delete = false;
                w.take_focus = false;
                for i in 0..data.len() / 4 {
                    match longs(i) {
                        Some(a) if a == delete && a != 0 => w.delete = true,
                        Some(a) if a == take_focus && a != 0 => w.take_focus = true,
                        _ => {}
                    }
                }
            }
            Prop::Motif => {
                // flags, functions, decorations, ...: MWM_HINTS_DECORATIONS
                // (bit 1) with no decorations.
                w.undecorated = longs(0).is_some_and(|f| f & 2 != 0) && longs(2) == Some(0);
            }
            Prop::TransientFor => w.transient_for = longs(0).unwrap_or(0),
        }
        if retitled && let (Some(s), true) = (w.surface, w.shown) {
            let title = core::mem::take(&mut w.title);
            shell.title(s, &title);
            if let Some(w) = self.window(window) {
                w.title = title;
            }
        }
        Ok(())
    }

    fn message<S: Shell>(&mut self, shell: &mut S, m: &[u8; 32]) -> Result {
        if self.trace {
            shell.note("received", le32(m, 4), format_args!("code {} seq {}", m[0], le16(m, 2)));
        }
        match m[0] & 0x7f {
            0 => {
                // An error: the request it answers waits no more.
                let seq = le16(m, 2);
                if let Some(i) = self.waits.iter().position(|(s, _)| *s == seq) {
                    self.waits.remove(i);
                }
                shell.note(
                    "error",
                    le32(m, 4),
                    format_args!("code {} major {} minor {}", m[1], m[10], le16(m, 8)),
                );
            }
            CREATE_NOTIFY => {
                let id = le32(m, 8);
                if le32(m, 4) != self.root || id == self.check || self.windows.len() >= MAX_WINDOWS
                {
                    return Ok(());
                }
                let w = Window {
                    id,
                    at: (le16(m, 12) as i16 as i32, le16(m, 14) as i16 as i32),
                    size: (u32::from(le16(m, 16)), u32::from(le16(m, 18))),
                    override_redirect: m[22] != 0,
                    ..Window::default()
                };
                self.windows.try_push(w)?;
            }
            DESTROY_NOTIFY => {
                let id = le32(m, 8);
                if let Some(i) = self.windows.iter().position(|w| w.id == id) {
                    let w = self.windows.remove(i);
                    if let (Some(s), true) = (w.surface, w.shown) {
                        shell.hide(s);
                    }
                    if self.focus == id {
                        self.focus = 0;
                    }
                }
            }
            MAP_REQUEST => self.map_request(shell, le32(m, 8))?,
            MAP_NOTIFY => {
                let id = le32(m, 8);
                if let Some(w) = self.window(id) {
                    w.mapped = true;
                    w.override_redirect = m[12] != 0;
                }
                self.try_show(shell, id);
            }
            UNMAP_NOTIFY => {
                let id = le32(m, 8);
                if le32(m, 4) != self.root {
                    return Ok(());
                }
                let Some(w) = self.window(id) else { return Ok(()) };
                w.mapped = false;
                // Xwayland gives a new surface at the next map.
                let surface = w.surface.take();
                let (shown, managed) = (core::mem::take(&mut w.shown), !w.override_redirect);
                if let (Some(s), true) = (surface, shown) {
                    shell.hide(s);
                }
                if managed {
                    let state = self.atom(WM_STATE);
                    self.set_property32(id, state, state, &[WITHDRAWN_STATE, 0])?;
                }
                if self.focus == id {
                    self.focus = 0;
                }
            }
            CONFIGURE_NOTIFY => {
                let id = le32(m, 8);
                if let Some(w) = self.window(id) {
                    w.at = (le16(m, 16) as i16 as i32, le16(m, 18) as i16 as i32);
                    w.size = (u32::from(le16(m, 20)), u32::from(le16(m, 22)));
                    w.override_redirect = m[26] != 0;
                }
            }
            CONFIGURE_REQUEST => self.configure_request(m)?,
            PROPERTY_NOTIFY => {
                let (id, atom) = (le32(m, 4), le32(m, 8));
                let prop = if atom == ATOM_WM_NAME {
                    Prop::WmName
                } else if atom == self.atom(NET_WM_NAME) {
                    Prop::NetWmName
                } else if atom == self.atom(WM_PROTOCOLS) {
                    Prop::Protocols
                } else if atom == self.atom(MOTIF_WM_HINTS) {
                    Prop::Motif
                } else {
                    return Ok(());
                };
                if self.window(id).is_some() {
                    self.get_property(id, prop)?;
                }
            }
            CLIENT_MESSAGE => {
                let (id, kind) = (le32(m, 4), le32(m, 8));
                if kind == self.atom(WL_SURFACE_ID) && kind != 0 {
                    let surface = le32(m, 12);
                    shell.note("surface", id, format_args!("wl_surface {surface}"));
                    if let Some(w) = self.window(id) {
                        w.surface = Some(surface);
                        w.shown = false;
                    }
                    self.try_show(shell, id);
                } else if kind == self.atom(NET_WM_STATE) && kind != 0 {
                    shell.note("ignored", id, format_args!("_NET_WM_STATE {}", le32(m, 12)));
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// A client maps a managed window: its properties are asked for (their
    /// replies come before the surface's message), WM_STATE set, and it
    /// is mapped.
    fn map_request<S: Shell>(&mut self, shell: &mut S, id: u32) -> Result {
        if self.window(id).is_none() {
            // Created before the redirect: known from now on.
            if self.windows.len() >= MAX_WINDOWS {
                return Ok(());
            }
            self.windows.try_push(Window { id, ..Window::default() })?;
        }
        shell.note("map request", id, format_args!(""));
        self.change_attributes(id, CW_EVENT_MASK, &[PROPERTY_CHANGE_MASK])?;
        for prop in
            [Prop::WmName, Prop::NetWmName, Prop::Protocols, Prop::Motif, Prop::TransientFor]
        {
            self.get_property(id, prop)?;
        }
        let state = self.atom(WM_STATE);
        self.set_property32(id, state, state, &[NORMAL_STATE, 0])?;
        self.request(MAP_WINDOW, 0, 2)?;
        self.put32(&[id])
    }

    /// A client's configure of a managed window: granted, except where a
    /// shown window is (the desktop places it); its root position then
    /// stays the window's.
    fn configure_request(&mut self, m: &[u8; 32]) -> Result {
        let id = le32(m, 8);
        let mask = le16(m, 26);
        let fixed = self.windows.iter().find(|w| w.id == id).filter(|w| w.shown).map(|w| w.at);
        let mut values: [u32; 7] = [0; 7];
        let mut n = 0;
        let mut out_mask = 0u16;
        let fields: [(u16, u32); 7] = [
            (CONFIG_X, fixed.map_or(le16(m, 16) as i16 as i32, |a| a.0) as u32),
            (CONFIG_Y, fixed.map_or(le16(m, 18) as i16 as i32, |a| a.1) as u32),
            (CONFIG_WIDTH, u32::from(le16(m, 20))),
            (CONFIG_HEIGHT, u32::from(le16(m, 22))),
            (CONFIG_BORDER, u32::from(le16(m, 24))),
            (CONFIG_SIBLING, le32(m, 12)),
            (CONFIG_STACK, u32::from(m[1])),
        ];
        for (bit, v) in fields {
            if mask & bit != 0 {
                values[n] = v;
                n += 1;
                out_mask |= bit;
            }
        }
        self.configure(id, out_mask, &values[..n])
    }

    /// Hands a mapped window's surface to the server, once both are known.
    fn try_show<S: Shell>(&mut self, shell: &mut S, id: u32) {
        let Some(w) = self.windows.iter().find(|w| w.id == id) else { return };
        let Some(surface) = w.surface else { return };
        if w.shown || !w.mapped {
            return;
        }
        let info = Info {
            window: id,
            override_redirect: w.override_redirect,
            at: w.at,
            title: &w.title,
            decorated: !w.override_redirect && !w.undecorated,
        };
        if shell.show(surface, &info) {
            shell.note(
                "shown",
                id,
                format_args!(
                    "wl_surface {surface} {}x{} at {},{}{}",
                    w.size.0,
                    w.size.1,
                    w.at.0,
                    w.at.1,
                    if w.override_redirect { " override-redirect" } else { "" }
                ),
            );
            if let Some(w) = self.window(id) {
                w.shown = true;
            }
        }
    }

    /// Asks again for the surfaces the server did not have yet.
    pub fn retry<S: Shell>(&mut self, shell: &mut S) {
        let mut i = 0;
        while i < self.windows.len() {
            let w = &self.windows[i];
            if w.surface.is_some() && w.mapped && !w.shown {
                let id = w.id;
                self.try_show(shell, id);
            }
            i += 1;
        }
    }

    /// Whether a window waits for its surface (the server retries).
    pub fn waiting(&self) -> bool {
        self.windows.iter().any(|w| w.surface.is_some() && w.mapped && !w.shown)
    }

    fn by_surface(&self, surface: u32) -> Option<u32> {
        self.windows.iter().find(|w| w.shown && w.surface == Some(surface)).map(|w| w.id)
    }

    /// The desktop put the window of `surface` at screen (x, y) (its
    /// client area): the X window moves there.
    pub fn placed(&mut self, surface: u32, x: i32, y: i32) -> Result {
        let Some(id) = self.by_surface(surface) else { return Ok(()) };
        let Some(w) = self.window(id) else { return Ok(()) };
        if w.override_redirect || w.at == (x, y) {
            return Ok(());
        }
        w.at = (x, y);
        self.configure(id, CONFIG_X | CONFIG_Y, &[x as u32, y as u32])
    }

    /// The desktop activated the window of `surface`: the X input focus
    /// goes there.
    pub fn activated(&mut self, surface: u32) -> Result {
        let Some(id) = self.by_surface(surface) else { return Ok(()) };
        let Some(w) = self.windows.iter().find(|w| w.id == id) else { return Ok(()) };
        if w.override_redirect || self.focus == id {
            return Ok(());
        }
        let take_focus = w.take_focus;
        self.focus = id;
        // RevertToPointerRoot, CurrentTime.
        self.request(SET_INPUT_FOCUS, 1, 3)?;
        self.put32(&[id, 0])?;
        if take_focus {
            let (protocols, take) = (self.atom(WM_PROTOCOLS), self.atom(WM_TAKE_FOCUS));
            self.client_message(id, protocols, [take, 0, 0, 0, 0])?;
        }
        let (root, active) = (self.root, self.atom(NET_ACTIVE_WINDOW));
        self.set_property32(root, active, ATOM_WINDOW, &[id])
    }

    /// The desktop asked the window of `surface` to close: WM_DELETE_WINDOW,
    /// or the client is killed when the window does not take it (ICCCM
    /// 4.2.8.1). Returns which.
    pub fn close(&mut self, surface: u32) -> Result<Option<&'static str>> {
        let Some(id) = self.by_surface(surface) else { return Ok(None) };
        let Some(w) = self.windows.iter().find(|w| w.id == id) else { return Ok(None) };
        if w.delete {
            let (protocols, delete) = (self.atom(WM_PROTOCOLS), self.atom(WM_DELETE_WINDOW));
            self.client_message(id, protocols, [delete, 0, 0, 0, 0])?;
            Ok(Some("WM_DELETE_WINDOW"))
        } else {
            self.request(KILL_CLIENT, 0, 2)?;
            self.put32(&[id])?;
            Ok(Some("KillClient"))
        }
    }

    /// One line per window for the state dump.
    pub fn describe(&self, out: &mut dyn FnMut(fmt::Arguments<'_>)) {
        out(format_args!(
            "xwm: root {:#x}, {} windows, focus {:#x}, {} requests, {} messages, {} replies awaited",
            self.root,
            self.windows.len(),
            self.focus,
            self.sent,
            self.events,
            self.waits.len()
        ));
        for w in &self.windows {
            out(format_args!(
                "xwm: window {:#x} {}x{} at {},{}{}{} surface {:?}{} title {:?}",
                w.id,
                w.size.0,
                w.size.1,
                w.at.0,
                w.at.1,
                if w.override_redirect { " override-redirect" } else { "" },
                if w.mapped { " mapped" } else { "" },
                w.surface,
                if w.shown { " shown" } else { "" },
                core::str::from_utf8(&w.title).unwrap_or("(not UTF-8)")
            ));
        }
    }

    /// Whether the window manager owns X window `id` (the dump, tests).
    pub fn knows(&self, id: u32) -> bool {
        self.windows.iter().any(|w| w.id == id)
    }

    /// The root window's cardinal type atom, for tests' replies.
    #[cfg(test)]
    pub fn atom_of(&self, name: &str) -> u32 {
        ATOM_NAMES.iter().position(|n| *n == name).map_or(0, |i| self.atoms[i])
    }
}

/// Keeps ATOM_CARDINAL used (WM_STATE's icon field is a window, 0).
const _: u32 = ATOM_CARDINAL;

#[cfg(test)]
mod tests {
    use super::*;
    use std::format;
    use std::string::String;
    use std::vec;

    /// What the server was asked to show: surface, window, override-redirect,
    /// decorated, place, title.
    type Shown = (u32, u32, bool, bool, (i32, i32), Vec<u8>);

    #[derive(Default)]
    struct Fake {
        surfaces: Vec<u32>,
        shown: Vec<Shown>,
        titles: Vec<(u32, Vec<u8>)>,
        hidden: Vec<u32>,
        notes: Vec<String>,
    }

    impl Shell for Fake {
        fn show(&mut self, surface: u32, info: &Info<'_>) -> bool {
            if !self.surfaces.contains(&surface) {
                return false;
            }
            self.shown.push((
                surface,
                info.window,
                info.override_redirect,
                info.decorated,
                info.at,
                info.title.to_vec(),
            ));
            true
        }
        fn title(&mut self, surface: u32, title: &[u8]) {
            self.titles.push((surface, title.to_vec()));
        }
        fn hide(&mut self, surface: u32) {
            self.hidden.push(surface);
        }
        fn note(&mut self, what: &'static str, window: u32, detail: fmt::Arguments<'_>) {
            self.notes.push(format!("{what} {window:#x} {detail}"));
        }
    }

    /// Requests in the output, as (opcode, body after the 4-byte header).
    fn requests(x: &mut Xwm) -> Vec<(u8, Vec<u8>)> {
        let mut out = Vec::new();
        let b = x.output().to_vec();
        let mut at = 0;
        while at < b.len() {
            let len = 4 * usize::from(le16(&b, at + 2));
            out.push((b[at], b[at + 4..at + len].to_vec()));
            at += len;
        }
        x.written(b.len());
        out
    }

    fn feed(x: &mut Xwm, shell: &mut Fake, bytes: &[u8]) {
        x.input_room()[..bytes.len()].copy_from_slice(bytes);
        x.filled(bytes.len());
        x.process(shell).unwrap();
    }

    fn setup_reply() -> Vec<u8> {
        // 8 + 32 fixed, vendor "gala" (4), one format (8), one screen
        // (40 bytes, no depths).
        let mut b = vec![0u8; 8 + 32 + 4 + 8 + 40];
        b[0] = 1;
        b[2] = 11;
        let extra = ((b.len() - 8) / 4) as u16;
        b[6..8].copy_from_slice(&extra.to_le_bytes());
        b[12..16].copy_from_slice(&0x0040_0000u32.to_le_bytes());
        b[16..20].copy_from_slice(&0x001f_ffffu32.to_le_bytes());
        b[24..26].copy_from_slice(&4u16.to_le_bytes());
        b[29] = 1;
        b[40..44].copy_from_slice(b"gala");
        let screen = 40 + 4 + 8;
        b[screen..screen + 4].copy_from_slice(&0x1e3u32.to_le_bytes());
        b
    }

    fn reply(seq: u16, extra: &[u8], b1: u8, words: &[u32]) -> Vec<u8> {
        let mut r = vec![0u8; 32];
        r[0] = 1;
        r[1] = b1;
        r[2..4].copy_from_slice(&seq.to_le_bytes());
        r[4..8].copy_from_slice(&((extra.len() / 4) as u32).to_le_bytes());
        for (i, w) in words.iter().enumerate() {
            r[8 + 4 * i..12 + 4 * i].copy_from_slice(&w.to_le_bytes());
        }
        r.extend_from_slice(extra);
        r
    }

    fn event(code: u8, fields: &[(usize, u32)]) -> Vec<u8> {
        let mut e = vec![0u8; 32];
        e[0] = code;
        for &(at, v) in fields {
            e[at..at + 4].copy_from_slice(&v.to_le_bytes());
        }
        e
    }

    /// A window manager past its start: atom i is 100 + i.
    fn started() -> (Xwm, Fake) {
        let mut x = Xwm::new().unwrap();
        let mut f = Fake::default();
        assert_eq!(&x.output()[..2], b"l\0");
        x.written(12);
        feed(&mut x, &mut f, &setup_reply());
        let interns = requests(&mut x);
        assert_eq!(interns.len(), ATOM_NAMES.len() + 1);
        assert!(interns[..ATOM_NAMES.len()].iter().all(|(op, _)| *op == INTERN_ATOM));
        assert_eq!(interns[ATOM_NAMES.len()].0, QUERY_EXTENSION);
        for i in 0..ATOM_NAMES.len() {
            feed(&mut x, &mut f, &reply(i as u16 + 1, &[], 0, &[100 + i as u32]));
        }
        // Not started before Composite's answer: present, major 142.
        assert!(!f.notes.iter().any(|n| n.starts_with("ready")));
        let mut r = reply(ATOM_NAMES.len() as u16 + 1, &[], 0, &[]);
        r[8] = 1;
        r[9] = 142;
        feed(&mut x, &mut f, &r);
        assert!(f.notes.iter().any(|n| n.starts_with("ready")));
        let first: Vec<_> = x.output()[..24].to_vec();
        // CompositeQueryVersion, then RedirectSubwindows(root, Manual).
        assert_eq!((first[0], first[1], first[12], first[13]), (142, 0, 142, 2));
        assert_eq!((le32(&first, 16), first[20]), (0x1e3, 1));
        (x, f)
    }

    #[test]
    fn start_redirects_the_root() {
        let (mut x, _) = started();
        let r = requests(&mut x);
        // After Composite's two: ChangeWindowAttributes(root, event-mask
        // with SubstructureRedirect).
        let r = &r[2..];
        assert_eq!(r[0].0, CHANGE_WINDOW_ATTRIBUTES);
        assert_eq!(le32(&r[0].1, 0), 0x1e3);
        assert_eq!(le32(&r[0].1, 4), CW_EVENT_MASK);
        assert!(le32(&r[0].1, 8) & SUBSTRUCTURE_REDIRECT_MASK != 0);
        assert_eq!(r[1].0, CREATE_WINDOW);
        assert!(r.iter().any(|(op, b)| *op == SET_SELECTION_OWNER && le32(b, 4) == 100 + 11));
    }

    #[test]
    fn managed_window_is_shown_with_its_title_and_placed() {
        let (mut x, mut f) = started();
        requests(&mut x);
        let seq0 = x.sent as u16;
        feed(&mut x, &mut f, &event(CREATE_NOTIFY, &[(4, 0x1e3), (8, 0x200001)]));
        feed(&mut x, &mut f, &event(MAP_REQUEST, &[(4, 0x1e3), (8, 0x200001)]));
        let r = requests(&mut x);
        let gets: Vec<_> = r.iter().filter(|(op, _)| *op == GET_PROPERTY).collect();
        assert_eq!(gets.len(), 5);
        assert!(r.iter().any(|(op, b)| *op == MAP_WINDOW && le32(b, 0) == 0x200001));
        // WM_NAME then _NET_WM_NAME: the latter wins.
        let mut name = b"xterm".to_vec();
        name.extend_from_slice(&[0, 0, 0]);
        feed(&mut x, &mut f, &reply(seq0 + 2, &name, 8, &[ATOM_STRING, 0, 5]));
        let mut net = b"xterm!".to_vec();
        net.extend_from_slice(&[0, 0]);
        feed(&mut x, &mut f, &reply(seq0 + 3, &net, 8, &[104, 0, 6]));
        // WM_PROTOCOLS: WM_DELETE_WINDOW.
        feed(&mut x, &mut f, &reply(seq0 + 4, &101u32.to_le_bytes(), 32, &[ATOM_ATOM, 0, 1]));
        feed(&mut x, &mut f, &event(MAP_NOTIFY, &[(4, 0x1e3), (8, 0x200001)]));
        // The surface's message before the server has the surface: asked
        // again later.
        feed(&mut x, &mut f, &event(CLIENT_MESSAGE, &[(4, 0x200001), (8, 106), (12, 7)]));
        assert!(f.shown.is_empty());
        assert!(x.waiting());
        f.surfaces.push(7);
        x.retry(&mut f);
        assert_eq!(f.shown.len(), 1);
        let (s, w, or, dec, _, title) = &f.shown[0];
        assert_eq!((*s, *w, *or, *dec), (7, 0x200001, false, true));
        assert_eq!(title, b"xterm!");
        // Placed by the desktop: the X window moves there.
        x.placed(7, 30, 70).unwrap();
        let r = requests(&mut x);
        assert!(r.iter().any(|(op, b)| *op == CONFIGURE_WINDOW
            && le32(b, 0) == 0x200001
            && le32(b, 8) == 30
            && le32(b, 12) == 70));
        // Activated: SetInputFocus; closed: WM_DELETE_WINDOW.
        x.activated(7).unwrap();
        let r = requests(&mut x);
        assert!(r.iter().any(|(op, b)| *op == SET_INPUT_FOCUS && le32(b, 0) == 0x200001));
        assert_eq!(x.close(7).unwrap(), Some("WM_DELETE_WINDOW"));
        let r = requests(&mut x);
        assert!(r.iter().any(|(op, b)| *op == SEND_EVENT && le32(b, 20) == 101));
        // A title change follows.
        feed(&mut x, &mut f, &event(PROPERTY_NOTIFY, &[(4, 0x200001), (8, 105)]));
        let seq = x.sent as u16;
        let mut t = b"other".to_vec();
        t.extend_from_slice(&[0, 0, 0]);
        feed(&mut x, &mut f, &reply(seq, &t, 8, &[104, 0, 5]));
        assert_eq!(f.titles, vec![(7, b"other".to_vec())]);
        // Unmapped and destroyed: hidden once.
        feed(&mut x, &mut f, &event(UNMAP_NOTIFY, &[(4, 0x1e3), (8, 0x200001)]));
        feed(&mut x, &mut f, &event(DESTROY_NOTIFY, &[(4, 0x1e3), (8, 0x200001)]));
        assert_eq!(f.hidden, vec![7]);
        assert!(!x.knows(0x200001));
    }

    #[test]
    fn override_redirect_window_is_shown_frameless_where_it_is() {
        let (mut x, mut f) = started();
        f.surfaces.push(9);
        let mut c = event(CREATE_NOTIFY, &[(4, 0x1e3), (8, 0x200005)]);
        c[12..14].copy_from_slice(&40i16.to_le_bytes());
        c[14..16].copy_from_slice(&50i16.to_le_bytes());
        c[22] = 1;
        feed(&mut x, &mut f, &c);
        let mut m = event(MAP_NOTIFY, &[(4, 0x1e3), (8, 0x200005)]);
        m[12] = 1;
        feed(&mut x, &mut f, &m);
        feed(&mut x, &mut f, &event(CLIENT_MESSAGE, &[(4, 0x200005), (8, 106), (12, 9)]));
        assert_eq!(f.shown.len(), 1);
        let (_, _, or, dec, at, _) = &f.shown[0];
        assert_eq!((*or, *dec, *at), (true, false, (40, 50)));
        // Without WM_DELETE_WINDOW a close kills the client.
        assert_eq!(x.close(9).unwrap(), Some("KillClient"));
    }

    #[test]
    fn malformed_and_refused() {
        let mut x = Xwm::new().unwrap();
        let mut f = Fake::default();
        let mut b = vec![0u8; 8];
        b[0] = 0;
        x.input_room()[..8].copy_from_slice(&b);
        x.filled(8);
        assert_eq!(x.process(&mut f), Err(XwmError::Refused));
        let (mut x, mut f) = started();
        let mut r = vec![0u8; 32];
        r[0] = 1;
        r[4..8].copy_from_slice(&0x10_0000u32.to_le_bytes());
        x.input_room()[..32].copy_from_slice(&r);
        x.filled(32);
        assert_eq!(x.process(&mut f), Err(XwmError::Malformed));
    }
}
