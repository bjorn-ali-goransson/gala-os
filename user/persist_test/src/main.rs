//! Files that survive a reboot (D2): what a program writes on C: is there
//! after the machine restarts, as on Windows. xtask boots the same disk
//! twice (`persist` boot option). The first boot finds no C:\persist and
//! creates it; the second finds it and checks everything:
//!
//! - a directory created with a security descriptor (CreateDirectoryW with
//!   SECURITY_ATTRIBUTES): GetFileSecurityW returns its DACL, which
//!   starts with the two ACEs given (Everyone: all access, BUILTIN\Users:
//!   read and execute), followed by what C:\ passes on;
//! - a small file's contents, its attributes (SetFileAttributesW: hidden)
//!   and its last-write time (SetFileTime);
//! - a 200,000-byte file written in pieces, read back byte for byte;
//! - a file truncated with SetEndOfFile (its size);
//! - a renamed file (MoveFileExW: only the new name exists) and a deleted
//!   one (DeleteFileW: gone);
//! - nested directories (sub\deep\x.txt) and FindFirstFileW's view of
//!   C:\persist;
//! - the registry (D3): HKCU\Software\GalaPersist with a REG_SZ, a
//!   REG_DWORD and a REG_BINARY value (RegQueryValueExW: type and data), a
//!   subkey with a value; a value and a subkey deleted in the first boot
//!   are gone (ERROR_FILE_NOT_FOUND), and so is a volatile subkey
//!   (REG_OPTION_VOLATILE: volatile keys do not survive a restart).
//!
//! Prints `persist_test: first boot` the first time, then `persist_test:
//! ok` or exits with the number of the failing check.

#![no_std]
#![no_main]

use core::ffi::c_void;

type Bool = i32;
type Handle = *mut c_void;

#[repr(C)]
struct SecurityAttributes {
    length: u32,
    descriptor: *mut c_void,
    inherit: Bool,
}

#[repr(C)]
struct FindData {
    attributes: u32,
    // FILETIMEs: pairs of DWORDs (4-byte aligned).
    times: [u32; 6],
    size_high: u32,
    size_low: u32,
    reserved: [u32; 2],
    name: [u16; 260],
    alternate: [u16; 14],
}

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn CreateFileW(
        name: *const u16,
        access: u32,
        share: u32,
        security: *const SecurityAttributes,
        disposition: u32,
        flags: u32,
        template: Handle,
    ) -> Handle;
    fn CreateDirectoryW(name: *const u16, security: *const SecurityAttributes) -> Bool;
    fn CloseHandle(h: Handle) -> Bool;
    fn ReadFile(h: Handle, buf: *mut u8, len: u32, read: *mut u32, ov: *mut c_void) -> Bool;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, ov: *mut c_void) -> Bool;
    fn SetFilePointerEx(h: Handle, distance: i64, new: *mut i64, method: u32) -> Bool;
    fn SetEndOfFile(h: Handle) -> Bool;
    fn GetFileSizeEx(h: Handle, size: *mut i64) -> Bool;
    fn SetFileTime(
        h: Handle,
        created: *const u64,
        accessed: *const u64,
        written: *const u64,
    ) -> Bool;
    fn GetFileTime(h: Handle, created: *mut u64, accessed: *mut u64, written: *mut u64) -> Bool;
    fn SetFileAttributesW(name: *const u16, attributes: u32) -> Bool;
    fn GetFileAttributesW(name: *const u16) -> u32;
    fn MoveFileExW(from: *const u16, to: *const u16, flags: u32) -> Bool;
    fn DeleteFileW(name: *const u16) -> Bool;
    fn FindFirstFileW(pattern: *const u16, data: *mut FindData) -> Handle;
    fn FindNextFileW(h: Handle, data: *mut FindData) -> Bool;
    fn FindClose(h: Handle) -> Bool;
    fn GetLastError() -> u32;
    fn GetStdHandle(which: u32) -> Handle;
    fn ExitProcess(code: u32) -> !;
}

