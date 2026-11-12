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

// The architecture's parts: x64 (System32's kernel32) or x86 (the 32-bit
// kernel32 in SysWOW64, ARCHITECTURE "Plan: WOW64"): TEB access, the
// offsets of the structures whose layout differs, SLIST_HEADER.
#[cfg(target_arch = "x86_64")]
#[path = "x64.rs"]
mod arch;
#[cfg(target_arch = "x86")]
#[path = "x86.rs"]
mod arch;
use arch::*;

mod actctx;
mod ansi;
mod appmodel;
mod atom;
mod buf;
mod calendar;
mod console;
mod context;
mod files;
mod global;
mod heap;
mod idn;
mod locale;
mod mapping;
mod messages;
mod nls;
mod path;
mod pipe;
mod pool;
mod probe;
mod process;
mod processor;
mod procinfo;
mod profile;
mod pty;
mod resource;
mod runtime;
mod sync;
mod sysmem;
mod text;
mod toolhelp;
mod unimplemented;
mod volume;

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
const ERROR_NOT_ENOUGH_MEMORY: u32 = 8;
const ERROR_FILENAME_EXCED_RANGE: u32 = 206;
const WAIT_FAILED: u32 = 0xffff_ffff;
const INFINITE: u32 = 0xffff_ffff;

#[repr(C)]
struct UnicodeString {
    length: u16,
    maximum_length: u16,
    buffer: *const u16,
}

/// UNICODE_STRING.Buffer's offset (after Length and MaximumLength,
/// pointer-aligned): for strings inside other structures.
const US_BUFFER: usize = core::mem::offset_of!(UnicodeString, buffer);

#[repr(C)]
struct AnsiString {
    length: u16,
    maximum_length: u16,
    buffer: *const u8,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct ObjectAttributes {
    length: u32,
    root_directory: usize,
    object_name: *const UnicodeString,
    attributes: u32,
    security_descriptor: usize,
    security_quality_of_service: usize,
}

// The C runtime helpers the compiler may call, exported by ntdll (as the
// real ntdll does). Only compiler-generated code calls them. On x86 they
// come from ntrt's x86 runtime (`x86.rs`): a raw-dylib import does not
// give the symbols the compiler calls there.
#[cfg(target_arch = "x86_64")]
#[allow(dead_code)]
#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "C" {
    fn memset(dst: *mut u8, value: i32, n: usize) -> *mut u8;
    fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memcmp(a: *const u8, b: *const u8, n: usize) -> i32;
    fn strlen(s: *const u8) -> usize;
    fn memmove(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    // MSVC-target unwind tables reference it (gala's ntdll provides it).
    fn __CxxFrameHandler3();
    // The stack probe for frames over a page (ntdll exports it).
    fn __chkstk();
}

/// MSVC-target codegen references this marker symbol directly, so it cannot
/// come from an import. (It becomes a harmless extra data export.) On x86,
/// ntrt's.
#[cfg(target_arch = "x86_64")]
#[unsafe(no_mangle)]
pub static _fltused: i32 = 0;

#[cfg_attr(
    target_arch = "x86",
    link(name = "ntdll", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "ntdll", kind = "raw-dylib"))]
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
    fn NtOpenProcess(
        handle: *mut usize,
        access: u32,
        attributes: *const ObjectAttributes,
        client_id: *const [usize; 2],
    ) -> u32;
    fn NtOpenThread(
        handle: *mut usize,
        access: u32,
        attributes: *const ObjectAttributes,
        client_id: *const [usize; 2],
    ) -> u32;
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
    fn NtReadFile(
        h: Handle,
        event: usize,
        apc: usize,
        apc_context: usize,
        iosb: *mut [usize; 2],
        buffer: *mut u8,
        length: u32,
        offset: *const i64,
        key: *const u32,
    ) -> u32;
    fn NtQueryInformationFile(
        h: Handle,
        iosb: *mut [usize; 2],
        buf: *mut u8,
        len: u32,
        class: u32,
    ) -> u32;
    fn NtQueryDirectoryFile(
        h: Handle,
        event: usize,
        apc: usize,
        apc_context: usize,
        iosb: *mut [usize; 2],
        buf: *mut u8,
        len: u32,
        class: u32,
        single: u8,
        mask: *const UnicodeString,
        restart: u8,
    ) -> u32;
    fn NtQueryAttributesFile(attributes: *const ObjectAttributes, info: *mut u8) -> u32;
    fn NtFlushBuffersFile(h: Handle, iosb: *mut [usize; 2]) -> u32;
    fn NtCreateFile(
        handle: *mut Handle,
        access: u32,
        attributes: *const ObjectAttributes,
        iosb: *mut [usize; 2],
        allocation: *const i64,
        file_attributes: u32,
        share: u32,
        disposition: u32,
        options: u32,
        ea: *const u8,
        ea_length: u32,
    ) -> u32;
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
    fn NtSetInformationThread(h: Handle, class: u32, buf: *const u8, len: u32) -> u32;
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
    fn NtCancelIoFileEx(h: Handle, request: *mut c_void, iosb: *mut [usize; 2]) -> u32;
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
    fn NtCancelTimer(h: Handle, current: *mut u8) -> u32;
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

/// The PEB (TEB.ProcessEnvironmentBlock).
fn peb() -> usize {
    teb_read(TEB_PEB)
}

/// The process parameters (PEB.ProcessParameters, always set).
fn process_parameters() -> usize {
    // SAFETY: the PEB's ProcessParameters field.
    unsafe { *((peb() + PEB_PROCESS_PARAMETERS) as *const usize) }
}

/// Reports an unsupported part of `function` to the missing-features ledger.
fn unsupported(function: &[u8], detail: u64) {
    // SAFETY: a valid buffer; the service only reads it.
    unsafe { GalaRecordMissing(function.as_ptr(), function.len(), 2, detail) };
}

/// Records something worth knowing that is not a gap (a refused input, a
/// fallback taken) in the ledger as "other".
fn note(what: &[u8], detail: u64) {
    // SAFETY: a valid buffer; the service only reads it.
    unsafe { GalaRecordMissing(what.as_ptr(), what.len(), 5, detail) };
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
    teb_read(TEB_LAST_ERROR) as u32
}

#[unsafe(no_mangle)]
pub extern "system" fn SetLastError(code: u32) {
    teb_write_u32(TEB_LAST_ERROR, code);
}

#[unsafe(no_mangle)]
pub extern "system" fn GetCurrentProcessId() -> u32 {
    teb_read(TEB_CLIENT_PROCESS) as u32
}

#[unsafe(no_mangle)]
pub extern "system" fn GetCurrentThreadId() -> u32 {
    teb_read(TEB_CLIENT_THREAD) as u32
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
    let Some(offset) = std_handle_offset(which) else {
        return fail_with(6, -1isize as Handle); // INVALID_HANDLE_VALUE
    };
    // SAFETY: the process parameters' standard handle.
    unsafe { *((process_parameters() + offset) as *const Handle) }
}

