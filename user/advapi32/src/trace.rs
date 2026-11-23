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
//!
//! Nothing a program's write path reaches takes a lock: not the walk
//! that weighs an event against what the sessions ask, not the keeping
//! of it, and not the system call that hands it to the kernel. The
//! lists are only ever added to, a block is never freed, the fields a
//! write reads are either written before the block is published or
//! atomic, and a session's room is reserved with a compare-exchange.
//! Office suspends its own threads to walk their stacks while the
//! process traces, so a lock on this path is one a stopped thread can
//! be holding, and every other writer would then spin for good -- which
//! is where Word's main thread sat (`user/etwlock_test`, ROADMAP OF).
//! What is left under a lock is the changing of the lists themselves,
//! which no write waits for, and that lock reports what it waits for
//! (`gala_lock`).

use core::sync::atomic::{
    AtomicBool, AtomicPtr, AtomicU8, AtomicU32, AtomicU64, AtomicUsize, Ordering,
};

use gala_lock::Lock;

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

/// An enable's `Demands` as a write may find them: `EnableTraceEx2`
/// rewrites them under the list lock while a write is weighing an event
/// against them, and a write takes no lock. The three parts are loaded
/// one at a time, so a write that lands exactly on a change can weigh
/// its event against a level from one side of it and keywords from the
/// other. That is the same answer Windows gives such a write: the
/// change reaches a provider through its callback, and an event already
/// on its way is weighed against whichever side of it the thread saw.
struct AtomicDemands {
    level: AtomicU8,
    any: AtomicU64,
    all: AtomicU64,
}

impl AtomicDemands {
    fn load(&self) -> Demands {
        Demands {
            level: self.level.load(Ordering::Relaxed),
            any: self.any.load(Ordering::Relaxed),
            all: self.all.load(Ordering::Relaxed),
        }
    }

    fn store(&self, demands: Demands) {
        self.level.store(demands.level, Ordering::Relaxed);
        self.any.store(demands.any, Ordering::Relaxed);
        self.all.store(demands.all, Ordering::Relaxed);
    }
}

/// How much of a session's name is kept. Windows' limit is 1024
/// characters; a longer name keeps its first `NAME_MAX`, which is what
/// the session is then known by.
const NAME_MAX: usize = 128;

/// A session's events: how much room they have, where the next one
/// goes, and how many did not fit. One of these per `StartTraceW`,
/// allocated with `capacity` bytes after it, and never freed -- a write
/// that holds no lock may be copying into it at the moment the session
/// is stopped, and nothing short of waiting for that write says
/// otherwise, which is the wait this whole path exists to remove. A
/// stopped session's room therefore stays the process's, which is what
/// `BUFFERS_MAX` bounds.
#[repr(C)]
struct Buf {
    /// How many bytes of events fit: the buffer count times the buffer
    /// size the session asked for.
    capacity: usize,
    /// Where the next event goes. A write reserves its room here with a
    /// compare-exchange and then copies into what it reserved, so two
    /// threads writing at once get two ranges and neither waits for the
    /// other -- nor for one that is suspended.
    at: AtomicUsize,
    /// Events that did not fit, which is what EventsLost counts.
    lost: AtomicU32,
}

impl Buf {
    /// The events themselves, which the allocation holds after the
    /// header: `capacity` bytes of them.
    fn bytes(&self) -> *mut u8 {
        // SAFETY: this block was allocated with `capacity` bytes after
        // the header, and `size_of::<Buf>()` is where they start.
        unsafe { (self as *const Buf as *mut u8).add(size_of::<Buf>()) }
    }
}

/// How much of a process's memory may be in trace buffers over its
/// whole run. A buffer is never freed (`Buf`), so this is what keeps a
/// program that starts and stops sessions in a loop from spending the
/// process on them; one maximal session is 64 MiB of it. A start past
/// it is told so and answered ERROR_NOT_ENOUGH_MEMORY, as a start whose
/// allocation failed is.
const BUFFERS_MAX: usize = 256 * 1024 * 1024;

/// How much of `BUFFERS_MAX` is spent.
static BUFFERS_SPENT: AtomicUsize = AtomicUsize::new(0);

