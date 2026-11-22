//! A manifest provider's events: the kind whose field names are in no
//! part of the event (nt::etw, OBSERVABILITY "A session gala starts").
//!
//! TraceLogging ships an event's names with it, so a session can read
//! every field. A manifest provider does not: its events are numbers
//! and bytes, and the names belong to a `WEVT_TEMPLATE` resource in
//! some other file. Every event Word's startup wrote that a session
//! kept is one of these, including the 494-byte error it writes before
//! it gives up (ROADMAP OF), and the line for it could say only how
//! many bytes the program had built.
//!
//! So the line says what those bytes carried. A payload is mostly
//! strings -- a message, a path, a name -- and a string is readable
//! without knowing which field it is. This program writes the four
//! shapes that answer has to get right:
//!
//! - a UTF-16 message with a number behind it, which is an error event;
//! - 8-bit text with binary bytes behind it;
//! - numbers alone, where saying there is no string in them is itself
//!   the finding;
//! - more strings than a line prints, the first longer than a line
//!   gives one string.
//!
//! What it checks holds on both systems, because it checks the
//! provider's own two answers against each other rather than a number
//! only one of them has: `EventEnabled` and `EventProviderEnabled`
//! agree for every event, and every write is accepted whether anybody
//! is listening or not. The bytes each event carried are printed from
//! the same constants it wrote, so the line and the payload cannot
//! drift. Reading them is the session's business, which is the
//! `etwraw_armed` scenario's.
//!
//! Prints what it read and `etwraw_test: ok`, or
//! `etwraw_test: check N got X (line L)` and exits with N.

#![no_std]
#![no_main]

use core::ffi::c_void;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use ntrt as _;

const NAME: &str = "etwraw_test";

type Handle = usize;

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, overlapped: usize) -> i32;
    fn ExitProcess(code: u32) -> !;
}

/// What ETW calls when a session's demands on a provider change.
type EnableCallback = unsafe extern "system" fn(
    source: *const [u8; 16],
    is_enabled: u32,
    level: u8,
    match_any: u64,
    match_all: u64,
    filter: *const c_void,
    context: *mut c_void,
);

#[link(name = "advapi32", kind = "raw-dylib")]
unsafe extern "system" {
    fn EventRegister(
        provider: *const [u8; 16],
        callback: Option<EnableCallback>,
        context: *mut c_void,
        handle: *mut u64,
    ) -> u32;
    fn EventUnregister(handle: u64) -> u32;
    fn EventEnabled(handle: u64, descriptor: *const EventDescriptor) -> u8;
    fn EventProviderEnabled(handle: u64, level: u8, keyword: u64) -> u8;
    fn EventWrite(
        handle: u64,
        descriptor: *const EventDescriptor,
        count: u32,
        data: *const DataDescriptor,
    ) -> u32;
}

/// EVENT_DESCRIPTOR.
#[repr(C)]
struct EventDescriptor {
    id: u16,
    version: u8,
    channel: u8,
    level: u8,
    opcode: u8,
    task: u16,
    keyword: u64,
}

impl EventDescriptor {
    /// A manifest provider's event is named by a number its manifest
    /// describes, which is why nothing in the event names it.
    const fn new(id: u16, level: u8, keyword: u64) -> Self {
        Self { id, version: 0, channel: 0, level, opcode: 0, task: 0, keyword }
    }
}

/// EVENT_DATA_DESCRIPTOR. `kind` is 0 for the event's own data, 1 for
/// the event metadata and 2 for the provider metadata; a manifest
/// provider writes nothing but kind 0.
#[repr(C)]
struct DataDescriptor {
    ptr: u64,
    size: u32,
    kind: u32,
}

impl DataDescriptor {
    fn new(bytes: &[u8]) -> Self {
        Self { ptr: bytes.as_ptr() as u64, size: bytes.len() as u32, kind: 0 }
    }
}

