//! The YAML subset winget manifests use (YAML 1.2 block style, as the
//! winget-pkgs validation pipeline and komac write them):
//!
//! - block mappings (`Key: value`, `Key:` with a nested block), block
//!   sequences (`- item`, `- Key: value` starting a mapping), a sequence
//!   under a key at the key's own indentation (`Tags:` then `- cli`);
//! - scalars: plain (several lines fold into one with spaces; ` #` starts
//!   a comment), single-quoted (`''` is a quote), double-quoted (the
//!   common escapes), block literals and folded blocks (`|`, `|-`, `|+`,
//!   `>`, `>-`, an indentation digit);
//! - flow sequences of scalars (`[a, b, "c"]`) and empty flow mappings;
//! - comments, `---` and `...` document markers (one document).
//!
//! Not supported (an error names the line): anchors and aliases, tags,
//! complex keys, nested flow collections, tabs in indentation. Every
//! scalar stays text: manifests' numbers and booleans are read by the
//! fields that want them (`manifest`).

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::fmt;

/// A parsed node.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Value {
    Null,
    Str(String),
    Seq(Vec<Value>),
    /// Keys in document order.
    Map(Vec<(String, Value)>),
}

impl Value {
    /// The value of `key` in a mapping (exact match first, then any case:
    /// hand-written manifests get key case wrong and winget accepts it).
    pub fn get(&self, key: &str) -> Option<&Value> {
        let Value::Map(entries) = self else { return None };
        entries
            .iter()
            .find(|(k, _)| k == key)
            .or_else(|| entries.iter().find(|(k, _)| k.eq_ignore_ascii_case(key)))
            .map(|(_, v)| v)
    }

    /// A scalar's text (None for null, sequences and mappings).
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }

    /// `get(key)` as text, empty strings as None.
    pub fn str(&self, key: &str) -> Option<&str> {
        self.get(key).and_then(Value::as_str).filter(|s| !s.is_empty())
    }

    /// A sequence's items (empty for anything else).
    pub fn items(&self) -> &[Value] {
        match self {
            Value::Seq(v) => v,
            _ => &[],
        }
    }
}

/// A parse error: the line (1-based) and what was wrong.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    pub line: usize,
    pub message: &'static str,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "line {}: {}", self.line, self.message)
    }
}

#[derive(Clone)]
struct Line<'a> {
    /// 1-based line number in the document.
    number: usize,
    indent: usize,
    /// The text after the indentation (comments are stripped later,
    /// knowing whether the text is a scalar).
    text: &'a str,
}

struct Parser<'a> {
    lines: Vec<Line<'a>>,
    at: usize,
}

fn err<T>(line: usize, message: &'static str) -> Result<T, Error> {
    Err(Error { line, message })
}

/// Parses one document.
pub fn parse(text: &str) -> Result<Value, Error> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut lines = Vec::new();
    for (i, raw) in text.split('\n').enumerate() {
        let raw = raw.strip_suffix('\r').unwrap_or(raw);
        let trimmed = raw.trim_start_matches(' ');
        let indent = raw.len() - trimmed.len();
        if trimmed.starts_with('\t') && !trimmed.trim().is_empty() {
            return err(i + 1, "a tab in the indentation");
        }
        if indent == 0 && (trimmed == "---" || trimmed.starts_with("--- ") || trimmed == "...") {
            if lines.iter().any(|l: &Line| !is_blank(l.text)) && trimmed != "---" {
                break;
            }
            continue;
        }
        if trimmed.starts_with('%') && indent == 0 {
            continue; // a directive (%YAML 1.2)
        }
        lines.push(Line { number: i + 1, indent, text: trimmed.trim_end_matches([' ', '\t']) });
    }
    let mut p = Parser { lines, at: 0 };
    p.skip_blank();
    if p.at >= p.lines.len() {
        return Ok(Value::Null);
    }
    let indent = p.lines[p.at].indent;
    let v = p.node(indent)?;
    p.skip_blank();
    if p.at < p.lines.len() {
        return err(p.lines[p.at].number, "text after the document's top level");
    }
    Ok(v)
}

