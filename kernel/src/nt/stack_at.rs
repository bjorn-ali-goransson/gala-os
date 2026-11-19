//! The user-mode stack capture (docs/OBSERVABILITY.md, "Stack captures"):
//! `stack_at=<service>[:<n>]` logs the calling thread's user stack the
//! first `n` times a process reaches that system call, and
//! `stack_from=<text>` keeps only the calls a module whose name contains
//! `text` is on the stack of. `stack_words=<n>` reads further up the
//! stack than the default, for a service whose own frame is large.
//!
//! What it answers: a program stops and nothing gala owns records a gap,
//! because the decision was made inside a DLL whose source we do not
//! have. The event stream says which calls happened; this says who made
//! them. The call that prints an error message, the one that frees the
//! window, the first failing open -- each becomes a call path named
//! `<module>+<offset>`, which is the part of the DLL to look at and the
//! gala call it reached last.
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

/// The most a count may ask for. A number is typed on purpose, so this
/// is a guard against a typo rather than a budget: when the calls in
/// between cannot be told apart by `stack_from=`, the one in question
/// can still be deep into the hundreds.
const MAX_CAPTURES: u32 = 512;

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
/// Captures asked for, and how many are left.
static ASKED: AtomicU32 = AtomicU32::new(0);
static LEFT: AtomicU32 = AtomicU32::new(0);
/// Words of the caller's stack a capture reads (`stack_words=`).
static WORDS: AtomicU32 = AtomicU32::new(SCAN_WORDS as u32);
/// Calls of the service that `stack_from=` passed over.
static PASSED: AtomicU32 = AtomicU32::new(0);
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

/// Whether `text` contains `want`, ignoring case (module names are
/// written as the image was named, a command line is not).
fn contains_ignore_case(text: &str, want: &[u8]) -> bool {
    let text = text.as_bytes();
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
            && contains_ignore_case(m.name(), want)
        {
            return true;
        }
    }
    false
}

/// A system call `name` is starting: capture the caller's stack if this
/// is the service asked for, the filter wants it and a capture is left.
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
    capture(&call.process, &regs, &bytes[..read], regs.sp(), name, nth);
}

/// Says once that the count is spent, with what the filter passed over:
/// a filter that matched nothing and one that matched everything look
/// the same from the captures alone.
fn report_spent(name: &str) {
    if SPENT.swap(true, Ordering::Relaxed) {
        return;
    }
    let asked = ASKED.load(Ordering::Relaxed);
    let passed = PASSED.load(Ordering::Relaxed);
    if FILTERED.load(Ordering::Relaxed) {
        let stored = FROM.lock();
        let want = core::str::from_utf8(&stored.0[..stored.1]).unwrap_or("?");
        info!(
            "stack: {name}: the {asked} captures asked for are spent \
             ({passed} calls so far did not reach stack_from={want}); \
             ask for more with stack_at={name}:<n>"
        );
    } else {
        info!(
            "stack: {name}: the {asked} captures asked for are spent; ask for more with \
             stack_at={name}:<n>"
        );
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
    if FILTERED.load(Ordering::Relaxed) {
        let from = FROM.lock();
        let want = core::str::from_utf8(&from.0[..from.1]).unwrap_or("?");
        info!(
            "stack: {name}: {taken} captures of the {asked} asked for, {} calls passed over by stack_from={want}",
            PASSED.load(Ordering::Relaxed)
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

/// Logs where `regs` came from: the instruction, then every word of the
/// already read `stack` that points into a loaded image.
fn capture(process: &Process, regs: &UserRegs, stack: &[u8], sp: u64, name: &str, nth: u32) {
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
        assert!(contains_ignore_case("wwlib.dll", b"wwlib"));
        assert!(contains_ignore_case("WWLIB.DLL", b"wwlib"));
        assert!(contains_ignore_case("kernel32.dll", b"NEL32"));
        assert!(contains_ignore_case("d3d11.dll", b"d3d11.dll"));
        assert!(!contains_ignore_case("d3d11.dll", b"wwlib"));
        // Not a fragment of itself the wrong way round.
        assert!(!contains_ignore_case("wwlib", b"wwlib.dll"));
    }
}
