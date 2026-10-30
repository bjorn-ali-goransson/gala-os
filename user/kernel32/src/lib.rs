//! gala's kernel32.dll (first part, ROADMAP G1): the Win32 functions
//! programs import, written on top of ntdll's exports (gala's testntdll)
//! from Microsoft's public documentation of each function. On Windows the
//! test programs use the real kernel32; this one is only shipped on gala
//! (as `C:\Windows\System32\kernel32.dll`).
//!
//! Conventions, as documented for Win32: failures return FALSE/NULL (or
//! the documented failure value) and set the thread's last-error code
//! (TEB+0x68) to the Win32 error of the NTSTATUS (`RtlNtStatusToDosError`).
//! Parts of a function gala does not support yet fail with
//! ERROR_NOT_SUPPORTED (or ERROR_CALL_NOT_IMPLEMENTED) and are reported to
//! the kernel's missing-features ledger (`GalaRecordMissing`): never a
//! silent no-op.

#![no_std]
#![allow(non_snake_case)]
// Every export is a C-ABI function called by foreign code with the raw
// pointers its documentation describes; Rust callers do not exist.
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use core::ffi::c_void;

type Bool = i32;
type Handle = *mut c_void;

const TRUE: Bool = 1;
const FALSE: Bool = 0;
const CURRENT_PROCESS: isize = -1;
const STATUS_SUCCESS: u32 = 0;
const STATUS_TIMEOUT: u32 = 0x102;
const STATUS_PENDING: u32 = 0x103;
const STATUS_OBJECT_NAME_EXISTS: u32 = 0x4000_0000;
const ERROR_ALREADY_EXISTS: u32 = 183;
const ERROR_NOT_SUPPORTED: u32 = 50;
const ERROR_INVALID_PARAMETER: u32 = 87;
const WAIT_FAILED: u32 = 0xffff_ffff;
const INFINITE: u32 = 0xffff_ffff;

#[repr(C)]
struct UnicodeString {
    length: u16,
    maximum_length: u16,
    buffer: *const u16,
}

#[repr(C)]
struct AnsiString {
    length: u16,
    maximum_length: u16,
    buffer: *const u8,
}

#[repr(C)]
struct ObjectAttributes {
    length: u32,
    root_directory: usize,
    object_name: *const UnicodeString,
    attributes: u32,
    security_descriptor: usize,
    security_quality_of_service: usize,
}

// The C runtime helpers the compiler may call, exported by ntdll (as the
// real ntdll does). Only compiler-generated code calls them.
#[allow(dead_code)]
#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "C" {
    fn memset(dst: *mut u8, value: i32, n: usize) -> *mut u8;
    fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memcmp(a: *const u8, b: *const u8, n: usize) -> i32;
    fn strlen(s: *const u8) -> usize;
    // MSVC-target unwind tables reference it (gala's ntdll provides it).
    fn __CxxFrameHandler3();
}

/// MSVC-target codegen references this marker symbol directly, so it cannot
/// come from an import. (It becomes a harmless extra data export.)
#[unsafe(no_mangle)]
pub static _fltused: i32 = 0;

#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "system" {
    fn NtWaitForSingleObject(h: Handle, alertable: u8, timeout: *const i64) -> u32;
    fn NtWriteFile(
        h: Handle,
        event: usize,
        apc: usize,
        apc_context: usize,
        iosb: *mut [usize; 2],
        buffer: *const u8,
        length: u32,
        offset: *const i64,
        key: *const u32,
    ) -> u32;
    fn NtSetEvent(h: Handle, previous: *mut i32) -> u32;
    fn NtAllocateVirtualMemory(
        process: isize,
        base: *mut usize,
        zero_bits: usize,
        size: *mut usize,
        kind: u32,
        protect: u32,
    ) -> u32;
    fn NtFreeVirtualMemory(process: isize, base: *mut usize, size: *mut usize, kind: u32) -> u32;
    fn NtQueryPerformanceCounter(counter: *mut i64, frequency: *mut i64) -> u32;
    fn NtDelayExecution(alertable: u8, interval: *const i64) -> u32;
    fn NtCreateEvent(
        handle: *mut Handle,
        access: u32,
        attributes: *const ObjectAttributes,
        kind: u32,
        initial: u8,
    ) -> u32;
    fn NtClose(h: Handle) -> u32;
    fn NtTerminateProcess(process: isize, status: u32) -> u32;
    fn RtlNtStatusToDosError(status: u32) -> u32;
    fn LdrLoadDll(
        path: *const u16,
        flags: *const u32,
        name: *const UnicodeString,
        base: *mut Handle,
    ) -> u32;
    fn LdrGetProcedureAddress(
        base: Handle,
        name: *const AnsiString,
        ordinal: u32,
        address: *mut *mut c_void,
    ) -> u32;
    fn LdrUnloadDll(base: Handle) -> u32;
    fn GalaRecordMissing(name: *const u8, length: usize, what: u32, detail: u64) -> u32;
    fn NtQueryInformationProcess(
        process: isize,
        class: u32,
        buf: *mut u8,
        len: u32,
        ret: *mut u32,
    ) -> u32;
    fn NtQueryVirtualMemory(
        process: isize,
        address: usize,
        class: u32,
        buf: *mut u8,
        len: usize,
        ret: *mut usize,
    ) -> u32;
    fn NtQuerySystemInformation(class: u32, buf: *mut u8, len: u32, ret: *mut u32) -> u32;
    fn NtDuplicateObject(
        source_process: Handle,
        source: Handle,
        target_process: Handle,
        target: *mut Handle,
        access: u32,
        attributes: u32,
        options: u32,
    ) -> u32;
    fn NtYieldExecution() -> u32;
    fn NtQueryVolumeInformationFile(
        h: Handle,
        iosb: *mut [usize; 2],
        buf: *mut u8,
        len: u32,
        class: u32,
    ) -> u32;
    fn NtQueryInformationThread(
        h: Handle,
        class: u32,
        buf: *mut u8,
        len: u32,
        ret: *mut u32,
    ) -> u32;
    fn NtCreateThread(
        handle: *mut Handle,
        access: u32,
        attributes: *const ObjectAttributes,
        process: isize,
        client_id: *mut [usize; 2],
        context: *const Context,
        initial_teb: *const [usize; 5],
        suspended: u8,
    ) -> u32;
    fn NtResumeThread(h: Handle, previous: *mut u32) -> u32;
    fn NtSuspendThread(h: Handle, previous: *mut u32) -> u32;
    fn NtTerminateThread(h: isize, status: u32) -> u32;
    fn NtGetContextThread(h: Handle, context: *mut Context) -> u32;
    fn NtSetInformationFile(
        h: Handle,
        iosb: *mut [usize; 2],
        buf: *const u8,
        len: u32,
        class: u32,
    ) -> u32;
    fn NtCreateIoCompletion(
        handle: *mut Handle,
        access: u32,
        attributes: *const ObjectAttributes,
        count: u32,
    ) -> u32;
    fn NtSetIoCompletion(
        h: Handle,
        key: usize,
        apc_context: usize,
        status: u32,
        information: usize,
    ) -> u32;
    fn NtRemoveIoCompletion(
        h: Handle,
        key: *mut usize,
        apc_context: *mut usize,
        iosb: *mut [usize; 2],
        timeout: *const i64,
    ) -> u32;
    fn NtCreateTimer(
        handle: *mut Handle,
        access: u32,
        attributes: *const ObjectAttributes,
        kind: u32,
    ) -> u32;
    fn NtSetTimer(
        h: Handle,
        due: *const i64,
        routine: usize,
        context: usize,
        resume: u8,
        period: i32,
        previous: *mut u8,
    ) -> u32;
    fn RtlAddVectoredExceptionHandler(first: u32, handler: usize) -> *mut c_void;
    fn RtlRemoveVectoredExceptionHandler(cookie: *mut c_void) -> u32;
    fn RtlAddVectoredContinueHandler(first: u32, handler: usize) -> *mut c_void;
    fn RtlRemoveVectoredContinueHandler(cookie: *mut c_void) -> u32;
    fn NtSetContextThread(h: Handle, context: *const Context) -> u32;
    fn NtWaitForMultipleObjects(
        count: u32,
        handles: *const Handle,
        wait_type: u32,
        alertable: u8,
        timeout: *const i64,
    ) -> u32;
}

