//! The registry, with the results real Windows gives (documented
//! contracts of advapi32's Reg* functions and ntdll's Nt*Key services):
//!
//! - Predefined keys, zero- or sign-extended to 64 bits.
//! - Keys real programs read exist: HKLM\SOFTWARE\Microsoft\Windows
//!   NT\CurrentVersion (CurrentBuild "26100", CurrentMajorVersionNumber 10,
//!   ProductName "Windows ..."), HKLM\SYSTEM\CurrentControlSet\Control\Nls
//!   \CodePage, HARDWARE\DESCRIPTION\System\CentralProcessor\0,
//!   HKCU\Environment, HKU\.DEFAULT. Names are case-insensitive.
//! - RegQueryValueExW: the type and size alone (NULL data), ERROR_MORE_DATA
//!   with the needed size, ERROR_FILE_NOT_FOUND for a missing value.
//! - RegGetValueW: type restrictions (ERROR_UNSUPPORTED_TYPE), REG_EXPAND_SZ
//!   expanded and reported as REG_SZ, RRF_RT_REG_EXPAND_SZ needs
//!   RRF_NOEXPAND.
//! - RegCreateKeyExW creates missing intermediate keys; dispositions
//!   (REG_CREATED_NEW_KEY, REG_OPENED_EXISTING_KEY); a volatile key's
//!   children must be volatile (ERROR_CHILD_MUST_BE_VOLATILE).
//! - RegSetValueExW of each type; RegQueryInfoKeyW counts and maximum
//!   lengths; RegEnumKeyExW in name order (case-insensitive), RegEnumValueW,
//!   ERROR_NO_MORE_ITEMS, ERROR_MORE_DATA for a short name buffer.
//! - Access checks: ERROR_ACCESS_DENIED without KEY_SET_VALUE or
//!   KEY_ENUMERATE_SUB_KEYS.
//! - RegDeleteValueW; RegDeleteKeyW of a key with subkeys is
//!   ERROR_ACCESS_DENIED; a deleted key's handles get ERROR_KEY_DELETED;
//!   RegDeleteTreeW.
//! - The A functions convert names and string data.
//! - Nt*Key directly: NtOpenKey by \Registry path (case-insensitive even
//!   without OBJ_CASE_INSENSITIVE), NtQueryValueKey partial information and
//!   STATUS_BUFFER_OVERFLOW / STATUS_BUFFER_TOO_SMALL with the needed
//!   length, NtQueryKey basic and name information (the full name starts
//!   \REGISTRY\MACHINE), STATUS_NO_MORE_ENTRIES.
//!
//! The test's own keys are volatile and deleted at the end, so a run
//! leaves nothing behind on Windows.
//!
//! Prints `registry_test: ok` or exits with the number of the failing
//! check.

#![no_std]
#![no_main]

use core::ffi::c_void;

type Hkey = usize;

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
    fn SetEnvironmentVariableW(name: *const u16, value: *const u16) -> i32;
}

