//! kernel32's lightweight synchronization and per-thread data, as the
//! Microsoft C runtime and the JVM use them, with the results real Windows
//! gives (documented contracts):
//!
//! - SRW locks: exclusive and shared, the Try forms (not recursive: the
//!   owner's TryAcquire fails), four threads counting under one lock.
//! - Condition variables with a critical section and with an SRW lock
//!   (shared mode): a waiter woken by another thread; a timeout returns
//!   FALSE with ERROR_TIMEOUT.
//! - InitOnceExecuteOnce: the callback runs once, its context is kept; a
//!   callback that fails leaves the once uninitialized.
//! - Fiber local storage: FlsAlloc/Set/GetValue, a new thread starts with
//!   null, the callback runs at thread exit and at FlsFree for non-null
//!   values.
//! - Interlocked singly linked lists: push returns the previous first
//!   entry, pop, flush, depth.
//! - Semaphores: CreateSemaphoreW/A, waits, ReleaseSemaphore's previous
//!   count, ERROR_TOO_MANY_POSTS past the maximum.
//! - InitializeCriticalSectionEx.
//!
//! Prints `kernel32_sync_test: ok` or exits with the number of the failing
//! check.

#![no_std]
#![no_main]

use core::ffi::c_void;
use core::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};

type Bool = i32;
type Handle = *mut c_void;
type StartRoutine = extern "system" fn(*mut c_void) -> u32;
type InitOnceFn = extern "system" fn(*mut usize, *mut c_void, *mut *mut c_void) -> Bool;
type FlsCallback = extern "system" fn(*mut c_void);

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetLastError() -> u32;
    fn SetLastError(e: u32);
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, ov: *mut c_void) -> Bool;
    fn CreateThread(
        attrs: *mut c_void,
        stack: usize,
        start: StartRoutine,
        param: *mut c_void,
        flags: u32,
        id: *mut u32,
    ) -> Handle;
    fn WaitForSingleObject(h: Handle, ms: u32) -> u32;
    fn CloseHandle(h: Handle) -> Bool;
    fn Sleep(ms: u32);
    fn ExitProcess(code: u32) -> !;
    fn InitializeSRWLock(lock: *mut usize);
    fn AcquireSRWLockExclusive(lock: *mut usize);
    fn ReleaseSRWLockExclusive(lock: *mut usize);
    fn AcquireSRWLockShared(lock: *mut usize);
    fn ReleaseSRWLockShared(lock: *mut usize);
    fn TryAcquireSRWLockExclusive(lock: *mut usize) -> u8;
    fn TryAcquireSRWLockShared(lock: *mut usize) -> u8;
    fn InitializeConditionVariable(cv: *mut usize);
    fn SleepConditionVariableCS(cv: *mut usize, cs: *mut CriticalSection, ms: u32) -> Bool;
    fn SleepConditionVariableSRW(cv: *mut usize, lock: *mut usize, ms: u32, flags: u32) -> Bool;
    fn WakeConditionVariable(cv: *mut usize);
    fn WakeAllConditionVariable(cv: *mut usize);
    fn InitializeCriticalSection(cs: *mut CriticalSection);
    fn InitializeCriticalSectionEx(cs: *mut CriticalSection, spin: u32, flags: u32) -> Bool;
    fn EnterCriticalSection(cs: *mut CriticalSection);
    fn LeaveCriticalSection(cs: *mut CriticalSection);
    fn DeleteCriticalSection(cs: *mut CriticalSection);
    fn InitOnceExecuteOnce(
        once: *mut usize,
        f: InitOnceFn,
        param: *mut c_void,
        context: *mut *mut c_void,
    ) -> Bool;
    fn FlsAlloc(callback: Option<FlsCallback>) -> u32;
    fn FlsFree(index: u32) -> Bool;
    fn FlsGetValue(index: u32) -> *mut c_void;
    fn FlsSetValue(index: u32, value: *mut c_void) -> Bool;
    fn InitializeSListHead(head: *mut SListHeader);
    fn InterlockedPushEntrySList(head: *mut SListHeader, entry: *mut SListEntry)
    -> *mut SListEntry;
    fn InterlockedPopEntrySList(head: *mut SListHeader) -> *mut SListEntry;
    fn InterlockedFlushSList(head: *mut SListHeader) -> *mut SListEntry;
    fn QueryDepthSList(head: *mut SListHeader) -> u16;
    fn CreateSemaphoreW(attrs: *mut c_void, initial: i32, max: i32, name: *const u16) -> Handle;
    fn CreateSemaphoreA(attrs: *mut c_void, initial: i32, max: i32, name: *const u8) -> Handle;
    fn ReleaseSemaphore(h: Handle, count: i32, previous: *mut i32) -> Bool;
}

