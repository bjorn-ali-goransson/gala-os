//! The interface and unicast address tables (netioapi.h layouts, x64)
//! over the kernel's GALA_NETWORK_INTERFACES records.

use core::ffi::c_void;

#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "system" {
    fn NtQuerySystemInformation(class: u32, buf: *mut u8, len: u32, ret: *mut u32) -> u32;
}

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetProcessHeap() -> *mut c_void;
    fn HeapAlloc(heap: *mut c_void, flags: u32, bytes: usize) -> *mut u8;
    fn HeapFree(heap: *mut c_void, flags: u32, p: *mut u8) -> i32;
}

const NO_ERROR: u32 = 0;
const ERROR_FILE_NOT_FOUND: u32 = 2;
const ERROR_NOT_ENOUGH_MEMORY: u32 = 8;
const ERROR_INVALID_PARAMETER: u32 = 87;

/// The kernel's class and record (kernel/src/nt/sysinfo.rs).
const GALA_NETWORK_INTERFACES: u32 = 0x6761_0001;
const RECORD: usize = 36;

const IF_TYPE_ETHERNET_CSMACD: u32 = 6;
const IF_TYPE_SOFTWARE_LOOPBACK: u32 = 24;
const AF_UNSPEC: u16 = 0;
const AF_INET: u16 = 2;
const AF_INET6: u16 = 23;

/// MIB_IF_ROW2 and MIB_UNICASTIPADDRESS_ROW sizes; tables put their rows
/// at 8 (after NumEntries, aligned).
const ROW2: usize = 1352;
const UNICAST_ROW: usize = 80;

#[derive(Clone, Copy)]
struct Interface {
    index: u32,
    if_type: u32,
    mtu: u32,
    mac_len: u32,
    mac: [u8; 8],
    ipv4: [u8; 4],
    prefix: u32,
    luid_index: u32,
}