#[link(name = "advapi32", kind = "raw-dylib")]
unsafe extern "system" {
    fn RegCreateKeyExW(
        key: Handle,
        subkey: *const u16,
        reserved: u32,
        class: *const u16,
        options: u32,
        access: u32,
        security: *const c_void,
        result: *mut Handle,
        disposition: *mut u32,
    ) -> u32;
    fn RegOpenKeyExW(
        key: Handle,
        subkey: *const u16,
        options: u32,
        access: u32,
        out: *mut Handle,
    ) -> u32;
    fn RegSetValueExW(
        key: Handle,
        name: *const u16,
        reserved: u32,
        kind: u32,
        data: *const u8,
        len: u32,
    ) -> u32;
    fn RegQueryValueExW(
        key: Handle,
        name: *const u16,
        reserved: *mut u32,
        kind: *mut u32,
        data: *mut u8,
        len: *mut u32,
    ) -> u32;
    fn RegDeleteValueW(key: Handle, name: *const u16) -> u32;
    fn RegDeleteKeyW(key: Handle, subkey: *const u16) -> u32;
    fn RegCloseKey(key: Handle) -> u32;
    fn InitializeSecurityDescriptor(sd: *mut c_void, revision: u32) -> Bool;
    fn InitializeAcl(acl: *mut c_void, len: u32, revision: u32) -> Bool;
    fn AddAccessAllowedAce(acl: *mut c_void, revision: u32, mask: u32, sid: *const c_void) -> Bool;
    fn SetSecurityDescriptorDacl(
        sd: *mut c_void,
        present: Bool,
        acl: *mut c_void,
        def: Bool,
    ) -> Bool;
    fn CreateWellKnownSid(
        kind: u32,
        domain: *const c_void,
        sid: *mut c_void,
        len: *mut u32,
    ) -> Bool;
    fn GetFileSecurityW(
        name: *const u16,
        info: u32,
        sd: *mut c_void,
        len: u32,
        needed: *mut u32,
    ) -> Bool;
    fn GetSecurityDescriptorDacl(
        sd: *const c_void,
        present: *mut Bool,
        acl: *mut *mut c_void,
        defaulted: *mut Bool,
    ) -> Bool;
    fn GetAce(acl: *const c_void, index: u32, ace: *mut *mut c_void) -> Bool;
    fn EqualSid(a: *const c_void, b: *const c_void) -> Bool;
}

// The compiler's memcpy/memset/memcmp, from ntdll.
#[allow(dead_code)]
#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "C" {
    fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memset(dst: *mut u8, value: i32, n: usize) -> *mut u8;
    fn memcmp(a: *const u8, b: *const u8, n: usize) -> i32;
}

// What the compiler's output expects from a C runtime.
#[unsafe(no_mangle)]
pub static _fltused: i32 = 0;

#[unsafe(no_mangle)]
pub extern "C" fn __CxxFrameHandler3() -> ! {
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xdeac) }
}

const GENERIC_READ: u32 = 0x8000_0000;
const GENERIC_WRITE: u32 = 0x4000_0000;
const CREATE_ALWAYS: u32 = 2;
const OPEN_EXISTING: u32 = 3;
const FILE_BEGIN: u32 = 0;
const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;
const FILE_ATTRIBUTE_ARCHIVE: u32 = 0x20;
const INVALID_FILE_ATTRIBUTES: u32 = u32::MAX;
const DACL_SECURITY_INFORMATION: u32 = 4;
const ACL_REVISION: u32 = 2;
const WIN_WORLD_SID: u32 = 1;
const WIN_BUILTIN_USERS_SID: u32 = 27;
const FILE_ALL_ACCESS: u32 = 0x001f_01ff;
const FILE_GENERIC_READ_EXECUTE: u32 = 0x0012_00a9;
const ERROR_FILE_NOT_FOUND: u64 = 2;

