//! Frame-based exception handling, as documented in "x64 exception
//! handling" and "ARM64 exception handling" (Microsoft Learn): unwind
//! information in .pdata and .xdata, language-specific handlers and the
//! ntdll functions that use them. The functions under test are written in
//! assembly with explicit unwind directives, so that their prologues and
//! frames are known; each architecture's are its module's (`x64.rs`,
//! `arm64.rs`, with the same contracts), and so are the checks of its own
//! unwind format (the function tables, RtlVirtualUnwind in a prologue and
//! an epilogue). The rest is shared:
//!
//! - RtlPcToFileHeader and RtlLookupFunctionEntry find this image and the
//!   function's RUNTIME_FUNCTION.
//! - An indirect RUNTIME_FUNCTION (UnwindData with bit 0 set: the RVA of
//!   the entry whose unwind data it shares) is resolved: the lookup
//!   returns the entry it names (the C++ and GS handlers read the unwind
//!   information through DISPATCHER_CONTEXT.FunctionEntry; as Wine does).
//! - A growable function table (RtlAddGrowableFunctionTable,
//!   RtlGrowFunctionTable, RtlDeleteGrowableFunctionTable): its counted
//!   entries are found, relative to its range's base.
//! - RtlVirtualUnwind unwinds a frame from its body (restores the pushed
//!   registers, returns the handler, its data and the establisher frame),
//!   from inside its prologue (only what ran is undone, no handler) and
//!   from its epilogue (emulated, no handler).
//! - RtlCaptureContext records the caller's registers, Rip = its return
//!   address and Rsp = the caller's rsp after the return.
//! - RtlRaiseException dispatches to frame handlers, innermost first, with
//!   ExceptionAddress = its return address; a handler returning
//!   ExceptionContinueExecution resumes there.
//! - A handler returning ExceptionContinueSearch passes the exception on;
//!   an outer handler calls RtlUnwindEx to its own frame: the inner
//!   handler runs again with EXCEPTION_UNWINDING, the outer one with
//!   EXCEPTION_UNWINDING | EXCEPTION_TARGET_UNWIND, and execution resumes
//!   at the target with the return value (rax, x0) and the target frame's
//!   nonvolatile registers.
//! - The same for a hardware exception (an access violation).
//! - DISPATCHER_CONTEXT.ControlPcIsUnwound: TRUE for a frame whose
//!   ControlPc is a return address (reached by unwinding a call), in the
//!   search and the unwind (what __CxxFrameHandler4 uses to find the
//!   state of the call).
//! - Unwind consolidation (STATUS_UNWIND_CONSOLIDATE, how C++ catch
//!   blocks run): after the unwind the callback in ExceptionInformation[0]
//!   runs while the stack below the target is still intact, a walk from it
//!   reaches the target frame, and execution continues at the address it
//!   returns with the return value in rax (x0).
//!
//! The function tables' checks are each architecture's (x64's: an indirect
//! entry and a growable table; ARM64's: a table and a growable one, in its
//! 8-byte entries; ARM64 has no indirect entries).
//!
//! Prints `seh_test: ok` or exits with the number of the failing check.

#![no_std]
#![no_main]

use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use ntrt::*;

const EXCEPTION_UNWINDING: u32 = 2;
const EXCEPTION_TARGET_UNWIND: u32 = 0x20;
const EXCEPTION_CONTINUE: u32 = 0;
const EXCEPTION_CONTINUE_SEARCH: u32 = 1;
const UNW_FLAG_EHANDLER: u32 = 1;
const STATUS_ACCESS_VIOLATION: u32 = 0xc000_0005;
/// Raised and continued by the inner handler.
const CODE_CONTINUE: u32 = 0xe000_0001;
/// Raised and unwound by the outer handler.
const CODE_UNWIND: u32 = 0xe000_0002;
/// Unwound by the outer handler with a consolidation callback.
const CODE_CONSOLIDATE: u32 = 0xe000_0003;
const STATUS_UNWIND_CONSOLIDATE: u64 = 0x8000_0029;
/// The first nonvolatile register (rbx, x19) in seh_outer's body, and in
/// seh_inner's.
const OUTER_RBX: u64 = 0x1111_0000_0000_1111;
const INNER_RBX: u64 = 0x2222_0000_0000_2222;

