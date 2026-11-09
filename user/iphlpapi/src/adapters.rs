//! GetAdaptersInfo (iphlpapi.h, IP_ADAPTER_INFO) over the kernel's
//! interfaces (`tables::interfaces`), as Microsoft documents it and
//! Wine's conformance test (dlls/iphlpapi/tests/iphlpapi.c,
//! testGetAdaptersInfo, run on Windows) checks it: no SizePointer is
//! ERROR_INVALID_PARAMETER; no adapter but loopback ERROR_NO_DATA; a
//! buffer too small (or none) ERROR_BUFFER_OVERFLOW with the bytes
//! needed; otherwise one IP_ADAPTER_INFO per adapter, linked by Next,
//! loopback left out. Each: AdapterName the interface's GUID as
//! `{XXXXXXXX-XXXX-XXXX-XXXX-XXXXXXXXXXXX}`, Description and the MAC as
//! GetIfEntry2 gives them, Index and ComboIndex the interface index,
//! Type MIB_IF_TYPE_ETHERNET, DhcpEnabled FALSE (gala's address is
//! static), CurrentIpAddress NULL (reserved), the address and its mask,
//! the default gateway with mask 255.255.255.255 (0.0.0.0 when there is
//! none), the DHCP and WINS servers empty, no lease.
//!
//! The structures have pointers, so their layout is the caller's width:
//! `repr(C)` with `usize` pointers, held to the SDK header's offsets for
//! both (`check` below; x86 with the 64-bit time_t of MSVC's default,
//! 648 bytes: mingw's i686 headers have a 32-bit time_t there, 640).

use crate::tables::{IF_TYPE_SOFTWARE_LOOPBACK, Interface, interfaces};

const NO_ERROR: u32 = 0;
const ERROR_INVALID_PARAMETER: u32 = 87;
const ERROR_BUFFER_OVERFLOW: u32 = 111;
const ERROR_NO_DATA: u32 = 232;

/// MIB_IF_TYPE_ETHERNET.
const TYPE_ETHERNET: u32 = 6;

/// IP_ADDR_STRING.
#[repr(C)]
#[derive(Clone, Copy)]
struct IpAddrString {
    next: usize,
    address: [u8; 16],
    mask: [u8; 16],
    context: u32,
}

/// IP_ADAPTER_INFO.
#[repr(C)]
struct IpAdapterInfo {
    next: usize,
    combo_index: u32,
    adapter_name: [u8; 260],
    description: [u8; 132],
    address_length: u32,
    address: [u8; 8],
    index: u32,
    if_type: u32,
    dhcp_enabled: u32,
    current_ip_address: usize,
    ip_address_list: IpAddrString,
    gateway_list: IpAddrString,
    dhcp_server: IpAddrString,
    have_wins: i32,
    primary_wins_server: IpAddrString,
    secondary_wins_server: IpAddrString,
    lease_obtained: i64,
    lease_expires: i64,
}

/// The SDK's sizes and offsets (clang on iphlpapi.h for both targets).
const _: () = {
    use core::mem::{offset_of, size_of};
    let x86 = cfg!(target_arch = "x86");
    assert!(size_of::<IpAddrString>() == if x86 { 40 } else { 48 });
    assert!(offset_of!(IpAdapterInfo, adapter_name) == if x86 { 8 } else { 12 });
    assert!(offset_of!(IpAdapterInfo, description) == if x86 { 268 } else { 272 });
    assert!(offset_of!(IpAdapterInfo, address_length) == if x86 { 400 } else { 404 });
    assert!(offset_of!(IpAdapterInfo, index) == if x86 { 412 } else { 416 });
    assert!(offset_of!(IpAdapterInfo, current_ip_address) == if x86 { 424 } else { 432 });
    assert!(offset_of!(IpAdapterInfo, ip_address_list) == if x86 { 428 } else { 440 });
    assert!(offset_of!(IpAdapterInfo, gateway_list) == if x86 { 468 } else { 488 });
    assert!(offset_of!(IpAdapterInfo, have_wins) == if x86 { 548 } else { 584 });
    assert!(offset_of!(IpAdapterInfo, lease_obtained) == if x86 { 632 } else { 688 });
    assert!(size_of::<IpAdapterInfo>() == if x86 { 648 } else { 704 });
};

