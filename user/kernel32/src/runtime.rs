//! What language runtimes (CoreCLR) use beyond the basics: alertable
//! multi-object waits, SignalObjectAndWait, user APCs, stack guarantees,
//! per-thread error modes, thread cycle times, locked pages, memory
//! resource notifications, IsWow64Process2, jobs, the named-pipe server
//! calls, WER runtime modules and FlushInstructionCache. As documented
//! for each; gala's limits are noted where they apply.

use core::ffi::c_void;
use core::sync::atomic::{AtomicBool, Ordering};

use super::files::WaitForSingleObjectEx;
use super::process::GetThreadTimes;
use super::sync::{CreateEventW, ReleaseMutex, ReleaseSemaphore};
use super::{
    Bool, FALSE, GetLastError, Handle, NtWaitForMultipleObjects, STATUS_SUCCESS, STATUS_TIMEOUT,
    SetEvent, TRUE, fail, fail_with, teb_u64, timeout, unsupported,
};

const WAIT_FAILED: u32 = u32::MAX;
const ERROR_INVALID_HANDLE: u32 = 6;
const ERROR_INVALID_PARAMETER: u32 = 87;
const ERROR_NOT_LOCKED: u32 = 158;

/// WaitForMultipleObjectsEx: WAIT_IO_COMPLETION (0xc0) when an APC ran
/// during an alertable wait.
#[unsafe(no_mangle)]
pub extern "system" fn WaitForMultipleObjectsEx(
    count: u32,
    handles: *const Handle,
    all: Bool,
    ms: u32,
    alertable: Bool,
) -> u32 {
    let t = timeout(ms);
    let p = t.as_ref().map_or(core::ptr::null(), |t| t as *const i64);
    // WaitAll = 0, WaitAny = 1.
    let wait_type = if all != 0 { 0 } else { 1 };
    // SAFETY: the caller's handle array; valid or null timeout pointer.
    let status =
        unsafe { NtWaitForMultipleObjects(count, handles, wait_type, u8::from(alertable != 0), p) };
    match status {
        0..=0x3f | 0x80..=0xbf | 0xc0 | STATUS_TIMEOUT => status,
        _ => fail(status, WAIT_FAILED),
    }
}

/// SignalObjectAndWait: signals an event, semaphore (by one) or mutex,
/// then waits for the other object. On Windows the two steps are one
/// atomic operation; here another thread may run between them (TODO: a
/// kernel service).
#[unsafe(no_mangle)]
pub extern "system" fn SignalObjectAndWait(
    signal: Handle,
    wait: Handle,
    ms: u32,
    alertable: Bool,
) -> u32 {
    let signaled = SetEvent(signal) != FALSE
        || ReleaseSemaphore(signal, 1, core::ptr::null_mut()) != FALSE
        || ReleaseMutex(signal) != FALSE;
    if !signaled {
        return fail_with(GetLastError(), WAIT_FAILED);
    }
    WaitForSingleObjectEx(wait, ms, alertable)
}

/// QueueUserAPC: the routine runs with `data` in the thread's next
/// alertable wait (NtQueueApcThread passes it as the first argument).
#[unsafe(no_mangle)]
pub extern "system" fn QueueUserAPC(routine: usize, thread: Handle, data: usize) -> u32 {
    // SAFETY: the kernel validates the handle; the routine is the caller's.
    let status = unsafe { NtQueueApcThread(thread, routine, data, 0, 0) };
    if status != STATUS_SUCCESS {
        return fail(status, 0);
    }
    1
}

/// The current thread's TEB.
fn teb() -> usize {
    teb_u64(0x30) as usize
}

/// TEB.GuaranteedStackBytes (x64 offset 0x1748).
const TEB_GUARANTEED_STACK_BYTES: usize = 0x1748;
/// TEB.HardErrorMode (x64 offset 0x16b0).
const TEB_HARD_ERROR_MODE: usize = 0x16b0;

