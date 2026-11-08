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
//!   callback that fails leaves the once uninitialized; the callback gets
//!   the caller's Context pointer as it is, NULL when there is none (on
//!   Windows a callback writing through it faults).
//! - Fiber local storage: FlsAlloc/Set/GetValue, a new thread starts with
//!   null, the callback runs at thread exit and at FlsFree for non-null
//!   values.
//! - Interlocked singly linked lists: push returns the previous first
//!   entry, pop, flush, depth.
//! - Semaphores: CreateSemaphoreW/A, waits, ReleaseSemaphore's previous
//!   count, ERROR_TOO_MANY_POSTS past the maximum.
//! - InitializeCriticalSectionEx.
//! - A thread blocked on a condition variable, an SRW lock (exclusive or
//!   shared), a critical section or another thread's InitOnce sleeps: over
//!   300 ms its ContextSwitches (SystemProcessInformation) grow by at
//!   most a few (a waiter that polls would switch hundreds of times).
//!   Counted switches, not elapsed time (a loaded host only delays).
//! - No lost wake-ups: two threads hand a turn back and forth 3000 times
//!   through a condition variable with an SRW lock, with a critical
//!   section, and through WaitOnAddress; four threads (more than the
//!   CPUs, so they sleep) pass a turn round-robin through a condition
//!   variable and through WaitOnAddress, waking all; four threads enter one
//!   critical section 3000 times each (an exact count). A lost wake hangs
//!   it.
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

#[link(name = "api-ms-win-core-synch-l1-2-0", kind = "raw-dylib")]
unsafe extern "system" {
    // WaitOnAddress and WakeByAddressSingle: through their API set
    // (KernelBase on Windows, kernel32 on gala).
    fn WaitOnAddress(address: *mut c_void, compare: *const c_void, size: usize, ms: u32) -> Bool;
    fn WakeByAddressSingle(address: *mut c_void);
    fn WakeByAddressAll(address: *mut c_void);
}

#[allow(dead_code)]
#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "C" {
    fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memset(dst: *mut u8, value: i32, n: usize) -> *mut u8;
    fn memcmp(a: *const u8, b: *const u8, n: usize) -> i32;
}

#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "system" {
    fn NtQuerySystemInformation(class: u32, buf: *mut u8, len: u32, ret: *mut u32) -> i32;
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
    #[track_caller]
    fn ok(&mut self, cond: bool, got: u64) -> Result<(), u32> {
        self.0 += 1;
        if cond {
            return Ok(());
        }
        out(b"kernel32_sync_test: check ");
        out(&hex(self.0));
        out(b" got ");
        out(&hex(got as u32));
        check_line(core::panic::Location::caller().line());
        out(b"\r\n");
        Err(self.0)
    }

    #[track_caller]
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

/// The Context pointer the last once_ok call was given.
static ONCE_CONTEXT: AtomicUsize = AtomicUsize::new(usize::MAX);

