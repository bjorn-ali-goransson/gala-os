//! Kernel console: the COM1 serial port (the log of record, read by the
//! test harness) and, once the bootloader's framebuffer is attached, the
//! same text on screen.
//!
//! By default the screen shows a splash instead of the text: the Masmak
//! fortress over the wordmark قلعة on a dark background, with a progress
//! bar the log's lines advance (the boot option `bootlog` shows the text;
//! a panic switches to it).
//!
//! The screen console wraps around instead of scrolling (scrolling copies
//! the whole framebuffer for every line, which is slow under emulation):
//! the line being written is cleared first, and the next one is marked.

use core::fmt::{self, Write};

use noto_sans_mono_bitmap::{FontWeight, RasterHeight, get_raster, get_raster_width};

#[cfg(not(feature = "hosted"))]
use crate::arch::serial;
use crate::boot::Framebuffer;
use crate::sync::IrqMutex;

const FONT: FontWeight = FontWeight::Regular;
const SIZE: RasterHeight = RasterHeight::Size16;
const BACKGROUND: u32 = 0x00_10_20_40;
const TEXT: u32 = 0x00_d8_de_e9;
const WARNING: u32 = 0x00_f0_c0_50;
const ERROR: u32 = 0x00_ff_60_60;
/// Header bar: Saudi green.
const HEADER: u32 = 0x00_00_6c_35;
const HEADER_TEXT: u32 = 0x00_ff_ff_ff;
const HEADER_HEIGHT: u64 = 24;
const MARGIN: u64 = 8;

/// The splash's pictures (baked by `cargo xtask bake-icons`): the
/// fortress, 160 x 160 coverage bytes; the wordmark, width and height
/// (u16 each) then coverage bytes.
static FORTRESS: &[u8] = include_bytes!("../assets/fortress-160.alpha");
static WORDMARK: &[u8] = include_bytes!("../assets/wordmark-220.alpha");
const SPLASH_TOP: u32 = 0x00_0c_16_2c;
const SPLASH_BOTTOM: u32 = 0x00_03_07_12;
const SPLASH_INK: u32 = 0x00_f2_f5_fa;
const BAR_TRACK: u32 = 0x00_26_32_4c;
const BAR_FILL: u32 = 0x00_4c_c2_ff;
const BAR_W: u64 = 240;
/// About how many log lines a boot writes before the desktop or the
/// program takes over (the bar stops short of full until then).
const BOOT_LINES: u64 = 60;

/// The splash's progress bar.
struct Splash {
    x: u64,
    y: u64,
    lines: u64,
    filled: u64,
}

struct Screen {
    fb: Framebuffer,
    /// Some: the splash shows instead of the text.
    splash: Option<Splash>,
    // gala-um (`hosted`) has no screen: the panic path never reads it.
    #[cfg_attr(feature = "hosted", allow(dead_code))]
    title: &'static str,
    cell_w: u64,
    cell_h: u64,
    cols: u64,
    rows: u64,
    col: u64,
    row: u64,
    /// Colour of the current line (chosen by its first characters).
    color: u32,
    line_start: [u8; 7],
    line_len: usize,
}

impl Screen {
    fn origin(&self, col: u64, row: u64) -> (u64, u64) {
        (MARGIN + col * self.cell_w, HEADER_HEIGHT + MARGIN + row * self.cell_h)
    }

    fn clear_row(&self, row: u64) {
        let (_, y) = self.origin(0, row);
        self.fb.fill_rect(0, y, self.fb.width(), self.cell_h, BACKGROUND);
    }

    /// A thin marker on the row after the cursor, where wrapped-around
    /// output continues, so old and new text can be told apart.
    fn mark_next_row(&self) {
        let next = (self.row + 1) % self.rows;
        self.clear_row(next);
        let (_, y) = self.origin(0, next);
        self.fb.fill_rect(0, y + self.cell_h / 2, self.fb.width(), 1, 0x00_30_50_80);
    }

