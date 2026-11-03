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
//! reference (a pre-GC address). A "noise" thread meanwhile commits,
//! touches, protects and frees memory (page faults and shootdowns while
//! mutators run), and timer preemption moves the threads between
//! processors.
//!
//! Written for the Java heap corruption (ROADMAP Priorities, 2026-11-03).
//! Prints `safepoint_test: ok <rounds>` or exits with the number of the
//! failing check (details on stdout).

#![no_std]
#![no_main]

use core::arch::global_asm;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};

use ntrt::*;

const STATUS_ACCESS_VIOLATION: u32 = 0xc000_0005;
const EXCEPTION_CONTINUE_EXECUTION: i32 = -1;
const EXCEPTION_CONTINUE_SEARCH: i32 = 0;
const SEMAPHORE_ALL_ACCESS: u32 = 0x1f_0003;
const MUTATORS: usize = 3;
/// Collections to run (fewer if the time limit comes first).
const ROUNDS: u32 = 600;
const TIME_LIMIT_MS: i64 = 30_000;
/// Values a mutator holds: 14 general-purpose registers (all but rsp and
/// r11, the poll's scratch), 8 stack slots, 4 XMM registers (xmm8-11).
const GPRS: usize = 14;
const SLOTS: usize = 8;
const XMMS: usize = 4;
const VALUES: usize = GPRS + SLOTS + XMMS;
/// Heap words per mutator, one per page.
const HEAP_PAGES: usize = 16;
const PAGE: usize = 0x1000;
/// Poll loop iterations per call of `sp_mutate`.
const POLLS: u64 = 2000;

/// CONTEXT offsets (x64) of the registers `sp_mutate` loads, in its
/// order: rax, rbx, rcx, rdx, rsi, rdi, rbp, r8, r9, r10, r12, r13, r14,
/// r15; then Rsp, Rip and Xmm8.
const CTX_GPR: [usize; GPRS] =
    [0x78, 0x90, 0x80, 0x88, 0xa8, 0xb0, 0xa0, 0xb8, 0xc0, 0xc8, 0xd8, 0xe0, 0xe8, 0xf0];
const CTX_RSP: usize = 0x98;
const CTX_RIP: usize = 0xf8;
const CTX_XMM8: usize = 0x1a0 + 8 * 16;

