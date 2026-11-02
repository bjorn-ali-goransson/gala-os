//! A semaphore's count under threads on two processors at once, as on real
//! Windows: two threads pinned to processors 0 and 1 each release one
//! semaphore (NtReleaseSemaphore, count 1) and take it back without waiting
//! (NtWaitForSingleObject with a zero timeout), in step, many times. Every
//! release adds exactly one to the count and every satisfied wait takes
//! exactly one away, so at the end the waits satisfied plus the count left
//! must equal the releases. A second round has one thread only release and
//! the other only take (with an infinite wait), so a lost release shows as
//! a taker that never wakes.
//!
//! NtReleaseSemaphore read the count, let go of the dispatcher lock, and
//! then stored the count plus one: a wait satisfied on the other processor
//! in between was undone (a count too many: a thread waiting at a JVM
//! safepoint's wait barrier, Windows semaphores in HotSpot, then left the
//! barrier while the garbage collector was moving objects), and two
//! releases at once added one (a lost wake-up: a hang). gala before
//! 2026-11-02 failed check 5 or 9.
//!
//! Needs NumberOfProcessors >= 2 (else one thread does both sides, which
//! still checks the counting).
//! Prints `smpsem_test: ok` or exits with the number of the failing check.

#![no_std]
#![no_main]

use core::arch::global_asm;
use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

use ntrt::*;

const THREAD_AFFINITY_MASK: u32 = 4;
const SEMAPHORE_ALL_ACCESS: u32 = 0x1f_0003;
const STATUS_TIMEOUT: NtStatus = 0x102;
/// Iterations per thread in each round.
const ROUNDS: u32 = 20_000;

// The semaphore services of the pinned build (ntdll's stub shape; the
// numbers match kernel/src/nt/syscalls.rs).
global_asm!(
    ".globl smpsem_create_semaphore",
    "smpsem_create_semaphore:",
    "mov r10, rcx",
    "mov eax, 0xcf",
    "syscall",
    "ret",
    ".globl smpsem_release_semaphore",
    "smpsem_release_semaphore:",
    "mov r10, rcx",
    "mov eax, 0x0a",
    "syscall",
    "ret",
    ".globl smpsem_query_semaphore",
    "smpsem_query_semaphore:",
    "mov r10, rcx",
    "mov eax, 0x15a",
    "syscall",
    "ret",
);

unsafe extern "C" {
    fn smpsem_create_semaphore(
        handle: *mut Handle,
        access: u32,
        attributes: *const ObjectAttributes,
        initial: i32,
        maximum: i32,
    ) -> NtStatus;
    fn smpsem_release_semaphore(handle: Handle, count: i32, previous: *mut i32) -> NtStatus;
    fn smpsem_query_semaphore(
        handle: Handle,
        class: u32,
        info: *mut [i32; 2],
        length: u32,
        returned: *mut u32,
    ) -> NtStatus;
}

fn release(h: Handle) -> NtStatus {
    let mut previous = -1;
    // SAFETY: valid output pointer; the handle is checked by the kernel.
    unsafe { smpsem_release_semaphore(h, 1, &mut previous) }
}

/// SEMAPHORE_BASIC_INFORMATION's CurrentCount.
fn current_count(h: Handle) -> (NtStatus, i32) {
    let mut info = [-1; 2];
    let mut returned = 0;
    // SAFETY: an 8-byte buffer and a valid output pointer.
    let s = unsafe { smpsem_query_semaphore(h, 0, &mut info, 8, &mut returned) };
    (s, info[0])
}

/// Check `id` (also the exit status when it fails).
fn check(id: u32, cond: bool, got: u64) -> Result<(), u32> {
    if cond {
        return Ok(());
    }
    print("smpsem_test: check ");
    write_file(stdout(), &hex32(id));
    print(" got ");
    write_file(stdout(), &hex32(got as u32));
    print("\r\n");
    Err(id)
}

fn status(id: u32, s: NtStatus) -> Result<(), u32> {
    check(id, s == STATUS_SUCCESS, s.into())
}

fn pin(cpu: usize) -> NtStatus {
    set_thread(CURRENT_THREAD, THREAD_AFFINITY_MASK, &(1u64 << cpu).to_le_bytes())
}

static SEMAPHORE: AtomicUsize = AtomicUsize::new(0);
static WORKERS: AtomicUsize = AtomicUsize::new(2);
/// Iterations each worker has done in the current round (kept in step).
static AT: [AtomicU32; 2] = [const { AtomicU32::new(0) }; 2];
/// Waits each worker had satisfied (round 1).
static TAKEN: [AtomicU32; 2] = [const { AtomicU32::new(0) }; 2];
/// The first unexpected status a worker saw.
static FAILED: AtomicU32 = AtomicU32::new(0);

/// Waits until the other worker is at most one iteration behind.
fn keep_step(t: usize, i: u32) {
    if WORKERS.load(Ordering::Relaxed) == 2 {
        let other = 1 - t;
        while AT[other].load(Ordering::Acquire) + 1 < i {
            core::hint::spin_loop();
        }
    }
    AT[t].store(i, Ordering::Release);
}

