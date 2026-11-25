//! The user-mode stack capture (docs/OBSERVABILITY.md, "Stack captures"):
//! `stack_at=<service>[:<n>]` logs the calling thread's user stack the
//! first `n` times a process reaches that system call, and
//! `stack_from=<text>` keeps only the calls a module whose name contains
//! `text` is on the stack of, and `stack_match=<text>` only the calls
//! one of whose arguments points at text containing `text`.
//! `stack_words=<n>` reads further up the stack than the default, for a
//! service whose own frame is large.
//!
//! What it answers: a program stops and nothing gala owns records a gap,
//! because the decision was made inside a DLL whose source we do not
//! have. The event stream says which calls happened; this says who made
//! them. The call that prints an error message, the one that frees the
//! window, the first failing open -- each becomes a call path named
//! `<module>+<offset>`, which is the part of the DLL to look at and the
//! gala call it reached last.
//!
//! A capture also says what the call was made with: the first
//! [`ARGS_SHOWN`] arguments, and the text any of them points at. That is
//! what turns "who called this" into "who called this, and for what" --
//! the name a gap was recorded under (`GalaRecordMissing`), the bytes a
//! write was about to write, the path an open failed on -- without a
//! trigger per service.
//!
//! The stack is scanned, not unwound: every word of it that points into
//! a loaded image is named. That brings dead values from earlier frames
//! with it, so a line is evidence rather than proof -- but it needs
//! nothing of the program. Unwinding needs the unwind data and runs in
//! user mode (gala's ntdll fills `crashreport::Walk` while it dispatches
//! an exception, and the kernel only reads it); at a system call the
//! kernel has the stack and the loaded images, and the scan is what
//! those two give.
//!
//! Why `stack_from=` exists. "The first `n`" is the wrong shape of
//! trigger on its own: a service another thread calls in a loop spends
//! the count before the call in question. Word's own write of "Word was
//! unable to start" is behind thousands of DXVK log writes through the
//! same `NtWriteFile`, and no count reaches it -- raising the number
//! only moves the flood. `stack_from=wwlib` says which stack is wanted
//! instead of how many to take, and a call that does not match is passed
//! over without spending the count (it is counted, and the line that
//! says the count is spent says how many were passed over, so a filter
//! that matches nothing is visible rather than silent).
//!
//! Why `stack_match=` exists as well. A module filter cannot separate
//! the calls one module makes about everything. Every gap a Word
//! startup records reaches `GalaRecordMissing` through the same two
//! modules, so `stack_from=` keeps all of them and the gap in question
//! -- one call of the hundreds, late in the boot -- is behind a count
//! no number reaches. What tells those calls apart is the argument: the
//! service's own name for the thing it was asked about, which a capture
//! already quotes. `stack_match=empty%20path` captures the call that
//! recorded that gap and passes the rest over, which turns every row of
//! the ledger into a trigger. The two filters compose: both must want a
//! call for it to be captured.
//!
//! Why `stack_at=etw` exists. A wall a program decides on inside its
//! own code may reach no system call that is about it at all. Office's
//! resource loader fails its caller from a precondition that is already
//! false in the process, and the write of the line it logs that failure
//! on hands the kernel a buffer whose text is not the line, so neither
//! filter above reaches it: a run of 838 writes of Word's own log
//! passed every one of them over. What the program does say in words is
//! the line itself, and a line reaches the kernel as a trace event on
//! the thread that logged it -- gala's advapi32 hands each one over
//! inline (`GalaEtw`, user/advapi32/src/trace.rs) -- so the stack at
//! that handover is the program's own path to the decision.
//! [`LOGGED`] is the capture there: `stack_at=etw` with
//! `stack_match=FGimmeFileFull` is "whose call path logged that", and
//! the phrase is read from the program's log rather than guessed at.
//! `stack_match=` then matches the bytes of the event at either width
//! instead of what a call's arguments point at, because an event is a
//! header and a run of values whose layout lives in a manifest outside
//! the program. It takes a trace session to carry the lines (`etw=`),
//! and it reads deeper than the default ([`LOGGED_WORDS`]).
//!
//! Scope and cost. Off unless `stack_at=` is given, and then one relaxed
//! load on the system-call path (`armed`); a capture reads at most
//! [`SCAN_WORDS`] words of the caller's stack, or what `stack_words=`
//! asks for up to [`MAX_SCAN_WORDS`], and allocates nothing.
//! With `stack_from=` the stack is read before the count is claimed, so
//! a passed-over call costs that read -- the service is already the one
//! named, and it buys the trigger the program's own call path. Which
//! processes are captured is not a knob of its own: it is `events_pid=`
//! and `events_match=`, the same choice the system-call events are
//! filtered by (with neither given, every process).
//!
//! Limit: the capture is at the system-call boundary, which in a WOW64
//! process is the 64-bit thunk, so the words are 64-bit and the 32-bit
//! caller's own frames are above what is scanned. A 32-bit program's
//! path is not placed by this yet.

