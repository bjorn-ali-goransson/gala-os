//! clusapi on a machine where failover clustering is not installed (a
//! Windows client), with the results Windows documents: GetNodeClusterState
//! succeeds with ClusterStateNotInstalled (0); OpenCluster finds no
//! cluster (NULL and an error); every function a program resolves at run
//! time exists (GetProcAddress), as SQL Server needs. resutils: the
//! property-list helpers (ResUtilFind*Property over a CLUSPROP list:
//! names matched case-insensitively, ERROR_FILE_NOT_FOUND when absent,
//! strings returned in LocalAlloc'ed memory) and ResUtilResourceTypesEqual.
//!
//! Prints `clusapi_test: ok` or exits with the number of the failing check.

#![no_std]
#![no_main]

use core::ffi::c_void;

type Handle = *mut c_void;

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, ov: *mut c_void) -> i32;
    fn ExitProcess(code: u32) -> !;
    fn GetLastError() -> u32;
    fn GetModuleHandleA(name: *const u8) -> usize;
    fn GetProcAddress(module: usize, name: *const u8) -> usize;
}

#[link(name = "resutils", kind = "raw-dylib")]
unsafe extern "system" {
    fn ResUtilFindDwordProperty(list: *const u8, size: u32, name: *const u16, out: *mut u32)
    -> u32;
    fn ResUtilFindSzProperty(
        list: *const u8,
        size: u32,
        name: *const u16,
        out: *mut *mut u16,
    ) -> u32;
    fn ResUtilResourceTypesEqual(a: *const u16, b: *const u16) -> i32;
}

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn LocalFree(p: *mut u16) -> *mut u16;
}

/// A NUL-terminated UTF-16 copy of an ASCII string.
fn w(s: &str) -> [u16; 32] {
    let mut b = [0u16; 32];
    for (d, c) in b.iter_mut().zip(s.bytes()) {
        *d = c.into();
    }
    b
}

/// Appends a CLUSPROP value (syntax, length, data padded to 4 bytes).
fn push_value(list: &mut [u8; 256], at: &mut usize, syntax: u32, data: &[u8]) {
    list[*at..*at + 4].copy_from_slice(&syntax.to_le_bytes());
    list[*at + 4..*at + 8].copy_from_slice(&(data.len() as u32).to_le_bytes());
    list[*at + 8..*at + 8 + data.len()].copy_from_slice(data);
    *at += 8 + data.len().div_ceil(4) * 4;
}

/// A property list: Name "Port" = DWORD 1433, "Path" = SZ "C:\\x".
fn property_list() -> ([u8; 256], usize) {
    const NAME: u32 = 0x0004_0003; // CLUSPROP_SYNTAX_NAME
    const DWORD: u32 = 0x0001_0002; // CLUSPROP_SYNTAX_LIST_VALUE_DWORD
    const SZ: u32 = 0x0001_0003; // CLUSPROP_SYNTAX_LIST_VALUE_SZ
    let mut list = [0u8; 256];
    list[..4].copy_from_slice(&2u32.to_le_bytes());
    let mut at = 4;
    let utf16 = |s: &str, out: &mut [u8; 64]| {
        let mut n = 0;
        for c in s.encode_utf16().chain([0]) {
            out[n..n + 2].copy_from_slice(&c.to_le_bytes());
            n += 2;
        }
        n
    };
    let mut buf = [0u8; 64];
    let n = utf16("Port", &mut buf);
    push_value(&mut list, &mut at, NAME, &buf[..n]);
    push_value(&mut list, &mut at, DWORD, &1433u32.to_le_bytes());
    push_value(&mut list, &mut at, 0, &[]); // CLUSPROP_SYNTAX_ENDMARK
    let n = utf16("Path", &mut buf);
    push_value(&mut list, &mut at, NAME, &buf[..n]);
    let n = utf16("C:\\x", &mut buf);
    push_value(&mut list, &mut at, SZ, &buf[..n]);
    push_value(&mut list, &mut at, 0, &[]);
    (list, at)
}

#[link(name = "clusapi", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetNodeClusterState(node: *const u16, state: *mut u32) -> u32;
    fn OpenCluster(name: *const u16) -> *mut c_void;
}

#[allow(dead_code)]
#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "C" {
    fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memset(dst: *mut u8, value: i32, n: usize) -> *mut u8;
    fn memcmp(a: *const u8, b: *const u8, n: usize) -> i32;
}

#[unsafe(no_mangle)]
pub static _fltused: i32 = 0;

#[unsafe(no_mangle)]
pub extern "C" fn __CxxFrameHandler3() -> ! {
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xdeac) }
}

const ERROR_SUCCESS: u64 = 0;

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
    fn ok(&mut self, cond: bool, got: u64) -> Result<(), u32> {
        self.0 += 1;
        if cond {
            return Ok(());
        }
        out(b"clusapi_test: check ");
        out(&hex(self.0));
        out(b" got ");
        out(&hex(got as u32));
        out(b"\r\n");
        Err(self.0)
    }

    fn eq(&mut self, got: u64, want: u64) -> Result<(), u32> {
        self.ok(got == want, got)
    }
}

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    // SAFETY: clusapi calls with buffers of ours.
    unsafe {
        let mut state = 0xffff;
        c.eq(GetNodeClusterState(core::ptr::null(), &mut state).into(), ERROR_SUCCESS)?;
        c.eq(state.into(), 0)?; // ClusterStateNotInstalled
        c.ok(OpenCluster(core::ptr::null()).is_null(), 0)?;
        c.ok(GetLastError() != 0, 0)?;
        let module = GetModuleHandleA(c"clusapi.dll".as_ptr().cast());
        c.ok(module != 0, 0)?;
        for name in [
            c"OpenClusterResource",
            c"ClusterResourceControl",
            c"GetClusterResourceState",
            c"ClusterRegQueryValue",
            c"CloseCluster",
        ] {
            c.ok(GetProcAddress(module, name.as_ptr().cast()) != 0, 0)?;
        }

        let (list, size) = property_list();
        let mut port = 0;
        let r = ResUtilFindDwordProperty(list.as_ptr(), size as u32, w("port").as_ptr(), &mut port);
        c.eq(r.into(), ERROR_SUCCESS)?;
        c.eq(port.into(), 1433)?;
        let mut path = core::ptr::null_mut();
        let r = ResUtilFindSzProperty(list.as_ptr(), size as u32, w("Path").as_ptr(), &mut path);
        c.eq(r.into(), ERROR_SUCCESS)?;
        c.ok(!path.is_null() && *path == 0x43 && *path.add(3) == 0x78 && *path.add(4) == 0, 0)?;
        LocalFree(path);
        let r = ResUtilFindDwordProperty(list.as_ptr(), size as u32, w("Nope").as_ptr(), &mut port);
        c.eq(r.into(), 2)?; // ERROR_FILE_NOT_FOUND
        c.ok(
            ResUtilResourceTypesEqual(w("SQL Server").as_ptr(), w("sql server").as_ptr()) != 0,
            0,
        )?;
        c.eq(ResUtilResourceTypesEqual(w("SQL Server").as_ptr(), w("SQL").as_ptr()) as u64, 0)?;
    }
    Ok(())
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    let code = match run() {
        Ok(()) => {
            out(b"clusapi_test: ok\r\n");
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
