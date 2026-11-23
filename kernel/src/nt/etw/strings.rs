//! What the bytes of an event nothing describes carried.
//!
//! A TraceLogging event brings the name, type and shape of every field
//! with it, so `tracelogging` reads it field by field. A manifest
//! provider's event brings none of that: the names are in a
//! `WEVT_TEMPLATE` resource in some other file, and what reaches the
//! kernel is numbers and bytes. Every event Word's startup writes that
//! a session keeps is one of these, the error it writes before it gives
//! up among them (ROADMAP OF), and a line that says only how many
//! bytes there were tells nobody anything.
//!
//! So the line says what the bytes carried. A payload's text -- a
//! message, a path, a name, a reason -- is readable without knowing
//! which field it belongs to, and it is the part of an event a person
//! can act on. The rest (which number was in which field) is what the
//! manifest would be needed for, and this module does not guess at it:
//! an event of numbers alone is reported as holding no string, which is
//! itself a finding and not a failure to read.
//!
//! Nothing here allocates or keeps a buffer: the strings are written
//! straight to the line's formatter, out of the record already on the
//! kernel's frame.

use core::fmt;

/// How many strings a line names before it counts the rest. Enough for
/// an error event's message, module and path together, few enough that
/// one event cannot push the lines around it out of a log.
const SHOW: usize = 6;

/// How many characters of one string a line gives. An Office event's
/// message is a sentence and then the JSON of the values behind it
/// (`BootRun {"SH_ErrorId": 13, "BootTask": "IfrRegisterWndClasses"}`),
/// and the values are the half worth reading; a serialized blob still
/// does not get the whole line.
const ONE: usize = 160;

/// The shortest run that is text rather than a coincidence. Three
/// printable bytes happen inside lengths, timestamps and GUIDs all the
/// time; four with a letter or digit in them rarely do.
const MIN: usize = 4;

/// The strings in an event's bytes, as a line's worth of them.
pub struct In<'a> {
    bytes: &'a [u8],
    /// Whether the record was cut to fit, so the line says the strings
    /// are those of the bytes that arrived and not of the event.
    cut: bool,
}

impl<'a> In<'a> {
    pub fn new(bytes: &'a [u8], cut: bool) -> Self {
        Self { bytes, cut }
    }
}

/// Whether a byte is a character text is written with: printable ASCII,
/// the space included. A tab or a newline inside a payload is as likely
/// to be a length as a character, so neither counts.
fn printable(b: u8) -> bool {
    (0x20..=0x7e).contains(&b)
}

/// The run of text at `at`, if there is one: how many characters it has,
/// how many bytes they took, and whether it was UTF-16LE.
///
/// Wide is tried first, because a wide string's first character is a
/// printable byte and a nul: read 8-bit, a message would be reported as
/// its first letter alone. A run must hold a letter or a digit, so that
/// padding, runs of dots and the printable halves of numbers are not
/// reported as text.
fn run(bytes: &[u8], at: usize) -> Option<(usize, usize, bool)> {
    let mut chars = 0;
    let mut said = false;
    let mut i = at;
    while i + 1 < bytes.len() && printable(bytes[i]) && bytes[i + 1] == 0 {
        said |= bytes[i].is_ascii_alphanumeric();
        chars += 1;
        i += 2;
    }
    if chars >= MIN && said {
        return Some((chars, chars * 2, true));
    }
    let mut chars = 0;
    let mut said = false;
    let mut i = at;
    while i < bytes.len() && printable(bytes[i]) {
        said |= bytes[i].is_ascii_alphanumeric();
        chars += 1;
        i += 1;
    }
    if chars >= MIN && said {
        return Some((chars, chars, false));
    }
    None
}