const HKEY_CURRENT_USER: Handle = 0x8000_0001usize as Handle;
const KEY_ALL_ACCESS: u32 = 0x000f_003f;
const REG_OPTION_VOLATILE: u32 = 1;
const REG_SZ: u32 = 1;
const REG_BINARY: u32 = 3;
const REG_DWORD: u32 = 4;
/// "gala" as REG_SZ data (UTF-16 with its NUL).
const NAME: [u8; 10] = [b'g', 0, b'a', 0, b'l', 0, b'a', 0, 0, 0];
const BLOB: [u8; 5] = [1, 2, 3, 0xfe, 0xff];

const BIG: usize = 200_000;
const TEXT: &[u8] = b"hello from the first boot\r\n";
/// A fixed last-write time (a FILETIME).
const WRITTEN: u64 = 0x01d9_1234_5678_9abc;

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
        out(b"persist_test: check ");
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

/// A NUL-terminated UTF-16 path.
struct W([u16; 64]);

fn w(s: &str) -> W {
    let mut p = [0u16; 64];
    for (d, b) in p.iter_mut().zip(s.bytes()) {
        *d = u16::from(b);
    }
    W(p)
}

impl W {
    fn p(&self) -> *const u16 {
        self.0.as_ptr()
    }
}

fn last_error() -> u64 {
    // SAFETY: no arguments.
    unsafe { GetLastError() }.into()
}

/// The pattern of the big file.
fn pattern(i: usize) -> u8 {
    (i * 7 % 251) as u8
}

fn create(path: &str) -> Handle {
    // SAFETY: a valid path.
    unsafe {
        CreateFileW(
            w(path).p(),
            GENERIC_READ | GENERIC_WRITE,
            0,
            core::ptr::null(),
            CREATE_ALWAYS,
            0,
            core::ptr::null_mut(),
        )
    }
}

fn open(path: &str) -> Handle {
    // SAFETY: a valid path.
    unsafe {
        CreateFileW(
            w(path).p(),
            GENERIC_READ | GENERIC_WRITE,
            0,
            core::ptr::null(),
            OPEN_EXISTING,
            0,
            core::ptr::null_mut(),
        )
    }
}

fn write(h: Handle, data: &[u8]) -> bool {
    let mut n = 0;
    // SAFETY: valid buffer and count.
    let ok =
        unsafe { WriteFile(h, data.as_ptr(), data.len() as u32, &mut n, core::ptr::null_mut()) };
    ok != 0 && n as usize == data.len()
}

fn read(h: Handle, buf: &mut [u8]) -> Option<usize> {
    let mut n = 0;
    // SAFETY: valid buffer and count.
    let ok =
        unsafe { ReadFile(h, buf.as_mut_ptr(), buf.len() as u32, &mut n, core::ptr::null_mut()) };
    (ok != 0).then_some(n as usize)
}

/// Everyone: all access; BUILTIN\Users: read and execute.
fn make_descriptor(c: &mut Checks, sd: &mut [u64; 4], acl: &mut [u64; 16]) -> Result<(), u32> {
    let (mut world, mut users) = ([0u64; 9], [0u64; 9]);
    let (mut wl, mut ul) = (72u32, 72u32);
    // SAFETY: buffers of the sizes given.
    unsafe {
        let null = core::ptr::null();
        c.ok(
            CreateWellKnownSid(WIN_WORLD_SID, null, world.as_mut_ptr().cast(), &mut wl) != 0,
            last_error(),
        )?;
        c.ok(
            CreateWellKnownSid(WIN_BUILTIN_USERS_SID, null, users.as_mut_ptr().cast(), &mut ul)
                != 0,
            last_error(),
        )?;
        let a = acl.as_mut_ptr().cast();
        c.ok(InitializeAcl(a, 128, ACL_REVISION) != 0, last_error())?;
        c.ok(
            AddAccessAllowedAce(a, ACL_REVISION, FILE_ALL_ACCESS, world.as_ptr().cast()) != 0,
            last_error(),
        )?;
        c.ok(
            AddAccessAllowedAce(a, ACL_REVISION, FILE_GENERIC_READ_EXECUTE, users.as_ptr().cast())
                != 0,
            last_error(),
        )?;
        let s = sd.as_mut_ptr().cast();
        c.ok(InitializeSecurityDescriptor(s, 1) != 0, last_error())?;
        c.ok(SetSecurityDescriptorDacl(s, 1, a, 0) != 0, last_error())?;
    }
    Ok(())
}

