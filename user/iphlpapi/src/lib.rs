//! gala's iphlpapi.dll: what the JDK's net.dll imports. gala does not
//! enumerate its network interfaces through this API yet (TODO: the
//! virtio card and loopback): the table and adapter functions return
//! ERROR_NOT_SUPPORTED and are recorded in the ledger, as are ICMP echo
//! and address-change notification. ConvertLengthToIpv4Mask is computed.

#![no_std]
#![allow(non_snake_case)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

#[allow(dead_code)]
#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "C" {
    fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memmove(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memset(dst: *mut u8, value: i32, n: usize) -> *mut u8;
    fn memcmp(a: *const u8, b: *const u8, n: usize) -> i32;
    fn strlen(s: *const u8) -> usize;
    // MSVC-target unwind tables reference it (gala's ntdll provides it).
    fn __CxxFrameHandler3();
    // The stack probe for frames over a page (ntdll exports it).
    fn __chkstk();
}

/// MSVC-target codegen references this marker symbol directly, so it cannot
/// come from an import. (It becomes a harmless extra data export.)
#[unsafe(no_mangle)]
pub static _fltused: i32 = 0;

#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "system" {
    fn GalaRecordMissing(name: *const u8, length: usize, what: u32, detail: u64) -> u32;
}

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn SetLastError(e: u32);
}

const ERROR_NOT_SUPPORTED: u32 = 50;
const ERROR_INVALID_PARAMETER: u32 = 87;

fn missing(name: &[u8]) -> u32 {
    // SAFETY: a valid buffer.
    unsafe { GalaRecordMissing(name.as_ptr(), name.len(), 2, 0) };
    ERROR_NOT_SUPPORTED
}

/// Functions that return ERROR_NOT_SUPPORTED (a Win32 error code as
/// their result), recorded.
macro_rules! not_supported {
    ($($name:ident($($arg:ident: $ty:ty),*);)*) => {$(
        #[unsafe(no_mangle)]
        pub extern "system" fn $name($($arg: $ty),*) -> u32 {
            $(let _ = $arg;)*
            missing(concat!("iphlpapi.dll!", stringify!($name)).as_bytes())
        }
    )*};
}

not_supported! {
    GetAdaptersAddresses(family: u32, flags: u32, reserved: usize, addresses: usize, size: usize);
    GetIfTable2(table: usize);
    GetIfEntry2(row: usize);
    GetUnicastIpAddressTable(family: u16, table: usize);
    GetAnycastIpAddressTable(family: u16, table: usize);
    ConvertInterfaceLuidToNameW(luid: usize, name: usize, length: usize);
    ConvertInterfaceNameToLuidW(name: usize, luid: usize);
    NotifyAddrChange(handle: usize, overlapped: usize);
}

/// FreeMibTable: nothing was allocated.
#[unsafe(no_mangle)]
pub extern "system" fn FreeMibTable(_table: usize) {}

/// ConvertLengthToIpv4Mask(MaskLength, *Mask): the mask in network byte
/// order; ERROR_INVALID_PARAMETER (and INADDR_NONE) past 32.
#[unsafe(no_mangle)]
pub extern "system" fn ConvertLengthToIpv4Mask(length: u32, mask: *mut u32) -> u32 {
    if mask.is_null() {
        return ERROR_INVALID_PARAMETER;
    }
    let (value, status) = match length {
        0 => (0u32, 0),
        1..=32 => ((u32::MAX << (32 - length)).to_be(), 0),
        _ => (u32::MAX, ERROR_INVALID_PARAMETER),
    };
    // SAFETY: the caller's output.
    unsafe { *mask = value };
    status
}

/// IcmpCreateFile/Icmp6CreateFile: no ICMP echo (INVALID_HANDLE_VALUE).
#[unsafe(no_mangle)]
pub extern "system" fn IcmpCreateFile() -> isize {
    // SAFETY: a plain call.
    unsafe { SetLastError(missing(b"iphlpapi.dll!IcmpCreateFile")) };
    -1
}

#[unsafe(no_mangle)]
pub extern "system" fn Icmp6CreateFile() -> isize {
    // SAFETY: a plain call.
    unsafe { SetLastError(missing(b"iphlpapi.dll!Icmp6CreateFile")) };
    -1
}

#[unsafe(no_mangle)]
pub extern "system" fn IcmpCloseHandle(_h: isize) -> i32 {
    1
}

/// The echo functions: no replies (0), ERROR_NOT_SUPPORTED.
macro_rules! no_replies {
    ($($name:ident($($arg:ident: $ty:ty),*);)*) => {$(
        #[unsafe(no_mangle)]
        pub extern "system" fn $name($($arg: $ty),*) -> u32 {
            $(let _ = $arg;)*
            // SAFETY: a plain call.
            unsafe { SetLastError(missing(concat!("iphlpapi.dll!", stringify!($name)).as_bytes())) };
            0
        }
    )*};
}

no_replies! {
    IcmpSendEcho(h: isize, to: u32, data: usize, size: u16, options: usize, reply: usize, reply_size: u32, timeout: u32);
    IcmpSendEcho2Ex(h: isize, event: usize, routine: usize, context: usize, from: u32, to: u32, data: usize, size: u16, options: usize, reply: usize, reply_size: u32, timeout: u32);
    Icmp6SendEcho2(h: isize, event: usize, routine: usize, context: usize, from: usize, to: usize, data: usize, size: u16, options: usize, reply: usize, reply_size: u32, timeout: u32);
}

/// DllMain (the entry point our link flags name).
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