#[cfg(target_arch = "x86_64")]
#[path = "x64.rs"]
mod arch;
#[cfg(target_arch = "aarch64")]
#[path = "arm64.rs"]
mod arch;
use arch::*;

// EXCEPTION_RECORD and DISPATCHER_CONTEXT (the 64-bit layouts, x64's and
// ARM64's alike); the CONTEXT's offsets are the architecture's
// (`arch::CTX_*`: the pc, sp, and three nonvolatile registers: rbx, rsi
// and r12 on x64, x19, x20 and x21 on ARM64).
const ER_CODE: usize = 0x00;
const ER_FLAGS: usize = 0x04;
const ER_ADDRESS: usize = 0x10;
const ER_NUMBER_PARAMETERS: usize = 0x18;
const ER_INFORMATION: usize = 0x20;
const RECORD_SIZE: usize = 0x98;
const DC_CONTROL_PC: usize = 0x00;
const DC_IMAGE_BASE: usize = 0x08;
const DC_CONTEXT_RECORD: usize = 0x28;
const DC_LANGUAGE_HANDLER: usize = 0x30;
const DC_HISTORY_TABLE: usize = 0x40;
/// ControlPcIsUnwound (BOOLEAN after ScopeIndex).
const DC_CONTROL_PC_IS_UNWOUND: usize = 0x4c;

static OUTER_RSP: AtomicU64 = AtomicU64::new(0);
static RESUME_RBX: AtomicU64 = AtomicU64::new(0);
static RESUME_RSI: AtomicU64 = AtomicU64::new(0);
static CAPTURE_RSP: AtomicU64 = AtomicU64::new(0);
static RTL_RAISE: AtomicUsize = AtomicUsize::new(0);
static RTL_CAPTURE: AtomicUsize = AtomicUsize::new(0);
static RTL_UNWIND_EX: AtomicUsize = AtomicUsize::new(0);
static RTL_LOOKUP: AtomicUsize = AtomicUsize::new(0);
static RTL_VIRTUAL_UNWIND: AtomicUsize = AtomicUsize::new(0);
/// What the consolidation callback saw: ran, the original record's code,
/// whether it ran below that record, the rsp and rbx a walk from it
/// reached at the target frame.
static CONSOLIDATED: [AtomicU64; 5] = [const { AtomicU64::new(0) }; 5];

/// What the handlers saw. Inner: calls, flags of each call (up to 2),
/// code, ExceptionAddress, parameter count, info[0], info[1],
/// Context.Rip. Outer: calls, flags of each call, code, establisher
/// frame, ControlPc, ImageBase, LanguageHandler.
static INNER: [AtomicU64; 9] = [const { AtomicU64::new(0) }; 9];
static OUTER: [AtomicU64; 8] = [const { AtomicU64::new(0) }; 8];
/// The value the outer handler unwinds with.
static UNWIND_VALUE: AtomicU64 = AtomicU64::new(0);
/// ControlPcIsUnwound as each handler saw it: inner search, inner unwind,
/// outer search, outer unwind (2: not called).
static UNWOUND: [AtomicU64; 4] = [const { AtomicU64::new(2) }; 4];

fn rd(p: *const u8, off: usize) -> u64 {
    // SAFETY: offsets inside the structures the system passes.
    unsafe { (p.add(off) as *const u64).read_unaligned() }
}

fn rd32(p: *const u8, off: usize) -> u32 {
    rd(p, off) as u32
}

