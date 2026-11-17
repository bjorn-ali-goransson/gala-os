//! A garbage collector's safepoint, as a JVM (HotSpot) does it on Windows,
//! under threads on two processors, as on real Windows: "mutator" threads
//! hold values ("references") in their registers (general-purpose and
//! XMM) and in stack slots while they run a loop with a safepoint poll,
//! `test [r11], eax` with r11 loaded from a poll pointer. A "collector"
//! thread arms the poll (points it at a PAGE_NOACCESS page): each mutator
//! faults there, and a vectored handler (through ntdll's exception
//! dispatch, as HotSpot's) publishes its CONTEXT and blocks on a
//! semaphore. Once all have arrived the collector checks every value in
//! the CONTEXTs, on the stopped stacks and in each mutator's "heap" words
//! (all must hold the previous epoch), rewrites them all with the next
//! epoch (as a GC moving objects updates references), churns memory
//! (commit, protect, decommit: TLB shootdowns), disarms the poll and
//! releases the semaphore. The handler resumes the poll's next
//! instruction (NtContinue), and the mutator must then see the new epoch
//! everywhere: a value of an older epoch is what a JVM sees as a stale
//! reference (a pre-GC address). Every other collection stops the
//! mutators as HotSpot does instead: the handler only redirects the
//! thread (NtContinue) to a stub that saves every register on the stack
//! and blocks there; the collector rewrites that save area. The
//! collector also patches constants embedded in "compiled code" (an
//! RWX page), as HotSpot patches object addresses in compiled methods:
//! one each mutator calls, and one inside the very loop (with its poll)
//! two "looper" threads are stopped in. A "noise" thread meanwhile commits,
//! touches, protects and frees memory (page faults and shootdowns while
//! mutators run), and timer preemption moves the threads between
//! processors.
//!
//! Written for the Java heap corruption (WORKFLOW "Known intermittent").
//! Prints `safepoint_test: ok <rounds>` or exits with the number of the
//! failing check (details on stdout).

#![no_std]
#![no_main]

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};

use ntrt::*;

#[cfg(target_arch = "aarch64")]
#[path = "arm64.rs"]
mod arch;
#[cfg(target_arch = "x86_64")]
#[path = "x64.rs"]
mod arch;
use arch::{sp_mutate, sp_release_semaphore, sp_stub};

const STATUS_ACCESS_VIOLATION: u32 = 0xc000_0005;
const EXCEPTION_CONTINUE_EXECUTION: i32 = -1;
const EXCEPTION_CONTINUE_SEARCH: i32 = 0;
const PAGE_EXECUTE_READWRITE: u32 = 0x40;
const MUTATORS: usize = 3;
/// "Loopers": threads running compiled code (LOOP_CODE) whose embedded
/// constant the collector patches while they are stopped at the poll in
/// that same code, as HotSpot patches object addresses in a compiled
/// method a thread is stopped in (a full collection moves every object).
const LOOPERS: usize = 2;
const THREADS: usize = MUTATORS + LOOPERS;
/// Collections to run (fewer if the time limit comes first).
const ROUNDS: u32 = 600;
const TIME_LIMIT_MS: i64 = 30_000;
/// Values a mutator holds: 14 general-purpose registers (x64: all but rsp
/// and r11, the poll's scratch; ARM64: x0-x6, x19-x25), 8 stack slots, 4
/// vector registers (xmm8-11; d8-d11).
pub const GPRS: usize = 14;
const SLOTS: usize = 8;
const XMMS: usize = 4;
pub const VALUES: usize = GPRS + SLOTS + XMMS;
/// Heap words per mutator, one per page.
const HEAP_PAGES: usize = 16;
const PAGE: usize = 0x1000;
/// Poll loop iterations per call of `sp_mutate`.
const POLLS: u64 = 2000;

/// How the collector stops a mutator: in the handler (its CONTEXT), or
/// as HotSpot does, in sp_stub after NtContinue (its save area).
static STUB_MODE: AtomicBool = AtomicBool::new(false);
/// Per mutator: whether it is stopped in sp_stub (else in the handler).
static IN_STUB: [AtomicBool; THREADS] = [const { AtomicBool::new(false) }; THREADS];
/// Per mutator: where sp_stub returns to (after the poll that faulted).
static RESUME: [AtomicU64; THREADS] = [const { AtomicU64::new(0) }; THREADS];

