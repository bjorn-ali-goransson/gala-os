//! x64's part: xmm0-xmm15 and MXCSR (CONTEXT: MxCsr at 0x34, FltSave
//! with its MxCsr at 0x118, Xmm0 at 0x1a0).

use core::arch::asm;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use ntrt::*;

/// The vector registers a thread holds its pattern in.
pub const REGS: usize = 16;

/// MXCSR with all exceptions masked and rounding control 0..3.
pub const fn csr(rounding: u32) -> u32 {
    0x1f80 | (rounding << 13)
}

/// The value thread `t` keeps in xmm register `r` (both 64-bit lanes).
pub const fn pattern(t: u64, r: u64) -> u64 {
    0x5eed_0000_0000_0000 | (t << 16) | r
}

/// Loads xmm0..15 with `values` and MXCSR with `csr`, spins, then stores
/// the registers and MXCSR back. Nothing between can touch them.
pub fn hold_registers(values: &[u64; REGS], csr_value: u32, out: &mut [u64; REGS]) -> u32 {
    let mut csr_in = csr_value;
    let mut csr_out = 0u32;
    // SAFETY: only reads `values` and writes `out`; all xmm registers are
    // declared clobbered.
    unsafe {
        asm!(
            "ldmxcsr [{csr_in}]",
            "movq xmm0, [{v} + 0]", "movq xmm1, [{v} + 8]", "movq xmm2, [{v} + 16]",
            "movq xmm3, [{v} + 24]", "movq xmm4, [{v} + 32]", "movq xmm5, [{v} + 40]",
            "movq xmm6, [{v} + 48]", "movq xmm7, [{v} + 56]", "movq xmm8, [{v} + 64]",
            "movq xmm9, [{v} + 72]", "movq xmm10, [{v} + 80]", "movq xmm11, [{v} + 88]",
            "movq xmm12, [{v} + 96]", "movq xmm13, [{v} + 104]", "movq xmm14, [{v} + 112]",
            "movq xmm15, [{v} + 120]",
            "2:",
            "pause",
            "dec {n}",
            "jnz 2b",
            "movq [{o} + 0], xmm0", "movq [{o} + 8], xmm1", "movq [{o} + 16], xmm2",
            "movq [{o} + 24], xmm3", "movq [{o} + 32], xmm4", "movq [{o} + 40], xmm5",
            "movq [{o} + 48], xmm6", "movq [{o} + 56], xmm7", "movq [{o} + 64], xmm8",
            "movq [{o} + 72], xmm9", "movq [{o} + 80], xmm10", "movq [{o} + 88], xmm11",
            "movq [{o} + 96], xmm12", "movq [{o} + 104], xmm13", "movq [{o} + 112], xmm14",
            "movq [{o} + 120], xmm15",
            "stmxcsr [{csr_out}]",
            "ldmxcsr [{default}]",
            v = in(reg) values.as_ptr(),
            o = in(reg) out.as_mut_ptr(),
            n = inout(reg) super::SPINS => _,
            csr_in = in(reg) &mut csr_in,
            csr_out = in(reg) &mut csr_out,
            default = in(reg) &csr(0),
            out("xmm0") _, out("xmm1") _, out("xmm2") _, out("xmm3") _,
            out("xmm4") _, out("xmm5") _, out("xmm6") _, out("xmm7") _,
            out("xmm8") _, out("xmm9") _, out("xmm10") _, out("xmm11") _,
            out("xmm12") _, out("xmm13") _, out("xmm14") _, out("xmm15") _,
        )
    };
    csr_out
}

// Exceptions: CONTEXT offsets (public x64 layout).
const CTX_MX_CSR: usize = 0x34;
const CTX_R13: usize = 0xe0;
const CTX_RIP: usize = 0xf8;
const CTX_FLT_MX_CSR: usize = 0x118;
const CTX_XMM0: usize = 0x1a0;
const CTX_XMM1: usize = 0x1b0;
pub const V0_VALUE: u64 = 0x0123_4567_89ab_cdef;
pub const V1_VALUE: u64 = 0x1111_2222_3333_4444;
pub const V1_NEW: u64 = 0x5555_6666_7777_8888;