fn wr(p: *mut u8, off: usize, v: u64) {
    // SAFETY: offsets inside buffers of the test's own.
    unsafe { (p.add(off) as *mut u64).write_unaligned(v) }
}

fn reset() {
    for a in INNER.iter().chain(OUTER.iter()) {
        a.store(0, Ordering::SeqCst);
    }
    for a in &UNWOUND {
        a.store(2, Ordering::SeqCst);
    }
}

extern "system" fn inner_handler(
    record: *mut u8,
    _frame: u64,
    context: *mut u8,
    dispatcher: *mut u8,
) -> u32 {
    let calls = INNER[0].fetch_add(1, Ordering::SeqCst);
    let flags = rd32(record, ER_FLAGS);
    let unwound = u64::from(rd32(dispatcher, DC_CONTROL_PC_IS_UNWOUND) & 0xff);
    UNWOUND[usize::from(flags & EXCEPTION_UNWINDING != 0)].store(unwound, Ordering::SeqCst);
    if calls < 2 {
        INNER[1 + calls as usize].store(u64::from(flags), Ordering::SeqCst);
    }
    if flags & EXCEPTION_UNWINDING != 0 {
        return EXCEPTION_CONTINUE_SEARCH;
    }
    let values = [
        u64::from(rd32(record, ER_CODE)),
        rd(record, ER_ADDRESS),
        u64::from(rd32(record, ER_NUMBER_PARAMETERS)),
        rd(record, ER_INFORMATION),
        rd(record, ER_INFORMATION + 8),
        rd(context, CTX_PC),
    ];
    for (slot, v) in INNER[3..].iter().zip(values) {
        slot.store(v, Ordering::SeqCst);
    }
    if rd32(record, ER_CODE) == CODE_CONTINUE {
        EXCEPTION_CONTINUE
    } else {
        EXCEPTION_CONTINUE_SEARCH
    }
}

type UnwindEx = extern "system" fn(u64, usize, *mut u8, u64, *mut u8, *mut u8);

extern "system" fn outer_handler(
    record: *mut u8,
    frame: u64,
    _context: *mut u8,
    dispatcher: *mut u8,
) -> u32 {
    let calls = OUTER[0].fetch_add(1, Ordering::SeqCst);
    let flags = rd32(record, ER_FLAGS);
    let unwound = u64::from(rd32(dispatcher, DC_CONTROL_PC_IS_UNWOUND) & 0xff);
    UNWOUND[2 + usize::from(flags & EXCEPTION_UNWINDING != 0)].store(unwound, Ordering::SeqCst);
    if calls < 2 {
        OUTER[1 + calls as usize].store(u64::from(flags), Ordering::SeqCst);
    }
    if flags & EXCEPTION_UNWINDING != 0 {
        return EXCEPTION_CONTINUE_SEARCH;
    }
    let values = [
        u64::from(rd32(record, ER_CODE)),
        frame,
        rd(dispatcher, DC_CONTROL_PC),
        rd(dispatcher, DC_IMAGE_BASE),
        rd(dispatcher, DC_LANGUAGE_HANDLER),
    ];
    for (slot, v) in OUTER[3..].iter().zip(values) {
        slot.store(v, Ordering::SeqCst);
    }
    // SAFETY: the documented signature of ntdll's RtlUnwindEx.
    let unwind: UnwindEx = unsafe { core::mem::transmute(RTL_UNWIND_EX.load(Ordering::SeqCst)) };
    if rd32(record, ER_CODE) == CODE_CONSOLIDATE {
        // Unwind consolidation (how C++ catch blocks run): after the
        // unwind, the callback runs and says where to continue.
        let mut consolidate = [0u8; RECORD_SIZE];
        let cr = consolidate.as_mut_ptr();
        wr(cr, ER_CODE, STATUS_UNWIND_CONSOLIDATE);
        wr(cr, ER_NUMBER_PARAMETERS, 2);
        wr(cr, ER_INFORMATION, consolidate_callback as *const () as u64);
        wr(cr, ER_INFORMATION + 8, record as u64);
        unwind(
            frame,
            0,
            cr,
            UNWIND_VALUE.load(Ordering::SeqCst),
            rd(dispatcher, DC_CONTEXT_RECORD) as *mut u8,
            rd(dispatcher, DC_HISTORY_TABLE) as *mut u8,
        );
    }
    unwind(
        frame,
        seh_outer_resume as *const () as usize,
        record,
        UNWIND_VALUE.load(Ordering::SeqCst),
        rd(dispatcher, DC_CONTEXT_RECORD) as *mut u8,
        rd(dispatcher, DC_HISTORY_TABLE) as *mut u8,
    );
    EXCEPTION_CONTINUE_SEARCH // not reached
}

