//! Kernel command line: whitespace-separated `flag` and `key=value` words.

#[derive(Clone, Copy, Debug)]
pub struct Cmdline<'a>(&'a str);

impl<'a> Cmdline<'a> {
    pub fn new(raw: &'a str) -> Self {
        Self(raw)
    }

    pub fn as_str(&self) -> &'a str {
        self.0
    }

    /// Whether the bare word `name` is present.
    pub fn flag(&self, name: &str) -> bool {
        self.0.split_ascii_whitespace().any(|w| w == name)
    }

    /// The value of the last `key=value` word, if any.
    pub fn value(&self, key: &str) -> Option<&'a str> {
        self.0
            .split_ascii_whitespace()
            .rev()
            .filter_map(|w| w.split_once('='))
            .find(|&(k, _)| k == key)
            .map(|(_, v)| v)
    }

    /// Every `key=value` word with this key, in the order they are
    /// written. [`value`] answers with the last one, which is what a
    /// setting wants (a later word corrects an earlier one); a facility
    /// that takes more than one of something at once (`hook=`, which
    /// arms up to four) wants them all.
    ///
    /// [`value`]: Cmdline::value
    pub fn values(&self, key: &'a str) -> impl Iterator<Item = &'a str> + use<'a> {
        self.0
            .split_ascii_whitespace()
            .filter_map(|w| w.split_once('='))
            .filter(move |&(k, _)| k == key)
            .map(|(_, v)| v)
    }
}

/// The bytes `s` means with its `%XX` escapes decoded (so a value can
/// carry spaces and quotes: `%20`, `%22`, `%25` for `%` itself); a
/// malformed escape stays as it was written. The rules are here once,
/// for the two sinks below.
fn decoded_bytes(s: &str) -> impl Iterator<Item = u8> + use<'_> {
    let b = s.as_bytes();
    let hex = |c: u8| (c as char).to_digit(16).map(|d| d as u8);
    let mut i = 0;
    core::iter::from_fn(move || {
        let written = *b.get(i)?;
        let decoded = (written == b'%')
            .then(|| Some(hex(*b.get(i + 1)?)? << 4 | hex(*b.get(i + 2)?)?))
            .flatten();
        i += if decoded.is_some() { 3 } else { 1 };
        Some(decoded.unwrap_or(written))
    })
}

/// Decodes `%XX` escapes into a growing vector.
pub fn percent_decode(
    s: &str,
    out: &mut alloc::vec::Vec<u8>,
) -> Result<(), crate::mm::heap::AllocError> {
    use crate::mm::heap::TryVec;
    for byte in decoded_bytes(s) {
        out.try_push(byte)?;
    }
    Ok(())
}

/// Decodes `%XX` escapes into a buffer a facility already has, for an
/// option it keeps in one of its own and reads with interrupts off
/// (`stack_match=`): the decoded length, or None when the value does
/// not fit, which is refused rather than cut to length -- a filter
/// silently shortened would read as the one that was asked for.
pub fn percent_decode_fixed(s: &str, out: &mut [u8]) -> Option<usize> {
    let mut len = 0;
    for byte in decoded_bytes(s) {
        *out.get_mut(len)? = byte;
        len += 1;
    }
    Some(len)
}

/// Why `shell_words` failed.
#[derive(Debug, PartialEq, Eq)]
pub enum WordsError {
    UnclosedQuote,
    OutOfMemory,
}

impl From<crate::mm::heap::AllocError> for WordsError {
    fn from(_: crate::mm::heap::AllocError) -> Self {
        WordsError::OutOfMemory
    }
}

