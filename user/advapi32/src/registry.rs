//! The registry functions (Reg*), over ntdll's Nt*Key services.
//!
//! The predefined keys open, on first use, the key they stand for:
//! HKEY_LOCAL_MACHINE `\Registry\Machine`, HKEY_USERS `\Registry\User`,
//! HKEY_CURRENT_USER `\Registry\User\<the user's SID>`, HKEY_CLASSES_ROOT
//! `\Registry\Machine\SOFTWARE\Classes` (Windows merges the user's
//! classes over it; gala has no per-user classes yet), HKEY_CURRENT_CONFIG
//! the current hardware profile. Their handles are kept for the process
//! (RegCloseKey of a predefined key does nothing). Any other HKEY is the
//! NT key handle itself.
//!
//! The functions return Win32 error codes (they do not set the last-error
//! value), mapped from the NTSTATUS by RtlNtStatusToDosError. The A
//! functions convert names and string data (REG_SZ, REG_EXPAND_SZ,
//! REG_MULTI_SZ) between the ANSI code page and UTF-16.

use core::ffi::c_void;
use core::sync::atomic::{AtomicUsize, Ordering};

use super::missing;

#[repr(C)]
struct UnicodeString {
    length: u16,
    maximum: u16,
    buffer: *const u16,
}

#[repr(C)]
struct ObjectAttributes {
    length: u32,
    root: usize,
    name: *const UnicodeString,
    attributes: u32,
    security: usize,
    qos: usize,
}

#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "system" {
    fn NtOpenKey(out: *mut usize, access: u32, attributes: *const ObjectAttributes) -> u32;
    fn NtCreateKey(
        out: *mut usize,
        access: u32,
        attributes: *const ObjectAttributes,
        title: u32,
        class: *const UnicodeString,
        options: u32,
        disposition: *mut u32,
    ) -> u32;
    fn NtQueryKey(key: usize, class: u32, buf: *mut u8, len: u32, ret: *mut u32) -> u32;
    fn NtEnumerateKey(
        key: usize,
        index: u32,
        class: u32,
        buf: *mut u8,
        len: u32,
        ret: *mut u32,
    ) -> u32;
    fn NtQueryValueKey(
        key: usize,
        name: *const UnicodeString,
        class: u32,
        buf: *mut u8,
        len: u32,
        ret: *mut u32,
    ) -> u32;
    fn NtEnumerateValueKey(
        key: usize,
        index: u32,
        class: u32,
        buf: *mut u8,
        len: u32,
        ret: *mut u32,
    ) -> u32;
    fn NtSetValueKey(
        key: usize,
        name: *const UnicodeString,
        title: u32,
        kind: u32,
        data: *const u8,
        size: u32,
    ) -> u32;
    fn NtDeleteKey(key: usize) -> u32;
    fn NtDeleteValueKey(key: usize, name: *const UnicodeString) -> u32;
    fn NtClose(h: usize) -> u32;
    fn RtlNtStatusToDosError(status: u32) -> u32;
}

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn LocalAlloc(flags: u32, bytes: usize) -> *mut u8;
    fn LocalFree(memory: *mut c_void) -> *mut c_void;
    fn MultiByteToWideChar(
        cp: u32,
        flags: u32,
        src: *const u8,
        len: i32,
        dst: *mut u16,
        cap: i32,
    ) -> i32;
    fn WideCharToMultiByte(
        cp: u32,
        flags: u32,
        src: *const u16,
        len: i32,
        dst: *mut u8,
        cap: i32,
        default: *const u8,
        used: *mut i32,
    ) -> i32;
    fn ExpandEnvironmentStringsW(src: *const u16, dst: *mut u16, cap: u32) -> u32;
}

const ERROR_SUCCESS: i32 = 0;
const ERROR_FILE_NOT_FOUND: i32 = 2;
const ERROR_NOT_ENOUGH_MEMORY: i32 = 8;
const ERROR_INVALID_HANDLE: i32 = 6;
const ERROR_INVALID_PARAMETER: i32 = 87;
const ERROR_MORE_DATA: i32 = 234;
const ERROR_NO_MORE_ITEMS: i32 = 259;
const ERROR_NOACCESS: i32 = 998;
const ERROR_DATATYPE_MISMATCH: i32 = 1629;
const ERROR_UNSUPPORTED_TYPE: i32 = 1630;

const STATUS_BUFFER_OVERFLOW: u32 = 0x8000_0005;
const STATUS_BUFFER_TOO_SMALL: u32 = 0xc000_0023;

const MAXIMUM_ALLOWED: u32 = 0x0200_0000;
const DELETE: u32 = 0x0001_0000;
const KEY_QUERY_VALUE: u32 = 0x0001;
const KEY_SET_VALUE: u32 = 0x0002;
const KEY_ENUMERATE_SUB_KEYS: u32 = 0x0008;
const OBJ_CASE_INSENSITIVE: u32 = 0x40;
const REG_OPENED_EXISTING_KEY: u32 = 2;

const REG_NONE: u32 = 0;
const REG_SZ: u32 = 1;
const REG_EXPAND_SZ: u32 = 2;
const REG_BINARY: u32 = 3;
const REG_DWORD: u32 = 4;
const REG_MULTI_SZ: u32 = 7;
const REG_QWORD: u32 = 11;

const RRF_RT_REG_NONE: u32 = 0x1;
const RRF_RT_REG_SZ: u32 = 0x2;
const RRF_RT_REG_EXPAND_SZ: u32 = 0x4;
const RRF_RT_REG_BINARY: u32 = 0x8;
const RRF_RT_REG_DWORD: u32 = 0x10;
const RRF_RT_REG_MULTI_SZ: u32 = 0x20;
const RRF_RT_REG_QWORD: u32 = 0x40;
const RRF_RT_ANY: u32 = 0xffff;
const RRF_NOEXPAND: u32 = 0x1000_0000;
const RRF_ZEROONFAILURE: u32 = 0x2000_0000;

const CP_ACP: u32 = 0;
const BACKSLASH: u16 = b'\\' as u16;

/// Win32 error code of an NTSTATUS.
fn error(status: u32) -> i32 {
    if status == 0 {
        return ERROR_SUCCESS;
    }
    // SAFETY: plain call.
    unsafe { RtlNtStatusToDosError(status) as i32 }
}

// ---- Memory and strings ----

/// A LocalAlloc'd buffer, freed when dropped.
struct Mem {
    ptr: *mut u8,
    len: usize,
}

impl Mem {
    fn new(len: usize) -> Result<Mem, i32> {
        // SAFETY: plain call; LMEM_ZEROINIT.
        let ptr = unsafe { LocalAlloc(0x40, len.max(1)) };
        if ptr.is_null() { Err(ERROR_NOT_ENOUGH_MEMORY) } else { Ok(Mem { ptr, len }) }
    }

    fn bytes(&self) -> &[u8] {
        // SAFETY: our allocation of `len` bytes.
        unsafe { core::slice::from_raw_parts(self.ptr, self.len) }
    }

