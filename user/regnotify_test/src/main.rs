//! Registry change notification and key rename (ROADMAP OF: Office's
//! ntdll imports NtNotifyChangeMultipleKeys and NtRenameKey; programs
//! watch settings with RegNotifyChangeKeyValue). Expected values are those
//! of Wine's conformance tests, which pass on Windows (ntdll's
//! `test_notify` and `test_NtRenameKey`, advapi32's
//! `test_RegNotifyChangeKeyValue` and `test_RegRenameKey`), and what
//! RegNotifyChangeKeyValue's documentation states:
//!
//! - NtNotifyChangeKey: STATUS_PENDING; the first request on a handle
//!   fixes what it watches (a later one's filter changes nothing, a
//!   filter of 0 never fires); a subkey created or deleted completes
//!   every waiting request of a NAME watch on the parent, once (a request
//!   is one arm; its event is reset when it is made); closing the handle
//!   completes them too; NtNotifyChangeMultipleKeys without subordinates
//!   is the same. A handle without KEY_NOTIFY is STATUS_ACCESS_DENIED; a
//!   handle to a deleted key STATUS_KEY_DELETED.
//! - What fires what: REG_NOTIFY_CHANGE_LAST_SET a value set on the key
//!   (not a NAME-only watch), REG_NOTIFY_CHANGE_SECURITY the key's
//!   descriptor set; a watch of the subtree sees subkeys' subkeys and
//!   values, one without it does not.
//! - RegNotifyChangeKeyValue: asynchronous with an event; a filter bit
//!   outside REG_LEGAL_CHANGE_FILTER is ERROR_INVALID_PARAMETER; the
//!   request ends with its thread (the event is signaled when the thread
//!   that made it exits) unless made with REG_NOTIFY_THREAD_AGNOSTIC; a
//!   synchronous one returns ERROR_SUCCESS once a change came (another
//!   thread makes it).
//! - A completion routine (ApcRoutine) runs, with its context, at the
//!   thread's next alertable wait.
//! - NtRenameKey: STATUS_ACCESS_VIOLATION for a NULL name,
//!   STATUS_INVALID_PARAMETER for an empty one, STATUS_INVALID_HANDLE for
//!   a NULL handle, STATUS_ACCESS_DENIED without KEY_WRITE,
//!   STATUS_CANNOT_DELETE to its own name; renamed, the handle's
//!   KeyNameInformation has the new name and enumeration the new names.
//!   RegRenameKey: ERROR_INVALID_PARAMETER (no names), ERROR_INVALID_HANDLE
//!   (no key), ERROR_ACCESS_DENIED (KEY_READ), ERROR_FILE_NOT_FOUND (no
//!   such subkey), a subkey renamed by name.
//!
//! What the tests and the documentation leave open is printed as
//! `<name>: info ...` lines for a run on Windows to show, not checked:
//! the IO_STATUS_BLOCK's status after a change, a close and a thread's
//! end; whether a change while no request waits completes the next one
//! at once; a synchronous NtNotifyChangeKey's status; a deleted watched
//! key firing its own watch; a rename firing the parent's and its own
//! watch; another handle's name after a rename; a security change firing
//! an ATTRIBUTES watch; NtNotifyChangeMultipleKeys with one subordinate
//! (whose change fires the master's request) and with two; an
//! asynchronous RegNotifyChangeKeyValue without an event.
//!
//! The keys are volatile, under `HKCU\Software\Gala<name>`, deleted at
//! the end. The same source builds `wow64_regnotify_test`, the 32-bit
//! (x86) program. Prints `regnotify_test: ok` (`wow64_regnotify_test:
//! ok`), or `<name>: check N got X (line L)` and exits with N.

#![no_std]
#![no_main]

use core::ptr::{null, null_mut};
use core::sync::atomic::{AtomicUsize, Ordering};

use ntrt as _;

#[cfg(target_arch = "x86_64")]
const NAME: &str = "regnotify_test";
#[cfg(target_arch = "x86")]
const NAME: &str = "wow64_regnotify_test";

type Handle = usize;

#[repr(C)]
struct UnicodeString {
    length: u16,
    maximum: u16,
    buffer: *const u16,
}

#[repr(C)]
struct ObjectAttributes {
    length: u32,
    root: Handle,
    name: *const UnicodeString,
    attributes: u32,
    security: usize,
    qos: usize,
}

/// IO_STATUS_BLOCK (Status, Information).
type Iosb = [usize; 2];

#[cfg_attr(
    target_arch = "x86",
    link(name = "kernel32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "kernel32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, overlapped: usize) -> i32;
    fn ExitProcess(code: u32) -> !;
    fn CreateEventW(security: usize, manual: i32, initial: i32, name: *const u16) -> Handle;
    fn WaitForSingleObject(h: Handle, ms: u32) -> u32;
    fn WaitForMultipleObjects(count: u32, handles: *const Handle, all: i32, ms: u32) -> u32;
    fn CloseHandle(h: Handle) -> i32;
    fn CreateThread(
        security: usize,
        stack: usize,
        start: extern "system" fn(usize) -> u32,
        param: usize,
        flags: u32,
        id: *mut u32,
    ) -> Handle;
    fn SleepEx(ms: u32, alertable: i32) -> u32;
    fn Sleep(ms: u32);
}

