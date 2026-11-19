//! The display configuration (CCD, "Connecting and Configuring
//! Displays"): GetDisplayConfigBufferSizes, QueryDisplayConfig and
//! DisplayConfigGetDeviceInfo, the three calls a program makes to learn
//! what it is drawing on. Word's startup asks GetMonitorInfoW,
//! EnumDisplaySettingsW and then these (ROADMAP OT3b).
//!
//! What is checked is what any Windows answers, measured on build 26200
//! (one internal panel, 1920 x 1080 at 60 Hz) and true of any screen:
//!
//! - A call must name exactly one topology: no QDC_* bit, or two of them,
//!   is ERROR_INVALID_PARAMETER; QDC_ONLY_ACTIVE_PATHS with another bit
//!   beside it (QDC_VIRTUAL_MODE_AWARE) is not. A null count pointer is
//!   ERROR_INVALID_PARAMETER too, and the active configuration is at
//!   least one path and at least two mode records (a source and a
//!   target), no more than all the paths are.
//! - QueryDisplayConfig refuses a count of zero
//!   (ERROR_INVALID_PARAMETER) and too little room
//!   (ERROR_INSUFFICIENT_BUFFER), and wants a topology id only with
//!   QDC_DATABASE_CURRENT.
//! - The configuration it then writes holds together: an active path
//!   whose two ends are one adapter, whose mode indices are in range and
//!   point at a source record and a target record carrying that path's
//!   ids, both ends in use, the target available, and a refresh rate
//!   with a denominator.
//! - It holds together with the rest of the display, too: the source
//!   record's size is EnumDisplaySettingsW's current mode for the GDI
//!   device name DisplayConfigGetDeviceInfo gives for that source, at the
//!   same depth and frequency, and the target's active size is the same
//!   size again while the scaling is identity.
//! - The target's name carries the same output technology as the path and
//!   a device-interface path under GUID_DEVINTERFACE_MONITOR; the
//!   adapter's is a device path as well. A wrong structure size, an
//!   unknown target or an unknown adapter is ERROR_INVALID_PARAMETER, and
//!   no packet at all is ERROR_GEN_FAILURE.
//!
//! Assumed of the machine it runs on: a 32-bit desktop, and a first
//! active path that is not rotated or scaled (the common case, and gala's).
//!
//! Prints `display_test: ok`, or `display_test: check N got X (line L)`
//! and exits with N.

#![no_std]
#![no_main]

use ntrt as _;

const NAME: &str = "display_test";

type Handle = usize;

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, overlapped: usize) -> i32;
    fn ExitProcess(code: u32) -> !;
}

#[link(name = "user32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetDisplayConfigBufferSizes(flags: u32, paths: *mut u32, modes: *mut u32) -> i32;
    fn QueryDisplayConfig(
        flags: u32,
        paths: *mut u32,
        path_array: *mut u8,
        modes: *mut u32,
        mode_array: *mut u8,
        topology: *mut u32,
    ) -> i32;
    fn DisplayConfigGetDeviceInfo(packet: *mut u8) -> i32;
    fn EnumDisplaySettingsW(device: *const u16, mode: u32, devmode: *mut u8) -> i32;
}

const QDC_ALL_PATHS: u32 = 1;
const QDC_ONLY_ACTIVE_PATHS: u32 = 2;
const QDC_VIRTUAL_MODE_AWARE: u32 = 0x10;
const INVALID_PARAMETER: i32 = 87;
const INSUFFICIENT_BUFFER: i32 = 122;
const GEN_FAILURE: i32 = 31;
const ENUM_CURRENT_SETTINGS: u32 = 0xffff_ffff;

/// A path record is 72 bytes, a mode record 64; the room here is for 64
/// paths and 128 modes, which is more than a machine with sixteen screens
/// has active.
const PATH: usize = 72;
const MODE: usize = 64;
const PATHS: usize = 64;
const MODES: usize = 128;

struct Line {
    buf: [u8; 200],
    n: usize,
}

impl Line {
    fn new() -> Self {
        Self { buf: [0; 200], n: 0 }
    }

    fn s(mut self, s: &[u8]) -> Self {
        for &b in s {
            if self.n < self.buf.len() {
                self.buf[self.n] = b;
                self.n += 1;
            }
        }
        self
    }