    fn newline(&mut self) {
        self.col = 0;
        self.row = (self.row + 1) % self.rows;
        self.clear_row(self.row);
        self.mark_next_row();
        self.color = TEXT;
        self.line_len = 0;
    }

    /// The splash: a dark vertical gradient, the fortress and the
    /// wordmark centred, the progress bar's track under them.
    fn draw_splash(&mut self) {
        let (w, h) = (self.fb.width(), self.fb.height());
        for y in 0..h {
            self.fb.fill_rect(
                0,
                y,
                w,
                1,
                mix(SPLASH_TOP, SPLASH_BOTTOM, (y * 255 / h.max(1)) as u32),
            );
        }
        let (ww, wh) = (
            u64::from(u16::from_le_bytes([WORDMARK[0], WORDMARK[1]])),
            u64::from(u16::from_le_bytes([WORDMARK[2], WORDMARK[3]])),
        );
        let total = 160 + 28 + wh + 48 + 4;
        let top = h.saturating_sub(total) / 2;
        self.mask((w.saturating_sub(160)) / 2, top, 160, 160, FORTRESS, h);
        let wy = top + 160 + 28;
        self.mask(w.saturating_sub(ww) / 2, wy, ww, wh, WORDMARK.get(4..).unwrap_or(&[]), h);
        let (bx, by) = (w.saturating_sub(BAR_W) / 2, wy + wh + 48);
        self.fb.fill_rect(bx, by, BAR_W, 4, BAR_TRACK);
        self.splash = Some(Splash { x: bx, y: by, lines: 0, filled: 0 });
    }

    /// Draws coverage `alpha` (w x h) at (x, y) in the splash's ink over
    /// its gradient (of a screen `screen_h` high).
    fn mask(&self, x: u64, y: u64, w: u64, h: u64, alpha: &[u8], screen_h: u64) {
        let mut row = [0u32; 256];
        let w = w.min(256);
        for j in 0..h {
            let bg = mix(SPLASH_TOP, SPLASH_BOTTOM, ((y + j) * 255 / screen_h.max(1)) as u32);
            for (i, px) in row.iter_mut().enumerate().take(w as usize) {
                let a = alpha.get((j * w + i as u64) as usize).copied().unwrap_or(0);
                *px = mix(bg, SPLASH_INK, u32::from(a));
            }
            self.fb.write_row(x, y + j, &row[..w as usize]);
        }
    }

    /// The splash's bar after one more log line.
    fn advance(&mut self) {
        let Some(s) = self.splash.as_mut() else { return };
        s.lines += 1;
        let target = (s.lines * BAR_W / BOOT_LINES).min(BAR_W * 9 / 10);
        if target > s.filled {
            self.fb.fill_rect(s.x + s.filled, s.y, target - s.filled, 4, BAR_FILL);
            s.filled = target;
        }
    }

    /// Leaves the splash for the text console (a panic).
    #[cfg_attr(feature = "hosted", allow(dead_code))]
    fn show_text(&mut self) {
        if self.splash.take().is_some() {
            self.fb.fill(BACKGROUND);
            self.header(self.title);
            self.col = 0;
            self.row = 0;
        }
    }