    fn u32_at(&self, off: usize) -> u32 {
        let b = self.bytes();
        u32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
    }

    fn u64_at(&self, off: usize) -> u64 {
        u64::from(self.u32_at(off)) | u64::from(self.u32_at(off + 4)) << 32
    }

    /// UTF-16 units at `off`, `bytes` long.
    fn wide(&self, off: usize, bytes: usize) -> &[u16] {
        let end = (off + bytes).min(self.len);
        let n = end.saturating_sub(off) / 2;
        // SAFETY: inside our allocation; LocalAlloc memory is 8-aligned
        // and the offsets used are even.
        unsafe { core::slice::from_raw_parts(self.ptr.add(off).cast::<u16>(), n) }
    }
}

impl Drop for Mem {
    fn drop(&mut self) {
        // SAFETY: our allocation.
        unsafe { LocalFree(self.ptr.cast()) };
    }
}

/// UTF-16 text owned in local memory.
struct WStr {
    mem: Mem,
    len: usize,
}

impl WStr {
    fn units(&self) -> &[u16] {
        self.mem.wide(0, 2 * self.len)
    }
}

/// The caller's NUL-terminated UTF-16 string (empty if null).
fn wide_arg<'a>(p: *const u16) -> &'a [u16] {
    if p.is_null() {
        return &[];
    }
    let mut n = 0;
    // SAFETY: the caller's NUL-terminated string.
    unsafe {
        while *p.add(n) != 0 {
            n += 1;
        }
        core::slice::from_raw_parts(p, n)
    }
}

/// The caller's ANSI string (`len` bytes, or NUL-terminated if None) in
/// UTF-16; None for a null pointer.
fn ansi_to_wide(p: *const u8, len: Option<usize>) -> Result<Option<WStr>, i32> {
    if p.is_null() {
        return Ok(None);
    }
    let len = match len {
        Some(n) => n,
        None => {
            let mut n = 0;
            // SAFETY: the caller's NUL-terminated string.
            while unsafe { *p.add(n) } != 0 {
                n += 1;
            }
            n
        }
    };
    let mem = Mem::new(2 * len + 2)?;
    let n = if len == 0 {
        0
    } else {
        // SAFETY: the caller's bytes and our buffer of len + 1 units.
        unsafe { MultiByteToWideChar(CP_ACP, 0, p, len as i32, mem.ptr.cast(), len as i32 + 1) }
    };
    Ok(Some(WStr { mem, len: n.max(0) as usize }))
}

/// `w` in the ANSI code page into `out` (`cap` bytes); returns the bytes
/// needed (which may exceed `cap`, then nothing is written).
fn wide_to_ansi(w: &[u16], out: *mut u8, cap: usize) -> usize {
    if w.is_empty() {
        return 0;
    }
    let (null, nil) = (core::ptr::null_mut(), core::ptr::null_mut());
    // SAFETY: our UTF-16 and the caller's buffer of `cap` bytes.
    unsafe {
        let need =
            WideCharToMultiByte(CP_ACP, 0, w.as_ptr(), w.len() as i32, null, 0, null, nil) as usize;
        if !out.is_null() && need <= cap {
            WideCharToMultiByte(CP_ACP, 0, w.as_ptr(), w.len() as i32, out, cap as i32, null, nil);
        }
        need
    }
}

fn unicode(s: &[u16]) -> UnicodeString {
    let bytes = (2 * s.len()).min(0xfffe) as u16;
    UnicodeString { length: bytes, maximum: bytes, buffer: s.as_ptr() }
}

fn is_string(kind: u32) -> bool {
    matches!(kind, REG_SZ | REG_EXPAND_SZ | REG_MULTI_SZ)
}

/// Stores `value` through `p` if it is not null.
fn put<T>(p: *mut T, value: T) {
    if !p.is_null() {
        // SAFETY: the caller's output.
        unsafe { p.write_unaligned(value) };
    }
}

fn get(p: *const u32) -> u32 {
    // SAFETY: the caller's input (checked for null by the caller).
    if p.is_null() { 0 } else { unsafe { p.read_unaligned() } }
}

// ---- Keys ----

const HKEY_CLASSES_ROOT: usize = 0x8000_0000;
const HKEY_CURRENT_USER: usize = 0x8000_0001;
const HKEY_PERFORMANCE_DATA: usize = 0x8000_0004;

/// The predefined keys' NT handles, opened on first use.
static PREDEFINED: [AtomicUsize; 8] = [const { AtomicUsize::new(0) }; 8];

/// The index of a predefined key (0 for HKEY_CLASSES_ROOT ...), which
/// programs pass zero- or sign-extended (winreg.h's `(HKEY)(ULONG_PTR)
/// ((LONG)0x80000000)` is 0xFFFFFFFF80000000 on x64).
fn predefined(key: usize) -> Option<usize> {
    let high = key >> 32;
    let low = key & 0xffff_ffff;
    let extended = high == 0 || high == 0xffff_ffff;
    (extended && (HKEY_CLASSES_ROOT..=HKEY_CLASSES_ROOT + 7).contains(&low))
        .then(|| low - HKEY_CLASSES_ROOT)
}

/// `\Registry\User\S-1-5-21-...` of the current user into `out`.
fn current_user_path(out: &mut [u16; 128]) -> usize {
    let mut n = 0;
    let mut push = |c: u8, n: &mut usize| {
        out[*n] = c.into();
        *n += 1;
    };
    for &c in b"\\Registry\\User\\S-1-5" {
        push(c, &mut n);
    }
    for sub in super::security::USER_SUBS {
        push(b'-', &mut n);
        let mut digits = [0u8; 10];
        let mut v = sub;
        let mut d = 0;
        loop {
            digits[d] = b'0' + (v % 10) as u8;
            d += 1;
            v /= 10;
            if v == 0 {
                break;
            }
        }
        for &c in digits[..d].iter().rev() {
            push(c, &mut n);
        }
    }
    n
}

/// Opens `name` below `root` (0: an absolute name).
fn nt_open(root: usize, name: &[u16], access: u32) -> Result<usize, i32> {
    let name = unicode(name);
    let attributes = ObjectAttributes {
        length: size_of::<ObjectAttributes>() as u32,
        root,
        name: &name,
        attributes: OBJ_CASE_INSENSITIVE,
        security: 0,
        qos: 0,
    };
    let mut h = 0;
    // SAFETY: valid attributes and output.
    match unsafe { NtOpenKey(&mut h, access, &attributes) } {
        0 => Ok(h),
        status => Err(error(status)),
    }
}

