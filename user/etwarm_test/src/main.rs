//! The trace session gala starts on a program's behalf (`etw=`,
//! nt::etw; OBSERVABILITY "Trace sessions gala starts"): what a program
//! that starts no session of its own and asks nobody to listen -- which
//! is every real program, a Word startup registering 20 providers and
//! starting nothing -- sees when something else enables it.
//!
//! The program registers two TraceLogging providers and writes
//! TraceLogging-shaped events, as a program built on
//! `TraceLoggingProvider.h` does: the provider's name in a metadata
//! blob of type 2, the event's name and its fields in one of type 1,
//! the field values after them, and an event id of 0 because a
//! TraceLogging event is named by its metadata and not by a number. The
//! GUID each provider is known by is the hash of its name, by the
//! algorithm that header documents, so a session can name either one
//! without a manifest.
//!
//! What it checks is the same on both systems, because it checks the
//! provider's own two answers against each other rather than a number
//! only one of them has:
//!
//! - With nothing enabling a provider -- real Windows, where no session
//!   is running, and gala without `etw=` -- no callback comes,
//!   `EventEnabled` and `EventProviderEnabled` answer FALSE for every
//!   level and keyword, and a write is still accepted (it goes
//!   nowhere).
//! - With something enabling it, the callback is where the demands
//!   arrive, and both answers agree with those demands for every row of
//!   the matrix: in reach when the level is 0 or at most the session's
//!   and the keyword is 0 or overlaps MatchAnyKeyword and holds every
//!   bit of MatchAllKeyword.
//!
//! So the lines say which world the run was in, and the demands gala's
//! own session asked for are the scenario's business to pin down (the
//! `etw_arm`, `etw_arm_all` and `etw_arm_none` scenarios).
//!
//! Prints what it read and `etwarm_test: ok`, or
//! `etwarm_test: check N got X (line L)` and exits with N.

#![no_std]
#![no_main]

use core::ffi::c_void;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use ntrt as _;

const NAME: &str = "etwarm_test";

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

/// EVENT_DESCRIPTOR. A TraceLogging event's id is 0 and its channel is
/// 11: the header builds the descriptor out of the event's metadata,
/// which is where its name lives.
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
    const fn new(level: u8, keyword: u64) -> Self {
        Self { id: 0, version: 0, channel: 11, level, opcode: 0, task: 0, keyword }
    }
}

/// EVENT_DATA_DESCRIPTOR. `kind` is 0 for the event's own data, 1 for
/// the event metadata and 2 for the provider metadata.
#[repr(C)]
struct DataDescriptor {
    ptr: u64,
    size: u32,
    kind: u32,
}

impl DataDescriptor {
    fn new(bytes: &[u8], kind: u32) -> Self {
        Self { ptr: bytes.as_ptr() as u64, size: bytes.len() as u32, kind }
    }
}

/// The provider whose events a run asks for by name, and the GUID that
/// name hashes to (2542af41-3d78-596e-ff31-017dbbeacc32).
const ARM_NAME: &str = "Gala.Test.Arm";
const ARM: [u8; 16] = [
    0x41, 0xaf, 0x42, 0x25, 0x78, 0x3d, 0x6e, 0x59, 0xff, 0x31, 0x01, 0x7d, 0xbb, 0xea, 0xcc, 0x32,
];
/// A second provider of the same program, so a run that names the first
/// one shows that naming it is not the same as naming them all
/// (c7be3843-7267-5b9d-1d9c-ebe8fe08d1fd).
const QUIET_NAME: &str = "Gala.Test.Quiet";
const QUIET: [u8; 16] = [
    0x43, 0x38, 0xbe, 0xc7, 0x67, 0x72, 0x9d, 0x5b, 0x1d, 0x9c, 0xeb, 0xe8, 0xfe, 0x08, 0xd1, 0xfd,
];

/// The provider metadata of each: a `u16` of the whole blob's size,
/// then the name in UTF-8, nul-terminated.
const ARM_META: [u8; 16] =
    [16, 0, b'G', b'a', b'l', b'a', b'.', b'T', b'e', b's', b't', b'.', b'A', b'r', b'm', 0];
