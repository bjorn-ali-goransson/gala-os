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

#[cfg(test)]
mod tests {
    use super::*;

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