/// The NT handle an HKEY stands for.
fn resolve(key: usize) -> Result<usize, i32> {
    let Some(index) = predefined(key) else {
        return if key == 0 { Err(ERROR_INVALID_HANDLE) } else { Ok(key) };
    };
    let cached = PREDEFINED[index].load(Ordering::Acquire);
    if cached != 0 {
        return Ok(cached);
    }
    let mut path = [0u16; 128];
    let n = match index {
        0 => copy_ascii(&mut path, "\\Registry\\Machine\\SOFTWARE\\Classes"),
        1 => current_user_path(&mut path),
        2 => copy_ascii(&mut path, "\\Registry\\Machine"),
        3 => copy_ascii(&mut path, "\\Registry\\User"),
        5 => copy_ascii(
            &mut path,
            "\\Registry\\Machine\\SYSTEM\\CurrentControlSet\\Hardware Profiles\\Current",
        ),
        7 => {
            let n = current_user_path(&mut path);
            n + copy_ascii(&mut path[n..], "\\Software\\Classes\\Local Settings")
        }
        _ => {
            // HKEY_PERFORMANCE_DATA and its relatives, HKEY_DYN_DATA.
            missing(b"advapi32.dll!(HKEY_PERFORMANCE_DATA)");
            return Err(ERROR_INVALID_HANDLE);
        }
    };
    let h = nt_open(0, &path[..n], MAXIMUM_ALLOWED)?;
    match PREDEFINED[index].compare_exchange(0, h, Ordering::AcqRel, Ordering::Acquire) {
        Ok(_) => Ok(h),
        Err(other) => {
            // Another thread got there first.
            // SAFETY: our handle.
            unsafe { NtClose(h) };
            Ok(other)
        }
    }
}

fn copy_ascii(out: &mut [u16], s: &str) -> usize {
    for (d, c) in out.iter_mut().zip(s.bytes()) {
        *d = c.into();
    }
    s.len()
}

/// A key opened for the length of a call, closed when dropped (unless it
/// is the caller's own handle).
struct Opened {
    h: usize,
    owned: bool,
}

impl Drop for Opened {
    fn drop(&mut self) {
        if self.owned {
            // SAFETY: our handle.
            unsafe { NtClose(self.h) };
        }
    }
}

/// `key` itself (no subkey), or its subkey `sub` opened with `access`.
fn subkey(key: usize, sub: &[u16], access: u32) -> Result<Opened, i32> {
    let base = resolve(key)?;
    if sub.is_empty() {
        return Ok(Opened { h: base, owned: false });
    }
    Ok(Opened { h: nt_open(base, sub, access)?, owned: true })
}

fn open_w(key: usize, sub: &[u16], access: u32, out: *mut usize) -> i32 {
    if out.is_null() {
        return ERROR_INVALID_PARAMETER;
    }
    put(out, 0);
    // No subkey: a new handle to the same key.
    match resolve(key).and_then(|base| nt_open(base, sub, access)) {
        Ok(h) => {
            put(out, h);
            ERROR_SUCCESS
        }
        Err(e) => e,
    }
}

/// RegOpenKeyExW(hKey, lpSubKey, ulOptions, samDesired, phkResult)
#[unsafe(no_mangle)]
pub extern "system" fn RegOpenKeyExW(
    key: usize,
    sub: *const u16,
    _options: u32,
    access: u32,
    out: *mut usize,
) -> i32 {
    open_w(key, wide_arg(sub), access, out)
}

#[unsafe(no_mangle)]
pub extern "system" fn RegOpenKeyExA(
    key: usize,
    sub: *const u8,
    _options: u32,
    access: u32,
    out: *mut usize,
) -> i32 {
    match ansi_to_wide(sub, None) {
        Ok(w) => open_w(key, w.as_ref().map_or(&[], WStr::units), access, out),
        Err(e) => e,
    }
}

/// RegOpenKeyW(hKey, lpSubKey, phkResult): no subkey returns hKey itself.
#[unsafe(no_mangle)]
pub extern "system" fn RegOpenKeyW(key: usize, sub: *const u16, out: *mut usize) -> i32 {
    let sub = wide_arg(sub);
    if sub.is_empty() {
        put(out, key);
        return ERROR_SUCCESS;
    }
    open_w(key, sub, MAXIMUM_ALLOWED, out)
}

#[unsafe(no_mangle)]
pub extern "system" fn RegOpenKeyA(key: usize, sub: *const u8, out: *mut usize) -> i32 {
    match ansi_to_wide(sub, None) {
        Ok(Some(w)) if w.len > 0 => open_w(key, w.units(), MAXIMUM_ALLOWED, out),
        Ok(_) => {
            put(out, key);
            ERROR_SUCCESS
        }
        Err(e) => e,
    }
}

/// RegOpenCurrentUser(samDesired, phkResult): the current user's key.
#[unsafe(no_mangle)]
pub extern "system" fn RegOpenCurrentUser(access: u32, out: *mut usize) -> i32 {
    open_w(HKEY_CURRENT_USER, &[], access, out)
}

/// RegCloseKey: predefined keys stay open.
#[unsafe(no_mangle)]
pub extern "system" fn RegCloseKey(key: usize) -> i32 {
    if predefined(key).is_some() {
        return ERROR_SUCCESS;
    }
    // SAFETY: plain call.
    error(unsafe { NtClose(key) })
}

/// Creates (or opens) one component below `root`.
fn create_one(
    root: usize,
    name: &[u16],
    class: &[u16],
    options: u32,
    access: u32,
) -> Result<(usize, u32), i32> {
    let name = unicode(name);
    let class = unicode(class);
    let attributes = ObjectAttributes {
        length: size_of::<ObjectAttributes>() as u32,
        root,
        name: &name,
        attributes: OBJ_CASE_INSENSITIVE,
        security: 0,
        qos: 0,
    };
    let (mut h, mut disposition) = (0, 0);
    // SAFETY: valid attributes and outputs.
    let status =
        unsafe { NtCreateKey(&mut h, access, &attributes, 0, &class, options, &mut disposition) };
    if status == 0 { Ok((h, disposition)) } else { Err(error(status)) }
}

/// RegCreateKeyEx: creates the missing keys along `sub`, each with
/// `options`.
fn create_w(
    key: usize,
    sub: &[u16],
    class: &[u16],
    options: u32,
    access: u32,
    out: *mut usize,
    disposition: *mut u32,
) -> i32 {
    if out.is_null() {
        return ERROR_INVALID_PARAMETER;
    }
    let base = match resolve(key) {
        Ok(b) => b,
        Err(e) => return e,
    };
    // No subkey: the key itself, as a new handle.
    if sub.iter().all(|&c| c == BACKSLASH) {
        return match nt_open(base, &[], access) {
            Ok(h) => {
                put(out, h);
                put(disposition, REG_OPENED_EXISTING_KEY);
                ERROR_SUCCESS
            }
            Err(e) => e,
        };
    }
    // Usually the parent exists: one call.
    match create_one(base, sub, class, options, access) {
        Ok((h, d)) => {
            put(out, h);
            put(disposition, d);
            return ERROR_SUCCESS;
        }
        Err(ERROR_FILE_NOT_FOUND) => {}
        Err(e) => return e,
    }
    // Otherwise component by component.
    let parts = sub.split(|&c| c == BACKSLASH).filter(|p| !p.is_empty());
    let count = parts.clone().count();
    let mut at = Opened { h: base, owned: false };
    for (i, part) in parts.enumerate() {
        let last = i + 1 == count;
        let want = if last { access } else { MAXIMUM_ALLOWED };
        match create_one(at.h, part, if last { class } else { &[] }, options, want) {
            Ok((h, d)) if last => {
                put(out, h);
                put(disposition, d);
                return ERROR_SUCCESS;
            }
            Ok((h, _)) => at = Opened { h, owned: true },
            Err(e) => return e,
        }
    }
    ERROR_INVALID_PARAMETER
}