/// Called by sp_stub with its save area: the mutator stops there.
extern "C" fn sp_block(area: usize) {
    let Some(m) = (0..THREADS).find(|&m| {
        let top = STACK_TOPS[m].load(Ordering::SeqCst);
        top == teb_field(0x48)
    }) else {
        fail(0x22, 0, 0, area as u64, 0);
        return;
    };
    // SAFETY: the stub's slot for its return address.
    unsafe { ((area + arch::STUB_RETURN) as *mut u64).write(RESUME[m].load(Ordering::SeqCst)) };
    IN_STUB[m].store(true, Ordering::SeqCst);
    CONTEXTS[m].store(area, Ordering::SeqCst);
    ARRIVED.fetch_add(1, Ordering::SeqCst);
    let s = wait(Handle(SEMAPHORE.load(Ordering::SeqCst)), None);
    if s != STATUS_SUCCESS {
        fail(0x21, m, 0, s.into(), 0);
    }
}

/// The address the poll reads: the good page, or the bad one when armed.
#[unsafe(no_mangle)]
static SP_POLL: AtomicUsize = AtomicUsize::new(0);
static GOOD: AtomicUsize = AtomicUsize::new(0);
static BAD: AtomicUsize = AtomicUsize::new(0);
static SEMAPHORE: AtomicUsize = AtomicUsize::new(0);
/// Mutators stopped at the safepoint, and each one's CONTEXT there.
static ARRIVED: AtomicUsize = AtomicUsize::new(0);
static CONTEXTS: [AtomicUsize; THREADS] = [const { AtomicUsize::new(0) }; THREADS];
/// Each thread's id (TEB ClientId.UniqueThread), to tell them apart.
static STACK_TOPS: [AtomicUsize; THREADS] = [const { AtomicUsize::new(0) }; THREADS];
/// The epoch each mutator's values must have (set by the collector while
/// the mutator is stopped).
static EPOCH: [AtomicU64; MUTATORS] = [const { AtomicU64::new(1) }; MUTATORS];
/// Each mutator's heap: a word on each of HEAP_PAGES pages.
static HEAPS: [AtomicUsize; MUTATORS] = [const { AtomicUsize::new(0) }; MUTATORS];
/// "Compiled code" (as a JIT's, with an embedded constant the collector
/// patches, as HotSpot patches object addresses in compiled methods):
/// per mutator, `arch::write_code`'s at CODE + 32 * m.
static CODE: AtomicUsize = AtomicUsize::new(0);
/// Value index of the embedded constant.
const CODE_VALUE: usize = 200;

/// The constant's generation (bumped by each collection).
static LOOP_EPOCH: AtomicU64 = AtomicU64::new(1);

fn loop_value(generation: u64) -> u32 {
    0x10_0000 | (generation as u32 & 0xf_ffff)
}

/// LOOP_CODE's signature (out, iterations, unused, poll: the first and
/// fourth argument registers of Windows x64 and ARM64).
type LoopFn = extern "C" fn(*mut u32, u64, u64, *const AtomicUsize);

/// A looper: runs LOOP_CODE (polling meanwhile) and checks the constant.
extern "C" fn looper(t: usize) -> ! {
    STACK_TOPS[t].store(teb_field(0x48), Ordering::SeqCst);
    let at = CODE.load(Ordering::SeqCst) + arch::LOOP_CODE_AT;
    // SAFETY: LOOP_CODE uses only volatile registers.
    let f = unsafe { core::mem::transmute::<usize, LoopFn>(at) };
    while !STOP.load(Ordering::SeqCst) {
        let mut out = 0u32;
        f(&mut out, 500, 0, &SP_POLL);
        // The collector only patches while every looper is stopped, so
        // the generation cannot move between the last store and this.
        let want = loop_value(LOOP_EPOCH.load(Ordering::SeqCst));
        if out != want {
            fail(0x33, t, 0, out.into(), want.into());
        }
    }
    RUNNING.fetch_sub(1, Ordering::SeqCst);
    exit_thread(STATUS_SUCCESS)
}

