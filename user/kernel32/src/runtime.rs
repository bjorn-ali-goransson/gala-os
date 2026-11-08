//! What language runtimes (CoreCLR) use beyond the basics: alertable
//! multi-object waits, SignalObjectAndWait, user APCs, stack guarantees,
//! per-thread error modes, thread cycle times, locked pages, memory
//! resource notifications, IsWow64Process2, jobs, the named-pipe server
//! calls, WER runtime modules and FlushInstructionCache. As documented
//! for each; gala's limits are noted where they apply.

use core::ffi::c_void;
use core::sync::atomic::{AtomicBool, Ordering};

use super::arch::*;
use super::files::WaitForSingleObjectEx;
use super::process::GetThreadTimes;
use super::sync::{CreateEventW, ReleaseMutex, ReleaseSemaphore};
use super::{
    Bool, FALSE, GetLastError, Handle, NtWaitForMultipleObjects, STATUS_SUCCESS, STATUS_TIMEOUT,
    SetEvent, TRUE, fail, fail_with, timeout, unsupported,
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
pub(crate) fn teb() -> usize {
    teb_read(TEB_SELF)
}

/// GetCurrentThreadStackLimits: the stack the system reserved for the
/// thread, from the TEB's DeallocationStack (the reservation's base) to
/// its StackBase.
#[unsafe(no_mangle)]
pub extern "system" fn GetCurrentThreadStackLimits(low: *mut usize, high: *mut usize) {
    // SAFETY: the caller passes writable pointers, as documented.
    unsafe {
        low.write(teb_read(TEB_DEALLOCATION_STACK));
        high.write(teb_read(TEB_STACK_BASE));
    }
}

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

/// IsWow64GuestMachineSupported(machine, *supported): whether WOW64 runs
/// programs of that machine type: x86 (IMAGE_FILE_MACHINE_I386), as on
/// 64-bit Windows on x64 (ARCHITECTURE "Plan: WOW64"); no other.
#[unsafe(no_mangle)]
pub extern "system" fn IsWow64GuestMachineSupported(machine: u16, supported: *mut Bool) -> i32 {
    const E_INVALIDARG: i32 = 0x8007_0057u32 as i32;
    if supported.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: the caller's output.
    unsafe { *supported = Bool::from(machine == IMAGE_FILE_MACHINE_I386) };
    0
}

const IMAGE_FILE_MACHINE_I386: u16 = 0x14c;
const IMAGE_FILE_MACHINE_AMD64: u16 = 0x8664;

/// IsWow64Process2(process, *processMachine, *nativeMachine): the
/// process's machine if it runs through WOW64 (x86), else
/// IMAGE_FILE_MACHINE_UNKNOWN; the native machine is AMD64.
#[unsafe(no_mangle)]
pub extern "system" fn IsWow64Process2(
    process: Handle,
    machine: *mut u16,
    native: *mut u16,
) -> Bool {
    if machine.is_null() {
        return fail_with(ERROR_INVALID_PARAMETER, FALSE);
    }
    let wow64 = match super::process::is_wow64(process) {
        Ok(w) => w,
        Err(status) => return fail(status, FALSE),
    };
    // SAFETY: the caller's outputs.
    unsafe {
        *machine = if wow64 { IMAGE_FILE_MACHINE_I386 } else { 0 };
        if !native.is_null() {
            *native = IMAGE_FILE_MACHINE_AMD64;
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

#[cfg_attr(
    target_arch = "x86",
    link(name = "ntdll", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "ntdll", kind = "raw-dylib"))]
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

/// A named-pipe server control (FSCTL_PIPE_*) with no buffers. With an
/// OVERLAPPED it may pend (ERROR_IO_PENDING); without one, a request
/// that pends on an overlapped handle is waited for.
fn pipe_control(pipe: Handle, code: u32, overlapped: *mut c_void) -> Bool {
    let issue = |event: usize, context: usize, iosb: *mut [usize; 2]| {
        // SAFETY: a status block valid until the request completes.
        unsafe {
            NtFsControlFile(
                pipe,
                event,
                0,
                context,
                iosb,
                code,
                core::ptr::null(),
                0,
                core::ptr::null_mut(),
                0,
            )
        }
    };
    if !overlapped.is_null() {
        return crate::overlapped_request(
            core::ptr::null_mut(),
            overlapped,
            |event, context, iosb, _| issue(event, context, iosb),
        );
    }
    let mut iosb = [0usize; 2];
    let mut status = issue(0, 0, &mut iosb);
    if status == crate::STATUS_PENDING {
        // SAFETY: waits on the pipe handle, signaled when it completes.
        status = unsafe { crate::NtWaitForSingleObject(pipe, 0, core::ptr::null()) };
        if status == STATUS_SUCCESS {
            status = iosb[0] as u32;
        }
    }
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

/// HRESULT_FROM_NT of a failed status (the NTSTATUS with FACILITY_NT_BIT).
fn nt_hresult(status: u32) -> i32 {
    (status | 0x1000_0000) as i32
}

/// SetThreadDescription: the thread's name, kept by the kernel
/// (NtSetInformationThread ThreadNameInformation), so other processes,
/// the state dump and the event stream see it too.
#[unsafe(no_mangle)]
pub extern "system" fn SetThreadDescription(thread: Handle, text: *const u16) -> i32 {
    if text.is_null() {
        return 0x8007_0057u32 as i32; // E_INVALIDARG
    }
    let mut n = 0usize;
    // SAFETY: the caller's NUL-terminated string.
    while n < 0x7fff && unsafe { *text.add(n) } != 0 {
        n += 1;
    }
    // THREAD_NAME_INFORMATION { UNICODE_STRING }: Length, MaximumLength,
    // then the pointer at its natural alignment (16 bytes on 64-bit, 8 on
    // 32-bit).
    const P: usize = size_of::<usize>();
    let mut us = [0u8; 2 * P];
    us[0..2].copy_from_slice(&((n * 2) as u16).to_le_bytes());
    us[2..4].copy_from_slice(&((n * 2) as u16).to_le_bytes());
    us[P..2 * P].copy_from_slice(&(text as usize).to_le_bytes());
    // SAFETY: a THREAD_NAME_INFORMATION pointing at the caller's string.
    let status = unsafe { super::NtSetInformationThread(thread, 38, us.as_ptr(), (2 * P) as u32) };
    // Success too is HRESULT_FROM_NT(STATUS_SUCCESS), 0x10000000, not S_OK
    // (Windows 11 25H2).
    nt_hresult(status)
}

/// GetThreadDescription: a LocalAlloc'ed NUL-terminated copy of the
/// thread's name (empty if none was set).
#[unsafe(no_mangle)]
pub extern "system" fn GetThreadDescription(thread: Handle, out: *mut *mut u16) -> i32 {
    if out.is_null() {
        return 0x8007_0057u32 as i32;
    }
    // THREAD_NAME_INFORMATION and up to 256 characters (the kernel keeps
    // no more).
    let mut info = [0u64; 2 + 64];
    let mut ret = 0u32;
    // SAFETY: our buffer of the size given.
    let status = unsafe {
        super::NtQueryInformationThread(
            thread,
            38,
            info.as_mut_ptr().cast(),
            core::mem::size_of_val(&info) as u32,
            &mut ret,
        )
    };
    if status != STATUS_SUCCESS {
        return nt_hresult(status);
    }
    let chars = (info[0] & 0xffff) as usize / 2;
    // SAFETY: the UNICODE_STRING the kernel wrote at the start of `info`;
    // its Buffer, after Length and MaximumLength at pointer alignment.
    let text =
        unsafe { info.as_ptr().cast::<u8>().add(size_of::<usize>()).cast::<*const u16>().read() };
    let p = super::global::LocalAlloc(0, (chars + 1) * 2).cast::<u16>();
    if p.is_null() {
        return 0x8007_000eu32 as i32; // E_OUTOFMEMORY
    }
    // SAFETY: the characters the kernel wrote into `info` (Buffer points
    // there); a fresh block of chars + 1; the caller's output.
    unsafe {
        core::ptr::copy_nonoverlapping(text, p, chars);
        *p.add(chars) = 0;
        *out = p;
    }
    // HRESULT_FROM_NT(STATUS_SUCCESS), 0x10000000, as SetThreadDescription
    // (Windows 11 25H2).
    nt_hresult(STATUS_SUCCESS)
}

#[cfg_attr(
    target_arch = "x86",
    link(name = "ntdll", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "ntdll", kind = "raw-dylib"))]
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
    let processors = u64::from(super::basic_information().processors.max(1));
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

#[cfg_attr(
    target_arch = "x86",
    link(name = "ntdll", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "ntdll", kind = "raw-dylib"))]