fn first_boot(c: &mut Checks) -> Result<(), u32> {
    let (mut sd, mut acl) = ([0u64; 4], [0u64; 16]);
    make_descriptor(c, &mut sd, &mut acl)?;
    let sa = SecurityAttributes { length: 24, descriptor: sd.as_mut_ptr().cast(), inherit: 0 };
    // SAFETY: valid paths, handles and buffers throughout.
    unsafe {
        c.ok(CreateDirectoryW(w("C:\\persist").p(), &sa) != 0, last_error())?;

        let h = create("C:\\persist\\a.txt");
        c.ok(h as isize != -1, last_error())?;
        c.ok(write(h, TEXT), last_error())?;
        c.ok(SetFileTime(h, core::ptr::null(), core::ptr::null(), &WRITTEN) != 0, last_error())?;
        CloseHandle(h);
        let attributes = FILE_ATTRIBUTE_HIDDEN | FILE_ATTRIBUTE_ARCHIVE;
        c.ok(SetFileAttributesW(w("C:\\persist\\a.txt").p(), attributes) != 0, last_error())?;

        // The big file, in three pieces of different sizes.
        static mut DATA: [u8; BIG] = [0; BIG];
        let data = &mut *core::ptr::addr_of_mut!(DATA);
        for (i, b) in data.iter_mut().enumerate() {
            *b = pattern(i);
        }
        let h = create("C:\\persist\\big.bin");
        c.ok(h as isize != -1, last_error())?;
        c.ok(write(h, &data[..1000]), last_error())?;
        c.ok(write(h, &data[1000..70_000]), last_error())?;
        c.ok(write(h, &data[70_000..]), last_error())?;
        CloseHandle(h);

        let h = create("C:\\persist\\trunc.bin");
        c.ok(h as isize != -1, last_error())?;
        c.ok(write(h, &data[..10_000]), last_error())?;
        c.ok(SetFilePointerEx(h, 3000, core::ptr::null_mut(), FILE_BEGIN) != 0, last_error())?;
        c.ok(SetEndOfFile(h) != 0, last_error())?;
        CloseHandle(h);

        let h = create("C:\\persist\\old.txt");
        c.ok(write(h, b"renamed"), last_error())?;
        CloseHandle(h);
        c.ok(
            MoveFileExW(w("C:\\persist\\old.txt").p(), w("C:\\persist\\new.txt").p(), 0) != 0,
            last_error(),
        )?;

        let h = create("C:\\persist\\gone.txt");
        c.ok(write(h, b"deleted"), last_error())?;
        CloseHandle(h);
        c.ok(DeleteFileW(w("C:\\persist\\gone.txt").p()) != 0, last_error())?;

        let null = core::ptr::null();
        c.ok(CreateDirectoryW(w("C:\\persist\\sub").p(), null) != 0, last_error())?;
        c.ok(CreateDirectoryW(w("C:\\persist\\sub\\deep").p(), null) != 0, last_error())?;
        let h = create("C:\\persist\\sub\\deep\\x.txt");
        c.ok(write(h, b"deep"), last_error())?;
        CloseHandle(h);
    }
    Ok(())
}

