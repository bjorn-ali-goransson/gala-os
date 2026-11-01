//! NetWkstaGetInfo (lmwksta.h) and NetApiBufferFree (lmapibuf.h).

#![allow(non_snake_case)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetProcessHeap() -> usize;
    fn HeapAlloc(heap: usize, flags: u32, size: usize) -> *mut u8;
    fn HeapFree(heap: usize, flags: u32, block: *mut u8) -> i32;
    fn GetComputerNameW(buf: *mut u16, size: *mut u32) -> i32;
}

const NERR_SUCCESS: u32 = 0;
const ERROR_NOT_ENOUGH_MEMORY: u32 = 8;
const ERROR_INVALID_PARAMETER: u32 = 87;
const ERROR_INVALID_LEVEL: u32 = 124;
const PLATFORM_ID_NT: u32 = 500;

/// WKSTA_INFO_102; levels 100 and 101 are its first fields.
#[repr(C)]
struct WkstaInfo102 {
    platform_id: u32,
    computername: *mut u16,
    langroup: *mut u16,
    ver_major: u32,
    ver_minor: u32,
    lanroot: *mut u16,
    logged_on_users: u32,
}

/// NetWkstaGetInfo: this machine's levels 100, 101 and 102 (a buffer to
/// free with NetApiBufferFree). gala is not in a domain: the LAN group is
/// the workgroup WORKGROUP; one user is logged on. Only the local machine
/// (a null server name) is supported.
#[unsafe(no_mangle)]
pub extern "system" fn NetWkstaGetInfo(server: *const u16, level: u32, out: *mut *mut u8) -> u32 {
    if out.is_null() {
        return ERROR_INVALID_PARAMETER;
    }
    if !matches!(level, 100..=102) {
        return ERROR_INVALID_LEVEL;
    }
    // SAFETY: the caller's server name, if any.
    if !server.is_null() && unsafe { *server } != 0 {
        // Another machine: gala has no remote administration.
        return 53; // ERROR_BAD_NETPATH
    }
    let mut name = [0u16; 64];
    let mut size = name.len() as u32;
    // SAFETY: our buffer and its size.
    if unsafe { GetComputerNameW(name.as_mut_ptr(), &mut size) } == 0 {
        size = 0;
    }
    let strings: [&[u16]; 3] = [
        &name[..size as usize],
        &[87, 79, 82, 75, 71, 82, 79, 85, 80], // WORKGROUP
        &[67, 58, 92, 87, 105, 110, 100, 111, 119, 115], // C:\Windows
    ];
    let header = size_of::<WkstaInfo102>();
    let total = header + strings.iter().map(|s| 2 * (s.len() + 1)).sum::<usize>();
    // SAFETY: plain heap allocation.
    let block = unsafe { HeapAlloc(GetProcessHeap(), 0, total) };
    if block.is_null() {
        return ERROR_NOT_ENOUGH_MEMORY;
    }
    let mut at = header;
    let mut pointers = [core::ptr::null_mut::<u16>(); 3];
    for (slot, text) in pointers.iter_mut().zip(strings) {
        // SAFETY: inside the block just allocated (sized for every string
        // and its NUL); u16-aligned, as the header's size is even.
        unsafe {
            let p = block.add(at).cast::<u16>();
            core::ptr::copy_nonoverlapping(text.as_ptr(), p, text.len());
            p.add(text.len()).write(0);
            *slot = p;
        }
        at += 2 * (text.len() + 1);
    }
    let info = WkstaInfo102 {
        platform_id: PLATFORM_ID_NT,
        computername: pointers[0],
        langroup: pointers[1],
        ver_major: 10,
        ver_minor: 0,
        lanroot: pointers[2],
        logged_on_users: 1,
    };
    // SAFETY: the block starts with room for the header; the caller's
    // output pointer.
    unsafe {
        block.cast::<WkstaInfo102>().write(info);
        out.write(block);
    }
    NERR_SUCCESS
}

/// NetApiBufferFree: frees a buffer a Net function returned.
#[unsafe(no_mangle)]
pub extern "system" fn NetApiBufferFree(buffer: *mut u8) -> u32 {
    if !buffer.is_null() {
        // SAFETY: a block from the process heap (a Net function's).
        unsafe { HeapFree(GetProcessHeap(), 0, buffer) };
    }
    NERR_SUCCESS
}
