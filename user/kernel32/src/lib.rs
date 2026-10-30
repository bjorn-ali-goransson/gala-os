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
    LoadLibraryW(wide[..n].as_ptr(), n)
}

/// Loads a library by UTF-16 name of `len` characters.
fn LoadLibraryW(name: *const u16, len: usize) -> Handle {
    let us =
        UnicodeString { length: (len * 2) as u16, maximum_length: (len * 2) as u16, buffer: name };
    let mut base = core::ptr::null_mut();
    // SAFETY: valid UNICODE_STRING and output pointer.
    let status = unsafe { LdrLoadDll(core::ptr::null(), core::ptr::null(), &us, &mut base) };
    if status != STATUS_SUCCESS { fail(status, core::ptr::null_mut()) } else { base }
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
