//! Kernel hooks (docs/OBSERVABILITY.md, "Hooks"): `hook=<event>=<action>`
//! asks the kernel to do something the next few times something happens
//! inside it. One event so far -- a top-level window shown -- and one
//! action -- a screendump of the desktop, optionally a while after it.
//!
//! What it answers: what did the screen look like when the program put
//! its window up. The screendumps a run can be asked for
//! (`XTASK_SHOTS=`, WORKFLOW "Screenshots") are the host's, and the host
//! knows only two things: the wall clock, and the text of the serial
//! log. A second guessed from the clock misses a window that lives two
//! of ninety (Word's splash), and a log line only works when the line is
//! known in advance -- `XTASK_SHOTS='desktop: shown window+1'` catches
//! the *first* window of a run and nothing after it, and a window whose
//! title is empty, as Word's splash is, cannot be told from any other by
//! its line at all. The kernel is where a window being shown is a fact
//! rather than a string to guess, so the kernel is what asks for the
//! picture: every window, each to its own file, named after the program
//! that opened it.
//!
//! How the two halves meet. The kernel cannot encode a PNG and does not
//! own the screen in a form the host can read, so it does not take the
//! picture: it writes a request to the serial log,
//!
//! ```text
//! hook: shot +1.000 winword-exe-w3: window 'Document1 - Word' shown by WINWORD.EXE (pid 12)
//! ```
//!
//! and xtask, which already polls that log, takes the screendump when
//! the delay is up and writes it to
//! `target/screenshots/<scenario>-hook-<name>.png` (the `<name>` is the
//! request's, so the file says which window it is). A request nothing
//! reads is still a record of the event, which is why the line carries
//! the window and the program and not only the delay.
//!
//! Why the match and the count. The same reason `stack_from=` and
//! `stack_at=`'s count exist (see [`super::stack_at`]): a desktop with a
//! shell on it shows windows the whole run, so "every window" fills the
//! disk and buries the one that was wanted. `hook=window/WINWORD=shot`
//! says which windows instead of how many, matching the program's name
//! or the title, either case; `:<n>` bounds what is left. A window the
//! match passed over is counted, and the line that says the count is
//! spent says how many, so a match that catches nothing is visible
//! rather than silent.
//!
//! Scope and cost. Off unless `hook=` is given, and then one relaxed
//! load on the window-show path ([`armed`]) -- not a system call in a
//! loop but a window appearing, so the armed state is one mutex and not
//! an atomic per field. Nothing is allocated: the hooks live in a fixed
//! array of [`MAX_HOOKS`], their match text in [`MATCH_LEN`] bytes each,
//! and a request is formatted into the log and not kept.
//!
//! Limit: the event is `winstate::show`, where a top-level window
//! becomes visible on gala's desktop. A child control, a window a
//! program creates and never shows, and anything drawn without a window
//! at all (a framebuffer client) raise nothing.

use core::fmt::Write as _;
use core::sync::atomic::{AtomicBool, Ordering};

/// How many `hook=` words are kept. A hook is typed by hand on a command
/// line; more than a few at once is a mistake rather than a plan, and
/// the refusal says so.
const MAX_HOOKS: usize = 4;

/// The longest match text (a program name is at most 64 UTF-16 units,
/// and a word of a title is shorter still).
const MATCH_LEN: usize = 32;

/// The longest name a request carries, which the host reads back and
/// names a file by.
const NAME_LEN: usize = 48;

/// Room a name keeps for its `-w<id>` (a u32 is at most 10 digits).
const ID_LEN: usize = 12;

/// How much of a title a request's line shows.
const TITLE_LEN: usize = 64;

/// Times a hook fires when `:<n>` is not given. A screendump of a
/// 1024 x 768 desktop is about a quarter of a megabyte, so the default
/// is what a run can be read through, not what it could hold.
const DEFAULT_TIMES: u32 = 4;

/// The most `:<n>` may ask for.
const MAX_TIMES: u32 = 64;

/// The longest delay `+<secs>` may ask for: a minute, past which the
/// window the picture was wanted of is long gone.
const MAX_AFTER_MS: u32 = 60_000;

