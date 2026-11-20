//! QEMU's debug console (`-debugcon`): bytes written to port 0xe9 go to a
//! host file. Carries the machine-readable event stream (`crate::events`),
//! separate from the human log on the serial port. On hardware without the
//! device the writes go nowhere.
//!
//! A line at a time, not a byte at a time. Each port write leaves the
//! virtual machine for its hypervisor, about 10 us nested under WSL2, so
//! an event written byte by byte cost about 1.9 ms and made a traced run
//! ten to thirty times its own length. The bytes of a line are held in
//! [`Line`] until its newline and then written in one `rep outsb`
//! (`port::Port::write_all`), one exit for the line: what the stream costs
//! is now its lines and not its bytes. OBSERVABILITY "What it costs".

use core::sync::atomic::{AtomicU64, Ordering};

use super::port::Port;
use crate::sync::IrqMutex;

/// How much of a line is held. An event is 100 to 300 bytes; a longer one
/// (a recording's hex) is written in pieces of this size, which costs one
/// more exit each and loses nothing.
const LINE: usize = 1024;

/// The port, locked so that lines from different CPUs do not interleave.
pub static DEBUGCON: IrqMutex<DebugCon> = IrqMutex::new(DebugCon {
    // SAFETY: port 0xe9 is QEMU's debug console (or nothing); writing it
    // cannot affect memory, and this static is its only user.
    port: unsafe { Port::new(0xe9) },
    line: Line::new(),
});

/// How many writes of the port those lines took, for the summary: the
/// ratio to the events written is what the buffer is for.
static WRITES: AtomicU64 = AtomicU64::new(0);

/// The number of writes the device has taken.
pub fn writes() -> u64 {
    WRITES.load(Ordering::Relaxed)
}

/// Writes what is held of a line that has no newline yet, so that a run's
/// last bytes reach the host (the summary at the end of a run calls this).
pub fn flush() {
    let DebugCon { port, line } = &mut *DEBUGCON.lock();
    line.flush(&mut |bytes| {
        port.write_all(bytes);
        WRITES.fetch_add(1, Ordering::Relaxed);
    });
}

/// The bytes of a line, held until the line ends. Knows nothing of the
/// port, so the host tests can count the writes it asks for.
pub struct Line {
    held: [u8; LINE],
    len: usize,
}

impl Default for Line {
    fn default() -> Self {
        Self::new()
    }
}

impl Line {
    pub const fn new() -> Self {
        Self { held: [0; LINE], len: 0 }
    }

    /// Takes `s`, calling `out` once for each line it completes (the
    /// newline included) and once whenever what is held fills the buffer.
    /// Bytes after the last newline are held for the next call, a line
    /// being written in as few writes as it fits in.
    pub fn write(&mut self, s: &str, out: &mut impl FnMut(&[u8])) {
        let mut rest = s.as_bytes();
        while !rest.is_empty() {
            let room = self.held.len() - self.len;
            let take = match rest.iter().position(|&b| b == b'\n') {
                Some(end) if end < room => end + 1,
                _ => room.min(rest.len()),
            };
            let (head, tail) = rest.split_at(take);
            self.held[self.len..self.len + take].copy_from_slice(head);
            self.len += take;
            if self.len == self.held.len() || head.last() == Some(&b'\n') {
                out(&self.held[..self.len]);
                self.len = 0;
            }
            rest = tail;
        }
    }

    /// Writes an unfinished line, if any.
    pub fn flush(&mut self, out: &mut impl FnMut(&[u8])) {
        if self.len > 0 {
            out(&self.held[..self.len]);
            self.len = 0;
        }
    }
}

pub struct DebugCon {
    port: Port<u8>,
    line: Line,
}

impl core::fmt::Write for DebugCon {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let Self { port, line } = self;
        line.write(s, &mut |bytes| {
            port.write_all(bytes);
            WRITES.fetch_add(1, Ordering::Relaxed);
        });
        Ok(())
    }
}

/// The port needs no configuring (aarch64's file does: `events_file=`).
pub fn configure(_cmdline: &crate::cmdline::Cmdline<'_>) {}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every write the buffer asked for, in order: a test counts them,
    /// which is what the buffer changes. (Timing one would measure the
    /// host and not the code.)
    fn written(pieces: &[&str]) -> Vec<String> {
        let mut line = Line::new();
        let mut out: Vec<String> = Vec::new();
        for piece in pieces {
            line.write(piece, &mut |bytes| out.push(String::from_utf8_lossy(bytes).into_owned()));
        }
        line.flush(&mut |bytes| out.push(String::from_utf8_lossy(bytes).into_owned()));
        out
    }

    #[test]
    fn one_line_is_one_write() {
        let event = r#"{"t":1,"cpu":0,"by":4,"ev":"import"}"#;
        assert_eq!(written(&[event, "\n"]), [format!("{event}\n")]);
    }

    #[test]
    fn the_pieces_of_a_line_are_one_write() {
        // writeln! calls write_str once a piece of its format string, and
        // emit_record once a hex byte: the buffer is what joins them.
        let pieces = [r#"{"t":"#, "1", r#","ev":"rec","hex":""#, "ab", "cd", r#""}"#, "\n"];
        assert_eq!(written(&pieces), [r#"{"t":1,"ev":"rec","hex":"abcd"}"#.to_string() + "\n"]);
    }

    #[test]
    fn two_lines_in_one_piece_are_two_writes() {
        let got = written(&["{\"a\":1}\n{\"b\":2}\n"]);
        assert_eq!(got, ["{\"a\":1}\n", "{\"b\":2}\n"]);
    }

    #[test]
    fn bytes_without_a_newline_are_held() {
        let mut line = Line::new();
        let mut writes = 0;
        line.write(r#"{"t":1,"ev":"import""#, &mut |_| writes += 1);
        assert_eq!(writes, 0);
        line.write("}\n", &mut |_| writes += 1);
        assert_eq!(writes, 1);
    }

    #[test]
    fn a_line_longer_than_the_buffer_arrives_in_pieces() {
        let long = format!("{}\n", "x".repeat(LINE * 2 + 7));
        let got = written(&[&long]);
        assert_eq!(got.len(), 3);
        assert_eq!(got[0].len(), LINE);
        assert_eq!(got[1].len(), LINE);
        assert_eq!(got.concat(), long);
    }

    #[test]
    fn a_stream_of_events_costs_one_write_an_event() {
        let mut line = Line::new();
        let (mut writes, mut bytes) = (0u64, 0usize);
        for t in 0..1000u64 {
            let event = format!(
                r#"{{"t":{t},"cpu":0,"by":4,"ev":"import","call":"KERNEL32.dll!GetProcAddress","args":[140699891695616,5368717405,7,0]}}"#
            );
            bytes += event.len() + 1;
            let mut count = |b: &[u8]| {
                writes += 1;
                assert_eq!(b.last(), Some(&b'\n'));
            };
            line.write(&event, &mut count);
            line.write("\n", &mut count);
        }
        // One a line, where a byte at a time would have been 125 000.
        assert_eq!(writes, 1000);
        assert!(bytes > 100_000, "{bytes}");
    }
}
