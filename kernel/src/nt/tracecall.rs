//! `trace_from=<dll>!<Interface>::<Method>[:<n>]`: every call a program
//! makes into a DLL gala wrote, from the moment it first reaches the
//! method named (docs/OBSERVABILITY.md, "Call tracing").
//!
//! A COM call from a program into one of gala's DLLs is an indirect call
//! through a vtable in the program's own address space, so the kernel
//! never sees it. The only place that can record one is the DLL, and the
//! generated wrapper around every vtable slot (`cargo xtask gen-trace`)
//! is where it is recorded. The one-shot record each wrapper already
//! leaves is free -- a relaxed load -- but a record of *every* call costs
//! a system call per call, which a drawing loop cannot pay. So the
//! wrappers stay quiet until the method named here is first reached, and
//! then record every call until a budget runs out.
//!
//! The arming is the DLL's. It asks for this value once as it loads and
//! compares it against a slot's name on that slot's first call, which is
//! the one-shot it was going to pay anyway: there is no per-call string
//! compare. The kernel's half is this module -- it hands the value out,
//! writes the events and holds the budget -- and when the budget is
//! spent it says so in the answer, so the DLL stops paying for calls
//! nobody will read.
//!
//! What it answers that the one-shot records cannot: a *repeat*. A
//! program that reaches a method twice leaves one record, so the
//! sequence of a window -- the calls between the last thing that worked
//! and the failure -- is invisible by construction. This makes that
//! window readable without making a long run unaffordable.

use core::fmt;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use crate::events::{self, Category, Str};
use crate::sync::IrqMutex;

/// How many calls one run records unless `trace_from=<...>:<n>` asks for
/// another number. A call is a line of the event stream, so this is the
/// same shape of budget `stack_at=` has: enough to cover the window an
/// investigation is about, far short of what a drawing loop writes.
const BUDGET: u32 = 4096;

/// The most calls `:<n>` may ask for. The stream leaves the kernel a
/// line at a time; a budget past this buys nothing a snapshot would not
/// buy better, and a run that asked for it would be reading noise.
const MAX_BUDGET: u32 = 1 << 20;

/// The longest `trace_from=` value kept, which also bounds what the DLL
/// compares against. The ledger's own names are this long
/// (`ledger::USER_NAME_LEN`), and the value names one of them.
pub const NAME_LEN: usize = 96;

/// The most argument words one call records. Every COM method gala
/// wraps passes fewer than this; a method with more is recorded with its
/// first [`ARGS`] and the rest dropped, which OBSERVABILITY names.
pub const ARGS: usize = 8;

/// The `trace_from=` value, as given.
static FROM: IrqMutex<([u8; NAME_LEN], usize)> = IrqMutex::new(([0; NAME_LEN], 0));

/// Whether `trace_from=` was given at all, so the system call can answer
/// "nothing to trace" with one relaxed load instead of taking the lock.
static WANTED: AtomicBool = AtomicBool::new(false);

/// Calls left in the budget.
static LEFT: AtomicU32 = AtomicU32::new(0);

/// Calls recorded.
static RECORDED: AtomicU64 = AtomicU64::new(0);

/// Answers recorded, which is as many of those calls as were made to a
/// method that returns anything at all.
static ANSWERED: AtomicU64 = AtomicU64::new(0);

/// `trace_from=<dll>!<Interface>::<Method>[:<n>]`. A value that is not
/// one is refused outright rather than cut to the default: a budget that
/// silently differed from the one asked for reads afterwards as the
/// program having stopped calling, which is the wrong conclusion.
pub fn set_from(text: &str) {
    let Ok((name, budget)) = split_budget(text) else {
        warn!(
            "trace: trace_from={text} ends in a call count that is not \
             a number from 1 to {MAX_BUDGET}: nothing is traced"
        );
        return;
    };
    if !name.contains('!') {
        warn!(
            "trace: trace_from={text} names no DLL: it reads \
             <dll>!<Interface>::<Method>, such as \
             d2d1.dll!ID2D1RenderTarget::FillRectangle; nothing is traced"
        );
        return;
    }
    if name.len() > NAME_LEN {
        warn!("trace: trace_from={text} is longer than {NAME_LEN} bytes: nothing is traced");
        return;
    }
    {
        let mut stored = FROM.lock();
        stored.0[..name.len()].copy_from_slice(name.as_bytes());
        stored.1 = name.len();
    }
    LEFT.store(budget, Ordering::Relaxed);
    WANTED.store(true, Ordering::Release);
    info!("trace: every call into that DLL from the first {name}, {budget} at most (trace_from=)");
}

/// Splits `<name>[:<n>]`. A method name holds `::` of its own, so the
/// split is at the last colon that is not part of one.
fn split_budget(text: &str) -> Result<(&str, u32), ()> {
    let bytes = text.as_bytes();
    for (i, &b) in bytes.iter().enumerate().rev() {
        if b != b':' {
            continue;
        }
        if bytes.get(i + 1) == Some(&b':') || (i > 0 && bytes[i - 1] == b':') {
            break;
        }
        let tail = &text[i + 1..];
        let n = tail.parse::<u32>().ok().filter(|n| (1..=MAX_BUDGET).contains(n));
        return n.map(|n| (&text[..i], n)).ok_or(());
    }
    Ok((text, BUDGET))
}