    fn put_char(&mut self, c: char) {
        if self.splash.is_some() {
            if c == '\n' {
                self.advance();
            }
            return;
        }
        match c {
            '\n' => return self.newline(),
            '\r' => return,
            '\t' => {
                // To the next multiple of 8 columns.
                for _ in 0..8 - self.col % 8 {
                    self.put_char(' ');
                }
                return;
            }
            _ => {}
        }
        if self.line_len < self.line_start.len() && self.col == self.line_len as u64 {
            self.line_start[self.line_len] = c as u8;
            self.line_len += 1;
            let start = &self.line_start[..self.line_len];
            let color = if start.starts_with(b"[warn") {
                WARNING
            } else if start.starts_with(b"[error") || start.starts_with(b"KERNEL") {
                ERROR
            } else {
                self.color
            };
            if color != self.color {
                // Redraw what this line already shows in its colour.
                self.color = color;
                for (i, &b) in self.line_start[..self.line_len - 1].iter().enumerate() {
                    if let Some(glyph) = get_raster(char::from(b), FONT, SIZE) {
                        let (x, y) = self.origin(i as u64, self.row);
                        self.fb.draw_glyph(x, y, glyph.raster(), color, BACKGROUND);
                    }
                }
            }
        }
        if self.col == self.cols {
            self.newline();
        }
        let glyph = get_raster(c, FONT, SIZE).or_else(|| get_raster('?', FONT, SIZE));
        if let Some(glyph) = glyph {
            let (x, y) = self.origin(self.col, self.row);
            self.fb.draw_glyph(x, y, glyph.raster(), self.color, BACKGROUND);
        }
        self.col += 1;
    }

    fn header(&self, text: &str) {
        self.fb.fill_rect(0, 0, self.fb.width(), HEADER_HEIGHT, HEADER);
        for (i, c) in text.chars().enumerate() {
            if let Some(glyph) = get_raster(c, FONT, SIZE) {
                let x = MARGIN + i as u64 * self.cell_w;
                self.fb.draw_glyph(x, 4, glyph.raster(), HEADER_TEXT, HEADER);
            }
        }
    }
}

impl Write for Screen {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        s.chars().for_each(|c| self.put_char(c));
        Ok(())
    }
}

static SCREEN: IrqMutex<Option<Screen>> = IrqMutex::new(None);

/// Takes the framebuffer away from the console (the desktop draws on it
/// from then on; the console keeps the serial line).
#[cfg(not(feature = "hosted"))]
pub fn take_framebuffer() -> Option<Framebuffer> {
    SCREEN.lock().take().map(|s| s.fb)
}

pub fn init() {
    #[cfg(not(feature = "hosted"))]
    serial::COM1.lock().init();
}

/// `a` to `b` by `t` (0-255), per channel.
fn mix(a: u32, b: u32, t: u32) -> u32 {
    let ch = |s: u32| ((((a >> s) & 0xff) * (255 - t) + ((b >> s) & 0xff) * t) / 255) << s;
    ch(16) | ch(8) | ch(0)
}

/// Shows the console on `fb` from now on: the splash, or (`text`) the
/// log under a header bar with `title`.
pub fn attach_framebuffer(fb: Framebuffer, title: &'static str, text: bool) {
    let cell_w = get_raster_width(FONT, SIZE) as u64;
    let cell_h = SIZE.val() as u64;
    let cols = (fb.width().saturating_sub(2 * MARGIN) / cell_w).max(1);
    let rows = (fb.height().saturating_sub(HEADER_HEIGHT + 2 * MARGIN) / cell_h).max(2);
    let mut screen = Screen {
        fb,
        splash: None,
        title,
        cell_w,
        cell_h,
        cols,
        rows,
        col: 0,
        row: 0,
        color: TEXT,
        line_start: [0; 7],
        line_len: 0,
    };
    if text {
        screen.fb.fill(BACKGROUND);
        screen.header(title);
    } else {
        screen.draw_splash();
    }
    *SCREEN.lock() = Some(screen);
}

#[doc(hidden)]
#[cfg(feature = "hosted")]
pub fn print(args: fmt::Arguments) {
    // gala-um: the log goes to stderr (std allocates infallibly: no
    // injected failures here).
    let _host = crate::hosted::HostCode::enter();
    std::eprint!("{args}");
}

/// gala-um: user-mode output goes to stdout.
#[cfg(feature = "hosted")]
pub fn write_bytes(_pid: u64, bytes: &[u8]) {
    use std::io::Write;
    let _host = crate::hosted::HostCode::enter();
    let mut out = std::io::stdout().lock();
    let _ = out.write_all(bytes);
    let _ = out.flush();
}

