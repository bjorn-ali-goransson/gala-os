//! Byte-range file locks through kernel32, with the results real Windows
//! gives: LockFileEx exclusive and shared ranges on one file through two
//! handles (a conflicting LOCKFILE_FAIL_IMMEDIATELY request fails with
//! ERROR_LOCK_VIOLATION; shared locks coexist), reads and writes through
//! another handle of an exclusively locked range and writes to a shared
//! range (even by its owner) fail with ERROR_LOCK_VIOLATION, UnlockFileEx
//! of a range not locked fails with ERROR_NOT_LOCKED, LockFile/UnlockFile,
//! locks go away when their handle is closed, and a waiting LockFileEx on
//! another thread is granted when the holder unlocks. Prints
//! `lock_test: ok` or exits with the number of the failing check.

#![no_std]
#![no_main]

use core::ffi::c_void;
use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

type Bool = i32;
type Handle = *mut c_void;

#[repr(C)]
struct Overlapped {
    internal: usize,
    internal_high: usize,
    offset: u32,
    offset_high: u32,
    event: Handle,
}

impl Overlapped {
    fn at(offset: u32) -> Self {
        Overlapped {
            internal: 0,
            internal_high: 0,
            offset,
            offset_high: 0,
            event: core::ptr::null_mut(),
        }
    }
}

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetStdHandle(which: u32) -> Handle;
    fn GetLastError() -> u32;
    fn SetLastError(code: u32);
    fn GetTempPathW(size: u32, buf: *mut u16) -> u32;
    fn CreateFileW(
        path: *const u16,
        access: u32,
        share: u32,
        attrs: *mut c_void,
        disposition: u32,
        flags: u32,
        template: Handle,
    ) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, done: *mut u32, ov: *mut Overlapped) -> Bool;
    fn ReadFile(h: Handle, buf: *mut u8, len: u32, done: *mut u32, ov: *mut Overlapped) -> Bool;
    fn LockFileEx(
        h: Handle,
        flags: u32,
        reserved: u32,
        low: u32,
        high: u32,
        ov: *mut Overlapped,
    ) -> Bool;
    fn UnlockFileEx(h: Handle, reserved: u32, low: u32, high: u32, ov: *mut Overlapped) -> Bool;
    fn LockFile(h: Handle, off_low: u32, off_high: u32, low: u32, high: u32) -> Bool;
    fn UnlockFile(h: Handle, off_low: u32, off_high: u32, low: u32, high: u32) -> Bool;
    fn CloseHandle(h: Handle) -> Bool;
    fn DeleteFileW(path: *const u16) -> Bool;
    fn CreateThread(
        attrs: *mut c_void,
        stack: usize,
        start: extern "system" fn(*mut c_void) -> u32,
        param: *mut c_void,
        flags: u32,
        id: *mut u32,
    ) -> Handle;
    fn WaitForSingleObject(h: Handle, ms: u32) -> u32;
    fn Sleep(ms: u32);
    fn ExitProcess(code: u32) -> !;
}

// The memory functions compiled code may call, from ntdll.
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

/// Unwind personality referenced by the prebuilt `core` library's unwind
/// tables. With `panic = abort` nothing unwinds, so it is never called.
#[unsafe(no_mangle)]
pub extern "C" fn __CxxFrameHandler3() -> ! {
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xc000_0409) }
}

const GENERIC_READ: u32 = 0x8000_0000;
const GENERIC_WRITE: u32 = 0x4000_0000;
const SHARE_READ_WRITE: u32 = 3;
const CREATE_ALWAYS: u32 = 2;
const OPEN_EXISTING: u32 = 3;
const INVALID_HANDLE_VALUE: Handle = usize::MAX as Handle;
const LOCKFILE_FAIL_IMMEDIATELY: u32 = 1;
const LOCKFILE_EXCLUSIVE_LOCK: u32 = 2;
const ERROR_LOCK_VIOLATION: u32 = 33;
const ERROR_NOT_LOCKED: u32 = 158;
const WAIT_TIMEOUT: u32 = 258;
const EXCLUSIVE_NOW: u32 = LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY;

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
        out(b"lock_test: check ");
        out(&hex(self.0));
        out(b" got ");
        out(&hex(got as u32));
        out(b"\r\n");
        Err(self.0)
    }

    fn eq(&mut self, got: u64, want: u64) -> Result<(), u32> {
        self.ok(got == want, got)
    }

    /// `result` is a failure with last error `error`.
    fn fails(&mut self, result: Bool, error: u32) -> Result<(), u32> {
        // SAFETY: plain call.
        let last = unsafe { GetLastError() };
        self.ok(result == 0 && last == error, (u64::from(result as u32) << 16) | u64::from(last))
    }
}