#[allow(dead_code)]
#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "C" {
    fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memset(dst: *mut u8, value: i32, n: usize) -> *mut u8;
    fn memcmp(a: *const u8, b: *const u8, n: usize) -> i32;
}

#[unsafe(no_mangle)]
pub static _fltused: i32 = 0;

#[unsafe(no_mangle)]
pub extern "C" fn __CxxFrameHandler3() -> ! {
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xdeac) }
}

/// RTL_CRITICAL_SECTION (40 bytes on x64).
#[repr(C)]
struct CriticalSection([u64; 5]);

/// SLIST_HEADER: 16 bytes, 16-byte aligned.
#[repr(C, align(16))]
struct SListHeader([u64; 2]);

/// SLIST_ENTRY: the next pointer, 16-byte aligned.
#[repr(C, align(16))]
struct SListEntry {
    next: *mut SListEntry,
    value: u64,
}

const INFINITE: u32 = 0xffff_ffff;
const WAIT_TIMEOUT: u32 = 258;
const ERROR_TIMEOUT: u32 = 1460;
const ERROR_TOO_MANY_POSTS: u32 = 298;
const FLS_OUT_OF_INDEXES: u32 = 0xffff_ffff;
const CONDITION_VARIABLE_LOCKMODE_SHARED: u32 = 1;

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
        out(b"kernel32_sync_test: check ");
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

/// A shared SRW lock and the counter it protects.
struct Shared<T>(core::cell::UnsafeCell<T>);

// SAFETY: accessed under the test's locks only.
unsafe impl<T> Sync for Shared<T> {}

impl<T> Shared<T> {
    fn get(&self) -> *mut T {
        self.0.get()
    }
}

static LOCK: Shared<usize> = Shared(core::cell::UnsafeCell::new(0));
static COUNTER: Shared<u64> = Shared(core::cell::UnsafeCell::new(0));
const ROUNDS: u64 = 2000;

/// Increments COUNTER ROUNDS times under LOCK, not atomically.
extern "system" fn counter_thread(_: *mut c_void) -> u32 {
    for i in 0..ROUNDS {
        // SAFETY: the lock serializes the read-modify-write.
        unsafe {
            AcquireSRWLockExclusive(LOCK.get());
            let v = core::ptr::read_volatile(COUNTER.get());
            if i % 64 == 0 {
                Sleep(0);
            }
            core::ptr::write_volatile(COUNTER.get(), v + 1);
            ReleaseSRWLockExclusive(LOCK.get());
        }
    }
    0
}

static CS: Shared<CriticalSection> = Shared(core::cell::UnsafeCell::new(CriticalSection([0; 5])));
static CV: Shared<usize> = Shared(core::cell::UnsafeCell::new(0));
static FLAG: AtomicU32 = AtomicU32::new(0);

/// Sets FLAG under CS and wakes the condition variable.
extern "system" fn signal_cs(_: *mut c_void) -> u32 {
    // SAFETY: CS is initialized before this thread starts.
    unsafe {
        Sleep(20);
        EnterCriticalSection(CS.get());
        FLAG.store(1, Ordering::SeqCst);
        LeaveCriticalSection(CS.get());
        WakeConditionVariable(CV.get());
    }
    0
}

/// Sets FLAG under LOCK (exclusive) and wakes all waiters.
extern "system" fn signal_srw(_: *mut c_void) -> u32 {
    // SAFETY: as above.
    unsafe {
        Sleep(20);
        AcquireSRWLockExclusive(LOCK.get());
        FLAG.store(2, Ordering::SeqCst);
        ReleaseSRWLockExclusive(LOCK.get());
        WakeAllConditionVariable(CV.get());
    }
    0
}

static ONCE_CALLS: AtomicU32 = AtomicU32::new(0);

