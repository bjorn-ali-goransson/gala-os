//! Event Tracing for Windows, the provider side (EventRegister and
//! friends, as documented in "Writing Manifest-based Events"): the
//! registrations a program makes, what each one is told a session wants
//! of it, and where its events go. The session side is `trace`, which
//! owns what every running session asks for; this half is the list of
//! registrations and the answers a program gets on them.
//!
//! A provider no session enabled is not enabled, which is the usual
//! case: its events go nowhere, successfully, as they do on Windows.
//! One a session asked for is told so in its callback, answers
//! `EventEnabled` yes for the events in reach, and hands those to the
//! session, which keeps them.
//!
//! Every one of those answers leaves a `reached` row, because a program's
//! events are the one account of its own work it writes down: a provider
//! it registers names a subsystem it is about to run, and an `EventEnabled`
//! answered no is a payload it then does not build. Without the rows a
//! reader cannot tell a program that never traces from one whose trace
//! gala turned off, and the registrations name the providers a session
//! would have to enable to hear it. A Word startup registers 20 of them
//! and writes nothing at all: TraceLogging builds no payload for a
//! provider no session enabled.
//!
//! A REGHANDLE is one of those registrations, not a number: it is the
//! address of the block below, every block this DLL allocates stays on
//! the list whether a program still holds it or not, and a handle that is
//! not on that list is told so rather than dereferenced. That is what
//! Windows answers (measured on the pinned build): a write on an
//! unregistered handle is ERROR_INVALID_HANDLE where a write on a live
//! one is ERROR_SUCCESS and goes nowhere.
//!
//! Nothing a program's write path reaches takes a lock. The list is
//! only ever added to, a block is never freed, and the fields a write
//! reads are either written before the block is published or atomic, so
//! a walk reads what it finds and never waits. That is not a
//! refinement: Office suspends its own threads to walk their stacks
//! while the process traces, so a lock here is one a stopped thread can
//! be holding, and every other writer would then spin for good -- which
//! is where Word's main thread sat (`user/etwlock_test`, ROADMAP OF).

use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

use gala_lock::Lock;

/// TRACEHANDLE values for the classic (MOF) providers at the end of this
/// file: never 0 (0 is what failed registration leaves).
static NEXT: AtomicU64 = AtomicU64::new(1);

const ERROR_INVALID_HANDLE: u32 = 6;
const ERROR_NOT_ENOUGH_MEMORY: u32 = 8;
/// What a buffer that is no trace session's answers.
const ERROR_BAD_LENGTH: u32 = 24;
/// What Windows answers a second EventProviderSetTraits.
const ERROR_GEN_FAILURE: u32 = 31;
const ERROR_NOT_SUPPORTED: u32 = 50;
const ERROR_INVALID_PARAMETER: u32 = 87;

#[cfg_attr(
    target_arch = "x86",
    link(name = "kernel32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "kernel32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn GetProcessHeap() -> usize;
    fn HeapAlloc(heap: usize, flags: u32, size: usize) -> *mut u8;
}

/// One `EventRegister`: a provider this process registered, and the block
/// whose address is the REGHANDLE the program holds. A block is never
/// freed -- an unregistered one is taken by the next registration of
/// that same provider -- so the memory is the most providers a process
/// held at once (20 for a Word startup) and no stale handle is ever a
/// live block of something else.
struct Provider {
    /// The next block, live or not: the list is only ever added to, and
    /// a block is published after this and `guid` are written, so a
    /// walk that holds no lock never reads a half-made one.
    next: *mut Provider,
    /// Whether a program holds this handle now. EventUnregister clears
    /// it, and every call on it afterwards is ERROR_INVALID_HANDLE.
    live: AtomicBool,
    /// Whether this provider's traits have been set: Windows answers a
    /// second EventProviderSetTraits ERROR_GEN_FAILURE.
    traits: AtomicBool,
    /// The provider the registration is for: what a session names when
    /// it enables one, and what an event is then weighed against.
    ///
    /// Written before the block is published and never again: a block
    /// a program let go of is taken only by a registration of the same
    /// provider (`take_block`). That is what lets a write read it
    /// without a lock -- the sixteen bytes are this block's for good,
    /// so there is no half-written id to read.
    guid: [u8; 16],
    /// The program's enable callback and the context it registered
    /// with, 0 for a registration that gave none. Held as addresses
    /// because a block is plain memory under a lock the callback must
    /// not run beneath, and atomic because a walk reads them while
    /// another registration may be taking the block.
    callback: AtomicUsize,
    context: AtomicUsize,
}

