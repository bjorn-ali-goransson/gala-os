//! The device capabilities: GetDeviceCaps, what a program asks a
//! display DC before it lays text out. Word asks for them on its way
//! to a window, and an index gala had no answer for answered 0 -- for
//! TEXTCAPS that reads "this device can draw no text at all", and for
//! ASPECTX "a pixel is zero wide", neither of which a caller can tell
//! from a measurement (ROADMAP OF).
//!
//! What is checked is what any Windows answers, measured on build
//! 26300 at 96 dpi on a 1920 x 1080 panel:
//!
//! - The caps that describe GDI's drawing model are the same on any
//!   Windows display: every curve (511), every line but a bare pixel
//!   (254), every polygon (255), text that can be underlined, struck
//!   out, rotated at right angles and at any angle (0x7807), clipping
//!   to a rectangle (1), the raster operations a display offers
//!   (32409), a square pixel (36, 36, 51), an unlimited supply of
//!   brushes and pens (-1), 20 reserved palette entries, a driver
//!   version of 0x4000 and ICM in the device (2). A display's DC
//!   blends nothing itself (SHADEBLENDCAPS 0) and aligns a blit to a
//!   byte (BLTALIGNMENT 1), and the caps a printer answers (the
//!   physical page, the scaling factor, the palette size) are 0.
//! - The panel's own numbers are checked as the relations Windows
//!   holds them in, not as values: the desktop's resolution is the
//!   screen's, the dpi is the same across and down, and the panel has
//!   a size in millimetres and a refresh rate. (Windows reads the
//!   millimetres from the monitor itself where it can; a machine with
//!   no EDID, gala included, derives them from the dpi, so the value
//!   is not the same on two machines.)
//! - A memory DC created from the screen answers exactly what the
//!   screen answers, for every index -- the caps are the device's, not
//!   the bitmap's, so a 1 x 1 monochrome bitmap does not make a 1 x 1
//!   monochrome device.
//! - A handle that is not a DC answers 0 for every index, with no
//!   error.
//! - An index Windows does not document (0x7fff) answers 0, and on
//!   gala leaves `gdi32.dll!GetDeviceCaps(32767)` in the ledger.
//!
//! Assumed of the machine it runs on: one screen at 96 dpi, 32 bits a
//! pixel.
//!
//! Prints the table it read and `caps_test: ok`, or
//! `caps_test: check N got X (line L)` and exits with N.

#![no_std]
#![no_main]

use ntrt as _;

const NAME: &str = "caps_test";

type Handle = usize;

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, overlapped: usize) -> i32;
    fn ExitProcess(code: u32) -> !;
}

#[link(name = "user32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetDC(hwnd: usize) -> Handle;
    fn ReleaseDC(hwnd: usize, hdc: Handle) -> i32;
}

#[link(name = "gdi32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetDeviceCaps(hdc: Handle, index: i32) -> i32;
    fn CreateCompatibleDC(hdc: Handle) -> Handle;
    fn DeleteDC(hdc: Handle) -> i32;
}

struct Line {
    buf: [u8; 240],
    n: usize,
}

impl Line {
    fn new() -> Self {
        Self { buf: [0; 240], n: 0 }
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

    fn dec(self, v: i64) -> Self {
        let mut d = [0u8; 21];
        let mut i = d.len();
        let neg = v < 0;
        let mut left = v.unsigned_abs();
        loop {
            i -= 1;
            d[i] = b'0' + (left % 10) as u8;
            left /= 10;
            if left == 0 {
                break;
            }
        }
        if neg {
            i -= 1;
            d[i] = b'-';
        }
        self.s(&d[i..])
    }

    fn print(self) {
        // SAFETY: our buffer, its length.
        unsafe {
            let h = GetStdHandle(0xffff_fff5); // STD_OUTPUT_HANDLE
            let mut done = 0;
            WriteFile(h, self.buf.as_ptr(), self.n as u32, &mut done, 0);
        }
    }
}

/// Every index Windows documents, in order, and one it does not.
const INDICES: &[i32] = &[
    0, 2, 4, 6, 8, 10, 12, 14, 16, 18, 20, 22, 24, 26, 28, 30, 32, 34, 36, 38, 40, 42, 44, 88, 90,
    104, 106, 108, 110, 111, 112, 113, 114, 115, 116, 117, 118, 119, 120, 121, 0x7fff,
];

fn cap(dc: Handle, index: i32) -> i32 {
    // SAFETY: plain call.
    unsafe { GetDeviceCaps(dc, index) }
}

/// Prints what `dc` answers for every index, ten to a line.
fn survey(tag: &[u8], dc: Handle) {
    let mut line = Line::new().s(NAME.as_bytes()).s(b": ").s(tag).s(b":");
    let mut on = 0;
    for &i in INDICES {
        line = line.s(b" ").dec(i64::from(i)).s(b"=").dec(i64::from(cap(dc, i)));
        on += 1;
        if on == 10 {
            line.s(b"\r\n").print();
            line = Line::new().s(NAME.as_bytes()).s(b": ").s(tag).s(b":");
            on = 0;
        }
    }
    if on != 0 {
        line.s(b"\r\n").print();
    }
}

struct Checks {
    n: u32,
    dc: Handle,
}

impl Checks {
    #[track_caller]
    fn ok(&mut self, cond: bool, got: i64) -> Result<(), u32> {
        self.n += 1;
        if cond {
            return Ok(());
        }
        Line::new()
            .s(NAME.as_bytes())
            .s(b": check ")
            .dec(i64::from(self.n))
            .s(b" got ")
            .dec(got)
            .s(b" (line ")
            .dec(i64::from(core::panic::Location::caller().line()))
            .s(b")\r\n")
            .print();
        Err(self.n)
    }