#[allow(clippy::too_many_arguments)]
#[unsafe(no_mangle)]
pub extern "system" fn RegCreateKeyExW(
    key: usize,
    sub: *const u16,
    _reserved: u32,
    class: *const u16,
    options: u32,
    access: u32,
    _security: *mut c_void,
    out: *mut usize,
    disposition: *mut u32,
) -> i32 {
    create_w(key, wide_arg(sub), wide_arg(class), options, access, out, disposition)
}

#[allow(clippy::too_many_arguments)]
#[unsafe(no_mangle)]
pub extern "system" fn RegCreateKeyExA(
    key: usize,
    sub: *const u8,
    _reserved: u32,
    class: *const u8,
    options: u32,
    access: u32,
    _security: *mut c_void,
    out: *mut usize,
    disposition: *mut u32,
) -> i32 {
    let (sub, class) = match (ansi_to_wide(sub, None), ansi_to_wide(class, None)) {
        (Ok(s), Ok(c)) => (s, c),
        (Err(e), _) | (_, Err(e)) => return e,
    };
    let sub = sub.as_ref().map_or(&[][..], WStr::units);
    let class = class.as_ref().map_or(&[][..], WStr::units);
    create_w(key, sub, class, options, access, out, disposition)
}

#[unsafe(no_mangle)]
pub extern "system" fn RegCreateKeyW(key: usize, sub: *const u16, out: *mut usize) -> i32 {
    create_w(key, wide_arg(sub), &[], 0, MAXIMUM_ALLOWED, out, core::ptr::null_mut())
}

#[unsafe(no_mangle)]
pub extern "system" fn RegCreateKeyA(key: usize, sub: *const u8, out: *mut usize) -> i32 {
    let null = core::ptr::null_mut();
    RegCreateKeyExA(key, sub, 0, core::ptr::null(), 0, MAXIMUM_ALLOWED, null, out, null.cast())
}

// ---- Values ----

/// A value's type and data (KEY_VALUE_PARTIAL_INFORMATION: TitleIndex,
/// Type, DataLength, Data at 12).
fn read_value(h: usize, name: &[u16]) -> Result<(u32, Mem), i32> {
    let name = unicode(name);
    let mut len = 64u32;
    loop {
        let mem = Mem::new(len as usize)?;
        let mut ret = 0;
        // SAFETY: our buffer of `len` bytes.
        let status = unsafe { NtQueryValueKey(h, &name, 2, mem.ptr, len, &mut ret) };
        match status {
            0 => return Ok((mem.u32_at(4), mem)),
            STATUS_BUFFER_OVERFLOW | STATUS_BUFFER_TOO_SMALL if ret > len => len = ret,
            s => return Err(error(s)),
        }
    }
}

/// The data part of `read_value`'s buffer.
fn value_data(mem: &Mem) -> &[u8] {
    let n = mem.u32_at(8) as usize;
    &mem.bytes()[12..12 + n]
}

/// Copies `src` to the caller's `data`/`size` with RegQueryValueEx's
/// rules: a null `data` asks for the size; too small is ERROR_MORE_DATA.
fn give(src: &[u8], data: *mut u8, size: *mut u32) -> i32 {
    let cap = get(size) as usize;
    put(size, src.len() as u32);
    if data.is_null() {
        return ERROR_SUCCESS;
    }
    if cap < src.len() {
        return ERROR_MORE_DATA;
    }
    // SAFETY: the caller's buffer of `cap` >= src.len() bytes.
    unsafe { core::ptr::copy_nonoverlapping(src.as_ptr(), data, src.len()) };
    ERROR_SUCCESS
}

fn query_w(key: usize, name: &[u16], kind: *mut u32, data: *mut u8, size: *mut u32) -> i32 {
    if !data.is_null() && size.is_null() {
        return ERROR_INVALID_PARAMETER;
    }
    if predefined(key) == predefined(HKEY_PERFORMANCE_DATA) {
        missing(b"advapi32.dll!RegQueryValueEx(HKEY_PERFORMANCE_DATA)");
        return ERROR_FILE_NOT_FOUND;
    }
    let h = match resolve(key) {
        Ok(h) => h,
        Err(e) => return e,
    };
    match read_value(h, name) {
        Ok((k, mem)) => {
            put(kind, k);
            give(value_data(&mem), data, size)
        }
        Err(e) => e,
    }
}

/// RegQueryValueExW(hKey, lpValueName, lpReserved, lpType, lpData, lpcbData)
#[unsafe(no_mangle)]
pub extern "system" fn RegQueryValueExW(
    key: usize,
    name: *const u16,
    _reserved: *mut u32,
    kind: *mut u32,
    data: *mut u8,
    size: *mut u32,
) -> i32 {
    query_w(key, wide_arg(name), kind, data, size)
}

/// RegQueryValueExA: string data in the ANSI code page.
#[unsafe(no_mangle)]
pub extern "system" fn RegQueryValueExA(
    key: usize,
    name: *const u8,
    _reserved: *mut u32,
    kind: *mut u32,
    data: *mut u8,
    size: *mut u32,
) -> i32 {
    if !data.is_null() && size.is_null() {
        return ERROR_INVALID_PARAMETER;
    }
    let name = match ansi_to_wide(name, None) {
        Ok(n) => n,
        Err(e) => return e,
    };
    let h = match resolve(key) {
        Ok(h) => h,
        Err(e) => return e,
    };
    let (k, mem) = match read_value(h, name.as_ref().map_or(&[], WStr::units)) {
        Ok(v) => v,
        Err(e) => return e,
    };
    put(kind, k);
    give_a(k, value_data(&mem), data, size)
}

/// `give`, converting string data to ANSI.
fn give_a(kind: u32, src: &[u8], data: *mut u8, size: *mut u32) -> i32 {
    if !is_string(kind) {
        return give(src, data, size);
    }
    let w = match units_of(src) {
        Ok(w) => w,
        Err(e) => return e,
    };
    let cap = get(size) as usize;
    let need = wide_to_ansi(w.units(), core::ptr::null_mut(), 0);
    put(size, need as u32);
    if data.is_null() {
        return ERROR_SUCCESS;
    }
    if cap < need {
        return ERROR_MORE_DATA;
    }
    wide_to_ansi(w.units(), data, cap);
    ERROR_SUCCESS
}

