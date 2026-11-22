//! Event Tracing for Windows, the session side (`StartTraceW`,
//! `ControlTraceW`, `EnableTraceEx2`): what a program's own trace
//! session asks of a provider, and which of its events are then in
//! reach. The provider side is `etw`; this is the half that decides
//! whether a program assembles an event at all, because TraceLogging
//! builds no payload for a provider no session enabled.
//!
//! What gala has is the private in-process session: a session of the
//! calling process's own, keeping its events in memory. It is the one
//! kind a program can start without administrator rights, and the only
//! one that needs no file. A session that asks for a log file is told
//! gala cannot write one, and a system-wide session is told what a
//! standard user is told, rather than being handed a session that
//! quietly drops everything written to it.
//!
//! The enablement contract is Windows' own, measured on the pinned
//! build as a standard user (`user/etw_test` checks every answer on
//! both systems):
//!
//! - A provider's demands are the union of every running session's: the
//!   highest level and the keywords of all of them together. An event
//!   is in reach when its level is 0 or at most that level, and its
//!   keyword is 0 or overlaps the keywords asked for and holds every
//!   bit of MatchAllKeyword. A session that asks for keyword 0 means
//!   every keyword and is reported as MatchAnyKeyword ~0.
//! - The provider learns its demands in its enable callback and nowhere
//!   else: at the enable, at a registration made while a session
//!   already enables it, at a capture-state request, at the disable and
//!   at the session's stop. The callback gets the context the
//!   registration was made with and, for a private session, a source id
//!   of all zeros.
//!
//! Two answers here are gala's reading where Windows is not measured,
//! and are named in ARCHITECTURE "advapi32": what MatchAllKeyword means
//! when several sessions ask for one, and how much room `ControlTraceW`
//! wants in the block it writes the session's names back into (gala
//! asks only that they fit; Windows wants more than that).

use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

const ERROR_FILE_NOT_FOUND: u32 = 2;
const ERROR_ACCESS_DENIED: u32 = 5;
const ERROR_NOT_ENOUGH_MEMORY: u32 = 8;
const ERROR_NOT_SUPPORTED: u32 = 50;
const ERROR_INVALID_PARAMETER: u32 = 87;
const ERROR_INVALID_NAME: u32 = 123;
const ERROR_BAD_PATHNAME: u32 = 161;
const ERROR_ALREADY_EXISTS: u32 = 183;
const ERROR_MORE_DATA: u32 = 234;
/// What a trace handle or name no session has answers.
const ERROR_WMI_INSTANCE_NOT_FOUND: u32 = 4201;

/// EVENT_TRACE_PRIVATE_LOGGER_MODE: a session of one process's own.
const PRIVATE: u32 = 0x800;
/// EVENT_TRACE_PRIVATE_IN_PROC: and in that process's own memory.
const IN_PROC: u32 = 0x2_0000;
/// EVENT_TRACE_BUFFERING_MODE: the buffers are the session; nothing is
/// written out, so there is no file to name.
const BUFFERING: u32 = 0x400;

/// EVENT_CONTROL_CODE_DISABLE_PROVIDER / ENABLE_PROVIDER /
/// CAPTURE_STATE.
const DISABLE: u32 = 0;
const ENABLE: u32 = 1;
const CAPTURE_STATE: u32 = 2;

/// EVENT_TRACE_CONTROL_QUERY / STOP / FLUSH.
const QUERY: u32 = 0;
const STOP: u32 = 1;
const FLUSH: u32 = 3;

#[cfg_attr(
    target_arch = "x86",
    link(name = "kernel32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "kernel32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn GetProcessHeap() -> usize;
    fn HeapAlloc(heap: usize, flags: u32, size: usize) -> *mut u8;
    fn HeapFree(heap: usize, flags: u32, block: *mut u8) -> i32;
}

/// WNODE_HEADER, the WMI header a properties block starts with.
#[repr(C)]
pub struct Wnode {
    buffer_size: u32,
    provider_id: u32,
    historical_context: u64,
    timestamp: i64,
    guid: [u8; 16],
    client_context: u32,
    flags: u32,
}

