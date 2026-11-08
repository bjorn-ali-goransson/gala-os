//! Extended (AVX) register state, with the results real Windows gives
//! (Windows 11 enables XSAVE and the AVX state on every CPU that has it):
//! - CPUID reports OSXSAVE and AVX, and XGETBV(0) has x87, SSE and AVX
//!   enabled; GetEnabledXStateFeatures includes XSTATE_MASK_AVX and only
//!   features XCR0 enables; IsProcessorFeaturePresent reports
//!   PF_XSAVE_ENABLED and PF_AVX_INSTRUCTIONS_AVAILABLE; FMA (and AVX2)
//!   instructions execute where CPUID reports them.
//! - Whole YMM registers survive preemption, with three threads (on two
//!   or more CPUs) each holding its own pattern in ymm0-ymm15.
//! - Across NtDelayExecution (Sleep, made inline so no other code runs):
//!   the upper halves of every YMM register and the whole of ymm6-ymm15
//!   (the x64 ABI makes xmm0-xmm5 volatile across calls, the system call
//!   included; the upper halves are left alone by the kernel and its
//!   context switches save them).
//! - An APC (QueueUserAPC to a thread in an alertable wait) whose routine
//!   overwrites xmm0-xmm15 (legacy SSE encoding, which leaves the upper
//!   halves alone): ymm6-ymm15 and every upper half are back afterwards.
//! - An exception: the vectored handler's CONTEXT has CONTEXT_XSTATE with
//!   the AVX state (GetXStateFeaturesMask, LocateXStateFeature: 256
//!   bytes, the upper halves of ymm0-15); a change the handler makes there
//!   (SetXStateFeaturesMask) is what the thread resumes with, and the
//!   handler's own use of the YMM registers does not leak into the
//!   resumed code.
//! - GetThreadContext with CONTEXT_XSTATE (InitializeContext2,
//!   SetXStateFeaturesMask, LocateXStateFeature: the documented "Working
//!   with XState Context" sequence) reads a suspended thread's YMM
//!   registers, and SetThreadContext changes them.
//!
//! Prints `xstate_test: ok` or exits with the number of the failing check.

#![no_std]
#![no_main]

use core::arch::asm;
use core::ffi::c_void;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

type Bool = i32;
type Handle = *mut c_void;
type StartRoutine = extern "system" fn(*mut c_void) -> u32;
type VectoredHandler = extern "system" fn(*mut ExceptionPointers) -> i32;

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetLastError() -> u32;
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
    fn SuspendThread(h: Handle) -> u32;
    fn ResumeThread(h: Handle) -> u32;
    fn GetThreadContext(h: Handle, context: *mut u8) -> Bool;
    fn SetThreadContext(h: Handle, context: *const u8) -> Bool;
    fn WaitForSingleObject(h: Handle, ms: u32) -> u32;
    fn CloseHandle(h: Handle) -> Bool;
    fn Sleep(ms: u32);
    fn ExitProcess(code: u32) -> !;
    fn IsProcessorFeaturePresent(feature: u32) -> Bool;
    fn QueueUserAPC(routine: extern "system" fn(usize), thread: Handle, data: usize) -> u32;
    fn AddVectoredExceptionHandler(first: u32, handler: VectoredHandler) -> *mut c_void;
    fn RemoveVectoredExceptionHandler(handle: *mut c_void) -> u32;
    fn GetEnabledXStateFeatures() -> u64;
    fn InitializeContext2(
        buffer: *mut u8,
        flags: u32,
        context: *mut *mut u8,
        length: *mut u32,
        compaction_mask: u64,
    ) -> Bool;
    fn LocateXStateFeature(context: *mut u8, feature: u32, length: *mut u32) -> *mut u8;
    fn SetXStateFeaturesMask(context: *mut u8, mask: u64) -> Bool;
    fn GetXStateFeaturesMask(context: *mut u8, mask: *mut u64) -> Bool;
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