impl Interface {
    /// NET_LUID: NetLuidIndex in bits 24-47, IfType in 48-63.
    fn luid(&self) -> u64 {
        (u64::from(self.if_type) << 48) | (u64::from(self.luid_index) << 24)
    }
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

/// The interfaces the kernel reports (at most two).
fn interfaces() -> ([Option<Interface>; 2], usize) {
    let mut buf = [0u8; 2 * RECORD];
    let mut ret = 0u32;
    // SAFETY: our buffer.
    let status = unsafe {
        NtQuerySystemInformation(
            GALA_NETWORK_INTERFACES,
            buf.as_mut_ptr(),
            buf.len() as u32,
            &mut ret,
        )
    };
    let mut list = [None; 2];
    if status != 0 {
        return (list, 0);
    }
    let n = (ret as usize / RECORD).min(2);
    for (i, slot) in list.iter_mut().enumerate().take(n) {
        let r = &buf[i * RECORD..(i + 1) * RECORD];
        let mut mac = [0u8; 8];
        mac.copy_from_slice(&r[16..24]);
        *slot = Some(Interface {
            index: u32_at(r, 0),
            if_type: u32_at(r, 4),
            mtu: u32_at(r, 8),
            mac_len: u32_at(r, 12),
            mac,
            ipv4: [r[24], r[25], r[26], r[27]],
            prefix: u32_at(r, 28),
            luid_index: u32_at(r, 32),
        });
    }
    (list, n)
}

fn put(row: &mut [u8], at: usize, v: &[u8]) {
    row[at..at + v.len()].copy_from_slice(v);
}

fn put_wide(row: &mut [u8], at: usize, s: &str) {
    for (i, b) in s.bytes().enumerate() {
        put(row, at + 2 * i, &u16::from(b).to_le_bytes());
    }
}

/// Fills a MIB_IF_ROW2 (zeroed) for `i`.
fn if_row(i: &Interface, row: &mut [u8]) {
    let loopback = i.if_type == IF_TYPE_SOFTWARE_LOOPBACK;
    put(row, 0, &i.luid().to_le_bytes());
    put(row, 8, &i.index.to_le_bytes());
    // InterfaceGuid: stable per interface (gala's own values).
    put(row, 12, &[0x67, 0x61, 0x6c, 0x61, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, i.index as u8]);
    let (alias, description) = if loopback {
        ("Loopback Pseudo-Interface 1", "Software Loopback Interface 1")
    } else {
        ("Ethernet", "VirtIO Ethernet Adapter")
    };
    put_wide(row, 28, alias);
    put_wide(row, 542, description);
    put(row, 1056, &i.mac_len.to_le_bytes());
    put(row, 1060, &i.mac[..6]);
    put(row, 1092, &i.mac[..6]);
    put(row, 1124, &i.mtu.to_le_bytes());
    put(row, 1128, &i.if_type.to_le_bytes());
    // TunnelType none; MediaType NdisMedium802_3 (0); PhysicalMediumType
    // 802.3 for the card; AccessType loopback (1) or broadcast (2).
    if !loopback {
        put(row, 1140, &14u32.to_le_bytes());
    }
    put(row, 1144, &(if loopback { 1u32 } else { 2 }).to_le_bytes());
    // InterfaceAndOperStatusFlags: a hardware interface with a connector.
    if !loopback {
        row[1152] = 0b11;
    }
    put(row, 1156, &1u32.to_le_bytes()); // OperStatus: IfOperStatusUp
    put(row, 1160, &1u32.to_le_bytes()); // AdminStatus: up
    put(row, 1164, &1u32.to_le_bytes()); // MediaConnectState: connected
    put(row, 1184, &1u32.to_le_bytes()); // ConnectionType: dedicated
    let speed: u64 = if loopback { 1_073_741_824 } else { 1_000_000_000 };
    put(row, 1192, &speed.to_le_bytes());
    put(row, 1200, &speed.to_le_bytes());
}

/// Fills a MIB_UNICASTIPADDRESS_ROW (zeroed) for `i`'s IPv4 address.
fn unicast_row(i: &Interface, row: &mut [u8]) {
    put(row, 0, &AF_INET.to_le_bytes());
    put(row, 4, &i.ipv4);
    put(row, 32, &i.luid().to_le_bytes());
    put(row, 40, &i.index.to_le_bytes());
    // Prefix and suffix origins: well known (loopback) or manual.
    let origin: u32 = if i.if_type == IF_TYPE_SOFTWARE_LOOPBACK { 2 } else { 1 };
    put(row, 44, &origin.to_le_bytes());
    put(row, 48, &origin.to_le_bytes());
    put(row, 52, &u32::MAX.to_le_bytes()); // ValidLifetime: infinite
    put(row, 56, &u32::MAX.to_le_bytes()); // PreferredLifetime
    row[60] = i.prefix as u8;
    put(row, 64, &4u32.to_le_bytes()); // DadState: IpDadStatePreferred
}

/// A MIB_*_TABLE of `count` rows of `size` from the heap (FreeMibTable).
fn new_table(count: usize, size: usize) -> Option<*mut u8> {
    // SAFETY: plain calls; HEAP_ZERO_MEMORY.
    let p = unsafe { HeapAlloc(GetProcessHeap(), 8, 8 + count * size) };
    if p.is_null() {
        return None;
    }
    // SAFETY: our allocation.
    unsafe { p.cast::<u32>().write(count as u32) };
    Some(p)
}

/// The row at `i` of a table from `new_table`.
fn row_of<'a>(table: *mut u8, i: usize, size: usize) -> &'a mut [u8] {
    // SAFETY: inside the allocation of `new_table`.
    unsafe { core::slice::from_raw_parts_mut(table.add(8 + i * size), size) }
}

/// GetIfTable2(*Table): every interface.
#[unsafe(no_mangle)]
pub extern "system" fn GetIfTable2(out: *mut *mut u8) -> u32 {
    if out.is_null() {
        return ERROR_INVALID_PARAMETER;
    }
    let (list, n) = interfaces();
    let Some(table) = new_table(n, ROW2) else { return ERROR_NOT_ENOUGH_MEMORY };
    for (k, i) in list.iter().flatten().enumerate() {
        if_row(i, row_of(table, k, ROW2));
    }
    // SAFETY: the caller's output.
    unsafe { *out = table };
    NO_ERROR
}

/// GetIfTable2Ex(Level, *Table): as GetIfTable2 (gala has no hidden
/// filter interfaces).
#[unsafe(no_mangle)]
pub extern "system" fn GetIfTable2Ex(_level: u32, out: *mut *mut u8) -> u32 {
    GetIfTable2(out)
}

/// GetIfEntry2(*Row): the row of InterfaceLuid, or else InterfaceIndex.
#[unsafe(no_mangle)]
pub extern "system" fn GetIfEntry2(row: *mut u8) -> u32 {
    if row.is_null() {
        return ERROR_INVALID_PARAMETER;
    }
    // SAFETY: the caller's MIB_IF_ROW2.
    let row = unsafe { core::slice::from_raw_parts_mut(row, ROW2) };
    let luid = u64::from_le_bytes(row[0..8].try_into().unwrap_or([0; 8]));
    let index = u32_at(row, 8);
    if luid == 0 && index == 0 {
        return ERROR_INVALID_PARAMETER;
    }
    let (list, _) = interfaces();
    let found =
        list.iter().flatten().find(|i| if luid != 0 { i.luid() == luid } else { i.index == index });
    match found {
        Some(i) => {
            row.fill(0);
            if_row(i, row);
            NO_ERROR
        }
        None => ERROR_FILE_NOT_FOUND,
    }
}

/// GetUnicastIpAddressTable(Family, *Table): the IPv4 addresses (gala
/// has no IPv6: none for AF_INET6).
#[unsafe(no_mangle)]
pub extern "system" fn GetUnicastIpAddressTable(family: u16, out: *mut *mut u8) -> u32 {
    if out.is_null() || !matches!(family, AF_UNSPEC | AF_INET | AF_INET6) {
        return ERROR_INVALID_PARAMETER;
    }
    let (list, n) = interfaces();
    let n = if family == AF_INET6 { 0 } else { n };
    let Some(table) = new_table(n, UNICAST_ROW) else { return ERROR_NOT_ENOUGH_MEMORY };
    for (k, i) in list.iter().flatten().take(n).enumerate() {
        unicast_row(i, row_of(table, k, UNICAST_ROW));
    }
    // SAFETY: the caller's output.
    unsafe { *out = table };
    NO_ERROR
}

/// GetAnycastIpAddressTable(Family, *Table): none.
#[unsafe(no_mangle)]
pub extern "system" fn GetAnycastIpAddressTable(family: u16, out: *mut *mut u8) -> u32 {
    if out.is_null() || !matches!(family, AF_UNSPEC | AF_INET | AF_INET6) {
        return ERROR_INVALID_PARAMETER;
    }
    let Some(table) = new_table(0, 48) else { return ERROR_NOT_ENOUGH_MEMORY };
    // SAFETY: the caller's output.
    unsafe { *out = table };
    NO_ERROR
}

/// FreeMibTable: tables come from the process heap.
#[unsafe(no_mangle)]
pub extern "system" fn FreeMibTable(table: *mut u8) {
    if !table.is_null() {
        // SAFETY: a table from this DLL.
        unsafe { HeapFree(GetProcessHeap(), 0, table) };
    }
}

/// The interface-name prefix of an IFTYPE (`loopback_0`, `ethernet_32768`).
fn type_name(if_type: u32) -> Option<&'static str> {
    match if_type {
        IF_TYPE_SOFTWARE_LOOPBACK => Some("loopback"),
        IF_TYPE_ETHERNET_CSMACD => Some("ethernet"),
        _ => None,
    }
}