/// EVENT_TRACE_PROPERTIES: what a program asks a session for, and what
/// a query answers. A V2 block starts with the same fields; nothing
/// here reads past them.
#[repr(C)]
pub struct Properties {
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

/// What a session asks of a provider: the level and keywords an event
/// has to answer to be in reach.
#[derive(Clone, Copy)]
pub(crate) struct Demands {
    pub(crate) level: u8,
    pub(crate) any: u64,
    pub(crate) all: u64,
}

impl Demands {
    /// What a provider is told when the last session stops asking for
    /// it: nothing at all.
    pub(crate) const NONE: Self = Self { level: 0, any: 0, all: 0 };

    /// "Write down your state now": every level and every keyword, and
    /// not a change to what the session asked for.
    const EVERYTHING: Self = Self { level: 0xff, any: u64::MAX, all: 0 };

    /// Whether an event with this level and keyword is in reach. The
    /// whole rule, as Windows answers it: level 0 is past the level
    /// test, keyword 0 past the keyword test, and MatchAllKeyword is
    /// only a demand when the session made one.
    pub(crate) fn admits(&self, level: u8, keyword: u64) -> bool {
        let by_level = level == 0 || level <= self.level;
        let by_keyword = keyword == 0
            || (keyword & self.any != 0 && (self.all == 0 || keyword & self.all == self.all));
        by_level && by_keyword
    }