extern "system" fn once_ok(_: *mut usize, param: *mut c_void, context: *mut *mut c_void) -> Bool {
    ONCE_CALLS.fetch_add(1, Ordering::SeqCst);
    // SAFETY: the context output InitOnceExecuteOnce passes.
    unsafe { *context = param };
    1
}

extern "system" fn once_fail(_: *mut usize, _: *mut c_void, _: *mut *mut c_void) -> Bool {
    ONCE_CALLS.fetch_add(1, Ordering::SeqCst);
    0
}

static FLS_INDEX: AtomicU32 = AtomicU32::new(0);
/// Values the FLS callback was called with, and how often.
static FLS_SEEN: [AtomicUsize; 4] = [const { AtomicUsize::new(0) }; 4];
static FLS_CALLS: AtomicUsize = AtomicUsize::new(0);
/// What the FLS thread saw before setting its value.
static FLS_INITIAL: AtomicU64 = AtomicU64::new(u64::MAX);

extern "system" fn fls_callback(value: *mut c_void) {
    let n = FLS_CALLS.fetch_add(1, Ordering::SeqCst);
    if n < FLS_SEEN.len() {
        FLS_SEEN[n].store(value as usize, Ordering::SeqCst);
    }
}

extern "system" fn fls_thread(_: *mut c_void) -> u32 {
    let index = FLS_INDEX.load(Ordering::SeqCst);
    // SAFETY: an allocated FLS index.
    unsafe {
        FLS_INITIAL.store(FlsGetValue(index) as u64, Ordering::SeqCst);
        FlsSetValue(index, 42 as *mut c_void);
    }
    0
}

fn start(f: StartRoutine) -> Handle {
    // SAFETY: a thread running `f`.
    unsafe {
        CreateThread(core::ptr::null_mut(), 0, f, core::ptr::null_mut(), 0, core::ptr::null_mut())
    }
}

fn join(h: Handle) -> bool {
    // SAFETY: a thread handle of ours.
    unsafe {
        let r = WaitForSingleObject(h, 30_000);
        CloseHandle(h);
        r == 0
    }
}

fn srw(c: &mut Checks) -> Result<(), u32> {
    let lock = LOCK.get();
    // SAFETY: the test's own lock.
    unsafe {
        InitializeSRWLock(lock);
        c.eq(*lock as u64, 0)?;
        AcquireSRWLockExclusive(lock);
        c.eq(TryAcquireSRWLockExclusive(lock).into(), 0)?;
        c.eq(TryAcquireSRWLockShared(lock).into(), 0)?;
        ReleaseSRWLockExclusive(lock);
        c.eq(TryAcquireSRWLockShared(lock).into(), 1)?;
        c.eq(TryAcquireSRWLockShared(lock).into(), 1)?;
        c.eq(TryAcquireSRWLockExclusive(lock).into(), 0)?;
        ReleaseSRWLockShared(lock);
        c.eq(TryAcquireSRWLockExclusive(lock).into(), 0)?;
        ReleaseSRWLockShared(lock);
        c.eq(TryAcquireSRWLockExclusive(lock).into(), 1)?;
        ReleaseSRWLockExclusive(lock);
        AcquireSRWLockShared(lock);
        AcquireSRWLockShared(lock);
        ReleaseSRWLockShared(lock);
        ReleaseSRWLockShared(lock);
        c.eq(*lock as u64, 0)?; // free again

        let threads = [
            start(counter_thread),
            start(counter_thread),
            start(counter_thread),
            start(counter_thread),
        ];
        for t in threads {
            c.ok(join(t), 0)?;
        }
        c.eq(*COUNTER.get(), 4 * ROUNDS)?;
    }
    Ok(())
}