/// `a.b.c.d` with its NUL into a 16-byte IP_ADDRESS_STRING.
fn dotted(ip: [u8; 4]) -> [u8; 16] {
    let mut out = [0u8; 16];
    let mut n = 0;
    for (k, &b) in ip.iter().enumerate() {
        if k > 0 {
            out[n] = b'.';
            n += 1;
        }
        let digits = [b / 100, b / 10 % 10, b % 10];
        let first = if b >= 100 {
            0
        } else if b >= 10 {
            1
        } else {
            2
        };
        for &d in &digits[first..] {
            out[n] = b'0' + d;
            n += 1;
        }
    }
    out
}

fn addr_string(ip: [u8; 4], mask: [u8; 4]) -> IpAddrString {
    IpAddrString { next: 0, address: dotted(ip), mask: dotted(mask), context: 0 }
}

/// The GUID (memory order) as `{XXXXXXXX-XXXX-XXXX-XXXX-XXXXXXXXXXXX}`.
fn guid_text(g: [u8; 16]) -> [u8; 39] {
    let mut out = [0u8; 39];
    let mut n = 0;
    let mut put = |b: u8| {
        out[n] = b;
        n += 1;
    };
    let hex = |v: u8| b"0123456789ABCDEF"[usize::from(v & 15)];
    put(b'{');
    // Data1, Data2, Data3 are little-endian numbers; Data4 bytes in order.
    let order = [3, 2, 1, 0, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15];
    for (k, &i) in order.iter().enumerate() {
        if matches!(k, 4 | 6 | 8 | 10) {
            put(b'-');
        }
        put(hex(g[i] >> 4));
        put(hex(g[i]));
    }
    put(b'}');
    out
}

fn adapter(i: &Interface) -> IpAdapterInfo {
    let mut name = [0u8; 260];
    name[..38].copy_from_slice(&guid_text(i.guid())[..38]);
    let mut description = [0u8; 132];
    let d = i.description().as_bytes();
    description[..d.len()].copy_from_slice(d);
    let mask = if i.prefix == 0 { 0 } else { u32::MAX << (32 - i.prefix.min(32)) };
    let none = IpAddrString { next: 0, address: [0; 16], mask: [0; 16], context: 0 };
    IpAdapterInfo {
        next: 0,
        combo_index: i.index,
        adapter_name: name,
        description,
        address_length: i.mac_len.min(8),
        address: i.mac,
        index: i.index,
        if_type: TYPE_ETHERNET,
        dhcp_enabled: 0,
        current_ip_address: 0,
        ip_address_list: addr_string(i.ipv4, mask.to_be_bytes()),
        gateway_list: addr_string(i.gateway, [255; 4]),
        dhcp_server: none,
        have_wins: 0,
        primary_wins_server: none,
        secondary_wins_server: none,
        lease_obtained: 0,
        lease_expires: 0,
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn GetAdaptersInfo(info: *mut u8, size: *mut u32) -> u32 {
    if size.is_null() {
        return ERROR_INVALID_PARAMETER;
    }
    let (list, n) = interfaces();
    let mut adapters =
        list[..n].iter().flatten().filter(|i| i.if_type != IF_TYPE_SOFTWARE_LOOPBACK);
    let count = adapters.clone().count();
    if count == 0 {
        return ERROR_NO_DATA;
    }
    let needed = (count * size_of::<IpAdapterInfo>()) as u32;
    // SAFETY: the caller's ULONG.
    unsafe {
        if info.is_null() || *size < needed {
            *size = needed;
            return ERROR_BUFFER_OVERFLOW;
        }
    }
    let out = info.cast::<IpAdapterInfo>();
    for k in 0..count {
        let Some(i) = adapters.next() else { break };
        let mut a = adapter(i);
        if k + 1 < count {
            // SAFETY: inside the caller's buffer (`needed` bytes).
            a.next = unsafe { out.add(k + 1) } as usize;
        }
        // SAFETY: the caller's buffer of at least `needed` bytes (any
        // alignment).
        unsafe { out.add(k).write_unaligned(a) };
    }
    NO_ERROR
}