/// Bytes as UTF-16 units (in local memory, for alignment).
fn units_of(src: &[u8]) -> Result<WStr, i32> {
    let n = src.len() / 2;
    let mem = Mem::new(2 * n)?;
    // SAFETY: our buffer of 2n bytes.
    unsafe { core::ptr::copy_nonoverlapping(src.as_ptr(), mem.ptr, 2 * n) };
    Ok(WStr { mem, len: n })
}

/// The RRF_RT_* bit of a value type.
fn type_flag(kind: u32) -> u32 {
    match kind {
        REG_NONE => RRF_RT_REG_NONE,
        REG_SZ => RRF_RT_REG_SZ,
        REG_EXPAND_SZ => RRF_RT_REG_EXPAND_SZ,
        REG_BINARY => RRF_RT_REG_BINARY,
        REG_DWORD => RRF_RT_REG_DWORD,
        REG_MULTI_SZ => RRF_RT_REG_MULTI_SZ,
        REG_QWORD => RRF_RT_REG_QWORD,
        _ => RRF_RT_ANY,
    }
}

/// Expands environment variables in `src` (UTF-16 bytes, NUL-terminated
/// or not) into new local memory, as UTF-16 bytes with the terminator.
fn expand(src: &[u8]) -> Result<(Mem, usize), i32> {
    let n = src.len() / 2;
    let input = Mem::new(2 * n + 2)?;
    // SAFETY: our buffer of 2n + 2 bytes (zeroed: the terminator).
    unsafe { core::ptr::copy_nonoverlapping(src.as_ptr(), input.ptr, 2 * n) };
    let mut cap = (n as u32 + 1).max(64);
    loop {
        let out = Mem::new(2 * cap as usize)?;
        // SAFETY: our NUL-terminated input and output of `cap` units.
        let need = unsafe { ExpandEnvironmentStringsW(input.ptr.cast(), out.ptr.cast(), cap) };
        if need == 0 {
            return Err(ERROR_INVALID_PARAMETER);
        }
        if need <= cap {
            return Ok((out, 2 * need as usize));
        }
        cap = need;
    }
}

/// RegGetValueW(hkey, lpSubKey, lpValue, dwFlags, pdwType, pvData, pcbData):
/// RegQueryValueEx with a type filter, REG_EXPAND_SZ expanded (as REG_SZ)
/// unless RRF_NOEXPAND, and strings always NUL-terminated.
#[unsafe(no_mangle)]
pub extern "system" fn RegGetValueW(
    key: usize,
    sub: *const u16,
    value: *const u16,
    flags: u32,
    kind: *mut u32,
    data: *mut c_void,
    size: *mut u32,
) -> i32 {
    get_value_w(key, wide_arg(sub), wide_arg(value), flags, kind, data.cast(), size, false)
}

#[allow(clippy::too_many_arguments)]
fn get_value_w(
    key: usize,
    sub: &[u16],
    value: &[u16],
    flags: u32,
    kind: *mut u32,
    data: *mut u8,
    size: *mut u32,
    ansi: bool,
) -> i32 {
    if flags & RRF_RT_ANY == 0
        || (!data.is_null() && size.is_null())
        || (flags & RRF_RT_REG_EXPAND_SZ != 0 && flags & RRF_NOEXPAND == 0)
    {
        return ERROR_INVALID_PARAMETER;
    }
    let cap = get(size) as usize;
    let zero = |r: i32| {
        if r != ERROR_SUCCESS && r != ERROR_MORE_DATA || flags & RRF_ZEROONFAILURE == 0 {
            return r;
        }
        if !data.is_null() {
            // SAFETY: the caller's buffer of `cap` bytes.
            unsafe { core::ptr::write_bytes(data, 0, cap) };
        }
        r
    };
    if !data.is_null() && flags & RRF_ZEROONFAILURE != 0 {
        // Zeroed up front too; overwritten on success.
        // SAFETY: the caller's buffer of `cap` bytes.
        unsafe { core::ptr::write_bytes(data, 0, cap) };
    }
    let opened = match subkey(key, sub, KEY_QUERY_VALUE) {
        Ok(o) => o,
        Err(e) => return e,
    };
    let (mut k, mem) = match read_value(opened.h, value) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let raw = value_data(&mem);
    let expanding = k == REG_EXPAND_SZ && flags & RRF_NOEXPAND == 0;
    let filter = if expanding { RRF_RT_REG_SZ } else { type_flag(k) };
    let binary_as = k == REG_BINARY
        && ((flags & RRF_RT_REG_DWORD != 0 && raw.len() == 4)
            || (flags & RRF_RT_REG_QWORD != 0 && raw.len() == 8));
    if flags & filter == 0 && !binary_as {
        put(kind, 0);
        return zero(ERROR_UNSUPPORTED_TYPE);
    }
    if (k == REG_DWORD && raw.len() != 4) || (k == REG_QWORD && raw.len() != 8) {
        return zero(ERROR_DATATYPE_MISMATCH);
    }
    // Strings: expanded, and terminated if they were not.
    let expanded;
    let mut terminated: Mem;
    let bytes: &[u8] = if expanding {
        k = REG_SZ;
        expanded = match expand(raw) {
            Ok(e) => e,
            Err(e) => return e,
        };
        &expanded.0.bytes()[..expanded.1]
    } else if is_string(k) && !raw.ends_with(&[0, 0]) {
        terminated = match Mem::new(raw.len() + 2) {
            Ok(m) => m,
            Err(e) => return e,
        };
        // SAFETY: our buffer of raw.len() + 2 (zeroed) bytes.
        unsafe { core::ptr::copy_nonoverlapping(raw.as_ptr(), terminated.ptr, raw.len()) };
        terminated.len = (raw.len() & !1) + 2;
        terminated.bytes()
    } else {
        raw
    };
    put(kind, k);
    let r = if ansi { give_a(k, bytes, data, size) } else { give(bytes, data, size) };
    zero(r)
}

/// RegGetValueA: names and string data in the ANSI code page.
#[unsafe(no_mangle)]
pub extern "system" fn RegGetValueA(
    key: usize,
    sub: *const u8,
    value: *const u8,
    flags: u32,
    kind: *mut u32,
    data: *mut c_void,
    size: *mut u32,
) -> i32 {
    let (sub, value) = match (ansi_to_wide(sub, None), ansi_to_wide(value, None)) {
        (Ok(s), Ok(v)) => (s, v),
        (Err(e), _) | (_, Err(e)) => return e,
    };
    let sub = sub.as_ref().map_or(&[][..], WStr::units);
    let value = value.as_ref().map_or(&[][..], WStr::units);
    get_value_w(key, sub, value, flags, kind, data.cast(), size, true)
}

fn set_w(key: usize, name: &[u16], kind: u32, data: *const u8, size: u32) -> i32 {
    if data.is_null() && size > 0 {
        return ERROR_NOACCESS;
    }
    let h = match resolve(key) {
        Ok(h) => h,
        Err(e) => return e,
    };
    let name = unicode(name);
    // SAFETY: the caller's data of `size` bytes.
    error(unsafe { NtSetValueKey(h, &name, 0, kind, data, size) })
}