// sp_mutate(values: *const [u64; VALUES], out: *mut [u64; VALUES],
//           polls: u64): loads the values into registers and stack slots,
// runs `polls` safepoint polls (with a little spinning between them), and
// stores what the registers and slots hold then.
global_asm!(
    ".globl sp_mutate",
    "sp_mutate:",
    "push rbx",
    "push rbp",
    "push rsi",
    "push rdi",
    "push r12",
    "push r13",
    "push r14",
    "push r15",
    "sub rsp, 72",
    "movdqu [rsp], xmm8",
    "movdqu [rsp + 16], xmm9",
    "movdqu [rsp + 32], xmm10",
    "movdqu [rsp + 48], xmm11",
    "mov [rsp + 64], rdx",
    "push r8",
    "sub rsp, 64",
    // Stack slots: values[14..22] at [rsp + 8 * i].
    "mov r11, rcx",
    "mov rax, [r11 + 112]",
    "mov [rsp], rax",
    "mov rax, [r11 + 120]",
    "mov [rsp + 8], rax",
    "mov rax, [r11 + 128]",
    "mov [rsp + 16], rax",
    "mov rax, [r11 + 136]",
    "mov [rsp + 24], rax",
    "mov rax, [r11 + 144]",
    "mov [rsp + 32], rax",
    "mov rax, [r11 + 152]",
    "mov [rsp + 40], rax",
    "mov rax, [r11 + 160]",
    "mov [rsp + 48], rax",
    "mov rax, [r11 + 168]",
    "mov [rsp + 56], rax",
    "movq xmm8, [r11 + 176]",
    "movq xmm9, [r11 + 184]",
    "movq xmm10, [r11 + 192]",
    "movq xmm11, [r11 + 200]",
    "mov rax, [r11]",
    "mov rbx, [r11 + 8]",
    "mov rcx, [r11 + 16]",
    "mov rdx, [r11 + 24]",
    "mov rsi, [r11 + 32]",
    "mov rdi, [r11 + 40]",
    "mov rbp, [r11 + 48]",
    "mov r8, [r11 + 56]",
    "mov r9, [r11 + 64]",
    "mov r10, [r11 + 72]",
    "mov r12, [r11 + 80]",
    "mov r13, [r11 + 88]",
    "mov r14, [r11 + 96]",
    "mov r15, [r11 + 104]",
    "2:",
    "mov r11, [rip + SP_POLL]",
    // The poll (41 85 03): faults while the poll points at the bad page.
    "test dword ptr [r11], eax",
    "mov r11d, 300",
    "3:",
    "dec r11d",
    "jnz 3b",
    "dec qword ptr [rsp + 64]",
    "jnz 2b",
    "mov r11, [rsp + 136]",
    "mov [r11], rax",
    "mov [r11 + 8], rbx",
    "mov [r11 + 16], rcx",
    "mov [r11 + 24], rdx",
    "mov [r11 + 32], rsi",
    "mov [r11 + 40], rdi",
    "mov [r11 + 48], rbp",
    "mov [r11 + 56], r8",
    "mov [r11 + 64], r9",
    "mov [r11 + 72], r10",
    "mov [r11 + 80], r12",
    "mov [r11 + 88], r13",
    "mov [r11 + 96], r14",
    "mov [r11 + 104], r15",
    "mov rax, [rsp]",
    "mov [r11 + 112], rax",
    "mov rax, [rsp + 8]",
    "mov [r11 + 120], rax",
    "mov rax, [rsp + 16]",
    "mov [r11 + 128], rax",
    "mov rax, [rsp + 24]",
    "mov [r11 + 136], rax",
    "mov rax, [rsp + 32]",
    "mov [r11 + 144], rax",
    "mov rax, [rsp + 40]",
    "mov [r11 + 152], rax",
    "mov rax, [rsp + 48]",
    "mov [r11 + 160], rax",
    "mov rax, [rsp + 56]",
    "mov [r11 + 168], rax",
    "movq [r11 + 176], xmm8",
    "movq [r11 + 184], xmm9",
    "movq [r11 + 192], xmm10",
    "movq [r11 + 200], xmm11",
    "add rsp, 72",
    "movdqu xmm8, [rsp]",
    "movdqu xmm9, [rsp + 16]",
    "movdqu xmm10, [rsp + 32]",
    "movdqu xmm11, [rsp + 48]",
    "add rsp, 72",
    "pop r15",
    "pop r14",
    "pop r13",
    "pop r12",
    "pop rdi",
    "pop rsi",
    "pop rbp",
    "pop rbx",
    "ret",
    // The semaphore services of the pinned build (as smpsem_test).
    ".globl sp_create_semaphore",
    "sp_create_semaphore:",
    "mov r10, rcx",
    "mov eax, 0xcf",
    "syscall",
    "ret",
    ".globl sp_release_semaphore",
    "sp_release_semaphore:",
    "mov r10, rcx",
    "mov eax, 0x0a",
    "syscall",
    "ret",
);

unsafe extern "C" {
    fn sp_mutate(values: *const [u64; VALUES], out: *mut [u64; VALUES], polls: u64);
    fn sp_create_semaphore(
        handle: *mut Handle,
        access: u32,
        attributes: *const ObjectAttributes,
        initial: i32,
        maximum: i32,
    ) -> NtStatus;
    fn sp_release_semaphore(handle: Handle, count: i32, previous: *mut i32) -> NtStatus;
}

/// The address the poll reads: the good page, or the bad one when armed.
#[unsafe(no_mangle)]
static SP_POLL: AtomicUsize = AtomicUsize::new(0);
static GOOD: AtomicUsize = AtomicUsize::new(0);
static BAD: AtomicUsize = AtomicUsize::new(0);
static SEMAPHORE: AtomicUsize = AtomicUsize::new(0);
/// Mutators stopped at the safepoint, and each one's CONTEXT there.
static ARRIVED: AtomicUsize = AtomicUsize::new(0);
static CONTEXTS: [AtomicUsize; MUTATORS] = [const { AtomicUsize::new(0) }; MUTATORS];
/// Each mutator's stack (an address near its top), to tell them apart.
static STACK_TOPS: [AtomicUsize; MUTATORS] = [const { AtomicUsize::new(0) }; MUTATORS];
/// The epoch each mutator's values must have (set by the collector while
/// the mutator is stopped).
static EPOCH: [AtomicU64; MUTATORS] = [const { AtomicU64::new(1) }; MUTATORS];
/// Each mutator's heap: a word on each of HEAP_PAGES pages.
static HEAPS: [AtomicUsize; MUTATORS] = [const { AtomicUsize::new(0) }; MUTATORS];
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
    // are those of the public x64 EXCEPTION_RECORD and CONTEXT.
    unsafe {
        let (record, context) = ((*pointers).record, (*pointers).context);
        if (record as *const u32).read_unaligned() != STATUS_ACCESS_VIOLATION
            || (record.add(0x28) as *const u64).read_unaligned() as usize
                != BAD.load(Ordering::SeqCst)
        {
            return EXCEPTION_CONTINUE_SEARCH;
        }
        let rsp = (context.add(CTX_RSP) as *const u64).read_unaligned() as usize;
        let Some(m) = (0..MUTATORS).find(|&m| {
            let top = STACK_TOPS[m].load(Ordering::SeqCst);
            rsp < top && rsp + 0x10000 > top
        }) else {
            return EXCEPTION_CONTINUE_SEARCH;
        };
        let rip = context.add(CTX_RIP) as *mut u64;
        let at = rip.read_unaligned();
        if core::slice::from_raw_parts(at as *const u8, 3) != [0x41, 0x85, 0x03] {
            fail(0x20, m, 0, at, 0);
        }
        CONTEXTS[m].store(context as usize, Ordering::SeqCst);
        ARRIVED.fetch_add(1, Ordering::SeqCst);
        let s = wait(Handle(SEMAPHORE.load(Ordering::SeqCst)), None);
        if s != STATUS_SUCCESS {
            fail(0x21, m, 0, s.into(), 0);
        }
        rip.write_unaligned(at + 3);
    }
    EXCEPTION_CONTINUE_EXECUTION
}