/// The provider, which is a GUID and nothing else: a manifest
/// provider's name is not in the program, so no name hashes to this.
const GUID_TEXT: &str = "7a1b4e6c-2d3f-4a58-9b0c-1e2f3a4b5c6d";
const GUID: [u8; 16] = [
    0x6c, 0x4e, 0x1b, 0x7a, 0x3f, 0x2d, 0x58, 0x4a, 0x9b, 0x0c, 0x1e, 0x2f, 0x3a, 0x4b, 0x5c, 0x6d,
];

/// An ASCII string as a payload carries one: UTF-16LE, which is what a
/// program's own message is in. The length is checked here, so a
/// constant that does not match its text is a build error rather than a
/// line that disagrees with the bytes.
const fn wide<const N: usize>(s: &str) -> [u8; N] {
    let b = s.as_bytes();
    assert!(b.len() * 2 == N, "a wide string is twice its ASCII text");
    let mut out = [0u8; N];
    let mut i = 0;
    while i < b.len() {
        out[i * 2] = b[i];
        i += 1;
    }
    out
}

/// Event 100, the shape of an error: a message and the code behind it.
const MESSAGE_TEXT: &str = "a manifest event's message";
const MESSAGE: [u8; 52] = wide(MESSAGE_TEXT);
const CODE: u32 = 13;

/// Event 101: 8-bit text, with bytes that are not text behind it.
const TEXT: &[u8] = b"eight bit text";
const BLOB: [u8; 4] = [0xde, 0xad, 0xbe, 0xef];

/// Event 102: numbers, and not one string among them.
const NUMBERS: [u8; 8] = [1, 2, 3, 4, 5, 6, 7, 8];

/// Event 103: a string longer than a line gives one, then more strings
/// than a line names, each ended the way a payload ends one.
const LONG_TEXT: &str = "abcdefghijabcdefghijabcdefghijabcdefghijabcdefghijabcdefghijabcdefghijabcdefghijabcdefghijabcdefghijabcdefghijabcdefghijabcdefghijabcdefghijabcdefghijabcdefghijabcdefghijabcdefghijabcdefghijabcdefghij";
const LONG: [u8; 400] = wide(LONG_TEXT);
const NUL: [u8; 2] = [0, 0];
const PIECES: [[u8; 16]; 6] = [
    wide("piece-01"),
    wide("piece-02"),
    wide("piece-03"),
    wide("piece-04"),
    wide("piece-05"),
    wide("piece-06"),
];

/// What the provider's enable callback was told, and how often it was
/// called. A provider learns a session's demands here and nowhere else.
struct Seen {
    calls: AtomicU32,
    enabled: AtomicU32,
    level: AtomicU32,
    any: AtomicU64,
    all: AtomicU64,
}

static SEEN: Seen = Seen {
    calls: AtomicU32::new(0),
    enabled: AtomicU32::new(0),
    level: AtomicU32::new(0),
    any: AtomicU64::new(0),
    all: AtomicU64::new(0),
};

/// The context the registration is made with, and the one its callback
/// is handed back.
const CONTEXT: u64 = 0x5103;