    fn dec(self, v: u64) -> Self {
        let mut d = [0u8; 20];
        let mut i = d.len();
        let mut v = v;
        loop {
            i -= 1;
            d[i] = b'0' + (v % 10) as u8;
            v /= 10;
            if v == 0 {
                break;
            }
        }
        self.s(&d[i..])
    }

    fn hex(self, v: u64) -> Self {
        let mut d = [0u8; 18];
        d[0] = b'0';
        d[1] = b'x';
        for i in 0..16 {
            d[2 + i] = b"0123456789abcdef"[((v >> (60 - 4 * i)) & 0xf) as usize];
        }
        self.s(&d)
    }

    /// A wide string of the caller's, up to 80 characters, as ASCII.
    fn wide(mut self, w: &[u8]) -> Self {
        let mut i = 0;
        while i < 160 && i + 1 < w.len() {
            let c = u16::from_le_bytes([w[i], w[i + 1]]);
            if c == 0 {
                break;
            }
            let c = if (0x20..0x7f).contains(&c) { c as u8 } else { b'?' };
            if self.n < self.buf.len() {
                self.buf[self.n] = c;
                self.n += 1;
            }
            i += 2;
        }
        self
    }

    fn print(self) {
        let mut written = 0;
        // SAFETY: our buffer; the standard output handle.
        unsafe {
            WriteFile(
                GetStdHandle(-11i32 as u32),
                self.buf.as_ptr(),
                self.n as u32,
                &mut written,
                0,
            )
        };
    }
}

fn info(what: &[u8], v: u64) {
    Line::new().s(NAME.as_bytes()).s(b": info ").s(what).s(b" ").dec(v).s(b"\r\n").print();
}

fn info_text(what: &[u8], w: &[u8]) {
    Line::new().s(NAME.as_bytes()).s(b": info ").s(what).s(b" ").wide(w).s(b"\r\n").print();
}

struct Checks(u32);

impl Checks {
    #[track_caller]
    fn ok(&mut self, cond: bool, got: u64) -> Result<(), u32> {
        self.0 += 1;
        if cond {
            return Ok(());
        }
        Line::new()
            .s(NAME.as_bytes())
            .s(b": check ")
            .dec(u64::from(self.0))
            .s(b" got ")
            .hex(got)
            .s(b" (line ")
            .dec(u64::from(core::panic::Location::caller().line()))
            .s(b")\r\n")
            .print();
        Err(self.0)
    }

    #[track_caller]
    fn eq(&mut self, got: u64, want: u64) -> Result<(), u32> {
        self.ok(got == want, got)
    }

    #[track_caller]
    fn ret(&mut self, got: i32, want: i32) -> Result<(), u32> {
        self.ok(got == want, got as u32 as u64)
    }
}

fn u32at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

fn u64at(b: &[u8], at: usize) -> u64 {
    let (lo, hi) = (u32at(b, at), u32at(b, at + 4));
    u64::from(lo) | (u64::from(hi) << 32)
}

/// Whether a wide string starts with, or ends with, ASCII text.
fn starts_with(w: &[u8], text: &[u8]) -> bool {
    for (i, &c) in text.iter().enumerate() {
        if 2 * i + 1 >= w.len() || u16::from_le_bytes([w[2 * i], w[2 * i + 1]]) != u16::from(c) {
            return false;
        }
    }
    true
}

fn wide_len(w: &[u8]) -> usize {
    let mut i = 0;
    while 2 * i + 1 < w.len() && u16::from_le_bytes([w[2 * i], w[2 * i + 1]]) != 0 {
        i += 1;
    }
    i
}

fn ends_with(w: &[u8], text: &[u8]) -> bool {
    let len = wide_len(w);
    if len < text.len() {
        return false;
    }
    starts_with(&w[2 * (len - text.len())..], text)
}

/// A DISPLAYCONFIG_DEVICE_INFO_HEADER: the type, the structure's size,
/// the adapter and the source's or target's id.
fn header(packet: &mut [u8], kind: u32, size: u32, adapter: u64, id: u32) {
    for b in packet.iter_mut() {
        *b = 0;
    }
    packet[0..4].copy_from_slice(&kind.to_le_bytes());
    packet[4..8].copy_from_slice(&size.to_le_bytes());
    packet[8..16].copy_from_slice(&adapter.to_le_bytes());
    packet[16..20].copy_from_slice(&id.to_le_bytes());
}