/// One `StartTraceW`. A block is never freed: a stopped session's
/// handle stays known, because Windows takes `EnableTraceEx2` on it
/// without complaint (and enables nothing), and the next session takes
/// a block whose own is over.
///
/// A write reads `live`, `handle`, `to_kernel` and `buf` and no more,
/// all atomic and all published by the store of `live` that ends a
/// start. Everything else is read and written only with `LOCK` held,
/// which is why those fields are reached through a raw pointer rather
/// than a reference: a write holds a `&Session` to the same block at
/// the same time, and a `&mut` beside it would be a lie.
struct Session {
    /// The next block, running or not: the list is only ever added to,
    /// and a block is published after this is written.
    next: *mut Session,
    /// Whether the session is running. Stored last and with Release by
    /// a start, loaded first and with Acquire by a write.
    live: AtomicBool,
    /// Whether this block has ever been a session: a fresh one has no
    /// handle yet, and no handle of 0 is ever looked for.
    used: AtomicBool,
    handle: AtomicU64,
    name: [u16; NAME_MAX],
    name_len: usize,
    mode: u32,
    /// The events the session is keeping: null for a session that is
    /// over, and for the one gala started, which keeps none.
    buf: AtomicPtr<Buf>,
    /// How many buffers the capacity is, as a query reports them.
    buffers: u32,
    /// The session gala started for this program (`etw=`, nt::etw): it
    /// has no buffers, and every event in its reach goes to the kernel
    /// as it is written. The program neither started it nor has its
    /// handle, so it cannot query or stop it; it is a session here so
    /// that a provider learns of it exactly as of any other.
    to_kernel: AtomicBool,
    /// On that session: every provider the program registers, whatever
    /// its id (`etw=*`).
    every_provider: bool,
    /// On that session: the provider it asks for, when it does not ask
    /// for every one.
    ask_guid: [u8; 16],
    /// On that session: what it asks of that provider.
    ask: Demands,
}

/// One provider one session asked for.
///
/// Every field a write reads is atomic but `guid`, which is written
/// before the block is published and never again: a dead block is taken
/// only by an enable of the same provider (`set_enable`). That is what
/// lets a write key on it without a lock -- the sixteen bytes are this
/// block's for good, so there is no half-written id to read. The memory
/// is therefore the most sessions that ever enabled one provider at
/// once, per provider, rather than the number of enables a process made.
struct Enable {
    next: *mut Enable,
    live: AtomicBool,
    session: AtomicU64,
    guid: [u8; 16],
    demands: AtomicDemands,
}

/// The newest session block, and the newest enable (0: none yet).
/// Stored with Release once a fresh block's own fields are written, and
/// loaded with Acquire by every walk, so a walk that holds no lock sees
/// a finished block or no block at all.
static SESSIONS: AtomicUsize = AtomicUsize::new(0);
static ENABLES: AtomicUsize = AtomicUsize::new(0);

/// Held while either list is changed and while a block's lock-guarded
/// fields are read or written. Nothing a write reaches takes it, no
/// system call is made beneath it, and no call into a provider's
/// callback runs beneath it -- the callback is the program's own code,
/// and it may register a provider of its own.
static LOCK: Lock = Lock::new(b"advapi32.dll!the trace lists");
/// The next TRACEHANDLE. Never 0: 0 is what a failed start leaves, and
/// what `EnableTraceEx2` answers ERROR_INVALID_PARAMETER for.
static NEXT_HANDLE: AtomicU64 = AtomicU64::new(0x0100_0001);

/// Whether the kernel has been asked what `etw=` wants of this program.
/// One system call in a process's life, at its first `EventRegister`: a
/// program that registers no provider pays nothing at all, and one whose
/// run asked for no session pays one call and then nothing.
static ASKED: AtomicBool = AtomicBool::new(false);

/// The handle of the session gala started, or 0 for none: read without
/// the lists' lock, so that the common case costs one load.
static KERNEL: AtomicU64 = AtomicU64::new(0);

/// The demands on `guid`: the union of every running session's, or
/// `None` when no session asks for it. This is the question a provider
/// answers `EventEnabled` from.
///
/// No lock: this is the first thing every `EventWrite` asks, and a
/// writer must not wait for a thread that may be suspended.
pub(crate) fn demands(guid: &[u8; 16]) -> Option<Demands> {
    union_of(guid)
}

