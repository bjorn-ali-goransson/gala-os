//! The event stream: one JSON object per line on the debug console
//! (`arch::debugcon`), for tools rather than people (docs/OBSERVABILITY.md
//! section 1). Off by default; `events=syscall,image,thread,process,fault`
//! (or `events=all`) on the kernel command line turns categories on. Each
//! line starts with `"t"` (ns since boot), `"cpu"` and `"ev"`; the rest
//! depends on the event.

use core::fmt::{self, Write};
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Category {
    Syscall = 1,
    Image = 2,
    Thread = 4,
    Process = 8,
    Fault = 16,
}

const NAMES: [(&str, u32); 5] = [
    ("syscall", Category::Syscall as u32),
    ("image", Category::Image as u32),
    ("thread", Category::Thread as u32),
    ("process", Category::Process as u32),
    ("fault", Category::Fault as u32),
];

static MASK: AtomicU32 = AtomicU32::new(0);
static WRITTEN: AtomicU64 = AtomicU64::new(0);

/// The category mask for an `events=` value (unknown names are ignored).
pub fn parse_mask(value: &str) -> u32 {
    value
        .split(',')
        .map(|name| match name {
            "all" => u32::MAX,
            _ => NAMES.iter().find(|(n, _)| *n == name).map_or(0, |&(_, bit)| bit),
        })
        .fold(0, |m, bit| m | bit)
}

pub fn set_mask(mask: u32) {
    MASK.store(mask, Ordering::Relaxed);
}

/// Whether events of `category` are wanted (one relaxed load).
pub fn enabled(category: Category) -> bool {
    MASK.load(Ordering::Relaxed) & category as u32 != 0
}

/// Lines written so far.
pub fn written() -> u64 {
    WRITTEN.load(Ordering::Relaxed)
}

/// Writes one event: `{"t":..,"cpu":..,"ev":"<name>",<fields>}`. `fields`
/// are the remaining JSON members (without braces); strings in them must
/// be wrapped in [`Str`].
#[cfg(not(test))]
pub fn emit(category: Category, name: &str, fields: fmt::Arguments) {
    if !enabled(category) {
        return;
    }
    let t = crate::time::monotonic_ns();
    let cpu = crate::arch::percpu::cpu_index();
    let mut port = crate::arch::debugcon::DEBUGCON.lock();
    let _ = writeln!(port, r#"{{"t":{t},"cpu":{cpu},"ev":"{name}",{fields}}}"#);
    WRITTEN.fetch_add(1, Ordering::Relaxed);
}

/// Host unit tests have no debug console.
#[cfg(test)]
pub fn emit(_category: Category, _name: &str, _fields: fmt::Arguments) {}

/// A string escaped for JSON (quotes, backslashes, control characters).
pub struct Str<'a>(pub &'a str);

impl fmt::Display for Str<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_char('"')?;
        for c in self.0.chars() {
            match c {
                '"' => f.write_str("\\\"")?,
                '\\' => f.write_str("\\\\")?,
                c if (c as u32) < 0x20 => write!(f, "\\u{:04x}", c as u32)?,
                c => f.write_char(c)?,
            }
        }
        f.write_char('"')
    }
}

/// A UTF-16 string (e.g. an NT path) as a JSON string.
pub struct Utf16<'a>(pub &'a [u16]);

impl fmt::Display for Utf16<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_char('"')?;
        for c in char::decode_utf16(self.0.iter().copied()) {
            match c.unwrap_or(char::REPLACEMENT_CHARACTER) {
                '"' => f.write_str("\\\"")?,
                '\\' => f.write_str("\\\\")?,
                c if (c as u32) < 0x20 => write!(f, "\\u{:04x}", c as u32)?,
                c => f.write_char(c)?,
            }
        }
        f.write_char('"')
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masks() {
        assert_eq!(parse_mask("syscall,fault"), 17);
        assert_eq!(parse_mask("all"), u32::MAX);
        assert_eq!(parse_mask("nonsense"), 0);
    }

    #[test]
    fn escaping() {
        assert_eq!(alloc::format!("{}", Str("a\"b\\c\n")), r#""a\"b\\c\u000a""#);
        let path: alloc::vec::Vec<u16> = "C:\\x".encode_utf16().collect();
        assert_eq!(alloc::format!("{}", Utf16(&path)), r#""C:\\x""#);
    }
}