const XSTATE_MASK_LEGACY: u64 = 3;
const XSTATE_MASK_AVX: u64 = 4;
const XSTATE_LEGACY_SSE: u32 = 1;
const XSTATE_AVX: u32 = 2;
/// CONTEXT_FULL (CONTROL | INTEGER | FLOATING_POINT) | CONTEXT_XSTATE.
const CONTEXT_FULL_XSTATE: u32 = 0x10_004b;
const CONTEXT_XSTATE: u32 = 0x10_0040;
const CTX_FLAGS: usize = 0x30;
const CTX_RIP: usize = 0xf8;
const CTX_R13: usize = 0xe0;
const PF_XSAVE_ENABLED: u32 = 17;
const PF_AVX_INSTRUCTIONS_AVAILABLE: u32 = 39;
const ERROR_INSUFFICIENT_BUFFER: u32 = 122;
const STATUS_ILLEGAL_INSTRUCTION: u32 = 0xc000_001d;
const INFINITE: u32 = 0xffff_ffff;

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
        // One line, one write (see WORKFLOW).
        let mut line = [0u8; 88];
        let (at, at_len) = line_text(core::panic::Location::caller().line());
        let mut n = 0;
        for part in [
            &b"xstate_test: check "[..],
            &hex(self.0),
            b" got ",
            &hex(got as u32),
            &at[..at_len],
            b"\r\n",
        ] {
            line[n..n + part.len()].copy_from_slice(part);
            n += part.len();
        }
        out(&line[..n]);
        Err(self.0)
    }

    #[track_caller]
    fn eq(&mut self, got: u64, want: u64) -> Result<(), u32> {
        self.ok(got == want, got)
    }
}

/// ymm0..ymm15, four 64-bit lanes each (lanes 2 and 3: the upper half).
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(C, align(32))]
struct Ymm([[u64; 4]; 16]);

impl Ymm {
    const ZERO: Ymm = Ymm([[0; 4]; 16]);

    /// Thread `t`'s pattern: every lane of every register different.
    fn pattern(t: u64) -> Ymm {
        let mut y = Ymm::ZERO;
        for (r, reg) in y.0.iter_mut().enumerate() {
            for (l, lane) in reg.iter_mut().enumerate() {
                *lane = 0xa5a5_0000_0000_0000 | (t << 32) | ((r as u64) << 8) | l as u64;
            }
        }
        y
    }

    /// Which registers differ from `want` (bit r), looking at the upper
    /// halves of all and the lower halves of those in `whole`.
    fn diff(&self, want: &Ymm, whole: u32) -> u32 {
        let mut bad = 0;
        for r in 0..16 {
            let (got, exp) = (self.0[r], want.0[r]);
            let upper = got[2..] != exp[2..];
            let lower = whole & (1 << r) != 0 && got[..2] != exp[..2];
            if upper || lower {
                bad |= 1 << r;
            }
        }
        bad
    }
}

/// Registers whose whole value survives a system call (ymm6-ymm15).
const NONVOLATILE: u32 = 0xffc0;
const ALL: u32 = 0xffff;

/// Loads ymm0-15 from `input`, then `rounds` times: spins `spins` times
/// (pause), then, if `delay` is not null, makes the system call
/// NtDelayExecution(alertable, delay) inline (no other code between);
/// then stores ymm0-15 to `output`.
fn hold(input: &Ymm, output: &mut Ymm, spins: u64, rounds: u64, delay: *const i64, alertable: u64) {
    // SAFETY: reads `input`, writes `output`; the registers the system call
    // changes (rax, rcx, rdx, r10, r11; Windows may also change r8, r9) and
    // every vector register are declared clobbered. The loop state lives in
    // nonvolatile registers, which the system call keeps.
    unsafe {
        asm!(
            "vmovdqu ymm0, [{i}]",
            "vmovdqu ymm1, [{i} + 32]",
            "vmovdqu ymm2, [{i} + 64]",
            "vmovdqu ymm3, [{i} + 96]",
            "vmovdqu ymm4, [{i} + 128]",
            "vmovdqu ymm5, [{i} + 160]",
            "vmovdqu ymm6, [{i} + 192]",
            "vmovdqu ymm7, [{i} + 224]",
            "vmovdqu ymm8, [{i} + 256]",
            "vmovdqu ymm9, [{i} + 288]",
            "vmovdqu ymm10, [{i} + 320]",
            "vmovdqu ymm11, [{i} + 352]",
            "vmovdqu ymm12, [{i} + 384]",
            "vmovdqu ymm13, [{i} + 416]",
            "vmovdqu ymm14, [{i} + 448]",
            "vmovdqu ymm15, [{i} + 480]",
            "2:",
            "mov r14, rdi",
            "3:",
            "pause",
            "dec r14",
            "jnz 3b",
            "test r13, r13",
            "jz 4f",
            "mov r10, r12",
            "mov rdx, r13",
            "mov eax, 0x34", // NtDelayExecution
            "syscall",
            "4:",
            "dec r15",
            "jnz 2b",
            "vmovdqu [rsi], ymm0",
            "vmovdqu [rsi + 32], ymm1",
            "vmovdqu [rsi + 64], ymm2",
            "vmovdqu [rsi + 96], ymm3",
            "vmovdqu [rsi + 128], ymm4",
            "vmovdqu [rsi + 160], ymm5",
            "vmovdqu [rsi + 192], ymm6",
            "vmovdqu [rsi + 224], ymm7",
            "vmovdqu [rsi + 256], ymm8",
            "vmovdqu [rsi + 288], ymm9",
            "vmovdqu [rsi + 320], ymm10",
            "vmovdqu [rsi + 352], ymm11",
            "vmovdqu [rsi + 384], ymm12",
            "vmovdqu [rsi + 416], ymm13",
            "vmovdqu [rsi + 448], ymm14",
            "vmovdqu [rsi + 480], ymm15",
            "vzeroupper",
            i = in(reg) input,
            in("rsi") output,
            in("rdi") spins.max(1),
            inout("r15") rounds.max(1) => _,
            in("r12") alertable,
            in("r13") delay,
            out("r14") _,
            out("rax") _, out("rcx") _, out("rdx") _, out("r8") _, out("r9") _,
            out("r10") _, out("r11") _,
            out("xmm0") _, out("xmm1") _, out("xmm2") _, out("xmm3") _,
            out("xmm4") _, out("xmm5") _, out("xmm6") _, out("xmm7") _,
            out("xmm8") _, out("xmm9") _, out("xmm10") _, out("xmm11") _,
            out("xmm12") _, out("xmm13") _, out("xmm14") _, out("xmm15") _,
        )
    };
}