#[cfg_attr(
    target_arch = "x86",
    link(name = "advapi32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "advapi32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn RegCreateKeyExW(
        key: Handle,
        sub: *const u16,
        reserved: u32,
        class: *const u16,
        options: u32,
        access: u32,
        security: usize,
        out: *mut Handle,
        disposition: *mut u32,
    ) -> u32;
    fn RegOpenKeyExW(
        key: Handle,
        sub: *const u16,
        options: u32,
        access: u32,
        out: *mut Handle,
    ) -> u32;
    fn RegCloseKey(key: Handle) -> u32;
    fn RegSetValueExW(
        key: Handle,
        name: *const u16,
        reserved: u32,
        kind: u32,
        data: *const u8,
        size: u32,
    ) -> u32;
    fn RegDeleteKeyW(key: Handle, sub: *const u16) -> u32;
    fn RegDeleteTreeW(key: Handle, sub: *const u16) -> u32;
    fn RegNotifyChangeKeyValue(
        key: Handle,
        subtree: i32,
        filter: u32,
        event: Handle,
        asynchronous: i32,
    ) -> u32;
    fn RegRenameKey(key: Handle, sub: *const u16, new_name: *const u16) -> u32;
    fn RegGetKeySecurity(key: Handle, info: u32, sd: *mut u8, size: *mut u32) -> u32;
    fn RegSetKeySecurity(key: Handle, info: u32, sd: *const u8) -> u32;
}

#[cfg_attr(
    target_arch = "x86",
    link(name = "ntdll", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "ntdll", kind = "raw-dylib"))]
unsafe extern "system" {
    fn NtNotifyChangeKey(
        key: Handle,
        event: Handle,
        apc: usize,
        apc_context: usize,
        iosb: *mut Iosb,
        filter: u32,
        tree: u8,
        buffer: *mut u8,
        length: u32,
        asynchronous: u8,
    ) -> u32;
    fn NtNotifyChangeMultipleKeys(
        key: Handle,
        count: u32,
        subordinate: *const ObjectAttributes,
        event: Handle,
        apc: usize,
        apc_context: usize,
        iosb: *mut Iosb,
        filter: u32,
        tree: u8,
        buffer: *mut u8,
        length: u32,
        asynchronous: u8,
    ) -> u32;
    fn NtRenameKey(key: Handle, name: *const UnicodeString) -> u32;
    fn NtCreateKey(
        out: *mut Handle,
        access: u32,
        attributes: *const ObjectAttributes,
        title: u32,
        class: *const UnicodeString,
        options: u32,
        disposition: *mut u32,
    ) -> u32;
    fn NtDeleteKey(key: Handle) -> u32;
    fn NtQueryKey(key: Handle, class: u32, buf: *mut u8, len: u32, ret: *mut u32) -> u32;
    fn NtEnumerateKey(
        key: Handle,
        index: u32,
        class: u32,
        buf: *mut u8,
        len: u32,
        ret: *mut u32,
    ) -> u32;
    fn NtClose(h: Handle) -> u32;
}

const HKEY_CURRENT_USER: Handle = 0x8000_0001;
const KEY_QUERY_VALUE: u32 = 0x0001;
const KEY_NOTIFY: u32 = 0x0010;
const DELETE: u32 = 0x0001_0000;
const KEY_READ: u32 = 0x0002_0019;
const KEY_WRITE: u32 = 0x0002_0006;
const KEY_ALL_ACCESS: u32 = 0x000f_003f;
const REG_OPTION_VOLATILE: u32 = 1;
const REG_DWORD: u32 = 4;
const OBJ_CASE_INSENSITIVE: u32 = 0x40;
const DACL_SECURITY_INFORMATION: u32 = 4;

const REG_NOTIFY_CHANGE_NAME: u32 = 1;
const REG_NOTIFY_CHANGE_ATTRIBUTES: u32 = 2;
const REG_NOTIFY_CHANGE_LAST_SET: u32 = 4;
const REG_NOTIFY_CHANGE_SECURITY: u32 = 8;
const REG_NOTIFY_THREAD_AGNOSTIC: u32 = 0x1000_0000;

const STATUS_SUCCESS: u32 = 0;
const STATUS_PENDING: u32 = 0x103;
const STATUS_ACCESS_VIOLATION: u32 = 0xc000_0005;
const STATUS_INVALID_HANDLE: u32 = 0xc000_0008;
const STATUS_INVALID_PARAMETER: u32 = 0xc000_000d;
const STATUS_ACCESS_DENIED: u32 = 0xc000_0022;
const STATUS_CANNOT_DELETE: u32 = 0xc000_0121;
const STATUS_KEY_DELETED: u32 = 0xc000_017c;
const STATUS_NO_MORE_ENTRIES: u32 = 0x8000_001a;

const ERROR_SUCCESS: u32 = 0;
const ERROR_FILE_NOT_FOUND: u32 = 2;
const ERROR_ACCESS_DENIED: u32 = 5;
const ERROR_INVALID_HANDLE: u32 = 6;
const ERROR_INVALID_PARAMETER: u32 = 87;

const WAIT_OBJECT_0: u32 = 0;
const WAIT_TIMEOUT: u32 = 258;
const INFINITE: u32 = u32::MAX;
/// What an IO_STATUS_BLOCK holds before the kernel writes it.
const UNTOUCHED: usize = 0x5a5a_5a5a;

struct Line {
    buf: [u8; 400],
    n: usize,
}

