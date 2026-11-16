//! Mouse input for the desktop: a kernel thread polls the PS/2 controller
//! (`arch::ps2`) every few milliseconds, assembles the mouse's 3-byte
//! packets and hands the movement and buttons to `nt::desktop`, and turns
//! the keyboard's scan codes (set 1: the controller translates) into
//! Windows virtual keys for it, all to the desktop the console shows
//! (a console switch moves them). Started when the desktop takes the
//! screen, or when the console session's desktop goes live without one
//! (`presenter=`: a headless desktop still takes the local input).
//!
//! The keyboard and the mouse are found separately (`ps2::Devices`) and
//! each is reported, because a machine may have one, the other, both or
//! neither: this thread polls for as long as either answered. Nothing
//! else rides on it -- the desktop's clock is its own thread
//! (`nt::desktop::start_clock`), so a machine with no PS/2 mouse still
//! has a taskbar whose minute changes.

mod virtio;

use crate::arch::ps2;
use crate::mm::heap::TryVec;
use crate::sched;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};

static STARTED: AtomicBool = AtomicBool::new(false);

/// Starts the input thread (once; later calls do nothing).
pub fn start() {
    if STARTED.swap(true, Ordering::AcqRel) {
        return;
    }
    if sched::spawn("input", input_thread, 0).is_err() {
        crate::warn!("input: cannot start the input thread");
    }
}

/// Mouse packets and wheel notches (signed sum) the kernel received, keys
/// pressed: the `input:` line at exit says whether input a test sent
/// (QEMU's monitor) reached the kernel, when a screen check finds the
/// screen behind (a wheel notch missing: console_window's step 5).
static PACKETS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
static WHEEL_EVENTS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
static WHEEL_NOTCHES: core::sync::atomic::AtomicI64 = core::sync::atomic::AtomicI64::new(0);
static KEYS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
/// Mouse button presses (a packet with a button down that the packet
/// before had up).
static PRESSES: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// The bytes the input thread has read from the controller: the
/// keyboard's, then the mouse's. Its address is on the serial log
/// (`input: bytes read at`): xtask's screen checks read it with QEMU's
/// monitor and send their next input only once the guest has read all
/// they sent, because QEMU's PS/2 queues hold 16 bytes and what does not
/// fit is lost (a key dropped, a press merged with its release:
/// WORKFLOW "Screen checks").
#[repr(C)]
struct BytesRead {
    keyboard: core::sync::atomic::AtomicU64,
    mouse: core::sync::atomic::AtomicU64,
}

static BYTES_READ: BytesRead = BytesRead {
    keyboard: core::sync::atomic::AtomicU64::new(0),
    mouse: core::sync::atomic::AtomicU64::new(0),
};

/// The events the input thread has read from virtio-input devices: the
/// keyboards', then the mice's (every event, SYN_REPORT included). Its
/// address is on the serial log (`input: virtio events read at`): xtask
/// paces a screen check's input on it as on `BYTES_READ`.
static EVENTS_READ: BytesRead = BytesRead {
    keyboard: core::sync::atomic::AtomicU64::new(0),
    mouse: core::sync::atomic::AtomicU64::new(0),
};

/// The `input:` line (at the end of a run).
pub fn log_stats() {
    use core::sync::atomic::Ordering::Relaxed;
    let packets = PACKETS.load(Relaxed);
    let keys = KEYS.load(Relaxed);
    let presses = PRESSES.load(Relaxed);
    if packets == 0 && keys == 0 {
        return;
    }
    crate::info!(
        "input: {packets} mouse packets ({presses} button presses), {} with the wheel ({} notches away from the user), {keys} key presses",
        WHEEL_EVENTS.load(Relaxed),
        WHEEL_NOTCHES.load(Relaxed)
    );
}

