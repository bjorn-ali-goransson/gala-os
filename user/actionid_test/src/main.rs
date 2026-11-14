//! wintrust's trust provider registration, WintrustAddActionID and
//! WintrustRemoveActionID (wintrust.h, softpub.h), as Office's
//! Click-to-Run integrator registers its VBA trust provider (ROADMAP OF).
//! Expected values are from the functions' documentation and the
//! registry entries Windows' own providers have (WINTRUST_ACTION_GENERIC_
//! VERIFY_V2's under the same keys):
//!
//! - a NULL action ID or a NULL CRYPT_REGISTER_ACTIONID is
//!   ERROR_INVALID_PARAMETER;
//! - with its eight providers given, the action is added (with
//!   WT_ADD_ACTION_ID_RET_RESULT_FLAG: the return value is the result),
//!   writing `HKLM\Software\Microsoft\Cryptography\Providers\Trust\
//!   <Initialization|Message|Signature|Certificate|CertCheck|FinalPolicy|
//!   DiagnosticPolicy|Cleanup>\{GUID}` with `$DLL` and `$Function`
//!   (REG_SZ);
//! - removing it succeeds and deletes the eight keys.
//!
//! Writing HKLM needs an administrator (the scenario runs as SYSTEM; on
//! Windows run-all.ps1 runs elevated). Cases the documentation leaves
//! open print `actionid_test: info` with their result, to be compared with
//! a Windows run: a structure whose providers are all empty, and removing
//! an action never registered.
//!
//! Prints `actionid_test: ok`, or `actionid_test: check N got X (line L)`
//! and exits with N.

#![no_std]
#![no_main]

use core::ptr::null;

use ntrt as _;

const NAME: &str = "actionid_test";

type Handle = usize;

#[repr(C)]
struct Guid {
    data1: u32,
    data2: u16,
    data3: u16,
    data4: [u8; 8],
}

/// CRYPT_TRUST_REG_ENTRY.
#[repr(C)]
#[derive(Clone, Copy)]
struct RegEntry {
    size: u32,
    dll: *const u16,
    function: *const u16,
}

/// CRYPT_REGISTER_ACTIONID.
#[repr(C)]
struct RegisterActionId {
    size: u32,
    providers: [RegEntry; 8],
}

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, overlapped: usize) -> i32;
    fn ExitProcess(code: u32) -> !;
    fn GetLastError() -> u32;
    fn SetLastError(code: u32);
}

#[link(name = "wintrust", kind = "raw-dylib")]
unsafe extern "system" {
    fn WintrustAddActionID(action: *const Guid, flags: u32, info: *const RegisterActionId) -> i32;
    fn WintrustRemoveActionID(action: *const Guid) -> i32;
}

#[link(name = "advapi32", kind = "raw-dylib")]
unsafe extern "system" {
    fn RegOpenKeyExW(
        key: usize,
        path: *const u16,
        options: u32,
        access: u32,
        out: *mut usize,
    ) -> i32;
    fn RegQueryValueExW(
        key: usize,
        name: *const u16,
        reserved: *mut u32,
        kind: *mut u32,
        data: *mut u8,
        size: *mut u32,
    ) -> i32;
    fn RegCloseKey(key: usize) -> i32;
}

const HKEY_LOCAL_MACHINE: usize = 0x8000_0002;
const KEY_READ: u32 = 0x20019;
const REG_SZ: u32 = 1;
const ERROR_SUCCESS: u32 = 0;
const ERROR_INVALID_PARAMETER: u32 = 87;
const WT_ADD_ACTION_ID_RET_RESULT_FLAG: u32 = 1;

/// An action ID made for this test (no provider uses it), and its
/// registry name.
const ACTION: Guid = Guid {
    data1: 0x5e0b_74c1,
    data2: 0x9a3d,
    data3: 0x4c28,
    data4: [0xb1, 0x6e, 0x0f, 0x47, 0xd2, 0x83, 0xa5, 0x19],
};
const ACTION_TEXT: &str = "{5E0B74C1-9A3D-4C28-B16E-0F47D283A519}";
const DLL: &str = "C:\\gala\\actionid_test_dummy.dll";