/// Whether any hook is armed: the load the window-show path pays.
static ARMED: AtomicBool = AtomicBool::new(false);

/// The armed hooks.
static HOOKS: crate::sync::IrqMutex<Hooks> = crate::sync::IrqMutex::new(Hooks::NONE);

/// What a hook fires on.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Event {
    /// A top-level window became visible on the desktop
    /// (`winstate::show`).
    WindowShown,
}

impl Event {
    fn parse(name: &str) -> Option<Self> {
        match name {
            "window" => Some(Self::WindowShown),
            _ => None,
        }
    }

    /// The event as the arming line names it.
    fn as_str(self) -> &'static str {
        match self {
            Self::WindowShown => "window",
        }
    }
}

/// What a hook does when it fires.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Action {
    /// Ask the host for a screendump of the desktop (`hook: shot`).
    Shot,
}

impl Action {
    fn parse(name: &str) -> Option<Self> {
        match name {
            "shot" => Some(Self::Shot),
            _ => None,
        }
    }
}

/// One armed hook, and where it has got to.
#[derive(Clone, Copy)]
struct Hook {
    event: Event,
    action: Action,
    /// The wait between the event and the action (`+<secs>`).
    after_ms: u32,
    /// The text a window's program or title must hold, empty when
    /// `/<text>` was not given.
    text: [u8; MATCH_LEN],
    text_len: usize,
    /// Times asked for, and how many are left.
    asked: u32,
    left: u32,
    /// Events the match passed over.
    passed: u32,
    /// Whether the line saying the count is spent has been logged.
    spent: bool,
}

impl Hook {
    const NONE: Self = Self {
        event: Event::WindowShown,
        action: Action::Shot,
        after_ms: 0,
        text: [0; MATCH_LEN],
        text_len: 0,
        asked: 0,
        left: 0,
        passed: 0,
        spent: false,
    };

    /// The match text, empty when there is none.
    fn text(&self) -> &[u8] {
        &self.text[..self.text_len]
    }

    /// Whether a window with this program and title is one this hook
    /// wants (with no match text, every window is).
    fn wants(&self, program: &[u16], title: &[u16]) -> bool {
        self.text_len == 0
            || contains_ignore_case(program, self.text())
            || contains_ignore_case(title, self.text())
    }
}

/// The armed hooks, in the order their `hook=` words were given.
struct Hooks {
    n: usize,
    list: [Hook; MAX_HOOKS],
}

impl Hooks {
    const NONE: Self = Self { n: 0, list: [Hook::NONE; MAX_HOOKS] };
}

/// Whether anything is armed (the window-show path's load).
pub fn armed() -> bool {
    ARMED.load(Ordering::Relaxed)
}

/// `<secs>` as milliseconds, or None when it is not a delay from 0 to
/// [`MAX_AFTER_MS`] (`1`, `.5` and `1.250` are delays; `half` and `-1`
/// are not). Decimals and not a float: the kernel has no floating
/// point, and the host reads the delay back as text either way.
fn parse_ms(text: &str) -> Option<u32> {
    let (whole, frac) = match text.split_once('.') {
        Some((w, f)) => (w, f),
        None => (text, ""),
    };
    if whole.is_empty() && frac.is_empty() {
        return None;
    }
    let secs = if whole.is_empty() { 0 } else { whole.parse::<u32>().ok()? };
    // Three digits of a second, padded ("5" is 500 ms); a fourth digit
    // the log cannot carry is refused rather than rounded away.
    if frac.len() > 3 || !frac.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let mut ms = 0;
    for i in 0..3 {
        ms = 10 * ms + u32::from(frac.as_bytes().get(i).copied().unwrap_or(b'0') - b'0');
    }
    secs.checked_mul(1000)?.checked_add(ms).filter(|&ms| ms <= MAX_AFTER_MS)
}

