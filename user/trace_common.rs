//! The DLL half of `trace_from=` (docs/OBSERVABILITY.md, "Call
//! tracing"; the kernel's half is `kernel/src/nt/tracecall.rs`), and the
//! records a generated vtable wrapper leaves.
//!
//! One file, `user/trace_common.rs`, shared by every DLL whose vtable
//! slots `cargo xtask gen-trace` wraps (`DLLS` in
//! `xtask/src/gentrace.rs`): each declares it as its own `trace` module
//! with `#[path]`, as `gpu_common.rs` is included by the GPU stubs. A
//! DLL that declares it brings three things of its own -- `DLL_NAME`,
//! its name as the ledger spells it (`b"d2d1.dll!"`), `record_kind`,
//! which writes one ledger row, and `LEDGER_REACHED` among its ledger
//! kinds -- and may add an [`Arg`] impl for a type of its own that one
//! of its methods takes by value (d2d1's `Point`, `SizeU`).
//!
//! A COM call from a program into one of gala's DLLs is an indirect call
//! through a vtable in the program's own address space, so the kernel
//! never sees one. The generated wrapper around each slot does, and this
//! is what it reports through.
//!
//! What it costs when nothing is traced is the point. A wrapper's
//! one-shot [`reached!`] record already pays one relaxed load a call;
//! this adds a second, and nothing else -- no string compare, no system
//! call, no formatting. The compare against the `trace_from=` value
//! happens once per slot, folded into the one-shot the wrapper was going
//! to pay anyway, so a drawing loop that calls FillRectangle a thousand
//! times compares no names at all.
//!
//! The flag that second load reads is this DLL's own static, and the
//! arming is the process's: a trace armed at a method of one of gala's
//! DLLs records the calls a program then makes into the others. What
//! lets two DLLs' statics reach each other is ntdll, the one image a
//! process has exactly one of -- each offers its flag there on its
//! first wrapped call, and the DLL that arms sets them all
//! (`GalaTraceJoin`, `user/testntdll/src/trace.rs`). One call per DLL
//! per process in a run that traces at all, and nothing per call.

use core::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering};

/// Records `<dll>!<what>` the first time a program reaches the call
/// site: a call that works leaves nothing else behind, so the ledger
/// would go quiet exactly where a program stops in the middle of a path
/// this DLL answers. Once per site -- the record is the path a program
/// took through this DLL, not a count, and a drawing loop pays one
/// relaxed atomic a call (OBSERVABILITY "Missing-features ledger").
#[macro_export]
macro_rules! reached {
    ($what:expr) => {{
        static SEEN: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
        // A relaxed load once it has been recorded, not a locked
        // exchange: the call is on a drawing loop.
        if !SEEN.load(core::sync::atomic::Ordering::Relaxed)
            && !SEEN.swap(true, core::sync::atomic::Ordering::Relaxed)
        {
            $crate::trace::reached_now($what);
        }
    }};
}

/// `reached!`'s record, and -- once `trace_from=` has armed this method
/// -- every call after it. A generated vtable wrapper uses this rather
/// than `reached!` so that an investigation can read a *sequence*: the
/// one-shot record says a method was reached once, which leaves the
/// calls between the last thing that worked and a failure invisible by
/// construction.
///
/// What it costs a program that is not being traced is two relaxed
/// loads a call. The compare against the `trace_from=` value is folded
/// into the one-shot, so it is paid once per slot and never on a
/// drawing loop (OBSERVABILITY "Call tracing").
#[macro_export]
macro_rules! traced {
    ($what:expr, [$($arg:expr),* $(,)?]) => {{
        static SEEN: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
        if !SEEN.load(core::sync::atomic::Ordering::Relaxed)
            && !SEEN.swap(true, core::sync::atomic::Ordering::Relaxed)
        {
            $crate::trace::reached_now($what);
            $crate::trace::arm_if_named($what);
        }
        if $crate::trace::on() {
            $crate::trace::record($what, &[$($crate::trace::Arg::word(&$arg)),*]);
        }
    }};
}

/// `traced!` for a call that already records a ledger row of its own
/// every time it is made -- a recorder stub, which answers E_NOTIMPL
/// under its own name (d3d10_1's `recorders!`). The one-shot arms the
/// trace as `traced!`'s does and leaves no `reached` row, because the
/// stub's own row already says the method was reached; what the trace
/// adds is the order, which a row counted `x12` cannot give. A slot
/// that records itself is traced by having been written that way, so
/// the generator leaves it alone (`// gen-trace records <module>`).
#[macro_export]
macro_rules! traced_recorded {
    ($what:expr, [$($arg:expr),* $(,)?]) => {{
        static SEEN: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
        if !SEEN.load(core::sync::atomic::Ordering::Relaxed)
            && !SEEN.swap(true, core::sync::atomic::Ordering::Relaxed)
        {
            $crate::trace::arm_if_named($what);
        }
        if $crate::trace::on() {
            $crate::trace::record($what, &[$($crate::trace::Arg::word(&$arg)),*]);
        }
    }};
}

