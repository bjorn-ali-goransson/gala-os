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
//! ARM64: `movz w0; movk w0; ret`, and after each rewrite the writer
//! runs the cache maintenance Arm requires for modified code (clean to
//! the point of unification, invalidate the instruction cache: the kernel
//! lets user mode, as FlushInstructionCache and a JIT need); phase 2's
//! serializing instruction is ISB; no phase 3 (`LIVE_ROUNDS`).
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
/// Phase 3: rounds while the runner keeps calling. None on ARM64: code
/// changed while another processor runs it is architecturally defined
/// there only for a few instructions (B, BL, NOP, BRK, SVC, ...; Arm's
/// "Concurrent modification and execution of instructions"), not the
/// MOVZ/MOVK pair a JIT patches, which it changes with the runner stopped.
const LIVE_ROUNDS: u32 = if cfg!(target_arch = "x86_64") { 40_000 } else { 0 };
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

/// Function `i` (x64): three NOPs, `mov eax, imm32` (the immediate 4-byte
/// aligned), `ret`.
#[cfg(target_arch = "x86_64")]
fn write_function(base: usize, i: usize, imm: u32) {
    let f = base + i * STRIDE;
    let mut bytes = [0x90u8, 0x90, 0x90, 0xb8, 0, 0, 0, 0, 0xc3];
    bytes[4..8].copy_from_slice(&imm.to_le_bytes());
    for (k, b) in bytes.iter().enumerate() {
        // SAFETY: inside the committed RWX pages.
        unsafe { ((f + k) as *mut u8).write_volatile(*b) };
    }
}

#[cfg(target_arch = "x86_64")]
fn patch(base: usize, i: usize, imm: u32) {
    let at = base + i * STRIDE + 4;
    // SAFETY: the aligned immediate of function `i`; one 4-byte store, as
    // a JIT patches an embedded constant.
    unsafe { (*(at as *const AtomicU32)).store(imm, Ordering::Release) };
}

/// x64 keeps instruction fetch coherent with stores: nothing to do after
/// patching.
#[cfg(target_arch = "x86_64")]
fn publish(_base: usize) {}

#[cfg(target_arch = "x86_64")]
fn serialize() {
    // CPUID serializes instruction fetch (and has no other effect).
    core::arch::x86_64::__cpuid(0);
}

/// Function `i` (ARM64): `movz w0, #lo; movk w0, #hi, lsl #16` (one
/// 8-byte aligned pair), `ret`.
#[cfg(target_arch = "aarch64")]
fn write_function(base: usize, i: usize, imm: u32) {
    patch(base, i, imm);
    // SAFETY: inside the committed RWX pages.
    unsafe { ((base + i * STRIDE + 8) as *mut u32).write_volatile(0xd65f_03c0) }; // ret
}

/// One 8-byte store of the instruction pair, as a JIT patches a constant
/// it materializes.
#[cfg(target_arch = "aarch64")]
fn patch(base: usize, i: usize, imm: u32) {
    let movz = 0x5280_0000 | ((imm & 0xffff) << 5); // movz w0, #lo
    let movk = 0x72a0_0000 | ((imm >> 16) << 5); // movk w0, #hi, lsl #16
    let pair = u64::from(movz) | u64::from(movk) << 32;
    // SAFETY: the 8-byte aligned pair of function `i`.
    unsafe {
        (*((base + i * STRIDE) as *const core::sync::atomic::AtomicU64))
            .store(pair, Ordering::Release)
    };
}

/// ARM64 does not keep instruction fetch coherent with stores: after
/// patching, the code's data cache lines are cleaned to the point of
/// unification and its instruction cache lines invalidated (Arm's
/// sequence for modified code; lines of 4 << CTR_EL0's minimum sizes).
#[cfg(target_arch = "aarch64")]
fn publish(base: usize) {
    let ctr: u64;
    // SAFETY: reads CTR_EL0 (the kernel lets user mode).
    unsafe { core::arch::asm!("mrs {}, ctr_el0", out(reg) ctr, options(nomem, nostack)) };
    let (dline, iline) = (4usize << ((ctr >> 16) & 0xf), 4usize << (ctr & 0xf));
    let end = base + PAGES * 0x1000;
    let mut at = base;
    while at < end {
        // SAFETY: cleans a line of this program's code pages.
        unsafe { core::arch::asm!("dc cvau, {}", in(reg) at, options(nostack)) };
        at += dline;
    }
    // SAFETY: a barrier.
    unsafe { core::arch::asm!("dsb ish", options(nostack)) };
    let mut at = base;
    while at < end {
        // SAFETY: invalidates a line of this program's code pages.
        unsafe { core::arch::asm!("ic ivau, {}", in(reg) at, options(nostack)) };
        at += iline;
    }
    // SAFETY: barriers.
    unsafe { core::arch::asm!("dsb ish", "isb", options(nostack)) };
}

/// ISB: this processor fetches the instructions again (what a thread
/// that runs modified code without entering the kernel needs).
#[cfg(target_arch = "aarch64")]
fn serialize() {
    // SAFETY: a barrier only.
    unsafe { core::arch::asm!("isb", options(nostack)) };
}

fn call(base: usize, i: usize) -> u32 {
    let f = base + i * STRIDE;
    // SAFETY: function `i` returns its immediate.
    let f: extern "C" fn() -> u32 = unsafe { core::mem::transmute(f) };
    f()
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
    publish(base);
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
        publish(base);
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
        publish(base);
        GO.store(round as usize, Ordering::Release);
        while DONE.load(Ordering::Acquire) < round as usize {
            core::hint::spin_loop();
        }
        let bad = BAD.load(Ordering::Relaxed);
        check(9, bad == 0, bad as u64)?;
    }
    // Phase 3.
    for round in (2 * ROUNDS + 1..).take(LIVE_ROUNDS as usize) {
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
        publish(base);
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
