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
/// compares against. A value names a `<dll>!<export>` and nothing
/// Windows ships comes near this; a ledger name is longer
/// (`ledger::USER_NAME_LEN`) because its rows carry what a call asked
/// about as well as the function's name.
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

/// The most of gala's DLLs one trace names. A program reaches two or
/// three of them, and a run that named more than this would be naming
/// DLLs gala has not written.
const DLL_COUNT: usize = 8;

/// As much of a DLL's name as is kept. `bcryptprimitives.dll` is the
/// longest name gala ships, and a name longer than this is left out of
/// the list rather than cut: a cut name reads as a DLL nobody shipped.
const DLL_NAME_LEN: usize = 24;

/// The DLLs the recorded calls were made into. The arming is the
/// process's rather than one DLL's (`user/trace_common.rs`), so a trace
/// armed at a Direct3D 10.1 method records the Direct2D calls that
/// follow it -- and which DLLs a run covered is then something the
/// summary has to say, or a reader cannot tell a program that used one
/// DLL from a trace that reached only one of two.
static DLLS: IrqMutex<Dlls> = IrqMutex::new(Dlls::new());

/// The names [`DLLS`] holds, kept sorted so that a run's summary reads
/// the same whichever DLL the program happened to reach first.
struct Dlls {
    names: [[u8; DLL_NAME_LEN]; DLL_COUNT],
    lens: [usize; DLL_COUNT],
    count: usize,
}

impl Dlls {
    const fn new() -> Self {
        Dlls { names: [[0; DLL_NAME_LEN]; DLL_COUNT], lens: [0; DLL_COUNT], count: 0 }
    }

    /// Notes one name, if it is new and there is room for it.
    fn note(&mut self, name: &[u8]) {
        if name.is_empty() || name.len() > DLL_NAME_LEN {
            return;
        }
        let mut at = self.count;
        for i in 0..self.count {
            if self.name(i) == name {
                return;
            }
            if at == self.count && name < self.name(i) {
                at = i;
            }
        }
        if self.count == DLL_COUNT {
            return;
        }
        for i in (at..self.count).rev() {
            self.names[i + 1] = self.names[i];
            self.lens[i + 1] = self.lens[i];
        }
        self.names[at] = [0; DLL_NAME_LEN];
        self.names[at][..name.len()].copy_from_slice(name);
        self.lens[at] = name.len();
        self.count += 1;
    }

    fn name(&self, i: usize) -> &[u8] {
        &self.names[i][..self.lens[i]]
    }
}

/// [`Dlls`] as a sentence names a list: `d2d1.dll`, or
/// `d2d1.dll and d3d10_1.dll`, or three with commas between.
struct List<'a>(&'a Dlls);

impl fmt::Display for List<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for i in 0..self.0.count {
            if i > 0 {
                f.write_str(if i + 1 == self.0.count { " and " } else { ", " })?;
            }
            f.write_str(core::str::from_utf8(self.0.name(i)).unwrap_or("?"))?;
        }
        Ok(())
    }
}

/// Notes the DLL a recorded call was made into: the name before its
/// `!`, as the ledger spells it. A short scan and a lock per *recorded*
/// call, and nothing at all for a call that is not recorded.
fn note_dll(call: &str) {
    let Some(end) = call.find('!') else { return };
    DLLS.lock().note(&call.as_bytes()[..end]);
}

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
    info!(
        "trace: every call into a DLL gala wrote from the first {name}, {budget} at most \
         (trace_from=)"
    );
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
    note_dll(text);
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
    let dlls = DLLS.lock();
    let into = List(&dlls);
    if done == 0 {
        info!("trace: {name} was never reached: no call recorded (trace_from=)");
    } else if LEFT.load(Ordering::Relaxed) == 0 {
        // The DLL is told to stop as the budget runs out, so what
        // follows is not counted anywhere: say that the trace ends
        // before the program did, rather than let a run read as a
        // program that stopped calling.
        info!(
            "trace: {done} calls recorded from {name}, into {into} (trace_from=); \
             the budget is spent, so the trace stops here, not the program"
        );
    } else {
        info!("trace: {done} calls recorded from {name}, into {into} (trace_from=)");
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
    use super::{Answer, BUDGET, Dlls, Fields, List, MAX_BUDGET, split_budget};

    fn dlls(names: &[&str]) -> Dlls {
        let mut set = Dlls::new();
        for name in names {
            set.note(name.as_bytes());
        }
        set
    }

    /// The DLLs a trace covered, as the run's summary names them: one
    /// entry per DLL however many calls were made into it, sorted so
    /// that the line does not depend on which the program reached
    /// first, and a sentence's `and` before the last.
    #[test]
    fn the_dlls_a_trace_covered_are_named_once_each_in_order() {
        assert_eq!(alloc::format!("{}", List(&dlls(&[]))), "");
        assert_eq!(alloc::format!("{}", List(&dlls(&["d2d1.dll"]))), "d2d1.dll");
        assert_eq!(
            alloc::format!("{}", List(&dlls(&["d3d10_1.dll", "d2d1.dll", "d3d10_1.dll"]))),
            "d2d1.dll and d3d10_1.dll"
        );
        assert_eq!(
            alloc::format!("{}", List(&dlls(&["d3d10_1.dll", "dwrite.dll", "d2d1.dll"]))),
            "d2d1.dll, d3d10_1.dll and dwrite.dll"
        );
    }

    /// A name past what is kept is left out rather than cut: a cut name
    /// reads as a DLL nobody shipped. So does an empty one, which is
    /// what a call name beginning with its `!` would leave.
    #[test]
    fn a_name_that_does_not_fit_is_left_out() {
        let long = "d".repeat(super::DLL_NAME_LEN + 1);
        assert_eq!(alloc::format!("{}", List(&dlls(&[&long, "", "d2d1.dll"]))), "d2d1.dll");
    }

    /// Past the room there is, the names already noted stay as they
    /// are: a list that dropped its first entry to make room for a
    /// ninth would read as the trace having missed the DLL it started
    /// in.
    #[test]
    fn past_the_room_there_is_the_names_already_noted_stay() {
        let many: alloc::vec::Vec<alloc::string::String> =
            (0..super::DLL_COUNT + 2).map(|i| alloc::format!("d{i}.dll")).collect();
        let refs: alloc::vec::Vec<&str> = many.iter().map(|s| s.as_str()).collect();
        let set = dlls(&refs);
        assert_eq!(set.count, super::DLL_COUNT);
        assert_eq!(set.name(0), b"d0.dll");
    }

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
