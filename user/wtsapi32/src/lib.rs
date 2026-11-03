//! gala's wtsapi32.dll (Remote Desktop Services, wtsapi32.h): the one
//! session programs run in, console session 1, active, its user and
//! machine; session notifications are accepted and never sent (gala's
//! session never connects, disconnects or locks yet).

#![no_std]
#![allow(non_snake_case)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

/// MSVC-target codegen references this marker symbol directly, so it cannot
/// come from an import.
#[unsafe(no_mangle)]
pub static _fltused: i32 = 0;

// The C runtime helpers the compiler may call, exported by ntdll.
#[allow(dead_code)]
#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "C" {
    fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memmove(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memset(dst: *mut u8, value: i32, n: usize) -> *mut u8;
    fn __chkstk();
}

#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "system" {
    fn GalaRecordMissing(name: *const u8, length: usize, what: u32, detail: u64) -> u32;
}

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn LocalAlloc(flags: u32, bytes: usize) -> *mut u8;
    fn LocalFree(p: *mut u8) -> *mut u8;
    fn SetLastError(code: u32);
    fn GetComputerNameW(buf: *mut u16, size: *mut u32) -> i32;
    fn ProcessIdToSessionId(pid: u32, session: *mut u32) -> i32;
    fn GetCurrentProcessId() -> u32;
}

#[link(name = "advapi32", kind = "raw-dylib")]
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
