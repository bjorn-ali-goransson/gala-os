//! crypt32's subject interface package registration, CryptSIPAddProvider
//! and CryptSIPRemoveProvider (mssip.h), as Office's Click-to-Run
//! integrator registers its VBA project SIP (ROADMAP OF). Expected values
//! are Wine's crypt32 sip tests' (test_AddRemoveProvider), which pass on
//! Windows, and the registry entries Windows' own SIPs have:
//!
//! - a NULL provider or GUID is ERROR_INVALID_PARAMETER;
//! - removing a provider never registered fails with ERROR_FILE_NOT_FOUND;
//! - a provider without its IsMyFileType functions is added, writing
//!   `HKLM\Software\Microsoft\Cryptography\OID\EncodingType 0\
//!   CryptSIPDll<Function>\{GUID}` with `Dll` and `FuncName` (REG_SZ) for
//!   each function given and no key for those not given; removing it
//!   deletes its keys but fails with ERROR_FILE_NOT_FOUND (the two
//!   IsMyFileType keys were never there);
//! - with every function it is added (IsMyFileType2's key too) and
//!   removed with success, its keys gone.
//!
//! Writing HKLM needs an administrator (the scenario runs as SYSTEM; on
//! Windows run-all.ps1 runs elevated). A structure missing a required
//! function prints `sip_test: info` with its result (Wine's crypt32 fails
//! it with ERROR_INVALID_PARAMETER), to be compared with a Windows run.
//!
//! Prints `sip_test: ok`, or `sip_test: check N got X (line L)` and exits
//! with N.

#![no_std]
#![no_main]

use core::ptr::null;

use ntrt as _;

const NAME: &str = "sip_test";

type Handle = usize;

#[repr(C)]
struct Guid {
    data1: u32,
    data2: u16,
    data3: u16,
    data4: [u8; 8],
}

/// SIP_ADD_NEWPROVIDER (mssip.h, with Windows 8's pwszGetCapFuncName).
#[repr(C)]
struct NewProvider {
    size: u32,
    subject: *const Guid,
    dll: *const u16,
    magic: *const u16,
    is_function: *const u16,
    get: *const u16,
    put: *const u16,
    create: *const u16,
    verify: *const u16,
    remove: *const u16,
    is_function_fmt2: *const u16,
    get_cap: *const u16,
}

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, overlapped: usize) -> i32;
    fn ExitProcess(code: u32) -> !;
    fn GetLastError() -> u32;
    fn SetLastError(code: u32);
}