/// The newest block (0: none yet). Stored with Release once a fresh
/// block's own fields are written, and loaded with Acquire by every
/// walk, so a walk that holds no lock sees a finished block or no
/// block at all.
static HEAD: AtomicUsize = AtomicUsize::new(0);

/// Held while the list is added to and while a block is taken for a new
/// registration. Nothing a write reaches takes it, and no call into a
/// program's code runs beneath it -- an enable callback may register a
/// provider of its own.
static LOCK: Lock = Lock::new(b"advapi32.dll!the provider list");

/// The registration `handle` names, or `None` when no live registration
/// has that address.
///
/// No lock: this is the first thing every `EventWrite` asks, and a
/// writer must not wait for a thread that may be suspended. A block is
/// never freed and its `next` and `guid` are written before it is
/// published, so the walk reads what it finds; `live` is atomic because
/// `EventUnregister` and a new registration change it underfoot. A
/// handle whose block is taken again between the two loads below reads
/// as the registration it has become, which is what a program that
/// writes on a handle it let go of asks for.
fn find(handle: u64) -> Option<&'static Provider> {
    let mut at = HEAD.load(Ordering::Acquire);
    while at != 0 {
        // SAFETY: a block this DLL allocated and never frees, published
        // only once its own fields were written. The handle is
        // compared, never dereferenced.
        let p = unsafe { &*(at as *const Provider) };
        if at as u64 == handle && p.live.load(Ordering::Acquire) {
            return Some(p);
        }
        at = p.next as usize;
    }
    None
}

/// A block for a new registration: one a program let go of, else a fresh
/// one on the list. 0 when there is no memory for it.
fn take_block(guid: &[u8; 16], callback: usize, context: usize) -> usize {
    LOCK.with(|| {
        let mut at = HEAD.load(Ordering::Acquire);
        while at != 0 {
            // SAFETY: ours, never freed.
            let p = unsafe { &*(at as *const Provider) };
            // Only a block that was this same provider's: a block's
            // guid never changes, which is what lets a write read it
            // with no lock. A registration of another provider gets a
            // block of its own, so the memory is the most providers
            // this process ever held at once (20 for a Word startup)
            // rather than the most it registered.
            if !p.live.load(Ordering::Relaxed) && p.guid == *guid {
                p.traits.store(false, Ordering::Relaxed);
                p.callback.store(callback, Ordering::Relaxed);
                p.context.store(context, Ordering::Relaxed);
                // Last, and with Release: a walk that reads `live` with
                // Acquire sees the callback this registration gave.
                p.live.store(true, Ordering::Release);
                return at;
            }
            at = p.next as usize;
        }
        // SAFETY: plain allocation, zeroed (HEAP_ZERO_MEMORY).
        let fresh = unsafe { HeapAlloc(GetProcessHeap(), 8, size_of::<Provider>()) };
        let fresh = fresh.cast::<Provider>();
        if fresh.is_null() {
            return 0;
        }
        // SAFETY: a zeroed block of exactly this size, which no other
        // thread can reach until it is published below.
        unsafe {
            (*fresh).next = HEAD.load(Ordering::Relaxed) as *mut Provider;
            (*fresh).guid = *guid;
            (*fresh).traits = AtomicBool::new(false);
            (*fresh).callback = AtomicUsize::new(callback);
            (*fresh).context = AtomicUsize::new(context);
            (*fresh).live = AtomicBool::new(true);
        }
        // Release: a walk that loads HEAD with Acquire reads a block
        // whose own fields are all written.
        HEAD.store(fresh as usize, Ordering::Release);
        fresh as usize
    })
}