/// Loads ymm0-15 from `input`, sets `ready`, spins until `stop` is set,
/// then stores ymm0-15 to `output`.
fn hold_until(input: &Ymm, output: &mut Ymm, ready: &AtomicU32, stop: &AtomicU32) {
    // SAFETY: reads `input` and the flags, writes `output` and `ready`;
    // every vector register is declared clobbered.
    unsafe {
        asm!(
            "vmovdqu ymm0, [{i}]",
            "vmovdqu ymm1, [{i} + 32]",
            "vmovdqu ymm2, [{i} + 64]",
            "vmovdqu ymm3, [{i} + 96]",
            "vmovdqu ymm4, [{i} + 128]",
            "vmovdqu ymm5, [{i} + 160]",
            "vmovdqu ymm6, [{i} + 192]",
            "vmovdqu ymm7, [{i} + 224]",
            "vmovdqu ymm8, [{i} + 256]",
            "vmovdqu ymm9, [{i} + 288]",
            "vmovdqu ymm10, [{i} + 320]",
            "vmovdqu ymm11, [{i} + 352]",
            "vmovdqu ymm12, [{i} + 384]",
            "vmovdqu ymm13, [{i} + 416]",
            "vmovdqu ymm14, [{i} + 448]",
            "vmovdqu ymm15, [{i} + 480]",
            "lock inc dword ptr [{ready}]",
            "2:",
            "pause",
            "cmp dword ptr [{stop}], 0",
            "je 2b",
            "vmovdqu [{o}], ymm0",
            "vmovdqu [{o} + 32], ymm1",
            "vmovdqu [{o} + 64], ymm2",
            "vmovdqu [{o} + 96], ymm3",
            "vmovdqu [{o} + 128], ymm4",
            "vmovdqu [{o} + 160], ymm5",
            "vmovdqu [{o} + 192], ymm6",
            "vmovdqu [{o} + 224], ymm7",
            "vmovdqu [{o} + 256], ymm8",
            "vmovdqu [{o} + 288], ymm9",
            "vmovdqu [{o} + 320], ymm10",
            "vmovdqu [{o} + 352], ymm11",
            "vmovdqu [{o} + 384], ymm12",
            "vmovdqu [{o} + 416], ymm13",
            "vmovdqu [{o} + 448], ymm14",
            "vmovdqu [{o} + 480], ymm15",
            "vzeroupper",
            i = in(reg) input,
            o = in(reg) output,
            ready = in(reg) ready.as_ptr(),
            stop = in(reg) stop.as_ptr(),
            out("xmm0") _, out("xmm1") _, out("xmm2") _, out("xmm3") _,
            out("xmm4") _, out("xmm5") _, out("xmm6") _, out("xmm7") _,
            out("xmm8") _, out("xmm9") _, out("xmm10") _, out("xmm11") _,
            out("xmm12") _, out("xmm13") _, out("xmm14") _, out("xmm15") _,
        )
    };
}