unsafe extern "system" fn enable_callback(
    _source: *const [u8; 16],
    is_enabled: u32,
    level: u8,
    match_any: u64,
    match_all: u64,
    _filter: *const c_void,
    _context: *mut c_void,
) {
    SEEN.enabled.store(is_enabled, Ordering::Relaxed);
    SEEN.level.store(u32::from(level), Ordering::Relaxed);
    SEEN.any.store(match_any, Ordering::Relaxed);
    SEEN.all.store(match_all, Ordering::Relaxed);
    SEEN.calls.fetch_add(1, Ordering::Relaxed);
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

    fn hex(self, v: u64) -> Self {
        let mut d = [0u8; 16];
        let mut i = d.len();
        let mut left = v;
        loop {
            i -= 1;
            d[i] = b"0123456789abcdef"[(left & 0xf) as usize];
            left >>= 4;
            if left == 0 {
                break;
            }
        }
        self.s(b"0x").s(&d[i..])
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

/// Says what the provider was told, or that nothing told it anything.
fn told() {
    let line = Line::tag().s(GUID_TEXT.as_bytes()).s(b": ");
    if SEEN.calls.load(Ordering::Relaxed) == 0 {
        line.s(b"no session\r\n").print();
        return;
    }
    line.s(b"calls=")
        .dec(i64::from(SEEN.calls.load(Ordering::Relaxed)))
        .s(b" enabled=")
        .dec(i64::from(SEEN.enabled.load(Ordering::Relaxed)))
        .s(b" level=")
        .dec(i64::from(SEEN.level.load(Ordering::Relaxed)))
        .s(b" any=")
        .hex(SEEN.any.load(Ordering::Relaxed))
        .s(b" all=")
        .hex(SEEN.all.load(Ordering::Relaxed))
        .s(b"\r\n")
        .print();
}

fn main() -> Result<(), u32> {
    let mut c = Checks { n: 0 };
    let mut handle = 0u64;
    // SAFETY: our GUID, our callback and our output.
    let r =
        unsafe { EventRegister(&GUID, Some(enable_callback), CONTEXT as *mut c_void, &mut handle) };
    c.eq(i64::from(r), 0)?;
    c.ok(handle != 0, handle as i64)?;

    // A session that enables a provider has already called its callback
    // by the time the registration returns, so what it was told is
    // readable right here.
    told();

    let code = CODE.to_ne_bytes();
    let message = [DataDescriptor::new(&MESSAGE), DataDescriptor::new(&code)];
    let text = [DataDescriptor::new(TEXT), DataDescriptor::new(&BLOB)];
    let numbers = [DataDescriptor::new(&NUMBERS)];
    // A payload ends one string before the next begins, so the strings
    // in it are strings and not one long run.
    let strings = [
        DataDescriptor::new(&LONG),
        DataDescriptor::new(&NUL),
        DataDescriptor::new(&PIECES[0]),
        DataDescriptor::new(&NUL),
        DataDescriptor::new(&PIECES[1]),
        DataDescriptor::new(&NUL),
        DataDescriptor::new(&PIECES[2]),
        DataDescriptor::new(&NUL),
        DataDescriptor::new(&PIECES[3]),
        DataDescriptor::new(&NUL),
        DataDescriptor::new(&PIECES[4]),
        DataDescriptor::new(&NUL),
        DataDescriptor::new(&PIECES[5]),
        DataDescriptor::new(&NUL),
    ];

    let mut line = Line::tag().s(b"wrote");
    for (id, level, keyword, data) in [
        (100u16, 2u8, 0x1u64, &message[..]),
        (101, 4, 0x2, &text[..]),
        (102, 5, 0x4, &numbers[..]),
        (103, 4, 0x8, &strings[..]),
    ] {
        let descriptor = EventDescriptor::new(id, level, keyword);
        // SAFETY: our registration and our descriptor.
        let (by_event, by_demands) = unsafe {
            (EventEnabled(handle, &descriptor), EventProviderEnabled(handle, level, keyword))
        };
        // The two functions a provider may ask with always agree,
        // whether a session is listening or not.
        c.eq(i64::from(by_event), i64::from(by_demands))?;
        // SAFETY: our registration, our descriptor and the pieces,
        // which stay alive for the call.
        let r = unsafe { EventWrite(handle, &descriptor, data.len() as u32, data.as_ptr()) };
        // A write nobody listens to is accepted and goes nowhere, which
        // is what makes a quiet provider cost a program nothing.
        c.eq(i64::from(r), 0)?;
        let mut bytes = 0i64;
        for d in data {
            bytes += i64::from(d.size);
        }
        line = line.s(b" ").dec(bytes);
    }
    line.s(b" bytes\r\n").print();

    // SAFETY: our registration.
    c.eq(i64::from(unsafe { EventUnregister(handle) }), 0)?;
    Line::tag().s(b"ok\r\n").print();
    Ok(())
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    let code = match main() {
        Ok(()) => 0,
        Err(n) => n,
    };
    // SAFETY: the only way out.
    unsafe { ExitProcess(code) }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xc000_0409) }
}