use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use super::crashreport::{At, module_at};
use super::platform::{self, UserRegs};
use super::process::Process;
use super::syscalls::Call;

/// Words of the caller's stack a capture reads unless `stack_words=`
/// says otherwise (1 KiB, as a crash report's deep stack): enough for
/// several frames of a Windows DLL.
const SCAN_WORDS: usize = 128;

/// The most `stack_words=` may ask for: 4 KiB of a 64 KiB kernel stack
/// (`arch::context::STACK_PAGES`), read at the system-call boundary
/// where the kernel's own stack is still shallow. A deeper scan is for
/// the service whose own frame hides the caller: user32's MessageBoxW
/// keeps about a kilobyte of locals, which puts the program's frames
/// past [`SCAN_WORDS`], and a program that wraps its message boxes puts
/// them further again.
const MAX_SCAN_WORDS: usize = 512;

/// Captures when `stack_at=` gives no count.
const DEFAULT_CAPTURES: u32 = 4;

/// The name that stands for a line the program logged rather than a
/// system call (`stack_at=etw`): the handover of a trace event, which
/// is where a program's own words reach the kernel. Not a service of
/// the table's, so it names no call and cannot collide with one.
pub(super) const LOGGED: &str = "etw";

/// Words a capture at a logged line reads unless `stack_words=` says
/// otherwise. Deeper than [`SCAN_WORDS`] because what lies between is
/// known and large: gala's advapi32 copies the whole record onto its
/// own frame (`etw::RECORD_MAX`, 2 KiB) before it makes the call, so
/// the usual depth would end inside that buffer and never reach the
/// program's frames.
const LOGGED_WORDS: usize = MAX_SCAN_WORDS;

/// Arguments a capture names. Four are in registers on x64 and the rest
/// on the user stack (`abi::syscall_arg`), and eight is what it takes to
/// reach the ones that say what a call is about: `NtWriteFile`'s buffer
/// is its sixth. Past a service's own arguments the values are whatever
/// its caller left there, as the scanned stack words are: a line is
/// evidence, not proof.
const ARGS_SHOWN: usize = 8;

/// The longest text a capture quotes from an argument, and the shortest
/// run that is quoted at all: a run of three characters or fewer is as
/// likely the low bytes of a number as a name
/// (`crashreport::read_text`). The length is that of a record a program
/// logs: Word's own log lines run to about 160 characters and put the
/// message it gives up with 91 in, so a quote cut at a name's length
/// would show the timestamp of the write this facility is aimed at and
/// none of what it says.
const ARG_TEXT: usize = 256;
const ARG_TEXT_LEAST: usize = 4;

/// The most a count may ask for. A number is typed on purpose, so this
/// is a guard against a typo rather than a budget: when the calls in
/// between cannot be told apart by `stack_from=`, the one in question
/// can still be deep into the hundreds.
const MAX_CAPTURES: u32 = 512;

/// The longest `stack_match=` text. A capture quotes at most
/// [`ARG_TEXT`] characters of an argument, so a longer text could never
/// be contained in what it is compared against.
const MATCH_LEN: usize = ARG_TEXT;

/// The longest service name the option keeps (the table's are shorter).
const NAME_LEN: usize = 47;
/// The longest `stack_from=` text (a module name is at most 40).
const FROM_LEN: usize = 40;

/// Whether a capture is armed: the load every system call pays.
static ARMED: AtomicBool = AtomicBool::new(false);
/// The service name to capture at, as `stack_at=` gave it.
static NAME: crate::sync::IrqMutex<([u8; NAME_LEN], usize)> =
    crate::sync::IrqMutex::new(([0; NAME_LEN], 0));
/// The module text `stack_from=` gave, empty when it was not given.
static FROM: crate::sync::IrqMutex<([u8; FROM_LEN], usize)> =
    crate::sync::IrqMutex::new(([0; FROM_LEN], 0));
/// Whether `stack_from=` is in force (read before taking its lock).
static FILTERED: AtomicBool = AtomicBool::new(false);
/// The text `stack_match=` gave, decoded, empty when it was not given.
static MATCH: crate::sync::IrqMutex<([u8; MATCH_LEN], usize)> =
    crate::sync::IrqMutex::new(([0; MATCH_LEN], 0));
/// Whether `stack_match=` is in force (read before taking its lock).
static MATCHING: AtomicBool = AtomicBool::new(false);
/// Captures asked for, and how many are left.
static ASKED: AtomicU32 = AtomicU32::new(0);
static LEFT: AtomicU32 = AtomicU32::new(0);
/// Words of the caller's stack a capture reads (`stack_words=`).
static WORDS: AtomicU32 = AtomicU32::new(SCAN_WORDS as u32);
/// Calls of the service that `stack_from=` passed over.
static PASSED: AtomicU32 = AtomicU32::new(0);
/// Calls of the service that `stack_match=` passed over. Counted apart
/// from [`PASSED`] so a line can say which filter turned a call away.
static UNMATCHED: AtomicU32 = AtomicU32::new(0);
/// Whether the line saying the count is spent has been logged.
static SPENT: AtomicBool = AtomicBool::new(false);