/// EventRegister. A registration made while a session already asks for
/// the provider is told so in its callback right away (measured on the
/// pinned build): a program that registers late is not left thinking
/// nobody listens.
#[unsafe(no_mangle)]
pub extern "system" fn EventRegister(
    provider: *const [u8; 16],
    callback: *const core::ffi::c_void,
    context: *mut core::ffi::c_void,
    handle: *mut u64,
) -> u32 {
    if provider.is_null() || handle.is_null() {
        return ERROR_INVALID_PARAMETER;
    }
    // SAFETY: the caller's provider id.
    let guid = unsafe { *provider };
    let block = take_block(&guid, callback as usize, context as usize);
    if block == 0 {
        crate::missing(b"advapi32.dll!EventRegister(no memory)");
        return ERROR_NOT_ENOUGH_MEMORY;
    }
    note_provider(b"advapi32.dll!EventRegister(", &guid);
    // SAFETY: the caller's output.
    unsafe { *handle = block as u64 };
    // The session gala started for this program, if the run asked for
    // one: a registration is the only moment a provider learns anything,
    // so this is where that session's enable is made and the callback
    // below is the one that tells the provider about it.
    crate::trace::arm(&guid);
    if let Some(demands) = crate::trace::demands(&guid) {
        crate::reached(b"advapi32.dll!EventRegister(a session is asking)", 0);
        call_back(callback as usize, context as usize, 1, demands);
    }
    0
}

/// What a session asks of `guid`, told to every live registration for
/// it, newest first. The walk takes no lock, like every other walk
/// here; the callback, which is the program's code and may register a
/// provider of its own, runs with no lock held either.
pub(crate) fn tell(guid: &[u8; 16], is_enabled: u32, demands: crate::trace::Demands) {
    let mut at = HEAD.load(Ordering::Acquire);
    while at != 0 {
        // SAFETY: ours, never freed, published whole.
        let p = unsafe { &*(at as *const Provider) };
        let callback = p.callback.load(Ordering::Relaxed);
        if p.live.load(Ordering::Acquire) && p.guid == *guid && callback != 0 {
            call_back(callback, p.context.load(Ordering::Relaxed), is_enabled, demands);
        }
        at = p.next as usize;
    }
}

/// PENABLECALLBACK: what a provider's callback is given. The source id
/// is all zeros for a private session (measured), and gala passes no
/// filter -- a session's scope filters are not kept, which
/// `EnableTraceEx2` says in a row of its own.
type EnableCallback = unsafe extern "system" fn(
    source: *const [u8; 16],
    is_enabled: u32,
    level: u8,
    any: u64,
    all: u64,
    filter: *const core::ffi::c_void,
    context: *mut core::ffi::c_void,
);

/// One call into a program's enable callback, outside every lock.
fn call_back(callback: usize, context: usize, is_enabled: u32, demands: crate::trace::Demands) {
    if callback == 0 {
        return;
    }
    const NO_SOURCE: [u8; 16] = [0; 16];
    // SAFETY: the address the program passed EventRegister, called with
    // the arguments PENABLECALLBACK documents. A program that passes an
    // address that is not its callback has already broken its own
    // registration, which is as true on Windows.
    unsafe {
        let f: EnableCallback = core::mem::transmute(callback);
        f(
            &NO_SOURCE,
            is_enabled,
            demands.level,
            demands.any,
            demands.all,
            core::ptr::null(),
            context as *mut core::ffi::c_void,
        );
    }
}

/// `advapi32.dll!<call>(<guid>)`: a provider a program registered, or
/// one a session asked for, one row each, in the spelling a trace
/// session would be given to enable it.
pub(crate) fn note_provider(prefix: &[u8], id: &[u8; 16]) {
    /// Room for the longest prefix either caller passes, which is
    /// `EnableTraceEx2`'s.
    const ROOM: usize = 32;
    if prefix.len() > ROOM {
        return;
    }
    let mut row = [0u8; ROOM + 36 + 1];
    row[..prefix.len()].copy_from_slice(prefix);
    let mut at = prefix.len();
    // A GUID's first three fields are little-endian, the rest bytes.
    for group in [&[3u8, 2, 1, 0][..], &[5, 4], &[7, 6], &[8, 9], &[10, 11, 12, 13, 14, 15]] {
        if at > prefix.len() {
            row[at] = b'-';
            at += 1;
        }
        for &i in group {
            row[at] = HEX[(id[i as usize] >> 4) as usize];
            row[at + 1] = HEX[(id[i as usize] & 15) as usize];
            at += 2;
        }
    }
    row[at] = b')';
    crate::reached(&row[..at + 1], 0);
}