#[link(name = "advapi32", kind = "raw-dylib")]
unsafe extern "system" {
    fn RegOpenKeyExW(key: Hkey, sub: *const u16, options: u32, access: u32, out: *mut Hkey) -> i32;
    fn RegOpenKeyExA(key: Hkey, sub: *const u8, options: u32, access: u32, out: *mut Hkey) -> i32;
    fn RegCreateKeyExW(
        key: Hkey,
        sub: *const u16,
        reserved: u32,
        class: *const u16,
        options: u32,
        access: u32,
        security: *mut c_void,
        out: *mut Hkey,
        disposition: *mut u32,
    ) -> i32;
    fn RegCloseKey(key: Hkey) -> i32;
    fn RegQueryValueExW(
        key: Hkey,
        name: *const u16,
        reserved: *mut u32,
        kind: *mut u32,
        data: *mut u8,
        size: *mut u32,
    ) -> i32;
    fn RegQueryValueExA(
        key: Hkey,
        name: *const u8,
        reserved: *mut u32,
        kind: *mut u32,
        data: *mut u8,
        size: *mut u32,
    ) -> i32;
    fn RegGetValueW(
        key: Hkey,
        sub: *const u16,
        value: *const u16,
        flags: u32,
        kind: *mut u32,
        data: *mut c_void,
        size: *mut u32,
    ) -> i32;
    fn RegSetValueExW(
        key: Hkey,
        name: *const u16,
        reserved: u32,
        kind: u32,
        data: *const u8,
        size: u32,
    ) -> i32;
    fn RegSetValueExA(
        key: Hkey,
        name: *const u8,
        reserved: u32,
        kind: u32,
        data: *const u8,
        size: u32,
    ) -> i32;
    fn RegQueryInfoKeyW(
        key: Hkey,
        class: *mut u16,
        class_len: *mut u32,
        reserved: *mut u32,
        subkeys: *mut u32,
        max_subkey_len: *mut u32,
        max_class_len: *mut u32,
        values: *mut u32,
        max_value_name_len: *mut u32,
        max_value_len: *mut u32,
        security: *mut u32,
        last_write: *mut u64,
    ) -> i32;
    fn RegEnumKeyExW(
        key: Hkey,
        index: u32,
        name: *mut u16,
        name_len: *mut u32,
        reserved: *mut u32,
        class: *mut u16,
        class_len: *mut u32,
        last_write: *mut u64,
    ) -> i32;
    fn RegEnumKeyExA(
        key: Hkey,
        index: u32,
        name: *mut u8,
        name_len: *mut u32,
        reserved: *mut u32,
        class: *mut u8,
        class_len: *mut u32,
        last_write: *mut u64,
    ) -> i32;
    fn RegEnumValueW(
        key: Hkey,
        index: u32,
        name: *mut u16,
        name_len: *mut u32,
        reserved: *mut u32,
        kind: *mut u32,
        data: *mut u8,
        data_len: *mut u32,
    ) -> i32;
    fn RegDeleteValueW(key: Hkey, name: *const u16) -> i32;
    fn RegDeleteKeyW(key: Hkey, sub: *const u16) -> i32;
    fn RegDeleteTreeW(key: Hkey, sub: *const u16) -> i32;
}

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
    fn NtQueryValueKey(
        key: usize,
        name: *const UnicodeString,
        class: u32,
        buf: *mut u8,
        len: u32,
        ret: *mut u32,
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
    fn NtEnumerateValueKey(
        key: usize,
        index: u32,
        class: u32,
        buf: *mut u8,
        len: u32,
        ret: *mut u32,
    ) -> u32;
    fn NtClose(h: usize) -> u32;
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

const HKEY_CLASSES_ROOT: Hkey = 0x8000_0000;
const HKEY_CURRENT_USER: Hkey = 0x8000_0001;
const HKEY_LOCAL_MACHINE: Hkey = 0x8000_0002;
const HKEY_USERS: Hkey = 0x8000_0003;

const KEY_QUERY_VALUE: u32 = 0x0001;
const KEY_READ: u32 = 0x2_0019;
const KEY_ALL_ACCESS: u32 = 0xf_003f;
const REG_OPTION_NON_VOLATILE: u32 = 0;
const REG_OPTION_VOLATILE: u32 = 1;
const REG_CREATED_NEW_KEY: u64 = 1;
const REG_OPENED_EXISTING_KEY: u64 = 2;

const REG_SZ: u32 = 1;
const REG_EXPAND_SZ: u32 = 2;
const REG_BINARY: u32 = 3;
const REG_DWORD: u32 = 4;
const REG_MULTI_SZ: u32 = 7;
const REG_QWORD: u32 = 11;

const RRF_RT_REG_SZ: u32 = 0x2;
const RRF_RT_REG_EXPAND_SZ: u32 = 0x4;
const RRF_RT_REG_DWORD: u32 = 0x10;
const RRF_RT_DWORD: u32 = 0x18;
const RRF_RT_ANY: u32 = 0xffff;
const RRF_NOEXPAND: u32 = 0x1000_0000;

const ERROR_FILE_NOT_FOUND: u64 = 2;
const ERROR_ACCESS_DENIED: u64 = 5;
const ERROR_INVALID_HANDLE: u64 = 6;
const ERROR_INVALID_PARAMETER: u64 = 87;
const ERROR_MORE_DATA: u64 = 234;
const ERROR_NO_MORE_ITEMS: u64 = 259;
const ERROR_KEY_DELETED: u64 = 1018;
const ERROR_CHILD_MUST_BE_VOLATILE: u64 = 1021;
const ERROR_UNSUPPORTED_TYPE: u64 = 1630;

const STATUS_BUFFER_OVERFLOW: u64 = 0x8000_0005;
const STATUS_NO_MORE_ENTRIES: u64 = 0x8000_001a;
const STATUS_BUFFER_TOO_SMALL: u64 = 0xc000_0023;
const STATUS_OBJECT_NAME_NOT_FOUND: u64 = 0xc000_0034;

const CURRENT_VERSION: &str = "SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion";

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
        out(b"registry_test: check ");
        out(&hex(self.0));
        out(b" got ");
        out(&hex(got as u32));
        out(b"\r\n");
        Err(self.0)
    }

    fn eq(&mut self, got: u64, want: u64) -> Result<(), u32> {
        self.ok(got == want, got)
    }

    /// A Reg* result of ERROR_SUCCESS.
    fn ok0(&mut self, r: i32) -> Result<(), u32> {
        self.eq(r as u64, 0)
    }
}

