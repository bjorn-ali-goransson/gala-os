//! Subject interface package registration: CryptSIPAddProvider and
//! CryptSIPRemoveProvider (mssip.h). A SIP is registered as one registry
//! key per function under
//! `HKLM\Software\Microsoft\Cryptography\OID\EncodingType 0\CryptSIPDll<Function>\{GUID}`,
//! each with the values `Dll` and `FuncName` (REG_SZ), as Windows' own
//! SIPs are; the 32-bit DLL's keys land in the 32-bit view (advapi32's
//! WOW64 redirection). Behaviour follows Wine's crypt32 sip tests, which
//! pass on Windows: a NULL argument is ERROR_INVALID_PARAMETER, removing
//! deletes every function's key and fails with the last registry error
//! (ERROR_FILE_NOT_FOUND for a function never registered), so a provider
//! registered without its IsMyFileType functions is removed but the call
//! fails. gala loads no SIP (CryptSIPLoad stays a stub): Office's
//! integrator registers its VBA project SIP here and only the registry
//! keeps it. Each call is in the ledger as `crypt32.dll!CryptSIP<Add or
//! Remove>Provider({XXXXXXXX...})` (the GUID's first group) with its
//! error as the detail (0 on success).

#![allow(non_snake_case)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use core::ffi::c_void;
use core::mem::{offset_of, size_of};

use crate::layout::{check, get, has};

type RegCreate = unsafe extern "system" fn(
    usize,
    *const u16,
    u32,
    *const u16,
    u32,
    u32,
    *const c_void,
    *mut usize,
    *mut u32,
) -> i32;
type RegOpen = unsafe extern "system" fn(usize, *const u16, u32, u32, *mut usize) -> i32;
type RegSet = unsafe extern "system" fn(usize, *const u16, u32, u32, *const u8, u32) -> i32;
type RegDelete = unsafe extern "system" fn(usize, *const u16) -> i32;
type RegClose = unsafe extern "system" fn(usize) -> i32;

/// advapi32's registry functions, loaded at the first call (not imported:
/// programs that load crypt32 for certificates need no advapi32).
struct Reg {
    create: RegCreate,
    open: RegOpen,
    set: RegSet,
    delete: RegDelete,
    close: RegClose,
}

#[cfg_attr(
    target_arch = "x86",
    link(name = "kernel32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "kernel32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn LoadLibraryA(name: *const u8) -> usize;
    fn GetProcAddress(module: usize, name: *const u8) -> usize;
    fn SetLastError(code: u32);
}

fn reg() -> Option<Reg> {
    // SAFETY: loading advapi32 and resolving its documented functions with
    // the types they have.
    unsafe {
        let advapi = LoadLibraryA(c"advapi32.dll".as_ptr().cast());
        if advapi == 0 {
            return None;
        }
        let f = |name: &core::ffi::CStr| {
            let p = GetProcAddress(advapi, name.as_ptr().cast());
            (p != 0).then_some(p)
        };
        Some(Reg {
            create: core::mem::transmute::<usize, RegCreate>(f(c"RegCreateKeyExW")?),
            open: core::mem::transmute::<usize, RegOpen>(f(c"RegOpenKeyExW")?),
            set: core::mem::transmute::<usize, RegSet>(f(c"RegSetValueExW")?),
            delete: core::mem::transmute::<usize, RegDelete>(f(c"RegDeleteKeyW")?),
            close: core::mem::transmute::<usize, RegClose>(f(c"RegCloseKey")?),
        })
    }
}

const HKEY_LOCAL_MACHINE: usize = 0x8000_0002;
const KEY_ALL_ACCESS: u32 = 0xf003f;
const REG_SZ: u32 = 1;
const ERROR_SUCCESS: u32 = 0;
const ERROR_INVALID_PARAMETER: u32 = 87;
const ERROR_PROC_NOT_FOUND: u32 = 127;

/// GUID.
#[repr(C)]
pub struct Guid {
    data1: u32,
    data2: u16,
    data3: u16,
    data4: [u8; 8],
}