/// The consolidation callback: runs after the unwind to seh_outer, while
/// the original exception record (deeper on the stack) is still intact;
/// a walk from it reaches the target frame. Continues at seh_outer_resume.
extern "system" fn consolidate_callback(record: *mut u8) -> u64 {
    let original = rd(record, ER_INFORMATION + 8) as *const u8;
    let marker = 0u8;
    CONSOLIDATED[0].store(1, Ordering::SeqCst);
    CONSOLIDATED[1].store(rd32(original, ER_CODE).into(), Ordering::SeqCst);
    let below = (&raw const marker as usize) < original as usize;
    CONSOLIDATED[2].store(below.into(), Ordering::SeqCst);
    // SAFETY: the documented signatures of these ntdll exports.
    let (capture, lookup, unwind) = unsafe {
        (
            core::mem::transmute::<usize, extern "system" fn(*mut u8)>(
                RTL_CAPTURE.load(Ordering::SeqCst),
            ),
            core::mem::transmute::<usize, LookupFunctionEntry>(RTL_LOOKUP.load(Ordering::SeqCst)),
            core::mem::transmute::<usize, VirtualUnwind>(RTL_VIRTUAL_UNWIND.load(Ordering::SeqCst)),
        )
    };
    let mut ctx = Context([0; CONTEXT_SIZE]);
    let c = ctx.0.as_mut_ptr();
    capture(c);
    // seh_outer's body: its stack pointer is the establisher frame less
    // what its prologue allocated after it (the architecture's).
    let target = OUTER_RSP.load(Ordering::SeqCst) - OUTER_FRAME_ABOVE_BODY;
    for _ in 0..8 {
        if rd(c, CTX_SP) >= target {
            break;
        }
        let pc = rd(c, CTX_PC);
        let mut base = 0u64;
        let entry = lookup(pc, &mut base, core::ptr::null_mut());
        if entry.is_null() {
            leaf_step(c);
            continue;
        }
        let (mut data, mut frame) = (0u64, 0u64);
        unwind(0, base, pc, entry, c, &mut data, &mut frame, core::ptr::null_mut());
    }
    CONSOLIDATED[3].store(rd(c, CTX_SP) + OUTER_FRAME_ABOVE_BODY, Ordering::SeqCst);
    CONSOLIDATED[4].store(rd(c, CTX_NV1), Ordering::SeqCst);
    seh_outer_resume as *const () as u64
}

/// Raises `code` with parameters 7 and 9 through seh_raise.
extern "system" fn raise_it(code: u64) -> u64 {
    let mut record = [0u8; RECORD_SIZE];
    let r = record.as_mut_ptr();
    wr(r, ER_CODE, code);
    wr(r, ER_NUMBER_PARAMETERS, 2);
    wr(r, ER_INFORMATION, 7);
    wr(r, ER_INFORMATION + 8, 9);
    // SAFETY: seh_raise passes the record to RtlRaiseException.
    unsafe { seh_raise(r) }
}

