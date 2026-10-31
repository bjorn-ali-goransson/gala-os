//! Directory change notification (ReadDirectoryChangesW, over ntdll's
//! NtNotifyChangeDirectoryFile), as the JDK's WatchService uses it, with
//! the results real Windows gives (documented on ReadDirectoryChangesW and
//! FILE_NOTIFY_INFORMATION):
//!
//! - An overlapped request on a directory (FILE_FLAG_BACKUP_SEMANTICS |
//!   FILE_FLAG_OVERLAPPED) completes when a change matching the filter
//!   happens: FILE_NOTIFY_INFORMATION records (NextEntryOffset, Action,
//!   FileNameLength in bytes, the name relative to the directory).
//! - Actions: ADDED (1), REMOVED (2), MODIFIED (3), RENAMED_OLD_NAME (4)
//!   followed by RENAMED_NEW_NAME (5).
//! - Changes between requests are kept and returned at once by the next.
//! - A subtree watch names files below with their relative path.
//! - Completion through a completion port (the JDK's pattern); a
//!   synchronous handle's call blocks until a change.
//! - CancelIoEx ends a request with ERROR_OPERATION_ABORTED; closing the
//!   handle ends it with STATUS_NOTIFY_CLEANUP.
//!
//! Works in its own directory under the temporary directory. Prints
//! `dirchange_test: ok` or exits with the number of the failing check.

#![no_std]
#![no_main]

use core::ffi::c_void;

type Handle = *mut c_void;
type Bool = i32;

#[repr(C)]
struct Overlapped {
    internal: usize,
    internal_high: usize,
    offset: u64,
    event: Handle,
}

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, ov: *mut c_void) -> Bool;
    fn ExitProcess(code: u32) -> !;
    fn GetLastError() -> u32;
    fn GetTempPathW(len: u32, buf: *mut u16) -> u32;
    fn CreateDirectoryW(path: *const u16, security: *mut c_void) -> Bool;
    fn RemoveDirectoryW(path: *const u16) -> Bool;
    fn DeleteFileW(path: *const u16) -> Bool;
    fn MoveFileW(from: *const u16, to: *const u16) -> Bool;
    fn CreateFileW(
        name: *const u16,
        access: u32,
        share: u32,
        security: *mut c_void,
        disposition: u32,
        flags: u32,
        template: Handle,
    ) -> Handle;
    fn CloseHandle(h: Handle) -> Bool;
    fn CreateEventW(security: *mut c_void, manual: Bool, initial: Bool, name: *const u16)
    -> Handle;
    fn WaitForSingleObject(h: Handle, ms: u32) -> u32;
    fn ReadDirectoryChangesW(
        dir: Handle,
        buf: *mut u8,
        len: u32,
        subtree: Bool,
        filter: u32,
        returned: *mut u32,
        ov: *mut Overlapped,
        routine: usize,
    ) -> Bool;
    fn GetOverlappedResult(h: Handle, ov: *mut Overlapped, bytes: *mut u32, wait: Bool) -> Bool;
    fn CancelIoEx(h: Handle, ov: *mut Overlapped) -> Bool;
    fn CreateIoCompletionPort(file: Handle, port: Handle, key: usize, threads: u32) -> Handle;
    fn GetQueuedCompletionStatus(
        port: Handle,
        bytes: *mut u32,
        key: *mut usize,
        ov: *mut *mut Overlapped,
        ms: u32,
    ) -> Bool;
    fn CreateThread(
        security: *mut c_void,
        stack: usize,
        start: extern "system" fn(*mut c_void) -> u32,
        arg: *mut c_void,
        flags: u32,
        id: *mut u32,
    ) -> Handle;
    fn GetExitCodeThread(h: Handle, code: *mut u32) -> Bool;
    fn Sleep(ms: u32);
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