/// Reads a field of the current TEB.
fn teb_u64(offset: usize) -> u64 {
    let v: u64;
    // SAFETY: gs:[offset] is inside the current thread's TEB.
    unsafe {
        core::arch::asm!("mov {}, gs:[{}]", out(reg) v, in(reg) offset, options(nostack, readonly))
    };
    v
}

/// Reports an unsupported part of `function` to the missing-features ledger.
fn unsupported(function: &[u8], detail: u64) {
    // SAFETY: a valid buffer; the service only reads it.
    unsafe { GalaRecordMissing(function.as_ptr(), function.len(), 2, detail) };
}

/// Sets the last error from an NTSTATUS and returns `value` (the failure
/// value of the caller).
fn fail<T>(status: u32, value: T) -> T {
    // SAFETY: plain function call.
    SetLastError(unsafe { RtlNtStatusToDosError(status) });
    value
}

fn fail_with<T>(error: u32, value: T) -> T {
    SetLastError(error);
    value
}

/// An NT relative timeout for `ms` milliseconds (None = INFINITE).
fn timeout(ms: u32) -> Option<i64> {
    (ms != INFINITE).then(|| -(i64::from(ms) * 10_000))
}

#[unsafe(no_mangle)]
pub extern "system" fn GetLastError() -> u32 {
    teb_u64(0x68) as u32
}

#[unsafe(no_mangle)]
pub extern "system" fn SetLastError(code: u32) {
    // SAFETY: TEB.LastErrorValue (x64 offset 0x68) of the current thread.
    unsafe { core::arch::asm!("mov gs:[0x68], {:e}", in(reg) code, options(nostack)) };
}

#[unsafe(no_mangle)]
pub extern "system" fn GetCurrentProcessId() -> u32 {
    teb_u64(0x40) as u32 // TEB.ClientId.UniqueProcess
}

#[unsafe(no_mangle)]
pub extern "system" fn GetCurrentThreadId() -> u32 {
    teb_u64(0x48) as u32 // TEB.ClientId.UniqueThread
}

#[unsafe(no_mangle)]
pub extern "system" fn GetCurrentProcess() -> Handle {
    CURRENT_PROCESS as Handle
}

#[unsafe(no_mangle)]
pub extern "system" fn GetCurrentThread() -> Handle {
    -2isize as Handle
}