extern "C" fn mutator(m: usize) -> ! {
    let mark = 0u64;
    STACK_TOPS[m].store(&mark as *const u64 as usize, Ordering::SeqCst);
    let heap = HEAPS[m].load(Ordering::SeqCst);
    while !STOP.load(Ordering::SeqCst) {
        let epoch = EPOCH[m].load(Ordering::SeqCst);
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
        // SAFETY: valid pointers; sp_mutate keeps the Windows x64 ABI.
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
        if n % 8 == 0 {
            yield_execution();
        }
    }
    NOISE_ROUNDS.store(n, Ordering::SeqCst);
    RUNNING.fetch_sub(1, Ordering::SeqCst);
    exit_thread(STATUS_SUCCESS)
}

/// One collection: stop every mutator at its poll, check and rewrite its
/// values, churn memory, let them go.
fn collect(scratch: usize) -> Result<(), u32> {
    SP_POLL.store(BAD.load(Ordering::SeqCst), Ordering::SeqCst);
    let start = now_ms();
    while ARRIVED.load(Ordering::SeqCst) < MUTATORS {
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
        let mut check = |at: *mut u64, idx: usize, check: u32| {
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
            for (i, &off) in CTX_GPR.iter().enumerate() {
                check(ctx.add(off) as *mut u64, i, 0x11);
            }
            let rsp = (ctx.add(CTX_RSP) as *const u64).read_unaligned() as usize;
            for i in 0..SLOTS {
                check((rsp + 8 * i) as *mut u64, GPRS + i, 0x12);
            }
            for i in 0..XMMS {
                check(ctx.add(CTX_XMM8 + 16 * i) as *mut u64, GPRS + SLOTS + i, 0x13);
            }
            let heap = HEAPS[m].load(Ordering::SeqCst);
            for p in 0..HEAP_PAGES {
                check((heap + p * PAGE) as *mut u64, 100 + p, 0x14);
            }
        }
        EPOCH[m].store(new, Ordering::SeqCst);
    }
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
            MUTATORS as i32,
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
    for heap in &HEAPS {
        let (s, base, _) = allocate(0, HEAP_PAGES * PAGE, MEM_RESERVE | MEM_COMMIT, PAGE_READWRITE);
        ok(2, s == STATUS_SUCCESS)?;
        heap.store(base, Ordering::SeqCst);
    }
    let mut h = Handle(0);
    // SAFETY: a valid output pointer and no attributes.
    let s =
        unsafe { sp_create_semaphore(&mut h, SEMAPHORE_ALL_ACCESS, core::ptr::null(), 0, 1000) };
    ok(3, s == STATUS_SUCCESS)?;
    SEMAPHORE.store(h.0, Ordering::SeqCst);
    let add =
        find_ntdll().and_then(|n| find_export(n, b"RtlAddVectoredExceptionHandler")).ok_or(4u32)?;
    // SAFETY: ntdll's RtlAddVectoredExceptionHandler.
    let add = unsafe { core::mem::transmute::<usize, AddHandler>(add) };
    ok(4, add(1, handler) != 0)?;

    let mut threads = [Handle(0); MUTATORS + 1];
    RUNNING.store(MUTATORS + 1, Ordering::SeqCst);
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
        if let Err(e) = collect(scratch) {
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