impl Line {
    fn new() -> Self {
        Self { buf: [0; 400], n: 0 }
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

/// An `info` line: `what` and a value.
fn info(what: &str, v: u64) {
    Line::new()
        .s(NAME.as_bytes())
        .s(b": info ")
        .s(what.as_bytes())
        .s(b" ")
        .hex(v)
        .s(b"\r\n")
        .print();
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

    #[track_caller]
    fn eq(&mut self, got: u32, want: u32) -> Result<(), u32> {
        self.ok(got == want, got.into())
    }
}

fn wide<const N: usize>(text: &str) -> [u16; N] {
    let mut out = [0u16; N];
    for (slot, b) in out.iter_mut().zip(text.bytes()) {
        *slot = u16::from(b);
    }
    out
}

/// A NUL-terminated name.
type W = [u16; 64];

fn w(text: &str) -> W {
    wide(text)
}

fn len(s: &[u16]) -> usize {
    s.iter().position(|&c| c == 0).unwrap_or(s.len())
}

fn unicode(s: &[u16]) -> UnicodeString {
    let bytes = (2 * len(s)) as u16;
    UnicodeString { length: bytes, maximum: bytes, buffer: s.as_ptr() }
}

fn attributes(root: Handle, name: &UnicodeString) -> ObjectAttributes {
    ObjectAttributes {
        length: size_of::<ObjectAttributes>() as u32,
        root,
        name,
        attributes: OBJ_CASE_INSENSITIVE,
        security: 0,
        qos: 0,
    }
}

/// The base key's path below HKEY_CURRENT_USER: `Software\Gala<name>`.
fn base_path() -> W {
    let mut p = w("Software\\Gala");
    let n = len(&p);
    for (i, b) in NAME.bytes().enumerate() {
        p[n + i] = b.into();
    }
    p
}

/// Creates the volatile key `name` below `parent` with `access`.
fn create(parent: Handle, name: &str, access: u32) -> Result<Handle, u32> {
    let mut h = 0;
    let name = w(name);
    // SAFETY: a NUL-terminated name and our output.
    let e = unsafe {
        RegCreateKeyExW(
            parent,
            name.as_ptr(),
            0,
            null(),
            REG_OPTION_VOLATILE,
            access,
            0,
            &mut h,
            null_mut(),
        )
    };
    if e == ERROR_SUCCESS { Ok(h) } else { Err(e) }
}

fn open(parent: Handle, name: &str, access: u32) -> Result<Handle, u32> {
    let mut h = 0;
    let name = w(name);
    // SAFETY: a NUL-terminated name and our output.
    let e = unsafe { RegOpenKeyExW(parent, name.as_ptr(), 0, access, &mut h) };
    if e == ERROR_SUCCESS { Ok(h) } else { Err(e) }
}

/// NtCreateKey of the volatile `name` below `parent`.
fn nt_create(parent: Handle, name: &str, access: u32) -> Result<Handle, u32> {
    let name = w(name);
    let us = unicode(&name);
    let attr = attributes(parent, &us);
    let mut h = 0;
    // SAFETY: valid attributes and output.
    let s =
        unsafe { NtCreateKey(&mut h, access, &attr, 0, null(), REG_OPTION_VOLATILE, null_mut()) };
    if s == STATUS_SUCCESS { Ok(h) } else { Err(s) }
}

fn set_dword(key: Handle, name: &str, v: u32) -> u32 {
    let name = w(name);
    // SAFETY: a NUL-terminated name and 4 bytes of data.
    unsafe { RegSetValueExW(key, name.as_ptr(), 0, REG_DWORD, v.to_le_bytes().as_ptr(), 4) }
}

fn delete(parent: Handle, name: &str) -> u32 {
    let name = w(name);
    // SAFETY: a NUL-terminated name.
    unsafe { RegDeleteKeyW(parent, name.as_ptr()) }
}

fn close(h: Handle) {
    // SAFETY: our handle.
    unsafe { RegCloseKey(h) };
}

fn event(manual: bool, initial: bool) -> Handle {
    // SAFETY: an unnamed event.
    unsafe { CreateEventW(0, i32::from(manual), i32::from(initial), null()) }
}

fn wait(h: Handle) -> u32 {
    // SAFETY: our handle.
    unsafe { WaitForSingleObject(h, 0) }
}

/// NtNotifyChangeKey, asynchronous, with an event.
fn notify(key: Handle, e: Handle, iosb: &mut Iosb, filter: u32, tree: bool) -> u32 {
    *iosb = [UNTOUCHED; 2];
    // SAFETY: our key, event and IO_STATUS_BLOCK (alive until it completes).
    unsafe { NtNotifyChangeKey(key, e, 0, 0, iosb, filter, u8::from(tree), null_mut(), 0, 1) }
}

/// Wine's ntdll `test_notify`: the first request on a handle sets the
/// filter, every waiting request completes once, a close completes them.
fn nt_notify(c: &mut Checks, base: Handle) -> Result<(), u32> {
    let key = create(base, "notify", KEY_ALL_ACCESS).map_err(|_| c.0 + 1000)?;
    let key2 = open(base, "notify", KEY_ALL_ACCESS).map_err(|_| c.0 + 1000)?;
    let e = [event(true, true), event(true, true), event(true, true), event(true, true)];
    let mut io = [[0usize; 2]; 4];
    let [io0, io1, io2, io3] = &mut io;
    c.eq(notify(key, e[0], io0, REG_NOTIFY_CHANGE_NAME, false), STATUS_PENDING)?;
    c.eq(notify(key, e[1], io1, 0, false), STATUS_PENDING)?;
    c.eq(notify(key2, e[2], io2, 0, false), STATUS_PENDING)?;
    c.eq(notify(key2, e[3], io3, REG_NOTIFY_CHANGE_NAME, false), STATUS_PENDING)?;
    // SAFETY: our events.
    c.eq(unsafe { WaitForMultipleObjects(4, e.as_ptr(), 0, 0) }, WAIT_TIMEOUT)?;
    let sub = nt_create(key, "test_subkey", KEY_ALL_ACCESS).map_err(|s| c.0 + s)?;
    c.eq(wait(e[0]), WAIT_OBJECT_0)?;
    c.eq(wait(e[1]), WAIT_OBJECT_0)?;
    c.eq(wait(e[2]), WAIT_TIMEOUT)?;
    c.eq(wait(e[3]), WAIT_TIMEOUT)?;
    info("IO_STATUS_BLOCK status after a change", io[0][0] as u64);
    info("IO_STATUS_BLOCK information after a change", io[0][1] as u64);
    let [io0, io1, ..] = &mut io;
    c.eq(notify(key, e[0], io0, 0, false), STATUS_PENDING)?;
    c.eq(wait(e[0]), WAIT_TIMEOUT)?;
    c.eq(wait(e[1]), WAIT_OBJECT_0)?;
    c.eq(notify(key, e[1], io1, 0, false), STATUS_PENDING)?;
    // SAFETY: our events.
    c.eq(unsafe { WaitForMultipleObjects(4, e.as_ptr(), 0, 0) }, WAIT_TIMEOUT)?;
    // SAFETY: our key.
    c.eq(unsafe { NtDeleteKey(sub) }, STATUS_SUCCESS)?;
    c.eq(wait(e[0]), WAIT_OBJECT_0)?;
    c.eq(wait(e[1]), WAIT_OBJECT_0)?;
    c.eq(wait(e[2]), WAIT_TIMEOUT)?;
    c.eq(wait(e[3]), WAIT_TIMEOUT)?;
    // A deleted key's handle cannot be watched.
    let mut io_deleted = [0usize; 2];
    c.eq(notify(sub, e[0], &mut io_deleted, REG_NOTIFY_CHANGE_NAME, false), STATUS_KEY_DELETED)?;
    // SAFETY: our key.
    unsafe { NtClose(sub) };
    let [io0, io1, ..] = &mut io;
    c.eq(notify(key, e[0], io0, 0, false), STATUS_PENDING)?;
    c.eq(notify(key, e[1], io1, 0, false), STATUS_PENDING)?;
    close(key);
    c.eq(wait(e[0]), WAIT_OBJECT_0)?;
    c.eq(wait(e[1]), WAIT_OBJECT_0)?;
    c.eq(wait(e[2]), WAIT_TIMEOUT)?;
    c.eq(wait(e[3]), WAIT_TIMEOUT)?;
    info("IO_STATUS_BLOCK status after the handle's close", io[0][0] as u64);
    // NtNotifyChangeMultipleKeys without subordinates.
    let key = open(base, "notify", KEY_ALL_ACCESS).map_err(|_| c.0 + 1000)?;
    let mut iom = [UNTOUCHED; 2];
    // SAFETY: our key, event and IO_STATUS_BLOCK.
    let s = unsafe {
        NtNotifyChangeMultipleKeys(
            key,
            0,
            null(),
            e[0],
            0,
            0,
            &mut iom,
            REG_NOTIFY_CHANGE_NAME,
            0,
            null_mut(),
            0,
            1,
        )
    };
    c.eq(s, STATUS_PENDING)?;
    c.eq(wait(e[0]), WAIT_TIMEOUT)?;
    let sub = nt_create(key, "test_subkey", KEY_ALL_ACCESS).map_err(|s| c.0 + s)?;
    c.eq(wait(e[0]), WAIT_OBJECT_0)?;
    // SAFETY: our key.
    c.eq(unsafe { NtDeleteKey(sub) }, STATUS_SUCCESS)?;
    // SAFETY: our handles.
    unsafe {
        NtClose(sub);
    }
    close(key);
    close(key2);
    for h in e {
        // SAFETY: our event.
        unsafe { CloseHandle(h) };
    }
    Ok(())
}

/// Which changes fire which filters; KEY_NOTIFY is needed.
fn filters(c: &mut Checks, base: Handle) -> Result<(), u32> {
    let key = create(base, "filters", KEY_ALL_ACCESS).map_err(|_| c.0 + 1000)?;
    let e = event(true, false);
    let mut io = [0usize; 2];
    // A value set: LAST_SET fires, NAME does not.
    let name_only = open(base, "filters", KEY_ALL_ACCESS).map_err(|_| c.0 + 1000)?;
    let e2 = event(true, false);
    let mut io2 = [0usize; 2];
    c.eq(notify(name_only, e2, &mut io2, REG_NOTIFY_CHANGE_NAME, false), STATUS_PENDING)?;
    c.eq(notify(key, e, &mut io, REG_NOTIFY_CHANGE_LAST_SET, false), STATUS_PENDING)?;
    c.eq(set_dword(key, "v", 1), ERROR_SUCCESS)?;
    c.eq(wait(e), WAIT_OBJECT_0)?;
    c.eq(wait(e2), WAIT_TIMEOUT)?;
    close(name_only);
    // The descriptor set: SECURITY fires.
    let sec = open(base, "filters", KEY_ALL_ACCESS).map_err(|_| c.0 + 1000)?;
    let attr = open(base, "filters", KEY_ALL_ACCESS).map_err(|_| c.0 + 1000)?;
    let (e3, mut io3) = (event(true, false), [0usize; 2]);
    let (e4, mut io4) = (event(true, false), [0usize; 2]);
    c.eq(notify(sec, e3, &mut io3, REG_NOTIFY_CHANGE_SECURITY, false), STATUS_PENDING)?;
    c.eq(notify(attr, e4, &mut io4, REG_NOTIFY_CHANGE_ATTRIBUTES, false), STATUS_PENDING)?;
    let mut sd = [0u8; 512];
    let mut size = sd.len() as u32;
    // SAFETY: our buffer and its size.
    let r =
        unsafe { RegGetKeySecurity(key, DACL_SECURITY_INFORMATION, sd.as_mut_ptr(), &mut size) };
    c.eq(r, ERROR_SUCCESS)?;
    // SAFETY: the descriptor just read.
    c.eq(unsafe { RegSetKeySecurity(key, DACL_SECURITY_INFORMATION, sd.as_ptr()) }, ERROR_SUCCESS)?;
    c.eq(wait(e3), WAIT_OBJECT_0)?;
    info("a security change fires an ATTRIBUTES watch (0: yes)", wait(e4).into());
    close(sec);
    close(attr);
    // Without KEY_NOTIFY: refused.
    let weak = open(base, "filters", KEY_QUERY_VALUE).map_err(|_| c.0 + 1000)?;
    c.eq(notify(weak, e, &mut io, REG_NOTIFY_CHANGE_NAME, false), STATUS_ACCESS_DENIED)?;
    close(weak);
    // A filter bit outside REG_LEGAL_CHANGE_FILTER.
    let only_notify = open(base, "filters", KEY_NOTIFY).map_err(|_| c.0 + 1000)?;
    c.eq(notify(only_notify, e, &mut io, 0x100, false), STATUS_INVALID_PARAMETER)?;
    // SAFETY: our key and event.
    let r = unsafe { RegNotifyChangeKeyValue(only_notify, 0, 0x20, e, 1) };
    c.eq(r, ERROR_INVALID_PARAMETER)?;
    // SAFETY: as above; no event.
    let r = unsafe { RegNotifyChangeKeyValue(only_notify, 0, REG_NOTIFY_CHANGE_NAME, 0, 1) };
    info("asynchronous RegNotifyChangeKeyValue without an event", r.into());
    close(only_notify);
    // A change while no request waits: does the next one complete at once?
    c.eq(notify(key, e, &mut io, REG_NOTIFY_CHANGE_LAST_SET, false), STATUS_PENDING)?;
    c.eq(set_dword(key, "v", 2), ERROR_SUCCESS)?;
    c.eq(wait(e), WAIT_OBJECT_0)?;
    c.eq(set_dword(key, "v", 3), ERROR_SUCCESS)?;
    let s = notify(key, e, &mut io, REG_NOTIFY_CHANGE_LAST_SET, false);
    info("a request after an unwatched change returns", s.into());
    info("its event (0: signaled)", wait(e).into());
    // Deleting the watched key: does its own watch fire?
    let doomed = create(key, "doomed", KEY_ALL_ACCESS).map_err(|_| c.0 + 1000)?;
    let (e5, mut io5) = (event(true, false), [0usize; 2]);
    c.eq(notify(doomed, e5, &mut io5, REG_NOTIFY_CHANGE_NAME, false), STATUS_PENDING)?;
    c.eq(delete(key, "doomed"), ERROR_SUCCESS)?;
    info("deleting the watched key fires its watch (0: yes)", wait(e5).into());
    info("its IO_STATUS_BLOCK status", io5[0] as u64);
    close(doomed);
    close(key);
    for h in [e, e2, e3, e4, e5] {
        // SAFETY: our event.
        unsafe { CloseHandle(h) };
    }
    Ok(())
}

struct NotifyData {
    key: Handle,
    flags: u32,
    event: Handle,
    result: AtomicUsize,
}

extern "system" fn notify_change_thread(param: usize) -> u32 {
    // SAFETY: the caller's NotifyData, alive until this thread is waited for.
    let data = unsafe { &*(param as *const NotifyData) };
    // SAFETY: the caller's key and event.
    let r = unsafe {
        RegNotifyChangeKeyValue(data.key, 1, REG_NOTIFY_CHANGE_NAME | data.flags, data.event, 1)
    };
    data.result.store(r as usize, Ordering::SeqCst);
    0
}

fn run_thread(start: extern "system" fn(usize) -> u32, param: usize) -> Result<(), u32> {
    let mut id = 0;
    // SAFETY: a thread over `param`, which outlives it (waited for here).
    unsafe {
        let t = CreateThread(0, 0, start, param, 0, &mut id);
        if t == 0 {
            return Err(9999);
        }
        WaitForSingleObject(t, INFINITE);
        CloseHandle(t);
    }
    Ok(())
}

/// Wine's advapi32 `test_RegNotifyChangeKeyValue`.
fn reg_notify(c: &mut Checks, base: Handle) -> Result<(), u32> {
    const FILTER: u32 = REG_NOTIFY_CHANGE_NAME | REG_NOTIFY_CHANGE_LAST_SET;
    let e = event(false, true);
    let key = create(base, "TestKey", KEY_ALL_ACCESS).map_err(|_| c.0 + 1000)?;
    // SAFETY: (throughout) our keys and event.
    c.eq(unsafe { RegNotifyChangeKeyValue(key, 1, FILTER, e, 1) }, ERROR_SUCCESS)?;
    c.eq(wait(e), WAIT_TIMEOUT)?;
    let sub = create(key, "SubKey", KEY_ALL_ACCESS).map_err(|_| c.0 + 1000)?;
    c.eq(wait(e), WAIT_OBJECT_0)?;
    // Watching deeper keys.
    c.eq(unsafe { RegNotifyChangeKeyValue(key, 1, FILTER, e, 1) }, ERROR_SUCCESS)?;
    c.eq(wait(e), WAIT_TIMEOUT)?;
    let subsub = create(sub, "SubKey", KEY_ALL_ACCESS).map_err(|_| c.0 + 1000)?;
    c.eq(wait(e), WAIT_OBJECT_0)?;
    // Watching deeper values.
    c.eq(unsafe { RegNotifyChangeKeyValue(key, 1, FILTER, e, 1) }, ERROR_SUCCESS)?;
    c.eq(wait(e), WAIT_TIMEOUT)?;
    c.eq(set_dword(subsub, "", 1), ERROR_SUCCESS)?;
    c.eq(wait(e), WAIT_OBJECT_0)?;
    // Not watching deeper values (a new handle: the first request fixes
    // the subtree).
    close(key);
    let key = open(base, "TestKey", KEY_ALL_ACCESS).map_err(|_| c.0 + 1000)?;
    c.eq(unsafe { RegNotifyChangeKeyValue(key, 0, FILTER, e, 1) }, ERROR_SUCCESS)?;
    c.eq(wait(e), WAIT_TIMEOUT)?;
    c.eq(set_dword(subsub, "", 2), ERROR_SUCCESS)?;
    c.eq(wait(e), WAIT_TIMEOUT)?;
    c.eq(delete(sub, "SubKey"), ERROR_SUCCESS)?;
    c.eq(delete(key, "SubKey"), ERROR_SUCCESS)?;
    close(subsub);
    close(sub);
    close(key);
    // The same thread with REG_NOTIFY_THREAD_AGNOSTIC.
    let key = open(base, "TestKey", KEY_ALL_ACCESS).map_err(|_| c.0 + 1000)?;
    let r = unsafe {
        RegNotifyChangeKeyValue(key, 1, REG_NOTIFY_CHANGE_NAME | REG_NOTIFY_THREAD_AGNOSTIC, e, 1)
    };
    c.eq(r, ERROR_SUCCESS)?;
    let sub = create(key, "SubKey", KEY_ALL_ACCESS).map_err(|_| c.0 + 1000)?;
    c.eq(wait(e), WAIT_OBJECT_0)?;
    c.eq(delete(key, "SubKey"), ERROR_SUCCESS)?;
    close(sub);
    close(key);
    // Another thread without it: its exit signals the event.
    let key = open(base, "TestKey", KEY_ALL_ACCESS).map_err(|_| c.0 + 1000)?;
    let data = NotifyData { key, flags: 0, event: e, result: AtomicUsize::new(99) };
    run_thread(notify_change_thread, &raw const data as usize)?;
    c.eq(data.result.load(Ordering::SeqCst) as u32, ERROR_SUCCESS)?;
    c.eq(wait(e), WAIT_OBJECT_0)?;
    close(key);
    // Another thread with it: nothing until the change.
    let key = open(base, "TestKey", KEY_ALL_ACCESS).map_err(|_| c.0 + 1000)?;
    let data = NotifyData {
        key,
        flags: REG_NOTIFY_THREAD_AGNOSTIC,
        event: e,
        result: AtomicUsize::new(99),
    };
    run_thread(notify_change_thread, &raw const data as usize)?;
    c.eq(data.result.load(Ordering::SeqCst) as u32, ERROR_SUCCESS)?;
    c.eq(wait(e), WAIT_TIMEOUT)?;
    let sub = create(key, "SubKey", KEY_ALL_ACCESS).map_err(|_| c.0 + 1000)?;
    c.eq(wait(e), WAIT_OBJECT_0)?;
    c.eq(delete(key, "SubKey"), ERROR_SUCCESS)?;
    close(sub);
    close(key);
    // SAFETY: our event.
    unsafe { CloseHandle(e) };
    Ok(())
}

/// A key another thread changes after a moment.
static CHANGE_KEY: AtomicUsize = AtomicUsize::new(0);

extern "system" fn change_later(_: usize) -> u32 {
    // SAFETY: plain call.
    unsafe { Sleep(100) };
    let key = CHANGE_KEY.load(Ordering::SeqCst);
    let _ = set_dword(key, "later", 1);
    0
}

fn start_thread(start: extern "system" fn(usize) -> u32) -> Handle {
    let mut id = 0;
    // SAFETY: a thread with no parameter.
    unsafe { CreateThread(0, 0, start, 0, 0, &mut id) }
}

static APC_CALLS: AtomicUsize = AtomicUsize::new(0);
static APC_CONTEXT: AtomicUsize = AtomicUsize::new(0);
static APC_IOSB: AtomicUsize = AtomicUsize::new(0);

extern "system" fn apc(context: usize, iosb: usize, _reserved: u32) {
    APC_CALLS.fetch_add(1, Ordering::SeqCst);
    APC_CONTEXT.store(context, Ordering::SeqCst);
    APC_IOSB.store(iosb, Ordering::SeqCst);
}

/// Synchronous requests, completion routines, a subordinate key.
fn completions(c: &mut Checks, base: Handle) -> Result<(), u32> {
    let key = create(base, "sync", KEY_ALL_ACCESS).map_err(|_| c.0 + 1000)?;
    // Synchronous: returns once another thread set a value.
    CHANGE_KEY.store(key, Ordering::SeqCst);
    let t = start_thread(change_later);
    let mut io = [UNTOUCHED; 2];
    // SAFETY: (throughout) our keys, events and IO_STATUS_BLOCKs.
    let s = unsafe {
        NtNotifyChangeKey(key, 0, 0, 0, &mut io, REG_NOTIFY_CHANGE_LAST_SET, 0, null_mut(), 0, 0)
    };
    c.ok(s != STATUS_PENDING && s < 0x8000_0000, s.into())?;
    info("a synchronous NtNotifyChangeKey returns", s.into());
    unsafe {
        WaitForSingleObject(t, INFINITE);
        CloseHandle(t);
    }
    let t = start_thread(change_later);
    let r = unsafe { RegNotifyChangeKeyValue(key, 0, REG_NOTIFY_CHANGE_LAST_SET, 0, 0) };
    c.eq(r, ERROR_SUCCESS)?;
    unsafe {
        WaitForSingleObject(t, INFINITE);
        CloseHandle(t);
    }
    close(key);
    // A completion routine: runs at the next alertable wait.
    let key = open(base, "sync", KEY_ALL_ACCESS).map_err(|_| c.0 + 1000)?;
    let mut io = [UNTOUCHED; 2];
    let s = unsafe {
        NtNotifyChangeKey(
            key,
            0,
            apc as *const () as usize,
            0x1234,
            &mut io,
            REG_NOTIFY_CHANGE_LAST_SET,
            0,
            null_mut(),
            0,
            1,
        )
    };
    c.eq(s, STATUS_PENDING)?;
    c.eq(set_dword(key, "v", 1), ERROR_SUCCESS)?;
    c.eq(APC_CALLS.load(Ordering::SeqCst) as u32, 0)?;
    unsafe { SleepEx(0, 1) };
    c.eq(APC_CALLS.load(Ordering::SeqCst) as u32, 1)?;
    c.eq(APC_CONTEXT.load(Ordering::SeqCst) as u32, 0x1234)?;
    c.ok(APC_IOSB.load(Ordering::SeqCst) == &raw const io as usize, 0)?;
    info("the completion routine's IO_STATUS_BLOCK status", io[0] as u64);
    close(key);
    // One subordinate: a change to it completes the master's request.
    let master = create(base, "master", KEY_ALL_ACCESS).map_err(|_| c.0 + 1000)?;
    let other = create(base, "subordinate", KEY_ALL_ACCESS).map_err(|_| c.0 + 1000)?;
    let name = w("subordinate");
    let us = unicode(&name);
    let sub_attr = attributes(base, &us);
    let e = event(true, false);
    let mut io = [UNTOUCHED; 2];
    let s = unsafe {
        NtNotifyChangeMultipleKeys(
            master,
            1,
            &sub_attr,
            e,
            0,
            0,
            &mut io,
            REG_NOTIFY_CHANGE_LAST_SET,
            0,
            null_mut(),
            0,
            1,
        )
    };
    info("NtNotifyChangeMultipleKeys with one subordinate", s.into());
    c.eq(set_dword(other, "v", 1), ERROR_SUCCESS)?;
    info("a change to the subordinate fires (0: yes)", wait(e).into());
    let two = [attributes(base, &us), attributes(base, &us)];
    let mut io2 = [UNTOUCHED; 2];
    let s = unsafe {
        NtNotifyChangeMultipleKeys(
            other,
            2,
            two.as_ptr(),
            e,
            0,
            0,
            &mut io2,
            REG_NOTIFY_CHANGE_LAST_SET,
            0,
            null_mut(),
            0,
            1,
        )
    };
    info("NtNotifyChangeMultipleKeys with two subordinates", s.into());
    close(master);
    close(other);
    unsafe { CloseHandle(e) };
    Ok(())
}

/// The names of `key`'s subkeys, in enumeration order, joined by `,`.
fn subkeys(key: Handle) -> [u16; 128] {
    let mut out = [0u16; 128];
    let mut n = 0;
    for i in 0.. {
        let mut buf = [0u8; 200];
        let mut ret = 0;
        // SAFETY: our buffer; KeyBasicInformation.
        let s = unsafe { NtEnumerateKey(key, i, 0, buf.as_mut_ptr(), 200, &mut ret) };
        if s == STATUS_NO_MORE_ENTRIES || s != STATUS_SUCCESS {
            break;
        }
        let bytes = u32::from_le_bytes([buf[12], buf[13], buf[14], buf[15]]) as usize;
        if n > 0 && n < out.len() {
            out[n] = u16::from(b',');
            n += 1;
        }
        for k in 0..bytes / 2 {
            if n < out.len() - 1 {
                out[n] = u16::from_le_bytes([buf[16 + 2 * k], buf[17 + 2 * k]]);
                n += 1;
            }
        }
    }
    out
}

fn same(got: &[u16], want: &str) -> bool {
    len(got) == want.len() && got.iter().zip(want.bytes()).all(|(&g, b)| g == u16::from(b))
}

/// Whether `key`'s KeyNameInformation ends with `\<tail>`.
fn name_ends(key: Handle, tail: &str) -> bool {
    let mut buf = [0u8; 600];
    let mut ret = 0;
    // SAFETY: our buffer; KeyNameInformation.
    if unsafe { NtQueryKey(key, 3, buf.as_mut_ptr(), 600, &mut ret) } != STATUS_SUCCESS {
        return false;
    }
    let units = (u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]) / 2) as usize;
    let at = |i: usize| u16::from_le_bytes([buf[4 + 2 * i], buf[5 + 2 * i]]);
    let t = tail.as_bytes();
    units > t.len()
        && at(units - t.len() - 1) == u16::from(b'\\')
        && t.iter().enumerate().all(|(i, &b)| at(units - t.len() + i) == u16::from(b))
}