    /// The cap at `index` is `want`.
    #[track_caller]
    fn is(&mut self, index: i32, want: i32) -> Result<(), u32> {
        let got = cap(self.dc, index);
        self.ok(got == want, i64::from(got))
    }

    /// Every one of `indices` is `want`.
    #[track_caller]
    fn all(&mut self, indices: &[i32], want: i32) -> Result<(), u32> {
        for &i in indices {
            let got = cap(self.dc, i);
            self.ok(got == want, i64::from(got))?;
        }
        Ok(())
    }

    /// The cap at `index` is more than nothing: a number the panel
    /// decides, which is not the same on two machines.
    #[track_caller]
    fn some(&mut self, index: i32) -> Result<(), u32> {
        let got = cap(self.dc, index);
        self.ok(got > 0, i64::from(got))
    }

    /// `other` answers what this DC answers, for every index.
    #[track_caller]
    fn same(&mut self, other: Handle) -> Result<(), u32> {
        for &i in INDICES {
            let want = cap(self.dc, i);
            let got = cap(other, i);
            self.ok(got == want, i64::from(got))?;
        }
        Ok(())
    }
}

fn run(screen: Handle, memory: Handle) -> Result<(), u32> {
    let c = &mut Checks { n: 0, dc: screen };

    // The caps that describe GDI's drawing model, the same on any
    // Windows display.
    c.is(0, 0x4000)?; // DRIVERVERSION
    c.is(2, 1)?; // TECHNOLOGY: DT_RASDISPLAY
    c.is(12, 32)?; // BITSPIXEL
    c.is(14, 1)?; // PLANES
    c.all(&[16, 18], -1)?; // NUMBRUSHES, NUMPENS: no limit
    c.all(&[20, 22], 0)?; // NUMMARKERS, NUMFONTS: not a plotter
    c.is(24, -1)?; // NUMCOLORS: more than a palette
    c.is(26, 0)?; // PDEVICESIZE
    c.is(28, 511)?; // CURVECAPS: every curve
    c.is(30, 254)?; // LINECAPS: every line but a bare pixel
    c.is(32, 255)?; // POLYGONALCAPS: every polygon
    c.is(34, 0x7807)?; // TEXTCAPS
    c.is(36, 1)?; // CLIPCAPS: CP_RECTANGLE
    c.is(38, 32409)?; // RASTERCAPS
    c.all(&[40, 42], 36)?; // ASPECTX, ASPECTY: a square pixel
    c.is(44, 51)?; // ASPECTXY: its diagonal
    c.is(104, 0)?; // SIZEPALETTE: no palette
    c.is(106, 20)?; // NUMRESERVED
    c.is(108, 24)?; // COLORRES
    c.all(&[110, 111, 112, 113, 114, 115], 0)?; // the printer's page
    c.is(119, 1)?; // BLTALIGNMENT: a byte
    c.is(120, 0)?; // SHADEBLENDCAPS: the device blends nothing itself
    c.is(121, 2)?; // COLORMGMTCAPS: CM_DEVICE_ICM

    // The panel's own numbers, as the relations Windows holds them in.
    c.some(8)?; // HORZRES
    c.some(10)?; // VERTRES
    c.is(118, cap(screen, 8))?; // DESKTOPHORZRES is the screen's
    c.is(117, cap(screen, 10))?; // DESKTOPVERTRES
    c.is(88, 96)?; // LOGPIXELSX
    c.is(90, cap(screen, 88))?; // LOGPIXELSY: the same across and down
    c.some(4)?; // HORZSIZE (mm)
    c.some(6)?; // VERTSIZE
    c.some(116)?; // VREFRESH

    // A memory DC created from the screen is the same device.
    c.same(memory)?;

    // A handle that is not a DC answers 0, with no error.
    c.dc = 0;
    c.all(INDICES, 0)?;

    // An index Windows does not document is 0 (and a ledger row here).
    c.dc = screen;
    c.is(0x7fff, 0)?;
    Ok(())
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    // SAFETY: the screen DC, released below.
    let screen = unsafe { GetDC(0) };
    // SAFETY: a memory DC compatible with the screen, deleted below.
    let memory = unsafe { CreateCompatibleDC(screen) };
    survey(b"screen", screen);
    survey(b"memory", memory);
    survey(b"nodc", 0);

    let code = match run(screen, memory) {
        Ok(()) => {
            Line::new().s(NAME.as_bytes()).s(b": ok\r\n").print();
            0
        }
        Err(n) => n,
    };

    // SAFETY: ours, both.
    unsafe {
        DeleteDC(memory);
        ReleaseDC(0, screen);
    }
    // SAFETY: ends the process.
    unsafe { ExitProcess(code) }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xc000_0409) }
}
