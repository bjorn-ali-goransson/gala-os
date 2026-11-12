//! gala's wtsapi32.dll (Remote Desktop Services, wtsapi32.h): the
//! sessions (WTSEnumerateSessions, over the kernel's GALA_SESSIONS
//! class: "Services" for session 0, "Console" for the one the console
//! shows), the caller's session (its id, active, its user and machine);
//! session notifications are accepted and never sent (gala's session
//! never connects, disconnects or locks yet).

#![no_std]
#![allow(non_snake_case)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

// The 32-bit build (SysWOW64) takes the memory routines, 64-bit
// division helpers and unwind personality from ntrt's x86 runtime
// (ARCHITECTURE "Plan: WOW64").
#[cfg(target_arch = "x86")]
use ntrt as _;

/// MSVC-target codegen references this marker symbol directly, so it cannot
/// come from an import.
#[cfg(target_arch = "x86_64")]
#[unsafe(no_mangle)]
pub static _fltused: i32 = 0;

// The C runtime helpers the compiler may call, exported by ntdll.
#[cfg(target_arch = "x86_64")]
#[allow(dead_code)]
#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "C" {
    fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memmove(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memset(dst: *mut u8, value: i32, n: usize) -> *mut u8;
    fn __chkstk();
}

#[cfg_attr(
    target_arch = "x86",
    link(name = "ntdll", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "ntdll", kind = "raw-dylib"))]
unsafe extern "system" {
    fn GalaRecordMissing(name: *const u8, length: usize, what: u32, detail: u64) -> u32;
    fn NtQuerySystemInformation(class: u32, buf: *mut u8, len: u32, ret: *mut u32) -> u32;
    fn RtlNtStatusToDosError(status: u32) -> u32;
    fn GalaSession(op: u64, session: u32, arg: u64, buffer: u64, length: u64) -> u32;
}

#[cfg_attr(
    target_arch = "x86",
    link(name = "kernel32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "kernel32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn LocalAlloc(flags: u32, bytes: usize) -> *mut u8;
    fn LocalFree(p: *mut u8) -> *mut u8;
    fn SetLastError(code: u32);
    fn GetComputerNameW(buf: *mut u16, size: *mut u32) -> i32;
    fn ProcessIdToSessionId(pid: u32, session: *mut u32) -> i32;
    fn GetCurrentProcessId() -> u32;
}

#[cfg_attr(
    target_arch = "x86",
    link(name = "advapi32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "advapi32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn GetUserNameW(buf: *mut u16, size: *mut u32) -> i32;
}

fn fail(code: u32) -> i32 {
    // SAFETY: plain call.
    unsafe { SetLastError(code) };
    0
}

/// WTSRegisterSessionNotification(hWnd, dwFlags): accepted.
#[unsafe(no_mangle)]
pub extern "system" fn WTSRegisterSessionNotification(hwnd: usize, _flags: u32) -> i32 {
    if hwnd == 0 {
        return fail(87);
    }
    1
}

#[unsafe(no_mangle)]
pub extern "system" fn WTSUnRegisterSessionNotification(hwnd: usize) -> i32 {
    if hwnd == 0 {
        return fail(87);
    }
    1
}

/// WTSFreeMemory(pMemory): frees WTSQuerySessionInformationW's buffer.
#[unsafe(no_mangle)]
pub extern "system" fn WTSFreeMemory(memory: *mut u8) {
    if !memory.is_null() {
        // SAFETY: a block this DLL LocalAlloc'd.
        unsafe { LocalFree(memory) };
    }
}

/// Hands `bytes` to the caller in a new buffer.
fn give(bytes: &[u8], out: *mut *mut u8, size: *mut u32) -> i32 {
    // SAFETY: a plain allocation, then the caller's outputs.
    unsafe {
        let p = LocalAlloc(0x40, bytes.len());
        if p.is_null() {
            return fail(8);
        }
        core::ptr::copy_nonoverlapping(bytes.as_ptr(), p, bytes.len());
        *out = p;
        *size = bytes.len() as u32;
    }
    1
}

/// UTF-16 `w` (NUL included) as bytes in `buf`; its length in bytes.
fn wide_bytes(w: &[u16], buf: &mut [u8; 512]) -> usize {
    let mut n = 0;
    for &c in w {
        if n + 2 > buf.len() {
            break;
        }
        buf[n..n + 2].copy_from_slice(&c.to_le_bytes());
        n += 2;
    }
    n
}

