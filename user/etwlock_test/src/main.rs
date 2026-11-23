//! Tracing while another thread is suspended in the middle of writing
//! an event.
//!
//! Office suspends its own threads to walk their stacks while the
//! process traces -- Word's hang detector does it to its main thread
//! twice a second (ROADMAP OF, `suspend:` lines) -- so a program's
//! threads are stopped at arbitrary instructions, including inside
//! `EventWrite`, while others keep writing. Real Windows costs the
//! writers nothing for that: measured on build 26300 as a standard
//! user, with a private in-process session of the program's own
//! enabling the provider,
//!
//! - a thread suspended anywhere, including inside the write path,
//!   leaves every other thread's `EventWrite` returning
//!   ERROR_SUCCESS at once: 200 rounds of (suspend, 20 writes,
//!   resume) took 16, 47 and 78 ms in three runs;
//! - the suspended thread picks its loop up again when it is resumed
//!   -- it made 30777, 46755 and 90329 writes of its own in those
//!   runs, so the suspensions landed on a thread that was writing;
//! - a buffering session never refuses a write: tens of thousands of
//!   events into four 64 KiB buffers, every `EventWrite` still
//!   answered ERROR_SUCCESS (the last status the loop thread saw is
//!   printed, and was 0).
//!
//! What this pins down for gala: the provider side must not make a
//! writer wait for anything a suspended thread can be holding. A lock
//! around the lists, held while the event is handed on, is such a
//! thing -- one suspension inside it and every writer in the process
//! spins for good (which is where Word's main thread sat: ROADMAP OF).
//!
//! The `etwlock_armed` scenario is the same program with `etw=`
//! armed, where gala's own session is listening and each admitted
//! write is handed to the kernel: the path Word was in.
//!
//! Prints what it counted and `etwlock_test: ok`, or
//! `etwlock_test: check N got X (line L)` and exits with N.

#![no_std]
#![no_main]

use core::ffi::c_void;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use ntrt as _;

const NAME: &str = "etwlock_test";

/// What a line printed here ends with, as a console line does.
const CRLF: &[u8] = b"\r\n";

type Handle = usize;

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, overlapped: usize) -> i32;
    fn ExitProcess(code: u32) -> !;
    fn CreateThread(
        attributes: *const c_void,
        stack: usize,
        start: unsafe extern "system" fn(*mut c_void) -> u32,
        parameter: *mut c_void,
        flags: u32,
        id: *mut u32,
    ) -> Handle;
    fn SuspendThread(thread: Handle) -> u32;
    fn ResumeThread(thread: Handle) -> u32;
    fn WaitForSingleObject(h: Handle, ms: u32) -> u32;
    fn CloseHandle(h: Handle) -> i32;
    fn GetTickCount64() -> u64;
    fn Sleep(ms: u32);
}

#[link(name = "advapi32", kind = "raw-dylib")]
unsafe extern "system" {
    fn StartTraceW(session: *mut u64, name: *const u16, properties: *mut Block) -> u32;
    fn ControlTraceW(session: u64, name: *const u16, properties: *mut Block, code: u32) -> u32;
    fn EnableTraceEx2(
        session: u64,
        provider: *const [u8; 16],
        control: u32,
        level: u8,
        match_any: u64,
        match_all: u64,
        timeout: u32,
        filter: *const c_void,
    ) -> u32;
    fn EventRegister(
        provider: *const [u8; 16],
        callback: usize,
        context: *mut c_void,
        handle: *mut u64,
    ) -> u32;
    fn EventUnregister(handle: u64) -> u32;
    fn EventWrite(
        handle: u64,
        descriptor: *const EventDescriptor,
        count: u32,
        data: *const c_void,
    ) -> u32;
}

/// EVENT_DESCRIPTOR: what an event says about itself, and the level and
/// keyword a session's demands are tested against.
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
        Self { id: 1, version: 0, channel: 0, level, opcode: 0, task: 0, keyword }
    }
}

/// EVENT_DATA_DESCRIPTOR: one piece of an event's payload.
#[repr(C)]
struct DataDescriptor {
    data: u64,
    size: u32,
    reserved: u32,
}

/// WNODE_HEADER, the WMI header every trace properties block starts
/// with.
#[repr(C)]
struct Wnode {
    buffer_size: u32,
    provider_id: u32,
    historical_context: u64,
    timestamp: i64,
    guid: [u8; 16],
    client_context: u32,
    flags: u32,
}

/// EVENT_TRACE_PROPERTIES.
#[repr(C)]
struct Properties {
    wnode: Wnode,
    buffer_size: u32,
    minimum_buffers: u32,
    maximum_buffers: u32,
    maximum_file_size: u32,
    log_file_mode: u32,
    flush_timer: u32,
    enable_flags: u32,
    age_limit: i32,
    number_of_buffers: u32,
    free_buffers: u32,
    events_lost: u32,
    buffers_written: u32,
    log_buffers_lost: u32,
    real_time_buffers_lost: u32,
    logger_thread_id: usize,
    log_file_name_offset: u32,
    logger_name_offset: u32,
}