/// ConvertInterfaceLuidToNameW(*Luid, Name, Length): `<type>_<index>`.
#[unsafe(no_mangle)]
pub extern "system" fn ConvertInterfaceLuidToNameW(
    luid: *const u64,
    name: *mut u16,
    length: usize,
) -> u32 {
    if luid.is_null() || name.is_null() {
        return ERROR_INVALID_PARAMETER;
    }
    // SAFETY: the caller's LUID.
    let luid = unsafe { *luid };
    let Some(prefix) = type_name((luid >> 48) as u32) else { return ERROR_INVALID_PARAMETER };
    let mut digits = [0u8; 8];
    let mut v = (luid >> 24) & 0xff_ffff;
    let mut d = 0;
    loop {
        digits[d] = b'0' + (v % 10) as u8;
        d += 1;
        v /= 10;
        if v == 0 {
            break;
        }
    }
    let total = prefix.len() + 1 + d;
    if length < total + 1 {
        return ERROR_NOT_ENOUGH_MEMORY;
    }
    let chars = prefix.bytes().chain(*b"_").chain(digits[..d].iter().rev().copied()).chain([0]);
    for (k, b) in chars.enumerate() {
        // SAFETY: the caller's buffer of `length` > total characters.
        unsafe { *name.add(k) = u16::from(b) };
    }
    NO_ERROR
}