fn run() -> Result<(), u32> {
    let c = &mut Checks(0);
    let (mut paths, mut modes) = (0u32, 0u32);

    // One topology, and only one.
    // SAFETY: our two counts.
    let r = unsafe { GetDisplayConfigBufferSizes(0, &mut paths, &mut modes) };
    c.ret(r, INVALID_PARAMETER)?;
    // SAFETY: as above.
    let r = unsafe {
        GetDisplayConfigBufferSizes(QDC_ALL_PATHS | QDC_ONLY_ACTIVE_PATHS, &mut paths, &mut modes)
    };
    c.ret(r, INVALID_PARAMETER)?;
    // SAFETY: a null count pointer, which the call must refuse.
    let r = unsafe {
        GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, core::ptr::null_mut(), &mut modes)
    };
    c.ret(r, INVALID_PARAMETER)?;
    // SAFETY: our two counts.
    let r = unsafe { GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut paths, &mut modes) };
    c.ret(r, 0)?;
    info(b"active paths", u64::from(paths));
    info(b"active modes", u64::from(modes));
    c.ok(paths >= 1, u64::from(paths))?;
    c.ok(modes >= 2, u64::from(modes))?;
    c.ok(paths as usize <= PATHS, u64::from(paths))?;
    c.ok(modes as usize <= MODES, u64::from(modes))?;

    let (mut all_paths, mut all_modes) = (0u32, 0u32);
    // SAFETY: our two counts.
    let r = unsafe { GetDisplayConfigBufferSizes(QDC_ALL_PATHS, &mut all_paths, &mut all_modes) };
    c.ret(r, 0)?;
    c.ok(all_paths >= paths, u64::from(all_paths))?;
    c.ok(all_modes >= modes, u64::from(all_modes))?;
    // An extra bit beside the topology is not another topology.
    let (mut aware, mut aware_modes) = (0u32, 0u32);
    // SAFETY: our two counts.
    let r = unsafe {
        GetDisplayConfigBufferSizes(
            QDC_ONLY_ACTIVE_PATHS | QDC_VIRTUAL_MODE_AWARE,
            &mut aware,
            &mut aware_modes,
        )
    };
    c.ret(r, 0)?;

    let mut path_array = [0u8; PATH * PATHS];
    let mut mode_array = [0u8; MODE * MODES];

    // Counts of zero, too little room, and a topology id that was not
    // asked for.
    let (mut zero, mut zero_modes) = (0u32, 0u32);
    // SAFETY: our arrays and counts.
    let r = unsafe {
        QueryDisplayConfig(
            QDC_ONLY_ACTIVE_PATHS,
            &mut zero,
            path_array.as_mut_ptr(),
            &mut zero_modes,
            mode_array.as_mut_ptr(),
            core::ptr::null_mut(),
        )
    };
    c.ret(r, INVALID_PARAMETER)?;
    let (mut few, mut few_modes) = (paths, modes - 1);
    // SAFETY: as above.
    let r = unsafe {
        QueryDisplayConfig(
            QDC_ONLY_ACTIVE_PATHS,
            &mut few,
            path_array.as_mut_ptr(),
            &mut few_modes,
            mode_array.as_mut_ptr(),
            core::ptr::null_mut(),
        )
    };
    c.ret(r, INSUFFICIENT_BUFFER)?;
    let (mut p, mut m) = (paths, modes);
    let mut topology = 0u32;
    // SAFETY: as above, with a topology id this topology does not have.
    let r = unsafe {
        QueryDisplayConfig(
            QDC_ONLY_ACTIVE_PATHS,
            &mut p,
            path_array.as_mut_ptr(),
            &mut m,
            mode_array.as_mut_ptr(),
            &mut topology,
        )
    };
    c.ret(r, INVALID_PARAMETER)?;

    // The configuration itself.
    let (mut p, mut m) = (paths, modes);
    // SAFETY: our arrays, with room for the counts the sizes call gave.
    let r = unsafe {
        QueryDisplayConfig(
            QDC_ONLY_ACTIVE_PATHS,
            &mut p,
            path_array.as_mut_ptr(),
            &mut m,
            mode_array.as_mut_ptr(),
            core::ptr::null_mut(),
        )
    };
    c.ret(r, 0)?;
    c.ok(p >= 1 && p <= paths, u64::from(p))?;
    c.ok(m >= 2 && m <= modes, u64::from(m))?;

    // The first active path.
    let mut active = usize::MAX;
    for i in 0..p as usize {
        if u32at(&path_array[i * PATH..], 68) & 1 != 0 {
            active = i;
            break;
        }
    }
    c.ok(active != usize::MAX, u64::from(p))?;
    let path = &path_array[active * PATH..active * PATH + PATH];
    let flags = u32at(path, 68);
    info(b"path flags", u64::from(flags));
    // Virtual-mode indices were not asked for, so the indices are plain.
    c.eq(u64::from(flags & 8), 0)?;
    let source_adapter = u64at(path, 0);
    let source_id = u32at(path, 8);
    let source_mode = u32at(path, 12);
    let source_status = u32at(path, 16);
    let target_adapter = u64at(path, 20);
    let target_id = u32at(path, 28);
    let target_mode = u32at(path, 32);
    let technology = u32at(path, 36);
    let rotation = u32at(path, 40);
    let scaling = u32at(path, 44);
    let (hz_num, hz_den) = (u32at(path, 48), u32at(path, 52));
    let scanline = u32at(path, 56);
    let available = u32at(path, 60);
    let target_status = u32at(path, 64);
    info(b"output technology", u64::from(technology));
    info(b"rotation", u64::from(rotation));
    info(b"scaling", u64::from(scaling));
    c.eq(source_adapter, target_adapter)?;
    c.ok(source_mode < m, u64::from(source_mode))?;
    c.ok(target_mode < m, u64::from(target_mode))?;
    c.ok(source_status & 1 != 0, u64::from(source_status))?;
    c.ok(target_status & 1 != 0, u64::from(target_status))?;
    c.eq(u64::from(available), 1)?;
    c.ok((1..=4).contains(&rotation), u64::from(rotation))?;
    c.ok((1..=3).contains(&scanline), u64::from(scanline))?;
    c.ok(hz_den != 0, u64::from(hz_den))?;
    let hz = (u64::from(hz_num) + u64::from(hz_den) / 2) / u64::from(hz_den);
    info(b"refresh Hz", hz);
    c.ok((20..=400).contains(&hz), hz)?;

    // The two mode records the path points at.
    let source = &mode_array[source_mode as usize * MODE..source_mode as usize * MODE + MODE];
    let target = &mode_array[target_mode as usize * MODE..target_mode as usize * MODE + MODE];
    c.eq(u64::from(u32at(source, 0)), 1)?; // DISPLAYCONFIG_MODE_INFO_TYPE_SOURCE
    c.eq(u64::from(u32at(target, 0)), 2)?; // ..._TYPE_TARGET
    c.eq(u64::from(u32at(source, 4)), u64::from(source_id))?;
    c.eq(u64::from(u32at(target, 4)), u64::from(target_id))?;
    c.eq(u64at(source, 8), source_adapter)?;
    c.eq(u64at(target, 8), target_adapter)?;
    let (width, height) = (u32at(source, 16), u32at(source, 20));
    info(b"source width", u64::from(width));
    info(b"source height", u64::from(height));
    c.ok(width >= 640 && height >= 480, u64::from(width))?;
    c.eq(u64::from(u32at(source, 24)), 4)?; // DISPLAYCONFIG_PIXELFORMAT_32BPP
    let (active_cx, active_cy) = (u32at(target, 40), u32at(target, 44));
    let (total_cx, total_cy) = (u32at(target, 48), u32at(target, 52));
    if scaling == 1 {
        c.eq(u64::from(active_cx), u64::from(width))?;
        c.eq(u64::from(active_cy), u64::from(height))?;
    }
    c.ok(total_cx >= active_cx, u64::from(total_cx))?;
    c.ok(total_cy >= active_cy, u64::from(total_cy))?;
    c.ok(u64at(target, 16) > 0, u64at(target, 16))?; // pixelRate

    // The source's GDI device name, and the mode EnumDisplaySettingsW has
    // for it.
    let mut packet = [0u8; 420];
    header(&mut packet, 1, 84, source_adapter, source_id);
    // SAFETY: our packet, whose size field says how much is there.
    let r = unsafe { DisplayConfigGetDeviceInfo(packet.as_mut_ptr()) };
    c.ret(r, 0)?;
    info_text(b"source name", &packet[20..84]);
    c.ok(starts_with(&packet[20..84], br"\\.\DISPLAY"), 0)?;
    // The name is 32 wide characters in the packet; aligned, it is what
    // EnumDisplaySettingsW takes.
    let mut device = [0u16; 32];
    for (i, w) in device.iter_mut().enumerate() {
        *w = u16::from_le_bytes([packet[20 + 2 * i], packet[21 + 2 * i]]);
    }
    let mut devmode = [0u8; 220];
    devmode[68..70].copy_from_slice(&220u16.to_le_bytes());
    // SAFETY: the name from the packet above, and our DEVMODEW, whose
    // dmSize says how much is there.
    let r = unsafe {
        EnumDisplaySettingsW(device.as_ptr(), ENUM_CURRENT_SETTINGS, devmode.as_mut_ptr())
    };
    c.ok(r != 0, 0)?;
    c.eq(u64::from(u32at(&devmode, 172)), u64::from(width))?;
    c.eq(u64::from(u32at(&devmode, 176)), u64::from(height))?;
    c.eq(u64::from(u32at(&devmode, 168)), 32)?;
    c.eq(u64::from(u32at(&devmode, 184)), hz)?;

    // The target's name.
    header(&mut packet, 2, 420, target_adapter, target_id);
    // SAFETY: our packet of 420 bytes, as its size field says.
    let r = unsafe { DisplayConfigGetDeviceInfo(packet.as_mut_ptr()) };
    c.ret(r, 0)?;
    info(b"target name flags", u64::from(u32at(&packet, 20)));
    info_text(b"target friendly name", &packet[36..164]);
    info_text(b"target device path", &packet[164..420]);
    c.eq(u64::from(u32at(&packet, 24)), u64::from(technology))?;
    c.ok(starts_with(&packet[164..420], br"\\?\DISPLAY#"), 0)?;
    c.ok(ends_with(&packet[164..420], b"{e6f07b5f-ee97-4a90-b076-33f57bf4eaa7}"), 0)?;

    // The adapter's name.
    let mut adapter_packet = [0u8; 276];
    header(&mut adapter_packet, 4, 276, source_adapter, source_id);
    // SAFETY: our packet of 276 bytes, as its size field says.
    let r = unsafe { DisplayConfigGetDeviceInfo(adapter_packet.as_mut_ptr()) };
    c.ret(r, 0)?;
    info_text(b"adapter device path", &adapter_packet[20..276]);
    c.ok(starts_with(&adapter_packet[20..276], br"\\?\"), 0)?;

    // What it refuses: a size that is not the structure's, a target and an
    // adapter it does not have, and no packet at all.
    header(&mut packet, 2, 419, target_adapter, target_id);
    // SAFETY: our packet.
    let r = unsafe { DisplayConfigGetDeviceInfo(packet.as_mut_ptr()) };
    c.ret(r, INVALID_PARAMETER)?;
    header(&mut packet, 2, 420, target_adapter, 0x0dea_dbee);
    // SAFETY: our packet.
    let r = unsafe { DisplayConfigGetDeviceInfo(packet.as_mut_ptr()) };
    c.ret(r, INVALID_PARAMETER)?;
    header(&mut packet, 2, 420, 0x7fff_ffff, target_id);
    // SAFETY: our packet.
    let r = unsafe { DisplayConfigGetDeviceInfo(packet.as_mut_ptr()) };
    c.ret(r, INVALID_PARAMETER)?;
    // SAFETY: no packet, which the call must refuse.
    let r = unsafe { DisplayConfigGetDeviceInfo(core::ptr::null_mut()) };
    c.ret(r, GEN_FAILURE)?;
    Ok(())
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    let code = match run() {
        Ok(()) => {
            Line::new().s(NAME.as_bytes()).s(b": ok\r\n").print();
            0
        }
        Err(n) => n,
    };
    // SAFETY: ends the process.
    unsafe { ExitProcess(code) }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xc000_0409) }
}