/// Whether `trace_from=` was given (one relaxed load).
pub fn wanted() -> bool {
    WANTED.load(Ordering::Acquire)
}

/// The `trace_from=` value, written into `out`; the length written, or 0
/// when nothing is traced or `out` is too short to hold all of it. A
/// value cut in half would arm a method nobody asked for, so a short
/// buffer gets nothing rather than a prefix.
pub fn value(out: &mut [u8]) -> usize {
    if !wanted() {
        return 0;
    }
    let stored = FROM.lock();
    if stored.1 > out.len() {
        return 0;
    }
    out[..stored.1].copy_from_slice(&stored.0[..stored.1]);
    stored.1
}

/// Records one call and says whether the DLL should keep recording.
/// `false` means the budget is spent: the DLL stops there, so a run that
/// has seen what it came for stops paying a system call a call.
pub fn record(process: &str, pid: u64, name: &[u8], args: &[u64]) -> bool {
    // `events=call` and `events_pid=`/`events_match=` select what is
    // written as they do for every other category; a process they
    // leave out is told to stop rather than charged for lines nobody
    // asked for.
    if !wanted() || !events::enabled(Category::Call) || !events::syscall_pid_wanted(pid) {
        return false;
    }
    let Some(left) = take_line() else { return false };
    let text = core::str::from_utf8(name).unwrap_or("?");
    events::emit(
        Category::Call,
        "call",
        format_args!("{}", Fields { pid, process, call: text, args }),
    );
    RECORDED.fetch_add(1, Ordering::Relaxed);
    left > 0
}

/// Records what one call answered, and says whether the DLL should keep
/// recording. An answer is a line like any other and comes out of the
/// same budget as the calls, so a trace can end between a call and its
/// answer -- never the other way about, which would read as an answer to
/// a call the program never made.
pub fn record_answer(process: &str, pid: u64, name: &[u8], answer: u64) -> bool {
    if !wanted() || !events::enabled(Category::Call) || !events::syscall_pid_wanted(pid) {
        return false;
    }
    let Some(left) = take_line() else { return false };
    let text = core::str::from_utf8(name).unwrap_or("?");
    events::emit(
        Category::Call,
        "ret",
        format_args!("{}", Answer { pid, process, from: "", call: text, answer }),
    );
    ANSWERED.fetch_add(1, Ordering::Relaxed);
    left > 0
}

/// Takes one line from the budget: what is left after it, or None once
/// the budget is spent. Taken before the event is written, so two CPUs
/// recording at once cannot both spend the last line.
fn take_line() -> Option<u32> {
    let mut left = LEFT.load(Ordering::Relaxed);
    loop {
        if left == 0 {
            return None;
        }
        match LEFT.compare_exchange_weak(left, left - 1, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => return Some(left - 1),
            Err(now) => left = now,
        }
    }
}

/// What the run traced, for the end of a run. A trace that recorded
/// nothing says so: the method named was never reached, which is as much
/// of an answer as a thousand calls would have been.
pub fn log_stats() {
    if !wanted() {
        return;
    }
    let done = RECORDED.load(Ordering::Relaxed);
    let stored = FROM.lock();
    let name = core::str::from_utf8(&stored.0[..stored.1]).unwrap_or("?");
    if done == 0 {
        info!("trace: {name} was never reached: no call recorded (trace_from=)");
    } else if LEFT.load(Ordering::Relaxed) == 0 {
        // The DLL is told to stop as the budget runs out, so what
        // follows is not counted anywhere: say that the trace ends
        // before the program did, rather than let a run read as a
        // program that stopped calling.
        info!(
            "trace: {done} calls recorded from {name} (trace_from=); \
             the budget is spent, so the trace stops here, not the program"
        );
    } else {
        info!("trace: {done} calls recorded from {name} (trace_from=)");
    }
    // A separate line, because it counts a different thing: how many of
    // those calls were made to a method that returns anything at all.
    // None of them is an answer too -- a trace of methods that all
    // return nothing says so, rather than look like answers were lost.
    let answers = ANSWERED.load(Ordering::Relaxed);
    if done != 0 {
        info!("trace: {answers} of those calls recorded what they answered (trace_from=)");
    }
}

/// One `call` event's fields: the JSON members [`events::emit`] writes
/// after `"ev":"call",`, without braces and with every string quoted.
///
/// A type rather than a `format_args!` where the event is written, so
/// that a host test can read the line the stream would carry. The event
/// stream is split out of the serial log, and a boot scenario matches
/// the serial log, so a doubled comma or quote here is something no
/// scenario can see.
struct Fields<'a> {
    pid: u64,
    process: &'a str,
    call: &'a str,
    args: &'a [u64],
}

impl fmt::Display for Fields<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (pid, args) = (self.pid, Words(self.args));
        let (process, call) = (Str(self.process), Str(self.call));
        write!(f, r#""pid":{pid},"process":{process},"call":{call},"args":{args}"#)
    }
}

