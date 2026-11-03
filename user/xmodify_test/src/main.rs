//! Code patched by one processor and then run by another, as a JIT's
//! garbage collector does it (HotSpot rewrites the object addresses
//! embedded in compiled code while the threads that run it are stopped at
//! a safepoint, then lets them go): on real Windows the thread that runs
//! the code after it is released sees the new instructions.
//!
//! A thread pinned to processor 1 calls small functions in
//! PAGE_EXECUTE_READWRITE memory (`mov eax, imm32; ret`), one per 256
//! bytes over four pages; the main thread on processor 0 rewrites their
//! immediates. Phase 1: the runner waits on an event between rounds (as a
//! thread stopped at a safepoint waits in the kernel) and every function
//! must return what was written last. Phase 2: the runner never enters
//! the kernel; it spins on a counter, executes CPUID (serializing, as the
//! architecture asks of the processor that runs modified code) and then
//! calls the functions, which must return at least the round it saw.
//! Phase 3: the runner calls the functions in a tight loop while the main
//! thread rewrites them; every value read must be one that was written.
//! Phase 4: phase 1 again, the main thread also calling each function
//! after rewriting it: code translated while it changed (an emulator's
//! cache of translated code) must not outlive the next change.
//!
//! Prints `xmodify_test: ok` or exits with the number of the failing check.

#![no_std]
#![no_main]

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};

use ntrt::*;

const THREAD_AFFINITY_MASK: u32 = 4;
const PAGE_EXECUTE_READWRITE: u32 = 0x40;
const PAGES: usize = 4;
const STRIDE: usize = 256;
const FUNCS: usize = PAGES * 0x1000 / STRIDE;
const ROUNDS: u32 = 3000;
/// Phase 3: rounds while the runner keeps calling.
const LIVE_ROUNDS: u32 = 40_000;
/// Phase 4's first round.
const FINAL: u32 = 2 * ROUNDS + LIVE_ROUNDS + 1;

static CODE: AtomicUsize = AtomicUsize::new(0);
static GO: AtomicUsize = AtomicUsize::new(0);
static DONE: AtomicUsize = AtomicUsize::new(0);
static ROUND: AtomicU32 = AtomicU32::new(0);
static STOP: AtomicBool = AtomicBool::new(false);
/// The runner has left phase 3.
static STOPPED: AtomicBool = AtomicBool::new(false);
/// First wrong value seen: (phase << 60) | (function << 40) | (round << 20) | got.
static BAD: AtomicUsize = AtomicUsize::new(0);
static GO_EVENT: AtomicUsize = AtomicUsize::new(0);
static DONE_EVENT: AtomicUsize = AtomicUsize::new(0);