/// What a traced call answered, recorded right after the call itself
/// (`trace_from=`). The generator puts this in the wrapper of every slot
/// whose method returns anything; a method that returns nothing has no
/// answer to record. An answer costs what a call costs and comes out of
/// the same budget, so a trace can end between a call and its answer but
/// can never show an answer whose call it did not record.
#[macro_export]
macro_rules! answered_call {
    ($what:expr, $answer:expr) => {{
        if $crate::trace::on() {
            $crate::trace::record_answer($what, $crate::trace::Arg::word(&$answer));
        }
    }};
}

/// `reached!`'s record, in the including DLL's own ledger. Call it
/// through the macro, which keeps it to the first time.
pub fn reached_now(what: &[u8]) {
    crate::record_kind(what, crate::LEDGER_REACHED, 0);
}

/// As long as the ledger's names, which is what a `trace_from=` value
/// names (`record_kind`, and `tracecall::NAME_LEN` in the kernel).
const NAME_LEN: usize = 96;

/// The most argument words one call carries (`tracecall::ARGS`). A
/// method with more is recorded with its first [`ARGS`]; none of the
/// methods these DLLs wrap has more.
const ARGS: usize = 8;

/// GalaTrace's operations.
const OP_ASK: u64 = 0;
const OP_RECORD: u64 = 1;
const OP_ANSWER: u64 = 2;

/// Whether calls are being recorded. Set when the armed method is
/// first reached here, and by ntdll when another of gala's DLLs reaches
/// its own (`GalaTraceJoin`, `user/testntdll/src/trace.rs`): the arming
/// is the process's, not one DLL's. One relaxed load a call; nothing
/// else is paid until this is true.
static ON: AtomicBool = AtomicBool::new(false);

/// Whether the `trace_from=` value has been asked for: 0 not yet, 1 the
/// asking is under way, 2 a value is in [`VALUE`], 3 nothing is traced.
static ASKED: AtomicU8 = AtomicU8::new(0);
const UNASKED: u8 = 0;
const ASKING: u8 = 1;
const HAVE: u8 = 2;
const NONE: u8 = 3;

/// The `trace_from=` value, written once by whichever thread asks and
/// read by every thread after. Bytes rather than a buffer behind a lock:
/// this DLL has no lock to take and the write happens once.
static VALUE: [AtomicU8; NAME_LEN] = [const { AtomicU8::new(0) }; NAME_LEN];
static VALUE_LEN: AtomicUsize = AtomicUsize::new(0);

#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "system" {
    fn GalaTrace(op: u64, a: u64, b: u64, c: u64) -> u32;
    fn GalaTraceJoin(flag: *mut u8) -> u32;
    fn GalaTraceArm();
}

/// Whether calls are being recorded (one relaxed load).
pub fn on() -> bool {
    ON.load(Ordering::Relaxed)
}

/// Arms the trace if `what` is the method `trace_from=` named. Called
/// from a slot's one-shot, so once per slot: the string compare is paid
/// as many times as this DLL has slots, not as many times as a program
/// makes calls.
pub fn arm_if_named(what: &[u8]) {
    if ON.load(Ordering::Relaxed) || !asked() {
        return;
    }
    let mut full = [0u8; NAME_LEN];
    let len = full_name(what, &mut full);
    let want = VALUE_LEN.load(Ordering::Acquire);
    if len != want {
        return;
    }
    if (0..len).any(|i| VALUE[i].load(Ordering::Relaxed) != full[i]) {
        return;
    }
    ON.store(true, Ordering::Release);
    // And every other traced DLL in this process, whose calls are what
    // the trace was armed to read: the armed method is where recording
    // starts, and what a program does after it is as likely to be
    // another of gala's DLLs as this one.
    // SAFETY: ntdll's own function, which reads nothing of ours.
    unsafe { GalaTraceArm() };
}

/// Records one call. Only reached once [`on`] is true.
pub fn record(what: &[u8], args: &[u64]) {
    let mut name = [0u8; NAME_LEN];
    let len = full_name(what, &mut name);
    let count = args.len().min(ARGS);
    let mut block = [0u64; 1 + ARGS];
    block[0] = count as u64;
    block[1..1 + count].copy_from_slice(&args[..count]);
    // SAFETY: two buffers this frame owns, of the lengths given; the
    // service reads them and writes nothing.
    let status =
        unsafe { GalaTrace(OP_RECORD, name.as_ptr() as u64, len as u64, block.as_ptr() as u64) };
    // Anything but success is the kernel saying to stop: the budget is
    // spent, or this process is not one `events_pid=` asked for. Either
    // way there is no point paying a system call a call from here on.
    if status != 0 {
        ON.store(false, Ordering::Release);
    }
}