/// The PS/2 controller, set up, with its mouse packets' size; None when
/// the machine has none or nothing on it answered (each case logged).
fn ps2_setup() -> Option<(ps2::I8042, usize)> {
    let Some(mut controller) = ps2::claim() else {
        crate::info!("input: no PS/2 controller on this machine");
        return None;
    };
    // Which devices are on the controller is the machine's own business,
    // and a machine with none at all is a machine whose input this kernel
    // cannot read: each case says so, because the alternative is a desktop
    // that silently ignores the keyboard.
    let Some(found) = controller.enable() else {
        crate::warn!(
            "input: the PS/2 controller (ports 0x60 and 0x64) never answered: no local keyboard or mouse (a machine whose keyboard and mouse are USB alone has none until USB HID is driven)"
        );
        return None;
    };
    if found.keyboard {
        crate::info!("input: PS/2 keyboard scanning (set 1, the controller translating)");
    } else {
        crate::warn!(
            "input: no PS/2 keyboard: nothing on the controller's first port acknowledged 0xf4 (enable scanning), so keys will not reach the desktop (a USB keyboard is not driven yet)"
        );
    }
    if found.mouse {
        crate::info!("input: PS/2 mouse reporting (wheel: {})", found.wheel);
    } else {
        crate::warn!(
            "input: no PS/2 mouse: nothing on the controller's auxiliary port acknowledged 0xf6/0xf4 (defaults, enable reporting), so the pointer will not move (a USB mouse is not driven yet)"
        );
    }
    if !found.keyboard && !found.mouse {
        crate::warn!("input: no PS/2 device answered");
        return None;
    }
    crate::info!(
        "input: bytes read at {:#x} (keyboard, mouse: two u64), mouse packets of {} bytes",
        core::ptr::addr_of!(BYTES_READ) as usize,
        if found.wheel { 4 } else { 3 }
    );
    Some((controller, if found.wheel { 4 } else { 3 }))
}

/// The virtio-input devices this thread drives (keyboards and mice), each
/// logged; the others are named as not driven.
fn virtio_setup() -> Vec<virtio::VirtioInput> {
    let mut driven = Vec::new();
    for d in virtio::VirtioInput::probe_all() {
        match d.kind {
            virtio::Kind::Keyboard | virtio::Kind::Mouse => {
                crate::info!("input: virtio-input {:?} '{}' at {}", d.kind, d.name(), d.at);
                if driven.try_push(d).is_err() {
                    crate::warn!("input: no memory to drive a virtio-input device");
                }
            }
            kind => crate::warn!(
                "input: virtio-input '{}' at {} is a {kind:?}: not driven (keyboards and relative mice are)",
                d.name(),
                d.at
            ),
        }
    }
    if !driven.is_empty() {
        crate::info!(
            "input: virtio events read at {:#x} (keyboard, mouse: two u64)",
            core::ptr::addr_of!(EVENTS_READ) as usize
        );
    }
    driven
}

fn input_thread(_: usize) {
    let mut ps2 = ps2_setup();
    let mut devices = virtio_setup();
    if ps2.is_none() && devices.is_empty() {
        crate::warn!(
            "input: no PS/2 device and no virtio-input keyboard or mouse: nothing to read, the input thread stops"
        );
        return;
    }
    let mut packet = Packet { size: ps2.as_ref().map_or(3, |p| p.1), ..Packet::default() };
    let mut keys = Keys::default();
    let mut mouse = Coalescer::default();
    let mut report = Report::default();
    let mut held = 0u8;
    loop {
        // Moves with the buttons held are summed (one redraw per poll);
        // a press or release is delivered where it happened.
        let mut deliver = |dx, dy, b| deliver(dx, dy, b);
        while let Some((byte, aux)) = ps2.as_mut().and_then(|p| p.0.poll()) {
            let read = if aux { &BYTES_READ.mouse } else { &BYTES_READ.keyboard };
            read.fetch_add(1, core::sync::atomic::Ordering::Release);
            if !aux {
                if let Some((vk, scan, extended, down)) = keys.push(byte) {
                    key(vk, scan, extended, down, &mut mouse, &mut deliver);
                }
                continue;
            }
            let Some((dx, dy, buttons, dz)) = packet.push(byte) else { continue };
            pointer(dx, dy, buttons, dz, &mut held, &mut mouse, &mut deliver);
        }
        for d in &mut devices {
            let keyboard = d.kind == virtio::Kind::Keyboard;
            d.poll(|e| {
                let read = if keyboard { &EVENTS_READ.keyboard } else { &EVENTS_READ.mouse };
                read.fetch_add(1, core::sync::atomic::Ordering::Release);
                if keyboard {
                    if let Some((vk, scan, extended, down)) = virtio_key(e) {
                        key(vk, scan, extended, down, &mut mouse, &mut deliver);
                    }
                } else if let Some((dx, dy, buttons, dz)) = report.push(e) {
                    pointer(dx, dy, buttons, dz, &mut held, &mut mouse, &mut deliver);
                }
            });
        }
        mouse.flush(&mut deliver);
        sched::sleep_ms(5);
    }
}