    /// What two sessions asking for the same provider come to. The
    /// level and the keywords are what Windows reports and tests
    /// against (measured with two sessions); MatchAllKeyword is gala's
    /// reading -- a session that demands no particular bits frees the
    /// provider of the other's demand, because its events are in reach
    /// either way.
    fn with(self, other: Self) -> Self {
        Self {
            level: self.level.max(other.level),
            any: self.any | other.any,
            all: if self.all == 0 || other.all == 0 { 0 } else { self.all | other.all },
        }
    }
}

/// How much of a session's name is kept. Windows' limit is 1024
/// characters; a longer name keeps its first `NAME_MAX`, which is what
/// the session is then known by.
const NAME_MAX: usize = 128;

/// One `StartTraceW`. A block is never freed: a stopped session's
/// handle stays known, because Windows takes `EnableTraceEx2` on it
/// without complaint (and enables nothing), and the next session takes
/// a block whose own is over.
struct Session {
    /// The next block, running or not: the list is only ever added to.
    next: *mut Session,
    /// Whether the session is running.
    live: bool,
    /// Whether this block has ever been a session: a fresh one has no
    /// handle yet, and no handle of 0 is ever looked for.
    used: bool,
    handle: u64,
    name: [u16; NAME_MAX],
    name_len: usize,
    mode: u32,
    /// The events the session is keeping, and where the next one goes.
    buffer: *mut u8,
    capacity: usize,
    at: usize,
    /// How many buffers the capacity is, as a query reports them.
    buffers: u32,
    /// Events that did not fit, which is what EventsLost counts.
    lost: u32,
}

/// One provider one session asked for.
struct Enable {
    next: *mut Enable,
    live: bool,
    session: u64,
    guid: [u8; 16],
    demands: Demands,
}

/// The newest session block, and the newest enable (0: none yet).
static SESSIONS: AtomicUsize = AtomicUsize::new(0);
static ENABLES: AtomicUsize = AtomicUsize::new(0);
/// Held across a walk of either list and a block's own fields. Never
/// held while a provider's callback runs: the callback is the program's
/// own code, and it may register a provider of its own.
static LOCK: AtomicBool = AtomicBool::new(false);
/// The next TRACEHANDLE. Never 0: 0 is what a failed start leaves, and
/// what `EnableTraceEx2` answers ERROR_INVALID_PARAMETER for.
static NEXT_HANDLE: AtomicU64 = AtomicU64::new(0x0100_0001);

/// `f` with both lists held.
fn with_list<R>(f: impl FnOnce() -> R) -> R {
    while LOCK.swap(true, Ordering::Acquire) {
        core::hint::spin_loop();
    }
    let r = f();
    LOCK.store(false, Ordering::Release);
    r
}

/// The demands on `guid`: the union of every running session's, or
/// `None` when no session asks for it. This is the question a provider
/// answers `EventEnabled` from.
pub(crate) fn demands(guid: &[u8; 16]) -> Option<Demands> {
    with_list(|| union_of(guid))
}

/// The union of the live enables for `guid`. The lists are held.
fn union_of(guid: &[u8; 16]) -> Option<Demands> {
    let mut union: Option<Demands> = None;
    let mut at = ENABLES.load(Ordering::Relaxed);
    while at != 0 {
        // SAFETY: a block this DLL allocated and never frees.
        let e = unsafe { &mut *(at as *mut Enable) };
        if e.live && e.guid == *guid {
            union = Some(match union {
                Some(sofar) => sofar.with(e.demands),
                None => e.demands,
            });
        }
        at = e.next as usize;
    }
    union
}

/// What one session asks of `guid`, if it asks at all. The lists are
/// held.
fn asked_by(session: u64, guid: &[u8; 16]) -> Option<Demands> {
    let mut at = ENABLES.load(Ordering::Relaxed);
    while at != 0 {
        // SAFETY: ours, never freed.
        let e = unsafe { &mut *(at as *mut Enable) };
        if e.live && e.session == session && e.guid == *guid {
            return Some(e.demands);
        }
        at = e.next as usize;
    }
    None
}

/// The enable `session` has on `guid`, made or changed, and the union
/// that leaves. `None` when there is no memory for a new one. The
/// lists are held.
fn set_enable(session: u64, guid: &[u8; 16], demands: Demands) -> Option<Demands> {
    let mut free = 0usize;
    let mut at = ENABLES.load(Ordering::Relaxed);
    while at != 0 {
        // SAFETY: ours, never freed.
        let e = unsafe { &mut *(at as *mut Enable) };
        if e.live && e.session == session && e.guid == *guid {
            e.demands = demands;
            return union_of(guid);
        }
        if !e.live {
            free = at;
        }
        at = e.next as usize;
    }
    let block = if free != 0 {
        free
    } else {
        // SAFETY: plain allocation, zeroed (HEAP_ZERO_MEMORY).
        let fresh = unsafe { HeapAlloc(GetProcessHeap(), 8, size_of::<Enable>()) };
        if fresh.is_null() {
            return None;
        }
        let fresh = fresh.cast::<Enable>();
        // SAFETY: a zeroed block of exactly this size.
        unsafe { (*fresh).next = ENABLES.load(Ordering::Relaxed) as *mut Enable };
        ENABLES.store(fresh as usize, Ordering::Relaxed);
        fresh as usize
    };
    // SAFETY: ours.
    let e = unsafe { &mut *(block as *mut Enable) };
    e.live = true;
    e.session = session;
    e.guid = *guid;
    e.demands = demands;
    union_of(guid)
}

/// `session` stops asking for `guid`; what the sessions that are left
/// still ask for, if any. The lists are held.
fn drop_enable(session: u64, guid: &[u8; 16]) -> Option<Demands> {
    let mut at = ENABLES.load(Ordering::Relaxed);
    while at != 0 {
        // SAFETY: ours, never freed.
        let e = unsafe { &mut *(at as *mut Enable) };
        if e.live && e.session == session && e.guid == *guid {
            e.live = false;
        }
        at = e.next as usize;
    }
    union_of(guid)
}

/// `f` with the running session `handle` names (or, with `handle` 0,
/// the one called `name`), and whether a block with that handle is a
/// session that has stopped. The pair is what the two answers are made
/// of: `EnableTraceEx2` takes a stopped session's handle and enables
/// nothing, while `ControlTraceW` answers it the same as a handle no
/// session ever had.
fn with_session<R>(
    handle: u64,
    name: &[u16],
    f: impl FnOnce(&mut Session) -> R,
) -> (Option<R>, bool) {
    with_list(|| {
        let mut stopped = false;
        let mut at = SESSIONS.load(Ordering::Relaxed);
        while at != 0 {
            // SAFETY: ours, never freed.
            let s = unsafe { &mut *(at as *mut Session) };
            let named = handle == 0
                && !name.is_empty()
                && s.name_len == name.len()
                && s.name[..s.name_len] == *name;
            if s.used && ((handle != 0 && s.handle == handle) || named) {
                if s.live {
                    return (Some(f(s)), false);
                }
                stopped = true;
            }
            at = s.next as usize;
        }
        (None, stopped)
    })
}

/// A block for a new session: one whose session is over, else a fresh
/// one on the list. 0 when there is no memory for it. The lists are
/// held.
fn take_block() -> usize {
    let mut at = SESSIONS.load(Ordering::Relaxed);
    while at != 0 {
        // SAFETY: ours, never freed.
        let s = unsafe { &mut *(at as *mut Session) };
        if !s.live {
            return at;
        }
        at = s.next as usize;
    }
    // SAFETY: plain allocation, zeroed (HEAP_ZERO_MEMORY).
    let fresh = unsafe { HeapAlloc(GetProcessHeap(), 8, size_of::<Session>()) };
    if fresh.is_null() {
        return 0;
    }
    let fresh = fresh.cast::<Session>();
    // SAFETY: a zeroed block of exactly this size.
    unsafe { (*fresh).next = SESSIONS.load(Ordering::Relaxed) as *mut Session };
    SESSIONS.store(fresh as usize, Ordering::Relaxed);
    fresh as usize
}

/// The UTF-16 string at `at` in the caller's block, bounded by
/// `NAME_MAX` and by the size the block gave for itself. An offset
/// inside the structure itself names no string.
fn name_at(block: *const u8, size: usize, at: usize, into: &mut [u16; NAME_MAX]) -> usize {
    if at < size_of::<Properties>() || at >= size {
        return 0;
    }
    let mut n = 0;
    while n < NAME_MAX && at + (n + 1) * 2 <= size {
        // SAFETY: inside the block's own size, as the caller gave it.
        let c = unsafe { (block.add(at) as *const u16).add(n).read_unaligned() };
        if c == 0 {
            break;
        }
        into[n] = c;
        n += 1;
    }
    n
}

/// A UTF-16 string the caller passed as an argument, to its NUL.
fn string(text: *const u16, into: &mut [u16; NAME_MAX]) -> usize {
    if text.is_null() {
        return 0;
    }
    let mut n = 0;
    while n < NAME_MAX {
        // SAFETY: the caller's string, read to its NUL.
        let c = unsafe { *text.add(n) };
        if c == 0 {
            break;
        }
        into[n] = c;
        n += 1;
    }
    n
}

/// The buffers a session keeps, out of what it asked for: Windows
/// decides the count itself (it answered 24 for a block asking for 2),
/// and so does gala -- what a query reports is what the session holds.
const BUFFER_MIN: u32 = 2;
const BUFFER_MAX: u32 = 64;
const BYTES_MIN: u32 = 4 * 1024;
const BYTES_MAX: u32 = 1024 * 1024;

/// StartTraceW: a session of this process's own.
#[unsafe(no_mangle)]
pub extern "system" fn StartTraceW(
    handle: *mut u64,
    name: *const u16,
    properties: *mut Properties,
) -> u32 {
    if handle.is_null() || properties.is_null() {
        return ERROR_INVALID_PARAMETER;
    }
    let block = properties.cast::<u8>();
    // SAFETY: the caller's block, read field by field through the
    // structure the trace APIs document for it.
    let (size, mode, asked_buffers, asked_bytes, log_offset) = unsafe {
        (
            (*properties).wnode.buffer_size as usize,
            (*properties).log_file_mode,
            (*properties).minimum_buffers,
            (*properties).buffer_size,
            (*properties).log_file_name_offset as usize,
        )
    };
    if size < size_of::<Properties>() {
        return ERROR_INVALID_PARAMETER;
    }

    // The session's name is the argument's. A block that carries one
    // and no argument is ERROR_INVALID_NAME (measured): the name inside
    // the block is where a query writes the session's name back, not
    // where StartTraceW reads it.
    let mut session_name = [0u16; NAME_MAX];
    let name_len = string(name, &mut session_name);
    if name_len == 0 {
        return ERROR_INVALID_NAME;
    }

    // What kind of session it is. gala has the private in-process one;
    // anything else is told so rather than handed a session that drops
    // what is written to it.
    let mut log_file = [0u16; NAME_MAX];
    let has_file = name_at(block, size, log_offset, &mut log_file) != 0;
    if mode & (PRIVATE | IN_PROC) != PRIVATE | IN_PROC {
        crate::missing(b"advapi32.dll!StartTraceW(system-wide)");
        return ERROR_ACCESS_DENIED;
    }
    if has_file {
        crate::missing(b"advapi32.dll!StartTraceW(log file)");
        return ERROR_NOT_SUPPORTED;
    }
    if mode & BUFFERING == 0 {
        // Nowhere to put the events: no file, and not buffering.
        return ERROR_BAD_PATHNAME;
    }

    let buffers = asked_buffers.clamp(BUFFER_MIN, BUFFER_MAX);
    let bytes = asked_bytes.saturating_mul(1024).clamp(BYTES_MIN, BYTES_MAX);
    let capacity = buffers as usize * bytes as usize;
    let started = with_list(|| {
        // A name a running session already has.
        let mut at = SESSIONS.load(Ordering::Relaxed);
        while at != 0 {
            // SAFETY: ours, never freed.
            let s = unsafe { &mut *(at as *mut Session) };
            if s.live && s.name_len == name_len && s.name[..name_len] == session_name[..name_len] {
                return Err(ERROR_ALREADY_EXISTS);
            }
            at = s.next as usize;
        }
        let block = take_block();
        if block == 0 {
            return Err(ERROR_NOT_ENOUGH_MEMORY);
        }
        // SAFETY: plain allocation; the session's own buffers.
        let buffer = unsafe { HeapAlloc(GetProcessHeap(), 8, capacity) };
        if buffer.is_null() {
            return Err(ERROR_NOT_ENOUGH_MEMORY);
        }
        // SAFETY: ours.
        let s = unsafe { &mut *(block as *mut Session) };
        s.live = true;
        s.used = true;
        s.handle = NEXT_HANDLE.fetch_add(1, Ordering::Relaxed);
        s.name = session_name;
        s.name_len = name_len;
        s.mode = mode;
        s.buffer = buffer;
        s.capacity = capacity;
        s.at = 0;
        s.buffers = buffers;
        s.lost = 0;
        Ok(s.handle)
    });
    let started_handle = match started {
        Ok(handle) => handle,
        Err(ERROR_NOT_ENOUGH_MEMORY) => {
            crate::missing(b"advapi32.dll!StartTraceW(no memory)");
            return ERROR_NOT_ENOUGH_MEMORY;
        }
        Err(error) => return error,
    };
    // What a started session leaves in the block. Windows writes
    // neither name back here -- a query is what answers those -- but it
    // does point LoggerNameOffset at the end of the structure, where it
    // leaves an empty string, and a private session has no logger
    // thread (all measured).
    // SAFETY: the caller's block and output.
    unsafe {
        (*properties).number_of_buffers = buffers;
        (*properties).free_buffers = buffers;
        (*properties).events_lost = 0;
        (*properties).buffers_written = 0;
        (*properties).logger_name_offset = size_of::<Properties>() as u32;
        *handle = started_handle;
    }
    note_session(&session_name[..name_len]);
    0
}

/// `advapi32.dll!StartTraceW(<name>)`: the session a program started,
/// by the name a reader finds it under. A program that traces itself
/// names the session after what it is about to watch.
fn note_session(name: &[u16]) {
    const PREFIX: &[u8] = b"advapi32.dll!StartTraceW(";
    /// As much of a name as a row keeps.
    const ROOM: usize = 64;
    let mut row = [0u8; PREFIX.len() + ROOM + 1];
    row[..PREFIX.len()].copy_from_slice(PREFIX);
    let mut at = PREFIX.len();
    for &c in name {
        if at >= PREFIX.len() + ROOM {
            break;
        }
        // A session's name is ASCII in practice; anything else keeps
        // its place.
        row[at] = if c < 0x80 && (c as u8).is_ascii_graphic() { c as u8 } else { b'?' };
        at += 1;
    }
    row[at] = b')';
    crate::reached(&row[..at + 1], 0);
}

/// EnableTraceEx2: what a session asks of a provider. The provider is
/// told in its callback -- and one that has not registered yet is not
/// missed: `EventRegister` asks for the demands itself.
#[unsafe(no_mangle)]
pub extern "system" fn EnableTraceEx2(
    session: u64,
    provider: *const [u8; 16],
    control: u32,
    level: u8,
    any: u64,
    all: u64,
    _timeout: u32,
    filter: *const core::ffi::c_void,
) -> u32 {
    if session == 0 || provider.is_null() || control > CAPTURE_STATE {
        return ERROR_INVALID_PARAMETER;
    }
    // SAFETY: the caller's provider id.
    let guid = unsafe { *provider };
    if !filter.is_null() {
        // A session's scope filters (EVENT_FILTER_DESCRIPTOR): the
        // events still reach the session, unfiltered.
        crate::missing(b"advapi32.dll!EnableTraceEx2(filter)");
    }
    let (found, stopped) = with_session(session, &[], |s| s.handle);
    if found.is_none() {
        // A stopped session's handle is taken and enables nothing;
        // anything else is a handle no session has.
        return if stopped { 0 } else { ERROR_WMI_INSTANCE_NOT_FOUND };
    }
    crate::etw::note_provider(b"advapi32.dll!EnableTraceEx2(", &guid);
    match control {
        CAPTURE_STATE => {
            crate::etw::tell(&guid, CAPTURE_STATE, Demands::EVERYTHING);
            0
        }
        DISABLE => {
            match with_list(|| drop_enable(session, &guid)) {
                // What the sessions that are left still ask for.
                Some(left) => crate::etw::tell(&guid, ENABLE, left),
                None => crate::etw::tell(&guid, DISABLE, Demands::NONE),
            }
            0
        }
        // A session that asks for keyword 0 asks for every keyword, and
        // that is what the provider is told (measured).
        _ => {
            let asked = Demands { level, any: if any == 0 { u64::MAX } else { any }, all };
            match with_list(|| set_enable(session, &guid, asked)) {
                Some(union) => {
                    crate::etw::tell(&guid, ENABLE, union);
                    0
                }
                None => {
                    crate::missing(b"advapi32.dll!EnableTraceEx2(no memory)");
                    ERROR_NOT_ENOUGH_MEMORY
                }
            }
        }
    }
}

/// ControlTraceW: what a session says about itself, and its end.
#[unsafe(no_mangle)]
pub extern "system" fn ControlTraceW(
    session: u64,
    name: *const u16,
    properties: *mut Properties,
    code: u32,
) -> u32 {
    if properties.is_null() {
        return ERROR_INVALID_PARAMETER;
    }
    let block = properties.cast::<u8>();
    // SAFETY: the caller's block.
    let (size, name_offset) = unsafe {
        ((*properties).wnode.buffer_size as usize, (*properties).logger_name_offset as usize)
    };
    if size < size_of::<Properties>() {
        return ERROR_INVALID_PARAMETER;
    }
    if code != QUERY && code != STOP && code != FLUSH {
        // EVENT_TRACE_CONTROL_UPDATE and whatever else a program asks
        // for: the session is left as it is.
        crate::missing_detail(b"advapi32.dll!ControlTraceW(code)", u64::from(code));
        return ERROR_NOT_SUPPORTED;
    }
    // Where the session's name goes back: the offset the block gives,
    // or the end of the structure when it gives none.
    let at =
        if name_offset >= size_of::<Properties>() { name_offset } else { size_of::<Properties>() };
    // The session by handle, or by name when the program has none.
    let mut wanted = [0u16; NAME_MAX];
    let wanted_len = if session == 0 { string(name, &mut wanted) } else { 0 };
    let (found, _) = with_session(session, &wanted[..wanted_len], |s| {
        // A flush has nowhere to flush to: the buffers are the session.
        if code == FLUSH {
            return Err(ERROR_FILE_NOT_FOUND);
        }
        // The name the session is known by goes back into the block,
        // which has to have room for it where it said.
        if at + (s.name_len + 1) * 2 > size {
            return Err(ERROR_MORE_DATA);
        }
        // SAFETY: the caller's block, inside the size it gave, past the
        // structure's own fields.
        unsafe {
            for i in 0..s.name_len {
                (block.add(at) as *mut u16).add(i).write_unaligned(s.name[i]);
            }
            (block.add(at) as *mut u16).add(s.name_len).write_unaligned(0);
            // Where the name went, which is how a caller that gave no
            // offset finds it (measured).
            (*properties).logger_name_offset = at as u32;
            (*properties).log_file_mode = s.mode;
            (*properties).number_of_buffers = s.buffers;
            (*properties).free_buffers = s.buffers;
            (*properties).events_lost = s.lost;
            // Nothing is ever written out: a buffering session's events
            // are its buffers.
            (*properties).buffers_written = 0;
        }
        if code == STOP {
            s.live = false;
            let buffer = core::mem::replace(&mut s.buffer, core::ptr::null_mut());
            s.capacity = 0;
            s.at = 0;
            // SAFETY: this session's own buffers, which nothing reaches
            // once it is not live.
            unsafe { HeapFree(GetProcessHeap(), 0, buffer) };
            return Ok(Some(s.handle));
        }
        Ok(None)
    });
    let stopped = match found {
        None => return ERROR_WMI_INSTANCE_NOT_FOUND,
        Some(Err(error)) => return error,
        Some(Ok(stopped)) => stopped,
    };
    match stopped {
        // The stop tells every provider the session asked for that it
        // is gone, outside the lists' lock.
        Some(handle) => {
            stop_telling(handle);
            crate::reached(b"advapi32.dll!ControlTraceW(stopped)", 0);
        }
        None => crate::reached(b"advapi32.dll!ControlTraceW(query)", 0),
    }
    0
}

/// Every provider a stopped session asked for, told it is gone -- or
/// told what the sessions that are left still ask for.
fn stop_telling(handle: u64) {
    let mut at = ENABLES.load(Ordering::Relaxed);
    while at != 0 {
        let (next, gone) = with_list(|| {
            // SAFETY: ours, never freed.
            let e = unsafe { &mut *(at as *mut Enable) };
            let mine = e.live && e.session == handle;
            if mine {
                e.live = false;
            }
            (e.next as usize, if mine { Some((e.guid, union_of(&e.guid))) } else { None })
        });
        if let Some((guid, left)) = gone {
            match left {
                Some(demands) => crate::etw::tell(&guid, ENABLE, demands),
                None => crate::etw::tell(&guid, DISABLE, Demands::NONE),
            }
        }
        at = next;
    }
}

/// EVENT_DATA_DESCRIPTOR: one piece of an event's payload.
#[repr(C)]
struct DataDescriptor {
    data: u64,
    size: u32,
    kind: u32,
}

/// How long a record a session keeps: the header, the descriptor and
/// the payload. An event longer than this is counted lost rather than
/// cut in half, because half a TraceLogging event decodes as nothing.
const RECORD_MAX: usize = 4096;

/// What a kept event costs besides its payload: its length, the
/// provider, the descriptor and the payload's length.
const RECORD_HEADER: usize = 4 + 16 + 16 + 4;

/// An event in reach, kept by every running session that asked for it:
/// the provider, the descriptor and the payload's bytes, which is what
/// a reader of the buffers needs to decode it. Nothing reads them yet
/// (ROADMAP OF step 1 has the decoder); what they give now is the count
/// a query reports, and a session that holds a program's own account of
/// its work.
pub(crate) fn keep(guid: &[u8; 16], descriptor: *const u8, count: u32, data: *const u8) {
    let pieces = if data.is_null() { 0 } else { count as usize };
    let mut payload = 0usize;
    for i in 0..pieces {
        // SAFETY: the caller's array of `count` descriptors.
        let d = unsafe { &*(data as *const DataDescriptor).add(i) };
        payload = payload.saturating_add(d.size as usize);
    }
    let length = RECORD_HEADER + payload;
    with_list(|| {
        let mut at = SESSIONS.load(Ordering::Relaxed);
        while at != 0 {
            // SAFETY: ours, never freed.
            let s = unsafe { &mut *(at as *mut Session) };
            at = s.next as usize;
            if !s.live || asked_by(s.handle, guid).is_none() {
                continue;
            }
            if length > RECORD_MAX || s.at + length > s.capacity {
                s.lost = s.lost.saturating_add(1);
                continue;
            }
            // SAFETY: this session's buffer, with room for `length`
            // bytes at `s.at` as checked just above, and the caller's
            // descriptor and payload pieces.
            unsafe {
                let mut to = s.buffer.add(s.at);
                to.cast::<u32>().write_unaligned(length as u32);
                to = to.add(4);
                to.cast::<[u8; 16]>().write_unaligned(*guid);
                to = to.add(16);
                let mut event = [0u8; 16];
                if !descriptor.is_null() {
                    core::ptr::copy_nonoverlapping(descriptor, event.as_mut_ptr(), 16);
                }
                to.cast::<[u8; 16]>().write_unaligned(event);
                to = to.add(16);
                to.cast::<u32>().write_unaligned(payload as u32);
                to = to.add(4);
                for i in 0..pieces {
                    let d = &*(data as *const DataDescriptor).add(i);
                    if d.data != 0 && d.size != 0 {
                        core::ptr::copy_nonoverlapping(d.data as *const u8, to, d.size as usize);
                        to = to.add(d.size as usize);
                    }
                }
            }
            s.at += length;
        }
    });
}
