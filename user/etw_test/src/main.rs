//! A trace session of the program's own, and what it makes of a
//! provider. A provider that nobody listens to writes nothing
//! (advapi32_test covers that side); everything a program traces
//! starts with a session enabling it, and this is the contract that
//! decides which of its events a program bothers to assemble at all.
//!
//! The session started here is private, in-process and buffering, with
//! no log file: the one kind a program can start without administrator
//! rights, and the only one that needs no filesystem, so the same
//! binary runs on Windows and under gala. The answers checked are real
//! Windows' own, measured on build 26300 as a standard user:
//!
//! - `StartTraceW` with PRIVATE | PRIVATE_IN_PROC | BUFFERING and no
//!   log file succeeds and hands back a handle. The same name a second
//!   time is ERROR_ALREADY_EXISTS; a block with no name is
//!   ERROR_INVALID_NAME; private in-process with neither buffering nor
//!   a log file is ERROR_BAD_PATHNAME -- it has nowhere to put the
//!   events.
//! - A registration made while no session enables the provider gets no
//!   callback and `EventEnabled` answers FALSE. `EnableTraceEx2`
//!   (EVENT_CONTROL_CODE_ENABLE_PROVIDER) calls the callback with
//!   isEnabled 1 and the session's level and keywords, the context the
//!   registration was made with, no filter, and a source id of all
//!   zeros (a private session has no source). Enabling again calls it
//!   again with the new demands.
//! - Enabling a provider that has not registered yet is accepted
//!   silently, and the callback comes at the registration instead:
//!   which is why a DLL that registers late still learns it is traced.
//! - What `EventEnabled` and `EventProviderEnabled` then answer, the
//!   whole rule and the two functions always agreeing:
//!   an event is in reach when its level is 0 or at most the session's,
//!   and its keyword is 0 or overlaps MatchAnyKeyword and (when the
//!   session set one) holds every bit of MatchAllKeyword. A session
//!   that asks for keyword 0 means every keyword, and is reported to
//!   the callback as MatchAnyKeyword ~0 rather than 0.
//! - EVENT_CONTROL_CODE_CAPTURE_STATE calls the callback with
//!   isEnabled 2, level 255 and every keyword -- "write down your
//!   state now" -- and leaves the session's demands as they were.
//! - EVENT_CONTROL_CODE_DISABLE_PROVIDER calls the callback with
//!   isEnabled 0 and zeros, and only that provider stops being
//!   enabled. `EventUnregister` while enabled calls nothing.
//! - `ControlTraceW` queries (the block comes back with the session's
//!   name, the mode it was started with and its buffers), flushes a
//!   session with no file as ERROR_FILE_NOT_FOUND, and stops it: the
//!   stop calls every provider's callback with isEnabled 0 and the
//!   providers go quiet. Stopping twice, or a name no session has, is
//!   ERROR_WMI_INSTANCE_NOT_FOUND. A stopped session's handle still
//!   takes `EnableTraceEx2` without complaint and enables nothing.
//! - Handle 0 is ERROR_INVALID_PARAMETER, a handle no session has is
//!   ERROR_WMI_INSTANCE_NOT_FOUND, and a control code ETW does not
//!   define is ERROR_INVALID_PARAMETER.
//!
//! Prints what it read and `etw_test: ok`, or
//! `etw_test: check N got X (line L)` and exits with N.

#![no_std]
#![no_main]

use core::ffi::c_void;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use ntrt as _;

const NAME: &str = "etw_test";

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
/// for the two names at the offsets the properties give. `ControlTraceW`
/// writes the session's own names back into it, so the room has to be
/// there or it answers ERROR_MORE_DATA.
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