extern "system" fn once_ok(_: *mut usize, param: *mut c_void, context: *mut *mut c_void) -> Bool {
    ONCE_CALLS.fetch_add(1, Ordering::SeqCst);
    ONCE_CONTEXT.store(context as usize, Ordering::SeqCst);
    // Windows passes the caller's Context as it is: NULL when the caller
    // gave none (a write through it would fault there).
    if !context.is_null() {
        // SAFETY: the caller's context output, passed through.
        unsafe { *context = param };
    }
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
        // The callback was given the caller's own Context pointer.
        c.eq(ONCE_CONTEXT.load(Ordering::SeqCst) as u64, (&raw mut context) as u64)?;
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
        // No Context: the callback gets NULL, not a pointer of kernel32's.
        c.eq(ONCE_CONTEXT.load(Ordering::SeqCst) as u64, 0)?;
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

// --- Waiters sleep ---------------------------------------------------------

const SYSTEM_PROCESS_INFORMATION: u32 = 5;
/// SYSTEM_PROCESS_INFORMATION's size on x64 (its threads follow it), and
/// SYSTEM_THREAD_INFORMATION's.
const PROCESS_ENTRY: usize = 0x100;
const THREAD_ENTRY: usize = 0x50;
/// How many times a thread blocked for SLEEP_MS may be switched to: a few
/// (Windows: none or one); a polling waiter is switched to every
/// millisecond or more often.
const MOST_SWITCHES: u64 = 10;
const SLEEP_MS: u32 = 300;

/// Room for SystemProcessInformation on a real machine (hundreds of
/// processes and thousands of threads; 64 KiB overflowed on Windows):
/// zero pages until the query writes them.
static SNAPSHOT: Shared<[u8; 1 << 23]> = Shared(core::cell::UnsafeCell::new([0; 1 << 23]));

/// Thread `tid`'s ContextSwitches (SystemProcessInformation), or None.
fn switches(tid: u32) -> Option<u64> {
    // SAFETY: only the main thread uses the buffer.
    let buf = unsafe { &mut *SNAPSHOT.get() };
    let mut ret = 0u32;
    // SAFETY: a buffer of ours.
    let st = unsafe {
        NtQuerySystemInformation(
            SYSTEM_PROCESS_INFORMATION,
            buf.as_mut_ptr(),
            buf.len() as u32,
            &mut ret,
        )
    };
    if st != 0 {
        return None;
    }
    let read = |at: usize, n: usize| -> Option<u64> {
        let b = buf.get(at..at + n)?;
        let mut v = [0u8; 8];
        v[..n].copy_from_slice(b);
        Some(u64::from_le_bytes(v))
    };
    let mut at = 0;
    loop {
        let threads = read(at + 4, 4)? as usize;
        for i in 0..threads {
            let t = at + PROCESS_ENTRY + THREAD_ENTRY * i;
            if read(t + 0x30, 8)? == u64::from(tid) {
                return read(t + 0x40, 4);
            }
        }
        match read(at, 4)? {
            0 => return None,
            next => at += next as usize,
        }
    }
}

static SLEEP_LOCK: Shared<usize> = Shared(core::cell::UnsafeCell::new(0));
static SLEEP_CV: Shared<usize> = Shared(core::cell::UnsafeCell::new(0));
static SLEEP_CS: Shared<CriticalSection> =
    Shared(core::cell::UnsafeCell::new(CriticalSection([0; 5])));
static SLEEP_ONCE: Shared<usize> = Shared(core::cell::UnsafeCell::new(0));
/// Set to let the condition-variable and InitOnce waits end.
static RELEASED: AtomicU32 = AtomicU32::new(0);
/// Set by a sleeper thread just before it blocks.
static BLOCKING: AtomicU32 = AtomicU32::new(0);

extern "system" fn cv_sleeper(_: *mut c_void) -> u32 {
    // SAFETY: the test's lock and variable.
    unsafe {
        AcquireSRWLockExclusive(SLEEP_LOCK.get());
        BLOCKING.store(1, Ordering::SeqCst);
        while RELEASED.load(Ordering::SeqCst) == 0 {
            SleepConditionVariableSRW(SLEEP_CV.get(), SLEEP_LOCK.get(), INFINITE, 0);
        }
        ReleaseSRWLockExclusive(SLEEP_LOCK.get());
    }
    0
}

extern "system" fn srw_sleeper(_: *mut c_void) -> u32 {
    BLOCKING.store(1, Ordering::SeqCst);
    // SAFETY: the test's lock, held exclusively by the main thread.
    unsafe {
        AcquireSRWLockExclusive(SLEEP_LOCK.get());
        ReleaseSRWLockExclusive(SLEEP_LOCK.get());
    }
    0
}

extern "system" fn shared_sleeper(_: *mut c_void) -> u32 {
    BLOCKING.store(1, Ordering::SeqCst);
    // SAFETY: as above.
    unsafe {
        AcquireSRWLockShared(SLEEP_LOCK.get());
        ReleaseSRWLockShared(SLEEP_LOCK.get());
    }
    0
}

extern "system" fn cs_sleeper(_: *mut c_void) -> u32 {
    BLOCKING.store(1, Ordering::SeqCst);
    // SAFETY: the test's critical section, held by the main thread.
    unsafe {
        EnterCriticalSection(SLEEP_CS.get());
        LeaveCriticalSection(SLEEP_CS.get());
    }
    0
}

/// An InitOnce callback that runs until RELEASED.
extern "system" fn once_slow(_: *mut usize, _: *mut c_void, _: *mut *mut c_void) -> Bool {
    BLOCKING.store(1, Ordering::SeqCst);
    while RELEASED.load(Ordering::SeqCst) == 0 {
        // SAFETY: a plain sleep.
        unsafe { Sleep(10) };
    }
    1
}

extern "system" fn once_runner(_: *mut c_void) -> u32 {
    // SAFETY: the test's INIT_ONCE.
    unsafe {
        InitOnceExecuteOnce(
            SLEEP_ONCE.get(),
            once_slow,
            core::ptr::null_mut(),
            core::ptr::null_mut(),
        )
    };
    0
}

extern "system" fn once_sleeper(_: *mut c_void) -> u32 {
    BLOCKING.store(1, Ordering::SeqCst);
    // SAFETY: as above; the runner is in the callback.
    unsafe {
        InitOnceExecuteOnce(SLEEP_ONCE.get(), once_ok, core::ptr::null_mut(), core::ptr::null_mut())
    };
    0
}

/// Starts `f`, waits until it is about to block (and a little more, for
/// it to get there), counts its switches over SLEEP_MS, runs `release`,
/// joins it; checks the count.
fn sleeps(c: &mut Checks, f: StartRoutine, release: impl FnOnce()) -> Result<(), u32> {
    BLOCKING.store(0, Ordering::SeqCst);
    let mut tid = 0u32;
    // SAFETY: a thread running `f`.
    let h =
        unsafe { CreateThread(core::ptr::null_mut(), 0, f, core::ptr::null_mut(), 0, &mut tid) };
    c.ok(!h.is_null(), 0)?;
    while BLOCKING.load(Ordering::SeqCst) == 0 {
        // SAFETY: a plain sleep.
        unsafe { Sleep(1) };
    }
    // SAFETY: a plain sleep.
    unsafe { Sleep(50) };
    let before = switches(tid);
    c.ok(before.is_some(), 0)?;
    // SAFETY: a plain sleep.
    unsafe { Sleep(SLEEP_MS) };
    let after = switches(tid);
    c.ok(after.is_some(), 0)?;
    let grew = after.unwrap_or(0).wrapping_sub(before.unwrap_or(0));
    release();
    c.ok(join(h), 0)?;
    c.ok(grew <= MOST_SWITCHES, grew)
}

fn waiters_sleep(c: &mut Checks) -> Result<(), u32> {
    let lock = SLEEP_LOCK.get();
    // SAFETY: the test's own objects.
    unsafe {
        InitializeSRWLock(lock);
        InitializeConditionVariable(SLEEP_CV.get());
        InitializeCriticalSection(SLEEP_CS.get());
    }
    // A condition variable.
    sleeps(c, cv_sleeper, || {
        // SAFETY: as above.
        unsafe {
            AcquireSRWLockExclusive(lock);
            RELEASED.store(1, Ordering::SeqCst);
            ReleaseSRWLockExclusive(lock);
            WakeConditionVariable(SLEEP_CV.get());
        }
    })?;
    // An SRW lock held exclusively: an exclusive and a shared waiter.
    // SAFETY: as above.
    unsafe { AcquireSRWLockExclusive(lock) };
    sleeps(c, srw_sleeper, || unsafe { ReleaseSRWLockExclusive(lock) })?;
    // SAFETY: as above.
    unsafe { AcquireSRWLockExclusive(lock) };
    sleeps(c, shared_sleeper, || unsafe { ReleaseSRWLockExclusive(lock) })?;
    // A critical section.
    // SAFETY: as above.
    unsafe { EnterCriticalSection(SLEEP_CS.get()) };
    sleeps(c, cs_sleeper, || unsafe { LeaveCriticalSection(SLEEP_CS.get()) })?;
    // SAFETY: as above.
    unsafe { DeleteCriticalSection(SLEEP_CS.get()) };
    // Another thread's InitOnce callback running.
    RELEASED.store(0, Ordering::SeqCst);
    BLOCKING.store(0, Ordering::SeqCst);
    let runner = start(once_runner);
    while BLOCKING.load(Ordering::SeqCst) == 0 {
        // SAFETY: a plain sleep.
        unsafe { Sleep(1) };
    }
    sleeps(c, once_sleeper, || RELEASED.store(1, Ordering::SeqCst))?;
    c.ok(join(runner), 0)
}

// ---- Lost-wakeup stress: hand-offs that hang if a wake is ever lost ----

/// Hand-offs per pair of threads (under TCG a few seconds in all).
const HANDOFFS: u32 = 3000;

static PP_LOCK: Shared<usize> = Shared(core::cell::UnsafeCell::new(0));
static PP_CV: Shared<usize> = Shared(core::cell::UnsafeCell::new(0));
static PP_CS: Shared<CriticalSection> =
    Shared(core::cell::UnsafeCell::new(CriticalSection([0; 5])));
static PP_CS_CV: Shared<usize> = Shared(core::cell::UnsafeCell::new(0));
/// Whose turn it is (0 or 1) and how many hand-offs were made.
static TURN: AtomicU32 = AtomicU32::new(0);
static DONE: AtomicU32 = AtomicU32::new(0);

/// Takes `me`'s turns: waits on the condition variable (SRW lock) until
/// TURN is `me`, passes the turn and wakes the other side.
fn srw_ping_pong(me: u32) {
    for _ in 0..HANDOFFS {
        // SAFETY: the test's lock and condition variable.
        unsafe {
            AcquireSRWLockExclusive(PP_LOCK.get());
            while TURN.load(Ordering::Relaxed) != me {
                SleepConditionVariableSRW(PP_CV.get(), PP_LOCK.get(), INFINITE, 0);
            }
            TURN.store(1 - me, Ordering::Relaxed);
            DONE.fetch_add(1, Ordering::Relaxed);
            ReleaseSRWLockExclusive(PP_LOCK.get());
            WakeConditionVariable(PP_CV.get());
        }
    }
}

extern "system" fn srw_pong(_: *mut c_void) -> u32 {
    srw_ping_pong(1);
    0
}

/// The same with a critical section, the wake given while holding it.
fn cs_ping_pong(me: u32) {
    for _ in 0..HANDOFFS {
        // SAFETY: the test's critical section and condition variable.
        unsafe {
            EnterCriticalSection(PP_CS.get());
            while TURN.load(Ordering::Relaxed) != me {
                SleepConditionVariableCS(PP_CS_CV.get(), PP_CS.get(), INFINITE);
            }
            TURN.store(1 - me, Ordering::Relaxed);
            DONE.fetch_add(1, Ordering::Relaxed);
            WakeAllConditionVariable(PP_CS_CV.get());
            LeaveCriticalSection(PP_CS.get());
        }
    }
}

extern "system" fn cs_pong(_: *mut c_void) -> u32 {
    cs_ping_pong(1);
    0
}

/// WaitOnAddress hand-offs on TURN itself.
fn address_ping_pong(me: u32) {
    for _ in 0..HANDOFFS {
        loop {
            let seen = TURN.load(Ordering::SeqCst);
            if seen == me {
                break;
            }
            // SAFETY: TURN is a 4-byte value; `seen` is what we compare.
            unsafe { WaitOnAddress(TURN.as_ptr().cast(), (&raw const seen).cast(), 4, INFINITE) };
        }
        DONE.fetch_add(1, Ordering::Relaxed);
        TURN.store(1 - me, Ordering::SeqCst);
        // SAFETY: TURN's address.
        unsafe { WakeByAddressSingle(TURN.as_ptr().cast()) };
    }
}

extern "system" fn address_pong(_: *mut c_void) -> u32 {
    address_ping_pong(1);
    0
}

/// Round-robin among RING threads (more than the CPUs, so waiters really
/// sleep): thread `me` waits until TURN is `me`, passes it to the next
/// and wakes everyone. RING_ID hands each thread its number.
const RING: u32 = 4;
static RING_ID: AtomicU32 = AtomicU32::new(0);

extern "system" fn srw_ring(_: *mut c_void) -> u32 {
    let me = RING_ID.fetch_add(1, Ordering::SeqCst);
    for _ in 0..HANDOFFS / 4 {
        // SAFETY: the test's lock and condition variable.
        unsafe {
            AcquireSRWLockExclusive(PP_LOCK.get());
            while TURN.load(Ordering::Relaxed) != me {
                SleepConditionVariableSRW(PP_CV.get(), PP_LOCK.get(), INFINITE, 0);
            }
            TURN.store((me + 1) % RING, Ordering::Relaxed);
            DONE.fetch_add(1, Ordering::Relaxed);
            ReleaseSRWLockExclusive(PP_LOCK.get());
            WakeAllConditionVariable(PP_CV.get());
        }
    }
    0
}

extern "system" fn address_ring(_: *mut c_void) -> u32 {
    let me = RING_ID.fetch_add(1, Ordering::SeqCst);
    for _ in 0..HANDOFFS / 4 {
        loop {
            let seen = TURN.load(Ordering::SeqCst);
            if seen == me {
                break;
            }
            // SAFETY: TURN is a 4-byte value; `seen` is what we compare.
            unsafe { WaitOnAddress(TURN.as_ptr().cast(), (&raw const seen).cast(), 4, INFINITE) };
        }
        DONE.fetch_add(1, Ordering::Relaxed);
        TURN.store((me + 1) % RING, Ordering::SeqCst);
        // SAFETY: TURN's address.
        unsafe { WakeByAddressAll(TURN.as_ptr().cast()) };
    }
    0
}

/// Four threads entering one critical section, each many times: the
/// count is exact and nobody is left asleep.
static CONTENDED: Shared<CriticalSection> =
    Shared(core::cell::UnsafeCell::new(CriticalSection([0; 5])));
static CONTENDED_COUNT: Shared<u64> = Shared(core::cell::UnsafeCell::new(0));

extern "system" fn contend(_: *mut c_void) -> u32 {
    for _ in 0..HANDOFFS {
        // SAFETY: the test's critical section guards the count.
        unsafe {
            EnterCriticalSection(CONTENDED.get());
            *CONTENDED_COUNT.get() += 1;
            LeaveCriticalSection(CONTENDED.get());
        }
    }
    0
}

/// Lost-wakeup stress: each hand-off needs the other thread woken, so a
/// single lost wake hangs the scenario (its timeout's state dump then
/// names the keyed-event wait, its caller and key).
fn lost_wakeups(c: &mut Checks) -> Result<(), u32> {
    let pairs: [(fn(u32), StartRoutine); 3] =
        [(srw_ping_pong, srw_pong), (cs_ping_pong, cs_pong), (address_ping_pong, address_pong)];
    // SAFETY: the test's critical sections.
    unsafe {
        InitializeCriticalSection(PP_CS.get());
        InitializeCriticalSection(CONTENDED.get());
    }
    for (ping, pong) in pairs {
        TURN.store(0, Ordering::SeqCst);
        DONE.store(0, Ordering::SeqCst);
        let other = start(pong);
        ping(0);
        c.ok(join(other), 0)?;
        c.eq(DONE.load(Ordering::SeqCst).into(), u64::from(2 * HANDOFFS))?;
    }
    for ring in [srw_ring as StartRoutine, address_ring] {
        TURN.store(0, Ordering::SeqCst);
        DONE.store(0, Ordering::SeqCst);
        RING_ID.store(0, Ordering::SeqCst);
        let threads = [start(ring), start(ring), start(ring), start(ring)];
        for t in threads {
            c.ok(join(t), 0)?;
        }
        c.eq(DONE.load(Ordering::SeqCst).into(), u64::from(HANDOFFS))?;
    }
    let threads = [start(contend), start(contend), start(contend), start(contend)];
    for t in threads {
        c.ok(join(t), 0)?;
    }
    // SAFETY: every thread has finished.
    c.eq(unsafe { *CONTENDED_COUNT.get() }, 4 * u64::from(HANDOFFS))?;
    Ok(())
}

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    srw(&mut c)?;
    condition_variables(&mut c)?;
    init_once(&mut c)?;
    fls(&mut c)?;
    slist(&mut c)?;
    semaphores(&mut c)?;
    waiters_sleep(&mut c)?;
    lost_wakeups(&mut c)
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

/// ` (line L)`: the source line of the failing check, from
/// `#[track_caller]` on Checks' methods (so a report names the line
/// without anyone counting checks).
fn check_line(line: u32) {
    let mut b = *b" (line 0000000000)";
    let mut digits = [0u8; 10];
    let (mut n, mut len) = (line, 0);
    loop {
        digits[len] = b'0' + (n % 10) as u8;
        len += 1;
        n /= 10;
        if n == 0 {
            break;
        }
    }
    for i in 0..len {
        b[7 + i] = digits[len - 1 - i];
    }
    b[7 + len] = b')';
    out(&b[..8 + len]);
}