#[repr(C)]
pub struct ExceptionPointers {
    record: *const u8,
    context: *mut u8,
}

/// What the handler saw: Context.MxCsr, FltSave.MxCsr, Xmm0 low, Xmm1 low.
pub static SEEN: [AtomicU64; 4] = [const { AtomicU64::new(0) }; 4];

fn rd(p: *const u8, off: usize) -> u64 {
    // SAFETY: offsets inside the context the system passes.
    unsafe { (p.add(off) as *const u64).read_unaligned() }
}

pub extern "system" fn handler(pointers: *mut ExceptionPointers) -> i32 {
    // SAFETY: the dispatcher passes valid EXCEPTION_POINTERS.
    let context = unsafe { (*pointers).context };
    SEEN[0].store(rd(context, CTX_MX_CSR) & 0xffff_ffff, Ordering::SeqCst);
    SEEN[1].store(rd(context, CTX_FLT_MX_CSR) & 0xffff_ffff, Ordering::SeqCst);
    SEEN[2].store(rd(context, CTX_XMM0), Ordering::SeqCst);
    SEEN[3].store(rd(context, CTX_XMM1), Ordering::SeqCst);
    // SAFETY: rewriting the context that will be resumed.
    unsafe {
        (context.add(CTX_XMM1) as *mut u64).write_unaligned(V1_NEW);
        (context.add(CTX_MX_CSR) as *mut u32).write_unaligned(csr(1));
        (context.add(CTX_FLT_MX_CSR) as *mut u32).write_unaligned(csr(1));
        let resume = rd(context, CTX_R13);
        (context.add(CTX_RIP) as *mut u64).write_unaligned(resume);
    }
    -1 // EXCEPTION_CONTINUE_EXECUTION
}

pub const CONTEXT_FLOATING_POINT: u32 = 0x10_0008;
pub const V0_SET: u64 = 0x5678_9abc_def0_1234;

/// Faults with known xmm0/xmm1/MXCSR; returns them as seen after resuming.
pub fn fault_with_state() -> (u64, u64, u32) {
    let (x0, x1): (u64, u64);
    let mut csr_out = 0u32;
    // SAFETY: ud2 never completes; the handler resumes at 3:. MXCSR is
    // reset before leaving.
    unsafe {
        asm!(
            "ldmxcsr [{csr_in}]",
            "movq xmm0, {a}",
            "movq xmm1, {b}",
            "lea r13, [rip + 3f]",
            "ud2",
            "3:",
            "movq {x0}, xmm0",
            "movq {x1}, xmm1",
            "stmxcsr [{csr_out}]",
            "ldmxcsr [{default}]",
            csr_in = in(reg) &csr(3),
            csr_out = in(reg) &mut csr_out,
            default = in(reg) &csr(0),
            a = in(reg) V0_VALUE,
            b = in(reg) V1_VALUE,
            x0 = out(reg) x0,
            x1 = out(reg) x1,
            out("r13") _,
            out("xmm0") _,
            out("xmm1") _,
        )
    };
    (x0, x1, csr_out)
}

pub static NEW_THREAD_CSR: AtomicU32 = AtomicU32::new(0);
pub static NEW_THREAD_V0: AtomicU64 = AtomicU64::new(u64::MAX);

pub extern "C" fn report_csr(_: usize) -> ! {
    let mut csr = 0u32;
    let xmm0: u64;
    // SAFETY: stores MXCSR into a local and reads xmm0's low half.
    unsafe { asm!("stmxcsr [{}]", "movq {}, xmm0", in(reg) &mut csr, out(reg) xmm0) };
    NEW_THREAD_CSR.store(csr, Ordering::SeqCst);
    NEW_THREAD_V0.store(xmm0, Ordering::SeqCst);
    exit_thread(0)
}

/// Sets a CONTEXT's floating-point part: MxCsr, FltSave.MxCsr and xmm0's
/// low half.
pub fn set_context_fp(ctx: &mut Context, csr_value: u32, v0: u64) {
    ctx.mx_csr = csr_value;
    ctx.rest[0x18..0x1c].copy_from_slice(&csr_value.to_le_bytes());
    ctx.rest[0xa0..0xa8].copy_from_slice(&v0.to_le_bytes());
}