/// RegSetValueExW(hKey, lpValueName, Reserved, dwType, lpData, cbData)
#[unsafe(no_mangle)]
pub extern "system" fn RegSetValueExW(
    key: usize,
    name: *const u16,
    _reserved: u32,
    kind: u32,
    data: *const u8,
    size: u32,
) -> i32 {
    set_w(key, wide_arg(name), kind, data, size)
}

/// RegSetValueExA: string data converted from the ANSI code page.
#[unsafe(no_mangle)]
pub extern "system" fn RegSetValueExA(
    key: usize,
    name: *const u8,
    _reserved: u32,
    kind: u32,
    data: *const u8,
    size: u32,
) -> i32 {
    let name = match ansi_to_wide(name, None) {
        Ok(n) => n,
        Err(e) => return e,
    };
    let name = name.as_ref().map_or(&[][..], WStr::units);
    if !is_string(kind) || data.is_null() {
        return set_w(key, name, kind, data, size);
    }
    match ansi_to_wide(data, Some(size as usize)) {
        Ok(Some(w)) => set_w(key, name, kind, w.mem.ptr, 2 * w.len as u32),
        Ok(None) => set_w(key, name, kind, data, 0),
        Err(e) => e,
    }
}

/// RegSetKeyValueW(hKey, lpSubKey, lpValueName, dwType, lpData, cbData):
/// creates the subkey if needed.
#[unsafe(no_mangle)]
pub extern "system" fn RegSetKeyValueW(
    key: usize,
    sub: *const u16,
    name: *const u16,
    kind: u32,
    data: *const u8,
    size: u32,
) -> i32 {
    let sub = wide_arg(sub);
    if sub.is_empty() {
        return set_w(key, wide_arg(name), kind, data, size);
    }
    let mut h = 0;
    let r = create_w(key, sub, &[], 0, KEY_SET_VALUE, &mut h, core::ptr::null_mut());
    if r != ERROR_SUCCESS {
        return r;
    }
    let _opened = Opened { h, owned: true };
    set_w(h, wide_arg(name), kind, data, size)
}

/// RegDeleteValueW(hKey, lpValueName)
#[unsafe(no_mangle)]
pub extern "system" fn RegDeleteValueW(key: usize, name: *const u16) -> i32 {
    delete_value(key, &[], wide_arg(name))
}

#[unsafe(no_mangle)]
pub extern "system" fn RegDeleteValueA(key: usize, name: *const u8) -> i32 {
    match ansi_to_wide(name, None) {
        Ok(n) => delete_value(key, &[], n.as_ref().map_or(&[], WStr::units)),
        Err(e) => e,
    }
}

/// RegDeleteKeyValueW(hKey, lpSubKey, lpValueName)
#[unsafe(no_mangle)]
pub extern "system" fn RegDeleteKeyValueW(key: usize, sub: *const u16, name: *const u16) -> i32 {
    delete_value(key, wide_arg(sub), wide_arg(name))
}

fn delete_value(key: usize, sub: &[u16], name: &[u16]) -> i32 {
    let opened = match subkey(key, sub, KEY_SET_VALUE) {
        Ok(o) => o,
        Err(e) => return e,
    };
    let name = unicode(name);
    // SAFETY: plain call with our name.
    error(unsafe { NtDeleteValueKey(opened.h, &name) })
}

// ---- Enumeration and information ----

/// Runs an Nt query that fills a buffer, growing it as needed.
fn query_grow(mut f: impl FnMut(*mut u8, u32, &mut u32) -> u32) -> Result<Mem, i32> {
    let mut len = 256u32;
    loop {
        let mem = Mem::new(len as usize)?;
        let mut ret = 0;
        match f(mem.ptr, len, &mut ret) {
            0 => return Ok(mem),
            STATUS_BUFFER_OVERFLOW | STATUS_BUFFER_TOO_SMALL if ret > len => len = ret,
            s => return Err(error(s)),
        }
    }
}

/// Copies a name (and its terminator) to the caller's buffer of `*len`
/// characters; on success `*len` is the name's length.
fn give_name_w(name: &[u16], out: *mut u16, len: *mut u32) -> i32 {
    if out.is_null() || len.is_null() {
        return ERROR_INVALID_PARAMETER;
    }
    if (get(len) as usize) < name.len() + 1 {
        return ERROR_MORE_DATA;
    }
    // SAFETY: the caller's buffer of at least name.len() + 1 units.
    unsafe {
        core::ptr::copy_nonoverlapping(name.as_ptr(), out, name.len());
        *out.add(name.len()) = 0;
    }
    put(len, name.len() as u32);
    ERROR_SUCCESS
}

fn give_name_a(name: &[u16], out: *mut u8, len: *mut u32) -> i32 {
    if out.is_null() || len.is_null() {
        return ERROR_INVALID_PARAMETER;
    }
    let cap = get(len) as usize;
    let need = wide_to_ansi(name, core::ptr::null_mut(), 0);
    if cap < need + 1 {
        return ERROR_MORE_DATA;
    }
    wide_to_ansi(name, out, cap);
    // SAFETY: the caller's buffer of at least need + 1 bytes.
    unsafe { *out.add(need) = 0 };
    put(len, need as u32);
    ERROR_SUCCESS
}

/// KEY_NODE_INFORMATION of subkey `index`: (name, class, last write).
fn enum_key(key: usize, index: u32) -> Result<Mem, i32> {
    let h = resolve(key)?;
    // SAFETY: our buffer.
    query_grow(|buf, len, ret| unsafe { NtEnumerateKey(h, index, 1, buf, len, ret) })
}

/// KEY_NODE_INFORMATION: LastWriteTime, TitleIndex, ClassOffset,
/// ClassLength, NameLength, Name at 24.
fn node_name(mem: &Mem) -> &[u16] {
    mem.wide(24, mem.u32_at(20) as usize)
}

fn node_class(mem: &Mem) -> &[u16] {
    match mem.u32_at(12) {
        u32::MAX => &[],
        off => mem.wide(off as usize, mem.u32_at(16) as usize),
    }
}

/// RegEnumKeyExW(hKey, dwIndex, lpName, lpcchName, lpReserved, lpClass,
///               lpcchClass, lpftLastWriteTime): subkeys in name order.
#[allow(clippy::too_many_arguments)]
#[unsafe(no_mangle)]
pub extern "system" fn RegEnumKeyExW(
    key: usize,
    index: u32,
    name: *mut u16,
    name_len: *mut u32,
    _reserved: *mut u32,
    class: *mut u16,
    class_len: *mut u32,
    time: *mut u64,
) -> i32 {
    let mem = match enum_key(key, index) {
        Ok(m) => m,
        Err(e) => return e,
    };
    let r = give_name_w(node_name(&mem), name, name_len);
    if r != ERROR_SUCCESS {
        return r;
    }
    if !class.is_null() {
        let r = give_name_w(node_class(&mem), class, class_len);
        if r != ERROR_SUCCESS {
            return r;
        }
    }
    put(time, mem.u64_at(0));
    ERROR_SUCCESS
}