/// SetThreadStackGuarantee: raises the stack the thread keeps for its
/// stack-overflow handler (never lowers it) and returns the previous
/// guarantee; 0 only queries. Kept in the TEB; gala's guard page handling
/// does not reserve it yet (TODO).
#[unsafe(no_mangle)]
pub extern "system" fn SetThreadStackGuarantee(size: *mut u32) -> Bool {
    if size.is_null() {
        return fail_with(ERROR_INVALID_PARAMETER, FALSE);
    }
    let field = (teb() + TEB_GUARANTEED_STACK_BYTES) as *mut u32;
    // SAFETY: the caller's value; this thread's TEB.
    unsafe {
        let previous = *field;
        if *size > previous {
            *field = *size;
        }
        *size = previous;
    }
    TRUE
}

/// SetThreadErrorMode: SEM_FAILCRITICALERRORS, SEM_NOGPFAULTERRORBOX and
/// SEM_NOOPENFILEERRORBOX only (gala shows no error boxes either way).
#[unsafe(no_mangle)]
pub extern "system" fn SetThreadErrorMode(mode: u32, old: *mut u32) -> Bool {
    if mode & !(0x1 | 0x2 | 0x8000) != 0 {
        return fail_with(ERROR_INVALID_PARAMETER, FALSE);
    }
    let field = (teb() + TEB_HARD_ERROR_MODE) as *mut u32;
    // SAFETY: this thread's TEB; the caller's output.
    unsafe {
        if !old.is_null() {
            *old = *field;
        }
        *field = mode;
    }
    TRUE
}

#[unsafe(no_mangle)]
pub extern "system" fn GetThreadErrorMode() -> u32 {
    // SAFETY: this thread's TEB.
    unsafe { *((teb() + TEB_HARD_ERROR_MODE) as *const u32) }
}

/// QueryThreadCycleTime: the thread's processor time (kernel plus user,
/// in 100 ns units) scaled to cycles of a nominal 3 GHz clock, at least 1.
/// Windows counts real cycles; the documentation says not to convert the
/// value to time, so a scaled time is a faithful stand-in.
#[unsafe(no_mangle)]
pub extern "system" fn QueryThreadCycleTime(thread: Handle, cycles: *mut u64) -> Bool {
    let (mut created, mut exited, mut kernel, mut user) = (0u64, 0u64, 0u64, 0u64);
    if GetThreadTimes(thread, &mut created, &mut exited, &mut kernel, &mut user) == FALSE {
        return FALSE;
    }
    if cycles.is_null() {
        return fail_with(ERROR_INVALID_PARAMETER, FALSE);
    }
    // SAFETY: the caller's output.
    unsafe { *cycles = ((kernel + user) * 300).max(1) };
    TRUE
}

/// Pages locked with VirtualLock: (first page, end), as gala never pages
/// memory out locking only has to be remembered for VirtualUnlock.
const MAX_LOCKED: usize = 64;

struct Locked {
    lock: AtomicBool,
    ranges: core::cell::UnsafeCell<[(usize, usize); MAX_LOCKED]>,
}

// SAFETY: `ranges` is only touched with `lock` held.
unsafe impl Sync for Locked {}

static LOCKED: Locked = Locked {
    lock: AtomicBool::new(false),
    ranges: core::cell::UnsafeCell::new([(0, 0); MAX_LOCKED]),
};

fn with_locked<R>(f: impl FnOnce(&mut [(usize, usize); MAX_LOCKED]) -> R) -> R {
    while LOCKED
        .lock
        .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
        .is_err()
    {
        core::hint::spin_loop();
    }
    // SAFETY: the lock is held.
    let r = f(unsafe { &mut *LOCKED.ranges.get() });
    LOCKED.lock.store(false, Ordering::Release);
    r
}

fn page_range(address: *mut c_void, size: usize) -> (usize, usize) {
    let start = address as usize & !0xfff;
    (start, (address as usize + size.max(1) + 0xfff) & !0xfff)
}

/// VirtualLock: gala does not page memory out; the range is remembered.
#[unsafe(no_mangle)]
pub extern "system" fn VirtualLock(address: *mut c_void, size: usize) -> Bool {
    let range = page_range(address, size);
    let added = with_locked(|r| match r.iter_mut().find(|e| e.0 == 0) {
        Some(slot) => {
            *slot = range;
            true
        }
        None => false,
    });
    if !added {
        unsupported(b"kernel32.dll!VirtualLock(too many ranges)", MAX_LOCKED as u64);
    }
    TRUE
}