/// Loads ymm0-15 from `input`, raises an illegal-instruction exception
/// (ud2) whose handler resumes at the address in r13, then stores
/// ymm0-15 to `output`.
fn fault_holding(input: &Ymm, output: &mut Ymm) {
    // SAFETY: as `hold`; the handler resumes at 3:, with r13 unchanged.
    unsafe {
        asm!(
            "vmovdqu ymm0, [{i}]",
            "vmovdqu ymm1, [{i} + 32]",
            "vmovdqu ymm2, [{i} + 64]",
            "vmovdqu ymm3, [{i} + 96]",
            "vmovdqu ymm4, [{i} + 128]",
            "vmovdqu ymm5, [{i} + 160]",
            "vmovdqu ymm6, [{i} + 192]",
            "vmovdqu ymm7, [{i} + 224]",
            "vmovdqu ymm8, [{i} + 256]",
            "vmovdqu ymm9, [{i} + 288]",
            "vmovdqu ymm10, [{i} + 320]",
            "vmovdqu ymm11, [{i} + 352]",
            "vmovdqu ymm12, [{i} + 384]",
            "vmovdqu ymm13, [{i} + 416]",
            "vmovdqu ymm14, [{i} + 448]",
            "vmovdqu ymm15, [{i} + 480]",
            "lea r13, [rip + 3f]",
            "ud2",
            "3:",
            "vmovdqu [{o}], ymm0",
            "vmovdqu [{o} + 32], ymm1",
            "vmovdqu [{o} + 64], ymm2",
            "vmovdqu [{o} + 96], ymm3",
            "vmovdqu [{o} + 128], ymm4",
            "vmovdqu [{o} + 160], ymm5",
            "vmovdqu [{o} + 192], ymm6",
            "vmovdqu [{o} + 224], ymm7",
            "vmovdqu [{o} + 256], ymm8",
            "vmovdqu [{o} + 288], ymm9",
            "vmovdqu [{o} + 320], ymm10",
            "vmovdqu [{o} + 352], ymm11",
            "vmovdqu [{o} + 384], ymm12",
            "vmovdqu [{o} + 416], ymm13",
            "vmovdqu [{o} + 448], ymm14",
            "vmovdqu [{o} + 480], ymm15",
            "vzeroupper",
            i = in(reg) input,
            o = in(reg) output,
            out("r13") _,
            out("xmm0") _, out("xmm1") _, out("xmm2") _, out("xmm3") _,
            out("xmm4") _, out("xmm5") _, out("xmm6") _, out("xmm7") _,
            out("xmm8") _, out("xmm9") _, out("xmm10") _, out("xmm11") _,
            out("xmm12") _, out("xmm13") _, out("xmm14") _, out("xmm15") _,
        )
    };
}

/// Sets every bit of ymm0-15 (what a handler or APC using AVX might do).
fn scribble_ymm() {
    // SAFETY: only vector registers, declared clobbered.
    unsafe {
        asm!(
            "vpcmpeqd ymm0, ymm0, ymm0",
            "vpcmpeqd ymm1, ymm1, ymm1",
            "vpcmpeqd ymm2, ymm2, ymm2",
            "vpcmpeqd ymm3, ymm3, ymm3",
            "vpcmpeqd ymm4, ymm4, ymm4",
            "vpcmpeqd ymm5, ymm5, ymm5",
            "vpcmpeqd ymm6, ymm6, ymm6",
            "vpcmpeqd ymm7, ymm7, ymm7",
            "vpcmpeqd ymm8, ymm8, ymm8",
            "vpcmpeqd ymm9, ymm9, ymm9",
            "vpcmpeqd ymm10, ymm10, ymm10",
            "vpcmpeqd ymm11, ymm11, ymm11",
            "vpcmpeqd ymm12, ymm12, ymm12",
            "vpcmpeqd ymm13, ymm13, ymm13",
            "vpcmpeqd ymm14, ymm14, ymm14",
            "vpcmpeqd ymm15, ymm15, ymm15",
            out("xmm0") _, out("xmm1") _, out("xmm2") _, out("xmm3") _,
            out("xmm4") _, out("xmm5") _, out("xmm6") _, out("xmm7") _,
            out("xmm8") _, out("xmm9") _, out("xmm10") _, out("xmm11") _,
            out("xmm12") _, out("xmm13") _, out("xmm14") _, out("xmm15") _,
        )
    };
}