#[allow(clippy::too_many_arguments)]
#[unsafe(no_mangle)]
pub extern "system" fn RegEnumKeyExA(
    key: usize,
    index: u32,
    name: *mut u8,
    name_len: *mut u32,
    _reserved: *mut u32,
    class: *mut u8,
    class_len: *mut u32,
    time: *mut u64,
) -> i32 {
    let mem = match enum_key(key, index) {
        Ok(m) => m,
        Err(e) => return e,
    };
    let r = give_name_a(node_name(&mem), name, name_len);
    if r != ERROR_SUCCESS {
        return r;
    }
    if !class.is_null() {
        let r = give_name_a(node_class(&mem), class, class_len);
        if r != ERROR_SUCCESS {
            return r;
        }
    }
    put(time, mem.u64_at(0));
    ERROR_SUCCESS
}

/// RegEnumKeyW(hKey, dwIndex, lpName, cchName)
#[unsafe(no_mangle)]
pub extern "system" fn RegEnumKeyW(key: usize, index: u32, name: *mut u16, cch: u32) -> i32 {
    let mut len = cch;
    let null = core::ptr::null_mut();
    RegEnumKeyExW(key, index, name, &mut len, null, null.cast(), null, null.cast())
}

#[unsafe(no_mangle)]
pub extern "system" fn RegEnumKeyA(key: usize, index: u32, name: *mut u8, cch: u32) -> i32 {
    let mut len = cch;
    let null = core::ptr::null_mut();
    RegEnumKeyExA(key, index, name, &mut len, null, null.cast(), null, null.cast())
}

/// KEY_VALUE_FULL_INFORMATION of value `index`: TitleIndex, Type,
/// DataOffset, DataLength, NameLength, Name at 20.
fn enum_value(key: usize, index: u32) -> Result<Mem, i32> {
    let h = resolve(key)?;
    // SAFETY: our buffer.
    query_grow(|buf, len, ret| unsafe { NtEnumerateValueKey(h, index, 1, buf, len, ret) })
}

fn full_parts(mem: &Mem) -> (u32, &[u16], &[u8]) {
    let name = mem.wide(20, mem.u32_at(16) as usize);
    let (off, len) = (mem.u32_at(8) as usize, mem.u32_at(12) as usize);
    (mem.u32_at(4), name, &mem.bytes()[off..off + len])
}

/// RegEnumValueW(hKey, dwIndex, lpValueName, lpcchValueName, lpReserved,
///               lpType, lpData, lpcbData)
#[allow(clippy::too_many_arguments)]
#[unsafe(no_mangle)]
pub extern "system" fn RegEnumValueW(
    key: usize,
    index: u32,
    name: *mut u16,
    name_len: *mut u32,
    _reserved: *mut u32,
    kind: *mut u32,
    data: *mut u8,
    size: *mut u32,
) -> i32 {
    if !data.is_null() && size.is_null() {
        return ERROR_INVALID_PARAMETER;
    }
    let mem = match enum_value(key, index) {
        Ok(m) => m,
        Err(e) => return e,
    };
    let (k, value_name, bytes) = full_parts(&mem);
    let r = give_name_w(value_name, name, name_len);
    if r != ERROR_SUCCESS {
        return r;
    }
    put(kind, k);
    if size.is_null() { ERROR_SUCCESS } else { give(bytes, data, size) }
}

#[allow(clippy::too_many_arguments)]
#[unsafe(no_mangle)]
pub extern "system" fn RegEnumValueA(
    key: usize,
    index: u32,
    name: *mut u8,
    name_len: *mut u32,
    _reserved: *mut u32,
    kind: *mut u32,
    data: *mut u8,
    size: *mut u32,
) -> i32 {
    if !data.is_null() && size.is_null() {
        return ERROR_INVALID_PARAMETER;
    }
    let mem = match enum_value(key, index) {
        Ok(m) => m,
        Err(e) => return e,
    };
    let (k, value_name, bytes) = full_parts(&mem);
    let r = give_name_a(value_name, name, name_len);
    if r != ERROR_SUCCESS {
        return r;
    }
    put(kind, k);
    if size.is_null() { ERROR_SUCCESS } else { give_a(k, bytes, data, size) }
}

/// RegQueryInfoKeyW(hKey, lpClass, lpcchClass, lpReserved, lpcSubKeys,
///   lpcbMaxSubKeyLen, lpcbMaxClassLen, lpcValues, lpcbMaxValueNameLen,
///   lpcbMaxValueLen, lpcbSecurityDescriptor, lpftLastWriteTime): name
/// lengths in characters, data lengths in bytes.
#[allow(clippy::too_many_arguments)]
#[unsafe(no_mangle)]
pub extern "system" fn RegQueryInfoKeyW(
    key: usize,
    class: *mut u16,
    class_len: *mut u32,
    _reserved: *mut u32,
    subkeys: *mut u32,
    max_subkey: *mut u32,
    max_class: *mut u32,
    values: *mut u32,
    max_value_name: *mut u32,
    max_value: *mut u32,
    security: *mut u32,
    time: *mut u64,
) -> i32 {
    let mem = match key_full(key) {
        Ok(m) => m,
        Err(e) => return e,
    };
    if !class.is_null() {
        let r = give_name_w(full_class(&mem), class, class_len);
        if r != ERROR_SUCCESS {
            return r;
        }
    } else {
        put(class_len, full_class(&mem).len() as u32);
    }
    info_counts(&mem, subkeys, max_subkey, max_class, values, max_value_name, max_value);
    put(security, 0);
    put(time, mem.u64_at(0));
    ERROR_SUCCESS
}

#[allow(clippy::too_many_arguments)]
#[unsafe(no_mangle)]
pub extern "system" fn RegQueryInfoKeyA(
    key: usize,
    class: *mut u8,
    class_len: *mut u32,
    _reserved: *mut u32,
    subkeys: *mut u32,
    max_subkey: *mut u32,
    max_class: *mut u32,
    values: *mut u32,
    max_value_name: *mut u32,
    max_value: *mut u32,
    security: *mut u32,
    time: *mut u64,
) -> i32 {
    let mem = match key_full(key) {
        Ok(m) => m,
        Err(e) => return e,
    };
    if !class.is_null() {
        let r = give_name_a(full_class(&mem), class, class_len);
        if r != ERROR_SUCCESS {
            return r;
        }
    } else {
        put(class_len, full_class(&mem).len() as u32);
    }
    info_counts(&mem, subkeys, max_subkey, max_class, values, max_value_name, max_value);
    put(security, 0);
    put(time, mem.u64_at(0));
    ERROR_SUCCESS
}