/// A key event to the desktop (the pointer's moves before it first).
fn key(
    vk: u8,
    scan: u8,
    extended: bool,
    down: bool,
    mouse: &mut Coalescer,
    deliver: &mut impl FnMut(i64, i64, u8),
) {
    if down {
        KEYS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    }
    mouse.flush(deliver);
    if let Some((session, program)) = crate::nt::desktop::key_event(vk, scan, extended, down) {
        crate::nt::action::start_from_menu(session, program);
    }
}

/// A mouse report (a PS/2 packet, a virtio-input report) to the desktop.
fn pointer(
    dx: i64,
    dy: i64,
    buttons: u8,
    dz: i64,
    held: &mut u8,
    mouse: &mut Coalescer,
    deliver: &mut impl FnMut(i64, i64, u8),
) {
    PACKETS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    if buttons & !*held != 0 {
        PRESSES.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    }
    *held = buttons;
    if dz != 0 {
        WHEEL_EVENTS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        WHEEL_NOTCHES.fetch_add(dz, core::sync::atomic::Ordering::Relaxed);
        mouse.flush(deliver);
        crate::nt::desktop::wheel_event(dz);
    }
    mouse.packet(dx, dy, buttons, deliver);
}

/// A virtio-input keyboard's event as (virtual key, set 1 scan code,
/// extended, down): its evdev code through the set 1 code it stands for
/// (an autorepeat, value 2, is another press, as PS/2's typematic make).
fn virtio_key(e: virtio::Event) -> Option<(u8, u8, bool, bool)> {
    if e.kind != virtio::EV_KEY {
        return None;
    }
    let (scan, extended) = crate::wayland::input::scan_code(u32::from(e.code))?;
    let vk = vk_of(scan, extended);
    (vk != 0).then_some((vk, scan, extended, e.value != 0))
}

/// A virtio-input mouse's events gathered into reports: relative moves,
/// the wheel and the buttons, until SYN_REPORT ends one.
#[derive(Default)]
struct Report {
    dx: i64,
    dy: i64,
    dz: i64,
    buttons: u8,
    /// Whether anything changed since the last report.
    pending: bool,
}

impl Report {
    /// An event; a whole report gives (dx, dy screen-down, buttons, wheel
    /// notches away from the user), as `Packet::push`.
    fn push(&mut self, e: virtio::Event) -> Option<(i64, i64, u8, i64)> {
        let value = i64::from(e.value);
        match (e.kind, e.code) {
            (virtio::EV_REL, virtio::REL_X) => self.dx += value,
            (virtio::EV_REL, virtio::REL_Y) => self.dy += value,
            (virtio::EV_REL, virtio::REL_WHEEL) => self.dz += value,
            (virtio::EV_KEY, code) => {
                let bit = match code {
                    virtio::BTN_LEFT => 1,
                    virtio::BTN_RIGHT => 2,
                    virtio::BTN_MIDDLE => 4,
                    _ => return None,
                };
                if e.value != 0 {
                    self.buttons |= bit;
                } else {
                    self.buttons &= !bit;
                }
            }
            (virtio::EV_SYN, virtio::SYN_REPORT) => {
                if !core::mem::take(&mut self.pending) {
                    return None;
                }
                let r = (self.dx, self.dy, self.buttons, self.dz);
                (self.dx, self.dy, self.dz) = (0, 0, 0);
                return Some(r);
            }
            _ => return None,
        }
        self.pending = true;
        None
    }
}