/// Wine's ntdll `test_NtRenameKey` and advapi32 `test_RegRenameKey`.
fn rename(c: &mut Checks, base: Handle) -> Result<(), u32> {
    // SAFETY: (throughout) our keys and UNICODE_STRINGs.
    c.eq(unsafe { NtRenameKey(0, null()) }, STATUS_ACCESS_VIOLATION)?;
    let key = nt_create(base, "rename", KEY_READ | DELETE).map_err(|s| c.0 + s)?;
    let sub = nt_create(key, "rename_subkey", KEY_READ | DELETE).map_err(|s| c.0 + s)?;
    let empty = UnicodeString { length: 0, maximum: 0, buffer: null() };
    c.eq(unsafe { NtRenameKey(sub, &empty) }, STATUS_INVALID_PARAMETER)?;
    let (old, new) = (w("rename_subkey"), w("renamed_subkey"));
    let (old_us, new_us) = (unicode(&old), unicode(&new));
    c.eq(unsafe { NtRenameKey(sub, null()) }, STATUS_ACCESS_VIOLATION)?;
    c.eq(unsafe { NtRenameKey(0, &old_us) }, STATUS_INVALID_HANDLE)?;
    c.eq(unsafe { NtRenameKey(sub, &new_us) }, STATUS_ACCESS_DENIED)?;
    unsafe { NtClose(sub) };
    let sub = nt_create(key, "rename_subkey", KEY_WRITE | DELETE).map_err(|s| c.0 + s)?;
    // Watches on the parent and on the key itself (what fires is not
    // documented).
    let parent_watch = open(base, "rename", KEY_NOTIFY).map_err(|_| c.0 + 1000)?;
    let own_watch = open(key, "rename_subkey", KEY_NOTIFY).map_err(|_| c.0 + 1000)?;
    let other = open(key, "rename_subkey", KEY_READ).map_err(|_| c.0 + 1000)?;
    let (e1, e2) = (event(true, false), event(true, false));
    let (mut io1, mut io2) = ([0usize; 2], [0usize; 2]);
    c.eq(notify(parent_watch, e1, &mut io1, REG_NOTIFY_CHANGE_NAME, false), STATUS_PENDING)?;
    c.eq(notify(own_watch, e2, &mut io2, REG_NOTIFY_CHANGE_NAME, false), STATUS_PENDING)?;
    // Its own name.
    c.eq(unsafe { NtRenameKey(sub, &old_us) }, STATUS_CANNOT_DELETE)?;
    c.eq(unsafe { NtRenameKey(sub, &new_us) }, STATUS_SUCCESS)?;
    c.ok(name_ends(sub, "renamed_subkey"), 0)?;
    info(
        "another handle's name is the new one (1: yes)",
        name_ends(other, "renamed_subkey").into(),
    );
    info("a rename fires the parent's NAME watch (0: yes)", wait(e1).into());
    info("a rename fires the key's own NAME watch (0: yes)", wait(e2).into());
    close(parent_watch);
    close(own_watch);
    close(other);
    unsafe {
        CloseHandle(e1);
        CloseHandle(e2);
    }
    let mut subs = [0; 3];
    for (h, name) in subs.iter_mut().zip(["b", "c", "d"]) {
        *h = nt_create(key, name, KEY_ALL_ACCESS).map_err(|s| c.0 + s)?;
    }
    let a = w("a");
    c.eq(unsafe { NtRenameKey(subs[0], &unicode(&a)) }, STATUS_SUCCESS)?;
    c.ok(same(&subkeys(key), "a,c,d,renamed_subkey"), 0)?;
    let b = w("b");
    c.eq(unsafe { NtRenameKey(subs[0], &unicode(&b)) }, STATUS_SUCCESS)?;
    c.ok(same(&subkeys(key), "b,c,d,renamed_subkey"), 0)?;
    for h in subs.into_iter().chain([sub]) {
        unsafe {
            NtDeleteKey(h);
            NtClose(h);
        }
    }
    unsafe {
        NtDeleteKey(key);
        NtClose(key);
    }
    // RegRenameKey.
    let newname = w("newname");
    let oldname = w("oldname");
    c.eq(unsafe { RegRenameKey(0, null(), null()) }, ERROR_INVALID_PARAMETER)?;
    c.eq(unsafe { RegRenameKey(0, null(), newname.as_ptr()) }, ERROR_INVALID_HANDLE)?;
    c.eq(unsafe { RegRenameKey(0, oldname.as_ptr(), null()) }, ERROR_INVALID_HANDLE)?;
    c.eq(unsafe { RegRenameKey(0, oldname.as_ptr(), newname.as_ptr()) }, ERROR_INVALID_HANDLE)?;
    let key = create(base, "TestRenameKey", KEY_READ).map_err(|_| c.0 + 1000)?;
    let to = w("TestNewRenameKey");
    c.eq(unsafe { RegRenameKey(key, null(), to.as_ptr()) }, ERROR_ACCESS_DENIED)?;
    close(key);
    let key = create(base, "TestRenameKey", KEY_WRITE).map_err(|_| c.0 + 1000)?;
    c.eq(unsafe { RegRenameKey(key, null(), null()) }, ERROR_INVALID_PARAMETER)?;
    c.eq(unsafe { RegRenameKey(key, null(), to.as_ptr()) }, ERROR_SUCCESS)?;
    close(key);
    c.eq(delete(base, "TestNewRenameKey"), ERROR_SUCCESS)?;
    c.ok(delete(base, "TestRenameKey") != ERROR_SUCCESS, 0)?;
    let key = create(base, "TestRenameKey", KEY_WRITE).map_err(|_| c.0 + 1000)?;
    let unknown = w("unknown_subkey");
    let known = w("known_subkey");
    let renamed = w("renamed_subkey");
    c.eq(unsafe { RegRenameKey(key, unknown.as_ptr(), null()) }, ERROR_FILE_NOT_FOUND)?;
    c.eq(unsafe { RegRenameKey(key, unknown.as_ptr(), known.as_ptr()) }, ERROR_FILE_NOT_FOUND)?;
    let k2 = create(key, "known_subkey", KEY_WRITE).map_err(|_| c.0 + 1000)?;
    close(k2);
    c.eq(unsafe { RegRenameKey(key, known.as_ptr(), renamed.as_ptr()) }, ERROR_SUCCESS)?;
    c.eq(delete(key, "renamed_subkey"), ERROR_SUCCESS)?;
    c.ok(delete(key, "known_subkey") != ERROR_SUCCESS, 0)?;
    close(key);
    c.eq(delete(base, "TestRenameKey"), ERROR_SUCCESS)?;
    Ok(())
}

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    let path = base_path();
    // SAFETY: a NUL-terminated name; what a failed run left goes.
    unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, path.as_ptr()) };
    let mut base = 0;
    // SAFETY: our names and output.
    let e = unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            path.as_ptr(),
            0,
            null(),
            REG_OPTION_VOLATILE,
            KEY_ALL_ACCESS,
            0,
            &mut base,
            null_mut(),
        )
    };
    c.eq(e, ERROR_SUCCESS)?;
    let result = nt_notify(&mut c, base)
        .and_then(|()| filters(&mut c, base))
        .and_then(|()| reg_notify(&mut c, base))
        .and_then(|()| completions(&mut c, base))
        .and_then(|()| rename(&mut c, base));
    close(base);
    // SAFETY: as above.
    unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, path.as_ptr()) };
    result
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