/// `<service>[:<n>]` as a service name and a count, or None when it is
/// neither (an unparsable or zero count rejects the whole option: a
/// capture that silently became the default would be read as the one
/// that was asked for).
fn parse(spec: &str) -> Option<(&str, u32)> {
    let (name, count) = match spec.split_once(':') {
        Some((name, n)) => (name, n.parse::<u32>().ok()?),
        None => (spec, DEFAULT_CAPTURES),
    };
    if name.is_empty() || name.len() > NAME_LEN || count == 0 {
        return None;
    }
    Some((name, count.min(MAX_CAPTURES)))
}

/// Arms the capture from a `stack_at=` value (the boot's command line).
pub fn arm(spec: &str) {
    let Some((name, count)) = parse(spec) else {
        warn!(
            "stack: stack_at={spec} is not <service>[:<n>] with n from 1 to {MAX_CAPTURES}: \
             no capture armed"
        );
        return;
    };
    {
        let mut stored = NAME.lock();
        stored.0[..name.len()].copy_from_slice(name.as_bytes());
        stored.1 = name.len();
    }
    ASKED.store(count, Ordering::Relaxed);
    LEFT.store(count, Ordering::Relaxed);
    PASSED.store(0, Ordering::Relaxed);
    UNMATCHED.store(0, Ordering::Relaxed);
    SPENT.store(false, Ordering::Relaxed);
    let at_logged = name.eq_ignore_ascii_case(LOGGED);
    // The depth the capture starts at, which `stack_words=` overrides:
    // the options are read in a fixed order and this one is read first,
    // so a deeper default for a logged line cannot undo what was typed.
    WORDS.store(if at_logged { LOGGED_WORDS } else { SCAN_WORDS } as u32, Ordering::Relaxed);
    ARMED.store(true, Ordering::Relaxed);
    info!(
        "stack: armed: {name}, {count} captures of the calling thread's user stack \
         (stack_at=; the processes are the ones events_pid= and events_match= choose)"
    );
    if at_logged {
        info!(
            "stack:   at {LOGGED}: the stack a line the program logged reaches the kernel on, \
             which takes a trace session to carry the lines (etw=) and reads \
             {LOGGED_WORDS} words, the record's own frame being in between"
        );
    }
}

/// Keeps the captures to the calls a module whose name contains `text`
/// is on the stack of (a `stack_from=` value). Ignored without
/// `stack_at=`: a filter alone captures nothing, and saying so is more
/// use than a trigger that quietly armed itself.
pub fn only_from(text: &str) {
    if text.is_empty() || text.len() > FROM_LEN {
        warn!(
            "stack: stack_from={text} is not 1 to {FROM_LEN} characters of a module name: \
             no filter"
        );
        return;
    }
    if !armed() {
        warn!("stack: stack_from={text} without stack_at=<service>: nothing to filter");
        return;
    }
    {
        let mut stored = FROM.lock();
        stored.0[..text.len()].copy_from_slice(text.as_bytes());
        stored.1 = text.len();
    }
    FILTERED.store(true, Ordering::Relaxed);
    info!(
        "stack: only calls with a module whose name contains \"{text}\" on the stack \
         (stack_from=); the others are passed over and do not spend a capture"
    );
}

/// Keeps the captures to the calls one of whose first [`ARGS_SHOWN`]
/// arguments points at text containing `text` (a `stack_match=` value).
/// The value's `%XX` escapes are decoded, because the text wanted is a
/// service's own name for a thing and those carry spaces
/// (`stack_match=empty%20path`). Ignored without `stack_at=`, as
/// `stack_from=` is: a filter alone captures nothing.
pub fn only_matching(spec: &str) {
    let mut text = [0u8; MATCH_LEN];
    let len = crate::cmdline::percent_decode_fixed(spec, &mut text).unwrap_or(0);
    if len == 0 {
        warn!(
            "stack: stack_match={spec} is not 1 to {MATCH_LEN} characters \
             (the most a capture quotes of an argument) once its %XX escapes are \
             decoded: no filter"
        );
        return;
    }
    if !armed() {
        warn!("stack: stack_match={spec} without stack_at=<service>: nothing to filter");
        return;
    }
    {
        let mut stored = MATCH.lock();
        stored.0[..len].copy_from_slice(&text[..len]);
        stored.1 = len;
    }
    MATCHING.store(true, Ordering::Relaxed);
    let want = core::str::from_utf8(&text[..len]).unwrap_or("?");
    // The same option over the two triggers, and it says which one it
    // is in force on: at a logged line the text is matched against the
    // event's own bytes, and a line that reads as a call's argument is
    // not what was asked for.
    if wanted(LOGGED) {
        info!(
            "stack: only the events whose bytes hold \"{want}\" (stack_match=); the others \
             are passed over and do not spend a capture"
        );
    } else {
        info!(
            "stack: only calls one of whose arguments points at text containing \"{want}\" \
             (stack_match=); the others are passed over and do not spend a capture"
        );
    }
}