/// SIP_ADD_NEWPROVIDER (mssip.h; `get_cap` since Windows 8, read only
/// when `size` covers it).
#[repr(C)]
pub struct NewProvider {
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

check!(size_of::<NewProvider>(), 48, 96);
check!(offset_of!(NewProvider, get_cap), 44, 88);

/// The keys under `CryptSIPDll`, in the order they are written.
const PUT: &str = "PutSignedDataMsg";
const GET: &str = "GetSignedDataMsg";
const REMOVE: &str = "RemoveSignedDataMsg";
const CREATE: &str = "CreateIndirectData";
const VERIFY: &str = "VerifyIndirectData";
const IS_MY_FILE: &str = "IsMyFileType";
const IS_MY_FILE2: &str = "IsMyFileType2";
const FUNCTIONS: [&str; 7] = [PUT, GET, REMOVE, CREATE, VERIFY, IS_MY_FILE, IS_MY_FILE2];

const OID: &str = "Software\\Microsoft\\Cryptography\\OID\\EncodingType 0\\CryptSIPDll";

/// `{XXXXXXXX-XXXX-XXXX-XXXX-XXXXXXXXXXXX}`, upper case, as UTF-16.
fn guid_text(g: &Guid) -> [u16; 38] {
    let mut out = [0u16; 38];
    let mut at = 0;
    let mut put = |c: u8| {
        out[at] = c.into();
        at += 1;
    };
    let hex = |v: u64, digits: u32, put: &mut dyn FnMut(u8)| {
        for i in (0..digits).rev() {
            put(b"0123456789ABCDEF"[((v >> (4 * i)) & 0xf) as usize]);
        }
    };
    put(b'{');
    hex(g.data1.into(), 8, &mut put);
    put(b'-');
    hex(g.data2.into(), 4, &mut put);
    put(b'-');
    hex(g.data3.into(), 4, &mut put);
    put(b'-');
    for (i, b) in g.data4.iter().enumerate() {
        if i == 2 {
            put(b'-');
        }
        hex((*b).into(), 2, &mut put);
    }
    put(b'}');
    out
}

/// A NUL-terminated UTF-16 buffer built from parts (ASCII and UTF-16).
struct Wide {
    buf: [u16; 160],
    n: usize,
}

impl Wide {
    fn new() -> Self {
        Self { buf: [0; 160], n: 0 }
    }

    fn ascii(mut self, s: &str) -> Self {
        for b in s.bytes() {
            if self.n + 1 < self.buf.len() {
                self.buf[self.n] = b.into();
                self.n += 1;
            }
        }
        self
    }