/// GetStdHandle(STD_INPUT_HANDLE = -10, OUTPUT = -11, ERROR = -12): from
/// the process parameters.
#[unsafe(no_mangle)]
pub extern "system" fn GetStdHandle(which: u32) -> Handle {
    let offset = match which as i32 {
        -10 => 0x20,
        -11 => 0x28,
        -12 => 0x30,
        _ => return fail_with(6, -1isize as Handle), // INVALID_HANDLE_VALUE
    };
    // SAFETY: gs:[0x60] is the PEB; ProcessParameters (+0x20) is always set.
    unsafe {
        let peb = teb_u64(0x60) as usize;
        let params = *((peb + 0x20) as *const usize);
        *((params + offset) as *const Handle)
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn WriteFile(
    h: Handle,
    buffer: *const u8,
    length: u32,
    written: *mut u32,
    overlapped: *mut c_void,
) -> Bool {
    if !overlapped.is_null() {
        unsupported(b"kernel32.dll!WriteFile(overlapped)", 0);
        return fail_with(ERROR_NOT_SUPPORTED, FALSE);
    }
    let mut iosb = [0usize; 2];
    // SAFETY: the caller's buffer; a valid IO_STATUS_BLOCK.
    let mut status = unsafe {
        NtWriteFile(h, 0, 0, 0, &mut iosb, buffer, length, core::ptr::null(), core::ptr::null())
    };
    if status == STATUS_PENDING {
        // SAFETY: waits on the handle, as kernel32 does for synchronous use.
        status = unsafe { NtWaitForSingleObject(h, 0, core::ptr::null()) };
        if status == STATUS_SUCCESS {
            status = iosb[0] as u32;
        }
    }
    if !written.is_null() {
        // SAFETY: the caller's output pointer.
        unsafe { *written = iosb[1] as u32 };
    }
    if status != STATUS_SUCCESS { fail(status, FALSE) } else { TRUE }
}

#[unsafe(no_mangle)]
pub extern "system" fn VirtualAlloc(
    address: *mut c_void,
    size: usize,
    kind: u32,
    protect: u32,
) -> *mut c_void {
    let (mut base, mut size) = (address as usize, size);
    // SAFETY: valid in/out pointers.
    let status =
        unsafe { NtAllocateVirtualMemory(CURRENT_PROCESS, &mut base, 0, &mut size, kind, protect) };
    if status != STATUS_SUCCESS { fail(status, core::ptr::null_mut()) } else { base as *mut c_void }
}

#[unsafe(no_mangle)]
pub extern "system" fn VirtualFree(address: *mut c_void, size: usize, kind: u32) -> Bool {
    // MEM_RELEASE requires a zero size (documented).
    if kind == 0x8000 && size != 0 {
        return fail_with(ERROR_INVALID_PARAMETER, FALSE);
    }
    let (mut base, mut size) = (address as usize, size);
    // SAFETY: valid in/out pointers.
    let status = unsafe { NtFreeVirtualMemory(CURRENT_PROCESS, &mut base, &mut size, kind) };
    if status != STATUS_SUCCESS { fail(status, FALSE) } else { TRUE }
}

/// CreateEventW: named events are not supported yet (reported).
#[unsafe(no_mangle)]
pub extern "system" fn CreateEventW(
    attributes: *mut c_void,
    manual_reset: Bool,
    initial: Bool,
    name: *const u16,
) -> Handle {
    if !name.is_null() || !attributes.is_null() {
        unsupported(b"kernel32.dll!CreateEventW(name/attributes)", 0);
        return fail_with(ERROR_NOT_SUPPORTED, core::ptr::null_mut());
    }
    let mut h = core::ptr::null_mut();
    // NotificationEvent (0) for manual reset, else SynchronizationEvent (1).
    let kind = if manual_reset != 0 { 0 } else { 1 };
    // SAFETY: valid output pointer, no attributes.
    let status = unsafe {
        NtCreateEvent(&mut h, 0x1f_0003, core::ptr::null(), kind, u8::from(initial != 0))
    };
    match status {
        STATUS_SUCCESS => {
            SetLastError(0);
            h
        }
        STATUS_OBJECT_NAME_EXISTS => fail_with(ERROR_ALREADY_EXISTS, h),
        _ => fail(status, core::ptr::null_mut()),
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn SetEvent(h: Handle) -> Bool {
    // SAFETY: the kernel validates the handle.
    let status = unsafe { NtSetEvent(h, core::ptr::null_mut()) };
    if status != STATUS_SUCCESS { fail(status, FALSE) } else { TRUE }
}

/// WaitForSingleObject: WAIT_OBJECT_0, WAIT_ABANDONED (0x80), WAIT_TIMEOUT
/// (0x102) are the NTSTATUS values themselves; errors give WAIT_FAILED.
#[unsafe(no_mangle)]
pub extern "system" fn WaitForSingleObject(h: Handle, ms: u32) -> u32 {
    let t = timeout(ms);
    let p = t.as_ref().map_or(core::ptr::null(), |t| t as *const i64);
    // SAFETY: valid or null timeout pointer.
    let status = unsafe { NtWaitForSingleObject(h, 0, p) };
    match status {
        0 | 0x80 | STATUS_TIMEOUT => status,
        _ => fail(status, WAIT_FAILED),
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn CloseHandle(h: Handle) -> Bool {
    // SAFETY: the kernel validates the handle.
    let status = unsafe { NtClose(h) };
    if status != STATUS_SUCCESS { fail(status, FALSE) } else { TRUE }
}

#[unsafe(no_mangle)]
pub extern "system" fn Sleep(ms: u32) {
    let t = timeout(ms).unwrap_or(i64::MIN);
    // SAFETY: a valid interval pointer.
    unsafe { NtDelayExecution(0, &t) };
}

/// GetTickCount64: KUSER_SHARED_DATA's TickCount times TickCountMultiplier
/// (24-bit fixed point), as documented for the shared data page.
#[unsafe(no_mangle)]
pub extern "system" fn GetTickCount64() -> u64 {
    const KUSER: usize = 0x7ffe_0000;
    // SAFETY: KUSER_SHARED_DATA is mapped read-only in every process.
    unsafe {
        let multiplier = u64::from(core::ptr::read_volatile((KUSER + 0x004) as *const u32));
        loop {
            // KSYSTEM_TIME: LowPart, High1Time, High2Time (re-read on a tear).
            let high1 = core::ptr::read_volatile((KUSER + 0x324) as *const u32);
            let low = core::ptr::read_volatile((KUSER + 0x320) as *const u32);
            let high2 = core::ptr::read_volatile((KUSER + 0x328) as *const u32);
            if high1 == high2 {
                let ticks = (u64::from(high1) << 32) | u64::from(low);
                return ((u128::from(ticks) * u128::from(multiplier)) >> 24) as u64;
            }
        }
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn GetTickCount() -> u32 {
    GetTickCount64() as u32
}

#[unsafe(no_mangle)]
pub extern "system" fn QueryPerformanceCounter(counter: *mut i64) -> Bool {
    // SAFETY: the caller's output pointer.
    let status = unsafe { NtQueryPerformanceCounter(counter, core::ptr::null_mut()) };
    if status != STATUS_SUCCESS { fail(status, FALSE) } else { TRUE }
}

#[unsafe(no_mangle)]
pub extern "system" fn QueryPerformanceFrequency(frequency: *mut i64) -> Bool {
    let mut counter = 0;
    // SAFETY: valid output pointers.
    let status = unsafe { NtQueryPerformanceCounter(&mut counter, frequency) };
    if status != STATUS_SUCCESS { fail(status, FALSE) } else { TRUE }
}

/// LoadLibraryA: ASCII names only so far (others are reported).
#[unsafe(no_mangle)]
pub extern "system" fn LoadLibraryA(name: *const u8) -> Handle {
    let mut wide = [0u16; 260];
    let mut n = 0;
    // SAFETY: the caller passes a NUL-terminated string.
    unsafe {
        while *name.add(n) != 0 {
            let b = *name.add(n);
            if n == wide.len() || !b.is_ascii() {
                unsupported(b"kernel32.dll!LoadLibraryA(long or non-ASCII name)", 0);
                return fail_with(ERROR_INVALID_PARAMETER, core::ptr::null_mut());
            }
            wide[n] = u16::from(b);
            n += 1;
        }
    }
    load_library(wide[..n].as_ptr(), n, 0)
}

/// Loads a library by UTF-16 name of `len` characters; `search` holds the
/// LOAD_LIBRARY_SEARCH_* flags (passed to LdrLoadDll in its path argument
/// with the low bit set, as newer Windows does).
fn load_library(name: *const u16, len: usize, search: u32) -> Handle {
    let us =
        UnicodeString { length: (len * 2) as u16, maximum_length: (len * 2) as u16, buffer: name };
    let path = if search != 0 { (search as usize | 1) as *const u16 } else { core::ptr::null() };
    let mut base = core::ptr::null_mut();
    // SAFETY: valid UNICODE_STRING and output pointer.
    let status = unsafe { LdrLoadDll(path, core::ptr::null(), &us, &mut base) };
    if status != STATUS_SUCCESS { fail(status, core::ptr::null_mut()) } else { base }
}

/// Length of a NUL-terminated UTF-16 string.
fn wide_len(s: *const u16) -> usize {
    let mut n = 0;
    // SAFETY: callers pass NUL-terminated strings.
    unsafe {
        while core::ptr::read_volatile(s.add(n)) != 0 {
            n += 1;
        }
    }
    n
}

#[unsafe(no_mangle)]
pub extern "system" fn LoadLibraryW(name: *const u16) -> Handle {
    load_library(name, wide_len(name), 0)
}

/// LoadLibraryExW: no data/image-resource loading yet (reported); the
/// LOAD_LIBRARY_SEARCH_* flags choose where to look.
#[unsafe(no_mangle)]
pub extern "system" fn LoadLibraryExW(name: *const u16, file: Handle, flags: u32) -> Handle {
    const SEARCH_FLAGS: u32 = 0x0000_1f00; // LOAD_LIBRARY_SEARCH_* (0x100..0x1000)
    if !file.is_null() || flags & !SEARCH_FLAGS != 0 {
        unsupported(b"kernel32.dll!LoadLibraryExW(flags)", u64::from(flags));
        return fail_with(ERROR_INVALID_PARAMETER, core::ptr::null_mut());
    }
    // LOAD_LIBRARY_SEARCH_SYSTEM32 alone restricts the search to System32;
    // the default-directories flags include the application directory.
    let search = if flags & SEARCH_FLAGS == 0x800 { 0x800 } else { 0 };
    load_library(name, wide_len(name), search)
}

/// GetProcAddress: by name, or by ordinal when `name` < 0x10000.
#[unsafe(no_mangle)]
pub extern "system" fn GetProcAddress(module: Handle, name: *const u8) -> *mut c_void {
    let mut address = core::ptr::null_mut();
    let status = if (name as usize) < 0x1_0000 {
        // SAFETY: a null name selects the ordinal.
        unsafe { LdrGetProcedureAddress(module, core::ptr::null(), name as u32, &mut address) }
    } else {
        let mut n = 0;
        // SAFETY: the caller passes a NUL-terminated string.
        unsafe {
            while *name.add(n) != 0 {
                n += 1;
            }
        }
        let ansi = AnsiString { length: n as u16, maximum_length: n as u16, buffer: name };
        // SAFETY: valid ANSI_STRING and output pointer.
        unsafe { LdrGetProcedureAddress(module, &ansi, 0, &mut address) }
    };
    if status != STATUS_SUCCESS { fail(status, core::ptr::null_mut()) } else { address }
}

#[unsafe(no_mangle)]
pub extern "system" fn FreeLibrary(module: Handle) -> Bool {
    // SAFETY: the loader validates the module.
    let status = unsafe { LdrUnloadDll(module) };
    if status != STATUS_SUCCESS { fail(status, FALSE) } else { TRUE }
}

// ---- Console (gala has no console devices: like redirected output) ----

const ERROR_INVALID_HANDLE: u32 = 6;

/// GetConsoleMode: only console handles have a mode; gala's standard
/// handles are files or devices, as with redirected output on Windows.
#[unsafe(no_mangle)]
pub extern "system" fn GetConsoleMode(_h: Handle, _mode: *mut u32) -> Bool {
    fail_with(ERROR_INVALID_HANDLE, FALSE)
}

/// WriteConsoleW: as GetConsoleMode (callers fall back to WriteFile).
#[unsafe(no_mangle)]
pub extern "system" fn WriteConsoleW(
    _h: Handle,
    _buffer: *const u16,
    _length: u32,
    _written: *mut u32,
    _reserved: *mut c_void,
) -> Bool {
    fail_with(ERROR_INVALID_HANDLE, FALSE)
}

/// SetConsoleCtrlHandler: kept (gala raises no console control events).
#[unsafe(no_mangle)]
pub extern "system" fn SetConsoleCtrlHandler(_handler: usize, _add: Bool) -> Bool {
    TRUE
}

/// GetFileType: from the device type of the handle's file object
/// (FileFsDeviceInformation): FILE_TYPE_DISK (1), FILE_TYPE_CHAR (2),
/// FILE_TYPE_PIPE (3), or FILE_TYPE_UNKNOWN (0) with the error set.
#[unsafe(no_mangle)]
pub extern "system" fn GetFileType(h: Handle) -> u32 {
    let (mut iosb, mut info) = ([0usize; 2], [0u8; 8]);
    // SAFETY: an 8-byte FILE_FS_DEVICE_INFORMATION buffer.
    let status = unsafe { NtQueryVolumeInformationFile(h, &mut iosb, info.as_mut_ptr(), 8, 4) };
    if status != STATUS_SUCCESS {
        return fail(status, 0);
    }
    match u32_at(&info, 0) {
        0x11 => 3,                                    // FILE_DEVICE_NAMED_PIPE
        0x02 | 0x03 | 0x07 | 0x08 | 0x24 => 1,        // CD-ROM, disks, file systems
        0x0b | 0x15 | 0x1b | 0x50 | 0x16 | 0x17 => 2, // keyboard, null, serial, console...
        _ => fail_with(0, 0),
    }
}

/// FormatMessageW: no message tables yet (reported). Callers fall back to
/// printing the number (Go does).
#[unsafe(no_mangle)]
pub extern "system" fn FormatMessageW(
    _flags: u32,
    _source: *const c_void,
    message: u32,
    _language: u32,
    _buffer: *mut u16,
    _size: u32,
    _args: *mut c_void,
) -> u32 {
    unsupported(b"kernel32.dll!FormatMessageW", u64::from(message));
    fail_with(317, 0) // ERROR_MR_MID_NOT_FOUND
}

// ---- Process-wide settings ----

static ERROR_MODE: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
static WER_FLAGS: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

#[unsafe(no_mangle)]
pub extern "system" fn SetErrorMode(mode: u32) -> u32 {
    ERROR_MODE.swap(mode, core::sync::atomic::Ordering::Relaxed)
}

#[unsafe(no_mangle)]
pub extern "system" fn GetErrorMode() -> u32 {
    ERROR_MODE.load(core::sync::atomic::Ordering::Relaxed)
}

/// WerSetFlags/WerGetFlags: kept for the process (gala has no error
/// reporting service to act on them). S_OK.
#[unsafe(no_mangle)]
pub extern "system" fn WerSetFlags(flags: u32) -> i32 {
    WER_FLAGS.store(flags, core::sync::atomic::Ordering::Relaxed);
    0
}

#[unsafe(no_mangle)]
pub extern "system" fn WerGetFlags(_process: Handle, flags: *mut u32) -> i32 {
    // SAFETY: the caller's output pointer.
    unsafe { *flags = WER_FLAGS.load(core::sync::atomic::Ordering::Relaxed) };
    0
}

/// SetProcessPriorityBoost: gala's scheduler has no priority boosts, so
/// disabling them (what callers ask) already holds.
#[unsafe(no_mangle)]
pub extern "system" fn SetProcessPriorityBoost(_process: Handle, _disable: Bool) -> Bool {
    TRUE
}

// ---- System information ----

#[repr(C)]
pub struct SystemInfo {
    processor_architecture: u16,
    reserved: u16,
    page_size: u32,
    minimum_application_address: usize,
    maximum_application_address: usize,
    active_processor_mask: usize,
    number_of_processors: u32,
    processor_type: u32,
    allocation_granularity: u32,
    processor_level: u16,
    processor_revision: u16,
}

/// SystemBasicInformation (64 bytes on x64).
fn basic_information() -> [u8; 64] {
    let mut b = [0u8; 64];
    let mut ret = 0;
    // SAFETY: a 64-byte buffer.
    unsafe { NtQuerySystemInformation(0, b.as_mut_ptr(), 64, &mut ret) };
    b
}

fn u64_at(b: &[u8], i: usize) -> u64 {
    u64::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3], b[i + 4], b[i + 5], b[i + 6], b[i + 7]])
}

fn u32_at(b: &[u8], i: usize) -> u32 {
    u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])
}

#[unsafe(no_mangle)]
pub extern "system" fn GetSystemInfo(info: *mut SystemInfo) {
    let b = basic_information();
    let i = SystemInfo {
        processor_architecture: 9, // PROCESSOR_ARCHITECTURE_AMD64
        reserved: 0,
        page_size: u32_at(&b, 8),
        minimum_application_address: u64_at(&b, 32) as usize,
        maximum_application_address: u64_at(&b, 40) as usize,
        active_processor_mask: u64_at(&b, 48) as usize,
        number_of_processors: u32::from(b[56]),
        processor_type: 8664, // PROCESSOR_AMD_X8664
        allocation_granularity: u32_at(&b, 24),
        processor_level: 6,
        processor_revision: 0,
    };
    // SAFETY: the caller's output structure.
    unsafe { info.write(i) };
}

#[unsafe(no_mangle)]
pub extern "system" fn GetNativeSystemInfo(info: *mut SystemInfo) {
    GetSystemInfo(info);
}

/// GetSystemDirectoryA: `<NtSystemRoot>\system32` (from
/// KUSER_SHARED_DATA). Returns the length without the NUL, or the size
/// needed (with it) if `buffer` is too small.
#[unsafe(no_mangle)]
pub extern "system" fn GetSystemDirectoryA(buffer: *mut u8, size: u32) -> u32 {
    let mut path = [0u8; 280];
    let mut n = 0;
    // SAFETY: KUSER_SHARED_DATA.NtSystemRoot, a NUL-terminated WCHAR[260].
    unsafe {
        while n < 260 {
            let c = core::ptr::read_volatile((0x7ffe_0030 + 2 * n) as *const u16);
            if c == 0 {
                break;
            }
            path[n] = if c < 0x80 { c as u8 } else { b'?' };
            n += 1;
        }
    }
    for &b in b"\\system32" {
        path[n] = b;
        n += 1;
    }
    if (size as usize) <= n {
        return n as u32 + 1;
    }
    // SAFETY: the caller's buffer holds `size` > n bytes.
    unsafe {
        core::ptr::copy_nonoverlapping(path.as_ptr(), buffer, n);
        *buffer.add(n) = 0;
    }
    n as u32
}

#[unsafe(no_mangle)]
pub extern "system" fn GetProcessAffinityMask(
    process: Handle,
    process_mask: *mut usize,
    system_mask: *mut usize,
) -> Bool {
    // PROCESS_BASIC_INFORMATION.AffinityMask (offset 16).
    let mut pbi = [0u8; 48];
    let mut ret = 0;
    // SAFETY: a 48-byte buffer.
    let status =
        unsafe { NtQueryInformationProcess(process as isize, 0, pbi.as_mut_ptr(), 48, &mut ret) };
    if status != STATUS_SUCCESS {
        return fail(status, FALSE);
    }
    let system = u64_at(&basic_information(), 48);
    // SAFETY: the caller's output pointers.
    unsafe {
        *process_mask = u64_at(&pbi, 16) as usize;
        *system_mask = system as usize;
    }
    TRUE
}

// ---- Environment variables (the block in the process parameters) ----

/// Serializes changes to the environment block (and readers against them).
static ENV_LOCK: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
/// The block this DLL allocated last (freed when replaced), or 0.
static ENV_OWNED: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

fn with_env<R>(f: impl FnOnce() -> R) -> R {
    use core::sync::atomic::Ordering;
    while ENV_LOCK.compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed).is_err()
    {
        core::hint::spin_loop();
    }
    let r = f();
    ENV_LOCK.store(false, Ordering::Release);
    r
}

/// The process parameters (PEB+0x20).
fn process_parameters() -> usize {
    // SAFETY: gs:[0x60] is the PEB; ProcessParameters is always set.
    unsafe { *((teb_u64(0x60) as usize + 0x20) as *const usize) }
}

/// The environment block's strings (without the final empty one).
fn env_entries() -> impl Iterator<Item = &'static [u16]> {
    // SAFETY: ProcessParameters->Environment (+0x80): NUL-terminated
    // strings ending with an empty one, alive until replaced (callers hold
    // ENV_LOCK).
    let block = unsafe { *((process_parameters() + 0x80) as *const *const u16) };
    let mut i = 0;
    core::iter::from_fn(move || {
        // SAFETY: as above.
        unsafe {
            let start = i;
            while *block.add(i) != 0 {
                i += 1;
            }
            if i == start {
                return None;
            }
            i += 1;
            Some(core::slice::from_raw_parts(block.add(start), i - 1 - start))
        }
    })
}

/// Splits `NAME=value`; a leading `=` belongs to the name (`=C:=C:\`).
fn split_entry(entry: &[u16]) -> (&[u16], &[u16]) {
    match entry.iter().skip(1).position(|&c| c == u16::from(b'=')) {
        Some(i) => (&entry[..i + 1], &entry[i + 2..]),
        None => (entry, &[]),
    }
}

fn upper(c: u16) -> u16 {
    if (u16::from(b'a')..=u16::from(b'z')).contains(&c) { c - 32 } else { c }
}

/// Environment names compare case-insensitively.
fn same_name(a: &[u16], b: &[u16]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(&x, &y)| upper(x) == upper(y))
}

