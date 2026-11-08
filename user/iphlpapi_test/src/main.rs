//! iphlpapi's interface and address tables (netioapi.h), as Java's
//! NetworkInterface uses them, with the results real Windows gives: the
//! loopback interface is index 1, IF_TYPE_SOFTWARE_LOOPBACK, alias
//! "Loopback Pseudo-Interface 1", up, with 127.0.0.1/8; GetIfEntry2 finds
//! a row by index (ERROR_FILE_NOT_FOUND for an unknown one,
//! ERROR_INVALID_PARAMETER with neither LUID nor index); interface names
//! are `<type>_<NetLuidIndex>` (`loopback_0`) and convert back to the
//! LUID, also through if_nametoindex / if_indextoname (0 and NULL for
//! unknown ones); tables come from the heap and go back with FreeMibTable.
//!
//! Prints `iphlpapi_test: ok` or exits with the number of the failing
//! check.

#![no_std]
#![no_main]

use core::ffi::c_void;

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetStdHandle(which: u32) -> *mut c_void;
    fn WriteFile(
        h: *mut c_void,
        buf: *const u8,
        len: u32,
        written: *mut u32,
        ov: *mut c_void,
    ) -> i32;
    fn ExitProcess(code: u32) -> !;
}

#[link(name = "iphlpapi", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetIfTable2(table: *mut *mut u8) -> u32;
    fn GetIfEntry2(row: *mut u8) -> u32;
    fn GetUnicastIpAddressTable(family: u16, table: *mut *mut u8) -> u32;
    fn FreeMibTable(table: *mut u8);
    fn ConvertInterfaceLuidToNameW(luid: *const u64, name: *mut u16, length: usize) -> u32;
    fn ConvertInterfaceNameToLuidW(name: *const u16, luid: *mut u64) -> u32;
    fn if_nametoindex(name: *const u8) -> u32;
    fn if_indextoname(index: u32, name: *mut u8) -> *mut u8;
}

#[allow(dead_code)]
#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "C" {
    fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memset(dst: *mut u8, value: i32, n: usize) -> *mut u8;
    fn memcmp(a: *const u8, b: *const u8, n: usize) -> i32;
    fn __chkstk();
}

#[unsafe(no_mangle)]
pub static _fltused: i32 = 0;

#[unsafe(no_mangle)]
pub extern "C" fn __CxxFrameHandler3() -> ! {
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xdeac) }
}

/// MIB_IF_ROW2 (x64): InterfaceLuid 0, InterfaceIndex 8, Alias 28,
/// Description 542, Type 1128, OperStatus 1156; 1352 bytes.
const ROW2: usize = 1352;
/// MIB_UNICASTIPADDRESS_ROW: Address (SOCKADDR_INET) 0, InterfaceLuid 32,
/// InterfaceIndex 40, OnLinkPrefixLength 60; 80 bytes.
const UNICAST_ROW: usize = 80;
const IF_TYPE_SOFTWARE_LOOPBACK: u32 = 24;
const AF_INET: u16 = 2;
const ERROR_FILE_NOT_FOUND: u64 = 2;
const ERROR_INVALID_PARAMETER: u64 = 87;

fn out(text: &[u8]) {
    let mut written = 0;
    // SAFETY: valid buffer and output pointer.
    unsafe {
        WriteFile(
            GetStdHandle(-11i32 as u32),
            text.as_ptr(),
            text.len() as u32,
            &mut written,
            core::ptr::null_mut(),
        )
    };
}

fn hex(v: u32) -> [u8; 8] {
    let mut s = [0u8; 8];
    for (i, c) in s.iter_mut().enumerate() {
        let d = (v >> (28 - 4 * i)) & 0xf;
        *c = if d < 10 { b'0' + d as u8 } else { b'a' + d as u8 - 10 };
    }
    s
}

struct Checks(u32);

impl Checks {
    #[track_caller]
    fn ok(&mut self, cond: bool, got: u64) -> Result<(), u32> {
        self.0 += 1;
        if cond {
            return Ok(());
        }
        out(b"iphlpapi_test: check ");
        out(&hex(self.0));
        out(b" got ");
        out(&hex(got as u32));
        check_line(core::panic::Location::caller().line());
        out(b"\r\n");
        Err(self.0)
    }

    #[track_caller]
    fn eq(&mut self, got: u64, want: u64) -> Result<(), u32> {
        self.ok(got == want, got)
    }
}

fn u32_at(p: *const u8, off: usize) -> u32 {
    // SAFETY: callers read inside rows they were given.
    unsafe { p.add(off).cast::<u32>().read_unaligned() }
}

fn u64_at(p: *const u8, off: usize) -> u64 {
    // SAFETY: as above.
    unsafe { p.add(off).cast::<u64>().read_unaligned() }
}

