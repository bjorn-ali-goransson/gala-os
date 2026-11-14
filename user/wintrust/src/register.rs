//! Trust provider registration: WintrustAddActionID and
//! WintrustRemoveActionID (wintrust.h). An action ID's providers are
//! registered as one registry key per provider function under
//! `HKLM\Software\Microsoft\Cryptography\Providers\Trust\<Function>\{GUID}`
//! (`Initialization`, `Message`, `Signature`, `Certificate`, `CertCheck`,
//! `FinalPolicy`, `DiagnosticPolicy`, `Cleanup`, in CRYPT_REGISTER_ACTIONID's
//! order), each with the values `$DLL` and `$Function` (REG_SZ), as
//! Windows' own providers are; the 32-bit DLL's keys land where advapi32's
//! WOW64 view puts them. A NULL action ID or structure is
//! ERROR_INVALID_PARAMETER; a provider entry without a DLL or function
//! name writes no key; a registry error fails the call with it. Removing
//! deletes every function's key and succeeds whether or not they were
//! there. gala calls no registered provider (WinVerifyTrust implements
//! WINTRUST_ACTION_GENERIC_VERIFY_V2 itself): Office's integrator
//! registers its VBA trust provider here and only the registry keeps it.
//! Each call is in the ledger as `wintrust.dll!Wintrust<Add or
//! Remove>ActionID({XXXXXXXX...})` (the GUID's first group) with its
//! error as the detail (0 on success).

use core::ffi::c_void;
use core::mem::{offset_of, size_of};

use crate::layout::Guid;

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
/// programs that load wintrust to verify signatures need no advapi32).
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
const ERROR_FILE_NOT_FOUND: u32 = 2;
const ERROR_INVALID_PARAMETER: u32 = 87;
const ERROR_PROC_NOT_FOUND: u32 = 127;

/// CRYPT_TRUST_REG_ENTRY.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct RegEntry {
    size: u32,
    dll: *const u16,
    function: *const u16,
}

/// CRYPT_REGISTER_ACTIONID: its eight providers in `KEYS`' order.
#[repr(C)]
pub struct RegisterActionId {
    size: u32,
    providers: [RegEntry; 8],
}

/// x86's value, or x64's.
const fn by_width(x86: usize, x64: usize) -> usize {
    if cfg!(target_pointer_width = "64") { x64 } else { x86 }
}
const _: () = assert!(size_of::<RegEntry>() == by_width(12, 24));
const _: () = assert!(size_of::<RegisterActionId>() == by_width(100, 200));
const _: () = assert!(offset_of!(RegisterActionId, providers) == by_width(4, 8));

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
    hex(g.0.into(), 8, &mut put);
    put(b'-');
    hex(g.1.into(), 4, &mut put);
    put(b'-');
    hex(g.2.into(), 4, &mut put);
    put(b'-');
    for (i, b) in g.3.iter().enumerate() {
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

/// One ledger line naming the call, the action (its GUID's first group:
/// the ledger keeps 64 characters) and its result.
fn note(add: bool, guid: &[u16; 38], error: u32) {
    let prefix: &[u8] = if add {
        b"wintrust.dll!WintrustAddActionID("
    } else {
        b"wintrust.dll!WintrustRemoveActionID("
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
    crate::observed(&text[..n], error.into());
}

fn fail(code: u32) -> i32 {
    // SAFETY: plain call.
    unsafe { SetLastError(code) };
    0
}

/// Writes `Trust\<key>\<guid>` with `$DLL` and `$Function`.
fn write_provider(r: &Reg, key_name: &str, guid: &[u16; 38], entry: &RegEntry) -> u32 {
    let path = Wide::new().ascii(TRUST).ascii(key_name).ascii("\\").wide(guid);
    let mut key = 0usize;
    // SAFETY: registry calls with buffers of ours and the caller's
    // NUL-terminated strings (checked non-NULL by the caller).
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
        let dll_name = Wide::new().ascii("$DLL");
        let function_name = Wide::new().ascii("$Function");
        let mut e = (r.set)(
            key,
            dll_name.buf.as_ptr(),
            0,
            REG_SZ,
            entry.dll.cast(),
            ((wlen(entry.dll) + 1) * 2) as u32,
        );
        if e == 0 {
            e = (r.set)(
                key,
                function_name.buf.as_ptr(),
                0,
                REG_SZ,
                entry.function.cast(),
                ((wlen(entry.function) + 1) * 2) as u32,
            );
        }
        (r.close)(key);
        e as u32
    }
}

/// Deletes `Trust\<key>\<guid>`.
fn delete_provider(r: &Reg, key_name: &str, guid: &[u16; 38]) -> u32 {
    let parent = Wide::new().ascii(TRUST).ascii(key_name);
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

/// WintrustAddActionID: registers the providers `info` names for action
/// `action` (above). `flags` (WT_ADD_ACTION_ID_RET_RESULT_FLAG) changes
/// nothing: the return value is always the result.
#[unsafe(no_mangle)]
pub extern "system" fn WintrustAddActionID(
    action: *const Guid,
    _flags: u32,
    info: *const RegisterActionId,
) -> i32 {
    if action.is_null() || info.is_null() {
        return fail(ERROR_INVALID_PARAMETER);
    }
    // SAFETY: checked non-NULL above; the caller's structures.
    let (guid, providers) = unsafe { (guid_text(&*action), (*info).providers) };
    let Some(r) = reg() else {
        note(true, &guid, ERROR_PROC_NOT_FOUND);
        return fail(ERROR_PROC_NOT_FOUND);
    };
    for (key_name, entry) in KEYS.iter().zip(&providers) {
        if entry.dll.is_null() || entry.function.is_null() {
            continue;
        }
        let e = write_provider(&r, key_name, &guid, entry);
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

/// WintrustRemoveActionID: deletes every provider key of action `action`
/// (above); a key that is not there is not an error.
#[unsafe(no_mangle)]
pub extern "system" fn WintrustRemoveActionID(action: *const Guid) -> i32 {
    if action.is_null() {
        return fail(ERROR_INVALID_PARAMETER);
    }
    // SAFETY: the caller's GUID.
    let guid = guid_text(unsafe { &*action });
    let Some(r) = reg() else {
        note(false, &guid, ERROR_PROC_NOT_FOUND);
        return fail(ERROR_PROC_NOT_FOUND);
    };
    let mut error = ERROR_SUCCESS;
    for key_name in KEYS {
        let e = delete_provider(&r, key_name, &guid);
        if e != ERROR_SUCCESS && e != ERROR_FILE_NOT_FOUND {
            error = e;
        }
    }
    note(false, &guid, error);
    if error != ERROR_SUCCESS {
        return fail(error);
    }
    1
}