impl fmt::Display for In<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Counted before any of them is written, so the line can say
        // there are none -- and say how many it left out.
        let mut found = 0;
        let mut at = 0;
        while at < self.bytes.len() {
            match run(self.bytes, at) {
                Some((_, took, _)) => {
                    found += 1;
                    at += took;
                }
                None => at += 1,
            }
        }
        if found == 0 {
            return if self.cut {
                write!(f, "no string in the bytes that fit")
            } else {
                write!(f, "no string in its bytes")
            };
        }
        if self.cut {
            write!(f, "strings in the bytes that fit:")?;
        } else {
            write!(f, "strings:")?;
        }
        let mut shown = 0;
        let mut at = 0;
        while at < self.bytes.len() && shown < SHOW {
            let Some((chars, took, wide)) = run(self.bytes, at) else {
                at += 1;
                continue;
            };
            write!(f, "{}\"", if shown == 0 { " " } else { ", " })?;
            let step = if wide { 2 } else { 1 };
            for &b in self.bytes[at..at + took].iter().step_by(step).take(ONE) {
                // A quote or a backslash would end the string the line
                // puts it in, or start an escape in whatever reads the
                // line next.
                if b == b'"' || b == b'\\' {
                    write!(f, "\\")?;
                }
                write!(f, "{}", b as char)?;
            }
            // Inside the quotes, so that the dots are plainly the
            // line's and not the string's own.
            if chars > ONE {
                write!(f, "...")?;
            }
            write!(f, "\"")?;
            shown += 1;
            at += took;
        }
        if found > shown {
            write!(f, " and {} more", found - shown)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The bytes of an ASCII text as UTF-16LE, which is what a
    /// program's own message is in.
    fn wide(s: &str) -> std::vec::Vec<u8> {
        let mut out = std::vec::Vec::new();
        for b in s.bytes() {
            out.push(b);
            out.push(0);
        }
        out
    }

    fn line(bytes: &[u8]) -> std::string::String {
        std::format!("{}", In::new(bytes, false))
    }

    #[test]
    fn a_wide_message_reads_whole() {
        // The shape of an error event: a message, then the code behind
        // it, whose bytes are not text and are not reported as any.
        let mut bytes = wide("a manifest event's message");
        bytes.extend_from_slice(&13u32.to_le_bytes());
        assert_eq!(line(&bytes), "strings: \"a manifest event's message\"");
    }

    #[test]
    fn eight_bit_text_reads_too() {
        let mut bytes = std::vec::Vec::from(&b"eight bit text"[..]);
        bytes.extend_from_slice(&[0xde, 0xad, 0xbe, 0xef]);
        assert_eq!(line(&bytes), "strings: \"eight bit text\"");
    }

    #[test]
    fn a_wide_string_is_not_read_as_its_first_letter() {
        // Read 8-bit, "text" is four runs of one character; read wide,
        // it is one string. Wide is tried first, so it is one string.
        assert_eq!(line(&wide("text")), "strings: \"text\"");
    }

    #[test]
    fn numbers_hold_no_string() {
        assert_eq!(line(&[1, 2, 3, 4, 5, 6, 7, 8]), "no string in its bytes");
    }

    #[test]
    fn a_run_shorter_than_four_is_not_a_string() {
        // "abc" between bytes that are not text: three characters is
        // what a length or a timestamp produces by accident.
        assert_eq!(line(b"\x01abc\x02"), "no string in its bytes");
        assert_eq!(line(b"\x01abcd\x02"), "strings: \"abcd\"");
    }

    #[test]
    fn padding_is_not_a_string() {
        // Spaces and dots are printable and say nothing, so a run needs
        // a letter or a digit in it.
        assert_eq!(line(b"\x01      \x02......\x03"), "no string in its bytes");
    }

    #[test]
    fn one_string_is_cut_and_says_so() {
        let text = "abcdefghij".repeat(24);
        let got = line(&wide(&text));
        assert_eq!(got, std::format!("strings: \"{}...\"", &text[..ONE]));
    }

    #[test]
    fn the_strings_past_the_sixth_are_counted() {
        let mut bytes = std::vec::Vec::new();
        for n in 1..=8 {
            bytes.extend_from_slice(&wide(&std::format!("piece-0{n}")));
            bytes.extend_from_slice(&[0, 0]);
        }
        assert_eq!(
            line(&bytes),
            "strings: \"piece-01\", \"piece-02\", \"piece-03\", \"piece-04\", \"piece-05\", \"piece-06\" and 2 more"
        );
    }

    #[test]
    fn a_quote_in_a_string_is_escaped() {
        assert_eq!(line(b"say \"this\" now"), "strings: \"say \\\"this\\\" now\"");
        assert_eq!(line(b"C:\\Windows\\win.ini"), "strings: \"C:\\\\Windows\\\\win.ini\"");
    }

    #[test]
    fn a_cut_record_says_the_strings_are_of_what_fit() {
        let bytes = wide("a message");
        assert_eq!(
            std::format!("{}", In::new(&bytes, true)),
            "strings in the bytes that fit: \"a message\""
        );
        assert_eq!(
            std::format!("{}", In::new(&[1, 2, 3, 4], true)),
            "no string in the bytes that fit"
        );
    }

    #[test]
    fn no_bytes_at_all_hold_no_string() {
        assert_eq!(line(&[]), "no string in its bytes");
    }
}