/// A null output pointer of any type.
fn nul<T>() -> *mut T {
    core::ptr::null_mut()
}

/// A NUL-terminated UTF-16 copy of `s`.
fn w(s: &str) -> [u16; 96] {
    let mut b = [0u16; 96];
    for (d, ch) in b.iter_mut().zip(s.bytes()) {
        *d = ch.into();
    }
    b
}

/// A NUL-terminated byte copy of `s`.
fn a(s: &str) -> [u8; 96] {
    let mut b = [0u8; 96];
    b[..s.len()].copy_from_slice(s.as_bytes());
    b
}

/// Whether the UTF-16 units `got` are `want` (ASCII).
fn same(got: &[u16], want: &str) -> bool {
    got.len() == want.len() && got.iter().zip(want.bytes()).all(|(&g, b)| g == u16::from(b))
}

/// The UTF-16 bytes of `s` with its terminator (as REG_SZ data).
fn sz(s: &str, buf: &mut [u8; 192]) -> u32 {
    let n = s.len();
    for (i, b) in s.bytes().enumerate() {
        buf[2 * i] = b;
        buf[2 * i + 1] = 0;
    }
    buf[2 * n] = 0;
    buf[2 * n + 1] = 0;
    (2 * n + 2) as u32
}

fn units(bytes: &[u8]) -> [u16; 96] {
    let mut b = [0u16; 96];
    for (i, d) in b.iter_mut().enumerate().take(bytes.len() / 2) {
        *d = u16::from_le_bytes([bytes[2 * i], bytes[2 * i + 1]]);
    }
    b
}