fn check_descriptor(c: &mut Checks) -> Result<(), u32> {
    let mut buf = [0u64; 64];
    let mut needed = 0;
    let (mut world, mut wl) = ([0u64; 9], 72u32);
    // SAFETY: buffers of the sizes given; ACE pointers point into `buf`.
    unsafe {
        let ok = GetFileSecurityW(
            w("C:\\persist").p(),
            DACL_SECURITY_INFORMATION,
            buf.as_mut_ptr().cast(),
            512,
            &mut needed,
        );
        c.ok(ok != 0, last_error())?;
        let (mut present, mut defaulted) = (0, 0);
        let mut acl = core::ptr::null_mut();
        c.ok(
            GetSecurityDescriptorDacl(buf.as_ptr().cast(), &mut present, &mut acl, &mut defaulted)
                != 0,
            last_error(),
        )?;
        c.ok(present != 0 && !acl.is_null(), 0)?;
        // ACL { AclRevision, Sbz1, AclSize, AceCount, Sbz2 }
        // Our two ACEs first, then what C:\ passes on (the DACL is not
        // protected).
        let count = *acl.cast::<u16>().add(2);
        c.ok(count >= 2, count.into())?;
        let mut ace = core::ptr::null_mut();
        c.ok(GetAce(acl, 0, &mut ace) != 0, last_error())?;
        // ACCESS_ALLOWED_ACE { Header (4 bytes), Mask, SidStart }
        let mask = *ace.cast::<u32>().add(1);
        c.eq(mask.into(), FILE_ALL_ACCESS.into())?;
        c.ok(
            CreateWellKnownSid(
                WIN_WORLD_SID,
                core::ptr::null(),
                world.as_mut_ptr().cast(),
                &mut wl,
            ) != 0,
            last_error(),
        )?;
        c.ok(EqualSid(ace.cast::<u8>().add(8).cast(), world.as_ptr().cast()) != 0, 0)?;
        c.ok(GetAce(acl, 1, &mut ace) != 0, last_error())?;
        c.eq((*ace.cast::<u32>().add(1)).into(), FILE_GENERIC_READ_EXECUTE.into())?;
    }
    Ok(())
}

fn second_boot(c: &mut Checks) -> Result<(), u32> {
    check_descriptor(c)?;
    let mut buf = [0u8; 64];
    // SAFETY: valid paths, handles and buffers throughout.
    unsafe {
        let h = open("C:\\persist\\a.txt");
        c.ok(h as isize != -1, last_error())?;
        let n = read(h, &mut buf).unwrap_or(0);
        c.ok(&buf[..n] == TEXT, n as u64)?;
        let mut written = 0;
        c.ok(
            GetFileTime(h, core::ptr::null_mut(), core::ptr::null_mut(), &mut written) != 0,
            last_error(),
        )?;
        c.eq(written, WRITTEN)?;
        CloseHandle(h);
        let attributes = GetFileAttributesW(w("C:\\persist\\a.txt").p());
        c.eq(attributes.into(), (FILE_ATTRIBUTE_HIDDEN | FILE_ATTRIBUTE_ARCHIVE).into())?;

        static mut DATA: [u8; BIG] = [0; BIG];
        let data = &mut *core::ptr::addr_of_mut!(DATA);
        let h = open("C:\\persist\\big.bin");
        c.ok(h as isize != -1, last_error())?;
        let mut size = 0;
        c.ok(GetFileSizeEx(h, &mut size) != 0 && size as usize == BIG, size as u64)?;
        c.eq(read(h, data).unwrap_or(0) as u64, BIG as u64)?;
        let bad = data.iter().enumerate().position(|(i, &b)| b != pattern(i));
        c.ok(bad.is_none(), bad.unwrap_or(0) as u64)?;
        CloseHandle(h);

        let h = open("C:\\persist\\trunc.bin");
        c.ok(h as isize != -1, last_error())?;
        c.ok(GetFileSizeEx(h, &mut size) != 0 && size == 3000, size as u64)?;
        CloseHandle(h);

        let h = open("C:\\persist\\new.txt");
        c.ok(h as isize != -1, last_error())?;
        let n = read(h, &mut buf).unwrap_or(0);
        c.ok(&buf[..n] == b"renamed", n as u64)?;
        CloseHandle(h);
        c.eq(
            GetFileAttributesW(w("C:\\persist\\old.txt").p()).into(),
            INVALID_FILE_ATTRIBUTES.into(),
        )?;
        c.eq(last_error(), ERROR_FILE_NOT_FOUND)?;
        c.eq(
            GetFileAttributesW(w("C:\\persist\\gone.txt").p()).into(),
            INVALID_FILE_ATTRIBUTES.into(),
        )?;

        let h = open("C:\\persist\\sub\\deep\\x.txt");
        c.ok(h as isize != -1, last_error())?;
        let n = read(h, &mut buf).unwrap_or(0);
        c.ok(&buf[..n] == b"deep", n as u64)?;
        CloseHandle(h);

        // C:\persist holds a.txt, big.bin, new.txt, sub, trunc.bin (and
        // . and ..), in that (collation) order.
        let mut find: FindData = core::mem::zeroed();
        let f = FindFirstFileW(w("C:\\persist\\*").p(), &mut find);
        c.ok(f as isize != -1, last_error())?;
        let want: [&[u8]; 7] =
            [b".", b"..", b"a.txt", b"big.bin", b"new.txt", b"sub", b"trunc.bin"];
        let mut i = 0;
        loop {
            let name = &find.name;
            let len = name.iter().position(|&u| u == 0).unwrap_or(0);
            let same = i < want.len()
                && len == want[i].len()
                && name[..len].iter().zip(want[i]).all(|(&a, &b)| a == u16::from(b));
            c.ok(same, i as u64)?;
            if want[i] == b"sub" {
                c.ok(find.attributes & FILE_ATTRIBUTE_DIRECTORY != 0, find.attributes.into())?;
            }
            i += 1;
            if FindNextFileW(f, &mut find) == 0 {
                break;
            }
        }
        c.eq(i as u64, want.len() as u64)?;
        FindClose(f);
    }
    Ok(())
}