const HEX: &[u8; 16] = b"0123456789abcdef";

/// EventUnregister: the program lets the registration go, and every call
/// on that handle afterwards is ERROR_INVALID_HANDLE -- which is what a
/// handle that was never a registration's gets as well.
#[unsafe(no_mangle)]
pub extern "system" fn EventUnregister(handle: u64) -> u32 {
    match find(handle) {
        // Release: what the registration wrote is in view of whichever
        // registration takes this block next.
        Some(p) => {
            p.live.store(false, Ordering::Release);
            0
        }
        None => ERROR_INVALID_HANDLE,
    }
}

/// Whether an event of this level and keyword is in reach of a running
/// session, and the row that says why -- which is the payload the
/// program then builds, or does not. A handle no live registration has
/// is FALSE with no row, which is what Windows answers, and so is a
/// handle a program has let go of.
fn answer(handle: u64, level: u8, keyword: u64, rows: [&[u8]; 3]) -> u8 {
    let Some(guid) = find(handle).map(|p| p.guid) else {
        return 0;
    };
    match crate::trace::demands(&guid) {
        None => {
            crate::reached(rows[0], 0);
            0
        }
        Some(demands) if demands.admits(level, keyword) => {
            crate::reached(rows[1], 0);
            1
        }
        Some(_) => {
            crate::reached(rows[2], 0);
            0
        }
    }
}

/// EventEnabled: the level and keyword out of the event's descriptor
/// (EVENT_DESCRIPTOR: Id, Version, Channel, then Level at 4 and Keyword
/// at 8). Windows answers FALSE for a null descriptor and for a handle
/// it does not know, and so does this.
#[unsafe(no_mangle)]
pub extern "system" fn EventEnabled(handle: u64, descriptor: *const u8) -> u8 {
    if descriptor.is_null() {
        return 0;
    }
    // SAFETY: the caller's descriptor, which is 16 bytes by definition;
    // the keyword is read unaligned because the caller's may be.
    let (level, keyword) =
        unsafe { (*descriptor.add(4), descriptor.add(8).cast::<u64>().read_unaligned()) };
    answer(
        handle,
        level,
        keyword,
        [
            b"advapi32.dll!EventEnabled(no session)",
            b"advapi32.dll!EventEnabled(enabled)",
            b"advapi32.dll!EventEnabled(out of reach)",
        ],
    )
}

/// EventProviderEnabled: the same question with the level and keyword
/// passed outright, which is what a program asks before it has built a
/// descriptor.
#[unsafe(no_mangle)]
pub extern "system" fn EventProviderEnabled(handle: u64, level: u8, keyword: u64) -> u8 {
    answer(
        handle,
        level,
        keyword,
        [
            b"advapi32.dll!EventProviderEnabled(no session)",
            b"advapi32.dll!EventProviderEnabled(enabled)",
            b"advapi32.dll!EventProviderEnabled(out of reach)",
        ],
    )
}