const TRUST: &str = "Software\\Microsoft\\Cryptography\\Providers\\Trust\\";
/// The providers' keys, in CRYPT_REGISTER_ACTIONID's order.
const KEYS: [&str; 8] = [
    "Initialization",
    "Message",
    "Signature",
    "Certificate",
    "CertCheck",
    "FinalPolicy",
    "DiagnosticPolicy",
    "Cleanup",
];

struct Line {
    buf: [u8; 200],
    n: usize,
}

impl Line {
    fn new() -> Self {
        Self { buf: [0; 200], n: 0 }
    }

    fn s(mut self, s: &[u8]) -> Self {
        for &b in s {
            if self.n < self.buf.len() {
                self.buf[self.n] = b;
                self.n += 1;
            }
        }
        self
    }

    fn dec(self, v: u64) -> Self {
        let mut d = [0u8; 20];
        let mut i = d.len();
        let mut v = v;
        loop {
            i -= 1;
            d[i] = b'0' + (v % 10) as u8;
            v /= 10;
            if v == 0 {
                break;
            }
        }
        self.s(&d[i..])
    }

    fn hex(self, v: u64) -> Self {
        let mut d = [0u8; 18];
        d[0] = b'0';
        d[1] = b'x';
        for i in 0..16 {
            d[2 + i] = b"0123456789abcdef"[((v >> (60 - 4 * i)) & 0xf) as usize];
        }
        self.s(&d)
    }

    fn print(self) {
        let mut written = 0;
        // SAFETY: our buffer; the standard output handle.
        unsafe {
            WriteFile(
                GetStdHandle(-11i32 as u32),
                self.buf.as_ptr(),
                self.n as u32,
                &mut written,
                0,
            )
        };
    }
}

struct Checks(u32);

impl Checks {
    #[track_caller]
    fn ok(&mut self, cond: bool, got: u64) -> Result<(), u32> {
        self.0 += 1;
        if cond {
            return Ok(());
        }
        Line::new()
            .s(NAME.as_bytes())
            .s(b": check ")
            .dec(u64::from(self.0))
            .s(b" got ")
            .hex(got)
            .s(b" (line ")
            .dec(u64::from(core::panic::Location::caller().line()))
            .s(b")\r\n")
            .print();
        Err(self.0)
    }

    /// A call that returned `ret` must have failed with `error`.
    #[track_caller]
    fn fails(&mut self, ret: i32, error: u32) -> Result<(), u32> {
        // SAFETY: plain call.
        let e = unsafe { GetLastError() };
        self.ok(ret == 0, ret as u32 as u64)?;
        self.ok(e == error, e.into())
    }
}

/// A NUL-terminated UTF-16 copy of the parts (at most 159 characters).
fn wide(parts: &[&str]) -> [u16; 160] {
    let mut w = [0u16; 160];
    let mut i = 0;
    for p in parts {
        for b in p.bytes() {
            if i < 159 {
                w[i] = b.into();
                i += 1;
            }
        }
    }
    w
}

/// The `$DLL` and `$Function` values of `<key>\{ACTION}`: None if the
/// key is absent, else whether both are REG_SZ with this test's strings
/// (the function named after its key).
fn registered(key_name: &str) -> Option<bool> {
    let path = wide(&[TRUST, key_name, "\\", ACTION_TEXT]);
    let mut key = 0usize;
    // SAFETY: our buffers.
    if unsafe { RegOpenKeyExW(HKEY_LOCAL_MACHINE, path.as_ptr(), 0, KEY_READ, &mut key) } != 0 {
        return None;
    }
    let value = |name: &str, want: &[&str]| {
        let name = wide(&[name]);
        let mut data = [0u16; 160];
        let (mut kind, mut size) = (0u32, (data.len() * 2) as u32);
        // SAFETY: our key and buffers.
        let e = unsafe {
            RegQueryValueExW(
                key,
                name.as_ptr(),
                core::ptr::null_mut(),
                &mut kind,
                data.as_mut_ptr().cast(),
                &mut size,
            )
        };
        let want = wide(want);
        let n = want.iter().position(|&c| c == 0).unwrap_or(0) + 1;
        e == 0 && kind == REG_SZ && size as usize == n * 2 && data[..n] == want[..n]
    };
    let ok = value("$DLL", &[DLL]) && value("$Function", &["Dummy", key_name]);
    // SAFETY: our key.
    unsafe { RegCloseKey(key) };
    Some(ok)
}