fn condition_variables(c: &mut Checks) -> Result<(), u32> {
    // SAFETY: the test's own lock, critical section and variable.
    unsafe {
        InitializeCriticalSection(CS.get());
        InitializeConditionVariable(CV.get());
        c.eq(*CV.get() as u64, 0)?;

        // A timeout: FALSE, ERROR_TIMEOUT, the lock held again.
        EnterCriticalSection(CS.get());
        SetLastError(0);
        c.eq(SleepConditionVariableCS(CV.get(), CS.get(), 10) as u64, 0)?;
        c.eq(GetLastError().into(), ERROR_TIMEOUT.into())?;
        // Woken by another thread.
        let t = start(signal_cs);
        while FLAG.load(Ordering::SeqCst) == 0 {
            c.ok(
                SleepConditionVariableCS(CV.get(), CS.get(), INFINITE) != 0,
                GetLastError().into(),
            )?;
        }
        LeaveCriticalSection(CS.get());
        c.ok(join(t), 0)?;
        DeleteCriticalSection(CS.get());

        // With an SRW lock held shared.
        let lock = LOCK.get();
        AcquireSRWLockShared(lock);
        c.eq(
            SleepConditionVariableSRW(CV.get(), lock, 10, CONDITION_VARIABLE_LOCKMODE_SHARED)
                as u64,
            0,
        )?;
        c.eq(GetLastError().into(), ERROR_TIMEOUT.into())?;
        let t = start(signal_srw);
        while FLAG.load(Ordering::SeqCst) != 2 {
            c.ok(
                SleepConditionVariableSRW(
                    CV.get(),
                    lock,
                    INFINITE,
                    CONDITION_VARIABLE_LOCKMODE_SHARED,
                ) != 0,
                GetLastError().into(),
            )?;
        }
        ReleaseSRWLockShared(lock);
        c.ok(join(t), 0)?;
        c.eq(*lock as u64, 0)?;
    }
    Ok(())
}

fn init_once(c: &mut Checks) -> Result<(), u32> {
    let mut once = 0usize;
    let mut context: *mut c_void = core::ptr::null_mut();
    // SAFETY: the test's own INIT_ONCE.
    unsafe {
        c.ok(InitOnceExecuteOnce(&mut once, once_ok, 0x1230 as *mut c_void, &mut context) != 0, 0)?;
        c.eq(context as u64, 0x1230)?;
        context = core::ptr::null_mut();
        c.ok(InitOnceExecuteOnce(&mut once, once_ok, 0x4560 as *mut c_void, &mut context) != 0, 0)?;
        c.eq(context as u64, 0x1230)?; // the first context
        c.eq(ONCE_CALLS.load(Ordering::SeqCst).into(), 1)?;
        // A failing callback: FALSE, and the next call runs again.
        let mut failing = 0usize;
        c.eq(
            InitOnceExecuteOnce(
                &mut failing,
                once_fail,
                core::ptr::null_mut(),
                core::ptr::null_mut(),
            ) as u64,
            0,
        )?;
        c.ok(
            InitOnceExecuteOnce(&mut failing, once_ok, 8 as *mut c_void, core::ptr::null_mut())
                != 0,
            0,
        )?;
        c.eq(ONCE_CALLS.load(Ordering::SeqCst).into(), 3)?;
    }
    Ok(())
}

fn fls(c: &mut Checks) -> Result<(), u32> {
    // SAFETY: FLS calls with an index of ours.
    unsafe {
        let index = FlsAlloc(Some(fls_callback));
        c.ok(index != FLS_OUT_OF_INDEXES, GetLastError().into())?;
        FLS_INDEX.store(index, Ordering::SeqCst);
        c.eq(FlsGetValue(index) as u64, 0)?;
        c.ok(FlsSetValue(index, 7 as *mut c_void) != 0, 0)?;
        c.eq(FlsGetValue(index) as u64, 7)?;
        // A new thread starts with null; its value is passed to the
        // callback when it exits.
        c.ok(join(start(fls_thread)), 0)?;
        c.eq(FLS_INITIAL.load(Ordering::SeqCst), 0)?;
        c.eq(FLS_CALLS.load(Ordering::SeqCst) as u64, 1)?;
        c.eq(FLS_SEEN[0].load(Ordering::SeqCst) as u64, 42)?;
        c.eq(FlsGetValue(index) as u64, 7)?; // ours is untouched
        // FlsFree: the callback for our value.
        c.ok(FlsFree(index) != 0, 0)?;
        c.eq(FLS_CALLS.load(Ordering::SeqCst) as u64, 2)?;
        c.eq(FLS_SEEN[1].load(Ordering::SeqCst) as u64, 7)?;
        // A second index without a callback.
        let plain = FlsAlloc(None);
        c.ok(plain != FLS_OUT_OF_INDEXES, 0)?;
        c.ok(FlsSetValue(plain, 9 as *mut c_void) != 0, 0)?;
        c.ok(FlsFree(plain) != 0, 0)?;
        c.eq(FLS_CALLS.load(Ordering::SeqCst) as u64, 2)?;
    }
    Ok(())
}

