//! Extended register state (SSE/MXCSR), with results expected to match real
//! Windows: threads keep their xmm0-xmm15 and MXCSR across preemption;
//! an exception's CONTEXT carries the faulting thread's XMM registers and
//! MXCSR (to a vectored handler registered through ntdll), and changes the
//! handler makes are restored by NtContinue; NtCreateThread honours the
//! CONTEXT's MxCsr. Prints `fpu_test: ok` or exits with the number of the
//! failing check.

#![no_std]
#![no_main]

use core::arch::asm;
use core::ffi::c_void;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use ntrt::*;

const THREADS: usize = 3;
const SPINS: u64 = 3_000_000;
/// MXCSR with all exceptions masked and rounding control 0..3.
const fn mxcsr(rounding: u32) -> u32 {
    0x1f80 | (rounding << 13)
}

/// The value thread `t` keeps in xmm register `r` (both 64-bit lanes).
const fn pattern(t: u64, r: u64) -> u64 {
    0x5eed_0000_0000_0000 | (t << 16) | r
}

/// Loads xmm0..15 with `values` and MXCSR with `csr`, spins, then stores
/// the registers and MXCSR back. Nothing between can touch them.
fn hold_registers(values: &[u64; 16], csr: u32, out: &mut [u64; 16]) -> u32 {
    let mut csr_in = csr;
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
            n = inout(reg) SPINS => _,
            csr_in = in(reg) &mut csr_in,
            csr_out = in(reg) &mut csr_out,
            default = in(reg) &mxcsr(0),
            out("xmm0") _, out("xmm1") _, out("xmm2") _, out("xmm3") _,
            out("xmm4") _, out("xmm5") _, out("xmm6") _, out("xmm7") _,
            out("xmm8") _, out("xmm9") _, out("xmm10") _, out("xmm11") _,
            out("xmm12") _, out("xmm13") _, out("xmm14") _, out("xmm15") _,
        )
    };
    csr_out
}

/// Thread `t`'s registers survived: 1 = ok, else 2.
static RESULTS: [AtomicU32; THREADS + 1] = [const { AtomicU32::new(0) }; THREADS + 1];

fn hold_and_check(t: usize) -> bool {
    let mut values = [0u64; 16];
    for (r, v) in values.iter_mut().enumerate() {
        *v = pattern(t as u64, r as u64);
    }
    let mut out = [0u64; 16];
    let csr = hold_registers(&values, mxcsr(t as u32 % 4), &mut out);
    out == values && csr == mxcsr(t as u32 % 4)
}

extern "C" fn holder(t: usize) -> ! {
    let ok = hold_and_check(t);
    RESULTS[t].store(if ok { 1 } else { 2 }, Ordering::SeqCst);
    exit_thread(0)
}

// Exceptions: CONTEXT offsets (public x64 layout).
const CTX_MX_CSR: usize = 0x34;
const CTX_R13: usize = 0xe0;
const CTX_RIP: usize = 0xf8;
const CTX_FLT_MX_CSR: usize = 0x118;
const CTX_XMM0: usize = 0x1a0;
const CTX_XMM1: usize = 0x1b0;
const XMM0_VALUE: u64 = 0x0123_4567_89ab_cdef;
const XMM1_VALUE: u64 = 0x1111_2222_3333_4444;
const XMM1_NEW: u64 = 0x5555_6666_7777_8888;

#[repr(C)]
struct ExceptionPointers {
    record: *const u8,
    context: *mut u8,
}

/// What the handler saw: Context.MxCsr, FltSave.MxCsr, Xmm0 low, Xmm1 low.
static SEEN: [AtomicU64; 4] = [const { AtomicU64::new(0) }; 4];

fn rd(p: *const u8, off: usize) -> u64 {
    // SAFETY: offsets inside the context the system passes.
    unsafe { (p.add(off) as *const u64).read_unaligned() }
}

extern "system" fn handler(pointers: *mut ExceptionPointers) -> i32 {
    // SAFETY: the dispatcher passes valid EXCEPTION_POINTERS.
    let context = unsafe { (*pointers).context };
    SEEN[0].store(rd(context, CTX_MX_CSR) & 0xffff_ffff, Ordering::SeqCst);
    SEEN[1].store(rd(context, CTX_FLT_MX_CSR) & 0xffff_ffff, Ordering::SeqCst);
    SEEN[2].store(rd(context, CTX_XMM0), Ordering::SeqCst);
    SEEN[3].store(rd(context, CTX_XMM1), Ordering::SeqCst);
    // SAFETY: rewriting the context that will be resumed.
    unsafe {
        (context.add(CTX_XMM1) as *mut u64).write_unaligned(XMM1_NEW);
        (context.add(CTX_MX_CSR) as *mut u32).write_unaligned(mxcsr(1));
        (context.add(CTX_FLT_MX_CSR) as *mut u32).write_unaligned(mxcsr(1));
        let resume = rd(context, CTX_R13);
        (context.add(CTX_RIP) as *mut u64).write_unaligned(resume);
    }
    -1 // EXCEPTION_CONTINUE_EXECUTION
}

