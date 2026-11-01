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
    // gala-um: the log goes to stderr.
    std::eprint!("{args}");
}

/// gala-um: user-mode output goes to stdout.
#[cfg(feature = "hosted")]
pub fn write_bytes(bytes: &[u8]) {
    use std::io::Write;
    let mut out = std::io::stdout().lock();
    let _ = out.write_all(bytes);
    let _ = out.flush();
}

#[cfg(feature = "hosted")]
pub fn emergency_println(args: fmt::Arguments) {
    std::eprintln!("\n{args}");
}

#[doc(hidden)]
#[cfg(not(feature = "hosted"))]
pub fn print(args: fmt::Arguments) {
    let _ = serial::COM1.lock().write_fmt(args);
    if let Some(screen) = SCREEN.lock().as_mut() {
        let _ = screen.write_fmt(args);
    }
}

/// Writes raw bytes (no newline translation), e.g. user-mode output.
#[cfg(not(feature = "hosted"))]
pub fn write_bytes(bytes: &[u8]) {
    {
        let mut port = serial::COM1.lock();
        for &b in bytes {
            port.write_byte(b);
        }
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
    let _ = serial::com1_emergency().write_fmt(format_args!("\n{args}\n"));
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
