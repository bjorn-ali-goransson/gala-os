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

#[cfg(test)]
mod tests {
    use super::*;

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
