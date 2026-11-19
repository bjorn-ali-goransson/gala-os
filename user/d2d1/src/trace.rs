//! The DLL half of `trace_from=` (docs/OBSERVABILITY.md, "Call
//! tracing"; the kernel's half is `kernel/src/nt/tracecall.rs`).
//!
//! A COM call into this DLL is an indirect call through a vtable in the
//! program's own address space, so the kernel never sees one. The
//! generated wrapper around each vtable slot (`cargo xtask gen-trace`)
//! does, and this is what it reports through.
//!
//! What it costs when nothing is traced is the point. A wrapper's
//! one-shot `reached!` record already pays one relaxed load a call; this
//! adds a second, and nothing else -- no string compare, no system call,
//! no formatting. The compare against the `trace_from=` value happens
//! once per slot, folded into the one-shot the wrapper was going to pay
//! anyway, so a drawing loop that calls FillRectangle a thousand times
//! compares no names at all.
//!
//! Once armed, each call is one system call carrying the method's name
//! and its arguments as words, and a second carrying what the method
//! answered, for a method that returns anything at all. The kernel
//! answers whether to keep going, so a spent budget stops the trace
//! where it is rather than letting a run pay for lines nobody will read.

use core::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering};

/// As long as the ledger's names, which is what a `trace_from=` value
/// names (`record_kind`, and `tracecall::NAME_LEN` in the kernel).
const NAME_LEN: usize = 96;

/// The most argument words one call carries (`tracecall::ARGS`). A
/// method with more is recorded with its first [`ARGS`]; none of the
/// methods this DLL wraps has more.
const ARGS: usize = 8;

/// GalaTrace's operations.
const OP_ASK: u64 = 0;
const OP_RECORD: u64 = 1;
const OP_ANSWER: u64 = 2;

/// Whether the armed method has been reached, and so whether every call
/// is being recorded. One relaxed load a call; nothing else is paid
/// until this is true.
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
                        return true;
                    }
                    ASKED.store(NONE, Ordering::Release);
                    return false;
                }
            }
        }
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

/// `d2d1.dll!<what>` in `out`, as the ledger spells a name: the value
/// names the method the way the ledger's rows do, so one is read off the
/// other.
fn full_name(what: &[u8], out: &mut [u8; NAME_LEN]) -> usize {
    let mut n = 0;
    for &b in b"d2d1.dll!".iter().chain(what) {
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

impl Arg for crate::Point {
    fn word(&self) -> u64 {
        u64::from(self.x.to_bits()) | (u64::from(self.y.to_bits()) << 32)
    }
}

impl Arg for crate::SizeU {
    fn word(&self) -> u64 {
        u64::from(self.width) | (u64::from(self.height) << 32)
    }
}