/// `<event>[/<text>][:<n>]=<action>[+<secs>]` as a hook, or None when it
/// is not one (an unparsable part refuses the whole word: a hook that
/// silently became a different one would be read as the one that was
/// asked for).
fn parse(spec: &str) -> Option<Hook> {
    let (on, does) = spec.split_once('=')?;
    // `<event>[/<text>][:<n>]`
    let (on, times) = match on.split_once(':') {
        Some((on, n)) => (on, n.parse::<u32>().ok()?),
        None => (on, DEFAULT_TIMES),
    };
    let (event, text) = match on.split_once('/') {
        Some((event, text)) => (event, text),
        None => (on, ""),
    };
    // `<action>[+<secs>]`
    let (action, after_ms) = match does.split_once('+') {
        Some((action, secs)) => (action, parse_ms(secs)?),
        None => (does, 0),
    };
    if times == 0 || text.len() > MATCH_LEN {
        return None;
    }
    let mut hook = Hook {
        event: Event::parse(event)?,
        action: Action::parse(action)?,
        after_ms,
        asked: times.min(MAX_TIMES),
        left: times.min(MAX_TIMES),
        ..Hook::NONE
    };
    hook.text[..text.len()].copy_from_slice(text.as_bytes());
    hook.text_len = text.len();
    Some(hook)
}

/// Arms one `hook=` word (the boot's command line, where `hook=` may be
/// repeated; the words are taken in the order they are given).
pub fn arm(spec: &str) {
    let Some(hook) = parse(spec) else {
        warn!(
            "hook: hook={spec} is not <event>[/<text>][:<n>]=<action>[+<secs>] with an event of \
             \"window\", an action of \"shot\", n from 1 to {MAX_TIMES}, text of at most \
             {MATCH_LEN} characters and secs from 0 to {}: no hook armed",
            MAX_AFTER_MS / 1000
        );
        return;
    };
    {
        let mut hooks = HOOKS.lock();
        if hooks.n == MAX_HOOKS {
            warn!("hook: hook={spec} is past the {MAX_HOOKS} hooks a boot may arm: no hook armed");
            return;
        }
        let n = hooks.n;
        hooks.list[n] = hook;
        hooks.n = n + 1;
    }
    ARMED.store(true, Ordering::Relaxed);
    info!(
        "hook: armed: {} shown -> screendump, {} shot{}, {}.{:03} s after each (hook=){}{}{}",
        hook.event.as_str(),
        hook.asked,
        if hook.asked == 1 { "" } else { "s" },
        hook.after_ms / 1000,
        hook.after_ms % 1000,
        if hook.text_len == 0 { "" } else { ", only those whose program or title holds \"" },
        Ascii::<MATCH_LEN>::of_bytes(hook.text()),
        if hook.text_len == 0 { "" } else { "\" (either case)" },
    );
}

/// A top-level window became visible: every hook armed for it fires,
/// each writing its request to the log. Called with the desktop's lock
/// held (`winstate::show`), which is why nothing here takes it.
pub fn window_shown(id: u32, pid: u64, program: &[u16], title: &[u16]) {
    if !armed() {
        return;
    }
    let mut hooks = HOOKS.lock();
    let n = hooks.n;
    for hook in &mut hooks.list[..n] {
        if hook.event != Event::WindowShown {
            continue;
        }
        if !hook.wants(program, title) {
            hook.passed += 1;
            continue;
        }
        if hook.left == 0 {
            if !hook.spent {
                hook.spent = true;
                info!(
                    "hook: {}: no shots left of the {} asked for ({} windows passed over by \
                     the match)",
                    hook.event.as_str(),
                    hook.asked,
                    hook.passed
                );
            }
            continue;
        }
        hook.left -= 1;
        match hook.action {
            Action::Shot => info!(
                "hook: shot +{}.{:03} {}: window '{}' shown by {} (pid {pid})",
                hook.after_ms / 1000,
                hook.after_ms % 1000,
                Name::of(program, id),
                Ascii::<TITLE_LEN>::of(title),
                Ascii::<NAME_LEN>::of(program),
            ),
        }
    }
}

/// A window's name as the host reads it back from a request and names a
/// file by: the program's name in lower case with every run of anything
/// but a letter or a digit as one `-`, then `-w` and the window's id
/// (`WINWORD.EXE` window 3 is `winword-exe-w3`). The id is what tells
/// two windows of one program apart, the program is what makes the file
/// readable, and a program with no name at all is `window`.
struct Name {
    text: [u8; NAME_LEN],
    len: usize,
}