/// VirtualUnlock: ERROR_NOT_LOCKED unless the pages were locked.
#[unsafe(no_mangle)]
pub extern "system" fn VirtualUnlock(address: *mut c_void, size: usize) -> Bool {
    let (start, end) = page_range(address, size);
    let found =
        with_locked(|r| match r.iter_mut().find(|e| e.0 != 0 && e.0 <= start && end <= e.1) {
            Some(slot) => {
                *slot = (0, 0);
                true
            }
            None => false,
        });
    if found { TRUE } else { fail_with(ERROR_NOT_LOCKED, FALSE) }
}

/// CreateMemoryResourceNotification: memory is never low on gala's terms
/// (no paging), so the low-memory notification (0) is an event that stays
/// unsignaled and the high-memory one (1) one that stays signaled.
#[unsafe(no_mangle)]
pub extern "system" fn CreateMemoryResourceNotification(kind: u32) -> Handle {
    if kind > 1 {
        return fail_with(ERROR_INVALID_PARAMETER, core::ptr::null_mut());
    }
    CreateEventW(core::ptr::null_mut(), TRUE, Bool::from(kind == 1), core::ptr::null())
}

#[unsafe(no_mangle)]
pub extern "system" fn QueryMemoryResourceNotification(h: Handle, state: *mut Bool) -> Bool {
    if state.is_null() {
        return fail_with(ERROR_INVALID_PARAMETER, FALSE);
    }
    match WaitForSingleObjectEx(h, 0, FALSE) {
        0 => {
            // SAFETY: the caller's output.
            unsafe { *state = TRUE };
            TRUE
        }
        STATUS_TIMEOUT => {
            // SAFETY: as above.
            unsafe { *state = FALSE };
            TRUE
        }
        _ => FALSE,
    }
}

/// IsWow64Process2: every gala process is native x64
/// (IMAGE_FILE_MACHINE_UNKNOWN for the process, AMD64 for the machine).
#[unsafe(no_mangle)]
pub extern "system" fn IsWow64Process2(
    _process: Handle,
    machine: *mut u16,
    native: *mut u16,
) -> Bool {
    if machine.is_null() {
        return fail_with(ERROR_INVALID_PARAMETER, FALSE);
    }
    // SAFETY: the caller's outputs.
    unsafe {
        *machine = 0;
        if !native.is_null() {
            *native = 0x8664;
        }
    }
    TRUE
}

// ---- Job objects (the kernel's, nt::job) ----

const JOB_OBJECT_ALL_ACCESS: u32 = 0x1f_003f;
const STATUS_PROCESS_IN_JOB: u32 = 0x123;