/// Mutator `m`'s compiled function.
fn code_of(m: usize) -> extern "C" fn() -> u64 {
    let at = CODE.load(Ordering::SeqCst) + 32 * m;
    // SAFETY: CODE holds `arch::write_code`'s code for every mutator.
    unsafe { core::mem::transmute::<usize, extern "C" fn() -> u64>(at) }
}
static STOP: AtomicBool = AtomicBool::new(false);
static RUNNING: AtomicUsize = AtomicUsize::new(0);
/// The first failure: check, mutator, value index, value found.
static FAILED: AtomicU32 = AtomicU32::new(0);
static FAIL_INFO: [AtomicU64; 4] = [const { AtomicU64::new(0) }; 4];
static NOISE_ROUNDS: AtomicU64 = AtomicU64::new(0);
static SAFEPOINTS: AtomicU64 = AtomicU64::new(0);

/// Value `idx` of mutator `m` in `epoch`.
fn value(epoch: u64, m: usize, idx: usize) -> u64 {
    0x5afe_0000_0000_0000 | epoch << 20 | (m as u64) << 8 | idx as u64
}

fn fail(check: u32, m: usize, idx: usize, got: u64, want: u64) {
    if FAILED.compare_exchange(0, check, Ordering::SeqCst, Ordering::SeqCst).is_ok() {
        FAIL_INFO[0].store(m as u64, Ordering::SeqCst);
        FAIL_INFO[1].store(idx as u64, Ordering::SeqCst);
        FAIL_INFO[2].store(got, Ordering::SeqCst);
        FAIL_INFO[3].store(want, Ordering::SeqCst);
    }
    STOP.store(true, Ordering::SeqCst);
}

#[repr(C)]
struct ExceptionPointers {
    record: *const u8,
    context: *mut u8,
}

/// The safepoint handler: a fault at the bad page stops the mutator here
/// until the collector releases it, then resumes after the poll.
extern "system" fn handler(pointers: *mut ExceptionPointers) -> i32 {
    // SAFETY: the dispatcher passes valid EXCEPTION_POINTERS; the offsets
    // are those of the public 64-bit EXCEPTION_RECORD and the machine's
    // CONTEXT.
    unsafe {
        let (record, context) = ((*pointers).record, (*pointers).context);
        if (record as *const u32).read_unaligned() != STATUS_ACCESS_VIOLATION
            || (record.add(0x28) as *const u64).read_unaligned() as usize
                != BAD.load(Ordering::SeqCst)
        {
            return EXCEPTION_CONTINUE_SEARCH;
        }
        let Some(m) = (0..THREADS).find(|&m| {
            let top = STACK_TOPS[m].load(Ordering::SeqCst);
            top == teb_field(0x48)
        }) else {
            return EXCEPTION_CONTINUE_SEARCH;
        };
        let rip = context.add(arch::CTX_PC) as *mut u64;
        let at = rip.read_unaligned();
        let poll = arch::POLL.len() as u64;
        if core::slice::from_raw_parts(at as *const u8, arch::POLL.len()) != arch::POLL {
            fail(0x20, m, 0, at, 0);
        }
        if STUB_MODE.load(Ordering::SeqCst) {
            // As HotSpot: resume in the stub, which returns after the poll.
            RESUME[m].store(at + poll, Ordering::SeqCst);
            rip.write_unaligned(sp_stub as *const () as u64);
            return EXCEPTION_CONTINUE_EXECUTION;
        }
        IN_STUB[m].store(false, Ordering::SeqCst);
        CONTEXTS[m].store(context as usize, Ordering::SeqCst);
        ARRIVED.fetch_add(1, Ordering::SeqCst);
        let s = wait(Handle(SEMAPHORE.load(Ordering::SeqCst)), None);
        if s != STATUS_SUCCESS {
            fail(0x21, m, 0, s.into(), 0);
        }
        rip.write_unaligned(at + poll);
    }
    EXCEPTION_CONTINUE_EXECUTION
}

