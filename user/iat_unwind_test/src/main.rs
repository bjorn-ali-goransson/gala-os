//! An exception raised inside a call made through an armed import slot
//! and unwound past that call: the frame the unwinder must walk through
//! is the callee's in the DLL, whose return address `imports_answers=`
//! took away (docs/OBSERVABILITY.md, "Import tracing"). Frame-based
//! exception handling is seh_test's subject; this is the one thing about
//! it that import tracing can break.
//!
//! The chain is `uw_outer` (assembly, so that its frame and its handler
//! are known) -> `bridge` -> `testdll!gala_call_back` -> `step`, which
//! raises with RtlRaiseException; `uw_outer`'s handler unwinds to
//! `uw_outer_resume` with RtlUnwindEx. `step` can go through
//! `gala_call_back` again first, so that an unwind crosses one, two or
//! three frames whose returns are trapped at once.
//!
//! What the kernel must do is give those return addresses back when it
//! dispatches the exception: an unwinder reads them from the stack, and
//! a kernel trap address is in no image and in no function table, so
//! one left in place is where the unwind ends. The calls unwound past
//! never return, so no answer is lost by it.
//!
//! The raise can go through a slot of its own as well
//! (`testdll!gala_raise_at`, which jumps straight on to
//! RtlRaiseException): the call that raises is then the one whose
//! return is trapped, so the pc the program's own walk begins at is a
//! trap address and the walk ends on its first frame -- not one handler
//! asked -- unless the kernel gives that call's return address back
//! with it. That is the shape of Word's C++ throw, where
//! `_CxxThrowException`, `RaiseException` and `RtlRaiseException` are
//! each an import like any other.
//!
//! The expected results are Windows', where nothing is armed and the
//! unwind simply works: every check holds with the option and without
//! it, which is the claim. Prints `iat_unwind_test: ok`, or exits with
//! the number of the failing check.

#![no_std]
#![no_main]

use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

use ntrt::*;

#[link(name = "testdll", kind = "raw-dylib")]
unsafe extern "C" {
    fn gala_add(a: u64, b: u64) -> u64;
    fn gala_call_back(f: extern "C" fn(u64) -> u64, x: u64) -> u64;
    fn gala_raise_at(record: *mut u8, raiser: usize);
}

const EXCEPTION_CONTINUE_SEARCH: u32 = 1;
const EXCEPTION_UNWINDING: u32 = 2;
/// EXCEPTION_RECORD and DISPATCHER_CONTEXT (the 64-bit layouts).
const ER_CODE: usize = 0x00;
const ER_FLAGS: usize = 0x04;
const RECORD_SIZE: usize = 0x98;
const DC_CONTEXT_RECORD: usize = 0x28;
const DC_HISTORY_TABLE: usize = 0x40;
/// Raised by `step` and unwound by `uw_outer`'s handler.
const CODE_UNWIND: u32 = 0xe000_0101;
/// What `step` answers when it is not raising.
const PLAIN: u64 = 100;
/// The first nonvolatile register (rbx, x19) in `uw_outer`'s body: the
/// unwind must resume with it, as a `catch` block would.
const NV_HIGH: u64 = 0x5151;
const OUTER_NV: u64 = NV_HIGH << 48;

static RTL_RAISE: AtomicUsize = AtomicUsize::new(0);
static RTL_UNWIND_EX: AtomicUsize = AtomicUsize::new(0);
/// `uw_outer`'s establisher frame, recorded by itself: x64's is the
/// stack pointer of the function's body, ARM64's the sp unwinding the
/// function ends with (32 bytes above the body's: x19, x20 and the frame
/// record).
static OUTER_SP: AtomicU64 = AtomicU64::new(0);
/// The nonvolatile register at the resume point.
static RESUME_NV: AtomicU64 = AtomicU64::new(0);
/// What the handler saw: calls (the search pass only), the code, the
/// establisher frame.
static HANDLER: [AtomicU64; 3] = [const { AtomicU64::new(0) }; 3];
/// Whether `raise` goes through the import slot of testdll's
/// `gala_raise_at`, which jumps straight on to RtlRaiseException: the
/// call that raises is then the one whose return is trapped, so the pc
/// the walk begins at is that slot's return trap and the frame it
/// begins in is `raise`'s own.
static THROUGH_SLOT: AtomicBool = AtomicBool::new(false);
/// The value the handler unwinds with, and the code `step` raises (zero:
/// answer `PLAIN` instead).
static UNWIND_VALUE: AtomicU64 = AtomicU64::new(0);
static CODE: AtomicU64 = AtomicU64::new(0);

#[cfg(target_arch = "x86_64")]
core::arch::global_asm!(
    ".text",
    ".globl uw_outer",
    ".globl uw_outer_resume",
    ".def uw_outer; .scl 2; .type 32; .endef",
    "uw_outer:",
    ".seh_proc uw_outer",
    "push rbx",
    ".seh_pushreg rbx",
    "sub rsp, 0x20",
    ".seh_stackalloc 0x20",
    ".seh_endprologue",
    ".seh_handler {handler}, @except, @unwind",
    "mov [rip + {outer_sp}], rsp",
    "mov rbx, {outer_nv}",
    "call {bridge}",
    "uw_outer_resume:",
    "mov [rip + {resume_nv}], rbx",
    "add rsp, 0x20",
    "pop rbx",
    "ret",
    ".seh_endproc",
    handler = sym handler,
    bridge = sym bridge,
    outer_sp = sym OUTER_SP,
    resume_nv = sym RESUME_NV,
    outer_nv = const OUTER_NV,
);