type AddHandler =
    extern "system" fn(u32, extern "system" fn(*mut ExceptionPointers) -> i32) -> *mut c_void;

/// Faults with known xmm0/xmm1/MXCSR; returns them as seen after resuming.
fn fault_with_state() -> (u64, u64, u32) {
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
            csr_in = in(reg) &mxcsr(3),
            csr_out = in(reg) &mut csr_out,
            default = in(reg) &mxcsr(0),
            a = in(reg) XMM0_VALUE,
            b = in(reg) XMM1_VALUE,
            x0 = out(reg) x0,
            x1 = out(reg) x1,
            out("r13") _,
            out("xmm0") _,
            out("xmm1") _,
        )
    };
    (x0, x1, csr_out)
}

static NEW_THREAD_MXCSR: AtomicU32 = AtomicU32::new(0);

extern "C" fn report_mxcsr(_: usize) -> ! {
    let mut csr = 0u32;
    // SAFETY: stores MXCSR into a local.
    unsafe { asm!("stmxcsr [{}]", in(reg) &mut csr) };
    NEW_THREAD_MXCSR.store(csr, Ordering::SeqCst);
    exit_thread(0)
}

struct Checks(u32);

impl Checks {
    fn eq(&mut self, got: u64, want: u64) -> Result<(), u32> {
        self.ok(got == want, got)
    }

    fn ok(&mut self, cond: bool, got: u64) -> Result<(), u32> {
        self.0 += 1;
        if cond {
            return Ok(());
        }
        print("fpu_test: check ");
        write_file(stdout(), &hex32(self.0));
        print(" got ");
        write_file(stdout(), &hex32(got as u32));
        print("\r\n");
        Err(self.0)
    }
}

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    let ok = u64::from(STATUS_SUCCESS);

    // (1) Registers survive preemption, for every thread at once.
    let mut handles = [Handle(0); THREADS];
    for (t, h) in handles.iter_mut().enumerate() {
        let (s, handle, _) = create_thread(holder, t, false);
        c.eq(s.into(), ok)?;
        *h = handle;
    }
    let main_ok = hold_and_check(THREADS);
    for h in handles {
        c.eq(wait(h, None).into(), ok)?;
        close(h);
    }
    for (t, r) in RESULTS.iter().take(THREADS).enumerate() {
        c.eq(r.load(Ordering::SeqCst).into(), 1).map_err(|e| e + 100 * t as u32)?;
    }
    c.ok(main_ok, 0)?;

    // (2) CONTEXT carries XMM and MXCSR; NtContinue restores changes.
    let Some(ntdll) = find_ntdll() else { return c.ok(false, 0) };
    let Some(add) = find_export(ntdll, b"RtlAddVectoredExceptionHandler") else {
        return c.ok(false, 1);
    };
    // SAFETY: the documented signature of the export.
    let add = unsafe { core::mem::transmute::<usize, AddHandler>(add) };
    c.ok(!add(1, handler).is_null(), 0)?;
    let (x0, x1, csr) = fault_with_state();
    c.eq(SEEN[0].load(Ordering::SeqCst), mxcsr(3).into())?;
    c.eq(SEEN[1].load(Ordering::SeqCst), mxcsr(3).into())?;
    c.eq(SEEN[2].load(Ordering::SeqCst), XMM0_VALUE)?;
    c.eq(SEEN[3].load(Ordering::SeqCst), XMM1_VALUE)?;
    c.eq(x0, XMM0_VALUE)?;
    c.eq(x1, XMM1_NEW)?;
    c.eq(csr.into(), mxcsr(1).into())?;

    // (3) NtCreateThread honours CONTEXT.MxCsr.
    let mut ctx = Context::for_thread(report_mxcsr, 0, 0);
    ctx.mx_csr = mxcsr(2);
    let (s, t, _) = create_thread_with_context(&mut ctx);
    c.eq(s.into(), ok)?;
    c.eq(wait(t, None).into(), ok)?;
    close(t);
    c.eq(NEW_THREAD_MXCSR.load(Ordering::SeqCst).into(), mxcsr(2).into())?;
    Ok(())
}

fn main() -> u32 {
    match run() {
        Ok(()) => {
            print("fpu_test: ok\r\n");
            0
        }
        Err(id) => id,
    }
}

entry!(main);