/// A blank line or a comment line.
fn is_blank(text: &str) -> bool {
    text.is_empty() || text.starts_with('#')
}

fn is_seq_item(text: &str) -> bool {
    text == "-" || text.starts_with("- ")
}

/// The `key: value` split of a mapping line: (key, value text), or None
/// when the line is not a mapping entry.
fn split_key(text: &str) -> Option<(String, &str)> {
    if let Some(q @ ('"' | '\'')) = text.chars().next() {
        let (key, len) = quoted(text, q).ok()?;
        let rest = &text[len..];
        let rest = rest.strip_prefix(':')?;
        if rest.is_empty() || rest.starts_with(' ') {
            return Some((key, rest.trim_start()));
        }
        return None;
    }
    if text.starts_with(['[', '{', '#', '|', '>', '&', '*', '!']) {
        return None;
    }
    let bytes = text.as_bytes();
    for i in 0..bytes.len() {
        if bytes[i] == b':' && (i + 1 == bytes.len() || bytes[i + 1] == b' ') {
            let key = text[..i].trim_end();
            if key.is_empty() {
                return None;
            }
            return Some((key.to_string(), text[i + 1..].trim_start()));
        }
        if bytes[i] == b'#' && i > 0 && bytes[i - 1] == b' ' {
            return None;
        }
    }
    None
}

/// A quoted scalar at the start of `text` (quote `q`): its value and the
/// length of its source text.
fn quoted(text: &str, q: char) -> Result<(String, usize), &'static str> {
    let mut out = String::new();
    let mut chars = text.char_indices().skip(1).peekable();
    while let Some((i, c)) = chars.next() {
        if c == q {
            if q == '\'' && chars.peek().map(|&(_, n)| n) == Some('\'') {
                chars.next();
                out.push('\'');
                continue;
            }
            return Ok((out, i + 1));
        }
        if q == '"' && c == '\\' {
            let Some((_, e)) = chars.next() else { return Err("an escape at the end") };
            match e {
                'n' => out.push('\n'),
                't' => out.push('\t'),
                'r' => out.push('\r'),
                '0' => out.push('\0'),
                '"' | '\\' | '/' | ' ' => out.push(e),
                'x' | 'u' | 'U' => {
                    let n = match e {
                        'x' => 2,
                        'u' => 4,
                        _ => 8,
                    };
                    let mut v = 0u32;
                    for _ in 0..n {
                        let Some((_, h)) = chars.next() else { return Err("a short escape") };
                        v = v * 16 + h.to_digit(16).ok_or("a bad hex escape")?;
                    }
                    out.push(char::from_u32(v).ok_or("a bad character escape")?);
                }
                _ => return Err("an unknown escape"),
            }
            continue;
        }
        out.push(c);
    }
    Err("an unterminated quoted scalar")
}

/// A plain scalar's text with a trailing comment removed.
fn strip_comment(text: &str) -> &str {
    let bytes = text.as_bytes();
    for i in 1..bytes.len() {
        if bytes[i] == b'#' && (bytes[i - 1] == b' ' || bytes[i - 1] == b'\t') {
            return text[..i].trim_end();
        }
    }
    if text.starts_with('#') { "" } else { text }
}

impl<'a> Parser<'a> {
    fn skip_blank(&mut self) {
        while self.at < self.lines.len() && is_blank(self.lines[self.at].text) {
            self.at += 1;
        }
    }

    /// The node starting at the current line, whose indentation is
    /// `indent`.
    fn node(&mut self, indent: usize) -> Result<Value, Error> {
        self.skip_blank();
        let Some(line) = self.lines.get(self.at).cloned() else { return Ok(Value::Null) };
        if is_seq_item(line.text) {
            return self.sequence(indent);
        }
        if split_key(line.text).is_some() {
            return self.mapping(indent);
        }
        // A scalar on its own lines (a sequence item's text, say).
        self.at += 1;
        self.inline_value(line.text, line.number, indent.saturating_sub(1))
    }