#[cfg(feature = "hosted")]
pub fn emergency_println(args: fmt::Arguments) {
    let _host = crate::hosted::HostCode::enter();
    std::eprintln!("\n{args}");
}

#[cfg(feature = "hosted")]
pub fn emergency_line(args: fmt::Arguments) {
    let _host = crate::hosted::HostCode::enter();
    std::eprintln!("{args}");
}

/// The serial line discipline: programs and the kernel log share the
/// serial port, and programs write a line in pieces (msvcrt is
/// unbuffered; `puts` is two writes), several programs at once. Each
/// program's unfinished line waits here, per process, and goes out whole
/// when the program ends it, so nothing lands inside it: not a kernel log
/// line, not another program's line (a chrome.exe's stderr inside
/// uia_chromium_test's line made the scenario miss it). Whole lines go
/// out at once.
///
/// A program cannot hold its line back for long: after [`HOLD_NS`] the
/// timer tick shows it as it is (a prompt waiting for input never ends
/// its line), and so does a line too long to hold, or the need for its
/// slot when [`OPEN_LINES`] programs have one. A shown line the program
/// continues goes on on the port; anyone else's output first ends it
/// with a newline. The program's exit ends its line ([`Lines::end`]).
pub struct Lines {
    open: [Partial; OPEN_LINES],
    /// The process whose line is on the port, unfinished.
    shown: Option<u64>,
}

/// Programs that can have an unfinished line held at once.
pub const OPEN_LINES: usize = 8;
/// The longest unfinished line held.
pub const LINE_BYTES: usize = 1024;
/// How long a program's unfinished line is held.
pub const HOLD_NS: u64 = 100_000_000;

/// One program's unfinished line (`len` 0: a free slot).
struct Partial {
    pid: u64,
    /// When its first byte was held (monotonic ns).
    since: u64,
    len: usize,
    bytes: [u8; LINE_BYTES],
}

/// Where the lines go: the serial port (a test's buffer on the host).
pub trait Sink {
    fn put(&mut self, bytes: &[u8]);
}

impl Default for Lines {
    fn default() -> Self {
        Self::new()
    }
}

impl Lines {
    pub const fn new() -> Self {
        const FREE: Partial = Partial { pid: 0, since: 0, len: 0, bytes: [0; LINE_BYTES] };
        Self { open: [FREE; OPEN_LINES], shown: None }
    }

    fn slot_of(&self, pid: u64) -> Option<usize> {
        self.open.iter().position(|p| p.len != 0 && p.pid == pid)
    }

    /// Ends the unfinished line on the port, if any: someone else writes.
    fn end_shown(&mut self, out: &mut impl Sink) {
        if self.shown.take().is_some() {
            out.put(b"\r\n");
        }
    }

    /// Shows slot `i`'s held line on the port, unfinished.
    fn show(&mut self, i: usize, out: &mut impl Sink) {
        self.end_shown(out);
        let p = &mut self.open[i];
        out.put(&p.bytes[..p.len]);
        self.shown = Some(p.pid);
        p.len = 0;
    }

    /// Before a kernel log line: an unfinished line on the port ends.
    pub fn before_kernel(&mut self, out: &mut impl Sink) {
        self.end_shown(out);
    }