#[link(name = "crypt32", kind = "raw-dylib")]
unsafe extern "system" {
    fn CryptSIPAddProvider(p: *const NewProvider) -> i32;
    fn CryptSIPRemoveProvider(subject: *const Guid) -> i32;
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
const ERROR_FILE_NOT_FOUND: u32 = 2;
const ERROR_INVALID_PARAMETER: u32 = 87;

/// A GUID made for this test (no SIP uses it), and its registry name.
const SUBJECT: Guid = Guid {
    data1: 0x7c1d_93a2,
    data2: 0x40e5,
    data3: 0x4f6b,
    data4: [0x8d, 0x27, 0x3b, 0x9e, 0x51, 0xc0, 0x6a, 0x14],
};
const SUBJECT_TEXT: &str = "{7C1D93A2-40E5-4F6B-8D27-3B9E51C06A14}";
const DLL: &str = "C:\\gala\\sip_test_dummy.dll";
const FUNCTION: &str = "sip_test_dummy_function";

const OID: &str = "Software\\Microsoft\\Cryptography\\OID\\EncodingType 0\\CryptSIPDll";
const FUNCTIONS: [&str; 7] = [
    "PutSignedDataMsg",
    "GetSignedDataMsg",
    "RemoveSignedDataMsg",
    "CreateIndirectData",
    "VerifyIndirectData",
    "IsMyFileType",
    "IsMyFileType2",
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

/// The `Dll` and `FuncName` values of `CryptSIPDll<function>\{SUBJECT}`:
/// None if the key is absent, else whether both are REG_SZ with this
/// test's strings.
fn registered(function: &str) -> Option<bool> {
    let path = wide(&[OID, function, "\\", SUBJECT_TEXT]);
    let mut key = 0usize;
    // SAFETY: our buffers.
    if unsafe { RegOpenKeyExW(HKEY_LOCAL_MACHINE, path.as_ptr(), 0, KEY_READ, &mut key) } != 0 {
        return None;
    }
    let value = |name: &str, want: &str| {
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
        let want = wide(&[want]);
        let n = want.iter().position(|&c| c == 0).unwrap_or(0) + 1;
        e == 0 && kind == REG_SZ && size as usize == n * 2 && data[..n] == want[..n]
    };
    let ok = value("Dll", DLL) && value("FuncName", FUNCTION);
    // SAFETY: our key.
    unsafe { RegCloseKey(key) };
    Some(ok)
}

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    let dll = wide(&[DLL]);
    let function = wide(&[FUNCTION]);
    // Just in case an earlier run left it.
    // SAFETY: our GUID.
    unsafe { CryptSIPRemoveProvider(&SUBJECT) };

    // SAFETY: plain calls with NULL.
    unsafe {
        SetLastError(0xdead_beef);
        c.fails(CryptSIPRemoveProvider(null()), ERROR_INVALID_PARAMETER)?;
        SetLastError(0xdead_beef);
        c.fails(CryptSIPAddProvider(null()), ERROR_INVALID_PARAMETER)?;
        // Never registered: a registry error.
        SetLastError(0xdead_beef);
        c.fails(CryptSIPRemoveProvider(&SUBJECT), ERROR_FILE_NOT_FOUND)?;
    }

    // The five required functions; the IsMyFileType ones left NULL.
    let mut p = NewProvider {
        size: core::mem::size_of::<NewProvider>() as u32,
        subject: &SUBJECT,
        dll: dll.as_ptr(),
        magic: null(),
        is_function: null(),
        get: function.as_ptr(),
        put: function.as_ptr(),
        create: function.as_ptr(),
        verify: function.as_ptr(),
        remove: function.as_ptr(),
        is_function_fmt2: null(),
        get_cap: null(),
    };
    // SAFETY: our structure.
    let ret = unsafe { CryptSIPAddProvider(&p) };
    // SAFETY: plain call.
    c.ok(ret != 0, unsafe { GetLastError() }.into())?;
    for (i, f) in FUNCTIONS.iter().enumerate() {
        let r = registered(f);
        if i < 5 {
            c.ok(r == Some(true), i as u64)?;
        } else {
            c.ok(r.is_none(), i as u64)?;
        }
    }
    // Deleted, but the IsMyFileType keys were not there.
    // SAFETY: our GUID.
    unsafe {
        SetLastError(0xdead_beef);
        c.fails(CryptSIPRemoveProvider(&SUBJECT), ERROR_FILE_NOT_FOUND)?;
    }
    for (i, f) in FUNCTIONS.iter().enumerate() {
        c.ok(registered(f).is_none(), i as u64)?;
    }

    // Every function.
    p.is_function = function.as_ptr();
    p.is_function_fmt2 = function.as_ptr();
    // SAFETY: our structure.
    let ret = unsafe { CryptSIPAddProvider(&p) };
    // SAFETY: plain call.
    c.ok(ret != 0, unsafe { GetLastError() }.into())?;
    for (i, f) in FUNCTIONS.iter().enumerate() {
        c.ok(registered(f) == Some(true), i as u64)?;
    }
    // SAFETY: our GUID.
    unsafe {
        SetLastError(0xdead_beef);
        let ret = CryptSIPRemoveProvider(&SUBJECT);
        c.ok(ret != 0, GetLastError().into())?;
    }
    for (i, f) in FUNCTIONS.iter().enumerate() {
        c.ok(registered(f).is_none(), i as u64)?;
    }

    // A required function missing (Wine: ERROR_INVALID_PARAMETER).
    p.verify = null();
    // SAFETY: our structure.
    let (ret, e) = unsafe {
        SetLastError(0xdead_beef);
        (CryptSIPAddProvider(&p), GetLastError())
    };
    Line::new()
        .s(NAME.as_bytes())
        .s(b": info no verify function: ")
        .dec(ret as u32 as u64)
        .s(b" ")
        .dec(if ret != 0 { u64::from(ERROR_SUCCESS) } else { e.into() })
        .s(b"\r\n")
        .print();
    if ret != 0 {
        // SAFETY: our GUID.
        unsafe { CryptSIPRemoveProvider(&SUBJECT) };
    }
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