/// The block the trace APIs read and write: the properties, then room
/// for the two names at the offsets the properties give.
#[repr(C)]
struct Block {
    props: Properties,
    log_file: [u16; 1024],
    name: [u16; 1024],
}

const PROPS_SIZE: usize = core::mem::size_of::<Properties>();
const AREA: usize = 2048; // bytes for each name
/// EVENT_TRACE_PRIVATE_LOGGER_MODE | PRIVATE_IN_PROC | BUFFERING_MODE:
/// a session of this process's own, in memory, with no log file.
const PRIVATE_BUFFERING: u32 = 0x800 | 0x2_0000 | 0x400;
const ENABLE: u32 = 1;
const STOP: u32 = 1;

impl Block {
    fn new(name: &str, mode: u32) -> Self {
        let mut block = Self {
            props: Properties {
                wnode: Wnode {
                    buffer_size: (PROPS_SIZE + 2 * AREA) as u32,
                    provider_id: 0,
                    historical_context: 0,
                    timestamp: 0,
                    guid: [0; 16],
                    client_context: 1,  // the performance counter
                    flags: 0x0002_0000, // WNODE_FLAG_TRACED_GUID
                },
                buffer_size: 64,
                minimum_buffers: 2,
                maximum_buffers: 4,
                maximum_file_size: 0,
                log_file_mode: mode,
                flush_timer: 0,
                enable_flags: 0,
                age_limit: 0,
                number_of_buffers: 0,
                free_buffers: 0,
                events_lost: 0,
                buffers_written: 0,
                log_buffers_lost: 0,
                real_time_buffers_lost: 0,
                logger_thread_id: 0,
                log_file_name_offset: PROPS_SIZE as u32,
                logger_name_offset: (PROPS_SIZE + AREA) as u32,
            },
            log_file: [0; 1024],
            name: [0; 1024],
        };
        for (i, b) in name.bytes().enumerate() {
            block.name[i] = u16::from(b);
        }
        block
    }
}

/// The GUID this program's provider is known by, in the mixed
/// endianness a GUID is laid out in.
/// 2b8e5c91-4d07-4f3a-91c6-5e0d8a7b4f12
const PROVIDER: [u8; 16] = [
    0x91, 0x5c, 0x8e, 0x2b, 0x07, 0x4d, 0x3a, 0x4f, 0x91, 0xc6, 0x5e, 0x0d, 0x8a, 0x7b, 0x4f, 0x12,
];

/// The event the writers write: in reach of the session enabled below
/// (level 4 against its 5, keyword 1 against its every keyword), with
/// one small piece of payload, so a write does the work a real one
/// does rather than being turned away at the first test.
static EVENT: EventDescriptor = EventDescriptor::new(4, 1);

/// The provider handle the writer thread writes on, how many writes it
/// has made, and whether it is to stop.
static REGISTRATION: AtomicU64 = AtomicU64::new(0);
static WRITES: AtomicU64 = AtomicU64::new(0);
/// What the loop thread's last write answered, and after how many
/// writes, when it was not ERROR_SUCCESS.
static WRITER_STATUS: AtomicU32 = AtomicU32::new(0);
static WRITER_STOPPED_AT: AtomicU64 = AtomicU64::new(0);
static ENOUGH: AtomicU32 = AtomicU32::new(0);

/// How many writes the loop thread makes at most, which bounds what the
/// armed scenario hands the kernel.
const WRITER_MAX: u64 = 2_000_000;