/// `<n>` as a word count, or None when it is not one from 1 to
/// [`MAX_SCAN_WORDS`] (a depth that silently stayed the default would
/// be read as the one that was asked for).
fn parse_words(text: &str) -> Option<usize> {
    text.parse::<usize>().ok().filter(|n| (1..=MAX_SCAN_WORDS).contains(n))
}

/// Sets how far up the caller's stack a capture reads (a `stack_words=`
/// value). Ignored without `stack_at=`, as `stack_from=` is: a depth
/// alone captures nothing, and saying so is more use than a silent
/// setting.
pub fn scan_words(text: &str) {
    let Some(n) = parse_words(text) else {
        warn!(
            "stack: stack_words={text} is not a number from 1 to \
             {MAX_SCAN_WORDS}: the default {SCAN_WORDS} words stand"
        );
        return;
    };
    if !armed() {
        warn!("stack: stack_words={text} without stack_at=<service>: nothing to capture");
        return;
    }
    WORDS.store(n as u32, Ordering::Relaxed);
    info!("stack: {n} words of each caller's stack are read (stack_words=)");
}

/// Whether a capture is armed. The system-call path asks this first, so
/// the option costs one relaxed load when it is not given.
#[inline]
pub fn armed() -> bool {
    ARMED.load(Ordering::Relaxed)
}

/// Whether `name` is the service the option named (case is ignored: the
/// names are written as Windows writes them, a command line is not).
fn wanted(name: &str) -> bool {
    let stored = NAME.lock();
    let want = &stored.0[..stored.1];
    name.len() == want.len()
        && name.as_bytes().iter().zip(want).all(|(a, b)| a.eq_ignore_ascii_case(b))
}

/// Whether `text` contains `want`, ignoring case (a module name is
/// written as the image was named and an argument's text as the service
/// was given it, a command line is neither). Bytes rather than `str`:
/// what an argument points at is read from the program's memory and is
/// not known to be UTF-8.
fn contains_ignore_case(text: &[u8], want: &[u8]) -> bool {
    text.len() >= want.len()
        && (0..=text.len() - want.len()).any(|i| {
            text[i..i + want.len()].iter().zip(want).all(|(a, b)| a.eq_ignore_ascii_case(b))
        })
}

/// Whether the instruction or any scanned stack word is in a module
/// `stack_from=` names. True when there is no filter.
fn from_wanted(process: &Process, ip: u64, stack: &[u8]) -> bool {
    if !FILTERED.load(Ordering::Relaxed) {
        return true;
    }
    let stored = FROM.lock();
    let want = &stored.0[..stored.1];
    let names = core::iter::once(ip)
        .chain(stack.chunks_exact(8).map(|w| u64::from_le_bytes(w.try_into().unwrap_or([0; 8]))));
    for addr in names {
        if let Some(m) = module_at(process, addr)
            && contains_ignore_case(m.name().as_bytes(), want)
        {
            return true;
        }
    }
    false
}

/// Whether the text one of the call's arguments points at contains what
/// `stack_match=` named. True when there is no filter. Reads the same
/// runs a capture quotes, so what the filter chooses by is what the
/// capture then shows.
fn match_wanted(process: &Process, args: &[Option<u64>; ARGS_SHOWN]) -> bool {
    if !MATCHING.load(Ordering::Relaxed) {
        return true;
    }
    // The text is copied out from under its lock before a word of user
    // memory is read: such a read waits for a page that may not be
    // there, and a lock may not be held across a wait (which the
    // kernel's lockdep says in as many words, and did).
    let (want, len) = {
        let stored = MATCH.lock();
        (stored.0, stored.1)
    };
    let want = &want[..len];
    let read = |addr: u64, out: &mut [u8]| process.read(addr, out).is_ok();
    for value in args.iter().filter_map(|v| *v) {
        let mut text = [0u8; ARG_TEXT];
        let chars = super::crashreport::read_text(value, &mut text, &read);
        if chars >= ARG_TEXT_LEAST && contains_ignore_case(&text[..chars], want) {
            return true;
        }
    }
    false
}