fn info(what: &[u8], ret: i32, error: u32) {
    Line::new()
        .s(NAME.as_bytes())
        .s(b": info ")
        .s(what)
        .s(b": ")
        .dec(ret as u32 as u64)
        .s(b" ")
        .dec(if ret != 0 { u64::from(ERROR_SUCCESS) } else { error.into() })
        .s(b"\r\n")
        .print();
}

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    let dll = wide(&[DLL]);
    let functions = KEYS.map(|k| wide(&["Dummy", k]));
    // Just in case an earlier run left it.
    // SAFETY: our GUID.
    unsafe { WintrustRemoveActionID(&ACTION) };

    // SAFETY: plain calls with NULL.
    unsafe {
        SetLastError(0xdead_beef);
        c.fails(WintrustAddActionID(null(), 0, null()), ERROR_INVALID_PARAMETER)?;
        SetLastError(0xdead_beef);
        c.fails(WintrustAddActionID(&ACTION, 0, null()), ERROR_INVALID_PARAMETER)?;
    }

    // Every provider empty: left open (an info line).
    let empty = RegEntry { size: 0, dll: null(), function: null() };
    let mut info_struct = RegisterActionId {
        size: core::mem::size_of::<RegisterActionId>() as u32,
        providers: [empty; 8],
    };
    // SAFETY: our structure.
    let (ret, e) = unsafe {
        SetLastError(0xdead_beef);
        (
            WintrustAddActionID(&ACTION, WT_ADD_ACTION_ID_RET_RESULT_FLAG, &info_struct),
            GetLastError(),
        )
    };
    info(b"all providers empty", ret, e);
    // SAFETY: our GUID.
    unsafe { WintrustRemoveActionID(&ACTION) };
    for (i, k) in KEYS.iter().enumerate() {
        c.ok(registered(k).is_none(), i as u64)?;
    }

    // The eight providers.
    for (entry, function) in info_struct.providers.iter_mut().zip(&functions) {
        *entry = RegEntry {
            size: core::mem::size_of::<RegEntry>() as u32,
            dll: dll.as_ptr(),
            function: function.as_ptr(),
        };
    }
    // SAFETY: our structure.
    let ret =
        unsafe { WintrustAddActionID(&ACTION, WT_ADD_ACTION_ID_RET_RESULT_FLAG, &info_struct) };
    // SAFETY: plain call.
    c.ok(ret != 0, unsafe { GetLastError() }.into())?;
    for (i, k) in KEYS.iter().enumerate() {
        c.ok(registered(k) == Some(true), i as u64)?;
    }
    // Added again over itself (flags 0, as installers call it).
    // SAFETY: our structure.
    let ret = unsafe { WintrustAddActionID(&ACTION, 0, &info_struct) };
    // SAFETY: plain call.
    c.ok(ret != 0, unsafe { GetLastError() }.into())?;
    // SAFETY: our GUID.
    let ret = unsafe { WintrustRemoveActionID(&ACTION) };
    // SAFETY: plain call.
    c.ok(ret != 0, unsafe { GetLastError() }.into())?;
    for (i, k) in KEYS.iter().enumerate() {
        c.ok(registered(k).is_none(), i as u64)?;
    }

    // Never registered (now): left open.
    // SAFETY: our GUID.
    let (ret, e) = unsafe {
        SetLastError(0xdead_beef);
        (WintrustRemoveActionID(&ACTION), GetLastError())
    };
    info(b"remove unregistered", ret, e);
    Ok(())
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    let code = match run() {
        Ok(()) => {
            Line::new().s(NAME.as_bytes()).s(b": ok\r\n").print();
            0
        }
        Err(n) => n,
    };
    // SAFETY: ends the process.
    unsafe { ExitProcess(code) }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xc000_0409) }
}