/// The union of the live enables for `guid`. No lock: a block is never
/// freed, its `guid` never changes, and the rest of what is read is
/// atomic.
fn union_of(guid: &[u8; 16]) -> Option<Demands> {
    let mut union: Option<Demands> = None;
    let mut at = ENABLES.load(Ordering::Acquire);
    while at != 0 {
        // SAFETY: a block this DLL allocated and never frees, published
        // only once its own fields were written.
        let e = unsafe { &*(at as *const Enable) };
        if e.live.load(Ordering::Acquire) && e.guid == *guid {
            let demands = e.demands.load();
            union = Some(match union {
                Some(sofar) => sofar.with(demands),
                None => demands,
            });
        }
        at = e.next as usize;
    }
    union
}

/// What one session asks of `guid`, if it asks at all. No lock, for the
/// same reasons as `union_of`: every write asks it of every running
/// session.
fn asked_by(session: u64, guid: &[u8; 16]) -> Option<Demands> {
    let mut at = ENABLES.load(Ordering::Acquire);
    while at != 0 {
        // SAFETY: ours, never freed, published whole.
        let e = unsafe { &*(at as *const Enable) };
        if e.live.load(Ordering::Acquire)
            && e.guid == *guid
            && e.session.load(Ordering::Relaxed) == session
        {
            return Some(e.demands.load());
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
    let mut at = ENABLES.load(Ordering::Acquire);
    while at != 0 {
        // SAFETY: ours, never freed.
        let e = unsafe { &*(at as *const Enable) };
        let live = e.live.load(Ordering::Relaxed);
        if live && e.guid == *guid && e.session.load(Ordering::Relaxed) == session {
            e.demands.store(demands);
            return union_of(guid);
        }
        // Only a block this same provider's enables have had: a block's
        // guid never changes, which is what lets a write key on it with
        // no lock.
        if !live && e.guid == *guid {
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
        // SAFETY: a zeroed block of exactly this size, which no other
        // thread can reach until it is published below. `guid` is
        // written here and never again.
        unsafe {
            (*fresh).next = ENABLES.load(Ordering::Relaxed) as *mut Enable;
            (*fresh).guid = *guid;
        }
        // Release: a walk that loads ENABLES with Acquire reads a block
        // whose guid is written.
        ENABLES.store(fresh as usize, Ordering::Release);
        fresh as usize
    };
    // SAFETY: ours, never freed.
    let e = unsafe { &*(block as *const Enable) };
    e.session.store(session, Ordering::Relaxed);
    e.demands.store(demands);
    // Last, and with Release: a walk that reads `live` with Acquire
    // sees the session and demands this enable was made with.
    e.live.store(true, Ordering::Release);
    union_of(guid)
}

/// `session` stops asking for `guid`; what the sessions that are left
/// still ask for, if any. The lists are held.
fn drop_enable(session: u64, guid: &[u8; 16]) -> Option<Demands> {
    let mut at = ENABLES.load(Ordering::Acquire);
    while at != 0 {
        // SAFETY: ours, never freed.
        let e = unsafe { &*(at as *const Enable) };
        if e.live.load(Ordering::Relaxed)
            && e.guid == *guid
            && e.session.load(Ordering::Relaxed) == session
        {
            // Release: what this enable held is in view of whichever
            // enable takes the block next.
            e.live.store(false, Ordering::Release);
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
///
/// `LOCK` is held throughout, which is what lets `f` read and write the
/// block's lock-guarded fields; it gets a raw pointer rather than a
/// reference because a write holds a `&Session` to the same block at
/// the same time. This is the query and stop path, which no write
/// waits for.
/// The name a session block holds, as a slice of the block itself.
///
/// # Safety
///
/// `s` is a block on the session list and `LOCK` is held: the name and
/// its length are the lock's to read.
unsafe fn name_of(s: *mut Session) -> &'static [u16] {
    // SAFETY: the caller holds the lock, and `name_len` is never past
    // `NAME_MAX`, which is the array's length.
    unsafe { core::slice::from_raw_parts((&raw const (*s).name).cast::<u16>(), (*s).name_len) }
}

fn with_session<R>(
    handle: u64,
    name: &[u16],
    f: impl FnOnce(*mut Session) -> R,
) -> (Option<R>, bool) {
    LOCK.with(|| {
        let mut stopped = false;
        let mut at = SESSIONS.load(Ordering::Acquire);
        while at != 0 {
            // SAFETY: ours, never freed; the lock-guarded fields are
            // read with the lock held.
            let s = at as *mut Session;
            let (used, live, its_handle, named) = unsafe {
                (
                    (*s).used.load(Ordering::Relaxed),
                    (*s).live.load(Ordering::Relaxed),
                    (*s).handle.load(Ordering::Relaxed),
                    handle == 0
                        && !name.is_empty()
                        && (*s).name_len == name.len()
                        && name_of(s) == name,
                )
            };
            if used && ((handle != 0 && its_handle == handle) || named) {
                if live {
                    return (Some(f(s)), false);
                }
                stopped = true;
            }
            // SAFETY: as above.
            at = unsafe { (*s).next as usize };
        }
        (None, stopped)
    })
}

/// A block for a new session: one whose session is over, else a fresh
/// one on the list. 0 when there is no memory for it. The lists are
/// held.
fn take_block() -> usize {
    let mut at = SESSIONS.load(Ordering::Acquire);
    while at != 0 {
        // SAFETY: ours, never freed.
        let s = unsafe { &*(at as *const Session) };
        if !s.live.load(Ordering::Relaxed) {
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
    // SAFETY: a zeroed block of exactly this size, which no other
    // thread can reach until it is published below.
    unsafe { (*fresh).next = SESSIONS.load(Ordering::Relaxed) as *mut Session };
    // Release: a walk that loads SESSIONS with Acquire reads a block
    // whose `next` is written. Its own fields come into view with the
    // store of `live` that ends the start, and until then a walk sees
    // a block that is not running and skips it.
    SESSIONS.store(fresh as usize, Ordering::Release);
    fresh as usize
}

/// Room for a session's events: the process's budget taken and the
/// block allocated, or null when either is spent. `budget` says which,
/// because the two are different things to a reader -- a program past
/// `BUFFERS_MAX` has started and stopped more sessions than a process
/// may, where a failed allocation is the heap being out.
///
/// Called before `LOCK` is taken: it allocates, and the lock must not
/// be held across anything that can take a lock of its own.
fn reserve_buffers(capacity: usize, budget: &mut bool) -> *mut Buf {
    let mut spent = BUFFERS_SPENT.load(Ordering::Relaxed);
    loop {
        let want = spent.saturating_add(capacity);
        if want > BUFFERS_MAX {
            *budget = true;
            return core::ptr::null_mut();
        }
        match BUFFERS_SPENT.compare_exchange_weak(spent, want, Ordering::Relaxed, Ordering::Relaxed)
        {
            Ok(_) => break,
            Err(now) => spent = now,
        }
    }
    // SAFETY: plain allocation, zeroed (HEAP_ZERO_MEMORY), of the
    // header and the bytes that follow it.
    let block = unsafe { HeapAlloc(GetProcessHeap(), 8, size_of::<Buf>() + capacity) };
    if block.is_null() {
        BUFFERS_SPENT.fetch_sub(capacity, Ordering::Relaxed);
        return core::ptr::null_mut();
    }
    let buf = block.cast::<Buf>();
    // SAFETY: a zeroed block of at least this size, which no other
    // thread can reach until the session is published.
    unsafe { (*buf).capacity = capacity };
    buf
}

/// Buffers a start allocated and then did not publish. No thread but
/// this one has ever had them, which is the one case in which a
/// session's buffers can go back at all.
fn give_back(buf: *mut Buf, capacity: usize) {
    if buf.is_null() {
        return;
    }
    // SAFETY: this thread's own allocation, reached by nothing else.
    unsafe { HeapFree(GetProcessHeap(), 0, buf.cast::<u8>()) };
    BUFFERS_SPENT.fetch_sub(capacity, Ordering::Relaxed);
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
    // The buffers before the lists: allocating takes the heap's own
    // lock, and this one is not held across another.
    let mut budget = false;
    let buf = reserve_buffers(capacity, &mut budget);
    if buf.is_null() {
        if budget {
            crate::missing_detail(
                b"advapi32.dll!StartTraceW(the process has spent its trace buffers)",
                capacity as u64,
            );
        } else {
            crate::missing(b"advapi32.dll!StartTraceW(no memory)");
        }
        return ERROR_NOT_ENOUGH_MEMORY;
    }
    let started = LOCK.with(|| {
        // A name a running session already has.
        let mut at = SESSIONS.load(Ordering::Acquire);
        while at != 0 {
            // SAFETY: ours, never freed; the lock is held.
            let s = at as *mut Session;
            let taken = unsafe {
                (*s).live.load(Ordering::Relaxed)
                    && (*s).name_len == name_len
                    && name_of(s) == &session_name[..name_len]
            };
            if taken {
                return Err(ERROR_ALREADY_EXISTS);
            }
            // SAFETY: as above.
            at = unsafe { (*s).next as usize };
        }
        let block = take_block();
        if block == 0 {
            return Err(ERROR_NOT_ENOUGH_MEMORY);
        }
        let handle = NEXT_HANDLE.fetch_add(1, Ordering::Relaxed);
        // SAFETY: ours, never freed; the lock-guarded fields are
        // written with the lock held, and the atomic ones before the
        // store of `live` that publishes all of them.
        unsafe {
            let s = block as *mut Session;
            (*s).name = session_name;
            (*s).name_len = name_len;
            (*s).mode = mode;
            (*s).buffers = buffers;
            (*s).handle.store(handle, Ordering::Relaxed);
            (*s).to_kernel.store(false, Ordering::Relaxed);
            (*s).buf.store(buf, Ordering::Relaxed);
            (*s).used.store(true, Ordering::Relaxed);
            (*s).live.store(true, Ordering::Release);
        }
        Ok(handle)
    });
    let started_handle = match started {
        Ok(handle) => handle,
        Err(error) => {
            // Nothing was published, so the buffers are this thread's
            // alone: they go back, budget and all.
            give_back(buf, capacity);
            if error == ERROR_NOT_ENOUGH_MEMORY {
                crate::missing(b"advapi32.dll!StartTraceW(no memory)");
            }
            return error;
        }
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
    // SAFETY: the block the lock found, with the lock held.
    let (found, stopped) =
        with_session(session, &[], |s| unsafe { (*s).handle.load(Ordering::Relaxed) });
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
            match LOCK.with(|| drop_enable(session, &guid)) {
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
            match LOCK.with(|| set_enable(session, &guid, asked)) {
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
        // SAFETY: the block the lock found, with the lock held.
        let (name_len, mode, buffers, buf) =
            unsafe { ((*s).name_len, (*s).mode, (*s).buffers, (*s).buf.load(Ordering::Relaxed)) };
        // The name the session is known by goes back into the block,
        // which has to have room for it where it said.
        if at + (name_len + 1) * 2 > size {
            return Err(ERROR_MORE_DATA);
        }
        let lost = if buf.is_null() {
            0
        } else {
            // SAFETY: this session's buffers, never freed.
            unsafe { (*buf).lost.load(Ordering::Relaxed) }
        };
        // SAFETY: the caller's block, inside the size it gave, past the
        // structure's own fields; and the session's name, with the lock
        // held.
        unsafe {
            for (i, &c) in name_of(s).iter().enumerate() {
                (block.add(at) as *mut u16).add(i).write_unaligned(c);
            }
            (block.add(at) as *mut u16).add(name_len).write_unaligned(0);
            // Where the name went, which is how a caller that gave no
            // offset finds it (measured).
            (*properties).logger_name_offset = at as u32;
            (*properties).log_file_mode = mode;
            (*properties).number_of_buffers = buffers;
            (*properties).free_buffers = buffers;
            (*properties).events_lost = lost;
            // Nothing is ever written out: a buffering session's events
            // are its buffers.
            (*properties).buffers_written = 0;
        }
        if code == STOP {
            // SAFETY: as above.
            unsafe {
                // The buffers are let go of but not freed: a write that
                // holds no lock may be copying into them right now, and
                // nothing short of waiting for it says otherwise --
                // which is the wait this path exists not to have. They
                // stay the process's, counted against BUFFERS_MAX.
                (*s).buf.store(core::ptr::null_mut(), Ordering::Release);
                (*s).live.store(false, Ordering::Release);
                return Ok(Some((*s).handle.load(Ordering::Relaxed)));
            }
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
    let mut at = ENABLES.load(Ordering::Acquire);
    while at != 0 {
        let (next, gone) = LOCK.with(|| {
            // SAFETY: ours, never freed.
            let e = unsafe { &*(at as *const Enable) };
            let mine =
                e.live.load(Ordering::Relaxed) && e.session.load(Ordering::Relaxed) == handle;
            if mine {
                e.live.store(false, Ordering::Release);
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
/// `GalaEtw`'s operations (nt::etw): what the run asks of a session,
/// and one event a session kept.
const OP_ASK: usize = 0;
const OP_RECORD: usize = 1;

/// The ask the kernel answers op 0 with (`etw::ASK_LEN`): the provider
/// id, a flags byte, the level, six bytes of nothing, and the keyword as
/// a little-endian `u64`.
const ASK_LEN: usize = 32;
/// Bit 0 of that flags byte: every provider the program registers.
const ASK_EVERY: u8 = 1;
/// The header op 1 takes (`etw::RECORD_HEAD`): the provider id, the
/// event descriptor as the program built it, the whole payload's length,
/// then the lengths of the provider's metadata, the event's metadata and
/// the values, and the flags below.
const KERNEL_HEAD: usize = 44;
/// The most one record holds, header and blobs (`etw::RECORD_MAX`).
const KERNEL_MAX: usize = 2048;
/// The values did not fit and were cut (`etw::RECORD_CUT`).
const KERNEL_CUT: u16 = 1;
/// The event carried no TraceLogging metadata (`etw::RECORD_NO_META`).
const KERNEL_NO_META: u16 = 2;
/// What TraceLogging marks its two metadata pieces with
/// (EVENT_DATA_DESCRIPTOR_TYPE_PROVIDER_METADATA and _EVENT_METADATA).
const META_PROVIDER: u32 = 2;
const META_EVENT: u32 = 1;

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
/// (ROADMAP OT4 step 2 has the decoder, which is what the pieces are
/// kept whole for); what they give now is the count a query reports,
/// and a session that holds a program's own account of its work.
/// What `etw=` wants of this program, asked of the kernel, or `None`
/// when nothing did (or the kernel did not answer).
///
/// No lock is held: this is a system call, and a thread stopped inside
/// one must not be holding what another thread waits for. Asking twice
/// costs a second call and nothing else -- the answer is the same, and
/// the second one is dropped under the lock in `arm`.
fn ask_kernel() -> Option<[u8; ASK_LEN]> {
    let mut ask = [0u8; ASK_LEN];
    let mut written = 0u64;
    // SAFETY: a buffer and a word this frame owns, of the length given;
    // the service writes no more than it is told, and writes the buffer
    // only when it says it did.
    let status = unsafe {
        crate::GalaEtw(
            OP_ASK,
            ask.as_mut_ptr() as usize,
            ask.len(),
            (&raw mut written) as *mut u8 as usize,
        )
    };
    if status != 0 || written as usize != ASK_LEN {
        return None;
    }
    Some(ask)
}

/// The session `etw=` asks for, made from the kernel's answer. True if
/// there was no memory for it. The lists are held.
fn make_session(ask: &[u8; ASK_LEN]) -> bool {
    let block = take_block();
    if block == 0 {
        return true;
    }
    let handle = NEXT_HANDLE.fetch_add(1, Ordering::Relaxed);
    let keyword = u64::from_le_bytes(ask[24..32].try_into().unwrap_or_default());
    // SAFETY: ours, never freed; the lock-guarded fields are written
    // with the lock held, and the atomic ones before the store of
    // `live` that publishes all of them.
    unsafe {
        let s = block as *mut Session;
        // No name, which is what keeps the program from finding it: a
        // lookup by name never matches an empty one, and the handle is
        // never given out.
        (*s).name_len = 0;
        (*s).mode = PRIVATE | IN_PROC | BUFFERING;
        (*s).buffers = 0;
        (*s).every_provider = ask[16] & ASK_EVERY != 0;
        (*s).ask_guid.copy_from_slice(&ask[..16]);
        // A session that asks for keyword 0 asks for every keyword, as
        // `EnableTraceEx2` has it (the contract, measured).
        (*s).ask =
            Demands { level: ask[17], any: if keyword == 0 { u64::MAX } else { keyword }, all: 0 };
        (*s).handle.store(handle, Ordering::Relaxed);
        // It keeps no events of its own: they go to the kernel as they
        // are written.
        (*s).buf.store(core::ptr::null_mut(), Ordering::Relaxed);
        (*s).to_kernel.store(true, Ordering::Relaxed);
        (*s).used.store(true, Ordering::Relaxed);
        (*s).live.store(true, Ordering::Release);
    }
    KERNEL.store(handle, Ordering::Relaxed);
    false
}

/// The enable the session gala started has on `guid`, made now if the
/// run asked for that provider. A registration is the only moment a
/// provider learns anything, so this runs at every `EventRegister` and
/// the registration that follows it tells the provider about the session
/// the way it would about one the program started itself.
pub(crate) fn arm(guid: &[u8; 16]) {
    // The common case, and the only one for a program whose run asked
    // for no session: one load and no lock.
    if ASKED.load(Ordering::Acquire) && KERNEL.load(Ordering::Relaxed) == 0 {
        return;
    }
    // The system call before the lock, never beneath it: a thread
    // stopped in the middle of one must not be holding what another
    // thread waits for. Two registrations racing here both ask, and the
    // second answer is dropped under the lock below.
    let answer = if ASKED.load(Ordering::Relaxed) { None } else { ask_kernel() };
    let short = LOCK.with(|| {
        let mut short = false;
        if !ASKED.load(Ordering::Relaxed) {
            short = match answer {
                Some(ask) => make_session(&ask),
                None => false,
            };
            ASKED.store(true, Ordering::Release);
        }
        let handle = KERNEL.load(Ordering::Relaxed);
        if handle == 0 {
            return short;
        }
        let mut found = None;
        let mut at = SESSIONS.load(Ordering::Acquire);
        while at != 0 {
            // SAFETY: ours, never freed; the lock is held.
            let s = at as *mut Session;
            let mine = unsafe {
                (*s).live.load(Ordering::Relaxed) && (*s).handle.load(Ordering::Relaxed) == handle
            };
            if mine {
                // SAFETY: as above.
                found = Some(unsafe { ((*s).every_provider, (*s).ask_guid, (*s).ask) });
                break;
            }
            // SAFETY: as above.
            at = unsafe { (*s).next as usize };
        }
        let Some((every, wanted, ask)) = found else {
            return short;
        };
        if (!every && wanted != *guid) || asked_by(handle, guid).is_some() {
            return short;
        }
        short || set_enable(handle, guid, ask).is_none()
    });
    if short {
        crate::missing(b"advapi32.dll!EventRegister(no memory for the session etw= asks for)");
    }
}

/// One event handed to the session gala started (`GalaEtw` op 1). False
/// once its budget is spent, which is what ends that session: nothing is
/// gained by building payloads for a reader that has stopped reading.
///
/// The pieces go over as three blobs, because that is what makes the
/// event readable: TraceLogging marks the first two of its descriptors
/// as the provider's metadata and the event's, and those hold the names
/// of the event and of every field in it (the kernel's
/// `etw::tracelogging`). An event with neither is a manifest provider's,
/// whose names are in no part of it, and the record says so rather than
/// leaving the kernel to guess from the bytes.
fn to_kernel(
    guid: &[u8; 16],
    descriptor: *const u8,
    count: u32,
    data: *const u8,
    payload: usize,
) -> bool {
    let mut record = [0u8; KERNEL_MAX];
    record[..16].copy_from_slice(guid);
    if !descriptor.is_null() {
        // SAFETY: the caller's EVENT_DESCRIPTOR, sixteen bytes by
        // definition, copied into this frame's own buffer.
        unsafe { core::ptr::copy_nonoverlapping(descriptor, record.as_mut_ptr().add(16), 16) };
    }
    record[32..36].copy_from_slice(&(payload as u32).to_le_bytes());
    let pieces = if data.is_null() { 0 } else { count as usize };
    let piece = |i: usize| {
        // SAFETY: the caller's array of `count` descriptors, within it.
        unsafe { &*(data as *const DataDescriptor).add(i) }
    };
    let metadata = pieces >= 2 && piece(0).kind == META_PROVIDER && piece(1).kind == META_EVENT;
    let mut flags = if metadata { 0 } else { KERNEL_NO_META };
    let mut lengths = [0usize; 3];
    let mut at = KERNEL_HEAD;
    for i in 0..pieces {
        let d = piece(i);
        // The two metadata pieces are their own blobs; everything else
        // is the values, in the order the program wrote them.
        let which = if metadata && i < 2 { i } else { 2 };
        let mut n = if d.data == 0 { 0 } else { d.size as usize };
        if n > KERNEL_MAX - at {
            n = KERNEL_MAX - at;
            // Said in the record, so that the line says the values were
            // cut to fit rather than that the event ended there.
            flags |= KERNEL_CUT;
        }
        if n != 0 {
            // SAFETY: `n` bytes of the piece the caller gave, into this
            // frame's own buffer with that much room left in it.
            unsafe {
                core::ptr::copy_nonoverlapping(d.data as *const u8, record.as_mut_ptr().add(at), n)
            };
        }
        at += n;
        lengths[which] += n;
    }
    for (i, &length) in lengths.iter().enumerate() {
        record[36 + i * 2..38 + i * 2].copy_from_slice(&(length as u16).to_le_bytes());
    }
    record[42..44].copy_from_slice(&flags.to_le_bytes());
    // SAFETY: a buffer this frame owns, of the length given.
    let status = unsafe { crate::GalaEtw(OP_RECORD, record.as_ptr() as usize, at, 0) };
    status == 0
}

/// Every provider the session gala started asked for is told it is gone,
/// the way a session the program stops tells them. Called outside the
/// lists' lock, like every other call into a provider's callback.
pub(crate) fn session_over(handle: u64) {
    stop_telling(handle);
    crate::reached(b"advapi32.dll!EventWrite(the session etw= asked for is full)", 0);
}

pub(crate) fn keep(
    guid: &[u8; 16],
    descriptor: *const u8,
    count: u32,
    data: *const u8,
) -> Option<u64> {
    let pieces = if data.is_null() { 0 } else { count as usize };
    let mut payload = 0usize;
    for i in 0..pieces {
        // SAFETY: the caller's array of `count` descriptors.
        let d = unsafe { &*(data as *const DataDescriptor).add(i) };
        payload = payload.saturating_add(d.size as usize);
    }
    let length = RECORD_HEADER + payload;
    let mut over = None;
    // No lock, and no system call beneath one: this is the write path,
    // and a thread suspended anywhere in it stops nobody.
    let mut at = SESSIONS.load(Ordering::Acquire);
    while at != 0 {
        // SAFETY: ours, never freed, published whole.
        let s = unsafe { &*(at as *const Session) };
        at = s.next as usize;
        if !s.live.load(Ordering::Acquire) {
            continue;
        }
        let handle = s.handle.load(Ordering::Relaxed);
        if asked_by(handle, guid).is_none() {
            continue;
        }
        // The session gala started has no buffers: the event goes to
        // the kernel as it is written, and its budget running out is
        // what ends the session.
        if s.to_kernel.load(Ordering::Relaxed) {
            if !to_kernel(guid, descriptor, count, data, payload) {
                s.live.store(false, Ordering::Release);
                over = Some(handle);
            }
            continue;
        }
        let buf = s.buf.load(Ordering::Acquire);
        if buf.is_null() {
            continue;
        }
        // SAFETY: this session's buffers, never freed, with `capacity`
        // bytes after the header.
        let b = unsafe { &*buf };
        if length > RECORD_MAX {
            b.lost.fetch_add(1, Ordering::Relaxed);
            continue;
        }
        // The room is reserved with a compare-exchange rather than
        // under a lock: two threads writing at once get two ranges and
        // neither waits for the other, nor for one that is suspended.
        let mut from = b.at.load(Ordering::Relaxed);
        let reserved = loop {
            let want = from + length;
            if want > b.capacity {
                b.lost.fetch_add(1, Ordering::Relaxed);
                break None;
            }
            match b.at.compare_exchange_weak(from, want, Ordering::AcqRel, Ordering::Relaxed) {
                Ok(_) => break Some(from),
                Err(now) => from = now,
            }
        };
        let Some(from) = reserved else {
            continue;
        };
        // SAFETY: `length` bytes of this session's buffers, reserved by
        // the exchange above and so written by no other thread, and the
        // caller's descriptor and payload pieces.
        unsafe {
            let mut to = b.bytes().add(from);
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
    }
    over
}
