//! The Linux personality's ABI data (ROADMAP LX, ARCHITECTURE "The lx_abi
//! crate"): what a Linux x86_64 program sees of the kernel as plain data,
//! for the signal, terminal and file-system calls of LX milestones 2 and
//! 3.
//!
//! - [`signal`]: signal numbers, `sa_flags`, si_code values, 64-bit signal
//!   sets (SIGKILL and SIGSTOP unblockable), the default action of every
//!   signal (signal(7)), and the layouts of `siginfo_t`, `stack_t`, the
//!   signal context (`struct sigcontext`, the ucontext's `uc_mcontext`),
//!   the ucontext and the x86_64 rt_sigframe.
//! - [`termios`]: termios flags, `c_cc` indexes, a fresh pty's settings,
//!   cfmakeraw, `struct winsize`, the tty ioctl numbers.
//! - [`layout`]: `struct stat`, `struct statx`, `linux_dirent64`,
//!   `epoll_event` (packed on x86_64), `timespec`, `timeval`, `pollfd`,
//!   `rusage`.
//!
//! Sources (CLEAN_ROOM.md "Linux"; THIRD_PARTY.md): the man-pages
//! (signal(7), sigaction(2), termios(3), stat(2), statx(2),
//! getdents64(2), epoll_ctl(2), poll(2), getrusage(2)), the System V
//! AMD64 psABI, musl 1.2.4's headers read as data, and the transcripts of
//! gala's Linux test programs recorded on Linux 6.18: every layout and
//! default here is checked by a host test against what
//! `user/lx/<name>_test/expected.txt` shows real Linux doing (offsets the
//! programs observed, sizes of what the kernel wrote, a fresh pty's
//! termios, each signal's default action applied to a child). The Linux
//! kernel source was not used.
//!
//! A separate crate, not a module of `kernel/src/lx`, so that it builds
//! and is tested on the host without touching the kernel's module tree;
//! the kernel depends on it when the Linux system calls are built.
//! `no_std`, no allocation, no `unsafe`.
#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]
// The host tests may allocate (format! keys); the library never does.
#![cfg_attr(test, allow(clippy::disallowed_methods, clippy::disallowed_macros))]

pub mod layout;
pub mod signal;
pub mod termios;

/// Test support: reading values out of a recorded transcript.
#[cfg(test)]
pub(crate) mod transcript {
    /// The value of the first line `<key> = <value>` in `text`; decimal,
    /// negative, `0x` hex or a leading `0` octal number, followed by
    /// anything (an errno's name).
    pub fn value(text: &str, key: &str) -> i64 {
        for line in text.lines() {
            if let Some(rest) = line.strip_prefix(key).and_then(|r| r.strip_prefix(" = ")) {
                return number(rest).unwrap_or_else(|| panic!("{key}: not a number: {rest:?}"));
            }
        }
        panic!("no line {key:?} in the transcript");
    }

    /// The text after `<key> = ` or `<key>: ` on the first such line.
    pub fn text<'a>(text: &'a str, key: &str) -> &'a str {
        for line in text.lines() {
            if let Some(rest) = line.strip_prefix(key)
                && let Some(v) = rest.strip_prefix(" = ").or_else(|| rest.strip_prefix(": "))
            {
                return v;
            }
        }
        panic!("no line {key:?} in the transcript");
    }

    pub fn number(s: &str) -> Option<i64> {
        let word = s.split([' ', ',']).next()?;
        let (neg, digits) = match word.strip_prefix('-') {
            Some(d) => (true, d),
            None => (false, word),
        };
        let v = if let Some(h) = digits.strip_prefix("0x") {
            u64::from_str_radix(h, 16).ok()? as i64
        } else if digits.len() > 1 && digits.starts_with('0') {
            i64::from_str_radix(&digits[1..], 8).ok()?
        } else {
            digits.parse().ok()?
        };
        Some(if neg { -v } else { v })
    }

    #[test]
    fn numbers() {
        assert_eq!(number("12"), Some(12));
        assert_eq!(number("-22 (EINVAL)"), Some(-22));
        assert_eq!(number("0x8a3b, oflag"), Some(0x8a3b));
        assert_eq!(number("0644"), Some(0o644));
        assert_eq!(number("0"), Some(0));
        assert_eq!(number("yes"), None);
    }
}