/// Hands mouse input to the desktop; starts what its start menu chose.
fn deliver(dx: i64, dy: i64, buttons: u8) {
    if let Some((session, program)) = crate::nt::desktop::pointer_event(dx, dy, buttons) {
        crate::nt::action::start_from_menu(session, program);
    }
}

/// Mouse packets turned into the desktop's pointer events while the input
/// thread catches up with what queued: moves with the buttons held are
/// summed (one event, one redraw); a packet that changes the buttons is
/// delivered at once with its own move, after the moves before it. A
/// later move is never folded into a press or release: a release taken
/// late would otherwise land where the pointer went next (a drag ending
/// there, a click missing its target).
#[derive(Default)]
struct Coalescer {
    /// The buttons as last delivered.
    held: u8,
    /// Moves not delivered yet, with `held`.
    moves: Option<(i64, i64)>,
}

impl Coalescer {
    fn packet(&mut self, dx: i64, dy: i64, buttons: u8, out: &mut impl FnMut(i64, i64, u8)) {
        if buttons == self.held {
            let (x, y) = self.moves.unwrap_or((0, 0));
            self.moves = Some((x + dx, y + dy));
        } else {
            self.flush(out);
            out(dx, dy, buttons);
            self.held = buttons;
        }
    }

    /// Delivers the summed moves, if any.
    fn flush(&mut self, out: &mut impl FnMut(i64, i64, u8)) {
        if let Some((x, y)) = self.moves.take() {
            out(x, y, self.held);
        }
    }
}

/// A PS/2 mouse packet being assembled: flags (bit 3 always set; bits
/// 0-2 buttons; bits 4, 5 the signs of x and y; bits 6, 7 overflow), then
/// x and y (y up), and with the wheel on a fourth byte: its movement
/// (signed, positive towards the user).
#[derive(Default)]
struct Packet {
    bytes: [u8; 4],
    len: usize,
    /// 3, or 4 with the wheel.
    size: usize,
}

impl Packet {
    /// Adds a byte; a whole packet gives (dx, dy screen-down, buttons,
    /// wheel notches: positive away from the user, as WM_MOUSEWHEEL).
    fn push(&mut self, byte: u8) -> Option<(i64, i64, u8, i64)> {
        if self.len == 0 && byte & 0x08 == 0 {
            return None; // out of step: wait for a flags byte
        }
        self.bytes[self.len] = byte;
        self.len += 1;
        if self.len < self.size.max(3) {
            return None;
        }
        self.len = 0;
        let [flags, x, y, z] = self.bytes;
        // The low four bits, signed (IntelliMouse).
        let dz = if self.size == 4 { -i64::from(((z << 4) as i8) >> 4) } else { 0 };
        if flags & 0xc0 != 0 {
            return None; // overflowed
        }
        let dx = i64::from(x) - if flags & 0x10 != 0 { 256 } else { 0 };
        let dy = i64::from(y) - if flags & 0x20 != 0 { 256 } else { 0 };
        Some((dx, -dy, flags & 7, dz))
    }
}

/// Scan code set 1 to virtual keys (US layout), without and with the E0
/// prefix. 0: none.
const VK: [u8; 0x59] = [
    0, 0x1b, b'1', b'2', b'3', b'4', b'5', b'6', b'7', b'8', b'9', b'0', 0xbd, 0xbb, 0x08,
    0x09, // 0x00
    b'Q', b'W', b'E', b'R', b'T', b'Y', b'U', b'I', b'O', b'P', 0xdb, 0xdd, 0x0d, 0x11, b'A',
    b'S', // 0x10
    b'D', b'F', b'G', b'H', b'J', b'K', b'L', 0xba, 0xde, 0xc0, 0x10, 0xdc, b'Z', b'X', b'C',
    b'V', // 0x20
    b'B', b'N', b'M', 0xbc, 0xbe, 0xbf, 0x10, 0x6a, 0x12, 0x20, 0x14, 0x70, 0x71, 0x72, 0x73,
    0x74, // 0x30
    0x75, 0x76, 0x77, 0x78, 0x79, 0x90, 0x91, 0x67, 0x68, 0x69, 0x6d, 0x64, 0x65, 0x66, 0x6b,
    0x61, // 0x40
    0x62, 0x63, 0x60, 0x6e, 0, 0, 0, 0x7a, 0x7b, // 0x50
];