    fn wide(mut self, s: &[u16]) -> Self {
        for &c in s {
            if self.n + 1 < self.buf.len() {
                self.buf[self.n] = c;
                self.n += 1;
            }
        }
        self
    }
}

/// The caller's NUL-terminated string's length in units.
fn wlen(s: *const u16) -> usize {
    let mut n = 0;
    // SAFETY: the caller's NUL-terminated string.
    unsafe {
        while *s.add(n) != 0 {
            n += 1;
        }
    }
    n
}

/// One ledger line naming the call, the provider (its GUID's first group:
/// the ledger keeps 64 characters) and its result.
fn note(add: bool, guid: &[u16; 38], error: u32) {
    let prefix: &[u8] = if add {
        b"crypt32.dll!CryptSIPAddProvider("
    } else {
        b"crypt32.dll!CryptSIPRemoveProvider("
    };
    let mut text = [0u8; 80];
    let mut n = 0;
    let ascii = guid[..9].iter().map(|&c| c as u8);
    for b in prefix.iter().copied().chain(ascii).chain(*b"...})") {
        if n < text.len() {
            text[n] = b;
            n += 1;
        }
    }
    crate::crl::note(&text[..n], error.into());
}

fn fail(code: u32) -> i32 {
    // SAFETY: plain call.
    unsafe { SetLastError(code) };
    0
}

/// Writes `CryptSIPDll<function>\<guid>` with `Dll` and `FuncName`.
fn write_function(
    r: &Reg,
    function: &str,
    guid: &[u16; 38],
    dll: *const u16,
    name: *const u16,
) -> u32 {
    let path = Wide::new().ascii(OID).ascii(function).ascii("\\").wide(guid);
    let mut key = 0usize;
    // SAFETY: registry calls with buffers of ours and the caller's
    // NUL-terminated strings.
    unsafe {
        let e = (r.create)(
            HKEY_LOCAL_MACHINE,
            path.buf.as_ptr(),
            0,
            core::ptr::null(),
            0,
            KEY_ALL_ACCESS,
            core::ptr::null(),
            &mut key,
            core::ptr::null_mut(),
        );
        if e != 0 {
            return e as u32;
        }
        let func_name = Wide::new().ascii("FuncName");
        let dll_name = Wide::new().ascii("Dll");
        let mut e = (r.set)(
            key,
            func_name.buf.as_ptr(),
            0,
            REG_SZ,
            name.cast(),
            ((wlen(name) + 1) * 2) as u32,
        );
        if e == 0 {
            e = (r.set)(
                key,
                dll_name.buf.as_ptr(),
                0,
                REG_SZ,
                dll.cast(),
                ((wlen(dll) + 1) * 2) as u32,
            );
        }
        (r.close)(key);
        e as u32
    }
}

/// Deletes `CryptSIPDll<function>\<guid>`.
fn delete_function(r: &Reg, function: &str, guid: &[u16; 38]) -> u32 {
    let parent = Wide::new().ascii(OID).ascii(function);
    let mut key = 0usize;
    // SAFETY: registry calls with buffers of ours.
    unsafe {
        let e = (r.open)(HKEY_LOCAL_MACHINE, parent.buf.as_ptr(), 0, KEY_ALL_ACCESS, &mut key);
        if e != 0 {
            return e as u32;
        }
        let name = Wide::new().wide(guid);
        let e = (r.delete)(key, name.buf.as_ptr());
        (r.close)(key);
        e as u32
    }
}

/// CryptSIPAddProvider: registers the SIP `p` describes (above).
#[unsafe(no_mangle)]
pub extern "system" fn CryptSIPAddProvider(p: *const NewProvider) -> i32 {
    // The structure up to pwszIsFunctionNameFmt2 (Windows 7's size).
    const OLD_SIZE: usize = offset_of!(NewProvider, get_cap);
    if p.is_null() {
        return fail(ERROR_INVALID_PARAMETER);
    }
    let p = p.cast::<u8>();
    let size = get!(p, NewProvider, size);
    if (size as usize) < OLD_SIZE {
        return fail(ERROR_INVALID_PARAMETER);
    }
    let subject = get!(p, NewProvider, subject);
    let dll = get!(p, NewProvider, dll);
    let functions = [
        (PUT, get!(p, NewProvider, put)),
        (GET, get!(p, NewProvider, get)),
        (REMOVE, get!(p, NewProvider, remove)),
        (CREATE, get!(p, NewProvider, create)),
        (VERIFY, get!(p, NewProvider, verify)),
        (IS_MY_FILE, get!(p, NewProvider, is_function)),
        (IS_MY_FILE2, get!(p, NewProvider, is_function_fmt2)),
    ];
    // The five functions every SIP has.
    if subject.is_null() || dll.is_null() || functions[..5].iter().any(|f| f.1.is_null()) {
        return fail(ERROR_INVALID_PARAMETER);
    }
    // SAFETY: checked non-NULL above.
    let guid = guid_text(unsafe { &*subject });
    if has!(size, NewProvider, get_cap) && !get!(p, NewProvider, get_cap).is_null() {
        // Windows 8's GetCaps function: no key gala knows to write.
        super::missing_detail(b"crypt32.dll!CryptSIPAddProvider(pwszGetCapFuncName)", 0);
    }
    let Some(r) = reg() else {
        note(true, &guid, ERROR_PROC_NOT_FOUND);
        return fail(ERROR_PROC_NOT_FOUND);
    };
    for (function, name) in functions {
        if name.is_null() {
            continue;
        }
        let e = write_function(&r, function, &guid, dll, name);
        if e != ERROR_SUCCESS {
            note(true, &guid, e);
            return fail(e);
        }
    }
    note(true, &guid, ERROR_SUCCESS);
    // SAFETY: plain call.
    unsafe { SetLastError(ERROR_SUCCESS) };
    1
}

/// CryptSIPRemoveProvider: deletes every function's key of SIP `subject`;
/// fails with the last error if any key could not be deleted (above).
#[unsafe(no_mangle)]
pub extern "system" fn CryptSIPRemoveProvider(subject: *const Guid) -> i32 {
    if subject.is_null() {
        return fail(ERROR_INVALID_PARAMETER);
    }
    // SAFETY: the caller's GUID.
    let guid = guid_text(unsafe { &*subject });
    let Some(r) = reg() else {
        note(false, &guid, ERROR_PROC_NOT_FOUND);
        return fail(ERROR_PROC_NOT_FOUND);
    };
    let mut error = ERROR_SUCCESS;
    for function in FUNCTIONS {
        let e = delete_function(&r, function, &guid);
        if e != ERROR_SUCCESS {
            error = e;
        }
    }
    note(false, &guid, error);
    if error != ERROR_SUCCESS {
        return fail(error);
    }
    1
}