    /// Process `pid` writes `bytes` at `now` (0: no clock yet, nothing
    /// is held).
    pub fn write(&mut self, pid: u64, bytes: &[u8], now: u64, out: &mut impl Sink) {
        let Some(&last) = bytes.last() else { return };
        if self.shown == Some(pid) {
            // Its line is on the port already: it goes on there.
            out.put(bytes);
            if last == b'\n' {
                self.shown = None;
            }
            return;
        }
        self.end_shown(out);
        let (whole, rest) = match bytes.iter().rposition(|&b| b == b'\n') {
            Some(i) => bytes.split_at(i + 1),
            None => (&[][..], bytes),
        };
        if !whole.is_empty() {
            if let Some(i) = self.slot_of(pid) {
                let p = &mut self.open[i];
                out.put(&p.bytes[..p.len]);
                p.len = 0;
            }
            out.put(whole);
        }
        if rest.is_empty() {
            return;
        }
        let i = match self.slot_of(pid) {
            Some(i) => i,
            None if now == 0 => {
                out.put(rest);
                self.shown = Some(pid);
                return;
            }
            None => {
                let i = match self.open.iter().position(|p| p.len == 0) {
                    Some(i) => i,
                    None => {
                        // Every slot taken: the oldest line is shown.
                        let oldest =
                            (0..OPEN_LINES).min_by_key(|&i| self.open[i].since).unwrap_or(0);
                        self.show(oldest, out);
                        oldest
                    }
                };
                self.open[i].pid = pid;
                self.open[i].since = now;
                i
            }
        };
        let p = &mut self.open[i];
        if p.len + rest.len() > LINE_BYTES {
            // Too long to hold: shown as it is.
            self.end_shown(out);
            let p = &mut self.open[i];
            out.put(&p.bytes[..p.len]);
            out.put(rest);
            p.len = 0;
            self.shown = Some(pid);
            return;
        }
        p.bytes[p.len..p.len + rest.len()].copy_from_slice(rest);
        p.len += rest.len();
    }

    /// The tick at `now`: lines held past [`HOLD_NS`] are shown.
    pub fn release_overdue(&mut self, now: u64, out: &mut impl Sink) {
        for i in 0..OPEN_LINES {
            let p = &self.open[i];
            if p.len != 0 && now.saturating_sub(p.since) > HOLD_NS {
                self.show(i, out);
            }
        }
    }

    /// Whether a held line is past [`HOLD_NS`] at `now`.
    pub fn overdue(&self, now: u64) -> bool {
        self.open.iter().any(|p| p.len != 0 && now.saturating_sub(p.since) > HOLD_NS)
    }

    /// Process `pid` is gone: the line it left unfinished ends (a
    /// newline). Other programs' lines stay.
    pub fn end(&mut self, pid: u64, out: &mut impl Sink) {
        if let Some(i) = self.slot_of(pid) {
            self.show(i, out);
        }
        if self.shown == Some(pid) {
            self.end_shown(out);
        }
    }
}

#[cfg(not(feature = "hosted"))]
impl Sink for serial::SerialPort {
    fn put(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.write_byte(b);
        }
    }
}

/// The programs' lines. Taken only under the COM1 lock.
#[cfg(not(feature = "hosted"))]
static LINES: IrqMutex<Lines> = IrqMutex::new(Lines::new());

/// Whether a held line is overdue; read by the tick without the lock.
#[cfg(not(feature = "hosted"))]
static OLDEST_HELD: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(u64::MAX);

/// Records when the oldest held line was held (u64::MAX: none), for the
/// tick's check without locks.
#[cfg(not(feature = "hosted"))]
fn note_oldest(lines: &Lines) {
    let oldest = lines.open.iter().filter(|p| p.len != 0).map(|p| p.since).min();
    OLDEST_HELD.store(oldest.unwrap_or(u64::MAX), core::sync::atomic::Ordering::Relaxed);
}

/// From the timer tick: shows the lines held past [`HOLD_NS`]. Skips the
/// tick if the port is busy.
#[cfg(not(feature = "hosted"))]
pub fn release_overdue() {
    let oldest = OLDEST_HELD.load(core::sync::atomic::Ordering::Relaxed);
    let now = crate::time::raw_monotonic_ns();
    if oldest == u64::MAX || now.saturating_sub(oldest) <= HOLD_NS {
        return;
    }
    if let Some(mut port) = serial::COM1.try_lock()
        && let Some(mut lines) = LINES.try_lock()
    {
        lines.release_overdue(now, &mut *port);
        note_oldest(&lines);
    }
}

