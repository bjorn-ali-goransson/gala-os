//! The system metrics: GetSystemMetrics, the sizes and switches a
//! program lays its windows out from. Word's startup asks for them
//! before it draws anything, and an index gala has no answer for
//! answers 0 -- a value no caller can tell from a measurement, which
//! is why one is a ledger row now (ROADMAP OF).
//!
//! What is checked is what any Windows answers, measured on build
//! 26300 at 96 dpi on a 1920 x 1080 panel:
//!
//! - The sizes a theme decides are the same on any Windows 11 at 96
//!   dpi: a scroll bar and its thumb 17, an icon 32 and a small one 16,
//!   a menu bar 20 and its check mark 15, the double-click and drag
//!   slop 4, a window edge 2, a focus border 1, the menu bar's buttons
//!   19, ARW_HIDE (8) for SM_ARRANGE.
//! - The input the machine has: a mouse with three buttons and a
//!   wheel, no horizontal wheel, IME support, a blinking caret, one
//!   display format for every monitor, and a network.
//! - The switches a plain desktop answers no to: SM_DEBUG,
//!   SM_SWAPBUTTON, SM_MENUDROPALIGNMENT, SM_PENWINDOWS,
//!   SM_DBCSENABLED, SM_SECURE, SM_SHOWSOUNDS, SM_SLOWMACHINE,
//!   SM_TABLETPC, SM_MEDIACENTER, SM_STARTER, SM_SERVERR2,
//!   SM_REMOTESESSION, SM_SHUTTINGDOWN, SM_REMOTECONTROL and
//!   SM_CLEANBOOT -- the last of which is what a Click-to-Run Office
//!   refuses to start for.
//! - The sizes the window frame decides are checked as the relations
//!   Windows holds them in, not as values, because gala's desktop
//!   draws a frame of its own (a 32-pixel caption where this Windows
//!   has 23): the greatest size a window may be tracked to is the
//!   screen and two frames and two padded borders and four more, a
//!   maximized window is the work area and those frames and borders,
//!   the smallest window is a caption and those frames and borders
//!   high, and the smallest tracking size is that smallest window.
//! - An index Windows does not document (0x7fff) answers 0, and on
//!   gala leaves `user32.dll!GetSystemMetrics(32767)` in the ledger.
//!
//! Assumed of the machine it runs on: one screen, at the origin, whose
//! taskbar is horizontal, with the stock scroll-bar width and desktop
//! icon spacing.
//!
//! Prints the table it read and `metrics_test: ok`, or
//! `metrics_test: check N got X (line L)` and exits with N.

#![no_std]
#![no_main]

use ntrt as _;

const NAME: &str = "metrics_test";

type Handle = usize;

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, overlapped: usize) -> i32;
    fn ExitProcess(code: u32) -> !;
}

#[link(name = "user32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetSystemMetrics(index: i32) -> i32;
}

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

struct Checks(u32);

impl Checks {
    #[track_caller]
    fn ok(&mut self, cond: bool, got: i64) -> Result<(), u32> {
        self.0 += 1;
        if cond {
            return Ok(());
        }
        Line::new()
            .s(NAME.as_bytes())
            .s(b": check ")
            .dec(i64::from(self.0))
            .s(b" got ")
            .dec(got)
            .s(b" (line ")
            .dec(i64::from(core::panic::Location::caller().line()))
            .s(b")\r\n")
            .print();
        Err(self.0)
    }

    /// The metric at `index` is `want`.
    #[track_caller]
    fn is(&mut self, index: i32, want: i32) -> Result<(), u32> {
        let got = m(index);
        self.ok(got == want, i64::from(got))
    }

    /// Every one of `indices` is `want`.
    #[track_caller]
    fn all(&mut self, indices: &[i32], want: i32) -> Result<(), u32> {
        for &i in indices {
            let got = m(i);
            self.ok(got == want, i64::from(got))?;
        }
        Ok(())
    }

    /// The metric at `index` is more than nothing: a size the frame or
    /// the panel decides, whose value is not the same on two machines.
    #[track_caller]
    fn some(&mut self, index: i32) -> Result<(), u32> {
        let got = m(index);
        self.ok(got > 0, i64::from(got))
    }
}

fn m(index: i32) -> i32 {
    // SAFETY: plain call.
    unsafe { GetSystemMetrics(index) }
}

/// Every index Windows documents, in order, and one it does not.
const INDICES: &[i32] = &[
    0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 28, 29,
    30, 31, 32, 33, 34, 35, 36, 37, 38, 39, 40, 41, 42, 43, 44, 45, 46, 47, 48, 49, 50, 51, 52, 53,
    54, 55, 56, 57, 58, 59, 60, 61, 62, 63, 67, 68, 69, 70, 71, 72, 73, 74, 75, 76, 77, 78, 79, 80,
    81, 82, 83, 84, 86, 87, 88, 89, 90, 91, 92, 93, 94, 0x1000, 0x2000, 0x2001, 0x2002, 0x2003,
    0x2004, 0x7fff,
];