fn seeded(c: &mut Checks) -> Result<(), u32> {
    // SAFETY: advapi32 calls with buffers of ours.
    unsafe {
        let mut key = 0;
        c.ok0(RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            w(CURRENT_VERSION).as_ptr(),
            0,
            KEY_READ,
            &mut key,
        ))?;
        let mut kind = 0u32;
        let mut size = 0u32;
        let build = w("CurrentBuild");
        // The type and size alone.
        c.ok0(RegQueryValueExW(
            key,
            build.as_ptr(),
            core::ptr::null_mut(),
            &mut kind,
            core::ptr::null_mut(),
            &mut size,
        ))?;
        c.eq(kind.into(), REG_SZ.into())?;
        c.eq(size.into(), 12)?; // "26100" and its terminator
        let mut data = [0u8; 192];
        // Too small: the size needed.
        size = 4;
        let r = RegQueryValueExW(
            key,
            build.as_ptr(),
            core::ptr::null_mut(),
            &mut kind,
            data.as_mut_ptr(),
            &mut size,
        );
        c.eq(r as u64, ERROR_MORE_DATA)?;
        c.eq(size.into(), 12)?;
        size = 192;
        c.ok0(RegQueryValueExW(
            key,
            build.as_ptr(),
            core::ptr::null_mut(),
            &mut kind,
            data.as_mut_ptr(),
            &mut size,
        ))?;
        c.ok(same(&units(&data[..10])[..5], "26100"), size.into())?;
        // Value names are case-insensitive too.
        let mut major = 0u32;
        size = 4;
        c.ok0(RegQueryValueExW(
            key,
            w("currentmajorversionnumber").as_ptr(),
            core::ptr::null_mut(),
            &mut kind,
            (&raw mut major).cast(),
            &mut size,
        ))?;
        c.eq(kind.into(), REG_DWORD.into())?;
        c.eq(major.into(), 10)?;
        size = 192;
        c.ok0(RegQueryValueExW(
            key,
            w("ProductName").as_ptr(),
            core::ptr::null_mut(),
            &mut kind,
            data.as_mut_ptr(),
            &mut size,
        ))?;
        c.ok(same(&units(&data[..14])[..7], "Windows"), size.into())?;
        // A value that does not exist.
        size = 192;
        let r = RegQueryValueExW(
            key,
            w("NoSuchValue").as_ptr(),
            core::ptr::null_mut(),
            &mut kind,
            data.as_mut_ptr(),
            &mut size,
        );
        c.eq(r as u64, ERROR_FILE_NOT_FOUND)?;
        c.ok0(RegCloseKey(key))?;

        // The predefined keys as winreg.h defines them on x64, sign-extended
        // ((HKEY)(ULONG_PTR)((LONG)0x80000002)), as C programs and .NET
        // pass them.
        c.ok0(RegOpenKeyExW(
            HKEY_LOCAL_MACHINE | 0xffff_ffff_0000_0000,
            w(CURRENT_VERSION).as_ptr(),
            0,
            KEY_READ,
            &mut key,
        ))?;
        c.ok0(RegCloseKey(key))?;
        c.ok0(RegCloseKey(HKEY_LOCAL_MACHINE | 0xffff_ffff_0000_0000))?;

        // Paths are case-insensitive.
        c.ok0(RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            w("software\\MICROSOFT\\windows nt\\currentversion").as_ptr(),
            0,
            KEY_READ,
            &mut key,
        ))?;
        c.ok0(RegCloseKey(key))?;

        // RegGetValueW: a subkey and value in one call.
        let mut buf = [0u16; 32];
        size = 64;
        c.ok0(RegGetValueW(
            HKEY_LOCAL_MACHINE,
            w(CURRENT_VERSION).as_ptr(),
            build.as_ptr(),
            RRF_RT_REG_SZ,
            &mut kind,
            buf.as_mut_ptr().cast(),
            &mut size,
        ))?;
        c.eq(size.into(), 12)?;
        c.ok(same(&buf[..6], "26100\0"), buf[0].into())?;
        // The wrong type.
        size = 64;
        let r = RegGetValueW(
            HKEY_LOCAL_MACHINE,
            w(CURRENT_VERSION).as_ptr(),
            build.as_ptr(),
            RRF_RT_REG_DWORD,
            &mut kind,
            buf.as_mut_ptr().cast(),
            &mut size,
        );
        c.eq(r as u64, ERROR_UNSUPPORTED_TYPE)?;
        major = 0;
        size = 4;
        c.ok0(RegGetValueW(
            HKEY_LOCAL_MACHINE,
            w(CURRENT_VERSION).as_ptr(),
            w("CurrentMajorVersionNumber").as_ptr(),
            RRF_RT_DWORD,
            &mut kind,
            (&raw mut major).cast(),
            &mut size,
        ))?;
        c.eq(major.into(), 10)?;

        // Other keys programs read.
        c.ok0(RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            w("SYSTEM\\CurrentControlSet\\Control\\Nls\\CodePage").as_ptr(),
            0,
            KEY_READ,
            &mut key,
        ))?;
        size = 192;
        c.ok0(RegQueryValueExW(
            key,
            w("ACP").as_ptr(),
            core::ptr::null_mut(),
            &mut kind,
            data.as_mut_ptr(),
            &mut size,
        ))?;
        c.eq(kind.into(), REG_SZ.into())?;
        RegCloseKey(key);
        // The first processor's description (from CPUID).
        size = 192;
        c.ok0(RegGetValueW(
            HKEY_LOCAL_MACHINE,
            w("HARDWARE\\DESCRIPTION\\System\\CentralProcessor\\0").as_ptr(),
            w("Identifier").as_ptr(),
            RRF_RT_REG_SZ,
            &mut kind,
            data.as_mut_ptr().cast(),
            &mut size,
        ))?;
        let id = units(&data[..12]);
        c.ok(same(&id[..5], "Intel") || same(&id[..5], "AMD64"), id[0].into())?;
        c.ok0(RegOpenKeyExW(HKEY_CURRENT_USER, w("Environment").as_ptr(), 0, KEY_READ, &mut key))?;
        RegCloseKey(key);
        c.ok0(RegOpenKeyExW(HKEY_USERS, w(".DEFAULT").as_ptr(), 0, KEY_READ, &mut key))?;
        RegCloseKey(key);
        // No subkey: a new handle to the same key.
        c.ok0(RegOpenKeyExW(HKEY_CLASSES_ROOT, core::ptr::null(), 0, KEY_READ, &mut key))?;
        c.ok(key != 0, 0)?;
        c.ok0(RegCloseKey(key))?;
        // A missing key, and one below a missing key.
        let r = RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            w("SOFTWARE\\gala-test\\no-such-key").as_ptr(),
            0,
            KEY_READ,
            &mut key,
        );
        c.eq(r as u64, ERROR_FILE_NOT_FOUND)?;
        c.eq(RegCloseKey(0x1234) as u64, ERROR_INVALID_HANDLE)?;
        // Predefined keys close successfully.
        c.ok0(RegCloseKey(HKEY_LOCAL_MACHINE))?;
    }
    Ok(())
}