    fn sequence(&mut self, indent: usize) -> Result<Value, Error> {
        let mut items = Vec::new();
        loop {
            self.skip_blank();
            let Some(line) = self.lines.get(self.at).cloned() else { break };
            if line.indent < indent {
                break;
            }
            if line.indent > indent {
                return err(line.number, "unexpected indentation in a sequence");
            }
            if !is_seq_item(line.text) {
                break;
            }
            let rest = line.text[1..].trim_start_matches(' ');
            if is_blank(rest) {
                // The item is the nested block below (or null).
                self.at += 1;
                self.skip_blank();
                match self.lines.get(self.at) {
                    Some(next) if next.indent > indent => {
                        let next_indent = next.indent;
                        items.push(self.node(next_indent)?);
                    }
                    _ => items.push(Value::Null),
                }
                continue;
            }
            // The item's text starts at its own column: the line is read
            // again as if it began there (`- Key: value` starts a mapping
            // at that column).
            let column = line.indent + (line.text.len() - rest.len());
            self.lines[self.at] = Line { number: line.number, indent: column, text: rest };
            if is_seq_item(rest) || split_key(rest).is_some() {
                items.push(self.node(column)?);
            } else {
                self.at += 1;
                items.push(self.inline_value(rest, line.number, indent)?);
            }
        }
        Ok(Value::Seq(items))
    }

    fn mapping(&mut self, indent: usize) -> Result<Value, Error> {
        let mut entries: Vec<(String, Value)> = Vec::new();
        loop {
            self.skip_blank();
            let Some(line) = self.lines.get(self.at).cloned() else { break };
            if line.indent < indent {
                break;
            }
            if line.indent > indent {
                return err(line.number, "unexpected indentation in a mapping");
            }
            if is_seq_item(line.text) {
                break;
            }
            let Some((key, rest)) = split_key(line.text) else {
                return err(line.number, "expected `key: value`");
            };
            if entries.iter().any(|(k, _)| *k == key) {
                return err(line.number, "a key given twice");
            }
            self.at += 1;
            let value = if is_blank(rest) {
                self.skip_blank();
                match self.lines.get(self.at) {
                    Some(next) if next.indent > indent => {
                        let next_indent = next.indent;
                        self.node(next_indent)?
                    }
                    // `Key:` with its sequence at the key's own column.
                    Some(next) if next.indent == indent && is_seq_item(next.text) => {
                        self.sequence(indent)?
                    }
                    _ => Value::Null,
                }
            } else {
                self.inline_value(rest, line.number, indent)?
            };
            entries.push((key, value));
        }
        Ok(Value::Map(entries))
    }

    /// The value written after `key:` or `- ` on line `number`; following
    /// lines indented more than `parent` may continue it.
    fn inline_value(&mut self, text: &str, number: usize, parent: usize) -> Result<Value, Error> {
        match text.chars().next() {
            Some('|' | '>') => return self.block_scalar(text, number, parent),
            Some(q @ ('"' | '\'')) => {
                let (s, len) = quoted(text, q).or_else(|m| err(number, m))?;
                if !is_blank(text[len..].trim_start()) {
                    return err(number, "text after a quoted scalar");
                }
                return Ok(Value::Str(s));
            }
            Some('[') => return flow_sequence(text, number),
            Some('{') => {
                if strip_comment(text).replace(' ', "") == "{}" {
                    return Ok(Value::Map(Vec::new()));
                }
                return err(number, "flow mappings are not supported");
            }
            Some('&' | '*' | '!') => {
                return err(number, "anchors, aliases and tags are not supported");
            }
            _ => {}
        }
        let mut s = String::from(strip_comment(text));
        // A plain scalar continues on more indented lines.
        while let Some(next) = self.lines.get(self.at) {
            if next.indent <= parent || is_blank(next.text) {
                break;
            }
            if split_key(next.text).is_some() || is_seq_item(next.text) {
                return err(next.number, "unexpected indentation");
            }
            s.push(' ');
            s.push_str(strip_comment(next.text));
            self.at += 1;
        }
        if s == "~" || s == "null" || s == "Null" || s == "NULL" {
            return Ok(Value::Null);
        }
        Ok(Value::Str(s))
    }