fn open(path: &[u16], disposition: u32) -> Handle {
    // SAFETY: a NUL-terminated path.
    unsafe {
        CreateFileW(
            path.as_ptr(),
            GENERIC_READ | GENERIC_WRITE,
            SHARE_READ_WRITE,
            core::ptr::null_mut(),
            disposition,
            0x80,
            core::ptr::null_mut(),
        )
    }
}

fn lock(h: Handle, flags: u32, offset: u32, len: u32) -> Bool {
    let mut ov = Overlapped::at(offset);
    // SAFETY: valid OVERLAPPED.
    unsafe { LockFileEx(h, flags, 0, len, 0, &mut ov) }
}

fn unlock(h: Handle, offset: u32, len: u32) -> Bool {
    let mut ov = Overlapped::at(offset);
    // SAFETY: valid OVERLAPPED.
    unsafe { UnlockFileEx(h, 0, len, 0, &mut ov) }
}

fn read_at(h: Handle, offset: u32) -> Bool {
    let (mut ov, mut buf, mut done) = (Overlapped::at(offset), [0u8; 4], 0);
    // SAFETY: valid buffer and OVERLAPPED.
    unsafe { ReadFile(h, buf.as_mut_ptr(), 4, &mut done, &mut ov) }
}

fn write_at(h: Handle, offset: u32) -> Bool {
    let (mut ov, mut done) = (Overlapped::at(offset), 0);
    // SAFETY: valid buffer and OVERLAPPED.
    unsafe { WriteFile(h, b"wxyz".as_ptr(), 4, &mut done, &mut ov) }
}

/// The waiting thread's handle, and its progress (1: started, 2: locked).
static WAITER_HANDLE: AtomicUsize = AtomicUsize::new(0);
static WAITER_STATE: AtomicU32 = AtomicU32::new(0);