#[cfg(target_arch = "aarch64")]
core::arch::global_asm!(
    ".text",
    ".globl uw_outer",
    ".globl uw_outer_resume",
    ".p2align 2",
    "uw_outer:",
    ".seh_proc uw_outer",
    "stp x19, x20, [sp, #-32]!",
    ".seh_save_r19r20_x 32",
    "stp x29, x30, [sp, #16]",
    ".seh_save_fplr 16",
    ".seh_endprologue",
    ".seh_handler {handler}, @except, @unwind",
    "add x9, sp, #32",
    "adrp x10, {outer_sp}",
    "str x9, [x10, :lo12:{outer_sp}]",
    "movz x19, #{nv_high}, lsl #48",
    "bl {bridge}",
    "uw_outer_resume:",
    "adrp x10, {resume_nv}",
    "str x19, [x10, :lo12:{resume_nv}]",
    ".seh_startepilogue",
    "ldp x29, x30, [sp, #16]",
    ".seh_save_fplr 16",
    "ldp x19, x20, [sp], #32",
    ".seh_save_r19r20_x 32",
    ".seh_endepilogue",
    "ret",
    ".seh_endproc",
    handler = sym handler,
    bridge = sym bridge,
    outer_sp = sym OUTER_SP,
    resume_nv = sym RESUME_NV,
    nv_high = const NV_HIGH,
);

unsafe extern "C" {
    /// Calls `bridge(depth)` under a handler of its own; answers what it
    /// answered, or `UNWIND_VALUE` when the handler unwound to
    /// `uw_outer_resume`.
    fn uw_outer(depth: u64) -> u64;
    fn uw_outer_resume();
}

fn rd(p: *const u8, off: usize) -> u64 {
    // SAFETY: offsets inside the structures the system passes.
    unsafe { (p.add(off) as *const u64).read_unaligned() }
}

fn rd32(p: *const u8, off: usize) -> u32 {
    rd(p, off) as u32
}

fn wr(p: *mut u8, off: usize, v: u64) {
    // SAFETY: offsets inside a record of this program's own.
    unsafe { (p.add(off) as *mut u64).write_unaligned(v) }
}

/// The frame between `uw_outer` and the DLL, so that the call through
/// the import slot is an ordinary compiled call: its return address is
/// the word `imports_answers=` replaces.
extern "system" fn bridge(depth: u64) -> u64 {
    // SAFETY: imported from testdll, bound by the loader; the callback
    // is this program's own function.
    unsafe { gala_call_back(step, depth) }
}

/// One level of the chain: another call through the same slot while
/// `depth` is left, else the raise (or `PLAIN`, with nothing to raise).
extern "C" fn step(depth: u64) -> u64 {
    if depth > 1 {
        // SAFETY: as in `bridge`.
        return unsafe { gala_call_back(step, depth - 1) };
    }
    match CODE.load(Ordering::SeqCst) {
        0 => PLAIN,
        code => raise(code),
    }
}

/// RtlRaiseException with `code` and no parameters: dispatched to the
/// frame handlers, innermost first, so `uw_outer`'s sees it.
fn raise(code: u64) -> u64 {
    let mut record = [0u8; RECORD_SIZE];
    let r = record.as_mut_ptr();
    wr(r, ER_CODE, code);
    let raiser = RTL_RAISE.load(Ordering::SeqCst);
    if THROUGH_SLOT.load(Ordering::SeqCst) {
        // SAFETY: imported from testdll, bound by the loader; it jumps
        // straight on to `raiser` with the record, so the exception is
        // raised at this frame and with the stack pointer the call
        // through the slot would have returned with.
        unsafe { gala_raise_at(r, raiser) };
    } else {
        // SAFETY: the documented signature of ntdll's RtlRaiseException.
        let raise: extern "system" fn(*mut u8) = unsafe { core::mem::transmute(raiser) };
        raise(r);
    }
    // Not reached: the handler unwinds past this frame.
    0
}

type UnwindEx = extern "system" fn(u64, usize, *mut u8, u64, *mut u8, *mut u8);