/// Process `pid` is gone: the line it left unfinished ends (a newline).
/// Another program's line stays.
#[cfg(not(feature = "hosted"))]
pub fn end_program_line(pid: u64) {
    let mut port = serial::COM1.lock();
    let mut lines = LINES.lock();
    lines.end(pid, &mut *port);
    note_oldest(&lines);
}

#[cfg(feature = "hosted")]
pub fn end_program_line(_pid: u64) {}

#[cfg(feature = "hosted")]
pub fn release_overdue() {}

#[doc(hidden)]
#[cfg(not(feature = "hosted"))]
pub fn print(args: fmt::Arguments) {
    {
        let mut port = serial::COM1.lock();
        LINES.lock().before_kernel(&mut *port);
        let _ = port.write_fmt(args);
    }
    if let Some(screen) = SCREEN.lock().as_mut() {
        let _ = screen.write_fmt(args);
    }
}

/// Writes raw bytes (no newline translation): process `pid`'s output,
/// under the line discipline ([`Lines`]).
#[cfg(not(feature = "hosted"))]
pub fn write_bytes(pid: u64, bytes: &[u8]) {
    {
        let mut port = serial::COM1.lock();
        let mut lines = LINES.lock();
        lines.write(pid, bytes, crate::time::raw_monotonic_ns(), &mut *port);
        note_oldest(&lines);
    }
    if let Some(screen) = SCREEN.lock().as_mut() {
        for &b in bytes {
            screen.put_char(if b.is_ascii() { char::from(b) } else { '?' });
        }
    }
}

/// Prints even if the console lock is held, e.g. when a panic happened while
/// printing.
#[cfg(not(feature = "hosted"))]
pub fn emergency_println(args: fmt::Arguments) {
    emergency_write(format_args!("\n{args}\n"));
    emergency_screen(args);
}

/// `emergency_println` without the blank line before (the next lines of
/// a dump that began with one).
#[cfg(not(feature = "hosted"))]
pub fn emergency_line(args: fmt::Arguments) {
    emergency_write(format_args!("{args}\n"));
    emergency_screen(args);
}

/// Writes to the serial port under its lock when that comes free within
/// 10 ms, else without it. A watchdog report or a dump written without
/// the lock landed inside other CPUs' lines (a program's output line was
/// cut in two and a test missed it: gala_cli 2026-11-03); the lock may
/// be held by the very code this CPU interrupted, hence the limit.
#[cfg(not(feature = "hosted"))]
fn emergency_write(args: fmt::Arguments) {
    let start = crate::time::raw_monotonic_ns();
    loop {
        if let Some(mut port) = serial::COM1.try_lock_untracked() {
            // A program's unfinished line on the port ends first.
            if let Some(mut lines) = LINES.try_lock_untracked() {
                lines.before_kernel(&mut *port);
            }
            let _ = port.write_fmt(args);
            return;
        }
        let now = crate::time::raw_monotonic_ns();
        // No clock yet (0): do not wait.
        if now == 0 || now.saturating_sub(start) > 10_000_000 {
            let _ = serial::com1_emergency().write_fmt(args);
            return;
        }
        core::hint::spin_loop();
    }
}

#[cfg(not(feature = "hosted"))]
fn emergency_screen(args: fmt::Arguments) {
    // On screen only if nobody holds it (the panic may come from inside).
    if let Some(mut guard) = SCREEN.try_lock()
        && let Some(screen) = guard.as_mut()
    {
        screen.show_text();
        screen.newline();
        screen.color = ERROR;
        let _ = screen.write_fmt(format_args!("{args}"));
        screen.newline();
    }
}

#[macro_export]
macro_rules! kprint {
    ($($arg:tt)*) => { $crate::console::print(format_args!($($arg)*)) };
}

