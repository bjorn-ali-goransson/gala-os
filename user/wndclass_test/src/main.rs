//! Window classes, as many as a real program registers (user32's class
//! table, ARCHITECTURE "user32").
//!
//! A Win32 program's classes are not a handful: Word's startup registers
//! its own and every Office library's, and the boot task it dies in
//! under gala is `IfrRegisterWndClasses` (ROADMAP OF). Windows puts no
//! small bound on them -- a class costs a little of the desktop heap and
//! nothing else -- so a program may register as many as it has kinds of
//! window, and all of them keep working.
//!
//! This program registers 200 of them. The number is not Windows'
//! limit, which is far higher, and not Word's need, which is to be
//! measured: it is more than any fixed table of a few dozen holds, and
//! enough that a table which grows has to grow repeatedly to hold it.
//! Then it checks what the classes are afterwards, because a
//! registration that answered an atom and left nothing behind would
//! pass a count and fail a program:
//!
//! - every registration answers a distinct application atom (0xc000 and
//!   up), and none answers 0;
//! - the 200th is really there: GetClassInfoExW finds it and gives back
//!   the procedure and style it was registered with;
//! - a name registered twice is refused with ERROR_CLASS_ALREADY_EXISTS,
//!   and the first registration stands;
//! - an unregistered class frees its name: UnregisterClassW accepts it
//!   and the name registers again;
//! - UnregisterClassW of a name nobody registered is refused with
//!   ERROR_CLASS_DOES_NOT_EXIST;
//! - all 200 unregister.
//!
//! Prints `wndclass_test: registered 200 of 200`, the atoms it checked,
//! and `wndclass_test: ok`; a refusal prints the first index and its
//! error, so a table too small says where it ran out. On failure:
//! `wndclass_test: check N got X (line L)`, and exits with N.

#![no_std]
#![no_main]

use ntrt as _;

const NAME: &str = "wndclass_test";

type Handle = usize;
type Hwnd = usize;
type WndProc = unsafe extern "system" fn(Hwnd, u32, usize, isize) -> isize;

/// How many classes one program registers here. See the module note:
/// more than a fixed table of a few dozen holds, far less than Windows
/// allows.
const CLASSES: usize = 200;

/// The first application atom. RegisterClassEx answers one of these.
const FIRST_ATOM: u16 = 0xc000;

const ERROR_CLASS_DOES_NOT_EXIST: u32 = 1411;
const ERROR_CLASS_ALREADY_EXISTS: u32 = 1410;

const CS_HREDRAW: u32 = 0x0002;
const CS_VREDRAW: u32 = 0x0001;
const STYLE: u32 = CS_HREDRAW | CS_VREDRAW;

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, overlapped: usize) -> i32;
    fn ExitProcess(code: u32) -> !;
    fn GetLastError() -> u32;
    fn SetLastError(code: u32);
    fn GetModuleHandleW(name: *const u16) -> Handle;
}

#[link(name = "user32", kind = "raw-dylib")]
unsafe extern "system" {
    fn RegisterClassExW(class: *const WndClassExW) -> u16;
    fn UnregisterClassW(name: *const u16, instance: Handle) -> i32;
    fn GetClassInfoExW(instance: Handle, name: *const u16, out: *mut WndClassExW) -> i32;
    fn DefWindowProcW(hwnd: Hwnd, msg: u32, wparam: usize, lparam: isize) -> isize;
}

#[repr(C)]
#[derive(Clone, Copy)]
struct WndClassExW {
    size: u32,
    style: u32,
    proc_: Option<WndProc>,
    class_extra: i32,
    window_extra: i32,
    instance: Handle,
    icon: Handle,
    cursor: Handle,
    background: Handle,
    menu_name: *const u16,
    class_name: *const u16,
    icon_small: Handle,
}

impl WndClassExW {
    /// An empty one, for GetClassInfoExW to fill. Its `size` is the one
    /// field Windows reads on the way in.
    fn empty() -> Self {
        Self {
            size: core::mem::size_of::<Self>() as u32,
            style: 0,
            proc_: None,
            class_extra: 0,
            window_extra: 0,
            instance: 0,
            icon: 0,
            cursor: 0,
            background: 0,
            menu_name: core::ptr::null(),
            class_name: core::ptr::null(),
            icon_small: 0,
        }
    }
}