fn wide_slice(s: *const u16) -> &'static [u16] {
    // SAFETY: callers pass NUL-terminated strings.
    unsafe { core::slice::from_raw_parts(s, wide_len(s)) }
}

/// Copies `value` to `buf` (of `size` characters) the Win32 way: its length
/// if it fits with the NUL, else the size needed including the NUL.
fn copy_out(value: &[u16], buf: *mut u16, size: u32) -> u32 {
    if (size as usize) <= value.len() {
        return value.len() as u32 + 1;
    }
    // SAFETY: the caller's buffer holds `size` > len characters.
    unsafe {
        core::ptr::copy_nonoverlapping(value.as_ptr(), buf, value.len());
        *buf.add(value.len()) = 0;
    }
    value.len() as u32
}

const ERROR_ENVVAR_NOT_FOUND: u32 = 203;

#[unsafe(no_mangle)]
pub extern "system" fn GetEnvironmentVariableW(name: *const u16, buf: *mut u16, size: u32) -> u32 {
    let name = wide_slice(name);
    let found = with_env(|| {
        env_entries().find_map(|e| {
            let (n, v) = split_entry(e);
            same_name(n, name).then(|| copy_out(v, buf, size))
        })
    });
    found.unwrap_or_else(|| fail_with(ERROR_ENVVAR_NOT_FOUND, 0))
}