#[macro_export]
macro_rules! kprintln {
    () => { $crate::kprint!("\n") };
    ($($arg:tt)*) => { $crate::console::print(format_args!("{}\n", format_args!($($arg)*))) };
}

#[macro_export]
macro_rules! info {
    ($($arg:tt)*) => { $crate::kprintln!("[info ] {}", format_args!($($arg)*)) };
}

#[macro_export]
macro_rules! warn {
    ($($arg:tt)*) => { $crate::kprintln!("[warn ] {}", format_args!($($arg)*)) };
}

#[macro_export]
macro_rules! error {
    ($($arg:tt)*) => { $crate::kprintln!("[error] {}", format_args!($($arg)*)) };
}

#[cfg(test)]
mod line_tests {
    use super::{HOLD_NS, LINE_BYTES, Lines, OPEN_LINES, Sink};

    struct Out(Vec<u8>);
    impl Sink for Out {
        fn put(&mut self, bytes: &[u8]) {
            self.0.extend_from_slice(bytes);
        }
    }
    fn text(o: &Out) -> &str {
        core::str::from_utf8(&o.0).unwrap()
    }

    #[test]
    fn another_programs_line_never_lands_inside_an_unfinished_one() {
        // uia_chromium_test (pid 4) has half a line out when a chrome.exe
        // (pid 12) writes a whole one, and a kernel line comes between.
        let (mut l, mut o) = (Lines::new(), Out(Vec::new()));
        l.write(4, b"test: link f", 1, &mut o);
        l.write(12, b"chrome: error\r\n", 2, &mut o);
        l.before_kernel(&mut o);
        o.put(b"[info ] kernel\r\n");
        l.write(4, b"ollowed\r\n", 3, &mut o);
        assert_eq!(text(&o), "chrome: error\r\n[info ] kernel\r\ntest: link followed\r\n");
    }

    #[test]
    fn a_line_held_too_long_is_shown_and_broken_only_by_someone_elses_output() {
        let (mut l, mut o) = (Lines::new(), Out(Vec::new()));
        l.write(4, b"Password: ", 1, &mut o);
        assert!(!l.overdue(1 + HOLD_NS));
        l.release_overdue(2 + HOLD_NS, &mut o);
        assert_eq!(text(&o), "Password: ");
        // The program goes on on the port; then the kernel breaks the line.
        l.write(4, b"x", 3 + HOLD_NS, &mut o);
        l.before_kernel(&mut o);
        l.write(4, b"y\r\n", 4 + HOLD_NS, &mut o);
        assert_eq!(text(&o), "Password: x\r\ny\r\n");
    }

    #[test]
    fn an_exit_ends_only_the_exiting_programs_line() {
        let (mut l, mut o) = (Lines::new(), Out(Vec::new()));
        l.write(4, b"parent's ", 1, &mut o);
        l.write(8, b"child's half", 2, &mut o);
        l.end(8, &mut o);
        l.write(4, b"line\r\n", 3, &mut o);
        l.end(4, &mut o);
        assert_eq!(text(&o), "child's half\r\nparent's line\r\n");
    }

    #[test]
    fn without_a_clock_or_a_free_slot_or_room_a_line_is_shown() {
        let (mut l, mut o) = (Lines::new(), Out(Vec::new()));
        l.write(4, b"early", 0, &mut o);
        assert_eq!(text(&o), "early");
        l.write(4, b"\r\n", 0, &mut o);
        for pid in 0..OPEN_LINES as u64 {
            l.write(100 + pid, b"a", 10 + pid, &mut o);
        }
        l.write(200, b"b", 50, &mut o);
        // The oldest (pid 100) is shown to make room.
        assert_eq!(text(&o), "early\r\na");
        let long = [b'z'; LINE_BYTES + 1];
        l.write(300, &long, 60, &mut o);
        assert!(o.0.ends_with(&long));
    }
}