/// One `ret` event's fields: what a call answered, as the one word the
/// wrapper took it for. A type for the same reason as [`Fields`]: so a
/// host test can read the line the stream would carry.
/// Shared with `traceimports`, which records the same shape of answer
/// for a call through an import slot (`imports_answers=`).
pub(super) struct Answer<'a> {
    pub(super) pid: u64,
    pub(super) process: &'a str,
    /// The module the call was made from, where that is known, and
    /// empty where it is not -- which is every call recorded here, and
    /// why the field is written only when it has something to say. A
    /// method call is an indirect call through a vtable from anywhere
    /// in the program's own code, and the wrapper that records it is
    /// handed the call's arguments and not its caller; `traceimports`
    /// knows the module of every call it records, the slot the call
    /// went through being an entry of that module's own import address
    /// table.
    pub(super) from: &'a str,
    pub(super) call: &'a str,
    pub(super) answer: u64,
}

impl fmt::Display for Answer<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (pid, answer) = (self.pid, self.answer);
        let (process, call) = (Str(self.process), Str(self.call));
        write!(f, r#""pid":{pid},"process":{process}"#)?;
        if !self.from.is_empty() {
            write!(f, r#","from":{}"#, Str(self.from))?;
        }
        write!(f, r#","call":{call},"answer":{answer}"#)
    }
}

/// An argument list, as a JSON array of unsigned words. Shared with
/// `traceimports`, which records the same shape of call.
pub(super) struct Words<'a>(pub(super) &'a [u64]);

impl fmt::Display for Words<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[")?;
        for (i, word) in self.0.iter().enumerate() {
            if i > 0 {
                f.write_str(",")?;
            }
            write!(f, "{word}")?;
        }
        f.write_str("]")
    }
}

#[cfg(test)]
mod tests {
    use super::{Answer, BUDGET, Fields, MAX_BUDGET, split_budget};

    /// What the event stream carries, which no boot scenario sees: the
    /// scenarios match the serial log, and the events are split out of
    /// it.
    #[test]
    fn a_recorded_call_is_the_fields_of_one_json_event() {
        let fields = Fields {
            pid: 12,
            process: "WINWORD.EXE",
            call: "d2d1.dll!ID2D1RenderTarget::CreateSolidColorBrush",
            args: &[159706400, 2078216, 0],
        };
        assert_eq!(
            alloc::format!("{{\"ev\":\"call\",{fields}}}"),
            "{\"ev\":\"call\",\"pid\":12,\"process\":\"WINWORD.EXE\",\"call\":\
             \"d2d1.dll!ID2D1RenderTarget::CreateSolidColorBrush\",\"args\":\
             [159706400,2078216,0]}"
        );
    }

    /// A method with no arguments at all is still an array, not a
    /// missing member: a reader of the stream can take `args` as read.
    #[test]
    fn a_call_without_arguments_still_has_an_array() {
        let fields = Fields { pid: 1, process: "a.exe", call: "d2d1.dll!X::Y", args: &[] };
        assert_eq!(
            alloc::format!("{fields}"),
            "\"pid\":1,\"process\":\"a.exe\",\"call\":\"d2d1.dll!X::Y\",\"args\":[]"
        );
    }

    /// What a method answered is a line of its own, naming the same
    /// call: a reader pairs the two by name and by order. The word is
    /// what the wrapper was handed, so an HRESULT is its bits rather
    /// than a negative number.
    #[test]
    fn an_answer_is_the_fields_of_one_json_event() {
        let fields = Answer {
            pid: 12,
            process: "WINWORD.EXE",
            from: "",
            call: "d2d1.dll!ID2D1RenderTarget::CreateSolidColorBrush",
            answer: 0x8007_000e,
        };
        assert_eq!(
            alloc::format!("{{\"ev\":\"ret\",{fields}}}"),
            "{\"ev\":\"ret\",\"pid\":12,\"process\":\"WINWORD.EXE\",\"call\":\
             \"d2d1.dll!ID2D1RenderTarget::CreateSolidColorBrush\",\"answer\":\
             2147942414}"
        );
    }

    #[test]
    fn a_value_without_a_count_keeps_the_default() {
        let name = "d2d1.dll!ID2D1RenderTarget::FillRectangle";
        assert_eq!(split_budget(name), Ok((name, BUDGET)));
    }

    #[test]
    fn a_method_name_keeps_its_double_colon() {
        assert_eq!(
            split_budget("d2d1.dll!ID2D1RenderTarget::FillRectangle:9"),
            Ok(("d2d1.dll!ID2D1RenderTarget::FillRectangle", 9))
        );
    }

    #[test]
    fn a_count_that_is_not_one_is_refused() {
        for bad in ["d2d1.dll!A::B:0", "d2d1.dll!A::B:", "d2d1.dll!A::B:lots"] {
            assert!(split_budget(bad).is_err(), "{bad} was taken as a count");
        }
        let past = alloc::format!("d2d1.dll!A::B:{}", MAX_BUDGET as u64 + 1);
        assert!(split_budget(&past).is_err());
    }
}