/// Where the process parameters keep standard handle `which`
/// (STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE: -10, -11, -12).
fn std_handle_offset(which: u32) -> Option<usize> {
    let n = match which as i32 {
        -10 => 0,
        -11 => 1,
        -12 => 2,
        _ => return None,
    };
    Some(PARAMS_STANDARD_INPUT + n * size_of::<usize>())
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
        return overlapped_io(h, buffer.cast_mut(), length, written, overlapped, true);
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
    if status != STATUS_SUCCESS {
        return fail(status, FALSE);
    }
    if !buffer.is_null() && length > 0 {
        // SAFETY: the caller's buffer of `length` bytes.
        console::wrote(h, unsafe { core::slice::from_raw_parts(buffer, length as usize) });
    }
    TRUE
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

/// VirtualAllocEx: VirtualAlloc in the process `process` names (it needs
/// PROCESS_VM_OPERATION).
#[unsafe(no_mangle)]
pub extern "system" fn VirtualAllocEx(
    process: Handle,
    address: *mut c_void,
    size: usize,
    kind: u32,
    protect: u32,
) -> *mut c_void {
    let (mut base, mut size) = (address as usize, size);
    // SAFETY: valid in/out pointers.
    let status = unsafe {
        NtAllocateVirtualMemory(process as isize, &mut base, 0, &mut size, kind, protect)
    };
    if status != STATUS_SUCCESS { fail(status, core::ptr::null_mut()) } else { base as *mut c_void }
}

/// VirtualFreeEx: VirtualFree in the process `process` names.
#[unsafe(no_mangle)]
pub extern "system" fn VirtualFreeEx(
    process: Handle,
    address: *mut c_void,
    size: usize,
    kind: u32,
) -> Bool {
    if kind == 0x8000 && size != 0 {
        return fail_with(ERROR_INVALID_PARAMETER, FALSE);
    }
    let (mut base, mut size) = (address as usize, size);
    // SAFETY: valid in/out pointers.
    let status = unsafe { NtFreeVirtualMemory(process as isize, &mut base, &mut size, kind) };
    if status != STATUS_SUCCESS { fail(status, FALSE) } else { TRUE }
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
    if status != STATUS_SUCCESS {
        return fail(status, FALSE);
    }
    console::forget(h);
    TRUE
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

/// LoadLibraryExW: the LOAD_LIBRARY_SEARCH_* flags choose where to look;
/// data files and image resources are mapped, not loaded.
#[unsafe(no_mangle)]
pub extern "system" fn LoadLibraryExW(name: *const u16, file: Handle, flags: u32) -> Handle {
    const SEARCH_FLAGS: u32 = 0x0000_1f00; // LOAD_LIBRARY_SEARCH_* (0x100..0x1000)
    // LOAD_LIBRARY_AS_DATAFILE (2), _EXCLUSIVE (0x40), AS_IMAGE_RESOURCE
    // (0x20): mapped, not loaded; the handle's low bits say which (1 for a
    // data file, 2 for an image resource), as LDR_IS_DATAFILE and
    // LDR_IS_IMAGEMAPPING test them.
    const DATA_FLAGS: u32 = 0x2 | 0x20 | 0x40;
    // LOAD_WITH_ALTERED_SEARCH_PATH (8): a path's DLL is loaded from there
    // and its dependencies are looked for in its directory first (passed
    // on as LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR; Windows then skips the
    // executable's directory, gala looks there after).
    const ALTERED: u32 = 0x8;
    const DLL_LOAD_DIR: u32 = 0x100;
    if !file.is_null() || flags & !(SEARCH_FLAGS | DATA_FLAGS | ALTERED) != 0 {
        unsupported(b"kernel32.dll!LoadLibraryExW(flags)", u64::from(flags));
        return fail_with(ERROR_INVALID_PARAMETER, core::ptr::null_mut());
    }
    if flags & DATA_FLAGS != 0 {
        return match mapping::map_image_file(name) {
            Ok(base) => (base | if flags & 0x20 != 0 { 2 } else { 1 }) as Handle,
            Err(e) => fail_with(e, core::ptr::null_mut()),
        };
    }
    let altered = flags & ALTERED != 0;
    let flags = flags & !ALTERED;
    // No search flags: those of SetDefaultDllDirectories, if any.
    let flags = if flags & SEARCH_FLAGS == 0 {
        DEFAULT_DLL_DIRECTORIES.load(core::sync::atomic::Ordering::Relaxed)
    } else {
        flags
    };
    // The LOAD_LIBRARY_SEARCH_* flags go to the loader as they are: where
    // the DLL and every dependency of this load are looked for (ntdll's
    // `map_dll`: the DLL's own directory, the application's, the
    // AddDllDirectory directories, System32). LOAD_WITH_ALTERED_SEARCH_PATH
    // is passed on as LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR.
    let search = (flags & SEARCH_FLAGS) | if altered { DLL_LOAD_DIR } else { 0 };
    load_library(name, wide_len(name), search)
}

/// SetDefaultDllDirectories' flags (0: none set).
static DEFAULT_DLL_DIRECTORIES: core::sync::atomic::AtomicU32 =
    core::sync::atomic::AtomicU32::new(0);

/// SetDefaultDllDirectories: the LOAD_LIBRARY_SEARCH_* flags LoadLibrary
/// uses when it is given none.
#[unsafe(no_mangle)]
pub extern "system" fn SetDefaultDllDirectories(flags: u32) -> Bool {
    const ALLOWED: u32 = 0x200 | 0x400 | 0x800 | 0x1000;
    if flags == 0 || flags & !ALLOWED != 0 {
        return fail_with(ERROR_INVALID_PARAMETER, FALSE);
    }
    DEFAULT_DLL_DIRECTORIES.store(flags, core::sync::atomic::Ordering::Relaxed);
    TRUE
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
    // A data file or image resource mapping (LoadLibraryExW).
    if module as usize & 3 != 0 {
        return if mapping::unmap_image_file(module as usize & !3) {
            TRUE
        } else {
            fail_with(ERROR_INVALID_HANDLE, FALSE)
        };
    }
    // SAFETY: the loader validates the module.
    let status = unsafe { LdrUnloadDll(module) };
    if status != STATUS_SUCCESS { fail(status, FALSE) } else { TRUE }
}

// ---- Console (gala has no console devices: like redirected output) ----

const ERROR_INVALID_HANDLE: u32 = 6;

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

// ---- Paths ----

const MAX_PATH_CHARS: usize = 1024;
const BACKSLASH: u16 = b'\\' as u16;

/// A path being built (UTF-16, not NUL-terminated).
struct PathBuf {
    chars: [u16; MAX_PATH_CHARS],
    len: usize,
}

impl PathBuf {
    fn new() -> PathBuf {
        PathBuf { chars: [0; MAX_PATH_CHARS], len: 0 }
    }

    fn push(&mut self, s: &[u16]) -> Result<(), u32> {
        let end = self.len + s.len();
        if end >= MAX_PATH_CHARS {
            return Err(206); // ERROR_FILENAME_EXCED_RANGE
        }
        self.chars[self.len..end].copy_from_slice(s);
        self.len = end;
        Ok(())
    }

    fn as_slice(&self) -> &[u16] {
        &self.chars[..self.len]
    }
}

fn is_sep(c: u16) -> bool {
    c == BACKSLASH || c == u16::from(b'/')
}

/// A UNC path's root (`\\server\share`, either separator): its length,
/// or None when `path` is not a UNC path (`\\?\` and `\\.\` are not:
/// the callers take them first) or names no server.
fn unc_root_len(path: &[u16]) -> Option<usize> {
    if path.len() < 3 || !is_sep(path[0]) || !is_sep(path[1]) || is_sep(path[2]) {
        return None;
    }
    let server_end = path[2..].iter().position(|&c| is_sep(c)).map_or(path.len(), |i| i + 2);
    if server_end >= path.len() {
        return Some(path.len());
    }
    let share = server_end + 1;
    Some(path[share..].iter().position(|&c| is_sep(c)).map_or(path.len(), |i| i + share))
}

/// The full Win32 path of `path` (GetFullPathNameW's rules): relative paths
/// join the current directory, a leading separator takes the current drive,
/// `/` becomes `\\`, and `.`/`..` components are resolved. `\\\\?\\` paths are
/// returned unchanged.
fn full_path(path: &[u16]) -> Result<PathBuf, u32> {
    // An empty path names nothing (it is not the current directory): every
    // file function fails with ERROR_PATH_NOT_FOUND, as on Windows. A
    // program that builds a path from an unset folder gets no file
    // (ODT's setup.exe deletes "\*" recursively once "" exists); the
    // ledger keeps the caller's mistake visible.
    if path.is_empty() {
        let what = b"kernel32.dll!empty path";
        // SAFETY: a valid buffer; 5 is the ledger's "other".
        unsafe { GalaRecordMissing(what.as_ptr(), what.len(), 5, 0) };
        return Err(3); // ERROR_PATH_NOT_FOUND
    }
    let mut out = PathBuf::new();
    let verbatim = [BACKSLASH, BACKSLASH, u16::from(b'?'), BACKSLASH];
    let device = [BACKSLASH, BACKSLASH, u16::from(b'.'), BACKSLASH];
    if path.starts_with(&verbatim) || path.starts_with(&device) {
        // `\\?\` paths are kept as they are; `\\.\` names a device
        // (`\\.\pipe\name`), also not normalized here.
        out.push(path)?;
        return Ok(out);
    }
    let drive = path.len() >= 2 && path[1] == u16::from(b':');
    let mut base = PathBuf::new();
    // The root (`X:` or a UNC path's `\\server\share`) is the first
    // `root_len` characters of `base`; `..` never climbs above it.
    let root_len;
    if let Some(n) = unc_root_len(path) {
        base.push(&path[..n])?;
        root_len = n;
    } else if drive && path.len() >= 3 && is_sep(path[2]) {
        base.push(&path[..2])?;
        root_len = 2;
    } else {
        // The current directory (with its trailing backslash).
        let mut cwd = [0u16; MAX_PATH_CHARS];
        let n = GetCurrentDirectoryW(MAX_PATH_CHARS as u32, cwd.as_mut_ptr()) as usize;
        if n == 0 || n >= MAX_PATH_CHARS {
            return Err(3);
        }
        let cwd_root = if drive { 2 } else { unc_root_len(&cwd[..n]).unwrap_or(2) };
        if drive || (!path.is_empty() && is_sep(path[0])) {
            base.push(&cwd[..cwd_root])?; // the current drive ("X:rel" keeps it simple)
        } else {
            base.push(&cwd[..n])?;
        }
        root_len = cwd_root;
    }
    let rest = if let Some(n) = unc_root_len(path) {
        &path[n..]
    } else if drive {
        &path[2..]
    } else {
        path
    };
    // Components: the base's after the root, then the path's.
    let mut parts: [(usize, usize, bool); 128] = [(0, 0, false); 128]; // (start, len, from base)
    let mut count = 0;
    let add = |src: &[u16],
               from_base: bool,
               parts: &mut [(usize, usize, bool); 128],
               count: &mut usize| {
        let mut i = 0;
        while i < src.len() {
            while i < src.len() && is_sep(src[i]) {
                i += 1;
            }
            let start = i;
            while i < src.len() && !is_sep(src[i]) {
                i += 1;
            }
            let part = &src[start..i];
            if part.is_empty() || part == [u16::from(b'.')] {
                continue;
            }
            if part == [u16::from(b'.'), u16::from(b'.')] {
                *count = count.saturating_sub(1);
            } else if *count < parts.len() {
                parts[*count] = (start, i - start, from_base);
                *count += 1;
            }
        }
    };
    let base_rest = &base.as_slice()[root_len..];
    add(base_rest, true, &mut parts, &mut count);
    add(rest, false, &mut parts, &mut count);
    let unc = root_len > 2;
    // The root with `\` separators (`//server/share` is `\\server\share`).
    for &c in &base.as_slice()[..root_len] {
        out.push(&[if c == u16::from(b'/') { BACKSLASH } else { c }])?;
    }
    if !unc || count > 0 || path.last().is_some_and(|&c| is_sep(c)) {
        out.push(&[BACKSLASH])?;
    }
    for (i, &(start, len, from_base)) in parts[..count].iter().enumerate() {
        if i > 0 {
            out.push(&[BACKSLASH])?;
        }
        let src = if from_base { base_rest } else { rest };
        out.push(&src[start..start + len])?;
    }
    // A trailing separator on the input is kept (directory syntax).
    if count > 0 && path.last().is_some_and(|&c| is_sep(c)) {
        out.push(&[BACKSLASH])?;
    }
    Ok(out)
}

/// The NT path (`\\??\\X:\\...`) of a Win32 path.
fn nt_path(path: *const u16) -> Result<PathBuf, u32> {
    let mut nt = PathBuf::new();
    // The DOS device NUL (Go's os.DevNull, `> NUL` redirections).
    let name = wide_slice(path);
    // The console's buffers (CONOUT$, CONIN$): \??\CONOUT$ and \??\CONIN$.
    if console::console_name(name).is_some() {
        nt.push(&[BACKSLASH, u16::from(b'?'), u16::from(b'?'), BACKSLASH])?;
        nt.push(name)?;
        return Ok(nt);
    }
    if name.len() == 3 && name.iter().zip(b"NUL").all(|(&c, &n)| c & !0x20 == u16::from(n)) {
        nt.push(&[BACKSLASH, u16::from(b'?'), u16::from(b'?'), BACKSLASH])?;
        nt.push(&[u16::from(b'N'), u16::from(b'U'), u16::from(b'L')])?;
        return Ok(nt);
    }
    let full = full_path(name)?;
    let verbatim = [BACKSLASH, BACKSLASH, u16::from(b'?'), BACKSLASH];
    let device = [BACKSLASH, BACKSLASH, u16::from(b'.'), BACKSLASH];
    if full.as_slice().starts_with(&verbatim) || full.as_slice().starts_with(&device) {
        nt.push(&[BACKSLASH, u16::from(b'?'), u16::from(b'?'), BACKSLASH])?;
        nt.push(&full.as_slice()[4..])?;
    } else if unc_root_len(full.as_slice()).is_some() {
        // `\\server\share\x` is `\??\UNC\server\share\x` (nt::wslfs serves
        // WSL's servers below it).
        nt.push(&[BACKSLASH, u16::from(b'?'), u16::from(b'?'), BACKSLASH])?;
        nt.push(&[u16::from(b'U'), u16::from(b'N'), u16::from(b'C')])?;
        nt.push(&full.as_slice()[1..])?;
    } else {
        nt.push(&[BACKSLASH, u16::from(b'?'), u16::from(b'?'), BACKSLASH])?;
        nt.push(full.as_slice())?;
    }
    Ok(nt)
}

/// Runs `f` with OBJECT_ATTRIBUTES for the NT path of `path`
/// (case-insensitive).
fn with_object_attributes<R>(
    path: *const u16,
    f: impl FnOnce(&ObjectAttributes) -> R,
) -> Result<R, u32> {
    with_object_attributes_sa(path, core::ptr::null_mut(), f)
}

/// SECURITY_ATTRIBUTES.
#[repr(C)]
pub(crate) struct SecurityAttributes {
    pub length: u32,
    pub descriptor: usize,
    pub inherit: Bool,
}

/// The descriptor in SECURITY_ATTRIBUTES (or 0).
pub(crate) fn security_descriptor(sa: *mut c_void) -> usize {
    // SAFETY: the caller's SECURITY_ATTRIBUTES (or NULL).
    if sa.is_null() {
        0
    } else {
        unsafe { (&raw const (*sa.cast::<SecurityAttributes>()).descriptor).read_unaligned() }
    }
}

/// with_object_attributes with the SECURITY_ATTRIBUTES' descriptor and
/// inheritance.
fn with_object_attributes_sa<R>(
    path: *const u16,
    sa: *mut c_void,
    f: impl FnOnce(&ObjectAttributes) -> R,
) -> Result<R, u32> {
    let nt = nt_path(path)?;
    let name = UnicodeString {
        length: (nt.len * 2) as u16,
        maximum_length: (nt.len * 2) as u16,
        buffer: nt.chars.as_ptr(),
    };
    let attributes = ObjectAttributes {
        length: core::mem::size_of::<ObjectAttributes>() as u32,
        root_directory: 0,
        object_name: &name,
        // OBJ_CASE_INSENSITIVE, OBJ_INHERIT
        attributes: 0x40 | if sync::inheritable(sa) { 0x2 } else { 0 },
        security_descriptor: security_descriptor(sa),
        security_quality_of_service: 0,
    };
    Ok(f(&attributes))
}

/// GetFullPathNameW: the full path's length (or the size needed, with the
/// NUL); `file_part` points at its last component.
#[unsafe(no_mangle)]
pub extern "system" fn GetFullPathNameW(
    path: *const u16,
    size: u32,
    buf: *mut u16,
    file_part: *mut *mut u16,
) -> u32 {
    let full = match full_path(wide_slice(path)) {
        Ok(f) => f,
        Err(e) => return fail_with(e, 0),
    };
    let n = copy_out(full.as_slice(), buf, size);
    if !file_part.is_null() && n as usize == full.len {
        let last = full.as_slice().iter().rposition(|&c| c == BACKSLASH).map_or(0, |i| i + 1);
        // SAFETY: the caller's output pointer; `last` lies in `buf`.
        unsafe { *file_part = if last < full.len { buf.add(last) } else { core::ptr::null_mut() } };
    }
    n
}

// ---- Files ----

const SYNCHRONIZE: u32 = 0x10_0000;
const FILE_READ_ATTRIBUTES: u32 = 0x80;
const FILE_LIST_DIRECTORY: u32 = 0x1;
const DELETE: u32 = 0x1_0000;
const SHARE_ALL: u32 = 7;
const FILE_DIRECTORY_FILE: u32 = 0x1;
const FILE_SYNCHRONOUS_IO_NONALERT: u32 = 0x20;
const FILE_NON_DIRECTORY_FILE: u32 = 0x40;
const FILE_OPEN_FOR_BACKUP_INTENT: u32 = 0x4000;
const FILE_OPEN_REPARSE_POINT: u32 = 0x20_0000;

/// NtCreateFile on a Win32 path; Err: the Win32 error.
fn open_path(
    path: *const u16,
    access: u32,
    share: u32,
    disposition: u32,
    options: u32,
    attributes: u32,
) -> Result<(Handle, usize), u32> {
    open_path_sa(path, access, share, disposition, options, attributes, core::ptr::null_mut())
}

/// open_path with SECURITY_ATTRIBUTES (descriptor, inheritance).
fn open_path_sa(
    path: *const u16,
    access: u32,
    share: u32,
    disposition: u32,
    options: u32,
    attributes: u32,
    sa: *mut c_void,
) -> Result<(Handle, usize), u32> {
    let (mut h, mut iosb) = (core::ptr::null_mut(), [0usize; 2]);
    let status = with_object_attributes_sa(path, sa, |oa| {
        // SAFETY: valid pointers for the call.
        unsafe {
            NtCreateFile(
                &mut h,
                access,
                oa,
                &mut iosb,
                core::ptr::null(),
                attributes,
                share,
                disposition,
                options,
                core::ptr::null(),
                0,
            )
        }
    })?;
    if status != STATUS_SUCCESS {
        // SAFETY: plain call.
        return Err(unsafe { RtlNtStatusToDosError(status) });
    }
    Ok((h, iosb[1]))
}

/// CreateFileW: Win32 dispositions and flags mapped to NtCreateFile.
#[unsafe(no_mangle)]
pub extern "system" fn CreateFileW(
    path: *const u16,
    access: u32,
    share: u32,
    security: *mut c_void,
    disposition: u32,
    flags: u32,
    template: Handle,
) -> Handle {
    const FILE_FLAG_OVERLAPPED: u32 = 0x4000_0000;
    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
    const FILE_FLAG_DELETE_ON_CLOSE: u32 = 0x0400_0000;
    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
    if !template.is_null() {
        unsupported(b"kernel32.dll!CreateFileW(template)", 0);
    }
    // SAFETY: the caller's NUL-terminated path (its first unit).
    if !path.is_null() && unsafe { *path } == 0 {
        // An empty name: no such path (Windows; Wine's test_CreateFile2).
        return fail_with(3, INVALID_HANDLE_VALUE); // ERROR_PATH_NOT_FOUND
    }
    let nt_disposition = match disposition {
        1 => 2, // CREATE_NEW -> FILE_CREATE
        2 => 5, // CREATE_ALWAYS -> FILE_OVERWRITE_IF
        3 => 1, // OPEN_EXISTING -> FILE_OPEN
        4 => 3, // OPEN_ALWAYS -> FILE_OPEN_IF
        5 => 4, // TRUNCATE_EXISTING -> FILE_OVERWRITE
        _ => return fail_with(ERROR_INVALID_PARAMETER, INVALID_HANDLE_VALUE),
    };
    let mut options = 0;
    if flags & FILE_FLAG_OVERLAPPED == 0 {
        options |= FILE_SYNCHRONOUS_IO_NONALERT;
    }
    if flags & FILE_FLAG_BACKUP_SEMANTICS != 0 {
        options |= FILE_OPEN_FOR_BACKUP_INTENT;
    } else {
        options |= FILE_NON_DIRECTORY_FILE;
    }
    // FILE_FLAG_DELETE_ON_CLOSE asks for DELETE access too (as Windows'
    // CreateFileW does: NT refuses FILE_DELETE_ON_CLOSE without it).
    let mut access = access;
    if flags & FILE_FLAG_DELETE_ON_CLOSE != 0 {
        options |= 0x1000;
        access |= 0x1_0000; // DELETE
    }
    if flags & FILE_FLAG_OPEN_REPARSE_POINT != 0 {
        options |= FILE_OPEN_REPARSE_POINT;
    }
    // The caching flags, as CreateFile's documentation maps them to
    // NtCreateFile's options (FileModeInformation reports them): data
    // written through at once (FILE_WRITE_THROUGH), no buffering
    // (FILE_NO_INTERMEDIATE_BUFFERING: whole sectors only), the access
    // pattern hints (FILE_SEQUENTIAL_ONLY, FILE_RANDOM_ACCESS).
    for (flag, option) in [
        (0x8000_0000u32, 0x0002u32), // FILE_FLAG_WRITE_THROUGH
        (0x2000_0000, 0x0008),       // FILE_FLAG_NO_BUFFERING
        (0x0800_0000, 0x0004),       // FILE_FLAG_SEQUENTIAL_SCAN
        (0x1000_0000, 0x0800),       // FILE_FLAG_RANDOM_ACCESS
    ] {
        if flags & flag != 0 {
            options |= option;
        }
    }
    // As Windows' CreateFileW: FILE_READ_ATTRIBUTES always, SYNCHRONIZE
    // for synchronous handles.
    let synchronize = if flags & FILE_FLAG_OVERLAPPED == 0 { SYNCHRONIZE } else { 0 };
    // FILE_FLAG_DELETE_ON_CLOSE needs DELETE access (NtCreateFile refuses
    // FILE_DELETE_ON_CLOSE without it); Windows' CreateFileW asks for it,
    // so GENERIC_READ | GENERIC_WRITE with the flag works (NSIS's
    // installers open their temporary data file so).
    let delete = if flags & FILE_FLAG_DELETE_ON_CLOSE != 0 { 0x0001_0000 } else { 0 };
    let access = access | synchronize | FILE_READ_ATTRIBUTES | delete;
    match open_path_sa(path, access, share, nt_disposition, options, flags & 0x7fff, security) {
        Ok((h, information)) => {
            // OPEN_ALWAYS/CREATE_ALWAYS on an existing file: success, with
            // ERROR_ALREADY_EXISTS (FILE_OPENED = 1, FILE_OVERWRITTEN = 3).
            let existed = matches!(disposition, 2 | 4) && matches!(information, 1 | 3);
            SetLastError(if existed { ERROR_ALREADY_EXISTS } else { 0 });
            if let Some(input) = console::console_name(wide_slice(path)) {
                console::register(h, input);
            }
            h
        }
        // CREATE_NEW on an existing file: ERROR_FILE_EXISTS, not 183.
        Err(ERROR_ALREADY_EXISTS) if disposition == 1 => fail_with(80, INVALID_HANDLE_VALUE),
        Err(e) => fail_with(e, INVALID_HANDLE_VALUE),
    }
}

/// ReadFile: synchronous reads (end of file reads 0 bytes, TRUE).
#[unsafe(no_mangle)]
pub extern "system" fn ReadFile(
    h: Handle,
    buffer: *mut u8,
    length: u32,
    read: *mut u32,
    overlapped: *mut c_void,
) -> Bool {
    if !overlapped.is_null() {
        return overlapped_io(h, buffer, length, read, overlapped, false);
    }
    let mut iosb = [0usize; 2];
    // SAFETY: the caller's buffer; a valid IO_STATUS_BLOCK.
    let mut status = unsafe {
        NtReadFile(h, 0, 0, 0, &mut iosb, buffer, length, core::ptr::null(), core::ptr::null())
    };
    if status == STATUS_PENDING {
        // SAFETY: waits for the read to complete.
        status = unsafe { NtWaitForSingleObject(h, 0, core::ptr::null()) };
        if status == STATUS_SUCCESS {
            status = iosb[0] as u32;
        }
    }
    let count = if status == 0xc000_0011 { 0 } else { iosb[1] as u32 }; // STATUS_END_OF_FILE
    if !read.is_null() {
        // SAFETY: the caller's output pointer.
        unsafe { *read = count };
    }
    match status {
        STATUS_SUCCESS | 0xc000_0011 => TRUE,
        _ => fail(status, FALSE),
    }
}

/// LPOVERLAPPED_COMPLETION_ROUTINE(dwErrorCode, dwNumberOfBytesTransfered,
/// lpOverlapped).
type CompletionRoutine = extern "system" fn(u32, u32, *mut c_void);

/// The I/O APC of ReadFileEx/WriteFileEx: ApcContext is the caller's
/// completion routine, the status block its OVERLAPPED.
extern "system" fn io_completion(context: usize, iosb: *mut [usize; 2], _reserved: u32) {
    // SAFETY: the routine and OVERLAPPED the I/O was started with.
    unsafe {
        let routine = core::mem::transmute::<usize, CompletionRoutine>(context);
        let (status, count) = ((*iosb)[0] as u32, (*iosb)[1] as u32);
        let error = if status == STATUS_SUCCESS { 0 } else { RtlNtStatusToDosError(status) };
        routine(error, count, iosb.cast());
    }
}

/// ReadFileEx/WriteFileEx: an I/O at the OVERLAPPED's offset whose
/// completion routine runs as an APC in the thread's next alertable wait
/// (its hEvent is not used). TRUE once the I/O is under way or done.
fn io_ex(
    h: Handle,
    buffer: *mut u8,
    length: u32,
    ov: *mut c_void,
    routine: Option<CompletionRoutine>,
    write: bool,
) -> Bool {
    let (Some(routine), false) = (routine, ov.is_null()) else {
        return fail_with(ERROR_INVALID_PARAMETER, FALSE);
    };
    let ov = ov.cast::<Overlapped>();
    // SAFETY: the caller's OVERLAPPED, valid until the routine has run.
    let offset = unsafe {
        (*ov).internal = STATUS_PENDING as usize;
        (*ov).internal_high = 0;
        (*ov).position() as i64
    };
    let iosb = ov.cast::<[usize; 2]>();
    let (apc, context) = (io_completion as *const () as usize, routine as *const () as usize);
    // SAFETY: the caller's buffer of `length` bytes and OVERLAPPED.
    let status = unsafe {
        if write {
            NtWriteFile(h, 0, apc, context, iosb, buffer, length, &offset, core::ptr::null())
        } else {
            NtReadFile(h, 0, apc, context, iosb, buffer, length, &offset, core::ptr::null())
        }
    };
    match status {
        0xc000_0011 => fail_with(38, FALSE), // STATUS_END_OF_FILE: ERROR_HANDLE_EOF
        s if s >= 0xc000_0000 => fail(s, FALSE),
        _ => TRUE,
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn ReadFileEx(
    h: Handle,
    buffer: *mut u8,
    length: u32,
    ov: *mut c_void,
    routine: Option<CompletionRoutine>,
) -> Bool {
    io_ex(h, buffer, length, ov, routine, false)
}

#[unsafe(no_mangle)]
pub extern "system" fn WriteFileEx(
    h: Handle,
    buffer: *const u8,
    length: u32,
    ov: *mut c_void,
    routine: Option<CompletionRoutine>,
) -> Bool {
    io_ex(h, buffer.cast_mut(), length, ov, routine, true)
}

/// ReadFile/WriteFile with an OVERLAPPED: it is the status block, its
/// Offset/OffsetHigh the file offset, its hEvent the event (and, unless
/// its low bit is set, the completion packet carries it). An I/O that is
/// still pending fails with ERROR_IO_PENDING; a read at the end of the
/// file fails with ERROR_HANDLE_EOF.
fn overlapped_io(
    h: Handle,
    buffer: *mut u8,
    length: u32,
    count: *mut u32,
    ov: *mut c_void,
    write: bool,
) -> Bool {
    overlapped_request(count, ov, |event, context, iosb, offset| {
        // SAFETY: the caller's OVERLAPPED and buffer of `length` bytes.
        unsafe {
            if write {
                NtWriteFile(h, event, 0, context, iosb, buffer, length, offset, core::ptr::null())
            } else {
                NtReadFile(h, event, 0, context, iosb, buffer, length, offset, core::ptr::null())
            }
        }
    })
}

#[cfg_attr(
    target_arch = "x86",
    link(name = "ntdll", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "ntdll", kind = "raw-dylib"))]
unsafe extern "system" {
    fn NtReadFileScatter(
        h: Handle,
        event: usize,
        apc: usize,
        apc_context: usize,
        iosb: *mut [usize; 2],
        segments: *const u64,
        length: u32,
        offset: *const i64,
        key: *const u32,
    ) -> u32;
    fn NtWriteFileGather(
        h: Handle,
        event: usize,
        apc: usize,
        apc_context: usize,
        iosb: *mut [usize; 2],
        segments: *const u64,
        length: u32,
        offset: *const i64,
        key: *const u32,
    ) -> u32;
}

/// ReadFileScatter: an overlapped read into page-sized buffers (the
/// FILE_SEGMENT_ELEMENT array), at the OVERLAPPED's offset.
#[unsafe(no_mangle)]
pub extern "system" fn ReadFileScatter(
    h: Handle,
    segments: *const u64,
    length: u32,
    _reserved: *mut u32,
    ov: *mut c_void,
) -> Bool {
    if ov.is_null() || segments.is_null() {
        return fail_with(ERROR_INVALID_PARAMETER, FALSE);
    }
    overlapped_request(core::ptr::null_mut(), ov, |event, context, iosb, offset| {
        // SAFETY: the caller's segment array and OVERLAPPED.
        unsafe {
            NtReadFileScatter(
                h,
                event,
                0,
                context,
                iosb,
                segments,
                length,
                offset,
                core::ptr::null(),
            )
        }
    })
}

/// WriteFileGather: an overlapped write from page-sized buffers.
#[unsafe(no_mangle)]
pub extern "system" fn WriteFileGather(
    h: Handle,
    segments: *const u64,
    length: u32,
    _reserved: *mut u32,
    ov: *mut c_void,
) -> Bool {
    if ov.is_null() || segments.is_null() {
        return fail_with(ERROR_INVALID_PARAMETER, FALSE);
    }
    overlapped_request(core::ptr::null_mut(), ov, |event, context, iosb, offset| {
        // SAFETY: the caller's segment array and OVERLAPPED.
        unsafe {
            NtWriteFileGather(
                h,
                event,
                0,
                context,
                iosb,
                segments,
                length,
                offset,
                core::ptr::null(),
            )
        }
    })
}

/// An overlapped request: the OVERLAPPED {Internal, InternalHigh, Offset,
/// OffsetHigh, hEvent} is the IO_STATUS_BLOCK and gives the offset and
/// event (its low bit set: no completion port packet); `issue` makes the
/// system call.
fn overlapped_request(
    count: *mut u32,
    ov: *mut c_void,
    issue: impl FnOnce(usize, usize, *mut [usize; 2], *const i64) -> u32,
) -> Bool {
    let ov = ov.cast::<Overlapped>();
    // SAFETY: the caller's OVERLAPPED, valid until the I/O completes.
    let (offset, event) = unsafe {
        (*ov).internal = STATUS_PENDING as usize;
        (*ov).internal_high = 0;
        ((*ov).position() as i64, (*ov).event)
    };
    let context = if event & 1 != 0 { 0 } else { ov as usize };
    let iosb = ov.cast::<[usize; 2]>();
    let status = issue(event & !1, context, iosb, &offset);
    match status {
        STATUS_SUCCESS => {
            if !count.is_null() {
                // SAFETY: done: InternalHigh holds the count.
                unsafe { *count = (*ov).internal_high as u32 };
            }
            TRUE
        }
        STATUS_PENDING => fail_with(997, FALSE), // ERROR_IO_PENDING
        0xc000_0011 => {
            if !count.is_null() {
                // SAFETY: the caller's count.
                unsafe { *count = 0 };
            }
            fail_with(38, FALSE) // STATUS_END_OF_FILE: ERROR_HANDLE_EOF
        }
        _ => fail(status, FALSE),
    }
}

/// NtQueryInformationFile into `out`.
fn query_file(h: Handle, class: u32, out: &mut [u8]) -> Result<(), u32> {
    let mut iosb = [0usize; 2];
    // SAFETY: `out` is a valid buffer of its length.
    let status =
        unsafe { NtQueryInformationFile(h, &mut iosb, out.as_mut_ptr(), out.len() as u32, class) };
    if status != STATUS_SUCCESS {
        // SAFETY: plain call.
        return Err(unsafe { RtlNtStatusToDosError(status) });
    }
    Ok(())
}

fn set_file(h: Handle, class: u32, data: &[u8]) -> Result<(), u32> {
    let mut iosb = [0usize; 2];
    // SAFETY: `data` is a valid buffer of its length.
    let status =
        unsafe { NtSetInformationFile(h, &mut iosb, data.as_ptr(), data.len() as u32, class) };
    if status != STATUS_SUCCESS {
        // SAFETY: plain call.
        return Err(unsafe { RtlNtStatusToDosError(status) });
    }
    Ok(())
}

fn i64_at(b: &[u8], i: usize) -> i64 {
    u64_at(b, i) as i64
}

/// SetFilePointerEx: FILE_BEGIN (0), FILE_CURRENT (1), FILE_END (2).
#[unsafe(no_mangle)]
pub extern "system" fn SetFilePointerEx(
    h: Handle,
    distance: i64,
    new: *mut i64,
    method: u32,
) -> Bool {
    let base = match method {
        0 => Ok(0),
        1 => {
            let mut b = [0u8; 8];
            query_file(h, 14, &mut b).map(|()| i64_at(&b, 0)) // FilePositionInformation
        }
        2 => {
            let mut b = [0u8; 24];
            query_file(h, 5, &mut b).map(|()| i64_at(&b, 8)) // FileStandardInformation.EndOfFile
        }
        _ => Err(ERROR_INVALID_PARAMETER),
    };
    let position = match base.map(|b| b + distance) {
        Ok(p) if p >= 0 => p,
        Ok(_) => return fail_with(131, FALSE), // ERROR_NEGATIVE_SEEK
        Err(e) => return fail_with(e, FALSE),
    };
    if let Err(e) = set_file(h, 14, &position.to_le_bytes()) {
        return fail_with(e, FALSE);
    }
    if !new.is_null() {
        // SAFETY: the caller's output pointer.
        unsafe { *new = position };
    }
    TRUE
}

#[unsafe(no_mangle)]
pub extern "system" fn GetFileSizeEx(h: Handle, size: *mut i64) -> Bool {
    let mut b = [0u8; 24];
    match query_file(h, 5, &mut b) {
        Ok(()) => {
            // SAFETY: the caller's output pointer.
            unsafe { *size = i64_at(&b, 8) };
            TRUE
        }
        Err(e) => fail_with(e, FALSE),
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn FlushFileBuffers(h: Handle) -> Bool {
    let mut iosb = [0usize; 2];
    // SAFETY: valid IO_STATUS_BLOCK.
    let status = unsafe { NtFlushBuffersFile(h, &mut iosb) };
    if status != STATUS_SUCCESS { fail(status, FALSE) } else { TRUE }
}

/// FILETIME halves of an NT time.
fn filetime(t: i64) -> [u32; 2] {
    [t as u32, (t as u64 >> 32) as u32]
}

/// GetFileInformationByHandle: BY_HANDLE_FILE_INFORMATION from the basic,
/// standard, internal (index) and volume (serial) information.
#[unsafe(no_mangle)]
pub extern "system" fn GetFileInformationByHandle(h: Handle, info: *mut u32) -> Bool {
    let (mut basic, mut standard, mut internal) = ([0u8; 40], [0u8; 24], [0u8; 8]);
    let result = query_file(h, 4, &mut basic)
        .and_then(|()| query_file(h, 5, &mut standard))
        .and_then(|()| query_file(h, 6, &mut internal));
    if let Err(e) = result {
        return fail_with(e, FALSE);
    }
    let mut volume = [0u8; 64];
    let mut iosb = [0usize; 2];
    // SAFETY: a valid buffer (the label, if any, may be cut off: only the
    // serial number at offset 8 is used).
    unsafe { NtQueryVolumeInformationFile(h, &mut iosb, volume.as_mut_ptr(), 64, 1) };
    let [c0, c1] = filetime(i64_at(&basic, 0));
    let [a0, a1] = filetime(i64_at(&basic, 8));
    let [w0, w1] = filetime(i64_at(&basic, 16));
    let size = u64_at(&standard, 8);
    let index = u64_at(&internal, 0);
    let fields = [
        u32_at(&basic, 32),
        c0,
        c1,
        a0,
        a1,
        w0,
        w1,
        u32_at(&volume, 8),
        (size >> 32) as u32,
        size as u32,
        u32_at(&standard, 16),
        (index >> 32) as u32,
        index as u32,
    ];
    // SAFETY: the caller's 52-byte structure.
    unsafe { core::ptr::copy_nonoverlapping(fields.as_ptr(), info, fields.len()) };
    TRUE
}

/// GetFileAttributesW: NtQueryAttributesFile's FileAttributes, or
/// INVALID_FILE_ATTRIBUTES with the error.
#[unsafe(no_mangle)]
pub extern "system" fn GetFileAttributesW(path: *const u16) -> u32 {
    let mut basic = [0u8; 40];
    // SAFETY: a 40-byte FILE_BASIC_INFORMATION buffer.
    let status =
        with_object_attributes(path, |oa| unsafe { NtQueryAttributesFile(oa, basic.as_mut_ptr()) });
    match status {
        Ok(STATUS_SUCCESS) => u32_at(&basic, 32),
        Ok(status) => fail(status, u32::MAX),
        Err(e) => fail_with(e, u32::MAX),
    }
}

/// GetFileAttributesExW(GetFileExInfoStandard): attributes, times and size
/// (WIN32_FILE_ATTRIBUTE_DATA).
#[unsafe(no_mangle)]
pub extern "system" fn GetFileAttributesExW(path: *const u16, level: u32, info: *mut u32) -> Bool {
    if level != 0 {
        unsupported(b"kernel32.dll!GetFileAttributesExW(level)", u64::from(level));
        return fail_with(ERROR_INVALID_PARAMETER, FALSE);
    }
    let (h, _) = match open_path(
        path,
        FILE_READ_ATTRIBUTES | SYNCHRONIZE,
        SHARE_ALL,
        1, // FILE_OPEN
        // A symbolic link's own attributes, as GetFileAttributesW's.
        FILE_SYNCHRONOUS_IO_NONALERT | FILE_OPEN_FOR_BACKUP_INTENT | FILE_OPEN_REPARSE_POINT,
        0,
    ) {
        Ok(o) => o,
        Err(e) => return fail_with(e, FALSE),
    };
    let (mut basic, mut standard) = ([0u8; 40], [0u8; 24]);
    let result = query_file(h, 4, &mut basic).and_then(|()| query_file(h, 5, &mut standard));
    CloseHandle(h);
    if let Err(e) = result {
        return fail_with(e, FALSE);
    }
    let [c0, c1] = filetime(i64_at(&basic, 0));
    let [a0, a1] = filetime(i64_at(&basic, 8));
    let [w0, w1] = filetime(i64_at(&basic, 16));
    let size = u64_at(&standard, 8);
    let fields = [u32_at(&basic, 32), c0, c1, a0, a1, w0, w1, (size >> 32) as u32, size as u32];
    // SAFETY: the caller's 36-byte structure.
    unsafe { core::ptr::copy_nonoverlapping(fields.as_ptr(), info, fields.len()) };
    TRUE
}

#[unsafe(no_mangle)]
pub extern "system" fn CreateDirectoryW(path: *const u16, security: *mut c_void) -> Bool {
    match open_path_sa(
        path,
        FILE_LIST_DIRECTORY | SYNCHRONIZE,
        SHARE_ALL,
        2,
        FILE_DIRECTORY_FILE | FILE_SYNCHRONOUS_IO_NONALERT,
        0,
        security,
    ) {
        Ok((h, _)) => {
            CloseHandle(h);
            TRUE
        }
        Err(e) => fail_with(e, FALSE),
    }
}

/// Opens `path` for deletion and marks it (FileDispositionInformation).
fn delete_path(path: *const u16, directory: bool) -> Bool {
    // DELETE | FILE_READ_ATTRIBUTES, not synchronous (no SYNCHRONIZE): a
    // file the caller has no ACE on can be deleted through its
    // directory's FILE_DELETE_CHILD and FILE_LIST_DIRECTORY.
    // A symbolic link is deleted itself, not its target.
    let kind = if directory { FILE_DIRECTORY_FILE } else { FILE_NON_DIRECTORY_FILE };
    let options = kind | FILE_OPEN_FOR_BACKUP_INTENT | FILE_OPEN_REPARSE_POINT;
    let (h, _) = match open_path(path, DELETE | FILE_READ_ATTRIBUTES, SHARE_ALL, 1, options, 0) {
        Ok(o) => o,
        Err(e) => return fail_with(e, FALSE),
    };
    let result = set_file(h, 13, &[1]);
    CloseHandle(h);
    match result {
        Ok(()) => TRUE,
        Err(e) => fail_with(e, FALSE),
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn DeleteFileW(path: *const u16) -> Bool {
    delete_path(path, false)
}

#[unsafe(no_mangle)]
pub extern "system" fn RemoveDirectoryW(path: *const u16) -> Bool {
    delete_path(path, true)
}

/// GetVolumeInformationByHandleW: label, serial (FileFsVolumeInformation),
/// flags, maximum component length and file system name
/// (FileFsAttributeInformation).
#[allow(clippy::too_many_arguments)]
#[unsafe(no_mangle)]
pub extern "system" fn GetVolumeInformationByHandleW(
    h: Handle,
    name: *mut u16,
    name_size: u32,
    serial: *mut u32,
    max_component: *mut u32,
    flags: *mut u32,
    fs_name: *mut u16,
    fs_name_size: u32,
) -> Bool {
    let (mut volume, mut attributes, mut iosb) = ([0u8; 300], [0u8; 300], [0usize; 2]);
    // SAFETY: valid buffers.
    let status = unsafe { NtQueryVolumeInformationFile(h, &mut iosb, volume.as_mut_ptr(), 300, 1) };
    if status != STATUS_SUCCESS && status != 0x8000_0005 {
        return fail(status, FALSE);
    }
    // SAFETY: as above.
    let status =
        unsafe { NtQueryVolumeInformationFile(h, &mut iosb, attributes.as_mut_ptr(), 300, 5) };
    if status != STATUS_SUCCESS && status != 0x8000_0005 {
        return fail(status, FALSE);
    }
    let wide_at = |b: &[u8], len_at: usize, at: usize| -> [u16; 140] {
        let len = (u32_at(b, len_at) as usize / 2).min(139);
        let mut w = [0u16; 140];
        for (i, c) in w.iter_mut().enumerate().take(len) {
            *c = u16::from_le_bytes([b[at + 2 * i], b[at + 2 * i + 1]]);
        }
        w
    };
    // SAFETY: the caller's optional output pointers and buffers.
    unsafe {
        if !serial.is_null() {
            *serial = u32_at(&volume, 8);
        }
        if !max_component.is_null() {
            *max_component = u32_at(&attributes, 4);
        }
        if !flags.is_null() {
            *flags = u32_at(&attributes, 0);
        }
        if !name.is_null() {
            let label = wide_at(&volume, 12, 18);
            let n = label.iter().position(|&c| c == 0).unwrap_or(0);
            copy_out(&label[..n], name, name_size);
        }
        if !fs_name.is_null() {
            let fs = wide_at(&attributes, 8, 12);
            let n = fs.iter().position(|&c| c == 0).unwrap_or(0);
            copy_out(&fs[..n], fs_name, fs_name_size);
        }
    }
    TRUE
}

/// GetFileInformationByHandleEx: the directory listing classes (as
/// NtQueryDirectoryFile) and the basic, standard and attribute-tag ones.
#[unsafe(no_mangle)]
pub extern "system" fn GetFileInformationByHandleEx(
    h: Handle,
    class: u32,
    buffer: *mut u8,
    size: u32,
) -> Bool {
    // FILE_INFO_BY_HANDLE_CLASS -> (NT class, directory listing, restart).
    let (nt_class, listing, restart) = match class {
        0 => (4, false, false),   // FileBasicInfo
        1 => (5, false, false),   // FileStandardInfo
        2 => (9, false, false),   // FileNameInfo
        9 => (35, false, false),  // FileAttributeTagInfo
        10 => (37, true, false),  // FileIdBothDirectoryInfo
        11 => (37, true, true),   // FileIdBothDirectoryRestartInfo
        14 => (2, true, false),   // FileFullDirectoryInfo
        15 => (2, true, true),    // FileFullDirectoryRestartInfo
        18 => (59, false, false), // FileIdInfo
        _ => {
            unsupported(b"kernel32.dll!GetFileInformationByHandleEx(class)", u64::from(class));
            return fail_with(ERROR_INVALID_PARAMETER, FALSE);
        }
    };
    let mut iosb = [0usize; 2];
    let status = if listing {
        // SAFETY: the caller's buffer of `size` bytes.
        unsafe {
            NtQueryDirectoryFile(
                h,
                0,
                0,
                0,
                &mut iosb,
                buffer,
                size,
                nt_class,
                0,
                core::ptr::null(),
                u8::from(restart),
            )
        }
    } else {
        // SAFETY: as above.
        unsafe { NtQueryInformationFile(h, &mut iosb, buffer, size, nt_class) }
    };
    if status != STATUS_SUCCESS { fail(status, FALSE) } else { TRUE }
}

// ---- FindFirstFileW / FindNextFileW ----

/// What a find handle points at (allocated with VirtualAlloc).
#[repr(C)]
struct FindState {
    directory: Handle,
}

/// Fills WIN32_FIND_DATAW from one FILE_FULL_DIR_INFORMATION entry.
fn fill_find_data(entry: &[u8], data: *mut u16) {
    let ints = data.cast::<u32>();
    let name_len = (u32_at(entry, 60) as usize / 2).min(259);
    // SAFETY: the caller's WIN32_FIND_DATAW (592 bytes).
    unsafe {
        core::ptr::write_bytes(data.cast::<u8>(), 0, 592);
        *ints = u32_at(entry, 56); // dwFileAttributes
        for (i, at) in [8usize, 16, 24].iter().enumerate() {
            let [lo, hi] = filetime(i64_at(entry, *at));
            *ints.add(1 + 2 * i) = lo;
            *ints.add(2 + 2 * i) = hi;
        }
        let size = u64_at(entry, 40);
        *ints.add(7) = (size >> 32) as u32;
        *ints.add(8) = size as u32;
        // dwReserved0: a reparse point's tag (the entry's EaSize then).
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        if u32_at(entry, 56) & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            *ints.add(9) = u32_at(entry, 64);
        }
        for i in 0..name_len {
            *data.add(22 + i) = u16::from_le_bytes([entry[68 + 2 * i], entry[69 + 2 * i]]);
        }
    }
}

/// One entry from the directory (FileFullDirectoryInformation), matching
/// `mask` on the first call.
fn next_entry(directory: Handle, mask: Option<&UnicodeString>, data: *mut u16) -> Result<(), u32> {
    let mut entry = [0u8; 68 + 520];
    let mut iosb = [0usize; 2];
    let restart = mask.is_some();
    let mask = mask.map_or(core::ptr::null(), |m| m as *const UnicodeString);
    // SAFETY: valid buffer and pointers.
    let status = unsafe {
        NtQueryDirectoryFile(
            directory,
            0,
            0,
            0,
            &mut iosb,
            entry.as_mut_ptr(),
            entry.len() as u32,
            2,
            1,
            mask,
            u8::from(restart),
        )
    };
    if status != STATUS_SUCCESS {
        // SAFETY: plain call.
        return Err(unsafe { RtlNtStatusToDosError(status) });
    }
    fill_find_data(&entry, data);
    Ok(())
}

/// A Win32 pattern as the NT wildcards that keep its DOS meaning: `?`
/// becomes DOS_QM (`>`), a `.` before `?` or `*` DOS_DOT (`"`), a `*`
/// before `.` DOS_STAR (`<`); so `*.*` (`<"*`) matches names without a
/// dot too, and `*.txt` only that extension. The length written to `out`.
fn dos_wildcards(mask: &[u16], out: &mut [u16]) -> usize {
    let n = mask.len().min(out.len());
    for i in 0..n {
        let next = mask.get(i + 1).copied();
        let (star, dot, qm) = (u16::from(b'*'), u16::from(b'.'), u16::from(b'?'));
        out[i] = match mask[i] {
            c if c == qm => u16::from(b'>'),
            c if c == dot && (next == Some(qm) || next == Some(star)) => u16::from(b'"'),
            c if c == star && next == Some(dot) => u16::from(b'<'),
            c => c,
        };
    }
    n
}

/// FindFirstFileW: opens the pattern's directory and returns the first
/// entry matching its last component (`dos_wildcards`).
#[unsafe(no_mangle)]
pub extern "system" fn FindFirstFileW(pattern: *const u16, data: *mut u16) -> Handle {
    let full = match full_path(wide_slice(pattern)) {
        Ok(f) => f,
        Err(e) => return fail_with(e, INVALID_HANDLE_VALUE),
    };
    let path = full.as_slice();
    let split = path.iter().rposition(|&c| c == BACKSLASH).map_or(0, |i| i + 1);
    let (dir, mask) = (&path[..split], &path[split..]);
    let mut dir_z = [0u16; MAX_PATH_CHARS + 1];
    dir_z[..dir.len()].copy_from_slice(dir);
    let opened = open_path(
        dir_z.as_ptr(),
        FILE_LIST_DIRECTORY | SYNCHRONIZE,
        SHARE_ALL,
        1,
        FILE_DIRECTORY_FILE | FILE_SYNCHRONOUS_IO_NONALERT | FILE_OPEN_FOR_BACKUP_INTENT,
        0,
    );
    let (directory, _) = match opened {
        Ok(o) => o,
        Err(e) => return fail_with(e, INVALID_HANDLE_VALUE),
    };
    let mut dos = [0u16; MAX_PATH_CHARS + 1];
    let n = dos_wildcards(mask, &mut dos);
    let mask = UnicodeString {
        length: (n * 2) as u16,
        maximum_length: (n * 2) as u16,
        buffer: dos.as_ptr(),
    };
    if let Err(e) = next_entry(directory, Some(&mask), data) {
        CloseHandle(directory);
        // No match: ERROR_FILE_NOT_FOUND (as FindFirstFile reports it).
        return fail_with(if e == 18 { 2 } else { e }, INVALID_HANDLE_VALUE);
    }
    let state = VirtualAlloc(core::ptr::null_mut(), 0x1000, 0x3000, 4) as *mut FindState;
    if state.is_null() {
        CloseHandle(directory);
        return INVALID_HANDLE_VALUE;
    }
    // SAFETY: a fresh allocation.
    unsafe { state.write(FindState { directory }) };
    state.cast()
}

#[unsafe(no_mangle)]
pub extern "system" fn FindNextFileW(find: Handle, data: *mut u16) -> Bool {
    // SAFETY: `find` came from FindFirstFileW.
    let directory = unsafe { (*find.cast::<FindState>()).directory };
    match next_entry(directory, None, data) {
        Ok(()) => TRUE,
        Err(e) => fail_with(e, FALSE),
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn FindClose(find: Handle) -> Bool {
    // SAFETY: `find` came from FindFirstFileW.
    let directory = unsafe { (*find.cast::<FindState>()).directory };
    if !directory.is_null() {
        CloseHandle(directory);
    }
    VirtualFree(find, 0, 0x8000)
}

/// FindFirstStreamW(name, FindStreamInfoStandard, *WIN32_FIND_STREAM_DATA,
/// 0): gala's files have only their unnamed data stream, "::$DATA" with
/// the file's size; a directory has none (ERROR_HANDLE_EOF).
#[unsafe(no_mangle)]
pub extern "system" fn FindFirstStreamW(
    name: *const u16,
    level: u32,
    data: *mut u8,
    _flags: u32,
) -> Handle {
    if level != 0 || data.is_null() {
        return fail_with(ERROR_INVALID_PARAMETER, INVALID_HANDLE_VALUE);
    }
    let mut info = [0u32; 9];
    if GetFileAttributesExW(name, 0, info.as_mut_ptr()) == FALSE {
        return INVALID_HANDLE_VALUE;
    }
    if info[0] & 0x10 != 0 {
        return fail_with(38, INVALID_HANDLE_VALUE); // a directory: ERROR_HANDLE_EOF
    }
    let size = (u64::from(info[7]) << 32) | u64::from(info[8]);
    // WIN32_FIND_STREAM_DATA: StreamSize, then cStreamName[MAX_PATH + 36].
    // SAFETY: the caller's structure (8 + 592 bytes).
    unsafe {
        core::ptr::write_bytes(data, 0, 8 + 2 * 296);
        data.cast::<u64>().write_unaligned(size);
        for (i, c) in "::$DATA".encode_utf16().enumerate() {
            data.add(8 + 2 * i).cast::<u16>().write_unaligned(c);
        }
    }
    // A find handle FindClose takes (no directory behind it).
    let state = VirtualAlloc(core::ptr::null_mut(), 0x1000, 0x3000, 4) as *mut FindState;
    if state.is_null() {
        return INVALID_HANDLE_VALUE;
    }
    // SAFETY: a fresh allocation.
    unsafe { state.write(FindState { directory: core::ptr::null_mut() }) };
    state.cast()
}

/// FindNextStreamW: the only stream was the first (ERROR_HANDLE_EOF).
#[unsafe(no_mangle)]
pub extern "system" fn FindNextStreamW(_find: Handle, _data: *mut u8) -> Bool {
    fail_with(38, FALSE)
}

/// FindFirstFileNameW(name, 0, &length, linkName): the file's first hard
/// link name, from the volume root without the drive (`\dir\file`).
/// gala's file system has one name per file, so this is the file's own
/// path; `length` (in characters with the NUL) gets the size needed,
/// with ERROR_MORE_DATA when the buffer is too small.
#[unsafe(no_mangle)]
pub extern "system" fn FindFirstFileNameW(
    name: *const u16,
    flags: u32,
    length: *mut u32,
    link: *mut u16,
) -> Handle {
    if name.is_null() || length.is_null() || flags != 0 {
        return fail_with(ERROR_INVALID_PARAMETER, INVALID_HANDLE_VALUE);
    }
    let h = CreateFileW(name, 0, 7, core::ptr::null_mut(), 3, 0x0200_0000, core::ptr::null_mut());
    if h == INVALID_HANDLE_VALUE {
        return INVALID_HANDLE_VALUE;
    }
    // FILE_STANDARD_INFORMATION's NumberOfLinks: the other names of a
    // file with hard links are not listed yet (reported).
    let mut standard = [0u8; 24];
    let mut iosb = [0usize; 2];
    // SAFETY: a 24-byte FILE_STANDARD_INFORMATION.
    let status = unsafe { NtQueryInformationFile(h, &mut iosb, standard.as_mut_ptr(), 24, 5) };
    let links = u32::from_le_bytes([standard[16], standard[17], standard[18], standard[19]]);
    if status == STATUS_SUCCESS && links > 1 {
        unsupported(b"kernel32.dll!FindFirstFileNameW(hard links)", links.into());
    }
    let mut path = [0u16; 1100];
    let n = files::GetFinalPathNameByHandleW(h, path.as_mut_ptr(), path.len() as u32, 4);
    CloseHandle(h);
    if n == 0 || n as usize >= path.len() {
        return INVALID_HANDLE_VALUE;
    }
    let n = n as usize;
    // SAFETY: the caller's in/out length.
    let room = unsafe { *length } as usize;
    // SAFETY: as above.
    unsafe { *length = n as u32 + 1 };
    if link.is_null() || room <= n {
        return fail_with(234, INVALID_HANDLE_VALUE); // ERROR_MORE_DATA
    }
    // SAFETY: the caller's buffer of `room` > n characters.
    unsafe {
        core::ptr::copy_nonoverlapping(path.as_ptr(), link, n);
        *link.add(n) = 0;
    }
    // A find handle FindClose takes (no directory behind it).
    let state = VirtualAlloc(core::ptr::null_mut(), 0x1000, 0x3000, 4) as *mut FindState;
    if state.is_null() {
        return INVALID_HANDLE_VALUE;
    }
    // SAFETY: a fresh allocation.
    unsafe { state.write(FindState { directory: core::ptr::null_mut() }) };
    state.cast()
}

/// FindNextFileNameW: the only name was the first (ERROR_HANDLE_EOF).
#[unsafe(no_mangle)]
pub extern "system" fn FindNextFileNameW(
    _find: Handle,
    _length: *mut u32,
    _link: *mut u16,
) -> Bool {
    fail_with(38, FALSE)
}

/// CREATEFILE2_EXTENDED_PARAMETERS (fileapi.h).
#[repr(C)]
pub struct CreateFile2Parameters {
    size: u32,
    attributes: u32,
    flags: u32,
    security_qos_flags: u32,
    security: *mut c_void,
    template: Handle,
}

/// CreateFile2(name, access, share, disposition, parameters): CreateFileW
/// with the attributes, flags and QoS flags combined, as documented;
/// parameters may be NULL.
#[unsafe(no_mangle)]
pub extern "system" fn CreateFile2(
    name: *const u16,
    access: u32,
    share: u32,
    disposition: u32,
    parameters: *const CreateFile2Parameters,
) -> Handle {
    if parameters.is_null() {
        return CreateFileW(
            name,
            access,
            share,
            core::ptr::null_mut(),
            disposition,
            0,
            core::ptr::null_mut(),
        );
    }
    // SAFETY: the caller's structure (its size checked first).
    let p = unsafe { &*parameters };
    if p.size as usize != size_of::<CreateFile2Parameters>() {
        return fail_with(ERROR_INVALID_PARAMETER, INVALID_HANDLE_VALUE);
    }
    CreateFileW(
        name,
        access,
        share,
        p.security,
        disposition,
        p.attributes | p.flags | p.security_qos_flags,
        p.template,
    )
}

/// ReOpenFile: a new handle to the file `h` names (its own file
/// position), with new access, sharing and flags.
#[unsafe(no_mangle)]
pub extern "system" fn ReOpenFile(h: Handle, access: u32, share: u32, flags: u32) -> Handle {
    let mut path = [0u16; 1100];
    let n = files::GetFinalPathNameByHandleW(h, path.as_mut_ptr(), path.len() as u32, 0);
    if n == 0 || n as usize >= path.len() {
        return INVALID_HANDLE_VALUE;
    }
    CreateFileW(
        path.as_ptr(),
        access,
        share,
        core::ptr::null_mut(),
        3,
        flags,
        core::ptr::null_mut(),
    )
}

// ---- Process-wide settings ----

static ERROR_MODE: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
static WER_FLAGS: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

#[unsafe(no_mangle)]
pub extern "system" fn SetErrorMode(mode: u32) -> u32 {
    ERROR_MODE.swap(mode, core::sync::atomic::Ordering::Relaxed)
}

/// SetHandleCount: obsolete (a 16-bit Windows limit); no effect, returns
/// the number given, as documented. distlib's script launchers (pip's
/// console-script .exe files) import it.
#[unsafe(no_mangle)]
pub extern "system" fn SetHandleCount(number: u32) -> u32 {
    number
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

/// SetThreadPriorityBoost: as SetProcessPriorityBoost.
#[unsafe(no_mangle)]
pub extern "system" fn SetThreadPriorityBoost(_thread: Handle, _disable: Bool) -> Bool {
    TRUE
}

/// GetProcessPriorityBoost/GetThreadPriorityBoost(h, *pDisablePriorityBoost):
/// FALSE, boosts enabled, as Windows starts every process and thread
/// (gala's scheduler boosts nothing, so nothing to report either way).
#[unsafe(no_mangle)]
pub extern "system" fn GetProcessPriorityBoost(_process: Handle, disabled: *mut Bool) -> Bool {
    if disabled.is_null() {
        return fail_with(ERROR_INVALID_PARAMETER, FALSE);
    }
    // SAFETY: the caller's output.
    unsafe { *disabled = FALSE };
    TRUE
}

#[unsafe(no_mangle)]
pub extern "system" fn GetThreadPriorityBoost(thread: Handle, disabled: *mut Bool) -> Bool {
    GetProcessPriorityBoost(thread, disabled)
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

/// SYSTEM_BASIC_INFORMATION (pointer-sized fields: 64 bytes on x64, 44
/// on x86).
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(crate) struct SystemBasic {
    reserved: u32,
    timer_resolution: u32,
    pub page_size: u32,
    pub physical_pages: u32,
    lowest_physical_page: u32,
    highest_physical_page: u32,
    pub allocation_granularity: u32,
    pub minimum_address: usize,
    pub maximum_address: usize,
    pub processor_mask: usize,
    pub processors: u8,
}

/// SystemBasicInformation (zeroes if the query fails).
pub(crate) fn basic_information() -> SystemBasic {
    let mut b = SystemBasic::default();
    let mut ret = 0;
    // SAFETY: a buffer of the class's size.
    unsafe {
        NtQuerySystemInformation(0, (&raw mut b).cast(), size_of::<SystemBasic>() as u32, &mut ret)
    };
    b
}

/// OVERLAPPED (32 bytes on x64, 20 on x86): Internal and InternalHigh
/// are the IO_STATUS_BLOCK of the request.
#[repr(C)]
pub(crate) struct Overlapped {
    pub internal: usize,
    pub internal_high: usize,
    pub offset: u32,
    pub offset_high: u32,
    pub event: usize,
}

impl Overlapped {
    /// The byte offset (Offset, OffsetHigh).
    pub fn position(&self) -> u64 {
        u64::from(self.offset) | u64::from(self.offset_high) << 32
    }
}

/// MEMORY_BASIC_INFORMATION (48 bytes on x64, where PartitionId sits in
/// the padding after AllocationProtect; 28 on x86).
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(crate) struct MemoryBasic {
    pub base: usize,
    pub allocation_base: usize,
    pub allocation_protect: u32,
    pub region_size: usize,
    pub state: u32,
    pub protect: u32,
    pub kind: u32,
}

/// THREAD_BASIC_INFORMATION.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(crate) struct ThreadBasic {
    pub exit_status: u32,
    pub teb: usize,
    pub client_id: [usize; 2],
    pub affinity: usize,
    pub priority: i32,
    pub base_priority: i32,
}

/// ThreadBasicInformation of `thread`, or the status.
pub(crate) fn thread_basic(thread: Handle) -> Result<ThreadBasic, u32> {
    let mut b = ThreadBasic::default();
    let mut ret = 0;
    // SAFETY: a buffer of the class's size.
    let status = unsafe {
        NtQueryInformationThread(
            thread,
            0,
            (&raw mut b).cast(),
            size_of::<ThreadBasic>() as u32,
            &mut ret,
        )
    };
    if status != STATUS_SUCCESS { Err(status) } else { Ok(b) }
}

/// PROCESS_BASIC_INFORMATION.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(crate) struct ProcessBasic {
    pub exit_status: u32,
    pub peb: usize,
    pub affinity: usize,
    pub base_priority: i32,
    pub pid: usize,
    pub parent: usize,
}

/// ProcessBasicInformation of `process`, or the status.
pub(crate) fn process_basic(process: Handle) -> Result<ProcessBasic, u32> {
    let mut b = ProcessBasic::default();
    // SAFETY: a buffer of the class's size.
    let status = unsafe {
        NtQueryInformationProcess(
            process as isize,
            0,
            (&raw mut b).cast(),
            size_of::<ProcessBasic>() as u32,
            core::ptr::null_mut(),
        )
    };
    if status != STATUS_SUCCESS { Err(status) } else { Ok(b) }
}

fn u64_at(b: &[u8], i: usize) -> u64 {
    u64::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3], b[i + 4], b[i + 5], b[i + 6], b[i + 7]])
}

fn u32_at(b: &[u8], i: usize) -> u32 {
    u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])
}

/// GetSystemInfo: the processor (in a 32-bit process, what WOW64 tells
/// it: PROCESSOR_ARCHITECTURE_INTEL) and the caller's application range.
#[unsafe(no_mangle)]
pub extern "system" fn GetSystemInfo(info: *mut SystemInfo) {
    system_info(info, PROCESSOR_ARCHITECTURE, PROCESSOR_TYPE);
}

fn system_info(info: *mut SystemInfo, architecture: u16, processor_type: u32) {
    let b = basic_information();
    let i = SystemInfo {
        processor_architecture: architecture,
        reserved: 0,
        page_size: b.page_size,
        minimum_application_address: b.minimum_address,
        maximum_application_address: b.maximum_address,
        active_processor_mask: b.processor_mask,
        number_of_processors: u32::from(b.processors),
        processor_type,
        allocation_granularity: b.allocation_granularity,
        processor_level: 6,
        processor_revision: 0,
    };
    // SAFETY: the caller's output structure.
    unsafe { info.write(i) };
}

/// GetNativeSystemInfo: the machine's processor (AMD64: the same as
/// GetSystemInfo but in a 32-bit process, where it is not x86).
#[unsafe(no_mangle)]
pub extern "system" fn GetNativeSystemInfo(info: *mut SystemInfo) {
    system_info(info, 9, 8664); // PROCESSOR_ARCHITECTURE_AMD64, PROCESSOR_AMD_X8664
}

#[unsafe(no_mangle)]
pub extern "system" fn GetProcessAffinityMask(
    process: Handle,
    process_mask: *mut usize,
    system_mask: *mut usize,
) -> Bool {
    let pbi = match process_basic(process) {
        Ok(b) => b,
        Err(status) => return fail(status, FALSE),
    };
    let system = basic_information().processor_mask;
    // SAFETY: the caller's output pointers.
    unsafe {
        *process_mask = pbi.affinity;
        *system_mask = system;
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

/// The environment block's strings (without the final empty one).
fn env_entries() -> impl Iterator<Item = &'static [u16]> {
    // SAFETY: ProcessParameters->Environment: NUL-terminated
    // strings ending with an empty one, alive until replaced (callers hold
    // ENV_LOCK).
    let block = unsafe { *((process_parameters() + PARAMS_ENVIRONMENT) as *const *const u16) };
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

/// Expands `%NAME%` references in `src` with the environment, passing
/// each resulting character to `emit`: a variable that is not set (or an
/// empty name) stays as written, its closing `%` then starting the next
/// reference, as RtlExpandEnvironmentStrings does. Returns the characters
/// emitted.
fn expand_environment(src: &[u16], mut emit: impl FnMut(u16)) -> usize {
    let percent = u16::from(b'%');
    let mut count = 0;
    let mut out = |c: u16, count: &mut usize| {
        emit(c);
        *count += 1;
    };
    let mut i = 0;
    with_env(|| {
        while i < src.len() {
            if src[i] == percent
                && let Some(len) = src[i + 1..].iter().position(|&c| c == percent)
            {
                let name = &src[i + 1..i + 1 + len];
                let value = (!name.is_empty())
                    .then(|| env_entries().map(split_entry).find(|(n, _)| same_name(n, name)))
                    .flatten();
                if let Some((_, v)) = value {
                    for &c in v {
                        out(c, &mut count);
                    }
                    i += len + 2;
                } else {
                    // Kept: the % and the name; the closing % is looked at again.
                    for &c in &src[i..i + 1 + len] {
                        out(c, &mut count);
                    }
                    i += len + 1;
                }
                continue;
            }
            out(src[i], &mut count);
            i += 1;
        }
    });
    count
}

/// ExpandEnvironmentStringsW(lpSrc, lpDst, nSize): the characters needed
/// including the NUL; the expansion is written only if it fits.
#[unsafe(no_mangle)]
pub extern "system" fn ExpandEnvironmentStringsW(src: *const u16, dst: *mut u16, size: u32) -> u32 {
    if src.is_null() {
        return fail_with(ERROR_INVALID_PARAMETER, 0);
    }
    let src = wide_slice(src);
    let mut at = 0usize;
    let n = expand_environment(src, |c| {
        if !dst.is_null() && at < size as usize {
            // SAFETY: the caller's buffer of `size` characters.
            unsafe { *dst.add(at) = c };
        }
        at += 1;
    });
    if !dst.is_null() && n < size as usize {
        // SAFETY: as above.
        unsafe { *dst.add(n) = 0 };
    }
    n as u32 + 1
}

/// ExpandEnvironmentStringsA: as the W function, in the ANSI code page
/// (UTF-8 here); the count is in bytes.
#[unsafe(no_mangle)]
pub extern "system" fn ExpandEnvironmentStringsA(src: *const u8, dst: *mut u8, size: u32) -> u32 {
    if src.is_null() {
        return fail_with(ERROR_INVALID_PARAMETER, 0);
    }
    // SAFETY: the caller's NUL-terminated string.
    let bytes = unsafe { core::ffi::CStr::from_ptr(src.cast()) }.to_bytes();
    let Ok(text) = core::str::from_utf8(bytes) else {
        return fail_with(ERROR_INVALID_PARAMETER, 0);
    };
    let mut wide = [0u16; 2048];
    let mut len = 0;
    for c in text.encode_utf16() {
        if len == wide.len() {
            return fail_with(ERROR_INVALID_PARAMETER, 0);
        }
        wide[len] = c;
        len += 1;
    }
    let mut units = [0u16; 8192];
    let mut at = 0;
    let n = expand_environment(&wide[..len], |c| {
        if at < units.len() {
            units[at] = c;
        }
        at += 1;
    });
    let mut written = 0usize;
    for c in core::char::decode_utf16(units[..n.min(units.len())].iter().copied()) {
        let mut b = [0u8; 4];
        for &byte in c.unwrap_or('?').encode_utf8(&mut b).as_bytes() {
            if !dst.is_null() && written < size as usize {
                // SAFETY: the caller's buffer of `size` bytes.
                unsafe { *dst.add(written) = byte };
            }
            written += 1;
        }
    }
    if !dst.is_null() && written < size as usize {
        // SAFETY: as above.
        unsafe { *dst.add(written) = 0 };
    }
    written as u32 + 1
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
        // SAFETY: Environment and EnvironmentSize of this
        // process's parameters, writable; readers hold ENV_LOCK.
        unsafe {
            *((params + PARAMS_ENVIRONMENT) as *mut *mut u16) = block;
            *((params + PARAMS_ENVIRONMENT_SIZE) as *mut usize) = chars * 2;
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
    // SAFETY: CurrentDirectory.DosPath, a UNICODE_STRING.
    let dos = unsafe {
        let len = usize::from(*((params + PARAMS_CURRENT_DIRECTORY) as *const u16)) / 2;
        let buf = *((params + PARAMS_CURRENT_DIRECTORY + US_BUFFER) as *const *const u16);
        core::slice::from_raw_parts(buf, len)
    };
    let path = match dos {
        [.., b] if *b == u16::from(b'\\') && dos.len() > 3 => &dos[..dos.len() - 1],
        _ => dos,
    };
    copy_out(path, buf, size)
}

/// SetCurrentDirectoryW: ProcessParameters->CurrentDirectory: DosPath
/// (with a trailing backslash, in its own buffer) and Handle (an open
/// handle to the directory, as Windows keeps one).
#[unsafe(no_mangle)]
pub extern "system" fn SetCurrentDirectoryW(path: *const u16) -> Bool {
    let mut full = match full_path(wide_slice(path)) {
        Ok(f) => f,
        Err(e) => return fail_with(e, FALSE),
    };
    if !full.as_slice().ends_with(&[BACKSLASH]) && full.push(&[BACKSLASH]).is_err() {
        return fail_with(ERROR_FILENAME_EXCED_RANGE, FALSE);
    }
    let mut z = PathBuf::new();
    if z.push(full.as_slice()).is_err() || z.push(&[0]).is_err() {
        return fail_with(ERROR_FILENAME_EXCED_RANGE, FALSE);
    }
    let attributes = GetFileAttributesW(z.chars.as_ptr());
    if attributes == u32::MAX {
        return FALSE;
    }
    if attributes & 0x10 == 0 {
        return fail_with(267, FALSE); // ERROR_DIRECTORY
    }
    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
    // FILE_TRAVERSE | SYNCHRONIZE, as the directory handle Windows keeps.
    let h = CreateFileW(
        z.chars.as_ptr(),
        0x10_0020,
        3,
        core::ptr::null_mut(),
        3,
        FILE_FLAG_BACKUP_SEMANTICS,
        core::ptr::null_mut(),
    );
    if h == INVALID_HANDLE_VALUE {
        return FALSE;
    }
    let params = process_parameters();
    // SAFETY: CurrentDirectory: DosPath (a UNICODE_STRING: Length,
    // MaximumLength, Buffer), Handle.
    unsafe {
        let max = usize::from(*((params + PARAMS_CURRENT_DIRECTORY + 2) as *const u16)) / 2;
        let chars = full.as_slice();
        if chars.len() + 1 > max {
            CloseHandle(h);
            return fail_with(ERROR_FILENAME_EXCED_RANGE, FALSE);
        }
        let buf = *((params + PARAMS_CURRENT_DIRECTORY + US_BUFFER) as *const *mut u16);
        core::ptr::copy_nonoverlapping(chars.as_ptr(), buf, chars.len());
        *buf.add(chars.len()) = 0;
        *((params + PARAMS_CURRENT_DIRECTORY) as *mut u16) = (chars.len() * 2) as u16;
        let handle = (params + PARAMS_CURRENT_DIRECTORY_HANDLE) as *mut Handle;
        let old = core::mem::replace(&mut *handle, h);
        if !old.is_null() {
            CloseHandle(old);
        }
    }
    TRUE
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

/// GetTempFileNameW(path, prefix, unique, buf): `<path>\<up to 3
/// characters of prefix><unique in upper-case hex>.tmp` into `buf`
/// (MAX_PATH characters). With unique 0 the number starts from the tick
/// count's low 16 bits and the file is created (CREATE_NEW) with the
/// first free number; otherwise only the name is made. Returns the number
/// used, 0 on failure (the last error: CreateFileW's, ERROR_FILE_EXISTS
/// when every number is taken, ERROR_BUFFER_OVERFLOW for a path too long).
#[unsafe(no_mangle)]
pub extern "system" fn GetTempFileNameW(
    path: *const u16,
    prefix: *const u16,
    unique: u32,
    buf: *mut u16,
) -> u32 {
    const MAX_PATH: usize = 260;
    const ERROR_BUFFER_OVERFLOW: u32 = 111;
    const ERROR_FILE_EXISTS: u32 = 80;
    if path.is_null() || buf.is_null() {
        return fail_with(ERROR_INVALID_PARAMETER, 0);
    }
    let dir = locale::c_units(path);
    let pre = locale::c_units(prefix);
    let pre = &pre[..pre.len().min(3)];
    let mut name = [0u16; MAX_PATH];
    let mut n = 0;
    for &u in dir {
        if n >= MAX_PATH - 14 {
            return fail_with(ERROR_BUFFER_OVERFLOW, 0);
        }
        name[n] = u;
        n += 1;
    }
    if n > 0 && name[n - 1] != u16::from(b'\\') && name[n - 1] != u16::from(b'/') {
        name[n] = u16::from(b'\\');
        n += 1;
    }
    for &u in pre {
        name[n] = u;
        n += 1;
    }
    let base = n;
    let write = |name: &mut [u16; MAX_PATH], number: u32| -> usize {
        let mut n = base;
        let mut started = false;
        for i in (0..4).rev() {
            let d = ((number >> (4 * i)) & 0xf) as u8;
            if d != 0 || started || i == 0 {
                started = true;
                name[n] = u16::from(if d < 10 { b'0' + d } else { b'A' + d - 10 });
                n += 1;
            }
        }
        for &b in b".tmp" {
            name[n] = u16::from(b);
            n += 1;
        }
        name[n] = 0;
        n
    };
    let copy = |name: &[u16; MAX_PATH], len: usize| {
        // SAFETY: the caller's buffer of MAX_PATH characters.
        unsafe { core::ptr::copy_nonoverlapping(name.as_ptr(), buf, len + 1) };
    };
    if unique != 0 {
        let number = unique & 0xffff;
        let len = write(&mut name, number);
        copy(&name, len);
        SetLastError(0);
        return number;
    }
    let start = (GetTickCount() & 0xffff).max(1);
    let mut number = start;
    loop {
        let len = write(&mut name, number);
        const GENERIC_WRITE: u32 = 0x4000_0000;
        const CREATE_NEW: u32 = 1;
        let h = CreateFileW(
            name.as_ptr(),
            GENERIC_WRITE,
            0,
            core::ptr::null_mut(),
            CREATE_NEW,
            0x80,
            core::ptr::null_mut(),
        );
        if h != INVALID_HANDLE_VALUE {
            CloseHandle(h);
            copy(&name, len);
            SetLastError(0);
            return number;
        }
        let e = GetLastError();
        if e != ERROR_FILE_EXISTS && e != ERROR_ALREADY_EXISTS {
            return 0;
        }
        number = if number == 0xffff { 1 } else { number + 1 };
        if number == start {
            return fail_with(ERROR_FILE_EXISTS, 0);
        }
    }
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
    // SAFETY: ProcessParameters->CommandLine (a UNICODE_STRING): its
    // Buffer.
    unsafe { *((process_parameters() + PARAMS_COMMAND_LINE + US_BUFFER) as *const *const u16) }
}

/// GetEnvironmentStringsW: a private copy of the environment block
/// (`NAME=value\0...\0`), freed with FreeEnvironmentStringsW.
#[unsafe(no_mangle)]
pub extern "system" fn GetEnvironmentStringsW() -> *mut u16 {
    // SAFETY: ProcessParameters->Environment, a block ending with an
    // empty string.
    unsafe {
        let env = *((process_parameters() + PARAMS_ENVIRONMENT) as *const *const u16);
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
    if status != STATUS_SUCCESS {
        return fail(status, FALSE);
    }
    // A duplicate of a console handle in this process is a console handle
    // too (DUPLICATE_CLOSE_SOURCE: the source's entry goes).
    let current = |p: Handle| p as isize == -1 || p == GetCurrentProcess();
    if current(source_process)
        && current(target_process)
        && !target.is_null()
        && let Some(input) = console::kind(source)
    {
        // SAFETY: the kernel wrote the new handle.
        console::register(unsafe { *target }, input);
        if options & 1 != 0 {
            console::forget(source);
        }
    }
    TRUE
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

type StartRoutine = extern "system" fn(*mut c_void) -> u32;

/// Every thread CreateThread makes starts here (with the start routine
/// and its parameter as arguments: `thread_context`), like Windows' thread
/// start thunk: run the routine, exit with its result.
extern "system" fn thread_start(start: StartRoutine, param: *mut c_void) -> ! {
    // The thread's TLS blocks and DLL_THREAD_ATTACH (Windows does this in
    // ntdll before the thread's start routine).
    // SAFETY: a plain call.
    unsafe { GalaInitializeThread() };
    ExitThread(start(param))
}

#[cfg_attr(
    target_arch = "x86",
    link(name = "ntdll", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "ntdll", kind = "raw-dylib"))]
unsafe extern "system" {
    fn GalaInitializeThread();
    fn LdrShutdownThread();
    pub(crate) fn LdrDisableThreadCalloutsForDll(module: Handle) -> u32;
}

/// The executable's optional header's pointer-sized field at `offset`
/// (an image of this DLL's architecture).
fn image_optional_field(offset: usize) -> usize {
    let base = mapping::image_base() as usize;
    // SAFETY: the executable's mapped headers.
    unsafe {
        let nt = base + (*((base + 0x3c) as *const u32)) as usize;
        ((nt + 24 + offset) as *const usize).read_unaligned()
    }
}

/// The executable's SizeOfStackReserve: the default reservation for new
/// threads, as on Windows (1 MiB if unset).
fn default_stack() -> usize {
    let reserve = image_optional_field(OPTIONAL_STACK_RESERVE);
    if reserve == 0 { 0x10_0000 } else { reserve }
}

/// The executable's SizeOfStackCommit.
fn default_stack_commit() -> usize {
    image_optional_field(OPTIONAL_STACK_COMMIT)
}

/// gala's NtSetInformationThread class: the thread's stack is freed when it
/// ends (the kernel's GALA_THREAD_FREE_STACK).
const GALA_THREAD_FREE_STACK: u32 = 0x4761_0001;

/// CreateThread: the stack is reserved and partly committed here, with a
/// guard page (see below), then NtCreateThread starts the thread at
/// `thread_start`. SECURITY_ATTRIBUTES give the thread's descriptor and
/// make the handle inheritable.
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
    if flags & !(CREATE_SUSPENDED | STACK_SIZE_PARAM_IS_A_RESERVATION) != 0 {
        unsupported(b"kernel32.dll!CreateThread(flags)", u64::from(flags));
        return fail_with(ERROR_NOT_SUPPORTED, core::ptr::null_mut());
    }
    // A reservation given as such; otherwise the size is what to commit and
    // the reservation is the executable's default (or that size, rounded
    // up to a megabyte, if larger).
    let size = if flags & STACK_SIZE_PARAM_IS_A_RESERVATION != 0 && stack_size != 0 {
        stack_size
    } else if stack_size > default_stack() {
        stack_size.next_multiple_of(0x10_0000)
    } else {
        default_stack()
    }
    .next_multiple_of(0x1_0000);
    // As Windows lays out a thread stack: the reservation, its top pages
    // committed (the requested commit size, else the header's, at least
    // 16 KiB), a guard page below them that moves down as the stack grows
    // (the kernel's guard-page handling), the bottom page never committed.
    let commit = if flags & STACK_SIZE_PARAM_IS_A_RESERVATION == 0 && stack_size != 0 {
        stack_size
    } else {
        default_stack_commit()
    }
    .max(0x4000)
    .next_multiple_of(0x1000)
    .min(size / 2);
    let stack = VirtualAlloc(core::ptr::null_mut(), size, 0x2000, 4) as usize; // MEM_RESERVE
    if stack == 0 {
        return core::ptr::null_mut();
    }
    let top = stack + size;
    let limit = top - commit;
    let committed = !VirtualAlloc(limit as *mut c_void, commit, 0x1000, 4).is_null()
        && !VirtualAlloc((limit - 0x1000) as *mut c_void, 0x1000, 0x1000, 0x104).is_null(); // PAGE_GUARD
    if !committed {
        VirtualFree(stack as *mut c_void, 0, 0x8000);
        return core::ptr::null_mut();
    }
    let context =
        thread_context(thread_start as *const () as usize, [start as usize, param as usize], top);
    // INITIAL_TEB: OldStackBase, OldStackLimit, StackBase, StackLimit,
    // StackAllocationBase.
    let initial_teb = [0, 0, top, limit, stack];
    let (mut h, mut cid) = (core::ptr::null_mut(), [0usize; 2]);
    // SAFETY: valid pointers for the call.
    // SECURITY_ATTRIBUTES: the descriptor and inheritance, as
    // OBJECT_ATTRIBUTES without a name.
    let status = sync::with_name_sa(core::ptr::null(), attributes, |oa| unsafe {
        NtCreateThread(
            &mut h,
            0x1f_ffff, // THREAD_ALL_ACCESS
            oa,
            CURRENT_PROCESS,
            &mut cid,
            &context,
            &initial_teb,
            1, // suspended until the stack is the thread's (below)
        )
    })
    .unwrap_or(0xc000_000d); // STATUS_INVALID_PARAMETER
    if status != STATUS_SUCCESS {
        VirtualFree(stack as *mut c_void, 0, 0x8000);
        return fail(status, core::ptr::null_mut());
    }
    // The stack goes with the thread when it ends, as Windows frees a
    // CreateThread thread's stack (gala's own information class; before
    // the thread can run, so it cannot end first).
    // SAFETY: a handle just made; the class takes no data.
    unsafe { NtSetInformationThread(h, GALA_THREAD_FREE_STACK, core::ptr::null(), 0) };
    if flags & CREATE_SUSPENDED == 0 {
        let mut previous = 0;
        // SAFETY: a handle just made.
        unsafe { NtResumeThread(h, &mut previous) };
    }
    if !thread_id.is_null() {
        // SAFETY: the caller's output pointer.
        unsafe { *thread_id = cid[1] as u32 };
    }
    h
}

/// TerminateThread(thread, code): ends another thread (or this one) at
/// once with `code`, without its DLL notifications.
#[unsafe(no_mangle)]
pub extern "system" fn TerminateThread(thread: Handle, code: u32) -> Bool {
    // SAFETY: the kernel checks the handle.
    let status = unsafe { NtTerminateThread(thread as isize, code) };
    if status != STATUS_SUCCESS { fail(status, FALSE) } else { TRUE }
}

/// ExitThread: DLL_THREAD_DETACH, then NtTerminateThread; the kernel frees
/// the thread's stack and TEB as it ends (ARCHITECTURE "What an ended
/// thread leaves").
#[unsafe(no_mangle)]
pub extern "system" fn ExitThread(code: u32) -> ! {
    fls_thread_exit();
    // DLL_THREAD_DETACH, then the thread's TLS is freed.
    // SAFETY: a plain call.
    unsafe { LdrShutdownThread() };
    // SAFETY: ends the current thread (-2 = the current thread).
    unsafe { NtTerminateThread(-2, code) };
    loop {
        core::hint::spin_loop();
    }
}

/// GetExitCodeThread: STILL_ACTIVE (259) while it runs.
#[unsafe(no_mangle)]
pub extern "system" fn GetExitCodeThread(h: Handle, code: *mut u32) -> Bool {
    let basic = match thread_basic(h) {
        Ok(b) => b,
        Err(status) => return fail(status, FALSE),
    };
    // SAFETY: the caller's output pointer.
    unsafe { *code = basic.exit_status };
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

/// SetFileCompletionNotificationModes(FileHandle, Flags): FILE_SKIP_*
/// modes (FileIoCompletionNotificationInformation).
#[unsafe(no_mangle)]
pub extern "system" fn SetFileCompletionNotificationModes(file: Handle, flags: u8) -> Bool {
    let info = u32::from(flags);
    let mut iosb = [0usize; 2];
    // SAFETY: a 4-byte information buffer.
    let status = unsafe { NtSetInformationFile(file, &mut iosb, (&raw const info).cast(), 4, 41) };
    if status != STATUS_SUCCESS {
        return fail(status, FALSE);
    }
    TRUE
}

/// CancelIoEx(hFile, lpOverlapped): cancels the process's pending I/O on
/// the handle (the one using `overlapped`, or all); ERROR_NOT_FOUND if
/// there is none.
#[unsafe(no_mangle)]
pub extern "system" fn CancelIoEx(file: Handle, overlapped: *mut c_void) -> Bool {
    let mut iosb = [0usize; 2];
    // SAFETY: the OVERLAPPED starts with the request's IO_STATUS_BLOCK.
    let status = unsafe { NtCancelIoFileEx(file, overlapped, &mut iosb) };
    if status != STATUS_SUCCESS {
        return fail(status, FALSE);
    }
    TRUE
}

/// CancelIo(hFile): all of the handle's pending I/O.
// TODO: Windows cancels only the I/O the calling thread issued.
#[unsafe(no_mangle)]
pub extern "system" fn CancelIo(file: Handle) -> Bool {
    CancelIoEx(file, core::ptr::null_mut())
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
            // A packet of a failed I/O (an error or warning status) fails;
            // success codes (e.g. STATUS_NOTIFY_CLEANUP) do not.
            let status = e.internal as u32;
            if (status as i32) >= 0 { TRUE } else { fail(status, FALSE) }
        },
        Err(STATUS_TIMEOUT) => {
            // SAFETY: the caller's output pointer.
            unsafe { *overlapped = 0 };
            fail_with(258, FALSE)
        }
        // No packet (the port's last handle closed while waiting:
        // ERROR_ABANDONED_WAIT_0): *lpOverlapped is NULL, as documented.
        Err(status) => {
            // SAFETY: the caller's output pointer.
            unsafe { *overlapped = 0 };
            fail(status, FALSE)
        }
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

/// SetThreadPriority: ThreadBasePriority (the increment, -2..2 or the
/// saturated -15/15; ERROR_INVALID_PARAMETER otherwise).
#[unsafe(no_mangle)]
pub extern "system" fn SetThreadPriority(h: Handle, priority: i32) -> Bool {
    const THREAD_BASE_PRIORITY: u32 = 3;
    let v = priority.to_le_bytes();
    // SAFETY: a 4-byte buffer.
    let status = unsafe { NtSetInformationThread(h, THREAD_BASE_PRIORITY, v.as_ptr(), 4) };
    if status != STATUS_SUCCESS { fail(status, FALSE) } else { TRUE }
}

/// GetThreadPriority: THREAD_BASIC_INFORMATION.BasePriority, which is the
/// increment over the process's base (Windows 11 25H2), -15 and 15 the
/// saturated IDLE and TIME_CRITICAL.
#[unsafe(no_mangle)]
pub extern "system" fn GetThreadPriority(h: Handle) -> i32 {
    const THREAD_PRIORITY_ERROR_RETURN: i32 = 0x7fff_ffff;
    let basic = match thread_basic(h) {
        Ok(b) => b,
        Err(status) => return fail(status, THREAD_PRIORITY_ERROR_RETURN),
    };
    basic.base_priority
}

/// CreateWaitableTimerW / A: CreateWaitableTimerExW with
/// CREATE_WAITABLE_TIMER_MANUAL_RESET for a manual-reset timer.
#[unsafe(no_mangle)]
pub extern "system" fn CreateWaitableTimerW(
    attributes: *mut c_void,
    manual: Bool,
    name: *const u16,
) -> Handle {
    const TIMER_ALL_ACCESS: u32 = 0x1f_0003;
    CreateWaitableTimerExW(attributes, name, u32::from(manual != FALSE), TIMER_ALL_ACCESS)
}

#[unsafe(no_mangle)]
pub extern "system" fn CreateWaitableTimerA(
    attributes: *mut c_void,
    manual: Bool,
    name: *const u8,
) -> Handle {
    if !name.is_null() {
        // Named timers are not supported by the W version either.
        unsupported(b"kernel32.dll!CreateWaitableTimerA(name)", 0);
        return fail_with(ERROR_NOT_SUPPORTED, core::ptr::null_mut());
    }
    CreateWaitableTimerW(attributes, manual, core::ptr::null())
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

/// SetWaitableTimerEx: SetWaitableTimer; the wake context (a timer that
/// wakes the system from sleep) and the tolerable delay (coalescing) do
/// not apply here.
#[unsafe(no_mangle)]
pub extern "system" fn SetWaitableTimerEx(
    h: Handle,
    due: *const i64,
    period: i32,
    routine: usize,
    arg: usize,
    _wake_context: *mut c_void,
    _tolerable_delay: u32,
) -> Bool {
    SetWaitableTimer(h, due, period, routine, arg, FALSE)
}

/// CancelWaitableTimer: NtCancelTimer (the timer stays as it is, it
/// just does not fire).
#[unsafe(no_mangle)]
pub extern "system" fn CancelWaitableTimer(h: Handle) -> Bool {
    // SAFETY: a plain call.
    let status = unsafe { NtCancelTimer(h, core::ptr::null_mut()) };
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

// ---- Thread-local storage: TEB.TlsSlots (64 slots) ----

/// Allocated TLS indexes (bit set = in use). TODO: the expansion slots
/// (indexes 64..1088, TEB.TlsExpansionSlots).
static TLS_BITMAP: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
fn tls_slot(index: u32) -> usize {
    TEB_TLS_SLOTS + size_of::<usize>() * index as usize
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
    teb_read(tls_slot(index)) as *mut c_void
}

#[unsafe(no_mangle)]
pub extern "system" fn TlsSetValue(index: u32, value: *mut c_void) -> Bool {
    if index >= 64 {
        return fail_with(ERROR_INVALID_PARAMETER, FALSE);
    }
    teb_write(tls_slot(index), value as usize);
    TRUE
}

// ---- Fiber local storage ----
//
// gala has no fibers, so fiber local storage is thread local: FLS indexes
// are TLS indexes (from the same bitmap) with an optional callback, which
// runs for a non-null value when the thread exits and when the index is
// freed (TODO: for the other threads' values too, as Windows does).

/// The callback of each FLS index (0: none, or not an FLS index).
static FLS_CALLBACKS: [core::sync::atomic::AtomicUsize; 64] =
    [const { core::sync::atomic::AtomicUsize::new(0) }; 64];
/// FLS indexes in use (bit set).
static FLS_BITMAP: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

type FlsCallback = extern "system" fn(*mut c_void);

/// FlsAlloc(callback): FLS_OUT_OF_INDEXES (as TLS_OUT_OF_INDEXES) when
/// none is left.
#[unsafe(no_mangle)]
pub extern "system" fn FlsAlloc(callback: Option<FlsCallback>) -> u32 {
    use core::sync::atomic::Ordering;
    let index = TlsAlloc();
    if index == u32::MAX {
        return index;
    }
    FLS_CALLBACKS[index as usize].store(callback.map_or(0, |f| f as usize), Ordering::Release);
    FLS_BITMAP.fetch_or(1 << index, Ordering::AcqRel);
    index
}

/// Runs the callback of FLS index `index` for this thread's value, if
/// both are set, and clears the value.
fn fls_release(index: u32) {
    use core::sync::atomic::Ordering;
    let callback = FLS_CALLBACKS[index as usize].load(Ordering::Acquire);
    let value = teb_read(tls_slot(index)) as *mut c_void;
    if callback != 0 && !value.is_null() {
        TlsSetValue(index, core::ptr::null_mut());
        // SAFETY: registered as an FLS callback by FlsAlloc.
        let f: FlsCallback = unsafe { core::mem::transmute(callback) };
        f(value);
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn FlsFree(index: u32) -> Bool {
    use core::sync::atomic::Ordering;
    if index >= 64 || FLS_BITMAP.load(Ordering::Acquire) & 1 << index == 0 {
        return fail_with(ERROR_INVALID_PARAMETER, FALSE);
    }
    fls_release(index);
    FLS_BITMAP.fetch_and(!(1 << index), Ordering::AcqRel);
    FLS_CALLBACKS[index as usize].store(0, Ordering::Release);
    TlsFree(index)
}

#[unsafe(no_mangle)]
pub extern "system" fn FlsGetValue(index: u32) -> *mut c_void {
    TlsGetValue(index)
}

/// FlsGetValue2 and TlsGetValue2 (Windows 11 24H2): the slot's value
/// without changing the last error (and without checking the index, as
/// documented: the caller passes a valid one).
#[unsafe(no_mangle)]
pub extern "system" fn FlsGetValue2(index: u32) -> *mut c_void {
    TlsGetValue2(index)
}

#[unsafe(no_mangle)]
pub extern "system" fn TlsGetValue2(index: u32) -> *mut c_void {
    if index >= 64 {
        return core::ptr::null_mut();
    }
    teb_read(tls_slot(index)) as *mut c_void
}

#[unsafe(no_mangle)]
pub extern "system" fn FlsSetValue(index: u32, value: *mut c_void) -> Bool {
    TlsSetValue(index, value)
}

/// The FLS callbacks of the exiting thread.
fn fls_thread_exit() {
    use core::sync::atomic::Ordering;
    let mut bits = FLS_BITMAP.load(Ordering::Acquire);
    while bits != 0 {
        let index = bits.trailing_zeros();
        bits &= bits - 1;
        fls_release(index);
    }
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

/// __C_specific_handler: the language handler of SEH __try blocks
/// (kernel32 exports it on Windows; mingw's runtime imports it from
/// here). gala does not run __except filters yet: the exception keeps
/// searching (recorded), as msvcrt's. x64 only: x86 SEH has no language
/// handler.
#[cfg(target_arch = "x86_64")]
#[unsafe(no_mangle)]
pub extern "C" fn __C_specific_handler(
    _record: *mut c_void,
    _frame: usize,
    _context: *mut c_void,
    _dispatcher: *mut c_void,
) -> i32 {
    unsupported(b"kernel32.dll!__C_specific_handler", 0);
    1 // ExceptionContinueSearch
}

/// DllMain (the entry point our link flags name).
#[unsafe(no_mangle)]
pub extern "system" fn mainCRTStartup(_instance: usize, reason: u32, _reserved: usize) -> i32 {
    if reason == 1 {
        // DLL_PROCESS_ATTACH: unhandled exceptions go to
        // UnhandledExceptionFilter (and so to SetUnhandledExceptionFilter's
        // filter), as on Windows.
        // SAFETY: registers a function with the documented signature.
        unsafe {
            RtlSetUnhandledExceptionFilter(sysmem::UnhandledExceptionFilter as *const () as usize)
        };
        // A process in a console window gets its control events (Ctrl+C)
        // on a thread of its own, as on Windows.
        console::start_ctrl_thread();
    }
    1
}

#[cfg_attr(
    target_arch = "x86",
    link(name = "ntdll", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "ntdll", kind = "raw-dylib"))]
unsafe extern "system" {
    fn RtlSetUnhandledExceptionFilter(filter: usize);
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    ExitProcess(0xc000_0409) // STATUS_STACK_BUFFER_OVERRUN, as __fastfail would
}