impl Block {
    /// A block describing a session by that name, in that mode, with no
    /// log file.
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

/// The GUIDs this program's two providers are known by, in the mixed
/// endianness a GUID is laid out in.
/// 7f3a1d58-2c64-4b9e-8f05-6a1b3c7d9e20
const PROVIDER_A: [u8; 16] = [
    0x58, 0x1d, 0x3a, 0x7f, 0x64, 0x2c, 0x9e, 0x4b, 0x8f, 0x05, 0x6a, 0x1b, 0x3c, 0x7d, 0x9e, 0x20,
];
/// 6f1b0c47-21d8-4e5a-9c3f-0a7b2d8e6411
const PROVIDER_B: [u8; 16] = [
    0x47, 0x0c, 0x1b, 0x6f, 0xd8, 0x21, 0x5a, 0x4e, 0x9c, 0x3f, 0x0a, 0x7b, 0x2d, 0x8e, 0x64, 0x11,
];

/// What the enable callback was told, and how often it was called. A
/// provider learns a session's demands here and nowhere else, so the
/// test reads them from the same place a real provider would.
static CALLS: AtomicU32 = AtomicU32::new(0);
static LAST_ENABLED: AtomicU32 = AtomicU32::new(0);
static LAST_LEVEL: AtomicU32 = AtomicU32::new(0);
static LAST_ANY: AtomicU64 = AtomicU64::new(0);
static LAST_ALL: AtomicU64 = AtomicU64::new(0);
static LAST_CONTEXT: AtomicU64 = AtomicU64::new(0);
/// 1 when the source id was all zeros, as a private session's is.
static LAST_SOURCE_ZERO: AtomicU32 = AtomicU32::new(0);
/// 1 when no filter came with the demands.
static LAST_FILTER_NULL: AtomicU32 = AtomicU32::new(0);

unsafe extern "system" fn enable_callback(
    source: *const [u8; 16],
    is_enabled: u32,
    level: u8,
    match_any: u64,
    match_all: u64,
    filter: *const c_void,
    context: *mut c_void,
) {
    // SAFETY: ETW passes a source id of its own, or nothing; reading 16
    // bytes through a non-null pointer it gave us is what a provider's
    // callback does.
    let zero = unsafe { source.as_ref() }.is_some_and(|g| {
        let mut all = true;
        for &b in g {
            all &= b == 0;
        }
        all
    });
    LAST_ENABLED.store(is_enabled, Ordering::Relaxed);
    LAST_LEVEL.store(u32::from(level), Ordering::Relaxed);
    LAST_ANY.store(match_any, Ordering::Relaxed);
    LAST_ALL.store(match_all, Ordering::Relaxed);
    LAST_CONTEXT.store(context as u64, Ordering::Relaxed);
    LAST_SOURCE_ZERO.store(u32::from(zero), Ordering::Relaxed);
    LAST_FILTER_NULL.store(u32::from(filter.is_null()), Ordering::Relaxed);
    CALLS.fetch_add(1, Ordering::Relaxed);
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

    /// The callback has been called `calls` times, and the last call
    /// said this. The context is the provider's own, so a provider that
    /// keeps its state there finds it again.
    #[track_caller]
    fn callback(
        &mut self,
        calls: u32,
        enabled: u32,
        level: u8,
        any: u64,
        all: u64,
        context: u64,
    ) -> Result<(), u32> {
        self.eq(i64::from(CALLS.load(Ordering::Relaxed)), i64::from(calls))?;
        self.eq(i64::from(LAST_ENABLED.load(Ordering::Relaxed)), i64::from(enabled))?;
        self.eq(i64::from(LAST_LEVEL.load(Ordering::Relaxed)), i64::from(level))?;
        self.eq(LAST_ANY.load(Ordering::Relaxed) as i64, any as i64)?;
        self.eq(LAST_ALL.load(Ordering::Relaxed) as i64, all as i64)?;
        self.eq(LAST_CONTEXT.load(Ordering::Relaxed) as i64, context as i64)?;
        self.eq(i64::from(LAST_SOURCE_ZERO.load(Ordering::Relaxed)), 1)?;
        self.eq(i64::from(LAST_FILTER_NULL.load(Ordering::Relaxed)), 1)
    }

    /// What the provider answers for each (level, keyword, in reach)
    /// row, printed as it is read. The two functions a provider may ask
    /// with have to agree: one takes the event's descriptor, the other
    /// the level and keyword on their own.
    #[track_caller]
    fn matrix(&mut self, reg: u64, tag: &[u8], rows: &[(u8, u64, u8)]) -> Result<(), u32> {
        let mut line = Line::tag().s(tag).s(b":");
        for &(level, keyword, _) in rows {
            let descriptor = EventDescriptor::new(level, keyword);
            // SAFETY: our registration, our descriptor.
            let (by_event, by_value) = unsafe {
                (EventEnabled(reg, &descriptor), EventProviderEnabled(reg, level, keyword))
            };
            line = line
                .s(b" ")
                .dec(i64::from(level))
                .s(b"/")
                .hex(keyword)
                .s(b"=")
                .dec(i64::from(by_event))
                .dec(i64::from(by_value));
        }
        line.s(b"\r\n").print();
        for &(level, keyword, want) in rows {
            let descriptor = EventDescriptor::new(level, keyword);
            // SAFETY: as above.
            let (by_event, by_value) = unsafe {
                (EventEnabled(reg, &descriptor), EventProviderEnabled(reg, level, keyword))
            };
            self.eq(i64::from(by_event), i64::from(want))?;
            self.eq(i64::from(by_value), i64::from(want))?;
        }
        Ok(())
    }
}

/// Prints what a session's properties came back saying.
fn show(tag: &[u8], block: &Block) {
    let mut line = Line::tag()
        .s(tag)
        .s(b": mode=")
        .hex(u64::from(block.props.log_file_mode))
        .s(b" buffers=")
        .dec(i64::from(block.props.number_of_buffers))
        .s(b" written=")
        .dec(i64::from(block.props.buffers_written))
        .s(b" lost=")
        .dec(i64::from(block.props.events_lost))
        .s(b" name=");
    let at = block.props.logger_name_offset as usize;
    if at == PROPS_SIZE + AREA {
        let mut i = 0;
        let mut text = [0u8; 48];
        while i < text.len() && block.name[i] != 0 {
            text[i] = block.name[i] as u8;
            i += 1;
        }
        line = line.s(&text[..i]);
    }
    line.s(b"\r\n").print();
}

/// Prints the last call the enable callback saw.
fn show_callback(tag: &[u8]) {
    Line::tag()
        .s(tag)
        .s(b": calls=")
        .dec(i64::from(CALLS.load(Ordering::Relaxed)))
        .s(b" enabled=")
        .dec(i64::from(LAST_ENABLED.load(Ordering::Relaxed)))
        .s(b" level=")
        .dec(i64::from(LAST_LEVEL.load(Ordering::Relaxed)))
        .s(b" any=")
        .hex(LAST_ANY.load(Ordering::Relaxed))
        .s(b" all=")
        .hex(LAST_ALL.load(Ordering::Relaxed))
        .s(b" ctx=")
        .hex(LAST_CONTEXT.load(Ordering::Relaxed))
        .s(b"\r\n")
        .print();
}

const ENABLE: u32 = 1; // EVENT_CONTROL_CODE_ENABLE_PROVIDER
const DISABLE: u32 = 0; // EVENT_CONTROL_CODE_DISABLE_PROVIDER
const CAPTURE_STATE: u32 = 2; // EVENT_CONTROL_CODE_CAPTURE_STATE
const QUERY: u32 = 0; // EVENT_TRACE_CONTROL_QUERY
const STOP: u32 = 1; // EVENT_TRACE_CONTROL_STOP
const FLUSH: u32 = 3; // EVENT_TRACE_CONTROL_FLUSH

fn run() -> Result<(), u32> {
    let mut c = Checks { n: 0 };

    // The layout the trace APIs read the block through. A properties
    // block of the wrong size is read as a different structure, so the
    // size is checked before anything is passed.
    c.eq(PROPS_SIZE as i64, 120)?;
    c.eq(core::mem::size_of::<Wnode>() as i64, 48)?;
    c.eq(core::mem::size_of::<EventDescriptor>() as i64, 16)?;

    // A session of this program's own, in memory, with no log file.
    let mut block = Block::new("gala-etw-test", PRIVATE_BUFFERING);
    let mut session = 0u64;
    let name = block.name.as_ptr();
    // SAFETY: our block, which outlives the session; the name inside it.
    let r = unsafe { StartTraceW(&mut session, name, &mut block) };
    c.eq(i64::from(r), 0)?;
    c.ok(session != 0, session as i64)?;
    show(b"started", &block);
    // What a started session leaves in the block: neither name is
    // written back here (a query is what answers those), but the logger
    // name offset is moved to the end of the structure, where there is
    // an empty string, and the name the caller wrote stays where it put
    // it.
    c.eq(i64::from(block.props.logger_name_offset), PROPS_SIZE as i64)?;
    c.eq(i64::from(block.log_file[0]), 0)?;
    c.eq(i64::from(block.name[0]), i64::from(b'g'))?;

    // The same name again, a block with no name, and a private
    // in-process session with neither buffering nor a log file: it has
    // nowhere to put the events.
    let mut again = Block::new("gala-etw-test", PRIVATE_BUFFERING);
    let mut other = 0u64;
    let again_name = again.name.as_ptr();
    // SAFETY: as above.
    let r = unsafe { StartTraceW(&mut other, again_name, &mut again) };
    c.eq(i64::from(r), 183)?; // ERROR_ALREADY_EXISTS
    let mut unnamed = Block::new("", PRIVATE_BUFFERING);
    let unnamed_name = unnamed.name.as_ptr();
    // SAFETY: as above.
    let r = unsafe { StartTraceW(&mut other, unnamed_name, &mut unnamed) };
    c.eq(i64::from(r), 123)?; // ERROR_INVALID_NAME
    // The name is the argument's: a block that carries one and no
    // argument is ERROR_INVALID_NAME too. The name inside a block is
    // where a query writes the session's name back, not where a start
    // reads it.
    let mut in_block = Block::new("gala-etw-in-block", PRIVATE_BUFFERING);
    // SAFETY: our block; no name argument.
    let r = unsafe { StartTraceW(&mut other, core::ptr::null(), &mut in_block) };
    c.eq(i64::from(r), 123)?; // ERROR_INVALID_NAME
    let mut nowhere = Block::new("gala-etw-nowhere", 0x800 | 0x2_0000);
    let nowhere_name = nowhere.name.as_ptr();
    // SAFETY: as above.
    let r = unsafe { StartTraceW(&mut other, nowhere_name, &mut nowhere) };
    c.eq(i64::from(r), 161)?; // ERROR_BAD_PATHNAME

    // A provider registered while nothing enables it: no callback, and
    // nothing is in reach.
    let mut reg_a = 0u64;
    // SAFETY: our GUID, our callback, our handle.
    let r = unsafe {
        EventRegister(&PROVIDER_A, Some(enable_callback), 0x11 as *mut c_void, &mut reg_a)
    };
    c.eq(i64::from(r), 0)?;
    c.ok(reg_a != 0, reg_a as i64)?;
    c.eq(i64::from(CALLS.load(Ordering::Relaxed)), 0)?;
    c.matrix(reg_a, b"no session", &[(0, 0, 0), (1, 0, 0), (5, 0x10, 0)])?;

    // The session asks for level 5 and keywords 0x12. The callback is
    // where the provider learns it, with the context it registered
    // with and a source id of all zeros.
    // SAFETY: our session, our GUID.
    let r =
        unsafe { EnableTraceEx2(session, &PROVIDER_A, ENABLE, 5, 0x12, 0, 0, core::ptr::null()) };
    c.eq(i64::from(r), 0)?;
    show_callback(b"enabled");
    c.callback(1, 1, 5, 0x12, 0, 0x11)?;

    // The whole rule, as both functions answer it: level 0 or at most
    // the session's, keyword 0 or overlapping what it asked for.
    c.matrix(
        reg_a,
        b"level 5 any 0x12",
        &[
            (0, 0, 1),
            (1, 0, 1),
            (5, 0, 1),
            (6, 0, 0),
            (255, 0, 0),
            (5, 0x10, 1),
            (5, 0x40, 0),
            (5, 0x52, 1),
            (6, 0x10, 0),
            (1, 0x40, 0),
            (0, 0x40, 0),
        ],
    )?;

    // An event in reach is written; the session keeps it in memory.
    let descriptor = EventDescriptor::new(5, 0x10);
    // SAFETY: our registration, our descriptor, no data.
    let r = unsafe { EventWrite(reg_a, &descriptor, 0, core::ptr::null()) };
    c.eq(i64::from(r), 0)?;

    // MatchAllKeyword: now every bit of 0x12 has to be there. An event
    // with keyword 0 is still in reach.
    // SAFETY: as above.
    let r = unsafe {
        EnableTraceEx2(session, &PROVIDER_A, ENABLE, 5, 0x12, 0x12, 0, core::ptr::null())
    };
    c.eq(i64::from(r), 0)?;
    show_callback(b"re-enabled");
    c.callback(2, 1, 5, 0x12, 0x12, 0x11)?;
    c.matrix(
        reg_a,
        b"all 0x12",
        &[(5, 0x10, 0), (5, 0x12, 1), (5, 0x52, 1), (0, 0x10, 0), (5, 0, 1)],
    )?;

    // A session that asks for keyword 0 means every keyword, and is
    // reported to the callback as MatchAnyKeyword ~0.
    // SAFETY: as above.
    let r = unsafe { EnableTraceEx2(session, &PROVIDER_A, ENABLE, 4, 0, 0, 0, core::ptr::null()) };
    c.eq(i64::from(r), 0)?;
    show_callback(b"any keyword");
    c.callback(3, 1, 4, u64::MAX, 0, 0x11)?;
    c.matrix(
        reg_a,
        b"level 4 any 0",
        &[(0, 0, 1), (4, 0, 1), (5, 0, 0), (4, 0x40, 1), (5, 0x40, 0), (255, 0x40, 0)],
    )?;

    // A provider the session enables before it registers: accepted
    // silently, and the callback comes at the registration.
    // SAFETY: our session, our GUID.
    let r =
        unsafe { EnableTraceEx2(session, &PROVIDER_B, ENABLE, 3, 0x8, 0, 0, core::ptr::null()) };
    c.eq(i64::from(r), 0)?;
    c.eq(i64::from(CALLS.load(Ordering::Relaxed)), 3)?;
    let mut reg_b = 0u64;
    // SAFETY: as the first registration.
    let r = unsafe {
        EventRegister(&PROVIDER_B, Some(enable_callback), 0x22 as *mut c_void, &mut reg_b)
    };
    c.eq(i64::from(r), 0)?;
    show_callback(b"late registration");
    c.callback(4, 1, 3, 0x8, 0, 0x22)?;
    c.matrix(reg_b, b"provider b", &[(3, 0x8, 1), (4, 0x8, 0), (3, 0x10, 0)])?;

    // "Write down your state now": isEnabled 2, every level and
    // keyword, and the session's own demands are left as they were.
    // SAFETY: as above.
    let r = unsafe {
        EnableTraceEx2(session, &PROVIDER_A, CAPTURE_STATE, 0, 0, 0, 0, core::ptr::null())
    };
    c.eq(i64::from(r), 0)?;
    show_callback(b"capture state");
    c.callback(5, 2, 255, u64::MAX, 0, 0x11)?;
    c.matrix(reg_a, b"after capture", &[(4, 0, 1), (5, 0, 0), (4, 0x40, 1)])?;

    // Disabling one provider leaves the other where it was.
    // SAFETY: as above.
    let r = unsafe { EnableTraceEx2(session, &PROVIDER_A, DISABLE, 0, 0, 0, 0, core::ptr::null()) };
    c.eq(i64::from(r), 0)?;
    show_callback(b"disabled");
    c.callback(6, 0, 0, 0, 0, 0x11)?;
    c.matrix(reg_a, b"disabled a", &[(1, 0, 0), (4, 0, 0)])?;
    c.matrix(reg_b, b"b still on", &[(3, 0x8, 1)])?;

    // Unregistering while a session enables the provider says nothing
    // to the callback.
    // SAFETY: our registration.
    let r = unsafe { EventUnregister(reg_b) };
    c.eq(i64::from(r), 0)?;
    c.eq(i64::from(CALLS.load(Ordering::Relaxed)), 6)?;

    // Handles and codes a session cannot be asked about.
    // SAFETY: calls with a handle no session has, which is what is
    // being measured.
    let r = unsafe { EnableTraceEx2(0, &PROVIDER_A, ENABLE, 5, 0, 0, 0, core::ptr::null()) };
    c.eq(i64::from(r), 87)?; // ERROR_INVALID_PARAMETER
    // SAFETY: as above.
    let r = unsafe { EnableTraceEx2(1, &PROVIDER_A, ENABLE, 5, 0, 0, 0, core::ptr::null()) };
    c.eq(i64::from(r), 4201)?; // ERROR_WMI_INSTANCE_NOT_FOUND
    // SAFETY: as above.
    let r = unsafe { EnableTraceEx2(session, &PROVIDER_A, 7, 5, 0, 0, 0, core::ptr::null()) };
    c.eq(i64::from(r), 87)?;

    // What the session says about itself, and what it does with a flush
    // when it has no file to flush to.
    let mut query = Block::new("gala-etw-test", 0);
    let query_name = query.name.as_ptr();
    // SAFETY: our block, with room for the names ETW writes back.
    let r = unsafe { ControlTraceW(session, query_name, &mut query, QUERY) };
    c.eq(i64::from(r), 0)?;
    show(b"queried", &query);
    c.eq(i64::from(query.props.log_file_mode), i64::from(PRIVATE_BUFFERING))?;
    c.ok(query.props.number_of_buffers > 0, i64::from(query.props.number_of_buffers))?;
    c.eq(i64::from(query.name[0]), i64::from(b'g'))?;
    c.eq(i64::from(query.props.logger_name_offset), (PROPS_SIZE + AREA) as i64)?;
    // A query into a block that gives no offset at all: the name goes
    // right after the structure, and the offset comes back saying where
    // it went.
    let mut bare = Block::new("gala-etw-test", 0);
    bare.props.log_file_name_offset = 0;
    bare.props.logger_name_offset = 0;
    let bare_name = bare.name.as_ptr();
    // SAFETY: as above.
    let r = unsafe { ControlTraceW(session, bare_name, &mut bare, QUERY) };
    c.eq(i64::from(r), 0)?;
    c.eq(i64::from(bare.props.logger_name_offset), PROPS_SIZE as i64)?;
    c.eq(i64::from(bare.log_file[0]), i64::from(b'g'))?;
    let mut flush = Block::new("gala-etw-test", 0);
    let flush_name = flush.name.as_ptr();
    // SAFETY: as above.
    let r = unsafe { ControlTraceW(session, flush_name, &mut flush, FLUSH) };
    c.eq(i64::from(r), 2)?; // ERROR_FILE_NOT_FOUND

    // The stop tells every provider the session is gone.
    // SAFETY: as above.
    let r =
        unsafe { EnableTraceEx2(session, &PROVIDER_A, ENABLE, 5, 0x12, 0, 0, core::ptr::null()) };
    c.eq(i64::from(r), 0)?;
    c.callback(7, 1, 5, 0x12, 0, 0x11)?;
    let mut stop = Block::new("gala-etw-test", 0);
    let stop_name = stop.name.as_ptr();
    // SAFETY: as above.
    let r = unsafe { ControlTraceW(session, stop_name, &mut stop, STOP) };
    c.eq(i64::from(r), 0)?;
    show(b"stopped", &stop);
    show_callback(b"at the stop");
    c.callback(8, 0, 0, 0, 0, 0x11)?;
    c.matrix(reg_a, b"after the stop", &[(1, 0, 0), (5, 0x10, 0)])?;

    // A session that is gone, and a name no session has.
    let mut twice = Block::new("gala-etw-test", 0);
    let twice_name = twice.name.as_ptr();
    // SAFETY: as above.
    let r = unsafe { ControlTraceW(session, twice_name, &mut twice, STOP) };
    c.eq(i64::from(r), 4201)?;
    let mut nobody = Block::new("gala-etw-nobody", 0);
    let nobody_name = nobody.name.as_ptr();
    // SAFETY: as above.
    let r = unsafe { ControlTraceW(0, nobody_name, &mut nobody, STOP) };
    c.eq(i64::from(r), 4201)?;

    // The stopped session's handle is still taken, and enables nothing.
    // SAFETY: as above.
    let r =
        unsafe { EnableTraceEx2(session, &PROVIDER_A, ENABLE, 5, 0x12, 0, 0, core::ptr::null()) };
    c.eq(i64::from(r), 0)?;
    c.matrix(reg_a, b"enabled after stop", &[(5, 0x10, 0)])?;

    // SAFETY: our registration.
    let r = unsafe { EventUnregister(reg_a) };
    c.eq(i64::from(r), 0)?;
    Ok(())
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