/// The thread that is suspended in the middle of its writes.
unsafe extern "system" fn writer(_: *mut c_void) -> u32 {
    let handle = REGISTRATION.load(Ordering::Acquire);
    let payload = 0x5eed_1234_u32;
    let piece = DataDescriptor { data: (&raw const payload) as usize as u64, size: 4, reserved: 0 };
    while ENOUGH.load(Ordering::Relaxed) == 0 && WRITES.load(Ordering::Relaxed) < WRITER_MAX {
        // SAFETY: our registration, a descriptor and a payload piece
        // this frame owns.
        let r = unsafe { EventWrite(handle, &EVENT, 1, (&raw const piece).cast()) };
        if r != 0 {
            // Said rather than checked here: a failure is the main
            // thread's to report, with its count.
            WRITER_STATUS.store(r, Ordering::Relaxed);
            WRITER_STOPPED_AT.store(WRITES.load(Ordering::Relaxed), Ordering::Relaxed);
            return r;
        }
        WRITES.fetch_add(1, Ordering::Relaxed);
    }
    0
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

/// Waits until the loop thread has made another write, so that it is
/// stopped in the middle of its loop -- which is almost all
/// `EventWrite` -- rather than before it has begun or after it has
/// gone to sleep.
fn wrote_again(c: &mut Checks, since: u64, started: u64) -> Result<u64, u32> {
    loop {
        let now = WRITES.load(Ordering::Relaxed);
        if now > since {
            return Ok(now);
        }
        c.ok(ticks() - started < BUDGET_MS, now as i64)?;
        // SAFETY: plain call.
        unsafe { Sleep(0) };
    }
}

/// How many rounds of (suspend, write, resume), and how many writes
/// this thread makes in each: enough that a suspension lands inside
/// the write path, few enough that the whole step is a moment's work
/// on Windows.
const ROUNDS: u32 = 200;
const PER_ROUND: u32 = 20;
/// The whole step's budget. Windows takes milliseconds; a provider
/// side that makes a writer wait for the suspended thread never
/// returns at all, so this bound is what a failure is reported from
/// when the wait is merely long.
const BUDGET_MS: u64 = 30_000;

fn run() -> Result<(), u32> {
    let c = &mut Checks { n: 0 };

    // A session of this program's own, private, in memory, with no log
    // file: the one kind a program can start without administrator
    // rights, so the same binary measures this on Windows and runs
    // under gala.
    let mut block = Block::new("gala-etwlock-test", PRIVATE_BUFFERING);
    let mut session = 0u64;
    let name = block.name.as_ptr();
    // SAFETY: our block and name, and a word for the handle.
    let r = unsafe { StartTraceW(&mut session, name, &mut block) };
    c.eq(i64::from(r), 0)?;
    c.ok(session != 0, session as i64)?;

    let mut registration = 0u64;
    // SAFETY: our GUID and a word for the handle; no callback, because
    // what the provider is told is `etw_test`'s business.
    let r = unsafe { EventRegister(&PROVIDER, 0, core::ptr::null_mut(), &mut registration) };
    c.eq(i64::from(r), 0)?;
    REGISTRATION.store(registration, Ordering::Release);

    // Level 5 and every keyword: the event above is in reach, so every
    // write below is assembled and handed on for real.
    // SAFETY: our session and GUID.
    let r =
        unsafe { EnableTraceEx2(session, &PROVIDER, ENABLE, 5, u64::MAX, 0, 0, core::ptr::null()) };
    c.eq(i64::from(r), 0)?;

    // SAFETY: a thread of our own, with no parameter.
    let loop_thread = unsafe {
        CreateThread(core::ptr::null(), 0, writer, core::ptr::null_mut(), 0, core::ptr::null_mut())
    };
    c.ok(loop_thread != 0, loop_thread as i64)?;

    // It is writing before it is suspended, so that a suspension can
    // land inside a write.
    let started = ticks();
    let mut seen = wrote_again(c, 0, started)?;

    let mut writes = 0u64;
    for _ in 0..ROUNDS {
        // SAFETY: our thread's handle.
        let before = unsafe { SuspendThread(loop_thread) };
        c.ok(before != u32::MAX, i64::from(before))?;
        for _ in 0..PER_ROUND {
            // SAFETY: our registration and descriptor, with no payload.
            let r = unsafe { EventWrite(registration, &EVENT, 0, core::ptr::null()) };
            c.eq(i64::from(r), 0)?;
            writes += 1;
        }
        // SAFETY: as above.
        let had = unsafe { ResumeThread(loop_thread) };
        c.eq(i64::from(had), 1)?;
        c.ok(ticks() - started < BUDGET_MS, (ticks() - started) as i64)?;
        // And it writes again before the next round stops it.
        seen = wrote_again(c, seen, started)?;
    }
    let spent = ticks() - started;

    ENOUGH.store(1, Ordering::Relaxed);
    // SAFETY: our thread's handle.
    let waited = unsafe { WaitForSingleObject(loop_thread, 10_000) };
    c.eq(i64::from(waited), 0)?; // not WAIT_TIMEOUT
    // SAFETY: as above.
    unsafe { CloseHandle(loop_thread) };

    let looped = WRITES.load(Ordering::Relaxed);
    Line::tag()
        .s(b"wrote ")
        .dec(writes as i64)
        .s(b" events while the other thread was suspended, in ")
        .dec(ROUNDS as i64)
        .s(b" rounds taking ")
        .dec(spent as i64)
        .s(b" ms; it wrote ")
        .dec(looped as i64)
        .s(b", last status 0x")
        .hex(u64::from(WRITER_STATUS.load(Ordering::Relaxed)))
        .s(b" at ")
        .dec(WRITER_STOPPED_AT.load(Ordering::Relaxed) as i64)
        .s(CRLF)
        .print();

    // The suspended thread went on writing every time it was resumed,
    // so its count is not merely the one write it had made before the
    // rounds began.
    c.ok(looped > u64::from(ROUNDS), looped as i64)?;

    let mut stop = Block::new("gala-etwlock-test", 0);
    let stop_name = stop.name.as_ptr();
    // SAFETY: our block, with room for the names ETW writes back.
    let r = unsafe { ControlTraceW(session, stop_name, &mut stop, STOP) };
    c.eq(i64::from(r), 0)?;
    // SAFETY: our registration.
    let r = unsafe { EventUnregister(registration) };
    c.eq(i64::from(r), 0)?;

    Ok(())
}

/// `GetTickCount64`, wrapped so the calls above read as what they
/// measure.
fn ticks() -> u64 {
    // SAFETY: plain call.
    unsafe { GetTickCount64() }
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    let code = match run() {
        Ok(()) => {
            Line::tag().s(b"ok\r\n").print();
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