/// SetEnvironmentVariableW: builds a new block without the variable, plus
/// `name=value` unless `value` is null, and installs it in the process
/// parameters (as RtlSetEnvironmentVariable does).
#[unsafe(no_mangle)]
pub extern "system" fn SetEnvironmentVariableW(name: *const u16, value: *const u16) -> Bool {
    use core::sync::atomic::Ordering;
    let name = wide_slice(name);
    if name.is_empty() || name[1..].contains(&u16::from(b'=')) {
        return fail_with(ERROR_INVALID_PARAMETER, FALSE);
    }
    let value = (!value.is_null()).then(|| wide_slice(value));
    with_env(|| {
        let keep = |e: &&[u16]| !same_name(split_entry(e).0, name);
        let mut chars: usize = env_entries().filter(keep).map(|e| e.len() + 1).sum();
        if let Some(v) = value {
            chars += name.len() + 1 + v.len() + 1;
        }
        chars += 1; // the final empty string
        let block = VirtualAlloc(core::ptr::null_mut(), chars * 2, 0x3000, 4) as *mut u16;
        if block.is_null() {
            return FALSE;
        }
        let mut i = 0;
        let mut put = |s: &[u16]| {
            // SAFETY: `block` holds `chars` characters, counted above.
            unsafe { core::ptr::copy_nonoverlapping(s.as_ptr(), block.add(i), s.len()) };
            i += s.len();
        };
        for e in env_entries().filter(keep) {
            put(e);
            put(&[0]);
        }
        if let Some(v) = value {
            put(name);
            put(&[u16::from(b'=')]);
            put(v);
            put(&[0]);
        }
        put(&[0]);
        let params = process_parameters();
        // SAFETY: Environment (+0x80) and EnvironmentSize (+0x3f0) of this
        // process's parameters, writable; readers hold ENV_LOCK.
        unsafe {
            *((params + 0x80) as *mut *mut u16) = block;
            *((params + 0x3f0) as *mut usize) = chars * 2;
        }
        let old = ENV_OWNED.swap(block as usize, Ordering::Relaxed);
        if old != 0 {
            VirtualFree(old as *mut c_void, 0, 0x8000);
        }
        TRUE
    })
}

// ---- Time zone (gala keeps UTC) ----

/// GetTimeZoneInformation: UTC, no daylight saving (TIME_ZONE_ID_UNKNOWN);
/// gala's clock and KUSER_SHARED_DATA's TimeZoneBias are UTC. Deliberate
/// difference: the zone is named "UTC" (its time-zone key name), not
/// Windows' display name "Coordinated Universal Time", because programs
/// (Go's time package) map display names to keys through the registry,
/// which gala does not have yet.
#[unsafe(no_mangle)]
pub extern "system" fn GetTimeZoneInformation(info: *mut u8) -> u32 {
    const NAME: &[u8] = b"UTC";
    // SAFETY: the caller's 172-byte TIME_ZONE_INFORMATION.
    unsafe {
        core::ptr::write_bytes(info, 0, 172);
        // StandardName at +4 and DaylightName at +88, WCHAR[32] each.
        for offset in [4usize, 88] {
            for (i, &b) in NAME.iter().enumerate() {
                *(info.add(offset + 2 * i) as *mut u16) = u16::from(b);
            }
        }
    }
    0
}

// ---- Directories ----

/// GetCurrentDirectoryW: ProcessParameters->CurrentDirectory.DosPath
/// without its trailing backslash (kept for a root such as `C:\`).
#[unsafe(no_mangle)]
pub extern "system" fn GetCurrentDirectoryW(size: u32, buf: *mut u16) -> u32 {
    let params = process_parameters();
    // SAFETY: CurrentDirectory.DosPath: UNICODE_STRING at +0x38.
    let dos = unsafe {
        let len = usize::from(*((params + 0x38) as *const u16)) / 2;
        core::slice::from_raw_parts(*((params + 0x40) as *const *const u16), len)
    };
    let path = match dos {
        [.., b] if *b == u16::from(b'\\') && dos.len() > 3 => &dos[..dos.len() - 1],
        _ => dos,
    };
    copy_out(path, buf, size)
}

/// GetTempPathW: the first of TMP, TEMP, USERPROFILE, or the Windows
/// directory, with a trailing backslash (documented order).
#[unsafe(no_mangle)]
pub extern "system" fn GetTempPathW(size: u32, buf: *mut u16) -> u32 {
    let mut path = [0u16; 262];
    let mut n = 0;
    for name in [&b"TMP"[..], b"TEMP", b"USERPROFILE"] {
        let mut wide = [0u16; 12];
        for (w, &b) in wide.iter_mut().zip(name) {
            *w = u16::from(b);
        }
        n = GetEnvironmentVariableW(wide.as_ptr(), path.as_mut_ptr(), 260) as usize;
        if n > 0 && n < 260 {
            break;
        }
        n = 0;
    }
    if n == 0 {
        for &b in b"C:\\Windows" {
            path[n] = u16::from(b);
            n += 1;
        }
    }
    if path[n - 1] != u16::from(b'\\') {
        path[n] = u16::from(b'\\');
        n += 1;
    }
    SetLastError(0);
    copy_out(&path[..n], buf, size)
}