/// `uw_outer`'s language-specific handler: on the search pass it unwinds
/// to `uw_outer_resume` with `UNWIND_VALUE`, which is what a `catch`
/// block is. The unwind walks back through the frames of `step`, of the
/// DLL's `gala_call_back` and of `bridge`.
extern "system" fn handler(
    record: *mut u8,
    frame: u64,
    _context: *mut u8,
    dispatcher: *mut u8,
) -> u32 {
    if rd32(record, ER_FLAGS) & EXCEPTION_UNWINDING != 0 {
        return EXCEPTION_CONTINUE_SEARCH;
    }
    HANDLER[0].fetch_add(1, Ordering::SeqCst);
    HANDLER[1].store(u64::from(rd32(record, ER_CODE)), Ordering::SeqCst);
    HANDLER[2].store(frame, Ordering::SeqCst);
    // SAFETY: the documented signature of ntdll's RtlUnwindEx.
    let unwind: UnwindEx = unsafe { core::mem::transmute(RTL_UNWIND_EX.load(Ordering::SeqCst)) };
    unwind(
        frame,
        uw_outer_resume as *const () as usize,
        record,
        UNWIND_VALUE.load(Ordering::SeqCst),
        rd(dispatcher, DC_CONTEXT_RECORD) as *mut u8,
        rd(dispatcher, DC_HISTORY_TABLE) as *mut u8,
    );
    // Not reached.
    EXCEPTION_CONTINUE_SEARCH
}

/// One unwind across `depth` calls through the slot, with `value` as its
/// result: `uw_outer` answers it, the handler ran once on the search
/// pass with the code raised and `uw_outer`'s own establisher frame, and
/// the resume point has the nonvolatile register the body set.
fn unwound(c: &mut Checks, depth: u64, value: u64) -> Result<(), u32> {
    UNWIND_VALUE.store(value, Ordering::SeqCst);
    RESUME_NV.store(0, Ordering::SeqCst);
    for a in &HANDLER {
        a.store(0, Ordering::SeqCst);
    }
    // SAFETY: uw_outer calls into testdll under its handler, which
    // unwinds the exception raised inside it.
    c.eq(unsafe { uw_outer(depth) }, value)?;
    c.eq(HANDLER[0].load(Ordering::SeqCst), 1)?;
    c.eq(HANDLER[1].load(Ordering::SeqCst), CODE_UNWIND.into())?;
    c.eq(HANDLER[2].load(Ordering::SeqCst), OUTER_SP.load(Ordering::SeqCst))?;
    c.eq(RESUME_NV.load(Ordering::SeqCst), OUTER_NV)
}

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    let (Some(raise), Some(unwind)) =
        (ntdll_export(b"RtlRaiseException"), ntdll_export(b"RtlUnwindEx"))
    else {
        return c.ok(false, 0);
    };
    RTL_RAISE.store(raise, Ordering::SeqCst);
    RTL_UNWIND_EX.store(unwind, Ordering::SeqCst);

    // The control, with nothing raised: three calls through the slot,
    // each adding one to what the callback answered.
    CODE.store(0, Ordering::SeqCst);
    // SAFETY: as in `unwound`, with nothing to raise.
    c.eq(unsafe { uw_outer(3) }, PLAIN + 3)?;

    // One trapped return to unwind through, four times over: a pending
    // return the kernel gives back is gone for good, so the fifth unwind
    // must work as the first did.
    CODE.store(CODE_UNWIND.into(), Ordering::SeqCst);
    for i in 0..4 {
        unwound(&mut c, 1, 0x4000 + i)?;
    }
    // Two and three at once, the shape of a C++ throw through a few
    // frames of a DLL.
    unwound(&mut c, 2, 0x4100)?;
    unwound(&mut c, 3, 0x4200)?;

    // The raise through a slot of its own, across one trapped frame
    // and three: the pc the walk begins at is the slot's return trap,
    // so without the call's own return address back with it no handler
    // is asked at all and the exception kills the program.
    THROUGH_SLOT.store(true, Ordering::SeqCst);
    unwound(&mut c, 1, 0x4300)?;
    unwound(&mut c, 3, 0x4400)?;
    THROUGH_SLOT.store(false, Ordering::SeqCst);

    // And the slot is a slot still: the calls after all that are
    // recorded and answered as the first were.
    CODE.store(0, Ordering::SeqCst);
    // SAFETY: imported from testdll, bound by the loader.
    c.eq(unsafe { gala_add(2, 3) }, 5)?;
    // SAFETY: as in `unwound`, with nothing to raise.
    c.eq(unsafe { uw_outer(2) }, PLAIN + 2)?;
    Ok(())
}

fn main() -> u32 {
    match run() {
        Ok(()) => {
            print("iat_unwind_test: ok\r\n");
            0
        }
        Err(id) => id,
    }
}

entry!(main);

struct Checks(u32);

impl Checks {
    #[track_caller]
    fn eq(&mut self, got: u64, want: u64) -> Result<(), u32> {
        self.ok(got == want, got)
    }

    #[track_caller]
    fn ok(&mut self, cond: bool, got: u64) -> Result<(), u32> {
        self.0 += 1;
        if cond {
            return Ok(());
        }
        print("iat_unwind_test: check ");
        write_file(stdout(), &hex32(self.0));
        print(" got ");
        write_file(stdout(), &hex32((got >> 32) as u32));
        write_file(stdout(), &hex32(got as u32));
        check_line(core::panic::Location::caller().line());
        print("\r\n");
        Err(self.0)
    }
}

/// ` (line L)`: the source line of the failing check, from
/// `#[track_caller]` on Checks' methods.
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
    print(core::str::from_utf8(&b[..8 + len]).unwrap_or(""));
}