/// Whether the NUL-terminated UTF-16 at `p + off` is `want`.
fn text_is(p: *const u8, off: usize, want: &str) -> bool {
    // SAFETY: as above; the strings are terminated.
    unsafe {
        let s = p.add(off).cast::<u16>();
        want.bytes().enumerate().all(|(i, b)| s.add(i).read_unaligned() == u16::from(b))
            && s.add(want.len()).read_unaligned() == 0
    }
}

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    // SAFETY: iphlpapi calls with buffers of ours; tables from the API.
    unsafe {
        let mut table = core::ptr::null_mut();
        c.eq(GetIfTable2(&mut table).into(), 0)?;
        let count = u32_at(table, 0) as usize;
        c.ok(count >= 1, count as u64)?;
        // MIB_IF_TABLE2: NumEntries, then the rows (8-aligned).
        let rows = table.add(8);
        let lo = (0..count).map(|i| rows.add(i * ROW2)).find(|&r| u32_at(r, 8) == 1);
        c.ok(lo.is_some(), 0)?;
        let lo = lo.unwrap_or(rows);
        c.eq(u32_at(lo, 1128).into(), IF_TYPE_SOFTWARE_LOOPBACK.into())?;
        c.ok(text_is(lo, 28, "Loopback Pseudo-Interface 1"), 0)?;
        c.eq(u32_at(lo, 1156).into(), 1)?; // IfOperStatusUp
        let luid = u64_at(lo, 0);
        c.eq(luid >> 48, IF_TYPE_SOFTWARE_LOOPBACK.into())?; // NET_LUID.IfType
        FreeMibTable(table);

        // GetIfEntry2 by index.
        let mut row = [0u8; ROW2];
        row[8..12].copy_from_slice(&1u32.to_le_bytes());
        c.eq(GetIfEntry2(row.as_mut_ptr()).into(), 0)?;
        c.ok(text_is(row.as_ptr(), 28, "Loopback Pseudo-Interface 1"), 0)?;
        c.eq(u64_at(row.as_ptr(), 0), luid)?;
        let mut row = [0u8; ROW2];
        row[8..12].copy_from_slice(&9999u32.to_le_bytes());
        c.eq(GetIfEntry2(row.as_mut_ptr()).into(), ERROR_FILE_NOT_FOUND)?;
        let mut row = [0u8; ROW2];
        c.eq(GetIfEntry2(row.as_mut_ptr()).into(), ERROR_INVALID_PARAMETER)?;

        // Names.
        let mut name = [0u16; 64];
        c.eq(ConvertInterfaceLuidToNameW(&luid, name.as_mut_ptr(), 64).into(), 0)?;
        let want = b"loopback_0\0";
        c.ok(name[..11].iter().zip(want).all(|(&a, &b)| a == u16::from(b)), name[0].into())?;
        let mut back = 0u64;
        c.eq(ConvertInterfaceNameToLuidW(name.as_ptr(), &mut back).into(), 0)?;
        c.eq(back, luid)?;
        // The same names through if_nametoindex / if_indextoname.
        c.eq(if_nametoindex(c"loopback_0".as_ptr().cast()).into(), 1)?;
        c.eq(if_nametoindex(c"nosuch_0".as_ptr().cast()).into(), 0)?;
        let mut short = [0xffu8; 256]; // IF_NAMESIZE
        c.ok(if_indextoname(1, short.as_mut_ptr()) == short.as_mut_ptr(), 0)?;
        c.ok(&short[..11] == want, short[0].into())?;
        c.ok(if_indextoname(9999, short.as_mut_ptr()).is_null(), 0)?;

        // 127.0.0.1/8 on the loopback interface (SOCKADDR_IN: family,
        // port, address).
        let mut table = core::ptr::null_mut();
        c.eq(GetUnicastIpAddressTable(AF_INET, &mut table).into(), 0)?;
        let count = u32_at(table, 0) as usize;
        let rows = table.add(8);
        let found = (0..count).map(|i| rows.add(i * UNICAST_ROW)).find(|&r| {
            u32_at(r, 4) == u32::from_be_bytes([127, 0, 0, 1]).to_be()
                && u32_at(r, 0) & 0xffff == u32::from(AF_INET)
        });
        c.ok(found.is_some(), count as u64)?;
        let r = found.unwrap_or(rows);
        c.eq(u32_at(r, 40).into(), 1)?; // InterfaceIndex
        c.eq(u64_at(r, 32), luid)?;
        c.eq((u32_at(r, 60) & 0xff).into(), 8)?; // OnLinkPrefixLength
        FreeMibTable(table);
    }
    Ok(())
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    let code = match run() {
        Ok(()) => {
            out(b"iphlpapi_test: ok\r\n");
            0
        }
        Err(id) => id,
    };
    // SAFETY: ends the process.
    unsafe { ExitProcess(code) }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xdead) }
}

/// ` (line L)`: the source line of the failing check, from
/// `#[track_caller]` on Checks' methods (so a report names the line
/// without anyone counting checks).
fn check_line(line: u32) {
    let mut b = *b" (line 0000000000)";
    let mut digits = [0u8; 10];
    let (mut n, mut len) = (line, 0);
    loop {
        digits[len] = b'0' + (n % 10) as u8;
        len += 1;
        n /= 10;
        if n == 0 {
            break;
        }
    }
    for i in 0..len {
        b[7 + i] = digits[len - 1 - i];
    }
    b[7 + len] = b')';
    out(&b[..8 + len]);
}