fn vk_extended(code: u8) -> u8 {
    match code {
        0x1c => 0x0d,        // keypad Enter
        0x1d => 0x11,        // right Ctrl
        0x35 => 0x6f,        // keypad /
        0x38 => 0x12,        // right Alt
        0x47 => 0x24,        // Home
        0x48 => 0x26,        // Up
        0x49 => 0x21,        // Page Up
        0x4b => 0x25,        // Left
        0x4d => 0x27,        // Right
        0x4f => 0x23,        // End
        0x50 => 0x28,        // Down
        0x51 => 0x22,        // Page Down
        0x52 => 0x2d,        // Insert
        0x53 => 0x2e,        // Delete
        0x5b | 0x5c => 0x5b, // Windows keys
        0x5d => 0x5d,        // Menu
        _ => 0,
    }
}

/// Keyboard bytes being decoded (set 1: a byte per key, bit 7 set on
/// release, E0 before the extended keys).
#[derive(Default)]
struct Keys {
    extended: bool,
}

impl Keys {
    /// A byte; a whole key event gives (virtual key, scan code, extended,
    /// down).
    fn push(&mut self, byte: u8) -> Option<(u8, u8, bool, bool)> {
        if byte == 0xe0 {
            self.extended = true;
            return None;
        }
        if byte == 0xe1 || byte == 0xfa || byte == 0xfe {
            return None; // Pause's prefix, acknowledgements
        }
        let extended = core::mem::take(&mut self.extended);
        let (code, down) = (byte & 0x7f, byte & 0x80 == 0);
        let vk = vk_of(code, extended);
        (vk != 0).then_some((vk, code, extended, down))
    }
}

/// The virtual key of set 1 make code `code` (`extended`: E0-prefixed);
/// 0 for none.
fn vk_of(code: u8, extended: bool) -> u8 {
    if extended { vk_extended(code) } else { VK.get(usize::from(code)).copied().unwrap_or(0) }
}

#[cfg(test)]
mod tests {
    use super::virtio::{self, Event};
    use super::{Coalescer, Keys, Packet, Report, virtio_key};

    fn ev(kind: u16, code: u16, value: i32) -> Event {
        Event { kind, code, value }
    }

    #[test]
    fn virtio_mouse_events_become_reports_at_syn() {
        let mut r = Report::default();
        // A move: x, y, then the report (y as the screen goes: down).
        assert_eq!(r.push(ev(virtio::EV_REL, virtio::REL_X, 5)), None);
        assert_eq!(r.push(ev(virtio::EV_REL, virtio::REL_Y, -3)), None);
        assert_eq!(r.push(ev(virtio::EV_SYN, virtio::SYN_REPORT, 0)), Some((5, -3, 0, 0)));
        // A press, then its release: the buttons as each report ends.
        assert_eq!(r.push(ev(virtio::EV_KEY, virtio::BTN_LEFT, 1)), None);
        assert_eq!(r.push(ev(virtio::EV_SYN, virtio::SYN_REPORT, 0)), Some((0, 0, 1, 0)));
        assert_eq!(r.push(ev(virtio::EV_KEY, virtio::BTN_RIGHT, 1)), None);
        assert_eq!(r.push(ev(virtio::EV_KEY, virtio::BTN_LEFT, 0)), None);
        assert_eq!(r.push(ev(virtio::EV_SYN, virtio::SYN_REPORT, 0)), Some((0, 0, 2, 0)));
        // The wheel; an empty report is none.
        assert_eq!(r.push(ev(virtio::EV_REL, virtio::REL_WHEEL, 1)), None);
        assert_eq!(r.push(ev(virtio::EV_SYN, virtio::SYN_REPORT, 0)), Some((0, 0, 2, 1)));
        assert_eq!(r.push(ev(virtio::EV_SYN, virtio::SYN_REPORT, 0)), None);
    }