/// Records what one call answered. Only reached once [`on`] is true,
/// and only from the wrapper of a method that returns something: a
/// method that returns nothing has no answer to record. The answer is
/// passed by value rather than through a buffer -- it is one word, and a
/// word is cheaper to hand over than a pointer to read back.
pub fn record_answer(what: &[u8], answer: u64) {
    let mut name = [0u8; NAME_LEN];
    let len = full_name(what, &mut name);
    // SAFETY: a buffer this frame owns, of the length given; the
    // service reads it and writes nothing.
    let status = unsafe { GalaTrace(OP_ANSWER, name.as_ptr() as u64, len as u64, answer) };
    // As for a call: anything but success is the kernel saying to stop.
    if status != 0 {
        ON.store(false, Ordering::Release);
    }
}

/// Whether a `trace_from=` value is in [`VALUE`], asking the kernel for
/// it the first time. At most one thread asks; any other waits for it,
/// which happens once in a process's life.
fn asked() -> bool {
    loop {
        match ASKED.load(Ordering::Acquire) {
            HAVE => return true,
            NONE => return false,
            ASKING => core::hint::spin_loop(),
            _ => {
                if ASKED
                    .compare_exchange(UNASKED, ASKING, Ordering::Acquire, Ordering::Acquire)
                    .is_ok()
                {
                    let len = ask();
                    if len != 0 {
                        VALUE_LEN.store(len, Ordering::Release);
                        ASKED.store(HAVE, Ordering::Release);
                        join();
                        return true;
                    }
                    ASKED.store(NONE, Ordering::Release);
                    return false;
                }
            }
        }
    }
}

/// Offers this DLL's flag to the other traced DLLs in this process, and
/// takes from ntdll whether one of them has already armed. Once per
/// process, beside the one ask for the value: a DLL that loads after
/// the arming has no first call of its own left to arm it, and would
/// otherwise record nothing at all.
fn join() {
    // SAFETY: the address of this DLL's own static, which lives as long
    // as the DLL; ntdll stores a byte through it and reads nothing.
    let armed = unsafe { GalaTraceJoin((&raw const ON).cast::<u8>().cast_mut()) };
    if armed != 0 {
        ON.store(true, Ordering::Release);
    }
}

/// Reads the `trace_from=` value into [`VALUE`]; its length, or 0.
fn ask() -> usize {
    let mut buffer = [0u8; NAME_LEN];
    let mut written = 0u64;
    // SAFETY: a buffer and a word this frame owns, of the sizes given;
    // the service writes no more than the length it is told.
    let status = unsafe {
        GalaTrace(OP_ASK, buffer.as_mut_ptr() as u64, buffer.len() as u64, &raw mut written as u64)
    };
    if status != 0 {
        return 0;
    }
    let len = (written as usize).min(buffer.len());
    for (slot, &byte) in VALUE.iter().zip(&buffer[..len]) {
        slot.store(byte, Ordering::Relaxed);
    }
    len
}

/// `<dll>!<what>` in `out`, as the ledger spells a name: the value names
/// the method the way the ledger's rows do, so one is read off the
/// other. The prefix is the including DLL's own (`crate::DLL_NAME`).
fn full_name(what: &[u8], out: &mut [u8; NAME_LEN]) -> usize {
    let mut n = 0;
    for &b in crate::DLL_NAME.iter().chain(what) {
        if n == out.len() {
            break;
        }
        out[n] = b;
        n += 1;
    }
    n
}

/// One argument of a traced call, as the word the event carries. A
/// pointer is its address, a number is itself, and a float is its bits
/// (the event is JSON, which has no room for a float's exact value and
/// every room for the bits it was handed).
pub trait Arg {
    fn word(&self) -> u64;
}

impl<T> Arg for *const T {
    fn word(&self) -> u64 {
        *self as u64
    }
}

impl<T> Arg for *mut T {
    fn word(&self) -> u64 {
        *self as u64
    }
}

impl Arg for u32 {
    fn word(&self) -> u64 {
        u64::from(*self)
    }
}

impl Arg for u64 {
    fn word(&self) -> u64 {
        *self
    }
}

impl Arg for usize {
    fn word(&self) -> u64 {
        *self as u64
    }
}

impl Arg for i32 {
    fn word(&self) -> u64 {
        u64::from(*self as u32)
    }
}

impl Arg for f32 {
    fn word(&self) -> u64 {
        u64::from(self.to_bits())
    }
}

impl Arg for bool {
    fn word(&self) -> u64 {
        u64::from(*self)
    }
}