/// WTSQuerySessionInformationW(hServer, SessionId, WTSInfoClass,
/// *ppBuffer, *pBytesReturned): for the current session (or
/// WTS_CURRENT_SESSION) on this server: its id, user, station
/// ("Console"), domain (the machine), connect state (WTSActive) and
/// protocol (the console). Other classes are reported (ERROR_NOT_SUPPORTED).
#[unsafe(no_mangle)]
pub extern "system" fn WTSQuerySessionInformationW(
    _server: usize,
    session: u32,
    class: u32,
    out: *mut *mut u8,
    size: *mut u32,
) -> i32 {
    if out.is_null() || size.is_null() {
        return fail(87);
    }
    let mut mine = 0u32;
    // SAFETY: plain calls.
    unsafe { ProcessIdToSessionId(GetCurrentProcessId(), &mut mine) };
    if session != 0xffff_ffff && session != mine {
        return fail(7022); // ERROR_CTX_WINSTATION_NOT_FOUND
    }
    let mut text = [0u16; 256];
    let mut buf = [0u8; 512];
    match class {
        4 => give(&mine.to_le_bytes(), out, size),  // WTSSessionId
        8 => give(&0u32.to_le_bytes(), out, size),  // WTSConnectState: WTSActive
        16 => give(&0u16.to_le_bytes(), out, size), // WTSClientProtocolType: console
        5 | 7 => {
            let mut n = text.len() as u32;
            // SAFETY: our buffer and its size.
            let ok = unsafe {
                if class == 5 {
                    GetUserNameW(text.as_mut_ptr(), &mut n)
                } else {
                    GetComputerNameW(text.as_mut_ptr(), &mut n)
                }
            };
            if ok == 0 {
                return 0;
            }
            let len = text.iter().position(|&c| c == 0).unwrap_or(0);
            let b = wide_bytes(&text[..=len], &mut buf);
            give(&buf[..b], out, size)
        }
        6 => {
            let station: [u16; 8] = [67, 111, 110, 115, 111, 108, 101, 0]; // Console
            let b = wide_bytes(&station, &mut buf);
            give(&buf[..b], out, size)
        }
        _ => {
            let name = b"wtsapi32.dll!WTSQuerySessionInformationW(class)";
            // SAFETY: a valid buffer.
            unsafe { GalaRecordMissing(name.as_ptr(), name.len(), 2, class.into()) };
            fail(50)
        }
    }
}

/// WTS_SESSION_INFOW / WTS_SESSION_INFOA: SessionId, pWinStationName,
/// State.
#[repr(C)]
struct SessionInfo {
    id: u32,
    name: usize,
    state: u32,
}

const WTS_ACTIVE: u32 = 0;
const WTS_DISCONNECTED: u32 = 4;
const GALA_SESSIONS: u32 = 0x6761_0002;
const MAX_SESSIONS: usize = 64;

/// The sessions (id, shown by the console) into `out`; their count, or
/// the Win32 error.
fn sessions(out: &mut [(u32, bool); MAX_SESSIONS]) -> Result<usize, u32> {
    let mut records = [0u8; MAX_SESSIONS * 8];
    let mut n = 0u32;
    // SAFETY: our buffer and its size.
    let status = unsafe {
        NtQuerySystemInformation(GALA_SESSIONS, records.as_mut_ptr(), records.len() as u32, &mut n)
    };
    if status != 0 {
        // SAFETY: a plain conversion.
        return Err(unsafe { RtlNtStatusToDosError(status) });
    }
    let count = (n as usize / 8).min(MAX_SESSIONS);
    for (i, r) in records.chunks_exact(8).take(count).enumerate() {
        let id = u32::from_le_bytes([r[0], r[1], r[2], r[3]]);
        out[i] = (id, r[4] & 1 != 0);
    }
    Ok(count)
}

/// A session's station name and state: session 0 "Services",
/// disconnected (it has no console, as on Windows); the one the console
/// shows "Console", active; other sessions (gala's remote logons, ROADMAP
/// RS B4) an empty name, active.
fn station(id: u32, console: bool) -> (&'static [u8], u32) {
    match (id, console) {
        (_, true) => (b"Console", WTS_ACTIVE),
        (0, false) => (b"Services", WTS_DISCONNECTED),
        _ => (b"", WTS_ACTIVE),
    }
}