/// CreateJobObjectW(lpJobAttributes, lpName): a new job, or the existing
/// one of that name (ERROR_ALREADY_EXISTS).
#[unsafe(no_mangle)]
pub extern "system" fn CreateJobObjectW(attributes: *mut c_void, name: *const u16) -> Handle {
    let mut h = core::ptr::null_mut();
    let status = super::sync::with_name(name, super::sync::inheritable(attributes), |oa| {
        // SAFETY: valid output pointer and attributes.
        unsafe { NtCreateJobObject(&mut h, JOB_OBJECT_ALL_ACCESS, oa.cast()) }
    });
    match status {
        Ok(status) => super::sync::created(status, h),
        Err(e) => fail_with(e, core::ptr::null_mut()),
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn CreateJobObjectA(attributes: *mut c_void, name: *const u8) -> Handle {
    let mut buf = [0u16; 260];
    match super::sync::wide_name(name, &mut buf) {
        Ok(name) => CreateJobObjectW(attributes, name),
        Err(e) => fail_with(e, core::ptr::null_mut()),
    }
}

/// AssignProcessToJobObject(hJob, hProcess)
#[unsafe(no_mangle)]
pub extern "system" fn AssignProcessToJobObject(job: Handle, process: Handle) -> Bool {
    // SAFETY: plain call.
    let status = unsafe { NtAssignProcessToJobObject(job, process) };
    if status == STATUS_SUCCESS { TRUE } else { fail(status, FALSE) }
}

/// SetInformationJobObject(hJob, class, info, length): the basic and
/// extended limits (the active-process limit is enforced).
#[unsafe(no_mangle)]
pub extern "system" fn SetInformationJobObject(
    job: Handle,
    class: u32,
    info: *const c_void,
    length: u32,
) -> Bool {
    // SAFETY: the caller's buffer of `length` bytes.
    let status = unsafe { NtSetInformationJobObject(job, class, info, length) };
    if status == STATUS_SUCCESS { TRUE } else { fail(status, FALSE) }
}

/// QueryInformationJobObject(hJob, class, info, length, returned): basic
/// accounting, basic and extended limits; a null job is the caller's.
#[unsafe(no_mangle)]
pub extern "system" fn QueryInformationJobObject(
    job: Handle,
    class: u32,
    info: *mut c_void,
    length: u32,
    returned: *mut u32,
) -> Bool {
    // SAFETY: the caller's buffer of `length` bytes.
    let status = unsafe { NtQueryInformationJobObject(job, class, info, length, returned) };
    if status == STATUS_SUCCESS { TRUE } else { fail(status, FALSE) }
}

/// IsProcessInJob(ProcessHandle, JobHandle, Result): a null job means any.
#[unsafe(no_mangle)]
pub extern "system" fn IsProcessInJob(process: Handle, job: Handle, result: *mut Bool) -> Bool {
    if result.is_null() {
        return fail_with(ERROR_INVALID_PARAMETER, FALSE);
    }
    // SAFETY: plain call.
    let status = unsafe { NtIsProcessInJob(process, job) };
    if status & 0x8000_0000 != 0 {
        return fail(status, FALSE);
    }
    // SAFETY: the caller's output.
    unsafe { *result = Bool::from(status == STATUS_PROCESS_IN_JOB) };
    TRUE
}

// ---- Working sets ----

/// The working set limits SetProcessWorkingSetSize last set; Windows'
/// defaults (50 and 345 pages) before that. gala does not trim working
/// sets, so they are only reported back.
static WORKING_SET: [core::sync::atomic::AtomicUsize; 2] = [
    core::sync::atomic::AtomicUsize::new(50 * 4096),
    core::sync::atomic::AtomicUsize::new(345 * 4096),
];

/// SetProcessWorkingSetSize(hProcess, min, max): (SIZE_T)-1 for both
/// trims the working set (nothing to do here).
#[unsafe(no_mangle)]
pub extern "system" fn SetProcessWorkingSetSize(_process: Handle, min: usize, max: usize) -> Bool {
    if min == usize::MAX && max == usize::MAX {
        return TRUE;
    }
    if min > max {
        return fail_with(ERROR_INVALID_PARAMETER, FALSE);
    }
    WORKING_SET[0].store(min, Ordering::Relaxed);
    WORKING_SET[1].store(max, Ordering::Relaxed);
    TRUE
}

/// GetProcessWorkingSetSize(hProcess, *min, *max)
#[unsafe(no_mangle)]
pub extern "system" fn GetProcessWorkingSetSize(
    _process: Handle,
    min: *mut usize,
    max: *mut usize,
) -> Bool {
    if min.is_null() || max.is_null() {
        return fail_with(ERROR_INVALID_PARAMETER, FALSE);
    }
    // SAFETY: the caller's outputs.
    unsafe {
        *min = WORKING_SET[0].load(Ordering::Relaxed);
        *max = WORKING_SET[1].load(Ordering::Relaxed);
    }
    TRUE
}

/// VirtualQueryEx(hProcess, address, buffer, length): VirtualQuery of the
/// process the handle names (NtQueryVirtualMemory).
#[unsafe(no_mangle)]
pub extern "system" fn VirtualQueryEx(
    process: Handle,
    address: *const c_void,
    info: *mut u8,
    length: usize,
) -> usize {
    let mut ret = 0;
    // SAFETY: the caller's buffer of `length` bytes.
    let status = unsafe {
        super::NtQueryVirtualMemory(process as isize, address as usize, 0, info, length, &mut ret)
    };
    if status != STATUS_SUCCESS { fail(status, 0) } else { ret }
}

#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "system" {
    fn NtCreateJobObject(out: *mut Handle, access: u32, attributes: *const c_void) -> u32;
    fn NtAssignProcessToJobObject(job: Handle, process: Handle) -> u32;
    fn NtSetInformationJobObject(job: Handle, class: u32, info: *const c_void, len: u32) -> u32;
    fn NtQueryInformationJobObject(
        job: Handle,
        class: u32,
        info: *mut c_void,
        len: u32,
        ret: *mut u32,
    ) -> u32;
    fn NtIsProcessInJob(process: Handle, job: Handle) -> u32;
    fn NtQueueApcThread(thread: Handle, routine: usize, a1: usize, a2: usize, a3: usize) -> u32;
    fn NtFsControlFile(
        h: Handle,
        event: usize,
        apc: usize,
        context: usize,
        iosb: *mut [usize; 2],
        code: u32,
        input: *const u8,
        input_length: u32,
        output: *mut u8,
        output_length: u32,
    ) -> u32;
}

/// A named-pipe server control (FSCTL_PIPE_*): the file system answers
/// (gala's pipes are anonymous; other handles are not pipes).
fn pipe_control(pipe: Handle, code: u32, overlapped: *mut c_void) -> Bool {
    let mut iosb = [0usize; 2];
    let event = if overlapped.is_null() {
        0
    } else {
        // SAFETY: the caller's OVERLAPPED (hEvent at offset 24).
        unsafe { *(overlapped.cast::<u8>().add(24) as *const usize) }
    };
    // SAFETY: plain call with our status block.
    let status = unsafe {
        NtFsControlFile(
            pipe,
            event,
            0,
            0,
            &mut iosb,
            code,
            core::ptr::null(),
            0,
            core::ptr::null_mut(),
            0,
        )
    };
    if status != STATUS_SUCCESS {
        return fail(status, FALSE);
    }
    TRUE
}

/// ConnectNamedPipe: FSCTL_PIPE_LISTEN.
#[unsafe(no_mangle)]
pub extern "system" fn ConnectNamedPipe(pipe: Handle, overlapped: *mut c_void) -> Bool {
    if pipe.is_null() {
        return fail_with(ERROR_INVALID_HANDLE, FALSE);
    }
    pipe_control(pipe, 0x0011_0008, overlapped)
}

/// DisconnectNamedPipe: FSCTL_PIPE_DISCONNECT.
#[unsafe(no_mangle)]
pub extern "system" fn DisconnectNamedPipe(pipe: Handle) -> Bool {
    if pipe.is_null() {
        return fail_with(ERROR_INVALID_HANDLE, FALSE);
    }
    pipe_control(pipe, 0x0011_0004, core::ptr::null_mut())
}

/// WerRegisterRuntimeExceptionModule: there is no Windows Error Reporting
/// to call the module back; registering succeeds (S_OK).
#[unsafe(no_mangle)]
pub extern "system" fn WerRegisterRuntimeExceptionModule(
    _dll: *const u16,
    _context: *mut c_void,
) -> i32 {
    0
}

#[unsafe(no_mangle)]
pub extern "system" fn WerUnregisterRuntimeExceptionModule(
    _dll: *const u16,
    _context: *mut c_void,
) -> i32 {
    0
}

/// FlushInstructionCache: x86 keeps instruction caches coherent with
/// stores; nothing to do.
#[unsafe(no_mangle)]
pub extern "system" fn FlushInstructionCache(
    _process: Handle,
    _base: *const c_void,
    _size: usize,
) -> Bool {
    TRUE
}

/// Thread descriptions (SetThreadDescription), by thread id: up to 64
/// threads, 63 characters each (longer ones are cut).
const MAX_NAMED: usize = 64;

struct Names {
    lock: AtomicBool,
    names: core::cell::UnsafeCell<[(u32, [u16; 64]); MAX_NAMED]>,
}

// SAFETY: `names` is only touched with `lock` held.
unsafe impl Sync for Names {}

static NAMES: Names = Names {
    lock: AtomicBool::new(false),
    names: core::cell::UnsafeCell::new([(0, [0; 64]); MAX_NAMED]),
};

fn with_names<R>(f: impl FnOnce(&mut [(u32, [u16; 64]); MAX_NAMED]) -> R) -> R {
    while NAMES
        .lock
        .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
        .is_err()
    {
        core::hint::spin_loop();
    }
    // SAFETY: the lock is held.
    let r = f(unsafe { &mut *NAMES.names.get() });
    NAMES.lock.store(false, Ordering::Release);
    r
}

/// HRESULT_FROM_WIN32 of the last error.
fn last_hresult() -> i32 {
    ((GetLastError() & 0xffff) | 0x8007_0000) as i32
}

#[unsafe(no_mangle)]
pub extern "system" fn SetThreadDescription(thread: Handle, text: *const u16) -> i32 {
    let id = super::process::GetThreadId(thread);
    if id == 0 {
        return last_hresult();
    }
    if text.is_null() {
        return 0x8007_0057u32 as i32; // E_INVALIDARG
    }
    let mut name = [0u16; 64];
    // SAFETY: the caller's NUL-terminated string.
    unsafe {
        for (i, slot) in name.iter_mut().take(63).enumerate() {
            let ch = *text.add(i);
            if ch == 0 {
                break;
            }
            *slot = ch;
        }
    }
    let stored = with_names(|n| {
        let slot = n.iter().position(|e| e.0 == id).or_else(|| n.iter().position(|e| e.0 == 0));
        slot.map(|i| n[i] = (id, name)).is_some()
    });
    if !stored {
        unsupported(b"kernel32.dll!SetThreadDescription(too many threads)", MAX_NAMED as u64);
    }
    0
}

/// GetThreadDescription: a LocalAlloc'ed copy (empty if none was set).
#[unsafe(no_mangle)]
pub extern "system" fn GetThreadDescription(thread: Handle, out: *mut *mut u16) -> i32 {
    let id = super::process::GetThreadId(thread);
    if id == 0 {
        return last_hresult();
    }
    if out.is_null() {
        return 0x8007_0057u32 as i32;
    }
    let name = with_names(|n| n.iter().find(|e| e.0 == id).map_or([0; 64], |e| e.1));
    let p = super::heap::LocalAlloc(0, 128).cast::<u16>();
    if p.is_null() {
        return 0x8007_000eu32 as i32; // E_OUTOFMEMORY
    }
    // SAFETY: a fresh block of 64 characters; the caller's output.
    unsafe {
        core::ptr::copy_nonoverlapping(name.as_ptr(), p, 64);
        *out = p;
    }
    0
}

#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "system" {
    fn NtQuerySystemInformation(class: u32, buf: *mut u8, len: u32, ret: *mut u32) -> u32;
}