/// Whether `bytes` holds `want` as text, ignoring case, at either of
/// the widths an event's values are written in: a byte a character, or
/// two with the high one zero (UTF-16LE, which a program's message
/// usually is). From any offset, because an event is a header and a run
/// of values whose layout lives in a manifest outside the program:
/// where its text begins is not known here, and the text is the
/// trigger. Bytes rather than `str`: a record is what a program wrote.
fn holds_text(bytes: &[u8], want: &[u8]) -> bool {
    if want.is_empty() {
        return false;
    }
    for stride in [1usize, 2] {
        let span = (want.len() - 1) * stride + 1;
        if bytes.len() < span {
            continue;
        }
        for at in 0..=bytes.len() - span {
            // The first character alone decides for almost every
            // offset: an event is mostly bytes that are not it.
            if !bytes[at].eq_ignore_ascii_case(&want[0]) {
                continue;
            }
            let held = want.iter().enumerate().all(|(i, b)| {
                let at = at + i * stride;
                bytes[at].eq_ignore_ascii_case(b) && (stride == 1 || bytes[at + 1] == 0)
            });
            if held {
                return true;
            }
        }
    }
    false
}

/// Whether the line the event carries holds what `stack_match=` named.
/// True when there is no filter, as [`match_wanted`] is for a call: the
/// trigger is then the first lines a session carries.
fn logged_matches(data: &[u8]) -> bool {
    if !MATCHING.load(Ordering::Relaxed) {
        return true;
    }
    // Copied out from under the lock, as the call's matcher does. No
    // user memory is read here -- the record is already on the kernel's
    // frame, whole -- but the two matchers taking the lock the same way
    // is one fewer thing to be careful about.
    let (want, len) = {
        let stored = MATCH.lock();
        (stored.0, stored.1)
    };
    holds_text(data, &want[..len])
}

/// The call's first [`ARGS_SHOWN`] arguments, `None` for one that could
/// not be read (a stack argument on a page that is not there).
fn args_of(call: &Call<'_>) -> [Option<u64>; ARGS_SHOWN] {
    let mut args = [None; ARGS_SHOWN];
    for (i, slot) in args.iter_mut().enumerate() {
        *slot = call.arg(i).ok();
    }
    args
}

/// A system call `name` is starting: capture the caller's stack if this
/// is the service asked for, the filters want it and a capture is left.
/// Called from the dispatcher only when [`armed`].
pub(super) fn on_call(call: &Call<'_>, name: &str) {
    if !wanted(name) || !crate::events::syscall_pid_wanted(call.process.id) {
        return;
    }
    // Spent before the stack is read: once the count is gone the option
    // costs the service nothing but this load.
    if LEFT.load(Ordering::Relaxed) == 0 {
        report_spent(name);
        return;
    }
    // What the call is about decides before its stack is read:
    // `stack_match=` needs the arguments only, so a call it turns away
    // costs the service those reads and not the scan.
    let args = args_of(call);
    if !match_wanted(&call.process, &args) {
        UNMATCHED.fetch_add(1, Ordering::Relaxed);
        return;
    }
    take(call, name, &args);
}

/// A line the program logged is reaching the kernel, as the trace event
/// carrying it (`nt::syscalls`' GalaEtw, op 1, on the thread that
/// logged it): capture that thread's stack when the capture is armed at
/// [`LOGGED`] and `stack_match=` wants the line. `data` is the event's
/// values, already copied whole onto the kernel's frame.
pub(super) fn on_logged(call: &Call<'_>, data: &[u8]) {
    if !armed() || !wanted(LOGGED) || !crate::events::syscall_pid_wanted(call.process.id) {
        return;
    }
    if LEFT.load(Ordering::Relaxed) == 0 {
        report_spent(LOGGED);
        return;
    }
    // What the line says decides before the stack is read, as the
    // arguments do for a call: a line that is not the one asked for
    // costs this search and not the scan.
    if !logged_matches(data) {
        UNMATCHED.fetch_add(1, Ordering::Relaxed);
        return;
    }
    // GalaEtw's own arguments, which is what the call was made with:
    // the op, the record and its length. The text one of them reads as
    // is the record itself, so the capture quotes the line it followed.
    take(call, LOGGED, &args_of(call));
}

/// Whether an event is wanted for a capture whatever the event stream
/// makes of it. The stream's line budget bounds the log, and a session
/// whose budget is out is ended (the DLL stops sending), so a trigger
/// that let the budget answer for it would go quiet on the long run it
/// is for -- the one where the line in question is late.
pub(super) fn wants_events() -> bool {
    armed() && wanted(LOGGED) && LEFT.load(Ordering::Relaxed) != 0
}