extern "C" fn mutator(m: usize) -> ! {
    STACK_TOPS[m].store(teb_field(0x48), Ordering::SeqCst);
    let heap = HEAPS[m].load(Ordering::SeqCst);
    while !STOP.load(Ordering::SeqCst) {
        let epoch = EPOCH[m].load(Ordering::SeqCst);
        // The patched constant is the current one (run the code often, so
        // that an emulator keeps it translated).
        for _ in 0..16 {
            let got = code_of(m)();
            if got != value(epoch, m, CODE_VALUE) {
                fail(0x32, m, CODE_VALUE, got, value(epoch, m, CODE_VALUE));
            }
        }
        let mut values = [0u64; VALUES];
        for (i, v) in values.iter_mut().enumerate() {
            *v = value(epoch, m, i);
        }
        for p in 0..HEAP_PAGES {
            // SAFETY: the mutator's committed heap; the collector touches
            // it only while the mutator is stopped.
            unsafe { ((heap + p * PAGE) as *mut u64).write_volatile(value(epoch, m, 100 + p)) };
        }
        let mut out = [0u64; VALUES];
        // SAFETY: valid pointers; sp_mutate keeps the Windows ABI.
        unsafe { sp_mutate(&values, &mut out, POLLS) };
        // Stopped at safepoints meanwhile: everything has the epoch the
        // collector left.
        let now = EPOCH[m].load(Ordering::SeqCst);
        for (i, &v) in out.iter().enumerate() {
            if v != value(now, m, i) {
                fail(0x30, m, i, v, value(now, m, i));
            }
        }
        for p in 0..HEAP_PAGES {
            // SAFETY: as above.
            let v = unsafe { ((heap + p * PAGE) as *const u64).read_volatile() };
            if v != value(now, m, 100 + p) {
                fail(0x31, m, 100 + p, v, value(now, m, 100 + p));
            }
        }
    }
    RUNNING.fetch_sub(1, Ordering::SeqCst);
    exit_thread(STATUS_SUCCESS)
}

/// Commits, touches, protects and frees memory while the others run.
extern "C" fn noise(_: usize) -> ! {
    let mut n = 0u64;
    while !STOP.load(Ordering::SeqCst) {
        let (s, base, size) = allocate(0, 16 * PAGE, MEM_RESERVE | MEM_COMMIT, PAGE_READWRITE);
        if s != STATUS_SUCCESS {
            fail(0x40, 0, 0, s.into(), 0);
            break;
        }
        for p in (0..size).step_by(PAGE) {
            // SAFETY: committed read-write above.
            unsafe { ((base + p) as *mut u64).write_volatile(n) };
        }
        protect(base, 8 * PAGE, PAGE_READONLY);
        free(base + 8 * PAGE, 4 * PAGE, MEM_DECOMMIT);
        free(base, 0, MEM_RELEASE);
        n += 1;
        if n.is_multiple_of(8) {
            yield_execution();
        }
    }
    NOISE_ROUNDS.store(n, Ordering::SeqCst);
    RUNNING.fetch_sub(1, Ordering::SeqCst);
    exit_thread(STATUS_SUCCESS)
}