unsafe extern "system" fn window_proc(hwnd: Hwnd, msg: u32, wparam: usize, lparam: isize) -> isize {
    // SAFETY: a plain call with the caller's own message.
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

/// Class number `i` as `gala-wndclass-000`, NUL-terminated. Rebuilt
/// rather than kept, so 200 names cost nothing.
fn class_name(i: usize) -> [u16; 18] {
    let mut out = [0u16; 18];
    let base = b"gala-wndclass-";
    let mut k = 0;
    while k < base.len() {
        out[k] = base[k] as u16;
        k += 1;
    }
    out[14] = b'0' as u16 + (i / 100) as u16;
    out[15] = b'0' as u16 + ((i / 10) % 10) as u16;
    out[16] = b'0' as u16 + (i % 10) as u16;
    out
}

fn describe(instance: Handle, name: *const u16, proc_: WndProc) -> WndClassExW {
    WndClassExW {
        size: core::mem::size_of::<WndClassExW>() as u32,
        style: STYLE,
        proc_: Some(proc_),
        class_extra: 0,
        window_extra: 0,
        instance,
        icon: 0,
        cursor: 0,
        background: 0,
        menu_name: core::ptr::null(),
        class_name: name,
        icon_small: 0,
    }
}

struct Line {
    buf: [u8; 240],
    n: usize,
}

impl Line {
    fn new() -> Self {
        Self { buf: [0; 240], n: 0 }
    }

    fn tag() -> Self {
        Self::new().s(NAME.as_bytes()).s(b": ")
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

struct Checks {
    n: u32,
}

impl Checks {
    #[track_caller]
    fn ok(&mut self, cond: bool, got: i64) -> Result<(), u32> {
        self.n += 1;
        if cond {
            return Ok(());
        }
        Line::tag()
            .s(b"check ")
            .dec(i64::from(self.n))
            .s(b" got ")
            .dec(got)
            .s(b" (line ")
            .dec(i64::from(core::panic::Location::caller().line()))
            .s(b")\r\n")
            .print();
        Err(self.n)
    }

    #[track_caller]
    fn eq(&mut self, got: i64, want: i64) -> Result<(), u32> {
        self.ok(got == want, got)
    }
}

fn run(c: &mut Checks) -> Result<(), u32> {
    // SAFETY: plain calls with our own structures and names.
    unsafe {
        let instance = GetModuleHandleW(core::ptr::null());
        // The one pointer, passed in and read back, so the comparison
        // below is of a value and not of two spellings of a function.
        let proc_: WndProc = window_proc;

        // 200 classes. A refusal is counted and its first index kept,
        // so a table that runs out says where rather than only that it
        // did.
        let mut atoms = [0u16; CLASSES];
        let mut made = 0usize;
        let mut first_bad = -1i64;
        let mut first_err = 0u32;
        for (i, slot) in atoms.iter_mut().enumerate() {
            let name = class_name(i);
            let atom = RegisterClassExW(&describe(instance, name.as_ptr(), proc_));
            if atom == 0 {
                if first_bad < 0 {
                    first_bad = i as i64;
                    first_err = GetLastError();
                }
                continue;
            }
            *slot = atom;
            made += 1;
        }
        Line::tag()
            .s(b"registered ")
            .dec(made as i64)
            .s(b" of ")
            .dec(CLASSES as i64)
            .s(b"\r\n")
            .print();
        if first_bad >= 0 {
            Line::tag()
                .s(b"first refusal at ")
                .dec(first_bad)
                .s(b", error ")
                .dec(i64::from(first_err))
                .s(b"\r\n")
                .print();
        }
        c.eq(made as i64, CLASSES as i64)?;

        // Every atom is an application atom, and no two are the same.
        let mut low = 0;
        let mut same = 0;
        for i in 0..CLASSES {
            if atoms[i] < FIRST_ATOM {
                low += 1;
            }
            for k in (i + 1)..CLASSES {
                if atoms[i] == atoms[k] {
                    same += 1;
                }
            }
        }
        Line::tag()
            .s(b"atoms: ")
            .dec(i64::from(low))
            .s(b" below 0xc000, ")
            .dec(i64::from(same))
            .s(b" repeated\r\n")
            .print();
        c.eq(i64::from(low), 0)?;
        c.eq(i64::from(same), 0)?;

        // The last one is really there, with what it was registered with.
        let last = class_name(CLASSES - 1);
        let mut info = WndClassExW::empty();
        c.ok(GetClassInfoExW(instance, last.as_ptr(), &mut info) != 0, 0)?;
        c.ok(info.proc_.map(|q| q as usize) == Some(proc_ as usize), 0)?;
        c.eq(i64::from(info.style), i64::from(STYLE))?;

        // A name twice: refused, and the first registration stands.
        let first = class_name(0);
        SetLastError(0);
        c.eq(i64::from(RegisterClassExW(&describe(instance, first.as_ptr(), proc_))), 0)?;
        c.eq(i64::from(GetLastError()), i64::from(ERROR_CLASS_ALREADY_EXISTS))?;
        let mut again = WndClassExW::empty();
        c.ok(GetClassInfoExW(instance, first.as_ptr(), &mut again) != 0, 0)?;
        c.eq(i64::from(again.style), i64::from(STYLE))?;

        // Unregistering frees the name for the same program again.
        c.ok(UnregisterClassW(first.as_ptr(), instance) != 0, 0)?;
        let mut gone = WndClassExW::empty();
        c.eq(i64::from(GetClassInfoExW(instance, first.as_ptr(), &mut gone)), 0)?;
        c.ok(RegisterClassExW(&describe(instance, first.as_ptr(), proc_)) != 0, 0)?;

        // A name nobody registered.
        let never = [
            b'n' as u16,
            b'o' as u16,
            b'-' as u16,
            b's' as u16,
            b'u' as u16,
            b'c' as u16,
            b'h' as u16,
            b'-' as u16,
            b'c' as u16,
            b'l' as u16,
            b'a' as u16,
            b's' as u16,
            b's' as u16,
            0,
        ];
        SetLastError(0);
        c.eq(i64::from(UnregisterClassW(never.as_ptr(), instance)), 0)?;
        c.eq(i64::from(GetLastError()), i64::from(ERROR_CLASS_DOES_NOT_EXIST))?;

        // And all 200 go.
        let mut dropped = 0usize;
        for i in 0..CLASSES {
            let name = class_name(i);
            if UnregisterClassW(name.as_ptr(), instance) != 0 {
                dropped += 1;
            }
        }
        Line::tag().s(b"unregistered ").dec(dropped as i64).s(b"\r\n").print();
        c.eq(dropped as i64, CLASSES as i64)?;
    }
    Ok(())
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    let mut c = Checks { n: 0 };
    let code = match run(&mut c) {
        Ok(()) => {
            Line::tag().s(b"ok\r\n").print();
            0
        }
        Err(n) => n,
    };
    // SAFETY: a plain call.
    unsafe { ExitProcess(code) }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xc000_0409) }
}