    #[test]
    fn virtio_keys_become_virtual_keys() {
        // KEY_H (35 = set 1 0x23) down, up; autorepeat is a press.
        assert_eq!(virtio_key(ev(virtio::EV_KEY, 35, 1)), Some((b'H', 0x23, false, true)));
        assert_eq!(virtio_key(ev(virtio::EV_KEY, 35, 0)), Some((b'H', 0x23, false, false)));
        assert_eq!(virtio_key(ev(virtio::EV_KEY, 35, 2)), Some((b'H', 0x23, false, true)));
        // KEY_UP (103): E0 48, VK_UP.
        assert_eq!(virtio_key(ev(virtio::EV_KEY, 103, 1)), Some((0x26, 0x48, true, true)));
        // Not a key event, or a button: none.
        assert_eq!(virtio_key(ev(virtio::EV_SYN, 0, 0)), None);
        assert_eq!(virtio_key(ev(virtio::EV_KEY, virtio::BTN_LEFT, 1)), None);
    }

    #[test]
    fn a_release_is_delivered_where_it_happened() {
        // chromium_window's drag as a late input thread found it queued:
        // a move with the button held, the release, then the next step's
        // moves (QEMU splits a long one).
        let mut m = Coalescer { held: 1, moves: None };
        let mut got = std::vec::Vec::new();
        let mut out = |dx, dy, b| got.push((dx, dy, b));
        m.packet(50, 50, 1, &mut out);
        m.packet(50, 50, 1, &mut out);
        m.packet(0, 0, 0, &mut out);
        m.packet(-255, 255, 0, &mut out);
        m.packet(-45, 126, 0, &mut out);
        m.packet(0, 0, 1, &mut out);
        m.flush(&mut out);
        // The drag's moves summed, the release at their end, the next moves
        // summed, the press where they end.
        assert_eq!(got, [(100, 100, 1), (0, 0, 0), (-300, 381, 0), (0, 0, 1)]);
    }

    #[test]
    fn set_1_scan_codes_become_virtual_keys() {
        let mut k = Keys::default();
        assert_eq!(k.push(0x23), Some((b'H', 0x23, false, true)));
        assert_eq!(k.push(0xa3), Some((b'H', 0x23, false, false)));
        assert_eq!(k.push(0xe0), None);
        assert_eq!(k.push(0x48), Some((0x26, 0x48, true, true))); // Up
        assert_eq!(k.push(0x2a), Some((0x10, 0x2a, false, true))); // Shift
    }

    #[test]
    fn packets_decode_signed_movement_and_buttons() {
        let mut p = Packet::default();
        // Out of step: a byte without bit 3 is skipped.
        assert_eq!(p.push(0x00), None);
        assert_eq!(p.push(0x09), None);
        assert_eq!(p.push(5), None);
        assert_eq!(p.push(3), Some((5, -3, 1, 0)));
        // Negative x (sign bit 4), negative y (bit 5): left and down.
        assert_eq!(p.push(0x38), None);
        assert_eq!(p.push(0xff), None);
        assert_eq!(p.push(0xf0), Some((-1, 16, 0, 0)));
        // With the wheel: a fourth byte, signed in its low four bits
        // (positive towards the user: a negative WM_MOUSEWHEEL).
        let mut p = Packet { size: 4, ..Packet::default() };
        for b in [0x08, 0, 0] {
            assert_eq!(p.push(b), None);
        }
        assert_eq!(p.push(0x01), Some((0, 0, 0, -1)));
        for b in [0x08, 0, 0] {
            assert_eq!(p.push(b), None);
        }
        assert_eq!(p.push(0x0f), Some((0, 0, 0, 1)));
    }
}
