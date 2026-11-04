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
}

/// Decodes `%XX` escapes (so values can carry spaces and quotes: `%20`,
/// `%22`, `%25` for `%` itself); malformed escapes stay as they are.
pub fn percent_decode(
    s: &str,
    out: &mut alloc::vec::Vec<u8>,
) -> Result<(), crate::mm::heap::AllocError> {
    use crate::mm::heap::TryVec;
    let b = s.as_bytes();
    let hex = |c: u8| (c as char).to_digit(16).map(|d| d as u8);
    let mut i = 0;
    while i < b.len() {
        let decoded = (b[i] == b'%')
            .then(|| Some(hex(*b.get(i + 1)?)? << 4 | hex(*b.get(i + 2)?)?))
            .flatten();
        match decoded {
            Some(byte) => {
                out.try_push(byte)?;
                i += 3;
            }
            None => {
                out.try_push(b[i])?;
                i += 1;
            }
        }
    }
    Ok(())
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
    fn empty() {
        let c = Cmdline::new("");
        assert!(!c.flag(""));
        assert_eq!(c.value("a"), None);
    }
}