unsafe extern "system" {
    fn NtSetInformationProcess(process: Handle, class: u32, buf: *const u8, len: u32) -> u32;
}

/// ProcessPriorityClass.
const PROCESS_PRIORITY_CLASS: u32 = 18;
/// (Win32 priority class, PROCESS_PRIORITY_CLASS_* value).
/// The classes' bits from the lowest class up: the first one a value has
/// is its class (SetPriorityClass).
const PRIORITY_ORDER: [(u32, u8); 6] = [
    (0x40, 1),   // IDLE
    (0x4000, 5), // BELOW_NORMAL
    (0x20, 2),   // NORMAL
    (0x8000, 6), // ABOVE_NORMAL
    (0x80, 3),   // HIGH
    (0x100, 4),  // REALTIME
];

const PRIORITY_CLASSES: [(u32, u8); 6] = [
    (0x40, 1),   // IDLE
    (0x20, 2),   // NORMAL
    (0x80, 3),   // HIGH
    (0x100, 4),  // REALTIME
    (0x4000, 5), // BELOW_NORMAL
    (0x8000, 6), // ABOVE_NORMAL
];

/// GetPriorityClass: the process's priority class (0 on failure).
#[unsafe(no_mangle)]
pub extern "system" fn GetPriorityClass(process: Handle) -> u32 {
    let mut info = [0u8; 2];
    // SAFETY: our buffer of the given size.
    let status = unsafe {
        super::NtQueryInformationProcess(
            process as isize,
            PROCESS_PRIORITY_CLASS,
            info.as_mut_ptr().cast(),
            2,
            core::ptr::null_mut(),
        )
    };
    if status != STATUS_SUCCESS {
        return fail(status, 0);
    }
    PRIORITY_CLASSES.iter().find(|&&(_, nt)| nt == info[1]).map_or(0x20, |&(win32, _)| win32)
}