/// Reads address 8: an access violation.
extern "system" fn fault_it(_: u64) -> u64 {
    // SAFETY: the load faults; the outer handler unwinds past this frame.
    unsafe { core::ptr::read_volatile(core::ptr::without_provenance::<u64>(8)) }
}

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
        print("seh_test: check ");
        write_file(stdout(), &hex32(self.0));
        print(" got ");
        write_file(stdout(), &hex32((got >> 32) as u32));
        write_file(stdout(), &hex32(got as u32));
        check_line(core::panic::Location::caller().line());
        print("\r\n");
        Err(self.0)
    }
}

fn inner(i: usize) -> u64 {
    INNER[i].load(Ordering::SeqCst)
}

fn outer(i: usize) -> u64 {
    OUTER[i].load(Ordering::SeqCst)
}

type PcToFileHeader = extern "system" fn(usize, *mut usize) -> usize;
type LookupFunctionEntry = extern "system" fn(u64, *mut u64, *mut u8) -> *const Entry;
type VirtualUnwind =
    extern "system" fn(u32, u64, u64, *const Entry, *mut u8, *mut u64, *mut u64, *mut u8) -> u64;

/// A CONTEXT, 16-byte aligned.
#[repr(C, align(16))]
pub(crate) struct Context([u8; CONTEXT_SIZE]);

/// Unwinds seh_outer at `pc` with the stack pointer `stack`, the other
/// registers 0 but for `lr` (ARM64's link register, the return address
/// before the prologue saved it); returns (handler, handler data,
/// establisher frame) and leaves the result in `ctx`.
fn virtual_unwind(
    ntdll: &Fns,
    pc: u64,
    stack: *mut u64,
    lr: u64,
    ctx: &mut Context,
) -> Result<(u64, u64, u64), u32> {
    let mut base = 0u64;
    let entry = (ntdll.lookup)(pc, &mut base, core::ptr::null_mut());
    if entry.is_null() {
        return Err(0);
    }
    ctx.0 = [0; CONTEXT_SIZE];
    let c = ctx.0.as_mut_ptr();
    wr(c, CTX_FLAGS, CTX_FULL);
    wr(c, CTX_PC, pc);
    wr(c, CTX_SP, stack as u64);
    set_lr(c, lr);
    let (mut data, mut frame) = (0u64, 0u64);
    let handler = (ntdll.unwind)(
        UNW_FLAG_EHANDLER,
        base,
        pc,
        entry,
        c,
        &mut data,
        &mut frame,
        core::ptr::null_mut(),
    );
    Ok((handler, data, frame))
}