extern "system" fn waiter(_: *mut c_void) -> u32 {
    let h = WAITER_HANDLE.load(Ordering::SeqCst) as Handle;
    WAITER_STATE.store(1, Ordering::SeqCst);
    // Blocks until the range is free.
    let ok = lock(h, LOCKFILE_EXCLUSIVE_LOCK, 60, 10);
    WAITER_STATE.store(2, Ordering::SeqCst);
    u32::from(ok == 0)
}

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    let mut path = [0u16; 300];
    // SAFETY: documented kernel32 calls with valid arguments.
    unsafe {
        let n = GetTempPathW(260, path.as_mut_ptr()) as usize;
        for (i, &b) in b"gala_lock_test.dat\0".iter().enumerate() {
            path[n + i] = u16::from(b);
        }
        let h1 = open(&path, CREATE_ALWAYS);
        c.ok(h1 != INVALID_HANDLE_VALUE, GetLastError().into())?;
        let mut done = 0;
        let data = [b'.'; 100];
        c.ok(WriteFile(h1, data.as_ptr(), 100, &mut done, core::ptr::null_mut()) != 0, 0)?;
        let h2 = open(&path, OPEN_EXISTING);
        c.ok(h2 != INVALID_HANDLE_VALUE, GetLastError().into())?;

        // Exclusive: other handles can neither lock, read nor write it.
        c.ok(lock(h1, EXCLUSIVE_NOW, 0, 10) != 0, GetLastError().into())?;
        c.fails(lock(h2, EXCLUSIVE_NOW, 5, 10), ERROR_LOCK_VIOLATION)?;
        c.fails(lock(h2, LOCKFILE_FAIL_IMMEDIATELY, 9, 1), ERROR_LOCK_VIOLATION)?;
        c.fails(read_at(h2, 0), ERROR_LOCK_VIOLATION)?;
        c.fails(write_at(h2, 2), ERROR_LOCK_VIOLATION)?;
        c.ok(read_at(h2, 50) != 0, GetLastError().into())?; // outside
        c.ok(lock(h2, EXCLUSIVE_NOW, 10, 5) != 0, GetLastError().into())?; // adjacent
        c.ok(read_at(h1, 0) != 0 && write_at(h1, 0) != 0, GetLastError().into())?; // owner
        c.ok(unlock(h1, 0, 10) != 0, GetLastError().into())?;
        c.fails(unlock(h1, 0, 10), ERROR_NOT_LOCKED)?;
        c.fails(unlock(h1, 10, 5), ERROR_NOT_LOCKED)?; // h2's
        c.ok(unlock(h2, 10, 5) != 0, GetLastError().into())?;
        c.ok(read_at(h2, 0) != 0, GetLastError().into())?;

        // Shared: they coexist, exclude exclusive ones, and nobody writes.
        c.ok(lock(h1, LOCKFILE_FAIL_IMMEDIATELY, 20, 10) != 0, GetLastError().into())?;
        c.ok(lock(h2, LOCKFILE_FAIL_IMMEDIATELY, 25, 10) != 0, GetLastError().into())?;
        c.fails(lock(h2, EXCLUSIVE_NOW, 28, 1), ERROR_LOCK_VIOLATION)?;
        c.ok(read_at(h2, 20) != 0 && read_at(h1, 31) != 0, GetLastError().into())?;
        c.fails(write_at(h2, 20), ERROR_LOCK_VIOLATION)?;
        c.fails(write_at(h1, 20), ERROR_LOCK_VIOLATION)?;
        c.ok(unlock(h1, 20, 10) != 0 && unlock(h2, 25, 10) != 0, GetLastError().into())?;
        c.ok(write_at(h1, 20) != 0, GetLastError().into())?;

        // LockFile / UnlockFile: exclusive, failing at once.
        c.ok(LockFile(h1, 80, 0, 5, 0) != 0, GetLastError().into())?;
        SetLastError(0);
        c.fails(LockFile(h2, 82, 0, 1, 0), ERROR_LOCK_VIOLATION)?;
        c.ok(UnlockFile(h1, 80, 0, 5, 0) != 0, GetLastError().into())?;
        c.fails(UnlockFile(h1, 80, 0, 5, 0), ERROR_NOT_LOCKED)?;

        // Closing a handle releases its locks.
        c.ok(lock(h2, EXCLUSIVE_NOW, 40, 10) != 0, GetLastError().into())?;
        c.fails(lock(h1, EXCLUSIVE_NOW, 40, 10), ERROR_LOCK_VIOLATION)?;
        c.ok(CloseHandle(h2) != 0, 0)?;
        c.ok(lock(h1, EXCLUSIVE_NOW, 40, 10) != 0, GetLastError().into())?;
        c.ok(unlock(h1, 40, 10) != 0, GetLastError().into())?;

        // A waiting request is granted when the holder unlocks.
        let h3 = open(&path, OPEN_EXISTING);
        c.ok(h3 != INVALID_HANDLE_VALUE, GetLastError().into())?;
        c.ok(lock(h1, EXCLUSIVE_NOW, 60, 10) != 0, GetLastError().into())?;
        WAITER_HANDLE.store(h3 as usize, Ordering::SeqCst);
        let mut tid = 0;
        let t = CreateThread(core::ptr::null_mut(), 0, waiter, core::ptr::null_mut(), 0, &mut tid);
        c.ok(!t.is_null(), GetLastError().into())?;
        while WAITER_STATE.load(Ordering::SeqCst) == 0 {
            Sleep(1);
        }
        c.eq(WaitForSingleObject(t, 200).into(), WAIT_TIMEOUT.into())?;
        c.eq(WAITER_STATE.load(Ordering::SeqCst).into(), 1)?; // still waiting
        c.ok(unlock(h1, 60, 10) != 0, GetLastError().into())?;
        c.eq(WaitForSingleObject(t, 5000).into(), 0)?;
        c.eq(WAITER_STATE.load(Ordering::SeqCst).into(), 2)?;
        c.fails(lock(h1, EXCLUSIVE_NOW, 65, 1), ERROR_LOCK_VIOLATION)?; // h3 has it
        CloseHandle(t);
        CloseHandle(h3);
        CloseHandle(h1);
        c.ok(DeleteFileW(path.as_ptr()) != 0, GetLastError().into())?;
    }
    Ok(())
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    let code = match run() {
        Ok(()) => {
            out(b"lock_test: ok\r\n");
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
