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
    fn strlen(s: *const u8) -> usize;
}

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

// ---- Environment ----

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
