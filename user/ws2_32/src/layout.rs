//! ws2_32's caller-facing structures with pointers, `repr(C)` with
//! pointer-sized fields so that one definition has the x64 and the x86
//! layout (winsock2.h, ws2def.h; ARCHITECTURE "Plan: WOW64"). AFD's
//! buffers to the kernel keep the kernel's 64-bit layout on both.

use core::ffi::c_void;

/// WSADATA: its fields come in another order on x64 (408 bytes) than on
/// x86 (400 bytes).
#[cfg(target_arch = "x86_64")]
#[repr(C)]
pub struct WsaData {
    pub version: u16,
    pub high_version: u16,
    pub max_sockets: u16,
    pub max_udp_dg: u16,
    pub vendor_info: *mut u8,
    pub description: [u8; 257],
    pub system_status: [u8; 129],
}

#[cfg(target_arch = "x86")]
#[repr(C)]
pub struct WsaData {
    pub version: u16,
    pub high_version: u16,
    pub description: [u8; 257],
    pub system_status: [u8; 129],
    pub max_sockets: u16,
    pub max_udp_dg: u16,
    pub vendor_info: *mut u8,
}

const _: () =
    assert!(core::mem::size_of::<WsaData>() == if cfg!(target_arch = "x86") { 400 } else { 408 });

/// ADDRINFOA / ADDRINFOW (the same layout: the name's pointer differs).
#[repr(C)]
pub struct AddrInfo {
    pub flags: i32,
    pub family: i32,
    pub socktype: i32,
    pub protocol: i32,
    pub addrlen: usize,
    pub canonname: *mut c_void,
    pub addr: *mut u8,
    pub next: *mut AddrInfo,
}

/// ADDRINFOEXW.
#[repr(C)]
pub struct AddrInfoExW {
    pub flags: i32,
    pub family: i32,
    pub socktype: i32,
    pub protocol: i32,
    pub addrlen: usize,
    pub canonname: *mut u16,
    pub addr: *mut u8,
    pub blob: *mut c_void,
    pub bloblen: usize,
    pub provider: *mut c_void,
    pub next: *mut AddrInfoExW,
}
