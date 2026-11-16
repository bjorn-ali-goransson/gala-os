//! ARM64's part: the SIMD and floating-point registers' low halves
//! (d0-d31) and FPCR (its rounding mode, RMode at bits 22-23), as x64's
//! xmm registers and MXCSR. ARM64's CONTEXT: V0-V31 from 0x110 (16 bytes
//! each), Fpcr at 0x310, X20 at 0xa8, Pc at 0x108.

use core::arch::asm;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use ntrt::*;

/// The vector registers a thread holds its pattern in.
pub const REGS: usize = 32;

/// FPCR with rounding mode 0..3 (nearest, +inf, -inf, zero), no traps.
pub const fn csr(rounding: u32) -> u32 {
    rounding << 22
}

/// The value thread `t` keeps in register `r` (its low 64 bits).
pub const fn pattern(t: u64, r: u64) -> u64 {
    0x5eed_0000_0000_0000 | (t << 16) | r
}

/// Loads d0..d31 with `values` and FPCR with `csr_value`, spins, then
/// stores the registers and FPCR back. Nothing between can touch them.
pub fn hold_registers(values: &[u64; REGS], csr_value: u32, out: &mut [u64; REGS]) -> u32 {
    let csr_out: u64;
    // SAFETY: only reads `values` and writes `out`; all vector registers
    // are declared clobbered; FPCR is reset before leaving.
    unsafe {
        asm!(
            "msr fpcr, {csr_in}",
            "ldp d0, d1, [{v}, #0]", "ldp d2, d3, [{v}, #16]", "ldp d4, d5, [{v}, #32]",
            "ldp d6, d7, [{v}, #48]", "ldp d8, d9, [{v}, #64]", "ldp d10, d11, [{v}, #80]",
            "ldp d12, d13, [{v}, #96]", "ldp d14, d15, [{v}, #112]",
            "ldp d16, d17, [{v}, #128]", "ldp d18, d19, [{v}, #144]",
            "ldp d20, d21, [{v}, #160]", "ldp d22, d23, [{v}, #176]",
            "ldp d24, d25, [{v}, #192]", "ldp d26, d27, [{v}, #208]",
            "ldp d28, d29, [{v}, #224]", "ldp d30, d31, [{v}, #240]",
            "2:",
            "yield",
            "subs {n}, {n}, #1",
            "b.ne 2b",
            "stp d0, d1, [{o}, #0]", "stp d2, d3, [{o}, #16]", "stp d4, d5, [{o}, #32]",
            "stp d6, d7, [{o}, #48]", "stp d8, d9, [{o}, #64]", "stp d10, d11, [{o}, #80]",
            "stp d12, d13, [{o}, #96]", "stp d14, d15, [{o}, #112]",
            "stp d16, d17, [{o}, #128]", "stp d18, d19, [{o}, #144]",
            "stp d20, d21, [{o}, #160]", "stp d22, d23, [{o}, #176]",
            "stp d24, d25, [{o}, #192]", "stp d26, d27, [{o}, #208]",
            "stp d28, d29, [{o}, #224]", "stp d30, d31, [{o}, #240]",
            "mrs {csr_out}, fpcr",
            "msr fpcr, xzr",
            v = in(reg) values.as_ptr(),
            o = in(reg) out.as_mut_ptr(),
            n = inout(reg) super::SPINS => _,
            csr_in = in(reg) u64::from(csr_value),
            csr_out = out(reg) csr_out,
            out("v0") _, out("v1") _, out("v2") _, out("v3") _,
            out("v4") _, out("v5") _, out("v6") _, out("v7") _,
            out("v8") _, out("v9") _, out("v10") _, out("v11") _,
            out("v12") _, out("v13") _, out("v14") _, out("v15") _,
            out("v16") _, out("v17") _, out("v18") _, out("v19") _,
            out("v20") _, out("v21") _, out("v22") _, out("v23") _,
            out("v24") _, out("v25") _, out("v26") _, out("v27") _,
            out("v28") _, out("v29") _, out("v30") _, out("v31") _,
        )
    };
    csr_out as u32
}