/// Sets `name` in `key` to `data` of `kind`.
fn set(c: &mut Checks, key: Hkey, name: &str, kind: u32, data: &[u8]) -> Result<(), u32> {
    // SAFETY: our buffers.
    let r =
        unsafe { RegSetValueExW(key, w(name).as_ptr(), 0, kind, data.as_ptr(), data.len() as u32) };
    c.ok0(r)
}

fn written(c: &mut Checks) -> Result<(), u32> {
    // SAFETY: advapi32 calls with buffers of ours.
    unsafe {
        // Whatever an earlier, interrupted run left behind.
        RegDeleteTreeW(HKEY_CURRENT_USER, w("Software\\GalaRegistryTest").as_ptr());

        // Missing intermediate keys are created, with the same options.
        let mut sub = 0;
        let mut disposition = 0u32;
        c.ok0(RegCreateKeyExW(
            HKEY_CURRENT_USER,
            w("Software\\GalaRegistryTest\\Sub").as_ptr(),
            0,
            core::ptr::null(),
            REG_OPTION_VOLATILE,
            KEY_ALL_ACCESS,
            core::ptr::null_mut(),
            &mut sub,
            &mut disposition,
        ))?;
        c.eq(disposition.into(), REG_CREATED_NEW_KEY)?;
        c.ok0(RegCloseKey(sub))?;
        let mut test = 0;
        c.ok0(RegCreateKeyExW(
            HKEY_CURRENT_USER,
            w("Software\\GalaRegistryTest").as_ptr(),
            0,
            core::ptr::null(),
            REG_OPTION_VOLATILE,
            KEY_ALL_ACCESS,
            core::ptr::null_mut(),
            &mut test,
            &mut disposition,
        ))?;
        c.eq(disposition.into(), REG_OPENED_EXISTING_KEY)?;
        // A volatile key's children are volatile.
        let mut other = 0;
        let r = RegCreateKeyExW(
            test,
            w("stable").as_ptr(),
            0,
            core::ptr::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_ALL_ACCESS,
            core::ptr::null_mut(),
            &mut other,
            &mut disposition,
        );
        c.eq(r as u64, ERROR_CHILD_MUST_BE_VOLATILE)?;
        for name in ["alpha", "Beta"] {
            c.ok0(RegCreateKeyExW(
                test,
                w(name).as_ptr(),
                0,
                core::ptr::null(),
                REG_OPTION_VOLATILE,
                KEY_ALL_ACCESS,
                core::ptr::null_mut(),
                &mut other,
                &mut disposition,
            ))?;
            c.eq(disposition.into(), REG_CREATED_NEW_KEY)?;
            RegCloseKey(other);
        }

        // Values of each type; the unnamed (default) value.
        let mut b = [0u8; 192];
        let n = sz("hello", &mut b);
        set(c, test, "str", REG_SZ, &b[..n as usize])?;
        set(c, test, "num", REG_DWORD, &0x1234_5678u32.to_le_bytes())?;
        set(c, test, "num", REG_DWORD, &0x9abc_def0u32.to_le_bytes())?; // replaced
        set(c, test, "q", REG_QWORD, &0x1122_3344_5566_7788u64.to_le_bytes())?;
        set(c, test, "bin", REG_BINARY, &[1, 2, 3])?;
        let n = sz("def", &mut b);
        set(c, test, "", REG_SZ, &b[..n as usize])?;
        let n = sz("a\0bc\0", &mut b);
        set(c, test, "multi", REG_MULTI_SZ, &b[..n as usize])?;
        let n = sz("%GALA_REG_TEST%\\x", &mut b);
        set(c, test, "exp", REG_EXPAND_SZ, &b[..n as usize])?;

        let (mut subkeys, mut max_subkey, mut values, mut max_name, mut max_data) =
            (0u32, 0u32, 0u32, 0u32, 0u32);
        c.ok0(RegQueryInfoKeyW(
            test,
            nul(),
            nul(),
            nul(),
            &mut subkeys,
            &mut max_subkey,
            nul(),
            &mut values,
            &mut max_name,
            &mut max_data,
            nul(),
            core::ptr::null_mut(),
        ))?;
        c.eq(subkeys.into(), 3)?;
        c.eq(max_subkey.into(), 5)?; // "alpha"
        c.eq(values.into(), 7)?;
        c.eq(max_name.into(), 5)?; // "multi"
        c.eq(max_data.into(), 36)?; // "%GALA_REG_TEST%\x" and its terminator

        // Subkeys in name order, ignoring case.
        let mut name = [0u16; 32];
        for (i, want) in ["alpha", "Beta", "Sub"].iter().enumerate() {
            let mut len = 32u32;
            c.ok0(RegEnumKeyExW(
                test,
                i as u32,
                name.as_mut_ptr(),
                &mut len,
                nul(),
                nul(),
                nul(),
                core::ptr::null_mut(),
            ))?;
            c.eq(len.into(), want.len() as u64)?;
            c.ok(same(&name[..len as usize], want), name[0].into())?;
        }
        let mut len = 32u32;
        let r = RegEnumKeyExW(
            test,
            3,
            name.as_mut_ptr(),
            &mut len,
            nul(),
            nul(),
            nul(),
            core::ptr::null_mut(),
        );
        c.eq(r as u64, ERROR_NO_MORE_ITEMS)?;
        len = 3; // "alpha" and its terminator need 6
        let r = RegEnumKeyExW(
            test,
            0,
            name.as_mut_ptr(),
            &mut len,
            nul(),
            nul(),
            nul(),
            core::ptr::null_mut(),
        );
        c.eq(r as u64, ERROR_MORE_DATA)?;

        // Every value once.
        let mut seen = 0u32;
        let names = ["str", "num", "q", "bin", "", "multi", "exp"];
        for i in 0..7 {
            let mut len = 32u32;
            let mut kind = 0u32;
            let mut size = 192u32;
            c.ok0(RegEnumValueW(
                test,
                i,
                name.as_mut_ptr(),
                &mut len,
                nul(),
                &mut kind,
                b.as_mut_ptr(),
                &mut size,
            ))?;
            let at = names.iter().position(|n| same(&name[..len as usize], n));
            c.ok(at.is_some(), i.into())?;
            let at = at.unwrap_or(0);
            seen |= 1 << at;
            if at == 1 {
                c.eq(kind.into(), REG_DWORD.into())?;
                c.eq(size.into(), 4)?;
                c.eq(u32::from_le_bytes([b[0], b[1], b[2], b[3]]).into(), 0x9abc_def0)?;
            }
        }
        c.eq(seen.into(), 0x7f)?;
        let mut len = 32u32;
        let r = RegEnumValueW(test, 7, name.as_mut_ptr(), &mut len, nul(), nul(), nul(), nul());
        c.eq(r as u64, ERROR_NO_MORE_ITEMS)?;

        // The default value, through its empty name and through NULL.
        let mut kind = 0u32;
        let mut size = 192u32;
        c.ok0(RegQueryValueExW(
            test,
            core::ptr::null(),
            nul(),
            &mut kind,
            b.as_mut_ptr(),
            &mut size,
        ))?;
        c.eq(size.into(), 8)?;
        c.ok(same(&units(&b[..8])[..4], "def\0"), b[0].into())?;

        // REG_EXPAND_SZ is expanded by RegGetValueW (and reported as REG_SZ).
        SetEnvironmentVariableW(w("GALA_REG_TEST").as_ptr(), w("C:\\gala").as_ptr());
        let mut buf = [0u16; 48];
        size = 96;
        c.ok0(RegGetValueW(
            test,
            core::ptr::null(),
            w("exp").as_ptr(),
            RRF_RT_REG_SZ,
            &mut kind,
            buf.as_mut_ptr().cast(),
            &mut size,
        ))?;
        c.eq(kind.into(), REG_SZ.into())?;
        c.ok(same(&buf[..10], "C:\\gala\\x\0"), buf[0].into())?;
        c.eq(size.into(), 20)?;
        size = 96;
        c.ok0(RegGetValueW(
            test,
            core::ptr::null(),
            w("exp").as_ptr(),
            RRF_RT_ANY | RRF_NOEXPAND,
            &mut kind,
            buf.as_mut_ptr().cast(),
            &mut size,
        ))?;
        c.eq(kind.into(), REG_EXPAND_SZ.into())?;
        c.eq(size.into(), 36)?;
        size = 96;
        let r = RegGetValueW(
            test,
            core::ptr::null(),
            w("exp").as_ptr(),
            RRF_RT_REG_EXPAND_SZ,
            &mut kind,
            buf.as_mut_ptr().cast(),
            &mut size,
        );
        c.eq(r as u64, ERROR_INVALID_PARAMETER)?;

        // The A functions.
        let mut abuf = [0u8; 32];
        size = 32;
        c.ok0(RegQueryValueExA(
            test,
            a("str").as_ptr(),
            nul(),
            &mut kind,
            abuf.as_mut_ptr(),
            &mut size,
        ))?;
        c.eq(kind.into(), REG_SZ.into())?;
        c.eq(size.into(), 6)?;
        c.ok(&abuf[..6] == b"hello\0", abuf[0].into())?;
        c.ok0(RegSetValueExA(test, a("ansi").as_ptr(), 0, REG_SZ, c"abc".as_ptr().cast(), 4))?;
        size = 192;
        c.ok0(RegQueryValueExW(
            test,
            w("ANSI").as_ptr(),
            nul(),
            &mut kind,
            b.as_mut_ptr(),
            &mut size,
        ))?;
        c.eq(size.into(), 8)?;
        c.ok(same(&units(&b[..8])[..4], "abc\0"), b[0].into())?;
        let mut alen = 32u32;
        c.ok0(RegEnumKeyExA(
            test,
            1,
            abuf.as_mut_ptr(),
            &mut alen,
            nul(),
            core::ptr::null_mut(),
            nul(),
            core::ptr::null_mut(),
        ))?;
        c.eq(alen.into(), 4)?;
        c.ok(&abuf[..5] == b"Beta\0", abuf[0].into())?;
        let mut via_a = 0;
        c.ok0(RegOpenKeyExA(test, a("SUB").as_ptr(), 0, KEY_READ, &mut via_a))?;
        RegCloseKey(via_a);

        // Deleting values.
        c.ok0(RegDeleteValueW(test, w("bin").as_ptr()))?;
        c.eq(RegDeleteValueW(test, w("bin").as_ptr()) as u64, ERROR_FILE_NOT_FOUND)?;

        // Access is checked against what the handle was opened for.
        let mut reader = 0;
        c.ok0(RegOpenKeyExW(
            HKEY_CURRENT_USER,
            w("Software\\GalaRegistryTest").as_ptr(),
            0,
            KEY_QUERY_VALUE,
            &mut reader,
        ))?;
        let r = RegSetValueExW(reader, w("x").as_ptr(), 0, REG_BINARY, b.as_ptr(), 1);
        c.eq(r as u64, ERROR_ACCESS_DENIED)?;
        len = 32;
        let r = RegEnumKeyExW(
            reader,
            0,
            name.as_mut_ptr(),
            &mut len,
            nul(),
            nul(),
            nul(),
            core::ptr::null_mut(),
        );
        c.eq(r as u64, ERROR_ACCESS_DENIED)?;
        RegCloseKey(reader);

        // Deleting keys: not one with subkeys; open handles see the deletion.
        let mut beta = 0;
        c.ok0(RegOpenKeyExW(test, w("beta").as_ptr(), 0, KEY_ALL_ACCESS, &mut beta))?;
        c.ok0(RegDeleteKeyW(test, w("Beta").as_ptr()))?;
        size = 192;
        let r =
            RegQueryValueExW(beta, w("x").as_ptr(), nul(), &mut kind, b.as_mut_ptr(), &mut size);
        c.eq(r as u64, ERROR_KEY_DELETED)?;
        let r = RegSetValueExW(beta, w("x").as_ptr(), 0, REG_BINARY, b.as_ptr(), 1);
        c.eq(r as u64, ERROR_KEY_DELETED)?;
        c.ok0(RegCloseKey(beta))?;
        let r = RegOpenKeyExW(test, w("Beta").as_ptr(), 0, KEY_READ, &mut beta);
        c.eq(r as u64, ERROR_FILE_NOT_FOUND)?;
        let mut software = 0;
        c.ok0(RegOpenKeyExW(
            HKEY_CURRENT_USER,
            w("Software").as_ptr(),
            0,
            KEY_ALL_ACCESS,
            &mut software,
        ))?;
        let r = RegDeleteKeyW(software, w("GalaRegistryTest").as_ptr());
        c.eq(r as u64, ERROR_ACCESS_DENIED)?;

        nt(c, test)?;

        c.ok0(RegCloseKey(test))?;
        c.ok0(RegDeleteTreeW(software, w("GalaRegistryTest").as_ptr()))?;
        let r = RegOpenKeyExW(software, w("GalaRegistryTest").as_ptr(), 0, KEY_READ, &mut test);
        c.eq(r as u64, ERROR_FILE_NOT_FOUND)?;
        RegCloseKey(software);
    }
    Ok(())
}