    /// `|` or `>` with its chomping and indentation indicators; the text
    /// is the following lines indented more than `parent`.
    fn block_scalar(&mut self, header: &str, number: usize, parent: usize) -> Result<Value, Error> {
        let header = strip_comment(header);
        let folded = header.starts_with('>');
        let (mut keep, mut strip, mut explicit) = (false, false, None);
        for c in header[1..].chars() {
            match c {
                '-' => strip = true,
                '+' => keep = true,
                '1'..='9' => explicit = c.to_digit(10).map(|d| parent + d as usize),
                _ => return err(number, "a bad block scalar header"),
            }
        }
        // The block's lines: blank ones, or indented more than the parent.
        let start = self.at;
        let mut end = start;
        while let Some(l) = self.lines.get(end) {
            if l.text.is_empty() || l.indent > parent {
                end += 1;
            } else {
                break;
            }
        }
        // Comment-looking lines inside the block are text.
        let block_indent = explicit.unwrap_or_else(|| {
            self.lines[start..end]
                .iter()
                .find(|l| !l.text.is_empty())
                .map_or(parent + 1, |l| l.indent)
        });
        let mut out = String::new();
        let mut prev_more_indented = false;
        let mut pending_newlines = 0usize;
        let mut first = true;
        for l in &self.lines[start..end] {
            if l.text.is_empty() {
                pending_newlines += 1;
                continue;
            }
            if l.indent < block_indent {
                return err(l.number, "a block scalar line indented less than its first");
            }
            let mut text = String::new();
            for _ in block_indent..l.indent {
                text.push(' ');
            }
            text.push_str(l.text);
            let more_indented = l.indent > block_indent;
            if !first {
                if folded && pending_newlines == 0 && !more_indented && !prev_more_indented {
                    out.push(' ');
                } else {
                    let extra = if folded && !more_indented && !prev_more_indented { 0 } else { 1 };
                    for _ in 0..pending_newlines + extra {
                        out.push('\n');
                    }
                }
            } else {
                for _ in 0..pending_newlines {
                    out.push('\n');
                }
            }
            out.push_str(&text);
            pending_newlines = 0;
            prev_more_indented = more_indented;
            first = false;
        }
        self.at = end;
        if !out.is_empty() {
            if strip {
            } else if keep {
                for _ in 0..=pending_newlines {
                    out.push('\n');
                }
            } else {
                out.push('\n');
            }
        }
        Ok(Value::Str(out))
    }
}