/// SetPriorityClass: records the process's priority class (gala's
/// scheduler runs every process at the normal base priority). Background
/// mode is not supported (recorded).
#[unsafe(no_mangle)]
pub extern "system" fn SetPriorityClass(process: Handle, class: u32) -> Bool {
    if matches!(class, 0x0010_0000 | 0x0020_0000) {
        unsupported(b"kernel32.dll!SetPriorityClass(background mode)", class.into());
        return fail_with(super::ERROR_NOT_SUPPORTED, 0);
    }
    // As Windows: the bits that are no class are ignored and, of the
    // class bits there, the lowest class wins (measured over every pair:
    // IDLE beats all, REALTIME none); a value with no class bit (0 too) is
    // ERROR_INVALID_PARAMETER.
    let Some(&(_, nt)) = PRIORITY_ORDER.iter().find(|&&(bit, _)| class & bit != 0) else {
        return fail_with(ERROR_INVALID_PARAMETER, 0);
    };
    let info = [0u8, nt];
    // SAFETY: our buffer of the given size.
    let mut status =
        unsafe { NtSetInformationProcess(process, PROCESS_PRIORITY_CLASS, info.as_ptr(), 2) };
    // REALTIME without SeIncreaseBasePriorityPrivilege: TRUE, and the
    // class is HIGH (what Windows does for such a caller).
    const STATUS_PRIVILEGE_NOT_HELD: u32 = 0xc000_0061;
    if nt == 4 && status == STATUS_PRIVILEGE_NOT_HELD {
        let high = [0u8, 3];
        // SAFETY: as above.
        status =
            unsafe { NtSetInformationProcess(process, PROCESS_PRIORITY_CLASS, high.as_ptr(), 2) };
    }
    if status != STATUS_SUCCESS {
        return fail(status, 0);
    }
    1
}