fn check(id: u32, cond: bool, got: u64) -> Result<(), u32> {
    if cond {
        return Ok(());
    }
    print("xmodify_test: check ");
    write_file(stdout(), &hex32(id));
    print(" got ");
    write_file(stdout(), &hex32((got >> 32) as u32));
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

/// The value function `i` returns for round `round` (distinct per function).
fn value(i: usize, round: u32) -> u32 {
    round << 8 | i as u32
}

/// Function `i`: three NOPs, `mov eax, imm32` (the immediate 4-byte
/// aligned), `ret`.
fn write_function(base: usize, i: usize, imm: u32) {
    let f = base + i * STRIDE;
    let mut bytes = [0x90u8, 0x90, 0x90, 0xb8, 0, 0, 0, 0, 0xc3];
    bytes[4..8].copy_from_slice(&imm.to_le_bytes());
    for (k, b) in bytes.iter().enumerate() {
        // SAFETY: inside the committed RWX pages.
        unsafe { ((f + k) as *mut u8).write_volatile(*b) };
    }
}

fn patch(base: usize, i: usize, imm: u32) {
    let at = base + i * STRIDE + 4;
    // SAFETY: the aligned immediate of function `i`; one 4-byte store, as
    // a JIT patches an embedded constant.
    unsafe { (*(at as *const AtomicU32)).store(imm, Ordering::Release) };
}

fn call(base: usize, i: usize) -> u32 {
    let f = base + i * STRIDE;
    // SAFETY: function `i` is `mov eax, imm32; ret`.
    let f: extern "C" fn() -> u32 = unsafe { core::mem::transmute(f) };
    f()
}

fn serialize() {
    // CPUID serializes instruction fetch (and has no other effect).
    core::arch::x86_64::__cpuid(0);
}

fn note(phase: usize, i: usize, round: u32, got: u32) {
    let v = phase << 60 | i << 40 | (round as usize & 0xf_ffff) << 20 | (got as usize & 0xf_ffff);
    let _ = BAD.compare_exchange(0, v, Ordering::Relaxed, Ordering::Relaxed);
}

extern "C" fn runner(_: usize) -> ! {
    if pin(1) != STATUS_SUCCESS {
        exit_thread(1);
    }
    let base = CODE.load(Ordering::Acquire);
    let go = Handle(GO_EVENT.load(Ordering::Acquire));
    let done = Handle(DONE_EVENT.load(Ordering::Acquire));
    // Phase 1: released through the kernel.
    for round in 1..=ROUNDS {
        if wait(go, None) != STATUS_SUCCESS {
            exit_thread(2);
        }
        for i in 0..FUNCS {
            let got = call(base, i);
            if got != value(i, round) {
                note(1, i, round, got);
            }
        }
        set_event(done);
    }
    // Phase 2: released through memory, then CPUID.
    for round in ROUNDS + 1..=2 * ROUNDS {
        while GO.load(Ordering::Acquire) < round as usize {
            core::hint::spin_loop();
        }
        serialize();
        for i in 0..FUNCS {
            let got = call(base, i);
            if got >> 8 < round {
                note(2, i, round, got);
            }
        }
        DONE.store(round as usize, Ordering::Release);
    }
    // Phase 3: calls while the code changes; values must be ones written
    // (a round's value or the one before it, never a mix).
    while !STOP.load(Ordering::Acquire) {
        for i in 0..FUNCS {
            let got = call(base, i);
            if got & 0xff != i as u32 || got >> 8 >= FINAL {
                note(3, i, ROUND.load(Ordering::Relaxed), got);
            }
        }
    }
    STOPPED.store(true, Ordering::Release);
    // Phase 4: as phase 1, after code was translated while it changed.
    for round in FINAL - 1..FINAL + ROUNDS {
        if wait(go, None) != STATUS_SUCCESS {
            exit_thread(2);
        }
        for i in 0..FUNCS {
            let got = call(base, i);
            if got != value(i, round) {
                note(4, i, round, got);
            }
        }
        set_event(done);
    }
    exit_thread(STATUS_SUCCESS)
}

fn main() -> u32 {
    match run() {
        Ok(()) => {
            print("xmodify_test: ok\r\n");
            0
        }
        Err(id) => id,
    }
}

fn run() -> Result<(), u32> {
    let mut b = [0u8; 64];
    status(1, query_system(0, &mut b).0)?;
    let two = b[56] >= 2;
    status(2, pin(0))?;
    let (s, base, _) =
        allocate(0, PAGES * 0x1000, MEM_RESERVE | MEM_COMMIT, PAGE_EXECUTE_READWRITE);
    status(3, s)?;
    for i in 0..FUNCS {
        write_function(base, i, value(i, 0));
    }
    for i in 0..FUNCS {
        check(4, call(base, i) == value(i, 0), 0)?;
    }
    if !two {
        print("xmodify_test: one processor, phases skipped\r\n");
        return Ok(());
    }
    let (s, go) = create_event(EVENT_ALL_ACCESS, SYNCHRONIZATION_EVENT, false);
    status(5, s)?;
    let (s, done) = create_event(EVENT_ALL_ACCESS, SYNCHRONIZATION_EVENT, false);
    status(5, s)?;
    GO_EVENT.store(go.0, Ordering::Release);
    DONE_EVENT.store(done.0, Ordering::Release);
    CODE.store(base, Ordering::Release);
    let (s, thread, _) = create_thread(runner, 0, false);
    status(6, s)?;
    // Phase 1.
    for round in 1..=ROUNDS {
        for i in 0..FUNCS {
            patch(base, i, value(i, round));
        }
        set_event(go);
        status(7, wait(done, Some(-100_000_000)))?;
        let bad = BAD.load(Ordering::Relaxed);
        check(8, bad == 0, bad as u64)?;
    }
    // Phase 2.
    for round in ROUNDS + 1..=2 * ROUNDS {
        for i in 0..FUNCS {
            patch(base, i, value(i, round));
        }
        GO.store(round as usize, Ordering::Release);
        while DONE.load(Ordering::Acquire) < round as usize {
            core::hint::spin_loop();
        }
        let bad = BAD.load(Ordering::Relaxed);
        check(9, bad == 0, bad as u64)?;
    }
    // Phase 3.
    for round in 2 * ROUNDS + 1..=2 * ROUNDS + LIVE_ROUNDS {
        ROUND.store(round, Ordering::Relaxed);
        for i in 0..FUNCS {
            patch(base, i, value(i, round));
        }
    }
    STOP.store(true, Ordering::Release);
    while !STOPPED.load(Ordering::Acquire) {
        core::hint::spin_loop();
    }
    let bad = BAD.load(Ordering::Relaxed);
    check(11, bad == 0, bad as u64)?;
    // Phase 4, and each round the writer runs the code itself too.
    // Its first round rewrites nothing: the code must hold phase 3's last
    // values, for this thread and (released through the kernel) for the
    // runner.
    for round in FINAL - 1..FINAL + ROUNDS {
        for i in (0..FUNCS).filter(|_| round >= FINAL) {
            patch(base, i, value(i, round));
        }
        for i in 0..FUNCS {
            let got = call(base, i);
            check(12, got == value(i, round), (i as u64) << 32 | u64::from(got))?;
        }
        set_event(go);
        status(13, wait(done, Some(-100_000_000)))?;
        let bad = BAD.load(Ordering::Relaxed);
        check(14, bad == 0, bad as u64)?;
    }
    status(10, wait(thread, Some(-100_000_000)))?;
    close(thread);
    Ok(())
}

entry!(main);