fn reg_create(parent: Handle, name: &str, options: u32) -> Result<Handle, u32> {
    let mut key = core::ptr::null_mut();
    // SAFETY: valid name and output pointers.
    let e = unsafe {
        RegCreateKeyExW(
            parent,
            w(name).p(),
            0,
            core::ptr::null(),
            options,
            KEY_ALL_ACCESS,
            core::ptr::null(),
            &mut key,
            core::ptr::null_mut(),
        )
    };
    if e == 0 { Ok(key) } else { Err(e) }
}

fn reg_set(key: Handle, name: &str, kind: u32, data: &[u8]) -> u32 {
    // SAFETY: valid name and data.
    unsafe { RegSetValueExW(key, w(name).p(), 0, kind, data.as_ptr(), data.len() as u32) }
}

fn registry_first(c: &mut Checks) -> Result<(), u32> {
    let key = reg_create(HKEY_CURRENT_USER, "Software\\GalaPersist", 0);
    c.ok(key.is_ok(), key.err().unwrap_or(0).into())?;
    let key = key.unwrap_or(core::ptr::null_mut());
    c.eq(reg_set(key, "Name", REG_SZ, &NAME).into(), 0)?;
    c.eq(reg_set(key, "Count", REG_DWORD, &42u32.to_le_bytes()).into(), 0)?;
    c.eq(reg_set(key, "Blob", REG_BINARY, &BLOB).into(), 0)?;
    c.eq(reg_set(key, "Gone", REG_DWORD, &1u32.to_le_bytes()).into(), 0)?;
    // SAFETY: a valid key and names throughout.
    unsafe {
        c.eq(RegDeleteValueW(key, w("Gone").p()).into(), 0)?;
        let sub = reg_create(key, "Sub", 0);
        c.ok(sub.is_ok(), 0)?;
        let sub = sub.unwrap_or(core::ptr::null_mut());
        c.eq(reg_set(sub, "Inner", REG_DWORD, &7u32.to_le_bytes()).into(), 0)?;
        RegCloseKey(sub);
        let temp = reg_create(key, "Temp", 0);
        c.ok(temp.is_ok(), 0)?;
        RegCloseKey(temp.unwrap_or(core::ptr::null_mut()));
        c.eq(RegDeleteKeyW(key, w("Temp").p()).into(), 0)?;
        let volatile = reg_create(key, "Volatile", REG_OPTION_VOLATILE);
        c.ok(volatile.is_ok(), 0)?;
        let volatile = volatile.unwrap_or(core::ptr::null_mut());
        c.eq(reg_set(volatile, "Lost", REG_DWORD, &1u32.to_le_bytes()).into(), 0)?;
        RegCloseKey(volatile);
        RegCloseKey(key);
    }
    Ok(())
}