impl Name {
    fn of(program: &[u16], id: u32) -> Self {
        let mut text = [0u8; NAME_LEN];
        let mut len = 0;
        // Cut with the room for `-w<id>` kept: the window is what makes
        // the name unique, so it is the part that must not be lost.
        for &c in program {
            if len >= NAME_LEN - ID_LEN {
                break;
            }
            match u8::try_from(c) {
                Ok(b) if b.is_ascii_alphanumeric() => {
                    text[len] = b.to_ascii_lowercase();
                    len += 1;
                }
                // One `-` for a run of anything else, never a leading one.
                _ if len > 0 && text[len - 1] != b'-' => {
                    text[len] = b'-';
                    len += 1;
                }
                _ => {}
            }
        }
        while len > 0 && text[len - 1] == b'-' {
            len -= 1;
        }
        if len == 0 {
            text[..6].copy_from_slice(b"window");
            len = 6;
        }
        let mut name = Self { text, len };
        let _ = write!(name, "-w{id}");
        name
    }
}

impl core::fmt::Write for Name {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        for &b in s.as_bytes() {
            if self.len == NAME_LEN {
                break;
            }
            self.text[self.len] = b;
            self.len += 1;
        }
        Ok(())
    }
}

impl core::fmt::Display for Name {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(core::str::from_utf8(&self.text[..self.len]).unwrap_or("?"))
    }
}

/// Up to `N` characters of a UTF-16 string as printable ASCII, for a log
/// line: anything else is `?`, as a crash report's strings are, and a
/// longer string is cut (a title is the program's to choose, and a line
/// the length of one is no use).
struct Ascii<const N: usize> {
    text: [u8; N],
    len: usize,
}

impl<const N: usize> Ascii<N> {
    const NONE: Self = Self { text: [0; N], len: 0 };

    fn of(text: &[u16]) -> Self {
        let mut out = Self::NONE;
        for &c in text.iter().take(N) {
            out.text[out.len] = match u8::try_from(c) {
                Ok(b) if (0x20..0x7f).contains(&b) => b,
                _ => b'?',
            };
            out.len += 1;
        }
        out
    }

    fn of_bytes(text: &[u8]) -> Self {
        let mut out = Self::NONE;
        for &b in text.iter().take(N) {
            out.text[out.len] = if (0x20..0x7f).contains(&b) { b } else { b'?' };
            out.len += 1;
        }
        out
    }
}

impl<const N: usize> core::fmt::Display for Ascii<N> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(core::str::from_utf8(&self.text[..self.len]).unwrap_or("?"))
    }
}

/// Whether `text` (a program's name or a window's title) holds `needle`,
/// ignoring case as `stack_from=` does: the option is typed by hand, in
/// whichever case the name came to hand.
fn contains_ignore_case(text: &[u16], needle: &[u8]) -> bool {
    if needle.is_empty() || needle.len() > text.len() {
        return false;
    }
    let lower = |c: u16| match u8::try_from(c) {
        Ok(b) => u16::from(b.to_ascii_lowercase()),
        Err(_) => c,
    };
    text.windows(needle.len())
        .any(|w| w.iter().zip(needle).all(|(&c, &n)| lower(c) == u16::from(n.to_ascii_lowercase())))
}

#[cfg(test)]
mod tests {
    use alloc::format;
    use alloc::vec::Vec;

    use super::{
        Action, DEFAULT_TIMES, Event, MATCH_LEN, MAX_AFTER_MS, MAX_TIMES, NAME_LEN, Name,
        contains_ignore_case, parse, parse_ms,
    };

