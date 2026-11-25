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
/// (`crashreport::read_text`).
const ARG_TEXT: usize = 64;
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
    SPENT.store(false, Ordering::Relaxed);
    ARMED.store(true, Ordering::Relaxed);
    info!(
        "stack: armed: {name}, {count} captures of the calling thread's user stack \
         (stack_at=; the processes are the ones events_pid= and events_match= choose)"
    );
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
    info!(
        "stack: only calls one of whose arguments points at text containing \"{}\" \
         (stack_match=); the others are passed over and do not spend a capture",
        core::str::from_utf8(&text[..len]).unwrap_or("?")
    );
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
    capture(call, &regs, &bytes[..read], regs.sp(), name, nth, &args);
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
            info!(
                "stack:   argument {i} reads as \"{}\"",
                core::str::from_utf8(&text[..chars]).unwrap_or("?")
            );
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
        DEFAULT_CAPTURES, MAX_CAPTURES, MAX_SCAN_WORDS, NAME_LEN, contains_ignore_case, parse,
        parse_words,
    };

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