/// The wanted call's stack, captured if `stack_from=` reaches it and a
/// capture is left. What the two triggers share: they differ in what
/// makes a call wanted, not in what is logged once it is.
fn take(call: &Call<'_>, name: &str, args: &[Option<u64>; ARGS_SHOWN]) {
    let regs = call.frame.return_regs();
    let mut bytes = [0u8; MAX_SCAN_WORDS * 8];
    let words = (WORDS.load(Ordering::Relaxed) as usize).clamp(1, MAX_SCAN_WORDS);
    let read = read_stack(&call.process, regs.sp(), &mut bytes[..words * 8]);
    // The filter decides before the count is claimed, so the calls it
    // passes over cannot spend the captures the wanted one needs.
    if !from_wanted(&call.process, regs.ip(), &bytes[..read]) {
        PASSED.fetch_add(1, Ordering::Relaxed);
        return;
    }
    // One capture a claim, so two CPUs in the service at once take two
    // of the count rather than the same one.
    let mut left = LEFT.load(Ordering::Relaxed);
    loop {
        if left == 0 {
            report_spent(name);
            return;
        }
        match LEFT.compare_exchange_weak(left, left - 1, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => break,
            Err(now) => left = now,
        }
    }
    let nth = ASKED.load(Ordering::Relaxed).saturating_sub(left) + 1;
    capture(call, &regs, &bytes[..read], regs.sp(), name, nth, args);
}

/// Says once that the count is spent, with what the filter passed over:
/// a filter that matched nothing and one that matched everything look
/// the same from the captures alone.
fn report_spent(name: &str) {
    if SPENT.swap(true, Ordering::Relaxed) {
        return;
    }
    let asked = ASKED.load(Ordering::Relaxed);
    let filters = Filters::now();
    if filters.any() {
        info!(
            "stack: {name}: the {asked} captures asked for are spent \
             ({}); ask for more with stack_at={name}:<n>",
            filters.skipped(true)
        );
    } else {
        info!(
            "stack: {name}: the {asked} captures asked for are spent; ask for more with \
             stack_at={name}:<n>"
        );
    }
}

/// The filters' texts copied out from under their locks, with what each
/// has passed over. A line is formatted from this rather than from the
/// locks themselves: the log takes a lock of its own, and a formatter
/// that held one of these while it did would be taking the two in the
/// opposite order to every other reader of them.
struct Filters {
    from: [u8; FROM_LEN],
    from_len: usize,
    from_passed: u32,
    text: [u8; MATCH_LEN],
    text_len: usize,
    text_passed: u32,
}

impl Filters {
    /// The filters as they stand, each empty when it is not in force.
    fn now() -> Self {
        let mut f = Self {
            from: [0; FROM_LEN],
            from_len: 0,
            from_passed: PASSED.load(Ordering::Relaxed),
            text: [0; MATCH_LEN],
            text_len: 0,
            text_passed: UNMATCHED.load(Ordering::Relaxed),
        };
        if FILTERED.load(Ordering::Relaxed) {
            let stored = FROM.lock();
            f.from_len = stored.1;
            f.from[..stored.1].copy_from_slice(&stored.0[..stored.1]);
        }
        if MATCHING.load(Ordering::Relaxed) {
            let stored = MATCH.lock();
            f.text_len = stored.1;
            f.text[..stored.1].copy_from_slice(&stored.0[..stored.1]);
        }
        f
    }

    /// Whether either filter is in force.
    fn any(&self) -> bool {
        self.from_len != 0 || self.text_len != 0
    }

    /// What they have passed over, as a line writes it. `so_far` is the
    /// spent line's wording, which counts the calls not captured yet;
    /// the closing line's is the whole run's.
    fn skipped(&self, so_far: bool) -> Skipped<'_> {
        Skipped { filters: self, so_far }
    }
}

/// [`Filters::skipped`]'s clauses: one per filter in force, in the
/// order the options are read.
struct Skipped<'a> {
    filters: &'a Filters,
    so_far: bool,
}

impl core::fmt::Display for Skipped<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let f_ = self.filters;
        let mut clauses = 0;
        for (len, text, passed, verb, option) in [
            (f_.from_len, &f_.from[..], f_.from_passed, "reach", "stack_from"),
            (f_.text_len, &f_.text[..], f_.text_passed, "match", "stack_match"),
        ] {
            if len == 0 {
                continue;
            }
            if clauses != 0 {
                f.write_str(", ")?;
            }
            clauses += 1;
            let want = core::str::from_utf8(&text[..len]).unwrap_or("?");
            if self.so_far {
                write!(f, "{passed} calls so far did not {verb} {option}={want}")?;
            } else {
                write!(f, "{passed} calls passed over by {option}={want}")?;
            }
        }
        Ok(())
    }
}