/// Zeroes xmm0-15 with legacy SSE instructions (the upper halves stay).
fn scribble_xmm() {
    // SAFETY: only vector registers, declared clobbered.
    unsafe {
        asm!(
            "pxor xmm0, xmm0", "pxor xmm1, xmm1", "pxor xmm2, xmm2", "pxor xmm3, xmm3",
            "pxor xmm4, xmm4", "pxor xmm5, xmm5", "pxor xmm6, xmm6", "pxor xmm7, xmm7",
            "pxor xmm8, xmm8", "pxor xmm9, xmm9", "pxor xmm10, xmm10", "pxor xmm11, xmm11",
            "pxor xmm12, xmm12", "pxor xmm13, xmm13", "pxor xmm14, xmm14", "pxor xmm15, xmm15",
            out("xmm0") _, out("xmm1") _, out("xmm2") _, out("xmm3") _,
            out("xmm4") _, out("xmm5") _, out("xmm6") _, out("xmm7") _,
            out("xmm8") _, out("xmm9") _, out("xmm10") _, out("xmm11") _,
            out("xmm12") _, out("xmm13") _, out("xmm14") _, out("xmm15") _,
        )
    };
}

fn cpuid(leaf: u32, sub: u32) -> [u32; 4] {
    let r = core::arch::x86_64::__cpuid_count(leaf, sub);
    [r.eax, r.ebx, r.ecx, r.edx]
}

fn xgetbv0() -> u64 {
    let (lo, hi): (u32, u32);
    // SAFETY: XGETBV(0) is allowed in user mode once CR4.OSXSAVE is set
    // (checked first through CPUID).
    unsafe { asm!("xgetbv", in("ecx") 0, out("eax") lo, out("edx") hi, options(nomem, nostack)) };
    u64::from(hi) << 32 | u64::from(lo)
}

/// 2.0 * 3.0 + 1.0 with one fused multiply-add (vfmadd231sd).
fn fma() -> u64 {
    let result: u64;
    // SAFETY: vector registers only, declared clobbered.
    unsafe {
        asm!(
            "vmovq xmm0, {one}",
            "vmovq xmm1, {two}",
            "vmovq xmm2, {three}",
            "vfmadd231sd xmm0, xmm1, xmm2",
            "vmovq {r}, xmm0",
            one = in(reg) 0x3ff0_0000_0000_0000u64,
            two = in(reg) 0x4000_0000_0000_0000u64,
            three = in(reg) 0x4008_0000_0000_0000u64,
            r = out(reg) result,
            out("xmm0") _, out("xmm1") _, out("xmm2") _,
        )
    };
    result
}

/// vpaddq on whole YMM registers (AVX2): lane i of the sum, i = 0..3.
fn avx2_add(a: &[u64; 4], b: &[u64; 4]) -> [u64; 4] {
    let mut r = [0u64; 4];
    // SAFETY: reads `a`/`b`, writes `r`; vector registers declared clobbered.
    unsafe {
        asm!(
            "vmovdqu ymm0, [{a}]",
            "vmovdqu ymm1, [{b}]",
            "vpaddq ymm0, ymm0, ymm1",
            "vmovdqu [{r}], ymm0",
            "vzeroupper",
            a = in(reg) a.as_ptr(),
            b = in(reg) b.as_ptr(),
            r = in(reg) r.as_mut_ptr(),
            out("xmm0") _, out("xmm1") _,
        )
    };
    r
}

// --- Threads hammering their YMM registers ------------------------------

const HAMMERS: usize = 2;
/// Bad registers per hammer (bit r) in its three phases, +1 once done.
static HAMMER_RESULT: [[AtomicU32; 3]; HAMMERS + 1] =
    [const { [const { AtomicU32::new(u32::MAX) }; 3] }; HAMMERS + 1];

/// Phases: preemption only; Sleep(1)-like delays; Sleep(0)-like yields.
fn hammer(t: usize) {
    let pattern = Ymm::pattern(t as u64 + 1);
    let mut got = Ymm::ZERO;
    hold(&pattern, &mut got, 4_000_000, 1, core::ptr::null(), 0);
    HAMMER_RESULT[t][0].store(got.diff(&pattern, ALL), Ordering::SeqCst);
    let one_ms = -10_000i64;
    let mut got = Ymm::ZERO;
    hold(&pattern, &mut got, 20_000, 30, &one_ms, 0);
    HAMMER_RESULT[t][1].store(got.diff(&pattern, NONVOLATILE), Ordering::SeqCst);
    let zero = 0i64;
    let mut got = Ymm::ZERO;
    hold(&pattern, &mut got, 5_000, 200, &zero, 0);
    HAMMER_RESULT[t][2].store(got.diff(&pattern, NONVOLATILE), Ordering::SeqCst);
}

extern "system" fn hammer_thread(param: *mut c_void) -> u32 {
    hammer(param as usize);
    0
}

// --- APC ------------------------------------------------------------------

static APC_RAN: AtomicU32 = AtomicU32::new(0);
static APC_WAITING: AtomicU32 = AtomicU32::new(0);
static APC_STATUS: AtomicU64 = AtomicU64::new(0);
static APC_BAD: AtomicU32 = AtomicU32::new(u32::MAX);