/// One collection: stop every mutator at its poll, check and rewrite its
/// values, churn memory, let them go.
fn collect(scratch: usize, stub: bool) -> Result<(), u32> {
    STUB_MODE.store(stub, Ordering::SeqCst);
    SP_POLL.store(BAD.load(Ordering::SeqCst), Ordering::SeqCst);
    let start = now_ms();
    while ARRIVED.load(Ordering::SeqCst) < THREADS {
        if STOP.load(Ordering::SeqCst) {
            return Err(FAILED.load(Ordering::SeqCst));
        }
        if now_ms() - start > 10_000 {
            fail(0x10, ARRIVED.load(Ordering::SeqCst), 0, 0, 0);
            return Err(0x10);
        }
        yield_execution();
    }
    for m in 0..MUTATORS {
        let old = EPOCH[m].load(Ordering::SeqCst);
        let new = old + 1;
        let ctx = CONTEXTS[m].load(Ordering::SeqCst) as *mut u8;
        let check = |at: *mut u64, idx: usize, check: u32| {
            // SAFETY: the stopped mutator's CONTEXT, stack and heap, which
            // it does not touch until released.
            let v = unsafe { at.read_unaligned() };
            if v != value(old, m, idx) {
                fail(check, m, idx, v, value(old, m, idx));
            }
            // SAFETY: as above.
            unsafe { at.write_unaligned(value(new, m, idx)) };
        };
        // SAFETY (pointer arithmetic below): inside the CONTEXT, the
        // mutator's stack slots above its Rsp and its heap pages.
        unsafe {
            let stub = IN_STUB[m].load(Ordering::SeqCst);
            let (gprs, xmm8, rsp) = if stub {
                (&arch::STUB_GPR, arch::STUB_VEC, ctx as usize + arch::STUB_SP)
            } else {
                let rsp = (ctx.add(arch::CTX_SP) as *const u64).read_unaligned() as usize;
                (&arch::CTX_GPR, arch::CTX_VEC, rsp)
            };
            for (i, &off) in gprs.iter().enumerate() {
                check(ctx.add(off) as *mut u64, i, 0x11);
            }
            for i in 0..SLOTS {
                check((rsp + 8 * i) as *mut u64, GPRS + i, 0x12);
            }
            for i in 0..XMMS {
                check(ctx.add(xmm8 + 16 * i) as *mut u64, GPRS + SLOTS + i, 0x13);
            }
            let heap = HEAPS[m].load(Ordering::SeqCst);
            for p in 0..HEAP_PAGES {
                check((heap + p * PAGE) as *mut u64, 100 + p, 0x14);
            }
            // The compiled constant (an object address in compiled code).
            let code = CODE.load(Ordering::SeqCst) + 32 * m;
            let v = arch::read_code(code);
            if v != value(old, m, CODE_VALUE) {
                fail(0x15, m, CODE_VALUE, v, value(old, m, CODE_VALUE));
            }
            arch::write_code(code, value(new, m, CODE_VALUE));
        }
        EPOCH[m].store(new, Ordering::SeqCst);
    }
    // The loopers' compiled constant moves on (a 4-byte immediate, as a
    // compressed object address).
    let constant = LOOP_EPOCH.load(Ordering::SeqCst) + 1;
    let at = CODE.load(Ordering::SeqCst) + arch::LOOP_CODE_AT;
    // SAFETY: LOOP_CODE in the code page; no looper runs (all are stopped
    // at the poll).
    unsafe {
        let old = arch::read_loop(at);
        if old != loop_value(constant - 1) {
            fail(0x16, MUTATORS, 0, old.into(), loop_value(constant - 1).into());
        }
        arch::write_loop(at, loop_value(constant));
    }
    // The patched code, made visible to instruction fetch (as a JIT's
    // FlushInstructionCache; nothing to do on x64).
    arch::sync_code(CODE.load(Ordering::SeqCst), PAGE);
    LOOP_EPOCH.store(constant, Ordering::SeqCst);
    // Memory changes while the mutators are stopped (a heap resize).
    allocate(scratch, 8 * PAGE, MEM_COMMIT, PAGE_READWRITE);
    for p in 0..8 {
        // SAFETY: committed above.
        unsafe { ((scratch + p * PAGE) as *mut u64).write_volatile(p as u64) };
    }
    protect(scratch, 4 * PAGE, PAGE_READONLY);
    free(scratch, 8 * PAGE, MEM_DECOMMIT);
    SAFEPOINTS.fetch_add(1, Ordering::SeqCst);
    SP_POLL.store(GOOD.load(Ordering::SeqCst), Ordering::SeqCst);
    ARRIVED.store(0, Ordering::SeqCst);
    let mut previous = 0;
    // SAFETY: a valid output pointer.
    let s = unsafe {
        sp_release_semaphore(
            Handle(SEMAPHORE.load(Ordering::SeqCst)),
            THREADS as i32,
            &mut previous,
        )
    };
    if s != STATUS_SUCCESS {
        return Err(0x5);
    }
    Ok(())
}

type AddHandler =
    extern "system" fn(u32, extern "system" fn(*mut ExceptionPointers) -> i32) -> usize;

fn ok(id: u32, cond: bool) -> Result<(), u32> {
    if cond { Ok(()) } else { Err(id) }
}

