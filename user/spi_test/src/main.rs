//! The system parameters: the settings a program asks the desktop
//! for, through SystemParametersInfoW. Word asks whether animations
//! are on while it decides how to show its window, so an answer here
//! that no Windows would give is a divergence a program acts on.
//!
//! Measured on the Windows machine first (build 26300, 1920 x 1080 at
//! 96 dpi, a desktop with the visual settings it was installed with).
//! The survey prints every action and its answer; the checks pin the
//! ones every Windows answers the same, and only that an answer came
//! for the two that are a machine's own (the foreground lock timeout
//! and flash count). Of a machine's own values the work area is held
//! to the relation Windows holds it in: the screen less what the
//! desktop reserves.

#![no_std]
#![no_main]

use ntrt as _;

const NAME: &str = "spi_test";

type Handle = usize;

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, overlapped: usize) -> i32;
    fn GetLastError() -> u32;
    fn SetLastError(code: u32);
    fn ExitProcess(code: u32) -> !;
}

#[link(name = "user32", kind = "raw-dylib")]
unsafe extern "system" {
    fn SystemParametersInfoW(action: u32, param: u32, out: *mut u8, update: u32) -> i32;
    fn SystemParametersInfoA(action: u32, param: u32, out: *mut u8, update: u32) -> i32;
    fn GetSystemMetrics(index: i32) -> i32;
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

    fn hex(self, v: u32) -> Self {
        let mut d = [0u8; 4];
        for (i, b) in d.iter_mut().enumerate() {
            *b = b"0123456789abcdef"[((v >> (12 - 4 * i)) & 0xf) as usize];
        }
        self.s(&d)
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
/// Every action that answers a BOOL or a UINT, with the value the
/// Windows machine answers. The ones whose value is a setting and not
/// the system's are in SOME below instead.
const FLAGS: &[(u32, &str, i32)] = &[
    (0x0001, "GETBEEP", 1),
    (0x0005, "GETBORDER", 1),
    (0x000a, "GETKEYBOARDSPEED", 31),
    (0x0016, "GETKEYBOARDDELAY", 1),
    (0x0019, "GETICONTITLEWRAP", 1),
    (0x001b, "GETMENUDROPALIGNMENT", 0),
    (0x0026, "GETDRAGFULLWINDOWS", 1),
    (0x0044, "GETKEYBOARDPREF", 0),
    (0x0046, "GETSCREENREADER", 0),
    (0x004a, "GETFONTSMOOTHING", 1),
    (0x005f, "GETSNAPTODEFBUTTON", 0),
    (0x0062, "GETMOUSEHOVERWIDTH", 4),
    (0x0064, "GETMOUSEHOVERHEIGHT", 4),
    (0x0066, "GETMOUSEHOVERTIME", 400),
    (0x0068, "GETWHEELSCROLLLINES", 3),
    (0x006a, "GETMENUSHOWDELAY", 400),
    (0x006c, "GETWHEELSCROLLCHARS", 3),
    (0x1002, "GETMENUANIMATION", 1),
    (0x1004, "GETCOMBOBOXANIMATION", 1),
    (0x1006, "GETLISTBOXSMOOTHSCROLLING", 1),
    (0x1008, "GETGRADIENTCAPTIONS", 1),
    (0x100a, "GETKEYBOARDCUES", 0),
    (0x100c, "GETACTIVEWNDTRKZORDER", 0),
    (0x100e, "GETHOTTRACKING", 1),
    (0x1012, "GETMENUFADE", 1),
    (0x1014, "GETSELECTIONFADE", 1),
    (0x1016, "GETTOOLTIPANIMATION", 1),
    (0x1018, "GETTOOLTIPFADE", 1),
    (0x101a, "GETCURSORSHADOW", 0),
    (0x101c, "GETMOUSESONAR", 0),
    (0x101e, "GETMOUSECLICKLOCK", 0),
    (0x1020, "GETMOUSEVANISH", 1),
    (0x1022, "GETFLATMENU", 1),
    (0x1024, "GETDROPSHADOW", 1),
    (0x1026, "GETBLOCKSENDINPUTRESETS", 0),
    (0x103e, "GETUIEFFECTS", 1),
    (0x1040, "GETDISABLEOVERLAPPEDCONTENT", 0),
    (0x1042, "GETCLIENTAREAANIMATION", 1),
    (0x1044, "0x1044", 0),
    (0x1046, "0x1046", 0),
    (0x2002, "GETACTIVEWNDTRKTIMEOUT", 0),
    (0x2006, "GETCARETWIDTH", 1),
    (0x200a, "GETFONTSMOOTHINGTYPE", 2),
    (0x200c, "GETFONTSMOOTHINGCONTRAST", 1200),
    (0x200e, "GETFOCUSBORDERWIDTH", 1),
    (0x2010, "GETFOCUSBORDERHEIGHT", 1),
    (0x2012, "GETFONTSMOOTHINGORIENTATION", 1),
    (0x2014, "GETMINIMUMHITRADIUS", 0),
    (0x2016, "GETMESSAGEDURATION", 5),
    (0x201c, "GETMOUSEWHEELROUTING", 2),
];

/// The actions whose value is this machine's and not every Windows
/// machine's: answered, any value.
const SOME: &[(u32, &str)] =
    &[(0x2000, "GETFOREGROUNDLOCKTIMEOUT"), (0x2004, "GETFOREGROUNDFLASHCOUNT")];

/// The actions that answer a structure: the size to ask with (0 where
/// the structure has no cbSize of its own), how many words to print in
/// the survey, and the words this machine answers, from the first.
const STRUCTS: &[(u32, &str, u32, usize, &[i32])] = &[
    (0x0003, "GETMOUSE", 0, 12, &[6, 10, 1]),
    (0x001f, "GETICONTITLELOGFONT", 92, 32, &[-12, 0, 0, 0, 400]),
    (0x0029, "GETNONCLIENTMETRICS", 504, 32, &[504]),
    (0x002b, "GETMINIMIZEDMETRICS", 20, 20, &[20, 154, 0, 0, 8]),
    (0x002d, "GETICONMETRICS", 108, 32, &[108, 75, 75, 1, -12]),
    (0x0042, "GETHIGHCONTRAST", 16, 12, &[16]),
];

struct Checks {
    n: u32,
}

impl Checks {
    /// Asks for `action` into a zeroed buffer; `None` if it failed.
    fn get(&self, action: u32, cb: u32) -> Option<[u8; 512]> {
        let mut buf = [0u8; 512];
        if cb != 0 {
            buf[0..4].copy_from_slice(&cb.to_le_bytes());
        }
        // SAFETY: our buffer, 512 bytes, larger than every structure
        // asked for here (NONCLIENTMETRICS at 504 is the largest).
        let ok = unsafe {
            SetLastError(0);
            SystemParametersInfoW(action, 0, buf.as_mut_ptr(), 0)
        };
        if ok == 0 { None } else { Some(buf) }
    }

    fn head(&self, action: u32) -> Line {
        Line::new().s(NAME.as_bytes()).s(b": check ").dec(i64::from(self.n)).s(b" 0x").hex(action)
    }

    #[track_caller]
    fn refused(&self, action: u32) {
        // SAFETY: a call with no arguments.
        let err = unsafe { GetLastError() };
        let l = core::panic::Location::caller().line();
        self.head(action)
            .s(b" refused err=")
            .dec(i64::from(err))
            .s(b" (line ")
            .dec(i64::from(l))
            .s(b")\r\n")
            .print();
    }

    #[track_caller]
    fn wrong(&self, action: u32, at: usize, got: i32, want: i32) {
        let l = core::panic::Location::caller().line();
        self.head(action)
            .s(b" word ")
            .dec(at as i64)
            .s(b" got ")
            .dec(i64::from(got))
            .s(b" want ")
            .dec(i64::from(want))
            .s(b" (line ")
            .dec(i64::from(l))
            .s(b")\r\n")
            .print();
    }

    /// The first word answered is `want`.
    #[track_caller]
    fn is(&mut self, action: u32, want: i32) -> Result<(), ()> {
        self.words(action, 0, &[want])
    }

    /// Answered at all, whatever the value.
    #[track_caller]
    fn some(&mut self, action: u32) -> Result<(), ()> {
        self.n += 1;
        match self.get(action, 0) {
            Some(_) => Ok(()),
            None => {
                self.refused(action);
                Err(())
            }
        }
    }

    /// The words answered, from the first.
    #[track_caller]
    fn words(&mut self, action: u32, cb: u32, want: &[i32]) -> Result<(), ()> {
        self.n += 1;
        let buf = match self.get(action, cb) {
            Some(buf) => buf,
            None => {
                self.refused(action);
                return Err(());
            }
        };
        for (i, &w) in want.iter().enumerate() {
            let at = i * 4;
            let got = i32::from_le_bytes([buf[at], buf[at + 1], buf[at + 2], buf[at + 3]]);
            if got != w {
                self.wrong(action, i, got, w);
                return Err(());
            }
        }
        Ok(())
    }

    /// Refused with ERROR_INVALID_PARAMETER, as an action no Windows
    /// knows is.
    #[track_caller]
    fn no(&mut self, action: u32) -> Result<(), ()> {
        self.n += 1;
        if self.get(action, 0).is_some() {
            self.wrong(action, 0, 1, 0);
            return Err(());
        }
        // SAFETY: a call with no arguments.
        let err = unsafe { GetLastError() };
        if err != 87 {
            self.wrong(action, 0, err as i32, 87);
            return Err(());
        }
        Ok(())
    }
}

/// The work area is the screen less whatever the desktop reserves for
/// itself: it starts at the origin, is as wide as the screen and no
/// taller than it.
fn work_area(c: &mut Checks) -> Result<(), ()> {
    c.n += 1;
    let buf = match c.get(0x0030, 0) {
        Some(buf) => buf,
        None => {
            c.refused(0x0030);
            return Err(());
        }
    };
    let mut r = [0i32; 4];
    for (i, v) in r.iter_mut().enumerate() {
        let at = i * 4;
        *v = i32::from_le_bytes([buf[at], buf[at + 1], buf[at + 2], buf[at + 3]]);
    }
    // SAFETY: calls with one argument each.
    let (w, h) = unsafe { (GetSystemMetrics(0), GetSystemMetrics(1)) };
    if r[0] != 0 || r[1] != 0 {
        c.wrong(0x0030, 0, r[0] | r[1], 0);
        return Err(());
    }
    if r[2] != w {
        c.wrong(0x0030, 2, r[2], w);
        return Err(());
    }
    if r[3] <= 0 || r[3] > h {
        c.wrong(0x0030, 3, r[3], h);
        return Err(());
    }
    Ok(())
}

/// The non-client metrics are the same measurements GetSystemMetrics
/// answers, and Windows answers them the same through both: the
/// border, the scroll bars, and the caption's button and height. They
/// are a desktop's own sizes, so the agreement is the invariant and
/// not the numbers.
fn nonclient(c: &mut Checks) -> Result<(), ()> {
    c.n += 1;
    let buf = match c.get(0x0029, 504) {
        Some(buf) => buf,
        None => {
            c.refused(0x0029);
            return Err(());
        }
    };
    let word = |at: usize| i32::from_le_bytes([buf[at], buf[at + 1], buf[at + 2], buf[at + 3]]);
    for (at, index) in [
        (4, 5),   // iBorderWidth, SM_CXBORDER
        (8, 2),   // iScrollWidth, SM_CXVSCROLL
        (12, 3),  // iScrollHeight, SM_CYHSCROLL
        (16, 30), // iCaptionWidth, SM_CXSIZE
    ] {
        // SAFETY: a call with one argument.
        let want = unsafe { GetSystemMetrics(index) };
        if word(at) != want {
            c.wrong(0x0029, at / 4, word(at), want);
            return Err(());
        }
    }
    for (at, index) in [
        (116, 52), // iSmCaptionWidth, SM_CXSMSIZE
        (216, 54), // iMenuWidth, SM_CXMENUSIZE
        (220, 55), // iMenuHeight, SM_CYMENUSIZE
        (500, 92), // iPaddedBorderWidth, SM_CXPADDEDBORDER
    ] {
        // SAFETY: a call with one argument.
        let want = unsafe { GetSystemMetrics(index) };
        if word(at) != want {
            c.wrong(0x0029, at / 4, word(at), want);
            return Err(());
        }
    }
    // A caption's own height is inside the metric for it, which counts
    // the border below the caption as well: 22 against SM_CYCAPTION's
    // 23 on the Windows machine, and the same for the small caption.
    for (at, index) in [(20, 4), (120, 51)] {
        // SAFETY: a call with one argument.
        let want = unsafe { GetSystemMetrics(index) };
        if word(at) <= 0 || word(at) > want {
            c.wrong(0x0029, at / 4, word(at), want);
            return Err(());
        }
    }
    // The caption font is the GUI font at 96 dpi.
    if word(24) != -12 {
        c.wrong(0x0029, 6, word(24), -12);
        return Err(());
    }
    Ok(())
}

fn checks() -> Result<(), ()> {
    let mut c = Checks { n: 0 };
    for &(action, _, want) in FLAGS {
        c.is(action, want)?;
    }
    for &(action, _) in SOME {
        c.some(action)?;
    }
    for &(action, _, cb, _, want) in STRUCTS {
        c.words(action, cb, want)?;
    }
    work_area(&mut c)?;
    nonclient(&mut c)?;
    // High contrast is off; the rest of its flags are the machine's.
    c.n += 1;
    let buf = c.get(0x0042, 16).ok_or(())?;
    if buf[4] & 1 != 0 {
        c.wrong(0x0042, 1, i32::from(buf[4]), 0);
        return Err(());
    }
    // An action no Windows knows is refused, not answered 0.
    c.no(0x7fff)?;
    // The ANSI entry answers the simple actions the wide one does.
    c.n += 1;
    let mut v = 0u32;
    // SAFETY: a u32 out for an action that answers a BOOL.
    let ok = unsafe { SystemParametersInfoA(0x1042, 0, (&raw mut v).cast(), 0) };
    if ok == 0 || v != 1 {
        c.wrong(0x1042, 0, v as i32, 1);
        return Err(());
    }
    Ok(())
}

/// Asks for `action` into a zeroed buffer and prints what it wrote, or
/// the error it failed with: the survey, which runs whether the checks
/// pass or not, so that a failure on another machine is readable.
fn ask(action: u32, name: &str, cb: u32, words: usize) {
    let mut buf = [0u8; 512];
    if cb != 0 {
        buf[0..4].copy_from_slice(&cb.to_le_bytes());
    }
    // SAFETY: our buffer, 512 bytes, larger than every structure asked
    // for here (NONCLIENTMETRICS at 504 is the largest).
    let (ok, err) = unsafe {
        SetLastError(0);
        let ok = SystemParametersInfoW(action, 0, buf.as_mut_ptr(), 0);
        (ok, GetLastError())
    };
    let mut line = Line::new().s(NAME.as_bytes()).s(b": 0x").hex(action).s(b" ").s(name.as_bytes());
    if ok == 0 {
        line.s(b" refused err=").dec(i64::from(err)).s(b"\r\n").print();
        return;
    }
    for at in (0..words.min(32)).step_by(4) {
        let w = i32::from_le_bytes([buf[at], buf[at + 1], buf[at + 2], buf[at + 3]]);
        line = line.s(b" ").dec(i64::from(w));
    }
    line.s(b"\r\n").print();
}

/// The non-client metrics past the caption font: the small caption,
/// the menu bar and the padded border, with the system metrics they are
/// the same measurements as.
fn survey_nonclient() {
    let mut buf = [0u8; 512];
    buf[0..4].copy_from_slice(&504u32.to_le_bytes());
    // SAFETY: our buffer, 512 bytes for a 504-byte structure.
    let ok = unsafe { SystemParametersInfoW(0x0029, 0, buf.as_mut_ptr(), 0) };
    if ok == 0 {
        return;
    }
    let word = |at: usize| i32::from_le_bytes([buf[at], buf[at + 1], buf[at + 2], buf[at + 3]]);
    let mut line = Line::new().s(NAME.as_bytes()).s(b": 0x0029 rest");
    for at in [116, 120, 216, 220, 500] {
        line = line.s(b" ").dec(i64::from(word(at)));
    }
    // SM_CXSMSIZE, SM_CYSMCAPTION, SM_CXMENUSIZE, SM_CYMENUSIZE,
    // SM_CXPADDEDBORDER.
    line = line.s(b" metrics");
    for index in [52, 51, 54, 55, 92] {
        // SAFETY: a call with one argument.
        let m = unsafe { GetSystemMetrics(index) };
        line = line.s(b" ").dec(i64::from(m));
    }
    line.s(b"
")
        .print();
}

fn survey() {
    for &(action, name, _) in FLAGS {
        ask(action, name, 0, 4);
    }
    for &(action, name) in SOME {
        ask(action, name, 0, 4);
    }
    for &(action, name, cb, words, _) in STRUCTS {
        ask(action, name, cb, words);
    }
    ask(0x0030, "GETWORKAREA", 0, 16);
    survey_nonclient();
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    survey();
    if checks().is_ok() {
        Line::new().s(NAME.as_bytes()).s(b": ok\r\n").print();
    }
    // SAFETY: ends the process.
    unsafe { ExitProcess(0) }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xc000_0409) }
}