/// GetSystemTimes: FILETIMEs summed over the processors. gala does not
/// account processor time yet (TODO: in the scheduler), so every
/// processor counts as idle since boot: kernel time (which includes idle
/// time, as documented) equals idle time and user time is 0. Utilization
/// computed from these reads as 0%.
#[unsafe(no_mangle)]
pub extern "system" fn GetSystemTimes(idle: *mut u64, kernel: *mut u64, user: *mut u64) -> Bool {
    // SYSTEM_TIME_OF_DAY_INFORMATION: BootTime, CurrentTime, ...
    let mut tod = [0u64; 6];
    // SAFETY: a 48-byte buffer.
    let status =
        unsafe { NtQuerySystemInformation(3, tod.as_mut_ptr().cast(), 48, core::ptr::null_mut()) };
    if status != STATUS_SUCCESS {
        return fail(status, FALSE);
    }
    let mut basic = [0u8; 64];
    // SAFETY: a 64-byte SYSTEM_BASIC_INFORMATION buffer.
    unsafe { NtQuerySystemInformation(0, basic.as_mut_ptr(), 64, core::ptr::null_mut()) };
    let processors = u64::from(basic[56].max(1));
    let elapsed = tod[1].saturating_sub(tod[0]).max(1) * processors;
    // SAFETY: the caller's outputs (each may be null).
    unsafe {
        if !idle.is_null() {
            *idle = elapsed;
        }
        if !kernel.is_null() {
            *kernel = elapsed;
        }
        if !user.is_null() {
            *user = 0;
        }
    }
    TRUE
}