fn unicode(s: &[u16; 96]) -> UnicodeString {
    let n = s.iter().position(|&c| c == 0).unwrap_or(0) as u16;
    UnicodeString { length: 2 * n, maximum: 2 * n, buffer: s.as_ptr() }
}

fn attributes(root: usize, name: &UnicodeString, flags: u32) -> ObjectAttributes {
    ObjectAttributes {
        length: size_of::<ObjectAttributes>() as u32,
        root,
        name,
        attributes: flags,
        security: 0,
        qos: 0,
    }
}

/// Nt*Key directly; `test` is the test's key (subkeys alpha and Sub).
fn nt(c: &mut Checks, test: Hkey) -> Result<(), u32> {
    const OBJ_CASE_INSENSITIVE: u32 = 0x40;
    // SAFETY: ntdll calls with buffers of ours.
    unsafe {
        let path = w("\\Registry\\Machine\\SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion");
        let name = unicode(&path);
        let mut key = 0usize;
        c.eq(NtOpenKey(&mut key, KEY_READ, &attributes(0, &name, OBJ_CASE_INSENSITIVE)).into(), 0)?;
        // Registry names are case-insensitive even without the attribute.
        let lower = w("\\registry\\machine\\software\\microsoft\\windows nt\\currentversion");
        let lower_name = unicode(&lower);
        let mut other = 0usize;
        c.eq(NtOpenKey(&mut other, KEY_READ, &attributes(0, &lower_name, 0)).into(), 0)?;
        NtClose(other);
        // Relative to a key.
        let missing = w("NoSuchKey");
        let missing_name = unicode(&missing);
        let r =
            NtOpenKey(&mut other, KEY_READ, &attributes(key, &missing_name, OBJ_CASE_INSENSITIVE));
        c.eq(r.into(), STATUS_OBJECT_NAME_NOT_FOUND)?;

        // KEY_VALUE_PARTIAL_INFORMATION: TitleIndex, Type, DataLength, Data.
        let build = w("CurrentBuild");
        let build_name = unicode(&build);
        let mut buf = [0u8; 256];
        let mut ret = 0u32;
        c.eq(NtQueryValueKey(key, &build_name, 2, buf.as_mut_ptr(), 256, &mut ret).into(), 0)?;
        c.eq(ret.into(), 24)?;
        c.eq(u32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]).into(), REG_SZ.into())?;
        c.eq(u32::from_le_bytes([buf[8], buf[9], buf[10], buf[11]]).into(), 12)?;
        c.ok(same(&units(&buf[12..24])[..6], "26100\0"), buf[12].into())?;
        ret = 0;
        let r = NtQueryValueKey(key, &build_name, 2, buf.as_mut_ptr(), 12, &mut ret);
        c.eq(r.into(), STATUS_BUFFER_OVERFLOW)?;
        c.eq(ret.into(), 24)?;
        ret = 0;
        let r = NtQueryValueKey(key, &build_name, 2, buf.as_mut_ptr(), 4, &mut ret);
        c.eq(r.into(), STATUS_BUFFER_TOO_SMALL)?;
        c.eq(ret.into(), 24)?;
        let nope = w("nope");
        let r = NtQueryValueKey(key, &unicode(&nope), 2, buf.as_mut_ptr(), 256, &mut ret);
        c.eq(r.into(), STATUS_OBJECT_NAME_NOT_FOUND)?;

        // KEY_BASIC_INFORMATION: LastWriteTime, TitleIndex, NameLength, Name.
        c.eq(NtQueryKey(key, 0, buf.as_mut_ptr(), 256, &mut ret).into(), 0)?;
        c.eq(ret.into(), 16 + 28)?;
        c.eq(u32::from_le_bytes([buf[12], buf[13], buf[14], buf[15]]).into(), 28)?;
        c.ok(same(&units(&buf[16..44])[..14], "CurrentVersion"), buf[16].into())?;
        // KEY_NAME_INFORMATION: the full name.
        c.eq(NtQueryKey(key, 3, buf.as_mut_ptr(), 256, &mut ret).into(), 0)?;
        let full = "\\REGISTRY\\MACHINE\\SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion";
        c.eq(u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]).into(), 2 * full.len() as u64)?;
        c.ok(same(&units(&buf[4..4 + 2 * full.len()])[..full.len()], full), buf[4].into())?;
        NtClose(key);

        // Enumeration past the end.
        let r = NtEnumerateKey(test, 2, 0, buf.as_mut_ptr(), 256, &mut ret);
        c.eq(r.into(), STATUS_NO_MORE_ENTRIES)?;
        // The first subkey's basic information.
        c.eq(NtEnumerateKey(test, 0, 0, buf.as_mut_ptr(), 256, &mut ret).into(), 0)?;
        c.eq(ret.into(), 16 + 10)?;
        c.ok(same(&units(&buf[16..26])[..5], "alpha"), buf[16].into())?;
        let r = NtEnumerateValueKey(test, 99, 0, buf.as_mut_ptr(), 256, &mut ret);
        c.eq(r.into(), STATUS_NO_MORE_ENTRIES)?;
    }
    Ok(())
}

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    seeded(&mut c)?;
    written(&mut c)?;
    Ok(())
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    let code = match run() {
        Ok(()) => {
            out(b"registry_test: ok\r\n");
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