const QUIET_META: [u8; 18] = [
    18, 0, b'G', b'a', b'l', b'a', b'.', b'T', b'e', b's', b't', b'.', b'Q', b'u', b'i', b'e',
    b't', 0,
];

/// The event metadata of the event written here: the size, one tag byte
/// (no tags, high bit clear), the event's name, then a field each --
/// `count`, a `u32` (in-type 8), and `why`, a nul-terminated 8-bit
/// string (in-type 2).
const EVENT_META: [u8; 23] = [
    23, 0, 0, b'S', b't', b'a', b'r', b't', b'e', b'd', 0, b'c', b'o', b'u', b'n', b't', 0, 8,
    b'w', b'h', b'y', 0, 2,
];

/// The two fields' values, in the order the metadata names them.
const COUNT: u32 = 7;
const WHY: &[u8] = b"armed\0";

/// What each provider's enable callback was told, and how often it was
/// called. A provider learns a session's demands here and nowhere else.
struct Seen {
    calls: AtomicU32,
    enabled: AtomicU32,
    level: AtomicU32,
    any: AtomicU64,
    all: AtomicU64,
    context: AtomicU64,
}

impl Seen {
    const fn new() -> Self {
        Self {
            calls: AtomicU32::new(0),
            enabled: AtomicU32::new(0),
            level: AtomicU32::new(0),
            any: AtomicU64::new(0),
            all: AtomicU64::new(0),
            context: AtomicU64::new(0),
        }
    }
}

/// Slot 0 is the provider a run may name, slot 1 the other.
static SEEN: [Seen; 2] = [Seen::new(), Seen::new()];

/// The context each registration is made with, which is also the slot
/// its callback stores into: a provider that keeps its state in the
/// context finds it again this way.
const CONTEXT: [u64; 2] = [0x5101, 0x5102];