// Exceptions: CONTEXT offsets (ARM64's).
const CTX_X20: usize = 0xa8;
const CTX_PC: usize = 0x108;
const CTX_V0: usize = 0x110;
const CTX_V1: usize = 0x120;
const CTX_FPCR: usize = 0x310;
pub const V0_VALUE: u64 = 0x0123_4567_89ab_cdef;
pub const V1_VALUE: u64 = 0x1111_2222_3333_4444;
pub const V1_NEW: u64 = 0x5555_6666_7777_8888;

#[repr(C)]
pub struct ExceptionPointers {
    record: *const u8,
    context: *mut u8,
}

/// What the handler saw: Fpcr (twice: ARM64's CONTEXT has one copy where
/// x64's has two), V0 low, V1 low.
pub static SEEN: [AtomicU64; 4] = [const { AtomicU64::new(0) }; 4];

fn rd(p: *const u8, off: usize) -> u64 {
    // SAFETY: offsets inside the context the system passes.
    unsafe { (p.add(off) as *const u64).read_unaligned() }
}

pub extern "system" fn handler(pointers: *mut ExceptionPointers) -> i32 {
    // SAFETY: the dispatcher passes valid EXCEPTION_POINTERS.
    let context = unsafe { (*pointers).context };
    let fpcr = rd(context, CTX_FPCR) & 0xffff_ffff;
    SEEN[0].store(fpcr, Ordering::SeqCst);
    SEEN[1].store(fpcr, Ordering::SeqCst);
    SEEN[2].store(rd(context, CTX_V0), Ordering::SeqCst);
    SEEN[3].store(rd(context, CTX_V1), Ordering::SeqCst);
    // SAFETY: rewriting the context that will be resumed.
    unsafe {
        (context.add(CTX_V1) as *mut u64).write_unaligned(V1_NEW);
        (context.add(CTX_FPCR) as *mut u32).write_unaligned(csr(1));
        let resume = rd(context, CTX_X20);
        (context.add(CTX_PC) as *mut u64).write_unaligned(resume);
    }
    -1 // EXCEPTION_CONTINUE_EXECUTION
}

pub const CONTEXT_FLOATING_POINT: u32 = 0x40_0004;
pub const V0_SET: u64 = 0x5678_9abc_def0_1234;

/// Faults (`udf`) with known d0/d1/FPCR; returns them as seen after
/// resuming.
pub fn fault_with_state() -> (u64, u64, u32) {
    let (x0, x1, csr_out): (u64, u64, u64);
    // SAFETY: udf never completes; the handler resumes at 3: (x20). FPCR
    // is reset before leaving.
    unsafe {
        asm!(
            "msr fpcr, {csr_in}",
            "fmov d0, {a}",
            "fmov d1, {b}",
            "adr x20, 3f",
            "udf #0",
            "3:",
            "fmov {x0}, d0",
            "fmov {x1}, d1",
            "mrs {csr_out}, fpcr",
            "msr fpcr, xzr",
            csr_in = in(reg) u64::from(csr(3)),
            csr_out = out(reg) csr_out,
            a = in(reg) V0_VALUE,
            b = in(reg) V1_VALUE,
            x0 = out(reg) x0,
            x1 = out(reg) x1,
            out("x20") _,
            out("v0") _,
            out("v1") _,
        )
    };
    (x0, x1, csr_out as u32)
}

pub static NEW_THREAD_CSR: AtomicU32 = AtomicU32::new(u32::MAX);
pub static NEW_THREAD_V0: AtomicU64 = AtomicU64::new(u64::MAX);

pub extern "C" fn report_csr(_: usize) -> ! {
    let (fpcr, v0): (u64, u64);
    // SAFETY: reads FPCR and d0.
    unsafe { asm!("mrs {}, fpcr", "fmov {}, d0", out(reg) fpcr, out(reg) v0) };
    NEW_THREAD_CSR.store(fpcr as u32, Ordering::SeqCst);
    NEW_THREAD_V0.store(v0, Ordering::SeqCst);
    exit_thread(0)
}

/// Sets a CONTEXT's floating-point part: Fpcr and V0's low half.
pub fn set_context_fp(ctx: &mut Context, csr_value: u32, v0: u64) {
    let rest = |off: usize| off - 0x110;
    ctx.rest[rest(CTX_FPCR)..rest(CTX_FPCR) + 4].copy_from_slice(&csr_value.to_le_bytes());
    ctx.rest[rest(CTX_V0)..rest(CTX_V0) + 8].copy_from_slice(&v0.to_le_bytes());
}