/// GetTempPath2W: as GetTempPathW for processes not running as SYSTEM.
#[unsafe(no_mangle)]
pub extern "system" fn GetTempPath2W(size: u32, buf: *mut u16) -> u32 {
    GetTempPathW(size, buf)
}

// ---- Environment ----

/// GetCommandLineW: the process's command line (ProcessParameters->
/// CommandLine, a NUL-terminated buffer).
#[unsafe(no_mangle)]
pub extern "system" fn GetCommandLineW() -> *const u16 {
    // SAFETY: PEB (gs:[0x60]) -> ProcessParameters (+0x20) -> CommandLine
    // (UNICODE_STRING at +0x70, Buffer at +0x78).
    unsafe {
        let peb = teb_u64(0x60) as usize;
        let params = *((peb + 0x20) as *const usize);
        *((params + 0x78) as *const *const u16)
    }
}

/// GetEnvironmentStringsW: a private copy of the environment block
/// (`NAME=value\0...\0`), freed with FreeEnvironmentStringsW.
#[unsafe(no_mangle)]
pub extern "system" fn GetEnvironmentStringsW() -> *mut u16 {
    // SAFETY: PEB (gs:[0x60]) -> ProcessParameters (+0x20) -> Environment
    // (+0x80), a block ending with an empty string.
    unsafe {
        let peb = teb_u64(0x60) as usize;
        let params = *((peb + 0x20) as *const usize);
        let env = *((params + 0x80) as *const *const u16);
        let mut n = 0;
        while !(core::ptr::read_volatile(env.add(n)) == 0
            && core::ptr::read_volatile(env.add(n + 1)) == 0)
        {
            n += 1;
        }
        let bytes = (n + 2) * 2;
        let copy = VirtualAlloc(core::ptr::null_mut(), bytes, 0x3000, 4) as *mut u16;
        if !copy.is_null() {
            core::ptr::copy_nonoverlapping(env, copy, n + 2);
        }
        copy
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn FreeEnvironmentStringsW(block: *mut u16) -> Bool {
    VirtualFree(block.cast(), 0, 0x8000)
}

// ---- Handles, waits, memory, threads ----

#[unsafe(no_mangle)]
pub extern "system" fn CreateEventA(
    attributes: *mut c_void,
    manual_reset: Bool,
    initial: Bool,
    name: *const u8,
) -> Handle {
    if !name.is_null() {
        unsupported(b"kernel32.dll!CreateEventA(name)", 0);
        return fail_with(ERROR_NOT_SUPPORTED, core::ptr::null_mut());
    }
    CreateEventW(attributes, manual_reset, initial, core::ptr::null())
}

/// WaitForMultipleObjects: index of the signaled (or abandoned + 0x80)
/// object, WAIT_TIMEOUT, or WAIT_FAILED.
#[unsafe(no_mangle)]
pub extern "system" fn WaitForMultipleObjects(
    count: u32,
    handles: *const Handle,
    all: Bool,
    ms: u32,
) -> u32 {
    let t = timeout(ms);
    let p = t.as_ref().map_or(core::ptr::null(), |t| t as *const i64);
    // WaitAll = 0, WaitAny = 1.
    let wait_type = if all != 0 { 0 } else { 1 };
    // SAFETY: the caller's handle array; valid or null timeout pointer.
    let status = unsafe { NtWaitForMultipleObjects(count, handles, wait_type, 0, p) };
    match status {
        0..=0x3f | 0x80..=0xbf | STATUS_TIMEOUT => status,
        _ => fail(status, WAIT_FAILED),
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn DuplicateHandle(
    source_process: Handle,
    source: Handle,
    target_process: Handle,
    target: *mut Handle,
    access: u32,
    inherit: Bool,
    options: u32,
) -> Bool {
    let attributes = if inherit != 0 { 0x2 } else { 0 }; // OBJ_INHERIT
    // SAFETY: the kernel validates handles and the output pointer.
    let status = unsafe {
        NtDuplicateObject(
            source_process,
            source,
            target_process,
            target,
            access,
            attributes,
            options,
        )
    };
    if status != STATUS_SUCCESS { fail(status, FALSE) } else { TRUE }
}

/// VirtualQuery: MEMORY_BASIC_INFORMATION for `address`; returns the bytes
/// written, 0 on failure.
#[unsafe(no_mangle)]
pub extern "system" fn VirtualQuery(address: *const c_void, info: *mut u8, length: usize) -> usize {
    let mut ret = 0;
    // SAFETY: the caller's buffer of `length` bytes.
    let status = unsafe {
        NtQueryVirtualMemory(CURRENT_PROCESS, address as usize, 0, info, length, &mut ret)
    };
    if status != STATUS_SUCCESS { fail(status, 0) } else { ret }
}

/// SwitchToThread: whether another thread ran (STATUS_NO_YIELD_PERFORMED
/// means none was ready).
#[unsafe(no_mangle)]
pub extern "system" fn SwitchToThread() -> Bool {
    // SAFETY: no arguments.
    let status = unsafe { NtYieldExecution() };
    if status == 0x4000_0024 { FALSE } else { TRUE }
}

/// RaiseFailFastException: ends the process at once with the exception's
/// code (STATUS_FAIL_FAST_EXCEPTION without a record), bypassing handlers.
#[unsafe(no_mangle)]
pub extern "system" fn RaiseFailFastException(
    record: *const u32,
    _context: *const c_void,
    _flags: u32,
) -> ! {
    let code = if record.is_null() {
        0xc000_0602
    } else {
        // SAFETY: EXCEPTION_RECORD.ExceptionCode is its first field.
        unsafe { *record }
    };
    ExitProcess(code)
}

// ---- Threads ----

/// CONTEXT (x64 layout, 0x4d0 bytes, 16-byte aligned).
#[repr(C, align(16))]
pub struct Context {
    home: [u64; 6],
    context_flags: u32,
    mx_csr: u32,
    /// SegCs, SegDs, SegEs, SegFs, SegGs, SegSs.
    segs: [u16; 6],
    eflags: u32,
    dr: [u64; 6],
    /// Rax, Rcx, Rdx, Rbx, Rsp, Rbp, Rsi, Rdi, R8..R15.
    gpr: [u64; 16],
    rip: u64,
    rest: [u8; 0x4d0 - 0x100],
}

type StartRoutine = extern "system" fn(*mut c_void) -> u32;

/// Every thread CreateThread makes starts here (rcx = start, rdx = param),
/// like Windows' thread start thunk: run the routine, exit with its result.
extern "system" fn thread_start(start: StartRoutine, param: *mut c_void) -> ! {
    ExitThread(start(param))
}

/// Default stack reservation for new threads (Windows takes it from the
/// executable's header; 1 MiB is the usual value).
const DEFAULT_STACK: usize = 0x10_0000;

/// CreateThread: the stack is reserved and committed here (no guard page
/// yet: a thread that overflows it faults), then NtCreateThread starts the
/// thread at `thread_start`. Thread attributes are not supported yet.
#[unsafe(no_mangle)]
pub extern "system" fn CreateThread(
    attributes: *mut c_void,
    stack_size: usize,
    start: StartRoutine,
    param: *mut c_void,
    flags: u32,
    thread_id: *mut u32,
) -> Handle {
    const CREATE_SUSPENDED: u32 = 0x4;
    const STACK_SIZE_PARAM_IS_A_RESERVATION: u32 = 0x1_0000;
    if !attributes.is_null() || flags & !(CREATE_SUSPENDED | STACK_SIZE_PARAM_IS_A_RESERVATION) != 0
    {
        unsupported(b"kernel32.dll!CreateThread(attributes/flags)", u64::from(flags));
        return fail_with(ERROR_NOT_SUPPORTED, core::ptr::null_mut());
    }
    let size = stack_size.max(DEFAULT_STACK).next_multiple_of(0x1_0000);
    let stack = VirtualAlloc(core::ptr::null_mut(), size, 0x3000, 4) as usize;
    if stack == 0 {
        return core::ptr::null_mut();
    }
    let top = stack + size;
    // SAFETY: an all-zero CONTEXT is a valid bit pattern.
    let mut context: Context = unsafe { core::mem::zeroed() };
    context.context_flags = 0x10_000b; // CONTEXT_FULL
    context.mx_csr = 0x1f80;
    context.segs = [0x33, 0x2b, 0x2b, 0x53, 0x2b, 0x2b];
    context.eflags = 0x200;
    context.rip = thread_start as *const () as u64;
    context.gpr[1] = start as *const () as u64; // rcx
    context.gpr[2] = param as u64; // rdx
    context.gpr[4] = (top - 0x28) as u64; // rsp, as if called
    // INITIAL_TEB: OldStackBase, OldStackLimit, StackBase, StackLimit,
    // StackAllocationBase.
    let initial_teb = [0, 0, top, stack, stack];
    let (mut h, mut cid) = (core::ptr::null_mut(), [0usize; 2]);
    // SAFETY: valid pointers for the call.
    let status = unsafe {
        NtCreateThread(
            &mut h,
            0x1f_ffff, // THREAD_ALL_ACCESS
            core::ptr::null(),
            CURRENT_PROCESS,
            &mut cid,
            &context,
            &initial_teb,
            u8::from(flags & CREATE_SUSPENDED != 0),
        )
    };
    if status != STATUS_SUCCESS {
        VirtualFree(stack as *mut c_void, 0, 0x8000);
        return fail(status, core::ptr::null_mut());
    }
    if !thread_id.is_null() {
        // SAFETY: the caller's output pointer.
        unsafe { *thread_id = cid[1] as u32 };
    }
    h
}

/// ExitThread. TODO: DLL_THREAD_DETACH notifications; the stack is not
/// freed (Windows frees it).
#[unsafe(no_mangle)]
pub extern "system" fn ExitThread(code: u32) -> ! {
    // SAFETY: ends the current thread (-2 = the current thread).
    unsafe { NtTerminateThread(-2, code) };
    loop {
        core::hint::spin_loop();
    }
}

/// GetExitCodeThread: STILL_ACTIVE (259) while it runs.
#[unsafe(no_mangle)]
pub extern "system" fn GetExitCodeThread(h: Handle, code: *mut u32) -> Bool {
    let mut basic = [0u8; 48];
    let mut ret = 0;
    // SAFETY: a 48-byte THREAD_BASIC_INFORMATION buffer.
    let status = unsafe { NtQueryInformationThread(h, 0, basic.as_mut_ptr(), 48, &mut ret) };
    if status != STATUS_SUCCESS {
        return fail(status, FALSE);
    }
    // SAFETY: the caller's output pointer.
    unsafe { *code = u32_at(&basic, 0) };
    TRUE
}

/// SuspendThread/ResumeThread: the previous suspend count, or (DWORD)-1.
#[unsafe(no_mangle)]
pub extern "system" fn SuspendThread(h: Handle) -> u32 {
    let mut previous = 0;
    // SAFETY: valid output pointer.
    let status = unsafe { NtSuspendThread(h, &mut previous) };
    if status != STATUS_SUCCESS { fail(status, u32::MAX) } else { previous }
}

#[unsafe(no_mangle)]
pub extern "system" fn ResumeThread(h: Handle) -> u32 {
    let mut previous = 0;
    // SAFETY: valid output pointer.
    let status = unsafe { NtResumeThread(h, &mut previous) };
    if status != STATUS_SUCCESS { fail(status, u32::MAX) } else { previous }
}

#[unsafe(no_mangle)]
pub extern "system" fn GetThreadContext(h: Handle, context: *mut Context) -> Bool {
    // SAFETY: the caller's CONTEXT.
    let status = unsafe { NtGetContextThread(h, context) };
    if status != STATUS_SUCCESS { fail(status, FALSE) } else { TRUE }
}

#[unsafe(no_mangle)]
pub extern "system" fn SetThreadContext(h: Handle, context: *const Context) -> Bool {
    // SAFETY: the caller's CONTEXT.
    let status = unsafe { NtSetContextThread(h, context) };
    if status != STATUS_SUCCESS { fail(status, FALSE) } else { TRUE }
}

// ---- I/O completion ports ----

const INVALID_HANDLE_VALUE: Handle = -1isize as Handle;

/// CreateIoCompletionPort: a new port (`file` = INVALID_HANDLE_VALUE), or
/// `file` associated with `existing` (or a new port) under `key`.
#[unsafe(no_mangle)]
pub extern "system" fn CreateIoCompletionPort(
    file: Handle,
    existing: Handle,
    key: usize,
    threads: u32,
) -> Handle {
    let port = if existing.is_null() {
        let mut port = core::ptr::null_mut();
        // SAFETY: valid output pointer; IO_COMPLETION_ALL_ACCESS.
        let status =
            unsafe { NtCreateIoCompletion(&mut port, 0x1f_0003, core::ptr::null(), threads) };
        if status != STATUS_SUCCESS {
            return fail(status, core::ptr::null_mut());
        }
        port
    } else {
        existing
    };
    if file == INVALID_HANDLE_VALUE {
        return if existing.is_null() {
            port
        } else {
            fail_with(ERROR_INVALID_PARAMETER, core::ptr::null_mut())
        };
    }
    // FILE_COMPLETION_INFORMATION { Port, Key } (FileCompletionInformation).
    let info = [port as usize, key];
    let mut iosb = [0usize; 2];
    // SAFETY: a 16-byte information buffer.
    let status = unsafe { NtSetInformationFile(file, &mut iosb, info.as_ptr().cast(), 16, 30) };
    if status != STATUS_SUCCESS {
        if existing.is_null() {
            CloseHandle(port);
        }
        return fail(status, core::ptr::null_mut());
    }
    port
}

#[unsafe(no_mangle)]
pub extern "system" fn PostQueuedCompletionStatus(
    port: Handle,
    bytes: u32,
    key: usize,
    overlapped: usize,
) -> Bool {
    // SAFETY: the kernel validates the port.
    let status =
        unsafe { NtSetIoCompletion(port, key, overlapped, STATUS_SUCCESS, bytes as usize) };
    if status != STATUS_SUCCESS { fail(status, FALSE) } else { TRUE }
}

#[repr(C)]
pub struct OverlappedEntry {
    completion_key: usize,
    overlapped: usize,
    internal: usize,
    bytes: u32,
}

/// One entry from `port`, waiting up to `ms` (Err: the NTSTATUS).
fn remove_completion(port: Handle, ms: u32) -> Result<OverlappedEntry, u32> {
    let (mut key, mut context, mut iosb) = (0usize, 0usize, [0usize; 2]);
    let t = timeout(ms);
    let p = t.as_ref().map_or(core::ptr::null(), |t| t as *const i64);
    // SAFETY: valid output pointers.
    let status = unsafe { NtRemoveIoCompletion(port, &mut key, &mut context, &mut iosb, p) };
    match status {
        STATUS_SUCCESS => Ok(OverlappedEntry {
            completion_key: key,
            overlapped: context,
            internal: iosb[0],
            bytes: iosb[1] as u32,
        }),
        other => Err(other),
    }
}

/// GetQueuedCompletionStatusEx: waits up to `ms` for the first entry, then
/// takes what else is queued, up to `count`. Alertable waits are not
/// supported yet (reported).
#[unsafe(no_mangle)]
pub extern "system" fn GetQueuedCompletionStatusEx(
    port: Handle,
    entries: *mut OverlappedEntry,
    count: u32,
    removed: *mut u32,
    ms: u32,
    alertable: Bool,
) -> Bool {
    if alertable != 0 {
        unsupported(b"kernel32.dll!GetQueuedCompletionStatusEx(alertable)", 0);
    }
    let mut n = 0;
    while n < count {
        match remove_completion(port, if n == 0 { ms } else { 0 }) {
            // SAFETY: the caller's array holds `count` entries.
            Ok(entry) => unsafe { entries.add(n as usize).write(entry) },
            Err(STATUS_TIMEOUT) if n > 0 => break,
            Err(STATUS_TIMEOUT) => return fail_with(258, FALSE), // WAIT_TIMEOUT
            Err(status) => return fail(status, FALSE),
        }
        n += 1;
    }
    // SAFETY: the caller's output pointer.
    unsafe { *removed = n };
    TRUE
}

#[unsafe(no_mangle)]
pub extern "system" fn GetQueuedCompletionStatus(
    port: Handle,
    bytes: *mut u32,
    key: *mut usize,
    overlapped: *mut usize,
    ms: u32,
) -> Bool {
    match remove_completion(port, ms) {
        // SAFETY: the caller's output pointers.
        Ok(e) => unsafe {
            *bytes = e.bytes;
            *key = e.completion_key;
            *overlapped = e.overlapped;
            if e.internal as u32 == STATUS_SUCCESS { TRUE } else { fail(e.internal as u32, FALSE) }
        },
        Err(STATUS_TIMEOUT) => {
            // SAFETY: the caller's output pointer.
            unsafe { *overlapped = 0 };
            fail_with(258, FALSE)
        }
        Err(status) => fail(status, FALSE),
    }
}

// ---- Waitable timers ----

/// CreateWaitableTimerExW: unnamed timers (names and attributes are
/// reported). CREATE_WAITABLE_TIMER_HIGH_RESOLUTION is accepted: gala's
/// timers run on the 1 ms tick either way.
#[unsafe(no_mangle)]
pub extern "system" fn CreateWaitableTimerExW(
    attributes: *mut c_void,
    name: *const u16,
    flags: u32,
    access: u32,
) -> Handle {
    if !attributes.is_null() || !name.is_null() || flags & !3 != 0 {
        unsupported(
            b"kernel32.dll!CreateWaitableTimerExW(name/attributes/flags)",
            u64::from(flags),
        );
        return fail_with(ERROR_NOT_SUPPORTED, core::ptr::null_mut());
    }
    let kind = if flags & 1 != 0 { 0 } else { 1 }; // manual reset: NotificationTimer
    let mut h = core::ptr::null_mut();
    // SAFETY: valid output pointer.
    let status = unsafe { NtCreateTimer(&mut h, access, core::ptr::null(), kind) };
    if status != STATUS_SUCCESS { fail(status, core::ptr::null_mut()) } else { h }
}

/// SetWaitableTimer: the completion routine has the NT timer APC shape
/// (context, low, high), so it is passed through.
#[unsafe(no_mangle)]
pub extern "system" fn SetWaitableTimer(
    h: Handle,
    due: *const i64,
    period: i32,
    routine: usize,
    arg: usize,
    resume: Bool,
) -> Bool {
    // SAFETY: the caller's due time.
    let status = unsafe {
        NtSetTimer(h, due, routine, arg, u8::from(resume != 0), period, core::ptr::null_mut())
    };
    if status != STATUS_SUCCESS { fail(status, FALSE) } else { TRUE }
}

// ---- Vectored exception handling and unwinding (ntdll's) ----

#[unsafe(no_mangle)]
pub extern "system" fn AddVectoredExceptionHandler(first: u32, handler: usize) -> *mut c_void {
    // SAFETY: ntdll keeps the handler pointer.
    unsafe { RtlAddVectoredExceptionHandler(first, handler) }
}

#[unsafe(no_mangle)]
pub extern "system" fn RemoveVectoredExceptionHandler(cookie: *mut c_void) -> u32 {
    // SAFETY: plain call.
    unsafe { RtlRemoveVectoredExceptionHandler(cookie) }
}

#[unsafe(no_mangle)]
pub extern "system" fn AddVectoredContinueHandler(first: u32, handler: usize) -> *mut c_void {
    // SAFETY: ntdll keeps the handler pointer.
    unsafe { RtlAddVectoredContinueHandler(first, handler) }
}

#[unsafe(no_mangle)]
pub extern "system" fn RemoveVectoredContinueHandler(cookie: *mut c_void) -> u32 {
    // SAFETY: plain call.
    unsafe { RtlRemoveVectoredContinueHandler(cookie) }
}

// RtlLookupFunctionEntry and RtlVirtualUnwind are export forwarders to
// ntdll, as on Windows (build.rs: /EXPORT:Name=ntdll.Name).

// ---- Thread-local storage: TEB.TlsSlots (+0x1480, 64 slots) ----

/// Allocated TLS indexes (bit set = in use). TODO: the expansion slots
/// (indexes 64..1088, TEB.TlsExpansionSlots).
static TLS_BITMAP: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
const TLS_SLOTS: usize = 0x1480;

fn tls_slot(index: u32) -> usize {
    TLS_SLOTS + 8 * index as usize
}

/// TlsAlloc: the lowest free index, its slot zeroed in this thread (TODO:
/// zero it in the other threads too, as Windows does).
#[unsafe(no_mangle)]
pub extern "system" fn TlsAlloc() -> u32 {
    use core::sync::atomic::Ordering;
    let mut bits = TLS_BITMAP.load(Ordering::Relaxed);
    loop {
        if bits == u64::MAX {
            unsupported(b"kernel32.dll!TlsAlloc(expansion slots)", 64);
            return fail_with(8, u32::MAX); // ERROR_NOT_ENOUGH_MEMORY, TLS_OUT_OF_INDEXES
        }
        let index = bits.trailing_ones();
        match TLS_BITMAP.compare_exchange(
            bits,
            bits | 1 << index,
            Ordering::AcqRel,
            Ordering::Relaxed,
        ) {
            Ok(_) => {
                TlsSetValue(index, core::ptr::null_mut());
                return index;
            }
            Err(now) => bits = now,
        }
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn TlsFree(index: u32) -> Bool {
    use core::sync::atomic::Ordering;
    if index >= 64 || TLS_BITMAP.fetch_and(!(1 << index), Ordering::AcqRel) & 1 << index == 0 {
        return fail_with(ERROR_INVALID_PARAMETER, FALSE);
    }
    TRUE
}

/// TlsGetValue: also clears the last error on success, as documented (so
/// callers can tell a stored 0 from a failure).
#[unsafe(no_mangle)]
pub extern "system" fn TlsGetValue(index: u32) -> *mut c_void {
    if index >= 64 {
        return fail_with(ERROR_INVALID_PARAMETER, core::ptr::null_mut());
    }
    SetLastError(0);
    teb_u64(tls_slot(index)) as *mut c_void
}

#[unsafe(no_mangle)]
pub extern "system" fn TlsSetValue(index: u32, value: *mut c_void) -> Bool {
    if index >= 64 {
        return fail_with(ERROR_INVALID_PARAMETER, FALSE);
    }
    // SAFETY: the slot lies in the current thread's TEB.
    unsafe {
        core::arch::asm!("mov gs:[{}], {}", in(reg) tls_slot(index), in(reg) value as u64, options(nostack))
    };
    TRUE
}

/// ExitProcess. TODO: DLL_PROCESS_DETACH notifications and ending other
/// threads first (RtlExitUserProcess), as Windows does.
#[unsafe(no_mangle)]
pub extern "system" fn ExitProcess(code: u32) -> ! {
    // SAFETY: ends the current process.
    unsafe { NtTerminateProcess(CURRENT_PROCESS, code) };
    loop {
        core::hint::spin_loop();
    }
}

/// DllMain (the entry point our link flags name).
#[unsafe(no_mangle)]
pub extern "system" fn mainCRTStartup(_instance: usize, _reason: u32, _reserved: usize) -> i32 {
    1
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    ExitProcess(0xc000_0409) // STATUS_STACK_BUFFER_OVERRUN, as __fastfail would
}