/// What a write answers: the descriptor first (Windows answers
/// ERROR_INVALID_PARAMETER for a null one even on a handle that is no
/// registration's), then the handle, and then the event -- to every
/// session that asked for the provider and admits it, and otherwise
/// nowhere, successfully, as it goes on Windows when nobody listens.
///
/// `rows` is the pair this write leaves: kept by a session, or nowhere.
fn write(handle: u64, descriptor: *const u8, count: u32, data: *const u8, rows: [&[u8]; 2]) -> u32 {
    if descriptor.is_null() {
        return ERROR_INVALID_PARAMETER;
    }
    let Some(guid) = find(handle).map(|p| p.guid) else {
        return ERROR_INVALID_HANDLE;
    };
    // SAFETY: the caller's descriptor, 16 bytes by definition: Level at
    // 4 and Keyword at 8, the keyword unaligned because the caller's
    // may be.
    let (level, keyword) =
        unsafe { (*descriptor.add(4), descriptor.add(8).cast::<u64>().read_unaligned()) };
    match crate::trace::demands(&guid) {
        Some(demands) if demands.admits(level, keyword) => {
            if let Some(over) = crate::trace::keep(&guid, descriptor, count, data) {
                // The session gala started has had its fill: every
                // provider it asked for is told it is gone, outside the
                // lists' lock, exactly as a stopped session's are.
                crate::trace::session_over(over);
            }
            crate::reached(rows[0], 0);
        }
        // A session that does not admit the event is the same to the
        // program as no session at all: the write succeeds and the
        // event is gone. The row says the payload was built for
        // nothing, which is what a program avoids by asking
        // EventEnabled first.
        _ => crate::reached(rows[1], 0),
    }
    0
}

/// EventWriteString: a write with no descriptor and no payload
/// descriptors of its own, only a level, a keyword and a string. A
/// session that admits it is told the event happened; the string
/// itself is not kept, because the records a session holds are
/// TraceLogging's (provider metadata, event metadata, payload) and a
/// bare string is none of them. The row is the account of it.
fn wrote_string(handle: u64, level: u8, keyword: u64, rows: [&[u8]; 2]) -> u32 {
    let Some(guid) = find(handle).map(|p| p.guid) else {
        return ERROR_INVALID_HANDLE;
    };
    match crate::trace::demands(&guid) {
        Some(demands) if demands.admits(level, keyword) => crate::reached(rows[0], 0),
        _ => crate::reached(rows[1], 0),
    }
    0
}

#[unsafe(no_mangle)]
pub extern "system" fn EventWrite(
    handle: u64,
    descriptor: *const u8,
    count: u32,
    data: *const u8,
) -> u32 {
    write(
        handle,
        descriptor,
        count,
        data,
        [b"advapi32.dll!EventWrite(session)", b"advapi32.dll!EventWrite(no session)"],
    )
}

#[unsafe(no_mangle)]
pub extern "system" fn EventWriteTransfer(
    handle: u64,
    descriptor: *const u8,
    _activity: *const u8,
    _related: *const u8,
    count: u32,
    data: *const u8,
) -> u32 {
    write(
        handle,
        descriptor,
        count,
        data,
        [
            b"advapi32.dll!EventWriteTransfer(session)",
            b"advapi32.dll!EventWriteTransfer(no session)",
        ],
    )
}

#[unsafe(no_mangle)]
pub extern "system" fn EventWriteEx(
    handle: u64,
    descriptor: *const u8,
    _filter: u64,
    _flags: u32,
    _activity: *const u8,
    _related: *const u8,
    count: u32,
    data: *const u8,
) -> u32 {
    write(
        handle,
        descriptor,
        count,
        data,
        [b"advapi32.dll!EventWriteEx(session)", b"advapi32.dll!EventWriteEx(no session)"],
    )
}

#[unsafe(no_mangle)]
pub extern "system" fn EventWriteString(
    handle: u64,
    level: u8,
    keyword: u64,
    _text: *const u16,
) -> u32 {
    wrote_string(
        handle,
        level,
        keyword,
        [b"advapi32.dll!EventWriteString(session)", b"advapi32.dll!EventWriteString(no session)"],
    )
}

/// EVENT_INFO_CLASS's EventProviderSetTraits: the one class a provider
/// sets on itself.
const SET_TRAITS: u32 = 2;