/// Splits `text` into words as a POSIX shell does plain words: blanks
/// separate them; '...' keeps everything inside; "..." too, except that
/// a backslash escapes `"` and `\` there; outside quotes a backslash
/// escapes the next byte; an empty quoted word ('') is a word. For
/// `lxargs=` and `lxenv=` (a Linux program's argv and environment).
pub fn shell_words(
    text: &[u8],
    out: &mut alloc::vec::Vec<alloc::vec::Vec<u8>>,
) -> Result<(), WordsError> {
    use crate::mm::heap::TryVec;
    let blank = |c: u8| c == b' ' || c == b'\t';
    let mut i = 0;
    loop {
        while i < text.len() && blank(text[i]) {
            i += 1;
        }
        if i == text.len() {
            return Ok(());
        }
        let mut word = alloc::vec::Vec::new();
        while i < text.len() && !blank(text[i]) {
            match text[i] {
                b'\'' => {
                    let len = text[i + 1..].iter().position(|&c| c == b'\'');
                    let close = i + 1 + len.ok_or(WordsError::UnclosedQuote)?;
                    word.try_extend_from_slice(&text[i + 1..close])?;
                    i = close + 1;
                }
                b'"' => {
                    i += 1;
                    loop {
                        match text.get(i) {
                            None => return Err(WordsError::UnclosedQuote),
                            Some(b'"') => break,
                            Some(b'\\') if matches!(text.get(i + 1), Some(b'"' | b'\\')) => {
                                word.try_push(text[i + 1])?;
                                i += 2;
                            }
                            Some(&c) => {
                                word.try_push(c)?;
                                i += 1;
                            }
                        }
                    }
                    i += 1;
                }
                b'\\' if i + 1 < text.len() => {
                    word.try_push(text[i + 1])?;
                    i += 2;
                }
                c => {
                    word.try_push(c)?;
                    i += 1;
                }
            }
        }
        out.try_push(word)?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_words_split_like_a_shell() {
        let words = |s: &str| {
            let mut out = alloc::vec::Vec::new();
            shell_words(s.as_bytes(), &mut out).map(|()| out)
        };
        let w = words(r#"./auxv_test first '' 'third arg'"#).unwrap();
        assert_eq!(w, [&b"./auxv_test"[..], b"first", b"", b"third arg"]);
        let w = words(r#"  A=1 EMPTY= LONGER_NAME='some value' "q\"x" a\ b "#).unwrap();
        assert_eq!(w, [&b"A=1"[..], b"EMPTY=", b"LONGER_NAME=some value", b"q\"x", b"a b"]);
        assert!(words("").unwrap().is_empty());
        assert_eq!(words("'open"), Err(WordsError::UnclosedQuote));
        assert_eq!(words("\"open"), Err(WordsError::UnclosedQuote));
    }

    #[test]
    fn percent_decoding() {
        let mut v = alloc::vec::Vec::new();
        percent_decode("alpha%20%22beta%20gamma%22%25%2x%", &mut v).unwrap();
        assert_eq!(v, b"alpha \"beta gamma\"%%2x%");
    }

    #[test]
    fn percent_decoding_into_a_buffer_that_is_already_there() {
        let mut buf = [0u8; 32];
        // The same rules as the vector above, to the same bytes.
        let len = percent_decode_fixed("alpha%20%22beta%20gamma%22%25%2x%", &mut buf).unwrap();
        assert_eq!(&buf[..len], b"alpha \"beta gamma\"%%2x%");
        // An escape is one byte of the length, not three.
        assert_eq!(percent_decode_fixed("k32%20two", &mut buf), Some(7));
        assert_eq!(&buf[..7], b"k32 two");
        assert_eq!(percent_decode_fixed("", &mut buf), Some(0));
        // What does not fit is refused, not cut to length.
        assert_eq!(percent_decode_fixed("abcdef", &mut buf[..5]), None);
        assert_eq!(percent_decode_fixed("ab%20c", &mut buf[..4]), Some(4));
    }

    #[test]
    fn flags_and_values() {
        let c = Cmdline::new("  qemu_exit fault=ud  selftest fault=pagefault x= =y");
        assert!(c.flag("qemu_exit"));
        assert!(c.flag("selftest"));
        assert!(!c.flag("fault"));
        assert!(!c.flag("qemu"));
        assert_eq!(c.value("fault"), Some("pagefault"));
        assert_eq!(c.value("x"), Some(""));
        assert_eq!(c.value(""), Some("y"));
        assert_eq!(c.value("missing"), None);
    }

    #[test]
    fn a_key_written_more_than_once() {
        let c = Cmdline::new("hook=window=shot fault=ud hook=window/three:1=shot+1");
        // The last one is the value; all of them are the values, in the
        // order the command line writes them.
        assert_eq!(c.value("hook"), Some("window/three:1=shot+1"));
        let all: alloc::vec::Vec<_> = c.values("hook").collect();
        assert_eq!(all, ["window=shot", "window/three:1=shot+1"]);
        let one: alloc::vec::Vec<_> = c.values("fault").collect();
        assert_eq!(one, ["ud"]);
        assert_eq!(c.values("missing").count(), 0);
    }

    #[test]
    fn empty() {
        let c = Cmdline::new("");
        assert!(!c.flag(""));
        assert_eq!(c.value("a"), None);
    }
}
