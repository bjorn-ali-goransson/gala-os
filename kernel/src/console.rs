//! Kernel console: the COM1 serial port (the log of record, read by the
//! test harness) and, once the bootloader's framebuffer is attached, the
//! same text on screen.
//!
//! The screen console wraps around instead of scrolling (scrolling copies
//! the whole framebuffer for every line, which is slow under emulation):
//! the line being written is cleared first, and the next one is marked.

use core::fmt::{self, Write};

use noto_sans_mono_bitmap::{FontWeight, RasterHeight, get_raster, get_raster_width};

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

struct Screen {
    fb: Framebuffer,
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

    fn put_char(&mut self, c: char) {
        match c {
            '\n' => return self.newline(),
            '\r' => return,
            _ => {}
        }
        if self.line_len < self.line_start.len() {
            self.line_start[self.line_len] = c as u8;
            self.line_len += 1;
            let start = &self.line_start[..self.line_len];
            if start.starts_with(b"[warn") {
                self.color = WARNING;
            } else if start.starts_with(b"[error") || start.starts_with(b"KERNEL") {
                self.color = ERROR;
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

pub fn init() {
    serial::COM1.lock().init();
}

/// Shows the console on `fb` from now on, under a header bar with `title`.
pub fn attach_framebuffer(fb: Framebuffer, title: &str) {
    let cell_w = get_raster_width(FONT, SIZE) as u64;
    let cell_h = SIZE.val() as u64;
    let cols = (fb.width().saturating_sub(2 * MARGIN) / cell_w).max(1);
    let rows = (fb.height().saturating_sub(HEADER_HEIGHT + 2 * MARGIN) / cell_h).max(2);
    fb.fill(BACKGROUND);
    let screen = Screen {
        fb,
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
    screen.header(title);
    *SCREEN.lock() = Some(screen);
}

#[doc(hidden)]
pub fn print(args: fmt::Arguments) {
    let _ = serial::COM1.lock().write_fmt(args);
    if let Some(screen) = SCREEN.lock().as_mut() {
        let _ = screen.write_fmt(args);
    }
}

/// Writes raw bytes (no newline translation), e.g. user-mode output.
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
pub fn emergency_println(args: fmt::Arguments) {
    let _ = serial::com1_emergency().write_fmt(format_args!("\n{args}\n"));
    // On screen only if nobody holds it (the panic may come from inside).
    if let Some(mut guard) = SCREEN.try_lock()
        && let Some(screen) = guard.as_mut()
    {
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