/// One line for the end of a run, when a capture was armed: what the
/// captures alone cannot say is whether a filter that produced none
/// matched nothing or the service was never called, and the difference
/// is the next thing to type.
pub fn log_stats() {
    if !armed() {
        return;
    }
    let stored = NAME.lock();
    let name = core::str::from_utf8(&stored.0[..stored.1]).unwrap_or("?");
    let asked = ASKED.load(Ordering::Relaxed);
    let taken = asked.saturating_sub(LEFT.load(Ordering::Relaxed));
    let filters = Filters::now();
    if filters.any() {
        info!(
            "stack: {name}: {taken} captures of the {asked} asked for, {}",
            filters.skipped(false)
        );
    } else {
        info!("stack: {name}: {taken} captures of the {asked} asked for");
    }
    // A capture at a logged line took nothing because nothing was
    // logged to the kernel: the lines reach it inside a trace session
    // and none was asked for. Which option is missing is the next thing
    // to type, and the captures alone do not say it.
    if name.eq_ignore_ascii_case(LOGGED) && !super::etw::wanted() {
        warn!(
            "stack: {LOGGED}: no trace session was asked for, so no line a program logged \
             reached the kernel: ask for one with etw=<provider>[:<level>:...], \
             etw=* for every provider (docs/OBSERVABILITY.md, \"Trace events\")"
        );
    }
}

/// Reads up to `buf` of the stack at `sp`, a page's part at a time as a
/// crash report reads its stack: one read each rather than one a word,
/// and a stack that ends in an unmapped page gives what was there up to
/// it. Returns the bytes read.
fn read_stack(process: &Process, sp: u64, buf: &mut [u8]) -> usize {
    let end = sp.wrapping_add(buf.len() as u64);
    let mut at = sp;
    while at < end {
        let chunk = ((at | 0xfff) + 1).min(end) - at;
        let off = (at - sp) as usize;
        if process.read(at, &mut buf[off..off + chunk as usize]).is_err() {
            break;
        }
        at += chunk;
    }
    at.wrapping_sub(sp) as usize
}

/// A capture's arguments as the log writes them: hexadecimal, in order,
/// and `?` for one that could not be read (a stack argument on a page
/// that is not there).
struct Args<'a>(&'a [Option<u64>; ARGS_SHOWN]);

/// A run of text as one line of the log: a tab shown as `\t`, so a
/// record whose fields a tab separates is quoted whole, where its fields
/// end stays visible, and nothing a program wrote can break the line it
/// is quoted on.
struct Quoted<'a>(&'a [u8]);

impl core::fmt::Display for Quoted<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        for &b in self.0 {
            match b {
                b'\t' => f.write_str("\\t")?,
                _ => f.write_str(core::str::from_utf8(&[b]).unwrap_or("?"))?,
            }
        }
        Ok(())
    }
}

impl core::fmt::Display for Args<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        for (i, value) in self.0.iter().enumerate() {
            if i != 0 {
                f.write_str(", ")?;
            }
            match value {
                Some(v) => write!(f, "{v:#x}")?,
                None => f.write_str("?")?,
            }
        }
        Ok(())
    }
}

/// Logs what `regs` was called with and where it came from: the
/// arguments and the text they point at, the instruction, then every
/// word of the already read `stack` that points into a loaded image.
fn capture(
    call: &Call<'_>,
    regs: &UserRegs,
    stack: &[u8],
    sp: u64,
    name: &str,
    nth: u32,
    args: &[Option<u64>; ARGS_SHOWN],
) {
    let process = &call.process;
    let tid = super::thread::current()
        .and_then(|t| match &t.body {
            super::object::ObjectBody::Thread(t) => Some(t.id),
            _ => None,
        })
        .unwrap_or(0);
    info!(
        "stack: {name} from {} (pid {}, tid {tid}), capture {nth} of {}, at {}",
        process.name,
        process.id,
        ASKED.load(Ordering::Relaxed),
        At::of(process, regs.ip())
    );
    // What the call was made with, before the stack it was made from:
    // the values on one line, and a line per argument that points at
    // text, which is what a service's own name for the thing it was
    // asked about reads as.
    info!("stack:   arguments 0-{}: {}", ARGS_SHOWN - 1, Args(args));
    let read = |addr: u64, out: &mut [u8]| process.read(addr, out).is_ok();
    for (i, value) in args.iter().enumerate().filter_map(|(i, v)| v.map(|v| (i, v))) {
        let mut text = [0u8; ARG_TEXT];
        let chars = super::crashreport::read_text(value, &mut text, &read);
        if chars >= ARG_TEXT_LEAST {
            info!("stack:   argument {i} reads as \"{}\"", Quoted(&text[..chars]));
        }
    }
    let mut shown = 0;
    for (i, word) in stack.chunks_exact(8).enumerate() {
        let mut value = [0u8; 8];
        value.copy_from_slice(word);
        let value = u64::from_le_bytes(value);
        if module_at(process, value).is_some() {
            info!("stack:   [{}+{:#05x}] {}", platform::SP_NAME, 8 * i, At::of(process, value));
            shown += 1;
        }
    }
    info!(
        "stack:   ({shown} of the {} stack words read from {sp:#x} point into an image)",
        stack.len() / 8
    );
}

#[cfg(test)]
mod tests {
    use super::{
        DEFAULT_CAPTURES, MAX_CAPTURES, MAX_SCAN_WORDS, NAME_LEN, contains_ignore_case, holds_text,
        parse, parse_words,
    };