/// `[a, b, "c"]` on one line: a sequence of scalars.
fn flow_sequence(text: &str, number: usize) -> Result<Value, Error> {
    let mut items = Vec::new();
    let mut rest = text[1..].trim_start();
    loop {
        if let Some(after) = rest.strip_prefix(']') {
            if !is_blank(after.trim_start()) {
                return err(number, "text after a flow sequence");
            }
            return Ok(Value::Seq(items));
        }
        let (item, len) = match rest.chars().next() {
            Some(q @ ('"' | '\'')) => quoted(rest, q).or_else(|m| err(number, m))?,
            Some('[' | '{') => return err(number, "nested flow collections are not supported"),
            Some(_) => {
                let end = rest
                    .find([',', ']'])
                    .ok_or(Error { line: number, message: "an unterminated flow sequence" })?;
                (rest[..end].trim_end().to_string(), end)
            }
            None => return err(number, "an unterminated flow sequence"),
        };
        items.push(Value::Str(item));
        rest = rest[len..].trim_start();
        if let Some(after) = rest.strip_prefix(',') {
            rest = after.trim_start();
        } else if !rest.starts_with(']') {
            return err(number, "expected `,` or `]` in a flow sequence");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn s(v: &str) -> Value {
        Value::Str(v.into())
    }

    #[test]
    fn mappings_and_sequences() {
        let v = parse(
            "# comment\nPackageIdentifier: A.B # trailing\nTags:\n- cli\n- 'grep'\nInstallers:\n- Architecture: x64\n  NestedInstallerFiles:\n  - RelativeFilePath: d/rg.exe\n    PortableCommandAlias: rg\n  InstallerUrl: https://x/y.zip\n- Architecture: x86\nEmpty:\nManifestVersion: 1.12.0\n",
        )
        .unwrap();
        assert_eq!(v.str("PackageIdentifier"), Some("A.B"));
        assert_eq!(v.get("Tags").unwrap().items(), &[s("cli"), s("grep")]);
        let inst = v.get("Installers").unwrap().items();
        assert_eq!(inst.len(), 2);
        assert_eq!(inst[0].str("InstallerUrl"), Some("https://x/y.zip"));
        let nested = inst[0].get("NestedInstallerFiles").unwrap().items();
        assert_eq!(nested[0].str("PortableCommandAlias"), Some("rg"));
        assert_eq!(nested[0].str("RelativeFilePath"), Some("d/rg.exe"));
        assert_eq!(inst[1].str("Architecture"), Some("x86"));
        assert_eq!(v.get("Empty"), Some(&Value::Null));
        assert_eq!(v.str("ManifestVersion"), Some("1.12.0"));
    }

    #[test]
    fn scalars() {
        let v = parse(
            "a: \"q \\\"x\\\" \\u00e9\"\nb: 'it''s'\nc: plain text\n  continued here\nd: [x, \"y z\", 'w']\ne: {}\nf: http://h/#frag\ng: ~\n",
        )
        .unwrap();
        assert_eq!(v.str("a"), Some("q \"x\" \u{e9}"));
        assert_eq!(v.str("b"), Some("it's"));
        assert_eq!(v.str("c"), Some("plain text continued here"));
        assert_eq!(v.get("d").unwrap().items(), &[s("x"), s("y z"), s("w")]);
        assert_eq!(v.get("e"), Some(&Value::Map(vec![])));
        assert_eq!(v.str("f"), Some("http://h/#frag"));
        assert_eq!(v.get("g"), Some(&Value::Null));
    }

    #[test]
    fn block_scalars() {
        let v = parse(
            "lit: |\n  one\n   two\n\n  three\nstrip: |-\n  x\n  y\nfold: >-\n  a\n  b\n\n  c\nkeep: |+\n  k\n\nnext: 1\n",
        )
        .unwrap();
        assert_eq!(v.str("lit"), Some("one\n two\n\nthree\n"));
        assert_eq!(v.str("strip"), Some("x\ny"));
        assert_eq!(v.str("fold"), Some("a b\nc"));
        assert_eq!(v.str("keep"), Some("k\n\n"));
        assert_eq!(v.str("next"), Some("1"));
    }

    #[test]
    fn block_scalar_with_markdown_lines() {
        // Release notes often have `- ` and `#` lines; they stay text.
        let v = parse("Notes: |-\n  # Title\n  - item: one\n  > quote\nAfter: x\n").unwrap();
        assert_eq!(v.str("Notes"), Some("# Title\n- item: one\n> quote"));
        assert_eq!(v.str("After"), Some("x"));
    }

    #[test]
    fn nested_sequences_and_documents() {
        let v = parse("---\nk:\n  - - a\n    - b\n  -\n    x: 1\n...\n").unwrap();
        let k = v.get("k").unwrap().items();
        assert_eq!(k[0].items(), &[s("a"), s("b")]);
        assert_eq!(k[1].str("x"), Some("1"));
    }

    #[test]
    fn errors_name_the_line() {
        assert_eq!(parse("a: 1\na: 2\n").unwrap_err().line, 2);
        assert_eq!(parse("a: *ref\n").unwrap_err().line, 1);
        assert_eq!(parse("a: 1\n\tb: 2\n").unwrap_err().line, 2);
        assert_eq!(parse("a: 'open\n").unwrap_err().line, 1);
        assert_eq!(parse("a: 1\n   b: 2\n").unwrap_err().line, 2);
    }

    #[test]
    fn crlf_and_bom() {
        let v = parse("\u{feff}A: 1\r\nB:\r\n- x\r\n").unwrap();
        assert_eq!(v.str("A"), Some("1"));
        assert_eq!(v.get("B").unwrap().items(), &[s("x")]);
    }
}