/// ConvertInterfaceNameToLuidW(Name, *Luid): the inverse.
#[unsafe(no_mangle)]
pub extern "system" fn ConvertInterfaceNameToLuidW(name: *const u16, luid: *mut u64) -> u32 {
    if name.is_null() || luid.is_null() {
        return ERROR_INVALID_PARAMETER;
    }
    let mut text = [0u8; 32];
    let mut n = 0;
    // SAFETY: the caller's NUL-terminated name.
    unsafe {
        while *name.add(n) != 0 {
            if n == text.len() || *name.add(n) > 0x7f {
                return ERROR_INVALID_PARAMETER;
            }
            text[n] = *name.add(n) as u8;
            n += 1;
        }
    }
    let text = &text[..n];
    let Some(split) = text.iter().rposition(|&b| b == b'_') else { return ERROR_INVALID_PARAMETER };
    let (prefix, digits) = (&text[..split], &text[split + 1..]);
    let if_type = [IF_TYPE_SOFTWARE_LOOPBACK, IF_TYPE_ETHERNET_CSMACD]
        .into_iter()
        .find(|&t| type_name(t).is_some_and(|p| p.as_bytes() == prefix));
    let Some(if_type) = if_type else { return ERROR_INVALID_PARAMETER };
    if digits.is_empty() || digits.len() > 8 || !digits.iter().all(u8::is_ascii_digit) {
        return ERROR_INVALID_PARAMETER;
    }
    let index = digits.iter().fold(0u64, |v, &d| v * 10 + u64::from(d - b'0'));
    // SAFETY: the caller's output.
    unsafe { *luid = (u64::from(if_type) << 48) | ((index & 0xff_ffff) << 24) };
    NO_ERROR
}

/// ConvertInterfaceLuidToIndex(*Luid, *Index)
#[unsafe(no_mangle)]
pub extern "system" fn ConvertInterfaceLuidToIndex(luid: *const u64, index: *mut u32) -> u32 {
    if luid.is_null() || index.is_null() {
        return ERROR_INVALID_PARAMETER;
    }
    let (list, _) = interfaces();
    // SAFETY: the caller's LUID and output.
    unsafe {
        match list.iter().flatten().find(|i| i.luid() == *luid) {
            Some(i) => {
                *index = i.index;
                NO_ERROR
            }
            None => ERROR_FILE_NOT_FOUND,
        }
    }
}

/// ConvertInterfaceIndexToLuid(Index, *Luid)
#[unsafe(no_mangle)]
pub extern "system" fn ConvertInterfaceIndexToLuid(index: u32, luid: *mut u64) -> u32 {
    if luid.is_null() {
        return ERROR_INVALID_PARAMETER;
    }
    let (list, _) = interfaces();
    // SAFETY: the caller's output.
    unsafe {
        match list.iter().flatten().find(|i| i.index == index) {
            Some(i) => {
                *luid = i.luid();
                NO_ERROR
            }
            None => ERROR_FILE_NOT_FOUND,
        }
    }
}