fn run() -> Result<u32, u32> {
    let (s, good, _) = allocate(0, PAGE, MEM_RESERVE | MEM_COMMIT, PAGE_READONLY);
    ok(1, s == STATUS_SUCCESS)?;
    let (s, bad, _) = allocate(0, PAGE, MEM_RESERVE | MEM_COMMIT, PAGE_NOACCESS);
    ok(1, s == STATUS_SUCCESS)?;
    GOOD.store(good, Ordering::SeqCst);
    BAD.store(bad, Ordering::SeqCst);
    SP_POLL.store(good, Ordering::SeqCst);
    let (s, scratch, _) = allocate(0, 8 * PAGE, MEM_RESERVE, PAGE_READWRITE);
    ok(1, s == STATUS_SUCCESS)?;
    let (s, code, _) = allocate(0, PAGE, MEM_RESERVE | MEM_COMMIT, PAGE_EXECUTE_READWRITE);
    ok(2, s == STATUS_SUCCESS)?;
    for m in 0..MUTATORS {
        // SAFETY: the freshly committed code page.
        unsafe { arch::write_code(code + 32 * m, value(1, m, CODE_VALUE)) };
    }
    // SAFETY: the code page; LOOP_CODE fits after the mutators' code.
    unsafe {
        let at = (code + arch::LOOP_CODE_AT) as *mut u8;
        at.copy_from_nonoverlapping(arch::LOOP_CODE.as_ptr(), arch::LOOP_CODE.len());
        arch::write_loop(code + arch::LOOP_CODE_AT, loop_value(1));
    }
    arch::sync_code(code, PAGE);
    CODE.store(code, Ordering::SeqCst);
    for heap in &HEAPS {
        let (s, base, _) = allocate(0, HEAP_PAGES * PAGE, MEM_RESERVE | MEM_COMMIT, PAGE_READWRITE);
        ok(2, s == STATUS_SUCCESS)?;
        heap.store(base, Ordering::SeqCst);
    }
    let (s, h) = create_semaphore(0, 1000);
    ok(3, s == STATUS_SUCCESS)?;
    SEMAPHORE.store(h.0, Ordering::SeqCst);
    let add =
        find_ntdll().and_then(|n| find_export(n, b"RtlAddVectoredExceptionHandler")).ok_or(4u32)?;
    // SAFETY: ntdll's RtlAddVectoredExceptionHandler.
    let add = unsafe { core::mem::transmute::<usize, AddHandler>(add) };
    ok(4, add(1, handler) != 0)?;

    let mut threads = [Handle(0); THREADS + 1];
    RUNNING.store(THREADS + 1, Ordering::SeqCst);
    for l in 0..LOOPERS {
        let (s, th, _) = create_thread(looper, MUTATORS + l, false);
        ok(6, s == STATUS_SUCCESS)?;
        threads[MUTATORS + 1 + l] = th;
    }
    for (m, t) in threads.iter_mut().enumerate().take(MUTATORS) {
        let (s, th, _) = create_thread(mutator, m, false);
        ok(6, s == STATUS_SUCCESS)?;
        *t = th;
    }
    let (s, th, _) = create_thread(noise, 0, false);
    ok(6, s == STATUS_SUCCESS)?;
    threads[MUTATORS] = th;
    // Every mutator has published its stack before the first safepoint.
    while STACK_TOPS.iter().any(|t| t.load(Ordering::SeqCst) == 0) {
        yield_execution();
    }
    let start = now_ms();
    let mut rounds = 0;
    let mut result = Ok(());
    while rounds < ROUNDS && now_ms() - start < TIME_LIMIT_MS && !STOP.load(Ordering::SeqCst) {
        // A little time between collections (0 to 2 ms).
        if rounds % 3 != 0 {
            delay(-((rounds as i64 % 3) * 10_000));
        }
        if let Err(e) = collect(scratch, rounds % 2 == 1) {
            result = Err(e);
            break;
        }
        rounds += 1;
    }
    STOP.store(true, Ordering::SeqCst);
    for &t in &threads {
        if wait(t, Some(-300_000_000)) != STATUS_SUCCESS {
            terminate_thread(t, 1);
        }
        close(t);
    }
    let failed = FAILED.load(Ordering::SeqCst);
    if failed != 0 {
        print("safepoint_test: check ");
        write_file(stdout(), &hex32(failed));
        for (name, v) in ["mutator", "value", "got", "want"].iter().zip(&FAIL_INFO) {
            print(" ");
            print(name);
            print(" ");
            let v = v.load(Ordering::SeqCst);
            write_file(stdout(), &hex32((v >> 32) as u32));
            write_file(stdout(), &hex32(v as u32));
        }
        print("\r\n");
        return Err(failed);
    }
    result?;
    ok(7, rounds > 0)?;
    Ok(rounds)
}

fn main() -> u32 {
    match run() {
        Ok(rounds) => {
            print("safepoint_test: ok ");
            write_file(stdout(), &hex32(rounds));
            print(" safepoints, noise ");
            write_file(stdout(), &hex32(NOISE_ROUNDS.load(Ordering::SeqCst) as u32));
            print("\r\n");
            0
        }
        Err(id) => id,
    }
}

entry!(main);