/// WTSEnumerateSessionsW/A's work: one LocalAlloc'd block (WTSFreeMemory
/// frees it) of WTS_SESSION_INFO records followed by their names
/// (`wide`: UTF-16, else ANSI).
fn enumerate(reserved: u32, version: u32, out: *mut *mut u8, count: *mut u32, wide: bool) -> i32 {
    if reserved != 0 || version != 1 || out.is_null() || count.is_null() {
        return fail(87); // ERROR_INVALID_PARAMETER
    }
    let mut list = [(0u32, false); MAX_SESSIONS];
    let n = match sessions(&mut list) {
        Ok(n) => n,
        Err(e) => return fail(e),
    };
    let unit = if wide { 2 } else { 1 };
    let names: usize = list[..n].iter().map(|&(id, c)| (station(id, c).0.len() + 1) * unit).sum();
    let head = n * size_of::<SessionInfo>();
    // SAFETY: a plain allocation (zeroed: LMEM_ZEROINIT).
    let p = unsafe { LocalAlloc(0x40, head + names) };
    if p.is_null() {
        return fail(8);
    }
    let mut at = head;
    for (i, &(id, console)) in list[..n].iter().enumerate() {
        let (name, state) = station(id, console);
        // SAFETY: inside the block: the records, then the names (ASCII,
        // the same in both forms), each NUL-terminated by the zeroing.
        unsafe {
            for (k, &b) in name.iter().enumerate() {
                *p.add(at + k * unit) = b;
            }
            p.cast::<SessionInfo>().add(i).write(SessionInfo {
                id,
                name: p.add(at) as usize,
                state,
            });
        }
        at += (name.len() + 1) * unit;
    }
    // SAFETY: the caller's outputs.
    unsafe {
        *out = p;
        *count = n as u32;
    }
    1
}

/// WTSEnumerateSessionsW(hServer, Reserved, Version, *ppSessionInfo,
/// *pCount): the sessions on this machine (any server handle names it:
/// gala has no remote servers). Reserved 0 and Version 1, else
/// ERROR_INVALID_PARAMETER (as documented).
#[unsafe(no_mangle)]
pub extern "system" fn WTSEnumerateSessionsW(
    _server: usize,
    reserved: u32,
    version: u32,
    out: *mut *mut u8,
    count: *mut u32,
) -> i32 {
    enumerate(reserved, version, out, count, true)
}

/// WTSEnumerateSessionsA: the same with ANSI names.
#[unsafe(no_mangle)]
pub extern "system" fn WTSEnumerateSessionsA(
    _server: usize,
    reserved: u32,
    version: u32,
    out: *mut *mut u8,
    count: *mut u32,
) -> i32 {
    enumerate(reserved, version, out, count, false)
}

/// WTSQueryUserToken(SessionId, *phToken): a primary token of the user
/// logged on to the session (gala's GalaSession SESSION_USER_TOKEN): only
/// for a caller with SeTcbPrivilege (LocalSystem), else
/// ERROR_PRIVILEGE_NOT_HELD; ERROR_NO_TOKEN for a session without a
/// logged-on user (session 0), ERROR_FILE_NOT_FOUND for one that does not
/// exist, as documented.
#[unsafe(no_mangle)]
pub extern "system" fn WTSQueryUserToken(session: u32, token: *mut usize) -> i32 {
    const SESSION_USER_TOKEN: u64 = 4;
    const STATUS_NOT_FOUND: u32 = 0xc000_0225;
    if token.is_null() {
        return fail(87);
    }
    // The kernel writes the handle as a ULONG64 (in a 32-bit process too).
    let mut h = 0u64;
    // SAFETY: our output for the handle.
    let status =
        unsafe { GalaSession(SESSION_USER_TOKEN, session, (&raw mut h) as usize as u64, 0, 0) };
    match status {
        0 => {
            // SAFETY: the caller's output.
            unsafe { *token = h as usize };
            1
        }
        STATUS_NOT_FOUND => fail(2),
        // SAFETY: a plain conversion.
        s => fail(unsafe { RtlNtStatusToDosError(s) }),
    }
}

/// The DLL entry point: nothing to set up.
#[unsafe(no_mangle)]
pub extern "system" fn mainCRTStartup(_instance: usize, _reason: u32, _reserved: usize) -> i32 {
    1
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
