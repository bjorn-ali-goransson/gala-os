//! Event Tracing for Windows, the provider side (EventRegister and
//! friends, as documented in "Writing Manifest-based Events"): gala has no
//! trace sessions, so no provider is ever enabled. Registration succeeds
//! and events go nowhere, successfully - what Windows does for a provider
//! no session listens to.
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

use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

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
/// freed -- an unregistered one is taken by the next registration -- so
/// the memory is the most providers a process held at once (20 for a Word
/// startup) and no stale handle is ever a live block of something else.
struct Provider {
    /// The next block, live or not: the list is only ever added to.
    next: *mut Provider,
    /// Whether a program holds this handle now. EventUnregister clears
    /// it, and every call on it afterwards is ERROR_INVALID_HANDLE.
    live: bool,
    /// Whether this provider's traits have been set: Windows answers a
    /// second EventProviderSetTraits ERROR_GEN_FAILURE.
    traits: bool,
}

/// The newest block (0: none yet).
static HEAD: AtomicUsize = AtomicUsize::new(0);
/// Held across a walk of the list and a block's own fields.
static LOCK: AtomicBool = AtomicBool::new(false);

/// `f` with the provider list held.
fn with_list<R>(f: impl FnOnce() -> R) -> R {
    while LOCK.swap(true, Ordering::Acquire) {
        core::hint::spin_loop();
    }
    let r = f();
    LOCK.store(false, Ordering::Release);
    r
}

/// `f` with the registration `handle` names, or `None` when no live
/// registration has that address. The walk is over the providers this
/// process registered, on the calls a program makes once a provider and
/// on writes that go nowhere -- a provider no session enabled builds no
/// payload, so nothing on a drawing loop is behind it.
fn with_provider<R>(handle: u64, f: impl FnOnce(&mut Provider) -> R) -> Option<R> {
    with_list(|| {
        let mut at = HEAD.load(Ordering::Relaxed);
        while at != 0 {
            // SAFETY: a block this DLL allocated and never frees. The
            // handle is compared, never dereferenced.
            let p = unsafe { &mut *(at as *mut Provider) };
            if at as u64 == handle && p.live {
                return Some(f(p));
            }
            at = p.next as usize;
        }
        None
    })
}

/// A block for a new registration: one a program let go of, else a fresh
/// one on the list. 0 when there is no memory for it.
fn take_block() -> usize {
    with_list(|| {
        let mut at = HEAD.load(Ordering::Relaxed);
        while at != 0 {
            // SAFETY: ours, never freed.
            let p = unsafe { &mut *(at as *mut Provider) };
            if !p.live {
                p.live = true;
                p.traits = false;
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
        // SAFETY: a zeroed block of exactly this size.
        unsafe {
            (*fresh).next = HEAD.load(Ordering::Relaxed) as *mut Provider;
            (*fresh).live = true;
            (*fresh).traits = false;
        }
        HEAD.store(fresh as usize, Ordering::Relaxed);
        fresh as usize
    })
}

#[unsafe(no_mangle)]
pub extern "system" fn EventRegister(
    provider: *const [u8; 16],
    _callback: *const core::ffi::c_void,
    _context: *mut core::ffi::c_void,
    handle: *mut u64,
) -> u32 {
    if provider.is_null() || handle.is_null() {
        return ERROR_INVALID_PARAMETER;
    }
    let block = take_block();
    if block == 0 {
        crate::missing(b"advapi32.dll!EventRegister(no memory)");
        return ERROR_NOT_ENOUGH_MEMORY;
    }
    // SAFETY: the caller's provider id.
    note_provider(unsafe { &*provider });
    // SAFETY: the caller's output.
    unsafe { *handle = block as u64 };
    0
}

/// `advapi32.dll!EventRegister(<guid>)`: the provider a program is about
/// to write events for, one row each, in the spelling a trace session
/// would be given to enable it.
fn note_provider(id: &[u8; 16]) {
    const PREFIX: &[u8] = b"advapi32.dll!EventRegister(";
    let mut row = [0u8; PREFIX.len() + 36 + 1];
    row[..PREFIX.len()].copy_from_slice(PREFIX);
    let mut at = PREFIX.len();
    // A GUID's first three fields are little-endian, the rest bytes.
    for group in [&[3u8, 2, 1, 0][..], &[5, 4], &[7, 6], &[8, 9], &[10, 11, 12, 13, 14, 15]] {
        if at > PREFIX.len() {
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
    match with_provider(handle, |p| p.live = false) {
        Some(()) => 0,
        None => ERROR_INVALID_HANDLE,
    }
}

/// EventEnabled / EventProviderEnabled: no session enables anything, for
/// a registration of this process's or anything else (Windows answers
/// FALSE for a handle it does not know, and for a null descriptor). The
/// row is the payload the program then does not build.
#[unsafe(no_mangle)]
pub extern "system" fn EventEnabled(_handle: u64, _descriptor: *const u8) -> u8 {
    crate::reached(b"advapi32.dll!EventEnabled(no session)", 0);
    0
}

#[unsafe(no_mangle)]
pub extern "system" fn EventProviderEnabled(_handle: u64, _level: u8, _keyword: u64) -> u8 {
    crate::reached(b"advapi32.dll!EventProviderEnabled(no session)", 0);
    0
}

/// What a write answers: the descriptor first (Windows answers
/// ERROR_INVALID_PARAMETER for a null one even on a handle that is no
/// registration's), then the handle, and then the event goes nowhere,
/// successfully -- no session listens.
fn write(handle: u64, descriptor: *const u8, what: &[u8]) -> u32 {
    if descriptor.is_null() {
        return ERROR_INVALID_PARAMETER;
    }
    wrote_nowhere(handle, what)
}

/// A write with no descriptor of its own (EventWriteString): the handle,
/// then nowhere, successfully.
fn wrote_nowhere(handle: u64, what: &[u8]) -> u32 {
    if with_provider(handle, |_| ()).is_none() {
        return ERROR_INVALID_HANDLE;
    }
    crate::reached(what, 0);
    0
}

#[unsafe(no_mangle)]
pub extern "system" fn EventWrite(
    handle: u64,
    descriptor: *const u8,
    _count: u32,
    _data: *const u8,
) -> u32 {
    write(handle, descriptor, b"advapi32.dll!EventWrite(no session)")
}

#[unsafe(no_mangle)]
pub extern "system" fn EventWriteTransfer(
    handle: u64,
    descriptor: *const u8,
    _activity: *const u8,
    _related: *const u8,
    _count: u32,
    _data: *const u8,
) -> u32 {
    write(handle, descriptor, b"advapi32.dll!EventWriteTransfer(no session)")
}

#[unsafe(no_mangle)]
pub extern "system" fn EventWriteEx(
    handle: u64,
    descriptor: *const u8,
    _filter: u64,
    _flags: u32,
    _activity: *const u8,
    _related: *const u8,
    _count: u32,
    _data: *const u8,
) -> u32 {
    write(handle, descriptor, b"advapi32.dll!EventWriteEx(no session)")
}

#[unsafe(no_mangle)]
pub extern "system" fn EventWriteString(
    handle: u64,
    _level: u8,
    _keyword: u64,
    _text: *const u16,
) -> u32 {
    wrote_nowhere(handle, b"advapi32.dll!EventWriteString(no session)")
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
    let answer = with_provider(handle, |p| {
        if info.is_null() || length < 3 {
            return ERROR_INVALID_PARAMETER;
        }
        if p.traits {
            return ERROR_GEN_FAILURE;
        }
        p.traits = true;
        0
    });
    match answer {
        // The row is written outside the list's lock: it is a system call.
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