    fn utf16(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    #[test]
    fn parses_an_event_and_an_action() {
        let h = parse("window=shot").unwrap();
        assert!(h.event == Event::WindowShown && h.action == Action::Shot);
        assert_eq!((h.asked, h.after_ms, h.text_len), (DEFAULT_TIMES, 0, 0));
        // The delay, the count and the match, together and apart.
        assert_eq!(parse("window=shot+1").unwrap().after_ms, 1000);
        assert_eq!(parse("window=shot+0.25").unwrap().after_ms, 250);
        let h = parse("window:1=shot").unwrap();
        assert_eq!((h.asked, h.left), (1, 1));
        let h = parse("window/WINWORD:2=shot+1.5").unwrap();
        assert_eq!((h.text(), h.asked, h.after_ms), (&b"WINWORD"[..], 2, 1500));
        // More than the facility keeps is cut to what it keeps.
        assert_eq!(parse("window:1000=shot").unwrap().asked, MAX_TIMES);
    }

    #[test]
    fn refuses_what_is_not_one() {
        // An event or an action that does not exist, rather than a hook
        // that quietly fires on something else.
        assert!(parse("process=shot").is_none());
        assert!(parse("window=dump").is_none());
        // A part that is missing, or is not a number.
        assert!(parse("window").is_none());
        assert!(parse("=shot").is_none());
        assert!(parse("window:=shot").is_none());
        assert!(parse("window:many=shot").is_none());
        assert!(parse("window:0=shot").is_none());
        assert!(parse("window=shot+").is_none());
        assert!(parse("window=shot+soon").is_none());
        let long = "w".repeat(MATCH_LEN + 1);
        assert!(parse(&format!("window/{long}=shot")).is_none());
    }

    #[test]
    fn parses_a_delay_in_milliseconds() {
        assert_eq!(parse_ms("0"), Some(0));
        assert_eq!(parse_ms("1"), Some(1000));
        assert_eq!(parse_ms("1.5"), Some(1500));
        assert_eq!(parse_ms("0.001"), Some(1));
        assert_eq!(parse_ms(".5"), Some(500));
        assert_eq!(parse_ms("60"), Some(MAX_AFTER_MS));
        // Past the limit, or finer than the log carries: refused rather
        // than rounded, so the delay that was asked for is the one that
        // happens.
        assert_eq!(parse_ms("61"), None);
        assert_eq!(parse_ms("1.0005"), None);
        assert_eq!(parse_ms("-1"), None);
        assert_eq!(parse_ms(""), None);
        assert_eq!(parse_ms("."), None);
        assert_eq!(parse_ms("1.x"), None);
    }

    #[test]
    fn the_match_is_a_fragment_of_either_name() {
        let hook = parse("window/winword=shot").unwrap();
        assert!(hook.wants(&utf16("WINWORD.EXE"), &utf16("")));
        // The title counts too: a program that opens many windows is
        // told apart by them, and Word's splash has no title at all.
        let hook = parse("window/Document1=shot").unwrap();
        assert!(hook.wants(&utf16("WINWORD.EXE"), &utf16("Document1 - Word")));
        assert!(!hook.wants(&utf16("WINWORD.EXE"), &utf16("")));
        // With no match text every window is wanted.
        assert!(parse("window=shot").unwrap().wants(&utf16(""), &utf16("")));
    }

    #[test]
    fn a_fragment_is_a_run_of_characters_either_case() {
        assert!(contains_ignore_case(&utf16("WINWORD.EXE"), b"winword"));
        assert!(contains_ignore_case(&utf16("winword.exe"), b"WORD.EXE"));
        assert!(contains_ignore_case(&utf16("notepad.exe"), b"notepad.exe"));
        assert!(!contains_ignore_case(&utf16("notepad.exe"), b"winword"));
        // Not a fragment of itself the wrong way round, and never an
        // empty one (a hook with no match text does not come here).
        assert!(!contains_ignore_case(&utf16("word"), b"winword.exe"));
        assert!(!contains_ignore_case(&utf16("word"), b""));
    }

    #[test]
    fn a_name_is_the_program_and_the_window() {
        assert_eq!(format!("{}", Name::of(&utf16("WINWORD.EXE"), 3)), "winword-exe-w3");
        assert_eq!(format!("{}", Name::of(&utf16("gui_test.exe"), 1)), "gui-test-exe-w1");
        // A program with no name at all still names a file.
        assert_eq!(format!("{}", Name::of(&utf16(""), 7)), "window-w7");
        assert_eq!(format!("{}", Name::of(&utf16("..."), 7)), "window-w7");
        // A long name is cut, with the window it ends in kept.
        let long = "a".repeat(80);
        let name = format!("{}", Name::of(&utf16(&long), 12));
        assert!(name.ends_with("-w12") && name.len() <= NAME_LEN);
    }
}