/// Value `name` of `key` has type `kind` and data `want`.
fn reg_check(c: &mut Checks, key: Handle, name: &str, kind: u32, want: &[u8]) -> Result<(), u32> {
    let (mut got_kind, mut buf, mut len) = (0u32, [0u8; 32], 32u32);
    // SAFETY: valid buffers.
    let e = unsafe {
        RegQueryValueExW(
            key,
            w(name).p(),
            core::ptr::null_mut(),
            &mut got_kind,
            buf.as_mut_ptr(),
            &mut len,
        )
    };
    c.eq(e.into(), 0)?;
    c.eq(got_kind.into(), kind.into())?;
    c.ok(&buf[..len as usize] == want, len.into())
}

fn registry_second(c: &mut Checks) -> Result<(), u32> {
    let mut key = core::ptr::null_mut();
    // SAFETY: valid names, buffers and keys throughout.
    unsafe {
        let e = RegOpenKeyExW(
            HKEY_CURRENT_USER,
            w("Software\\GalaPersist").p(),
            0,
            KEY_ALL_ACCESS,
            &mut key,
        );
        c.eq(e.into(), 0)?;
        reg_check(c, key, "Name", REG_SZ, &NAME)?;
        reg_check(c, key, "Count", REG_DWORD, &42u32.to_le_bytes())?;
        reg_check(c, key, "Blob", REG_BINARY, &BLOB)?;
        let mut len = 4u32;
        let mut buf = [0u8; 4];
        let e = RegQueryValueExW(
            key,
            w("Gone").p(),
            core::ptr::null_mut(),
            core::ptr::null_mut(),
            buf.as_mut_ptr(),
            &mut len,
        );
        c.eq(e.into(), ERROR_FILE_NOT_FOUND)?;
        let mut sub = core::ptr::null_mut();
        c.eq(RegOpenKeyExW(key, w("Sub").p(), 0, KEY_ALL_ACCESS, &mut sub).into(), 0)?;
        reg_check(c, sub, "Inner", REG_DWORD, &7u32.to_le_bytes())?;
        RegCloseKey(sub);
        let mut other = core::ptr::null_mut();
        c.eq(
            RegOpenKeyExW(key, w("Temp").p(), 0, KEY_ALL_ACCESS, &mut other).into(),
            ERROR_FILE_NOT_FOUND,
        )?;
        c.eq(
            RegOpenKeyExW(key, w("Volatile").p(), 0, KEY_ALL_ACCESS, &mut other).into(),
            ERROR_FILE_NOT_FOUND,
        )?;
        RegCloseKey(key);
    }
    Ok(())
}

fn run() -> Result<bool, u32> {
    let mut c = Checks(0);
    // SAFETY: a valid path.
    let exists = unsafe { GetFileAttributesW(w("C:\\persist").p()) } != INVALID_FILE_ATTRIBUTES;
    if exists {
        second_boot(&mut c)?;
        registry_second(&mut c)?;
    } else {
        first_boot(&mut c)?;
        registry_first(&mut c)?;
    }
    Ok(exists)
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    let code = match run() {
        Ok(true) => {
            out(b"persist_test: ok\r\n");
            0
        }
        Ok(false) => {
            out(b"persist_test: first boot\r\n");
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