const FILE_LIST_DIRECTORY: u32 = 1;
const GENERIC_WRITE: u32 = 0x4000_0000;
const SHARE_ALL: u32 = 7;
const CREATE_NEW: u32 = 1;
const OPEN_EXISTING: u32 = 3;
const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
const FILE_FLAG_OVERLAPPED: u32 = 0x4000_0000;
const NAMES: u32 = 0x1 | 0x2; // FILE_NOTIFY_CHANGE_FILE_NAME | DIR_NAME
const LAST_WRITE: u32 = 0x10;
const WAIT_TIMEOUT: u64 = 0x102;
const ERROR_IO_INCOMPLETE: u64 = 996;
const ERROR_OPERATION_ABORTED: u64 = 995;
const STATUS_NOTIFY_CLEANUP: u64 = 0x10b;

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
        out(b"dirchange_test: check ");
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

/// A NUL-terminated path: the test directory, then `rest`.
struct Path([u16; 300]);

/// The test directory (`%TEMP%\\gala_dirchange\\`) followed by `rest`.
fn path(rest: &str) -> Path {
    let mut p = [0u16; 300];
    // SAFETY: our buffer of 260 characters (of 300).
    let n = unsafe { GetTempPathW(260, p.as_mut_ptr()) } as usize;
    for (i, b) in "gala_dirchange\\".bytes().chain(rest.bytes()).enumerate() {
        p[n + i] = u16::from(b);
    }
    Path(p)
}

/// The records in `buf[..len]` as (action, name) pairs, through `f`.
fn records(buf: &[u8], len: usize, mut f: impl FnMut(u32, &[u8])) {
    let mut at = 0;
    while at + 12 <= len {
        let word = |o: usize| u32::from_le_bytes([buf[o], buf[o + 1], buf[o + 2], buf[o + 3]]);
        let (next, action, name_len) = (word(at), word(at + 4), word(at + 8) as usize);
        let mut name = [0u8; 64];
        let n = (name_len / 2).min(64);
        for (i, c) in name.iter_mut().enumerate().take(n) {
            *c = buf[at + 12 + 2 * i];
        }
        f(action, &name[..n]);
        if next == 0 {
            break;
        }
        at += next as usize;
    }
}

/// Whether the records contain (action, name).
fn has(buf: &[u8], len: usize, action: u32, name: &[u8]) -> bool {
    let mut found = false;
    records(buf, len, |a, n| found |= a == action && n == name);
    found
}

/// Creates (and closes) file `rest`, writing `data` if not empty.
fn make(rest: &str, data: &[u8]) -> bool {
    let p = path(rest);
    // SAFETY: a NUL-terminated path.
    unsafe {
        let h = CreateFileW(
            p.0.as_ptr(),
            GENERIC_WRITE,
            SHARE_ALL,
            core::ptr::null_mut(),
            CREATE_NEW,
            0,
            core::ptr::null_mut(),
        );
        if h as isize == -1 {
            return false;
        }
        let mut written = 0;
        if !data.is_empty() {
            WriteFile(h, data.as_ptr(), data.len() as u32, &mut written, core::ptr::null_mut());
        }
        CloseHandle(h);
    }
    true
}

fn open_dir(overlapped: bool) -> Handle {
    let p = path("");
    let flags = FILE_FLAG_BACKUP_SEMANTICS | if overlapped { FILE_FLAG_OVERLAPPED } else { 0 };
    // SAFETY: a NUL-terminated path.
    unsafe {
        CreateFileW(
            p.0.as_ptr(),
            FILE_LIST_DIRECTORY,
            SHARE_ALL,
            core::ptr::null_mut(),
            OPEN_EXISTING,
            flags,
            core::ptr::null_mut(),
        )
    }
}