/// The table, ten to a line: what this machine answered, which is how
/// the expectations above were measured.
fn survey() {
    let mut line = Line::new().s(NAME.as_bytes()).s(b":");
    let mut on = 0;
    for &i in INDICES {
        line = line.s(b" ").dec(i64::from(i)).s(b"=").dec(i64::from(m(i)));
        on += 1;
        if on == 10 {
            line.s(b"\r\n").print();
            line = Line::new().s(NAME.as_bytes()).s(b":");
            on = 0;
        }
    }
    if on != 0 {
        line.s(b"\r\n").print();
    }
}

fn run() -> Result<(), u32> {
    let c = &mut Checks(0);

    // The sizes a theme decides, the same on any Windows 11 at 96 dpi.
    c.all(&[2, 3, 9, 10, 20, 21], 17)?; // scroll bars and their thumbs
    c.all(&[11, 12, 13, 14], 32)?; // an icon and a cursor
    c.all(&[49, 50], 16)?; // a small icon
    c.is(15, 20)?; // SM_CYMENU
    c.all(&[71, 72], 15)?; // the check mark
    c.all(&[36, 37], 4)?; // the double-click slop
    c.all(&[68, 69], 4)?; // the drag slop
    c.all(&[45, 46], 2)?; // a window edge
    c.all(&[83, 84], 1)?; // a focus border
    c.all(&[54, 55], 19)?; // the menu bar's buttons
    c.is(56, 8)?; // SM_ARRANGE: ARW_HIDE

    // The input this machine has.
    c.is(19, 1)?; // SM_MOUSEPRESENT
    c.is(43, 3)?; // SM_CMOUSEBUTTONS
    c.is(75, 1)?; // SM_MOUSEWHEELPRESENT
    c.is(91, 0)?; // SM_MOUSEHORIZONTALWHEELPRESENT
    c.is(82, 1)?; // SM_IMMENABLED
    c.is(0x2002, 1)?; // SM_CARETBLINKINGENABLED
    c.is(81, 1)?; // SM_SAMEDISPLAYFORMAT
    c.ok(m(63) != 0, i64::from(m(63)))?; // SM_NETWORK: a network is present

    // The switches a plain desktop answers no to.
    c.all(&[18, 22, 23, 40, 41, 42, 44, 67, 70, 73, 86, 87, 88, 89], 0)?;
    c.all(&[0x1000, 0x2000, 0x2001], 0)?;

    // One screen, at the origin.
    c.some(0)?;
    c.some(1)?;
    c.is(78, m(0))?;
    c.is(79, m(1))?;
    c.all(&[76, 77], 0)?;
    c.ok(m(80) >= 1, i64::from(m(80)))?;
    // The work area is inside the screen, and the caption above it.
    c.ok(m(16) > 0 && m(16) <= m(0), i64::from(m(16)))?;
    c.ok(m(17) > 0 && m(17) < m(1), i64::from(m(17)))?;

    // The sizes the frame decides, as the relations Windows holds them
    // in: gala's caption is its own height, so only these hold on both.
    let (frame, pad) = (m(32), m(92));
    c.ok(frame > 0, i64::from(frame))?; // SM_CXFRAME
    c.is(33, frame)?; // SM_CYFRAME: square
    c.ok(pad >= 0, i64::from(pad))?; // SM_CXPADDEDBORDER
    c.some(4)?; // SM_CYCAPTION
    c.some(5)?; // SM_CXBORDER
    c.is(6, m(5))?;
    c.some(7)?; // SM_CXDLGFRAME
    c.is(8, m(7))?;
    c.some(30)?; // SM_CXSIZE: the caption buttons
    c.some(31)?;
    c.some(51)?; // SM_CYSMCAPTION
    c.some(52)?; // SM_CXSMSIZE
    c.some(53)?;
    c.some(38)?; // SM_CXICONSPACING
    c.some(39)?;
    c.some(47)?; // SM_CXMINSPACING
    c.some(48)?;
    c.some(57)?; // SM_CXMINIMIZED
    c.some(58)?;
    // The greatest a window may be tracked to is the screen, two
    // frames, two padded borders and four more.
    c.is(59, m(0) + 2 * frame + 2 * pad + 4)?;
    c.is(60, m(1) + 2 * frame + 2 * pad + 4)?;
    // A maximized window is the work area and those frames and borders.
    c.is(61, m(0) + 2 * frame + 2 * pad)?;
    c.is(62, m(17) + m(4) + 2 * frame + 2 * pad)?;
    // The smallest window is a caption and those frames and borders
    // high, and the smallest tracking size is that window.
    c.is(29, m(4) + 2 * frame + 2 * pad)?;
    c.some(28)?;
    c.is(34, m(28))?;
    c.is(35, m(29))?;

    // An index Windows does not document is 0 (and a ledger row here).
    c.is(0x7fff, 0)?;
    Ok(())
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    survey();
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