/// KEY_FULL_INFORMATION: LastWriteTime, TitleIndex, ClassOffset,
/// ClassLength, SubKeys, MaxNameLen, MaxClassLen, Values, MaxValueNameLen,
/// MaxValueDataLen, Class.
fn key_full(key: usize) -> Result<Mem, i32> {
    let h = resolve(key)?;
    // SAFETY: our buffer.
    query_grow(|buf, len, ret| unsafe { NtQueryKey(h, 2, buf, len, ret) })
}

fn full_class(mem: &Mem) -> &[u16] {
    match mem.u32_at(12) {
        u32::MAX => &[],
        off => mem.wide(off as usize, mem.u32_at(16) as usize),
    }
}

#[allow(clippy::too_many_arguments)]
fn info_counts(
    mem: &Mem,
    subkeys: *mut u32,
    max_subkey: *mut u32,
    max_class: *mut u32,
    values: *mut u32,
    max_value_name: *mut u32,
    max_value: *mut u32,
) {
    put(subkeys, mem.u32_at(20));
    put(max_subkey, mem.u32_at(24) / 2);
    put(max_class, mem.u32_at(28) / 2);
    put(values, mem.u32_at(32));
    put(max_value_name, mem.u32_at(36) / 2);
    put(max_value, mem.u32_at(40));
}

// ---- Deleting keys ----

fn delete_key(key: usize, sub: &[u16]) -> i32 {
    if sub.is_empty() {
        return ERROR_INVALID_PARAMETER;
    }
    let opened = match subkey(key, sub, DELETE) {
        Ok(o) => o,
        Err(e) => return e,
    };
    // SAFETY: plain call.
    error(unsafe { NtDeleteKey(opened.h) })
}

/// RegDeleteKeyW(hKey, lpSubKey): the key must have no subkeys.
#[unsafe(no_mangle)]
pub extern "system" fn RegDeleteKeyW(key: usize, sub: *const u16) -> i32 {
    delete_key(key, wide_arg(sub))
}

#[unsafe(no_mangle)]
pub extern "system" fn RegDeleteKeyA(key: usize, sub: *const u8) -> i32 {
    match ansi_to_wide(sub, None) {
        Ok(s) => delete_key(key, s.as_ref().map_or(&[], WStr::units)),
        Err(e) => e,
    }
}

/// RegDeleteKeyExW(hKey, lpSubKey, samDesired, Reserved): one registry
/// view here, so samDesired's WOW64 flags change nothing.
#[unsafe(no_mangle)]
pub extern "system" fn RegDeleteKeyExW(
    key: usize,
    sub: *const u16,
    _access: u32,
    _reserved: u32,
) -> i32 {
    delete_key(key, wide_arg(sub))
}

#[unsafe(no_mangle)]
pub extern "system" fn RegDeleteKeyExA(
    key: usize,
    sub: *const u8,
    _access: u32,
    _reserved: u32,
) -> i32 {
    RegDeleteKeyA(key, sub)
}

/// Deletes every subkey of `h` (depth first) and, if `values`, its values.
fn delete_contents(h: usize) -> i32 {
    loop {
        // Always the first subkey: the others move up as each goes.
        let mem = match enum_key(h, 0) {
            Ok(m) => m,
            Err(ERROR_NO_MORE_ITEMS) => break,
            Err(e) => return e,
        };
        let child = match nt_open(
            h,
            node_name(&mem),
            DELETE | KEY_ENUMERATE_SUB_KEYS | KEY_QUERY_VALUE | KEY_SET_VALUE,
        ) {
            Ok(c) => Opened { h: c, owned: true },
            Err(e) => return e,
        };
        let r = delete_contents(child.h);
        if r != ERROR_SUCCESS {
            return r;
        }
        // SAFETY: plain call.
        let r = error(unsafe { NtDeleteKey(child.h) });
        if r != ERROR_SUCCESS {
            return r;
        }
    }
    loop {
        let mem = match enum_value(h, 0) {
            Ok(m) => m,
            Err(ERROR_NO_MORE_ITEMS) => return ERROR_SUCCESS,
            Err(e) => return e,
        };
        let name = unicode(full_parts(&mem).1);
        // SAFETY: plain call.
        let r = error(unsafe { NtDeleteValueKey(h, &name) });
        if r != ERROR_SUCCESS {
            return r;
        }
    }
}

/// RegDeleteTreeW(hKey, lpSubKey): the subkey and everything below it;
/// with no subkey, hKey's subkeys and values (hKey stays).
#[unsafe(no_mangle)]
pub extern "system" fn RegDeleteTreeW(key: usize, sub: *const u16) -> i32 {
    delete_tree(key, wide_arg(sub))
}

#[unsafe(no_mangle)]
pub extern "system" fn RegDeleteTreeA(key: usize, sub: *const u8) -> i32 {
    match ansi_to_wide(sub, None) {
        Ok(s) => delete_tree(key, s.as_ref().map_or(&[], WStr::units)),
        Err(e) => e,
    }
}

fn delete_tree(key: usize, sub: &[u16]) -> i32 {
    let access = DELETE | KEY_ENUMERATE_SUB_KEYS | KEY_QUERY_VALUE | KEY_SET_VALUE;
    let opened = match subkey(key, sub, access) {
        Ok(o) => o,
        Err(e) => return e,
    };
    let r = delete_contents(opened.h);
    if r != ERROR_SUCCESS || sub.is_empty() {
        return r;
    }
    // SAFETY: plain call.
    error(unsafe { NtDeleteKey(opened.h) })
}

// ---- Others ----

/// RegFlushKey: the registry is in memory.
#[unsafe(no_mangle)]
pub extern "system" fn RegFlushKey(key: usize) -> i32 {
    match resolve(key) {
        Ok(_) => ERROR_SUCCESS,
        Err(e) => e,
    }
}

/// RegDisablePredefinedCache(Ex): the cached handles stay valid.
#[unsafe(no_mangle)]
pub extern "system" fn RegDisablePredefinedCache() -> i32 {
    ERROR_SUCCESS
}

#[unsafe(no_mangle)]
pub extern "system" fn RegDisablePredefinedCacheEx() -> i32 {
    ERROR_SUCCESS
}

/// RegQueryValueW(hKey, lpSubKey, lpData, lpcbData): the subkey's unnamed
/// value (REG_SZ).
#[unsafe(no_mangle)]
pub extern "system" fn RegQueryValueW(
    key: usize,
    sub: *const u16,
    data: *mut u16,
    size: *mut i32,
) -> i32 {
    let opened = match subkey(key, wide_arg(sub), KEY_QUERY_VALUE) {
        Ok(o) => o,
        Err(e) => return e,
    };
    match read_value(opened.h, &[]) {
        Ok((_, mem)) => give(value_data(&mem), data.cast(), size.cast()),
        // No unnamed value: an empty string.
        Err(ERROR_FILE_NOT_FOUND) => give(&[0, 0], data.cast(), size.cast()),
        Err(e) => e,
    }
}