/// The synchronous call, on another thread: exits with the bytes returned.
extern "system" fn sync_watcher(_: *mut c_void) -> u32 {
    let h = open_dir(false);
    let mut buf = [0u8; 1024];
    let mut returned = 0u32;
    // SAFETY: our buffer; a synchronous handle blocks until a change.
    let ok = unsafe {
        ReadDirectoryChangesW(
            h,
            buf.as_mut_ptr(),
            1024,
            0,
            NAMES,
            &mut returned,
            core::ptr::null_mut(),
            0,
        )
    };
    // SAFETY: our handle.
    unsafe { CloseHandle(h) };
    if ok != 0 && has(&buf, returned as usize, 1, b"sync.txt") { returned } else { 0 }
}

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    // SAFETY: kernel32 calls with buffers of ours.
    unsafe {
        let mut probe = [0u16; 260];
        c.ok(GetTempPathW(260, probe.as_mut_ptr()) > 0, 0)?;
        CreateDirectoryW(path("").0.as_ptr(), core::ptr::null_mut());

        let dir = open_dir(true);
        c.ok(dir as isize != -1, GetLastError().into())?;
        let event = CreateEventW(core::ptr::null_mut(), 1, 0, core::ptr::null());
        let mut ov: Overlapped = core::mem::zeroed();
        ov.event = event;
        let mut buf = [0u8; 4096];
        let mut bytes = 0u32;

        // Nothing yet: the request is pending.
        let ok = ReadDirectoryChangesW(
            dir,
            buf.as_mut_ptr(),
            4096,
            0,
            NAMES | LAST_WRITE,
            core::ptr::null_mut(),
            &mut ov,
            0,
        );
        c.ok(ok != 0, GetLastError().into())?;
        c.eq(WaitForSingleObject(event, 0).into(), WAIT_TIMEOUT)?;
        // A new file completes it.
        c.ok(make("a.txt", b""), 0)?;
        c.eq(WaitForSingleObject(event, 5000).into(), 0)?;
        c.ok(GetOverlappedResult(dir, &mut ov, &mut bytes, 0) != 0, GetLastError().into())?;
        c.ok(bytes >= 12, bytes.into())?;
        let mut first = (0, [0u8; 8], 0);
        records(&buf, bytes as usize, |a, n| {
            if first.0 == 0 {
                first.0 = a;
                first.2 = n.len().min(8);
                first.1[..first.2].copy_from_slice(&n[..first.2]);
            }
        });
        c.eq(first.0.into(), 1)?; // FILE_ACTION_ADDED
        c.ok(&first.1[..first.2] == b"a.txt", first.2 as u64)?;

        // Changes while no request waits are kept for the next one: a
        // write (MODIFIED), then a rename (OLD_NAME then NEW_NAME).
        let w = path("a.txt");
        let h = CreateFileW(
            w.0.as_ptr(),
            GENERIC_WRITE,
            SHARE_ALL,
            core::ptr::null_mut(),
            OPEN_EXISTING,
            0,
            core::ptr::null_mut(),
        );
        let mut written = 0;
        WriteFile(h, b"hi".as_ptr(), 2, &mut written, core::ptr::null_mut());
        CloseHandle(h);
        c.ok(
            MoveFileW(path("a.txt").0.as_ptr(), path("b.txt").0.as_ptr()) != 0,
            GetLastError().into(),
        )?;
        Sleep(50);
        ov = core::mem::zeroed();
        ov.event = event;
        let ok = ReadDirectoryChangesW(
            dir,
            buf.as_mut_ptr(),
            4096,
            0,
            NAMES | LAST_WRITE,
            core::ptr::null_mut(),
            &mut ov,
            0,
        );
        c.ok(ok != 0, GetLastError().into())?;
        c.eq(WaitForSingleObject(event, 5000).into(), 0)?;
        c.ok(GetOverlappedResult(dir, &mut ov, &mut bytes, 0) != 0, 0)?;
        c.ok(has(&buf, bytes as usize, 3, b"a.txt"), bytes.into())?;
        let mut order = [0u32; 8];
        let mut k = 0;
        records(&buf, bytes as usize, |a, n| {
            if (a == 4 && n == b"a.txt") || (a == 5 && n == b"b.txt") {
                if k < 8 {
                    order[k] = a;
                }
                k += 1;
            }
        });
        c.ok(k == 2 && order[0] == 4 && order[1] == 5, k as u64)?;
        CloseHandle(dir);

        // A subtree watch through a completion port (the JDK's pattern).
        CreateDirectoryW(path("sub").0.as_ptr(), core::ptr::null_mut());
        let dir = open_dir(true);
        let port = CreateIoCompletionPort(dir, core::ptr::null_mut(), 7, 0);
        c.ok(!port.is_null(), GetLastError().into())?;
        ov = core::mem::zeroed();
        let ok = ReadDirectoryChangesW(
            dir,
            buf.as_mut_ptr(),
            4096,
            1,
            NAMES,
            core::ptr::null_mut(),
            &mut ov,
            0,
        );
        c.ok(ok != 0, GetLastError().into())?;
        c.ok(make("sub\\c.txt", b""), 0)?;
        let (mut key, mut got) = (0usize, core::ptr::null_mut());
        c.ok(
            GetQueuedCompletionStatus(port, &mut bytes, &mut key, &mut got, 5000) != 0,
            GetLastError().into(),
        )?;
        c.eq(key as u64, 7)?;
        c.ok(got == &raw mut ov, 0)?;
        c.ok(has(&buf, bytes as usize, 1, b"sub\\c.txt"), bytes.into())?;

        // CancelIoEx: ERROR_OPERATION_ABORTED (and a packet).
        ov = core::mem::zeroed();
        ReadDirectoryChangesW(
            dir,
            buf.as_mut_ptr(),
            4096,
            1,
            NAMES,
            core::ptr::null_mut(),
            &mut ov,
            0,
        );
        c.ok(CancelIoEx(dir, &mut ov) != 0, GetLastError().into())?;
        c.eq(GetQueuedCompletionStatus(port, &mut bytes, &mut key, &mut got, 5000) as u64, 0)?;
        c.eq(GetLastError().into(), ERROR_OPERATION_ABORTED)?;
        c.eq(GetOverlappedResult(dir, &mut ov, &mut bytes, 0) as u64, 0)?;
        c.eq(GetLastError().into(), ERROR_OPERATION_ABORTED)?;

        // Closing the handle ends a waiting request: STATUS_NOTIFY_CLEANUP.
        ov = core::mem::zeroed();
        ReadDirectoryChangesW(
            dir,
            buf.as_mut_ptr(),
            4096,
            1,
            NAMES,
            core::ptr::null_mut(),
            &mut ov,
            0,
        );
        c.eq(GetOverlappedResult(dir, &mut ov, &mut bytes, 0) as u64, 0)?;
        c.eq(GetLastError().into(), ERROR_IO_INCOMPLETE)?;
        CloseHandle(dir);
        c.ok(
            GetQueuedCompletionStatus(port, &mut bytes, &mut key, &mut got, 5000) != 0,
            GetLastError().into(),
        )?;
        c.eq(ov.internal as u64, STATUS_NOTIFY_CLEANUP)?;
        CloseHandle(port);

        // A synchronous handle's call blocks until a change.
        let t = CreateThread(
            core::ptr::null_mut(),
            0,
            sync_watcher,
            core::ptr::null_mut(),
            0,
            core::ptr::null_mut(),
        );
        Sleep(300);
        c.ok(make("sync.txt", b""), 0)?;
        c.eq(WaitForSingleObject(t, 5000).into(), 0)?;
        let mut code = 0u32;
        GetExitCodeThread(t, &mut code);
        c.ok(code >= 12, code.into())?;
        CloseHandle(t);

        // Clean up.
        for f in ["b.txt", "sub\\c.txt", "sync.txt"] {
            DeleteFileW(path(f).0.as_ptr());
        }
        RemoveDirectoryW(path("sub").0.as_ptr());
        RemoveDirectoryW(path("").0.as_ptr());
    }
    Ok(())
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    let code = match run() {
        Ok(()) => {
            out(b"dirchange_test: ok\r\n");
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
