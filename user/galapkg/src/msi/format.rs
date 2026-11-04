//! The Windows Installer's Formatted data type, written from the SDK's
//! "Formatted" page (learn.microsoft.com/windows/win32/msi/formatted):
//!
//! - `[name]`: the property's value; blank when it is not set or the
//!   name is not a valid property name;
//! - `[[name]]`: iterated from the inside out (the inner value names
//!   the property whose value is used);
//! - `[%name]`: an environment variable;
//! - `[\x]`: the character `x` alone (`[\[]` is `[`; anything after the
//!   first character is dropped);
//! - `[~]`: the NUL character (REG_MULTI_SZ lists in the Registry table);
//! - `[#filekey]`: the file's full path (its target when its component
//!   is installed locally, its source when run from source; blank
//!   otherwise); `[!filekey]`: its short path, in the Registry and
//!   IniFile tables' Value columns only (elsewhere it reads as
//!   `[#filekey]`); `[$componentkey]`: the component's directory by its
//!   action state;
//! - `{...}` with no bracketed reference inside: left as it is, braces
//!   included; with references, all of them resolved: the text without
//!   the braces. When a reference inside does not resolve, the page
//!   says nothing more; here the group is then blank (TODO: check on
//!   Windows);
//! - brackets and braces with no matching pair stay in the text.

use alloc::string::String;
use alloc::vec::Vec;

/// What formatting looks up.
pub trait Resolver {
    fn property(&self, name: &str) -> Option<String>;
    fn environment(&self, _name: &str) -> Option<String> {
        None
    }
    /// `[#key]`: a file's full path (None: blank).
    fn file_path(&self, _key: &str) -> Option<String> {
        None
    }
    /// `[!key]`: a file's short path.
    fn file_short_path(&self, key: &str) -> Option<String> {
        self.file_path(key)
    }
    /// `[$key]`: a component's directory.
    fn component_directory(&self, _key: &str) -> Option<String> {
        None
    }
}

impl Resolver for alloc::collections::BTreeMap<String, String> {
    fn property(&self, name: &str) -> Option<String> {
        self.get(name).cloned()
    }
}

/// Where the text comes from (the `[!key]` rule).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Column {
    /// The Registry or IniFile table's Value column.
    RegistryValue,
    Other,
}

/// A valid property name: a letter or `_`, then letters, digits, `_`
/// and `.`.
pub fn is_property_name(s: &str) -> bool {
    let mut c = s.chars();
    c.next().is_some_and(|f| f.is_ascii_alphabetic() || f == '_')
        && c.all(|x| x.is_ascii_alphanumeric() || x == '_' || x == '.')
}

struct Formatter<'r> {
    r: &'r dyn Resolver,
    column: Column,
}

/// The result of a bracketed reference.
enum Ref {
    /// Resolved (escapes and `[~]` included).
    Value(String),
    /// A reference that did not resolve: blank.
    Missing,
    /// `[\x]` or `[~]`: a character, not a property name.
    Char(char),
}

impl Formatter<'_> {
    /// Formats `c[from..to]`; returns the text and whether every
    /// reference in it resolved and how many references there were.
    fn run(&self, c: &[char], from: usize, to: usize) -> (String, bool, usize) {
        let mut out = String::new();
        let (mut ok, mut refs) = (true, 0);
        let mut i = from;
        while i < to {
            match c[i] {
                '[' => match self.bracket(c, i, to) {
                    Some((r, end)) => {
                        match r {
                            Ref::Value(v) => {
                                refs += 1;
                                out.push_str(&v);
                            }
                            Ref::Missing => {
                                refs += 1;
                                ok = false;
                            }
                            Ref::Char(ch) => out.push(ch),
                        }
                        i = end;
                    }
                    None => {
                        out.push('[');
                        i += 1;
                    }
                },
                '{' => match matching(c, i, to, '{', '}') {
                    Some(close) => {
                        let (inner, all, n) = self.run(c, i + 1, close);
                        if n == 0 {
                            out.extend(&c[i..=close]);
                        } else if all {
                            refs += n;
                            out.push_str(&inner);
                        } else {
                            refs += n;
                        }
                        i = close + 1;
                    }
                    None => {
                        out.push('{');
                        i += 1;
                    }
                },
                ch => {
                    out.push(ch);
                    i += 1;
                }
            }
        }
        (out, ok, refs)
    }

    /// The reference starting with `[` at `at`: its value and the index
    /// after its `]`; None when it has no closing bracket.
    fn bracket(&self, c: &[char], at: usize, to: usize) -> Option<(Ref, usize)> {
        // [\x]: the first character after the backslash, the rest up to
        // the closing bracket dropped.
        if c.get(at + 1) == Some(&'\\') && at + 2 < to {
            let x = c[at + 2];
            let close = (at + 3..to).find(|&j| c[j] == ']')?;
            return Some((Ref::Char(x), close + 1));
        }
        // The content, with nested references resolved first.
        let mut name = String::new();
        let mut i = at + 1;
        let mut nested_missing = false;
        while i < to {
            match c[i] {
                ']' => {
                    if nested_missing {
                        return Some((Ref::Missing, i + 1));
                    }
                    return Some((self.resolve(&name), i + 1));
                }
                '[' => {
                    let (r, end) = self.bracket(c, i, to)?;
                    match r {
                        Ref::Value(v) => name.push_str(&v),
                        Ref::Char(ch) => name.push(ch),
                        Ref::Missing => nested_missing = true,
                    }
                    i = end;
                }
                ch => {
                    name.push(ch);
                    i += 1;
                }
            }
        }
        None
    }

    fn resolve(&self, name: &str) -> Ref {
        if name == "~" {
            return Ref::Char('\0');
        }
        let v = if let Some(k) = name.strip_prefix('%') {
            self.r.environment(k)
        } else if let Some(k) = name.strip_prefix('#') {
            self.r.file_path(k)
        } else if let Some(k) = name.strip_prefix('!') {
            match self.column {
                Column::RegistryValue => self.r.file_short_path(k),
                Column::Other => self.r.file_path(k),
            }
        } else if let Some(k) = name.strip_prefix('$') {
            self.r.component_directory(k)
        } else if is_property_name(name) {
            self.r.property(name)
        } else {
            None
        };
        match v {
            Some(v) => Ref::Value(v),
            None => Ref::Missing,
        }
    }
}

/// The index of the `close` matching the `open` at `at`, nested pairs
/// counted.
fn matching(c: &[char], at: usize, to: usize, open: char, close: char) -> Option<usize> {
    let mut depth = 0usize;
    for (j, &ch) in c.iter().enumerate().take(to).skip(at) {
        if ch == open {
            depth += 1;
        } else if ch == close {
            depth -= 1;
            if depth == 0 {
                return Some(j);
            }
        }
    }
    None
}

/// Formats `text` (see the module documentation).
pub fn format(text: &str, r: &dyn Resolver, column: Column) -> String {
    let c: Vec<char> = text.chars().collect();
    Formatter { r, column }.run(&c, 0, c.len()).0
}

#[cfg(test)]
mod tests;