    /// An event as the DLL hands it over: values with no layout the
    /// kernel knows, the text somewhere inside them.
    fn event(text: &str, wide: bool) -> alloc::vec::Vec<u8> {
        let mut bytes = alloc::vec![0x11, 0x00, 0xff, 0x7f, 0x03];
        if wide {
            bytes.extend(text.encode_utf16().flat_map(|u| u.to_le_bytes()));
        } else {
            bytes.extend_from_slice(text.as_bytes());
        }
        bytes.extend_from_slice(&[0x00, 0x2a, 0x00, 0x00]);
        bytes
    }

    #[test]
    fn a_logged_line_matches_at_either_width() {
        // What the option is typed for: a phrase of the program's own
        // log, which a manifest provider writes as UTF-16 and a
        // string-only one as bytes.
        for wide in [false, true] {
            let e = event("FGimmeFileFull {\"cid\": 363}", wide);
            assert!(holds_text(&e, b"FGimmeFileFull"));
            assert!(holds_text(&e, b"fgimmefilefull"));
            assert!(holds_text(&e, b"\"cid\": 363"));
            assert!(!holds_text(&e, b"FGimmeFilePart"));
            assert!(!holds_text(&e, b"Sorry, something went wrong"));
        }
    }

    #[test]
    fn a_wide_line_is_not_matched_as_every_other_byte() {
        // The stride is the width of the text, not a way to skip bytes:
        // the 8-bit search must not read a wide line's characters as a
        // match for every second one of them, nor the wide search an
        // 8-bit line.
        let wide = event("abcdefgh", true);
        assert!(holds_text(&wide, b"abcd"));
        assert!(!holds_text(&wide, b"aceg"));
        let narrow = event("a\0b\0c\0d\0", false);
        assert!(holds_text(&narrow, b"abcd"));
        // An empty filter is not a match for everything: an empty
        // `stack_match=` is refused before this, and were it not, every
        // event answering to it would be the opposite of a filter.
        assert!(!holds_text(&wide, b""));
        assert!(!holds_text(&[], b"a"));
        // One character, at either width, is still the shortest search.
        assert!(holds_text(&wide, b"h"));
        assert!(holds_text(&narrow, b"d"));
    }

    #[test]
    fn parses_a_service_and_a_count() {
        assert_eq!(parse("NtWriteFile"), Some(("NtWriteFile", DEFAULT_CAPTURES)));
        assert_eq!(parse("NtWriteFile:2"), Some(("NtWriteFile", 2)));
        // More than the log can carry is cut to what it can.
        assert_eq!(parse("NtWriteFile:100000"), Some(("NtWriteFile", MAX_CAPTURES)));
    }

    #[test]
    fn refuses_what_is_not_one() {
        // A count that is not a number, or is none at all: the option is
        // refused rather than quietly armed with the default.
        assert_eq!(parse("NtWriteFile:"), None);
        assert_eq!(parse("NtWriteFile:many"), None);
        assert_eq!(parse("NtWriteFile:0"), None);
        assert_eq!(parse(""), None);
        assert_eq!(parse(":4"), None);
        let long = "N".repeat(NAME_LEN + 1);
        assert_eq!(parse(&long), None);
    }

    #[test]
    fn parses_a_word_count() {
        assert_eq!(parse_words("256"), Some(256));
        assert_eq!(parse_words("1"), Some(1));
        assert_eq!(parse_words(&MAX_SCAN_WORDS.to_string()), Some(MAX_SCAN_WORDS));
    }

    #[test]
    fn refuses_a_word_count_that_is_not_one() {
        // Out of range is refused rather than cut to the limit: a scan
        // that quietly became a different depth would be read as the
        // depth that was asked for.
        assert_eq!(parse_words(&(MAX_SCAN_WORDS + 1).to_string()), None);
        assert_eq!(parse_words("0"), None);
        assert_eq!(parse_words(""), None);
        assert_eq!(parse_words("deep"), None);
        assert_eq!(parse_words("-8"), None);
    }

    #[test]
    fn the_filter_is_a_name_fragment_either_case() {
        // `stack_from=wwlib` is how the option is meant to be typed:
        // part of a module's name, in whichever case it comes to hand.
        assert!(contains_ignore_case(b"wwlib.dll", b"wwlib"));
        assert!(contains_ignore_case(b"WWLIB.DLL", b"wwlib"));
        assert!(contains_ignore_case(b"kernel32.dll", b"NEL32"));
        assert!(contains_ignore_case(b"d3d11.dll", b"d3d11.dll"));
        assert!(!contains_ignore_case(b"d3d11.dll", b"wwlib"));
        // Not a fragment of itself the wrong way round.
        assert!(!contains_ignore_case(b"wwlib", b"wwlib.dll"));
    }
}