extern "system" fn apc_routine(data: usize) {
    scribble_xmm();
    APC_RAN.store(data as u32, Ordering::SeqCst);
}

extern "system" fn apc_waiter(_: *mut c_void) -> u32 {
    let pattern = Ymm::pattern(0x10);
    let mut got = Ymm::ZERO;
    let five_s = -50_000_000i64;
    APC_WAITING.store(1, Ordering::SeqCst);
    // One alertable wait, ended by the APC (STATUS_USER_APC).
    hold(&pattern, &mut got, 1, 1, &five_s, 1);
    APC_BAD.store(got.diff(&pattern, NONVOLATILE), Ordering::SeqCst);
    APC_STATUS.store(1, Ordering::SeqCst);
    0
}

// --- Exception --------------------------------------------------------------

#[repr(C)]
struct ExceptionPointers {
    record: *const u32,
    context: *mut u8,
}

const NEW_UPPER: [u64; 2] = [0x0bad_cafe_0000_0001, 0x0bad_cafe_0000_0002];
/// What the handler saw: code, ContextFlags, features mask, AVX length,
/// ymm3's upper half (two lanes).
static SEEN: [AtomicU64; 6] = [const { AtomicU64::new(0) }; 6];

extern "system" fn handler(pointers: *mut ExceptionPointers) -> i32 {
    // SAFETY: the dispatcher passes valid EXCEPTION_POINTERS whose context
    // is a CONTEXT (with its CONTEXT_EX when CONTEXT_XSTATE is set).
    unsafe {
        let code = *(*pointers).record;
        if code != STATUS_ILLEGAL_INSTRUCTION {
            return 0; // EXCEPTION_CONTINUE_SEARCH
        }
        let ctx = (*pointers).context;
        SEEN[0].store(code.into(), Ordering::SeqCst);
        SEEN[1].store((ctx.add(CTX_FLAGS) as *const u32).read().into(), Ordering::SeqCst);
        let mut mask = 0u64;
        if GetXStateFeaturesMask(ctx, &mut mask) != 0 {
            SEEN[2].store(mask, Ordering::SeqCst);
        }
        let mut len = 0u32;
        let ymm = LocateXStateFeature(ctx, XSTATE_AVX, &mut len) as *mut u64;
        SEEN[3].store(len.into(), Ordering::SeqCst);
        if !ymm.is_null() {
            // The AVX feature: the upper halves of ymm0-15, 16 bytes each.
            SEEN[4].store(ymm.add(3 * 2).read_unaligned(), Ordering::SeqCst);
            SEEN[5].store(ymm.add(3 * 2 + 1).read_unaligned(), Ordering::SeqCst);
            ymm.add(4 * 2).write_unaligned(NEW_UPPER[0]);
            ymm.add(4 * 2 + 1).write_unaligned(NEW_UPPER[1]);
            SetXStateFeaturesMask(ctx, mask | XSTATE_MASK_AVX);
        }
        // The handler's own AVX use must not reach the resumed code.
        scribble_ymm();
        let resume = (ctx.add(CTX_R13) as *const u64).read_unaligned();
        (ctx.add(CTX_RIP) as *mut u64).write_unaligned(resume);
    }
    -1 // EXCEPTION_CONTINUE_EXECUTION
}

// --- A suspended thread's context -------------------------------------------

static TARGET_READY: AtomicU32 = AtomicU32::new(0);
static TARGET_STOP: AtomicU32 = AtomicU32::new(0);
static mut TARGET_OUT: Ymm = Ymm::ZERO;
const TARGET_T: u64 = 0x20;

extern "system" fn target(_: *mut c_void) -> u32 {
    let pattern = Ymm::pattern(TARGET_T);
    let mut got = Ymm::ZERO;
    hold_until(&pattern, &mut got, &TARGET_READY, &TARGET_STOP);
    // SAFETY: read by the main thread only after this thread has ended.
    unsafe { TARGET_OUT = got };
    0
}

#[repr(C, align(64))]
struct ContextBuffer([u8; 16384]);
static mut CONTEXT_BUFFER: ContextBuffer = ContextBuffer([0; 16384]);

fn spawn(start: StartRoutine, param: usize) -> Handle {
    let mut tid = 0;
    // SAFETY: a thread running `start(param)`.
    unsafe { CreateThread(core::ptr::null_mut(), 0, start, param as *mut c_void, 0, &mut tid) }
}