/// Round 1, worker `t` (pinned to processor `t`): release, then take
/// without waiting, ROUNDS times.
extern "C" fn release_and_take(t: usize) -> ! {
    if pin(t) != STATUS_SUCCESS {
        exit_thread(1);
    }
    let h = Handle(SEMAPHORE.load(Ordering::Acquire));
    let mut taken = 0;
    for i in 1..=ROUNDS {
        keep_step(t, i);
        let s = release(h);
        if s != STATUS_SUCCESS {
            FAILED.store(s, Ordering::Relaxed);
        }
        match wait(h, Some(0)) {
            STATUS_SUCCESS => taken += 1,
            STATUS_TIMEOUT => {}
            s => FAILED.store(s, Ordering::Relaxed),
        }
    }
    TAKEN[t].store(taken, Ordering::Release);
    exit_thread(STATUS_SUCCESS)
}

/// Round 2, worker 0: only releases.
extern "C" fn releaser(t: usize) -> ! {
    if pin(t) != STATUS_SUCCESS {
        exit_thread(1);
    }
    let h = Handle(SEMAPHORE.load(Ordering::Acquire));
    for i in 1..=ROUNDS {
        keep_step(t, i);
        let s = release(h);
        if s != STATUS_SUCCESS {
            FAILED.store(s, Ordering::Relaxed);
        }
    }
    exit_thread(STATUS_SUCCESS)
}

/// Round 2, worker 1: takes ROUNDS times, waiting as long as it takes
/// (a lost release leaves it waiting: the main thread's wait times out).
extern "C" fn taker(t: usize) -> ! {
    if pin(t) != STATUS_SUCCESS {
        exit_thread(1);
    }
    let h = Handle(SEMAPHORE.load(Ordering::Acquire));
    for i in 1..=ROUNDS {
        keep_step(t, i);
        let s = wait(h, None);
        if s != STATUS_SUCCESS {
            FAILED.store(s, Ordering::Relaxed);
        }
    }
    exit_thread(STATUS_SUCCESS)
}

/// Starts `workers` threads running `entry`, resets the step counters.
fn start(
    workers: usize,
    entry: [extern "C" fn(usize) -> !; 2],
    threads: &mut [Handle; 2],
) -> Result<(), u32> {
    for a in &AT {
        a.store(0, Ordering::Relaxed);
    }
    for (t, h) in threads.iter_mut().enumerate().take(workers) {
        let (s, thread, _) = create_thread(entry[t], t, false);
        status(3, s)?;
        *h = thread;
    }
    Ok(())
}

/// Waits (at most 120 s) for the threads and checks their exit status.
fn join(threads: &[Handle], id: u32) -> Result<(), u32> {
    for &h in threads {
        status(id, wait(h, Some(-1_200_000_000)))?;
        let (_, code, _, _) = thread_basic_info(h);
        status(id + 1, code)?;
        close(h);
    }
    Ok(())
}

fn run() -> Result<(), u32> {
    let mut b = [0u8; 64];
    status(1, query_system(0, &mut b).0)?;
    let cpus = usize::from(b[56]);
    let workers = cpus.clamp(1, 2);
    WORKERS.store(workers, Ordering::Relaxed);

    let mut h = Handle(0);
    // SAFETY: valid output pointer, no attributes.
    let s = unsafe {
        smpsem_create_semaphore(&mut h, SEMAPHORE_ALL_ACCESS, core::ptr::null(), 0, i32::MAX)
    };
    status(2, s)?;
    SEMAPHORE.store(h.0, Ordering::Release);

    // Round 1: both release and take.
    let mut threads = [Handle(0); 2];
    start(workers, [release_and_take, release_and_take], &mut threads)?;
    join(&threads[..workers], 6)?;
    status(4, FAILED.load(Ordering::Relaxed))?;
    let taken: u32 = TAKEN[..workers].iter().map(|t| t.load(Ordering::Acquire)).sum();
    let (s, left) = current_count(h);
    status(8, s)?;
    // Releases = waits satisfied + count left; a difference is the count
    // gained (above 0x8000_0000: lost) by the race.
    let released = ROUNDS * workers as u32;
    check(5, taken + left as u32 == released, (taken + left as u32).wrapping_sub(released).into())?;
    // Empty it again.
    while wait(h, Some(0)) == STATUS_SUCCESS {}

    // Round 2: one releases, the other takes (both sides on one processor
    // when there is only one).
    if workers == 2 {
        start(2, [releaser, taker], &mut threads)?;
        let mut done = true;
        for &t in &threads {
            done &= wait(t, Some(-1_200_000_000)) == STATUS_SUCCESS;
        }
        // A taker still waiting: a release was lost.
        check(9, done, u64::from(AT[1].load(Ordering::Relaxed)))?;
        join(&threads, 10)?;
        status(12, FAILED.load(Ordering::Relaxed))?;
        let (s, left) = current_count(h);
        status(13, s)?;
        // Every release taken, none made up.
        check(14, left == 0, left as u64)?;
    }
    close(h);
    Ok(())
}

fn main() -> u32 {
    match run() {
        Ok(()) => {
            print("smpsem_test: ok\r\n");
            0
        }
        Err(id) => id,
    }
}

entry!(main);