/// EventSetInformation. The class is answered before the handle is looked
/// at, as Windows does (measured): the classes it has but whose argument
/// this is not (EventProviderBinaryTrackInfo 0,
/// EventProviderUseDescriptorType 3, and 4) are
/// ERROR_INVALID_PARAMETER, anything else it does not have is
/// ERROR_NOT_SUPPORTED, and only EventProviderSetTraits gets as far as
/// the registration -- where a handle that is none is
/// ERROR_INVALID_HANDLE, a blob too short to hold a name is
/// ERROR_INVALID_PARAMETER, and a second one is ERROR_GEN_FAILURE.
#[unsafe(no_mangle)]
pub extern "system" fn EventSetInformation(
    handle: u64,
    class: u32,
    info: *const u8,
    length: u32,
) -> u32 {
    if class != SET_TRAITS {
        return match class {
            0 | 3 | 4 => ERROR_INVALID_PARAMETER,
            _ => ERROR_NOT_SUPPORTED,
        };
    }
    let answer = find(handle).map(|p| {
        if info.is_null() || length < 3 {
            return ERROR_INVALID_PARAMETER;
        }
        // A second one is refused, and the first of two at once wins:
        // the swap is what decides which.
        if p.traits.swap(true, Ordering::Relaxed) {
            return ERROR_GEN_FAILURE;
        }
        0
    });
    match answer {
        Some(0) => {
            note_traits(info, length);
            0
        }
        Some(error) => error,
        None => ERROR_INVALID_HANDLE,
    }
}

/// `advapi32.dll!EventSetInformation(<name>)`: the provider's own name,
/// out of the traits TraceLogging sets on it right after registering (a
/// `u16` of the whole blob's size, the name in UTF-8 with its NUL, then
/// the optional chunks -- what `TraceLoggingProvider.h` builds). A GUID
/// says nothing to a reader; the name is what the program calls the
/// subsystem it is about to trace, and what a session would be told to
/// enable.
fn note_traits(info: *const u8, length: u32) {
    const PREFIX: &[u8] = b"advapi32.dll!EventSetInformation(";
    /// As much of a name as a row keeps.
    const NAME: usize = 64;
    let mut row = [0u8; PREFIX.len() + NAME + 1];
    row[..PREFIX.len()].copy_from_slice(PREFIX);
    let mut at = PREFIX.len();
    let mut i = 2;
    while i < length as usize && at < PREFIX.len() + NAME {
        // SAFETY: the caller's blob, inside the length it gave.
        let byte = unsafe { *info.add(i) };
        if byte == 0 {
            break;
        }
        // A name is ASCII in practice; anything else keeps its place.
        row[at] = if byte.is_ascii_graphic() || byte == b' ' { byte } else { b'?' };
        at += 1;
        i += 1;
    }
    row[at] = b')';
    crate::reached(&row[..at + 1], 0);
}

/// TEB.ActivityId (x64 offset 0x1710): the thread's ETW activity id.
#[cfg(target_arch = "x86_64")]
fn activity_id() -> *mut [u8; 16] {
    let teb: usize;
    // SAFETY: gs:[0x30] is the TEB's self pointer.
    unsafe { core::arch::asm!("mov {}, gs:[0x30]", out(reg) teb, options(nostack, readonly)) };
    (teb + 0x1710) as *mut [u8; 16]
}

/// TEB.ActivityId (x86 offset 0xf50): the thread's ETW activity id.
#[cfg(target_arch = "x86")]
fn activity_id() -> *mut [u8; 16] {
    let teb: usize;
    // SAFETY: fs:[0x18] is the TEB32's self pointer.
    unsafe { core::arch::asm!("mov {}, fs:[0x18]", out(reg) teb, options(nostack, readonly)) };
    (teb + 0xf50) as *mut [u8; 16]
}

/// TEB.ActivityId (the 64-bit TEB's 0x1710, as x64's): the thread's ETW
/// activity id.
#[cfg(target_arch = "aarch64")]
fn activity_id() -> *mut [u8; 16] {
    let teb: usize;
    // SAFETY: x18 holds the TEB in user mode (the ARM64 ABI).
    unsafe {
        core::arch::asm!("mov {}, x18", out(reg) teb, options(nomem, nostack, preserves_flags))
    };
    (teb + 0x1710) as *mut [u8; 16]
}

#[cfg_attr(
    target_arch = "x86",
    link(name = "bcryptprimitives", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "bcryptprimitives", kind = "raw-dylib"))]
unsafe extern "system" {
    fn ProcessPrng(data: *mut u8, length: usize) -> i32;
}