fn slist(c: &mut Checks) -> Result<(), u32> {
    let mut head = SListHeader([u64::MAX; 2]);
    let mut a = SListEntry { next: core::ptr::null_mut(), value: 1 };
    let mut b = SListEntry { next: core::ptr::null_mut(), value: 2 };
    let (h, pa, pb) =
        (&mut head as *mut SListHeader, &mut a as *mut SListEntry, &mut b as *mut SListEntry);
    // SAFETY: the test's own aligned header and entries.
    unsafe {
        InitializeSListHead(h);
        c.eq(QueryDepthSList(h).into(), 0)?;
        c.eq(InterlockedPopEntrySList(h) as u64, 0)?;
        c.eq(InterlockedPushEntrySList(h, pa) as u64, 0)?;
        c.eq(InterlockedPushEntrySList(h, pb) as u64, pa as u64)?;
        c.eq(QueryDepthSList(h).into(), 2)?;
        c.eq(InterlockedPopEntrySList(h) as u64, pb as u64)?;
        c.eq(QueryDepthSList(h).into(), 1)?;
        c.eq(InterlockedPushEntrySList(h, pb) as u64, pa as u64)?;
        let list = InterlockedFlushSList(h);
        c.eq(list as u64, pb as u64)?;
        c.eq((*list).next as u64, pa as u64)?;
        c.eq((*pa).next as u64, 0)?;
        c.eq(QueryDepthSList(h).into(), 0)?;
        c.eq(InterlockedFlushSList(h) as u64, 0)?;
        c.eq((*pb).value + (*pa).value, 3)?;
    }
    Ok(())
}

fn semaphores(c: &mut Checks) -> Result<(), u32> {
    // SAFETY: handles of ours.
    unsafe {
        let s = CreateSemaphoreW(core::ptr::null_mut(), 1, 2, core::ptr::null());
        c.ok(!s.is_null(), GetLastError().into())?;
        c.eq(WaitForSingleObject(s, 0).into(), 0)?;
        c.eq(WaitForSingleObject(s, 0).into(), WAIT_TIMEOUT.into())?;
        let mut previous = -1;
        c.ok(ReleaseSemaphore(s, 1, &mut previous) != 0, GetLastError().into())?;
        c.eq(previous as u64, 0)?;
        c.eq(ReleaseSemaphore(s, 2, &mut previous) as u64, 0)?; // 1 + 2 > 2
        c.eq(GetLastError().into(), ERROR_TOO_MANY_POSTS.into())?;
        c.ok(ReleaseSemaphore(s, 1, &mut previous) != 0, GetLastError().into())?;
        c.eq(previous as u64, 1)?;
        c.eq(WaitForSingleObject(s, 0).into(), 0)?;
        c.eq(WaitForSingleObject(s, 0).into(), 0)?;
        c.eq(WaitForSingleObject(s, 0).into(), WAIT_TIMEOUT.into())?;
        CloseHandle(s);
        let s = CreateSemaphoreA(core::ptr::null_mut(), 0, 1, c"gala-sync-test".as_ptr().cast());
        c.ok(!s.is_null(), GetLastError().into())?;
        c.eq(WaitForSingleObject(s, 0).into(), WAIT_TIMEOUT.into())?;
        CloseHandle(s);

        let mut cs = CriticalSection([0; 5]);
        c.ok(InitializeCriticalSectionEx(&mut cs, 4000, 0) != 0, GetLastError().into())?;
        EnterCriticalSection(&mut cs);
        EnterCriticalSection(&mut cs);
        LeaveCriticalSection(&mut cs);
        LeaveCriticalSection(&mut cs);
        DeleteCriticalSection(&mut cs);
    }
    Ok(())
}

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    srw(&mut c)?;
    condition_variables(&mut c)?;
    init_once(&mut c)?;
    fls(&mut c)?;
    slist(&mut c)?;
    semaphores(&mut c)
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    let code = match run() {
        Ok(()) => {
            out(b"kernel32_sync_test: ok\r\n");
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