unsafe extern "system" fn enable_callback(
    _source: *const [u8; 16],
    is_enabled: u32,
    level: u8,
    match_any: u64,
    match_all: u64,
    _filter: *const c_void,
    context: *mut c_void,
) {
    let slot = usize::from(context as u64 == CONTEXT[1]);
    let seen = &SEEN[slot];
    seen.enabled.store(is_enabled, Ordering::Relaxed);
    seen.level.store(u32::from(level), Ordering::Relaxed);
    seen.any.store(match_any, Ordering::Relaxed);
    seen.all.store(match_all, Ordering::Relaxed);
    seen.context.store(context as u64, Ordering::Relaxed);
    seen.calls.fetch_add(1, Ordering::Relaxed);
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

/// Whether an event of that level and keyword is in reach of those
/// demands: Windows' rule, which is what both answers have to follow.
fn in_reach(level: u8, keyword: u64, want_level: u8, any: u64, all: u64) -> bool {
    let by_level = level == 0 || level <= want_level;
    let by_keyword = keyword == 0 || (keyword & any != 0 && (all == 0 || keyword & all == all));
    by_level && by_keyword
}

/// The rows every provider is asked about, enabled or not.
const ROWS: [(u8, u64); 8] =
    [(0, 0), (1, 0), (4, 0), (5, 0), (6, 0), (255, 0), (4, 0x10), (4, 0x4000_0000_0000_0000)];

/// Reads one provider's two answers for every row and checks they agree
/// with each other and with the demands its callback was told. Prints
/// the row of answers, so a run says what each provider was asked for
/// and what it said.
fn matrix(c: &mut Checks, slot: usize, handle: u64, name: &str) -> Result<(), u32> {
    let seen = &SEEN[slot];
    let called = seen.calls.load(Ordering::Relaxed) != 0;
    let enabled = called && seen.enabled.load(Ordering::Relaxed) != 0;
    let level = seen.level.load(Ordering::Relaxed) as u8;
    let (any, all) = (seen.any.load(Ordering::Relaxed), seen.all.load(Ordering::Relaxed));
    let mut line = Line::tag().s(name.as_bytes()).s(b":");
    for &(row_level, keyword) in &ROWS {
        let descriptor = EventDescriptor::new(row_level, keyword);
        // SAFETY: our registration and our descriptor.
        let (by_event, by_demands) = unsafe {
            (EventEnabled(handle, &descriptor), EventProviderEnabled(handle, row_level, keyword))
        };
        line = line
            .s(b" ")
            .dec(i64::from(row_level))
            .s(b"/")
            .hex(keyword)
            .s(b"=")
            .dec(i64::from(by_event))
            .dec(i64::from(by_demands));
        // The two functions a provider may ask with always agree, and
        // what they agree on is the demands the callback brought (FALSE
        // for every row when nothing enabled it).
        c.eq(i64::from(by_event), i64::from(by_demands))?;
        let want = enabled && in_reach(row_level, keyword, level, any, all);
        c.eq(i64::from(by_event != 0), i64::from(want))?;
    }
    line.s(b"\r\n").print();
    Ok(())
}

/// Says what a provider was told, or that nothing told it anything.
fn told(slot: usize, name: &str) {
    let seen = &SEEN[slot];
    let line = Line::tag().s(name.as_bytes()).s(b": ");
    if seen.calls.load(Ordering::Relaxed) == 0 {
        line.s(b"no session\r\n").print();
        return;
    }
    line.s(b"calls=")
        .dec(i64::from(seen.calls.load(Ordering::Relaxed)))
        .s(b" enabled=")
        .dec(i64::from(seen.enabled.load(Ordering::Relaxed)))
        .s(b" level=")
        .dec(i64::from(seen.level.load(Ordering::Relaxed)))
        .s(b" any=")
        .hex(seen.any.load(Ordering::Relaxed))
        .s(b" all=")
        .hex(seen.all.load(Ordering::Relaxed))
        .s(b" ctx=")
        .hex(seen.context.load(Ordering::Relaxed))
        .s(b"\r\n")
        .print();
}

/// Writes the program's event as TraceLogging lays one out: the
/// provider's metadata, the event's, then the two field values.
fn write_event(handle: u64, meta: &[u8], level: u8, keyword: u64) -> u32 {
    let descriptor = EventDescriptor::new(level, keyword);
    let count = COUNT.to_ne_bytes();
    let data = [
        DataDescriptor::new(meta, 2),
        DataDescriptor::new(&EVENT_META, 1),
        DataDescriptor::new(&count, 0),
        DataDescriptor::new(WHY, 0),
    ];
    // SAFETY: our registration, our descriptor and our four pieces,
    // which stay alive for the call.
    unsafe { EventWrite(handle, &descriptor, data.len() as u32, data.as_ptr()) }
}

fn main() -> Result<(), u32> {
    let mut c = Checks { n: 0 };
    let mut handles = [0u64; 2];
    for (slot, guid) in [&ARM, &QUIET].into_iter().enumerate() {
        // SAFETY: our GUID, our callback and our output.
        let r = unsafe {
            EventRegister(
                guid,
                Some(enable_callback),
                CONTEXT[slot] as *mut c_void,
                &mut handles[slot],
            )
        };
        c.eq(i64::from(r), 0)?;
        c.ok(handles[slot] != 0, handles[slot] as i64)?;
    }

    // A session that enables a provider has already called its callback
    // by the time the registration returns, so what each provider was
    // told is readable right here.
    told(0, ARM_NAME);
    told(1, QUIET_NAME);
    matrix(&mut c, 0, handles[0], ARM_NAME)?;
    matrix(&mut c, 1, handles[1], QUIET_NAME)?;

    // Both providers write, in reach or not: a write nobody listens to
    // is accepted and goes nowhere, which is what makes a quiet
    // provider cost a program nothing.
    let mut wrote = 0;
    for (slot, meta) in [&ARM_META[..], &QUIET_META[..]].into_iter().enumerate() {
        for &(level, keyword) in &[(4u8, 0u64), (255, 0)] {
            let r = write_event(handles[slot], meta, level, keyword);
            c.eq(i64::from(r), 0)?;
            wrote += 1;
        }
    }
    Line::tag().s(b"wrote ").dec(wrote).s(b"\r\n").print();

    for handle in handles {
        // SAFETY: our registration.
        c.eq(i64::from(unsafe { EventUnregister(handle) }), 0)?;
    }
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