/// EventActivityIdControl: GET_ID (1), SET_ID (2), CREATE_ID (3, a new
/// version-4 GUID, not stored), GET_SET_ID (4, swap), CREATE_SET_ID (5,
/// store a new one, return the old).
#[unsafe(no_mangle)]
pub extern "system" fn EventActivityIdControl(code: u32, id: *mut [u8; 16]) -> u32 {
    if id.is_null() {
        return ERROR_INVALID_PARAMETER;
    }
    let new_id = || {
        let mut g = [0u8; 16];
        // SAFETY: our buffer.
        unsafe { ProcessPrng(g.as_mut_ptr(), 16) };
        g[7] = (g[7] & 0x0f) | 0x40;
        g[8] = (g[8] & 0x3f) | 0x80;
        g
    };
    let current = activity_id();
    // SAFETY: the caller's GUID; this thread's TEB.
    unsafe {
        match code {
            1 => *id = *current,
            2 => *current = *id,
            3 => *id = new_id(),
            4 => core::ptr::swap(id, current),
            5 => {
                *id = *current;
                *current = new_id();
            }
            _ => return ERROR_INVALID_PARAMETER,
        }
    }
    0
}

// ---- Classic (MOF) providers: RegisterTraceGuids and friends ----
//
// The same: a provider registers; no trace session ever enables it, so its
// control callback is never called, there is no logger handle to log to,
// and logging with a session handle it could not have got fails.

#[cfg_attr(
    target_arch = "x86",
    link(name = "kernel32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "kernel32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn SetLastError(code: u32);
}

/// RegisterTraceGuidsW: a registration handle (the control callback is not
/// called: no session enables the provider).
#[allow(clippy::too_many_arguments)]
#[unsafe(no_mangle)]
pub extern "system" fn RegisterTraceGuidsW(
    callback: usize,
    _context: *mut core::ffi::c_void,
    control: *const [u8; 16],
    _count: u32,
    _registrations: *mut [usize; 2],
    _resource_path: *const u16,
    _resource_name: *const u16,
    handle: *mut u64,
) -> u32 {
    if callback == 0 || control.is_null() || handle.is_null() {
        return ERROR_INVALID_PARAMETER;
    }
    // SAFETY: the caller's output.
    unsafe { *handle = NEXT.fetch_add(1, Ordering::Relaxed) };
    0
}

#[unsafe(no_mangle)]
pub extern "system" fn UnregisterTraceGuids(_handle: u64) -> u32 {
    0
}

/// GetTraceLoggerHandle: only the control callback's buffer names a
/// session, and it is never called, so no buffer a program has is one of
/// ours: INVALID_HANDLE_VALUE, with the error Windows gives for the
/// buffer itself -- ERROR_INVALID_PARAMETER for none at all, and
/// ERROR_BAD_LENGTH for one whose header is not a session's (measured
/// with a zeroed buffer).
#[unsafe(no_mangle)]
pub extern "system" fn GetTraceLoggerHandle(buffer: *mut core::ffi::c_void) -> u64 {
    let error = if buffer.is_null() { ERROR_INVALID_PARAMETER } else { ERROR_BAD_LENGTH };
    // SAFETY: plain call.
    unsafe { SetLastError(error) };
    u64::MAX
}

/// GetTraceEnableFlags / GetTraceEnableLevel of a session handle there
/// cannot be.
#[unsafe(no_mangle)]
pub extern "system" fn GetTraceEnableFlags(_session: u64) -> u32 {
    // SAFETY: plain call.
    unsafe { SetLastError(ERROR_INVALID_HANDLE) };
    0
}

#[unsafe(no_mangle)]
pub extern "system" fn GetTraceEnableLevel(_session: u64) -> u8 {
    // SAFETY: plain call.
    unsafe { SetLastError(ERROR_INVALID_HANDLE) };
    0
}

/// TraceEvent: no session to log to.
#[unsafe(no_mangle)]
pub extern "system" fn TraceEvent(_session: u64, _header: *mut u8) -> u32 {
    ERROR_INVALID_HANDLE
}