fn run() -> Result<(), u32> {
    let mut c = Checks(0);

    // (1) OS support for AVX, as CPUID and XGETBV report it.
    let leaf1 = cpuid(1, 0);
    c.ok(leaf1[2] & (1 << 26) != 0, leaf1[2].into())?; // XSAVE
    c.ok(leaf1[2] & (1 << 27) != 0, leaf1[2].into())?; // OSXSAVE
    c.ok(leaf1[2] & (1 << 28) != 0, leaf1[2].into())?; // AVX
    let xcr0 = xgetbv0();
    c.eq(xcr0 & 7, 7)?;
    // SAFETY: plain kernel32 calls.
    let enabled = unsafe { GetEnabledXStateFeatures() };
    c.eq(enabled & 7, 7)?;
    c.eq(enabled & !xcr0, 0)?;
    // SAFETY: as above.
    unsafe {
        c.ok(IsProcessorFeaturePresent(PF_XSAVE_ENABLED) != 0, 0)?;
        c.ok(IsProcessorFeaturePresent(PF_AVX_INSTRUCTIONS_AVAILABLE) != 0, 0)?;
    }
    if leaf1[2] & (1 << 12) != 0 {
        c.eq(fma(), 0x401c_0000_0000_0000)?; // 7.0
    }
    if cpuid(7, 0)[1] & (1 << 5) != 0 {
        let sum = avx2_add(&[1, 2, 3, u64::MAX], &[10, 20, 30, 2]);
        c.ok(sum == [11, 22, 33, 1], sum[3])?;
    }

    // (2) Three threads hold their own YMM patterns at once.
    let mut threads = [core::ptr::null_mut(); HAMMERS];
    for (t, h) in threads.iter_mut().enumerate() {
        *h = spawn(hammer_thread, t);
        c.ok(!h.is_null(), 0)?;
    }
    hammer(HAMMERS);
    for h in threads {
        // SAFETY: our thread handles.
        unsafe {
            c.eq(WaitForSingleObject(h, INFINITE).into(), 0)?;
            CloseHandle(h);
        }
    }
    for (t, phases) in HAMMER_RESULT.iter().enumerate() {
        for (p, bad) in phases.iter().enumerate() {
            let bad = bad.load(Ordering::SeqCst);
            c.eq(bad.into(), 0).map_err(|e| e + 0x100 * (t as u32 * 3 + p as u32 + 1))?;
        }
    }

    // (3) An APC run in an alertable wait.
    let h = spawn(apc_waiter, 0);
    c.ok(!h.is_null(), 0)?;
    while APC_WAITING.load(Ordering::SeqCst) == 0 {
        // SAFETY: plain kernel32 call.
        unsafe { Sleep(1) };
    }
    // SAFETY: plain kernel32 calls on our thread handle.
    unsafe {
        Sleep(50); // into its wait
        c.ok(QueueUserAPC(apc_routine, h, 7) != 0, GetLastError().into())?;
        c.eq(WaitForSingleObject(h, INFINITE).into(), 0)?;
        CloseHandle(h);
    }
    c.eq(APC_RAN.load(Ordering::SeqCst).into(), 7)?;
    c.eq(APC_STATUS.load(Ordering::SeqCst), 1)?;
    c.eq(APC_BAD.load(Ordering::SeqCst).into(), 0)?;

    // (4) An exception's CONTEXT carries the AVX state.
    // SAFETY: documented kernel32 calls.
    let registered = unsafe { AddVectoredExceptionHandler(1, handler) };
    c.ok(!registered.is_null(), 0)?;
    let pattern = Ymm::pattern(0x30);
    let mut got = Ymm::ZERO;
    fault_holding(&pattern, &mut got);
    // SAFETY: the handle AddVectoredExceptionHandler returned.
    unsafe { RemoveVectoredExceptionHandler(registered) };
    c.eq(SEEN[0].load(Ordering::SeqCst), STATUS_ILLEGAL_INSTRUCTION.into())?;
    let flags = SEEN[1].load(Ordering::SeqCst) as u32;
    c.eq((flags & CONTEXT_XSTATE).into(), CONTEXT_XSTATE.into())?;
    c.eq(SEEN[2].load(Ordering::SeqCst) & (XSTATE_MASK_AVX | XSTATE_MASK_LEGACY), 7)?;
    c.eq(SEEN[3].load(Ordering::SeqCst), 256)?;
    c.eq(SEEN[4].load(Ordering::SeqCst), pattern.0[3][2])?;
    c.eq(SEEN[5].load(Ordering::SeqCst), pattern.0[3][3])?;
    let mut want = pattern;
    want.0[4][2] = NEW_UPPER[0];
    want.0[4][3] = NEW_UPPER[1];
    c.eq(got.diff(&want, ALL).into(), 0)?;

    // (5) GetThreadContext/SetThreadContext with CONTEXT_XSTATE on a
    // suspended thread.
    let h = spawn(target, 0);
    c.ok(!h.is_null(), 0)?;
    while TARGET_READY.load(Ordering::SeqCst) == 0 {
        // SAFETY: plain kernel32 call.
        unsafe { Sleep(1) };
    }
    let pattern = Ymm::pattern(TARGET_T);
    // SAFETY: documented kernel32 calls; CONTEXT_BUFFER is used by this
    // thread only, and the pointers InitializeContext2 and
    // LocateXStateFeature return lie inside it.
    unsafe {
        c.eq(SuspendThread(h).into(), 0)?;
        let mut len = 0u32;
        let mut ctx: *mut u8 = core::ptr::null_mut();
        let ok = InitializeContext2(
            core::ptr::null_mut(),
            CONTEXT_FULL_XSTATE,
            &mut ctx,
            &mut len,
            XSTATE_MASK_AVX,
        );
        c.ok(ok == 0, 0)?;
        c.eq(GetLastError().into(), ERROR_INSUFFICIENT_BUFFER.into())?;
        let buffer = &raw mut CONTEXT_BUFFER;
        c.ok(len >= 0x4d0 + 0x18 + 64 + 256 && len as usize <= (*buffer).0.len(), len.into())?;
        let ok = InitializeContext2(
            (*buffer).0.as_mut_ptr(),
            CONTEXT_FULL_XSTATE,
            &mut ctx,
            &mut len,
            XSTATE_MASK_AVX,
        );
        c.ok(ok != 0, GetLastError().into())?;
        c.eq(ctx as u64 & 15, 0)?;
        c.ok(SetXStateFeaturesMask(ctx, XSTATE_MASK_AVX) != 0, GetLastError().into())?;
        c.ok(GetThreadContext(h, ctx) != 0, GetLastError().into())?;
        let mut mask = 0u64;
        c.ok(GetXStateFeaturesMask(ctx, &mut mask) != 0, GetLastError().into())?;
        c.eq(mask & XSTATE_MASK_AVX, XSTATE_MASK_AVX)?;
        let mut xmm_len = 0u32;
        let xmm = LocateXStateFeature(ctx, XSTATE_LEGACY_SSE, &mut xmm_len) as *mut u64;
        c.ok(!xmm.is_null(), 0)?;
        c.eq(xmm_len.into(), 256)?;
        let mut ymm_len = 0u32;
        let ymm = LocateXStateFeature(ctx, XSTATE_AVX, &mut ymm_len) as *mut u64;
        c.ok(!ymm.is_null(), 0)?;
        c.eq(ymm_len.into(), 256)?;
        let mut seen = Ymm::ZERO;
        for (r, reg) in seen.0.iter_mut().enumerate() {
            reg[0] = xmm.add(2 * r).read_unaligned();
            reg[1] = xmm.add(2 * r + 1).read_unaligned();
            reg[2] = ymm.add(2 * r).read_unaligned();
            reg[3] = ymm.add(2 * r + 1).read_unaligned();
        }
        c.eq(seen.diff(&pattern, ALL).into(), 0)?;
        // Change ymm7's upper half and xmm9; resume.
        ymm.add(2 * 7).write_unaligned(NEW_UPPER[0]);
        ymm.add(2 * 7 + 1).write_unaligned(NEW_UPPER[1]);
        xmm.add(2 * 9).write_unaligned(0x1234);
        c.ok(SetXStateFeaturesMask(ctx, XSTATE_MASK_AVX) != 0, GetLastError().into())?;
        c.ok(SetThreadContext(h, ctx) != 0, GetLastError().into())?;
        TARGET_STOP.store(1, Ordering::SeqCst);
        c.eq(ResumeThread(h).into(), 1)?;
        c.eq(WaitForSingleObject(h, 10_000).into(), 0)?;
        CloseHandle(h);
        let mut want = pattern;
        want.0[7][2] = NEW_UPPER[0];
        want.0[7][3] = NEW_UPPER[1];
        want.0[9][0] = 0x1234;
        let got = core::ptr::read(&raw const TARGET_OUT);
        c.eq(got.diff(&want, ALL).into(), 0)?;
    }
    Ok(())
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    let code = match run() {
        Ok(()) => {
            out(b"xstate_test: ok\r\n");
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

/// ` (line L)` for the failing check's source line (`#[track_caller]` on
/// Checks' methods), as bytes and their length.
fn line_text(line: u32) -> ([u8; 18], usize) {
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
    (b, 8 + len)
}