struct Fns {
    lookup: LookupFunctionEntry,
    unwind: VirtualUnwind,
}

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    let Some(ntdll) = find_ntdll() else { return c.ok(false, 0) };
    let names: [&[u8]; 6] = [
        b"RtlPcToFileHeader",
        b"RtlLookupFunctionEntry",
        b"RtlVirtualUnwind",
        b"RtlCaptureContext",
        b"RtlRaiseException",
        b"RtlUnwindEx",
    ];
    let mut f = [0usize; 6];
    for (slot, name) in f.iter_mut().zip(names) {
        *slot = find_export(ntdll, name).unwrap_or(0);
        c.ok(*slot != 0, 0)?;
    }
    RTL_CAPTURE.store(f[3], Ordering::SeqCst);
    RTL_RAISE.store(f[4], Ordering::SeqCst);
    RTL_UNWIND_EX.store(f[5], Ordering::SeqCst);
    RTL_LOOKUP.store(f[1], Ordering::SeqCst);
    RTL_VIRTUAL_UNWIND.store(f[2], Ordering::SeqCst);
    // SAFETY: the documented signatures of these ntdll exports.
    let (pc_to_header, fns) = unsafe {
        (
            core::mem::transmute::<usize, PcToFileHeader>(f[0]),
            Fns {
                lookup: core::mem::transmute::<usize, LookupFunctionEntry>(f[1]),
                unwind: core::mem::transmute::<usize, VirtualUnwind>(f[2]),
            },
        )
    };
    let image = image_base();
    let outer_fn = seh_outer as *const () as usize;

    // RtlPcToFileHeader: this image for its code, null outside images.
    let mut base = 0usize;
    c.eq(pc_to_header(outer_fn + 3, &mut base) as u64, image as u64)?;
    c.eq(base as u64, image as u64)?;
    c.eq(pc_to_header(16, &mut base) as u64, 0)?;
    c.eq(base as u64, 0)?;

    // RtlLookupFunctionEntry: seh_outer's entry, starting at its address.
    let mut base64 = 0u64;
    let entry = (fns.lookup)(outer_fn as u64 + 3, &mut base64, core::ptr::null_mut());
    c.ok(!entry.is_null(), 0)?;
    c.eq(base64, image as u64)?;
    // SAFETY: a RUNTIME_FUNCTION in this image's .pdata.
    let begin = unsafe { (*entry)[0] };
    c.eq(u64::from(begin), (outer_fn - image) as u64)?;

    // Function tables registered at run time (the architecture's format).
    table_checks(&mut c, ntdll, &fns)?;

    // RtlVirtualUnwind from seh_outer's body, its prologue and its
    // epilogue (the architecture's frame).
    unwind_checks(&mut c, &fns)?;

    // RtlCaptureContext: the caller's registers, its return address and
    // its stack pointer after the return.
    let mut ctx = Context([0; CONTEXT_SIZE]);
    // SAFETY: seh_capture fills the CONTEXT.
    unsafe { seh_capture(ctx.0.as_mut_ptr()) };
    let cp = ctx.0.as_ptr();
    c.eq(rd(cp, CTX_PC), seh_capture_ret as *const () as u64)?;
    c.eq(rd(cp, CTX_SP), CAPTURE_RSP.load(Ordering::SeqCst))?;
    c.eq(rd(cp, CTX_NV1), OUTER_RBX)?;
    c.eq(rd(cp, CTX_NV3), INNER_RBX)?;
    c.eq(rd(cp, CTX_FLAGS) & CTX_FULL, CTX_FULL)?;
    capture_checks(&mut c, cp)?;

    // RtlRaiseException, continued by the inner handler: seh_raise returns
    // 5 through raise_it, seh_inner and seh_outer.
    reset();
    // SAFETY: seh_outer calls raise_it(CODE_CONTINUE) under its handlers.
    let r = unsafe { seh_outer(raise_it, CODE_CONTINUE.into()) };
    c.eq(r, 5)?;
    c.eq(inner(0), 1)?;
    // EXCEPTION_SOFTWARE_ORIGINATE (0x80): the 64-bit RtlRaiseException
    // marks the record so itself (Windows 25H2; the 32-bit one does not,
    // wow64_seh_test).
    c.eq(inner(1), 0x80)?; // flags
    c.eq(inner(3), CODE_CONTINUE.into())?;
    c.eq(inner(4), seh_raise_ret as *const () as u64)?; // ExceptionAddress
    c.eq(inner(5), 2)?;
    c.eq(inner(6), 7)?;
    c.eq(inner(7), 9)?;
    c.eq(inner(8), seh_raise_ret as *const () as u64)?; // Context.Rip
    c.eq(outer(0), 0)?;

    // Passed on by the inner handler; the outer one unwinds to
    // seh_outer_resume with 42.
    reset();
    UNWIND_VALUE.store(42, Ordering::SeqCst);
    RESUME_RBX.store(0, Ordering::SeqCst);
    // SAFETY: as above; the outer handler unwinds to seh_outer_resume.
    let r = unsafe { seh_outer(raise_it, CODE_UNWIND.into()) };
    c.eq(r, 42)?;
    c.eq(inner(0), 2)?;
    c.eq(inner(1), 0x80)?; // dispatch: EXCEPTION_SOFTWARE_ORIGINATE, as above
    // The unwind does not keep the record's 0x80: the handlers see its
    // own flags only (Windows 11 25H2 measured 0x02 here).
    c.eq(inner(2), EXCEPTION_UNWINDING.into())?;
    c.eq(inner(3), CODE_UNWIND.into())?;
    c.eq(outer(0), 2)?;
    c.eq(outer(1), 0x80)?;
    c.eq(outer(2), (EXCEPTION_UNWINDING | EXCEPTION_TARGET_UNWIND).into())?;
    c.eq(outer(3), CODE_UNWIND.into())?;
    c.eq(outer(4), OUTER_RSP.load(Ordering::SeqCst))?; // establisher frame
    c.eq(outer(5), seh_outer_resume as *const () as u64)?; // ControlPc: the return address
    c.eq(outer(6), image as u64)?;
    c.eq(outer(7), outer_handler as *const () as u64)?; // LanguageHandler
    c.eq(RESUME_RBX.load(Ordering::SeqCst), OUTER_RBX)?;
    c.eq(RESUME_RSI.load(Ordering::SeqCst), OUTER_RBX)?;
    // Both frames' ControlPc is a return address (they called the raising
    // function), in the search and the unwind: ControlPcIsUnwound.
    for u in &UNWOUND {
        c.eq(u.load(Ordering::SeqCst), 1)?;
    }

    // A hardware exception: the access violation in fault_it, passed on by
    // the inner handler and unwound by the outer one with 43.
    reset();
    UNWIND_VALUE.store(43, Ordering::SeqCst);
    RESUME_RBX.store(0, Ordering::SeqCst);
    // SAFETY: as above.
    let r = unsafe { seh_outer(fault_it, 0) };
    c.eq(r, 43)?;
    c.eq(inner(0), 2)?;
    c.eq(inner(3), STATUS_ACCESS_VIOLATION.into())?;
    c.eq(inner(5), 2)?;
    c.eq(inner(6), 0)?; // read
    c.eq(inner(7), 8)?; // address
    c.eq(outer(0), 2)?;
    c.eq(outer(2), (EXCEPTION_UNWINDING | EXCEPTION_TARGET_UNWIND).into())?;
    c.eq(outer(3), STATUS_ACCESS_VIOLATION.into())?;
    c.eq(RESUME_RBX.load(Ordering::SeqCst), OUTER_RBX)?;
    // The faulting function has no handler; the frames above it were
    // reached by unwinding calls.
    for u in &UNWOUND {
        c.eq(u.load(Ordering::SeqCst), 1)?;
    }

    // Unwind consolidation: the callback runs, then seh_outer continues
    // where it said, with rax = 44 and its own nonvolatile registers.
    reset();
    UNWIND_VALUE.store(44, Ordering::SeqCst);
    RESUME_RBX.store(0, Ordering::SeqCst);
    // SAFETY: as above.
    let r = unsafe { seh_outer(raise_it, CODE_CONSOLIDATE.into()) };
    c.eq(r, 44)?;
    let seen = |i: usize| CONSOLIDATED[i].load(Ordering::SeqCst);
    c.eq(seen(0), 1)?;
    c.eq(seen(1), CODE_CONSOLIDATE.into())?;
    c.eq(seen(2), 1)?;
    c.eq(seen(3), OUTER_RSP.load(Ordering::SeqCst))?;
    c.eq(seen(4), OUTER_RBX)?;
    c.eq(outer(2), (EXCEPTION_UNWINDING | EXCEPTION_TARGET_UNWIND).into())?;
    c.eq(RESUME_RBX.load(Ordering::SeqCst), OUTER_RBX)?;
    Ok(())
}

fn main() -> u32 {
    match run() {
        Ok(()) => {
            print("seh_test: ok\r\n");
            0
        }
        Err(id) => id,
    }
}

entry!(main);

/// ` (line L)`: the source line of the failing check, from
/// `#[track_caller]` on Checks' methods (so a report names the line
/// without anyone counting checks).
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
