//! The YAML subset winget manifests are written in (YAML 1.2.2,
//! yaml.org/spec/1.2.2, as komac, wingetcreate, YamlCreate.ps1 and
//! hand-editors write manifests), with every node's line and column so
//! that errors, and the manifest reader's own complaints, point at the
//! text:
//!
//! - a byte-order mark, `%` directives, a leading `---`, a closing `...`
//!   (one document: a second `---` is an error), comments anywhere
//!   (`# yaml-language-server: $schema=...` is one), CRLF line ends;
//! - block mappings and block sequences, including the compact forms
//!   (`- Key: value` starts a mapping at the key's column; `Key:` followed
//!   by `- item` at the key's own column);
//! - flow sequences and flow mappings (`[a, "b"]`, `{}`, `{ k: v }`,
//!   nested, across lines, a trailing comma);
//! - plain scalars (several lines folded into one, ` #` starts a
//!   comment), single-quoted (`''`), double-quoted (YAML's escapes,
//!   `\` line continuation), both across lines with YAML's folding;
//! - block scalars `|` and `>` with chomping (`-`, `+`) and indentation
//!   indicators.
//!
//! Not supported, each an [`Error`] at its position: anchors, aliases,
//! tags, `?` complex keys, tabs in indentation, more than one document.
//! Keys must be unique (YAML requires it; winget rejects duplicates).
//! Scalars stay text, except a plain `~`, `null`, `Null`, `NULL` or
//! nothing, which are [`Value::Null`]: the manifest reader reads numbers
//! and booleans from the fields that want them.
//!
//! Separate from `crate::yaml` (which `crate::manifest` and the install
//! path use) so that the install path is unchanged until it switches to
//! `crate::winget` (ARCHITECTURE "Installing software", "The winget
//! resolver").

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

/// A position in the text: 1-based line and column (columns count
/// characters, not bytes).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Pos {
    pub line: u32,
    pub col: u32,
}

impl fmt::Display for Pos {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.line, self.col)
    }
}

/// A node and where it starts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Node {
    pub value: Value,
    pub at: Pos,
}

/// A mapping entry: the key, where the key is, the value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub key: String,
    pub at: Pos,
    pub value: Node,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Value {
    Null,
    Str(String),
    Seq(Vec<Node>),
    /// In document order.
    Map(Vec<Entry>),
}

impl Node {
    /// The entry for `key` in a mapping: the exact key first, then any
    /// case (hand-written manifests get key case wrong; winget's reader
    /// matches field names without regard to case).
    pub fn entry(&self, key: &str) -> Option<&Entry> {
        let Value::Map(entries) = &self.value else { return None };
        entries
            .iter()
            .find(|e| e.key == key)
            .or_else(|| entries.iter().find(|e| e.key.eq_ignore_ascii_case(key)))
    }

    /// The value of `key`, None when absent or null.
    pub fn get(&self, key: &str) -> Option<&Node> {
        self.entry(key).map(|e| &e.value).filter(|n| n.value != Value::Null)
    }

    /// A scalar's text.
    pub fn as_str(&self) -> Option<&str> {
        match &self.value {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }

    /// A sequence's items (empty for anything else).
    pub fn items(&self) -> &[Node] {
        match &self.value {
            Value::Seq(v) => v,
            _ => &[],
        }
    }

    /// A mapping's entries (empty for anything else).
    pub fn entries(&self) -> &[Entry] {
        match &self.value {
            Value::Map(v) => v,
            _ => &[],
        }
    }

    /// What kind of node this is, for messages.
    pub fn kind(&self) -> &'static str {
        match self.value {
            Value::Null => "null",
            Value::Str(_) => "a scalar",
            Value::Seq(_) => "a sequence",
            Value::Map(_) => "a mapping",
        }
    }
}

/// A parse error and where it is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    pub at: Pos,
    pub message: &'static str,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.at, self.message)
    }
}

type R<T> = Result<T, Error>;

/// Nesting deeper than this is refused (manifests nest four deep; the
/// limit keeps a hostile manifest from exhausting the stack).
const MAX_DEPTH: usize = 64;

struct P<'a> {
    s: &'a str,
    b: &'a [u8],
    i: usize,
    line: u32,
    line_start: usize,
    depth: usize,
}

fn is_ws(c: u8) -> bool {
    c == b' ' || c == b'\t'
}

fn is_flow_indicator(c: u8) -> bool {
    matches!(c, b',' | b'[' | b']' | b'{' | b'}')
}

/// Parses one document.
pub fn parse(text: &str) -> R<Node> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut p = P { s: text, b: text.as_bytes(), i: 0, line: 1, line_start: 0, depth: 0 };
    p.prologue()?;
    p.skip_to_content()?;
    let at = p.pos();
    let node = if p.at_document_end() {
        Node { value: Value::Null, at }
    } else {
        let col = p.col0();
        p.block_node(col, None)?
    };
    p.skip_to_content()?;
    if p.at_marker(b"...") {
        p.i += 3;
        p.skip_line_rest()?;
        p.skip_to_content()?;
    }
    if p.at_marker(b"---") {
        return Err(p.err("a second document (one manifest is one document)"));
    }
    if p.i < p.b.len() {
        return Err(p.err("text after the document's top-level node"));
    }
    Ok(node)
}

impl P<'_> {
    fn pos(&self) -> Pos {
        let col = self.s[self.line_start..self.i].chars().count() as u32 + 1;
        Pos { line: self.line, col }
    }

    fn err(&self, message: &'static str) -> Error {
        Error { at: self.pos(), message }
    }

    /// The 0-based column of the cursor (indentation is spaces only, so
    /// bytes and characters agree where it matters).
    fn col0(&self) -> usize {
        self.i - self.line_start
    }

    fn peek(&self) -> Option<u8> {
        self.b.get(self.i).copied()
    }

    fn peek_at(&self, n: usize) -> Option<u8> {
        self.b.get(self.i + n).copied()
    }

    fn newline(&mut self) {
        // At a `\n` (a preceding `\r` already consumed or skipped).
        self.i += 1;
        self.line += 1;
        self.line_start = self.i;
    }

    fn at_eol(&self) -> bool {
        match self.peek() {
            None | Some(b'\n') => true,
            Some(b'\r') => self.peek_at(1).is_none_or(|c| c == b'\n'),
            _ => false,
        }
    }

    /// Consumes the line end at the cursor, if any.
    fn eat_eol(&mut self) -> bool {
        if self.peek() == Some(b'\r') && self.peek_at(1) == Some(b'\n') {
            self.i += 1;
        }
        if self.peek() == Some(b'\n') {
            self.newline();
            return true;
        }
        if self.peek() == Some(b'\r') {
            // A lone CR at the end of the text.
            self.i += 1;
        }
        false
    }

    /// `---` or `...` at column 0, followed by a space or the line end.
    fn at_marker(&self, m: &[u8]) -> bool {
        self.col0() == 0
            && self.b[self.i..].starts_with(m)
            && self.b.get(self.i + 3).is_none_or(|&c| is_ws(c) || c == b'\n' || c == b'\r')
    }

    fn at_document_end(&self) -> bool {
        self.i >= self.b.len() || self.at_marker(b"...") || self.at_marker(b"---")
    }

    /// Directives and the `---` that starts the document.
    fn prologue(&mut self) -> R<()> {
        loop {
            self.skip_to_content()?;
            if self.col0() == 0 && self.peek() == Some(b'%') {
                self.skip_line_rest_raw();
                continue;
            }
            if self.at_marker(b"---") {
                self.i += 3;
                // `--- ` may be followed by a comment; content on the
                // marker's line is not used by manifests.
                self.skip_ws();
                if !(self.at_eol() || self.peek() == Some(b'#')) {
                    return Err(self.err("content on the `---` line"));
                }
                self.skip_line_rest_raw();
            }
            return Ok(());
        }
    }

    fn skip_ws(&mut self) {
        while self.peek().is_some_and(is_ws) {
            self.i += 1;
        }
    }

    /// The rest of the line, whatever it is, and its line end.
    fn skip_line_rest_raw(&mut self) {
        while !self.at_eol() {
            self.i += 1;
        }
        self.eat_eol();
    }

    /// Spaces, an optional comment, then the line end; anything else is an
    /// error.
    fn skip_line_rest(&mut self) -> R<()> {
        self.skip_ws();
        if self.peek() == Some(b'#') || self.at_eol() {
            self.skip_line_rest_raw();
            return Ok(());
        }
        Err(self.err("unexpected text at the end of the line"))
    }

    /// Moves to the next content character: past spaces, comments and
    /// blank lines. Leaves the cursor at the content (its column is the
    /// line's indentation) or at the end of the text. A tab before the
    /// content of a line is an error (YAML indents with spaces only).
    fn skip_to_content(&mut self) -> R<()> {
        loop {
            let line_begin = self.i == self.line_start;
            while self.peek() == Some(b' ') {
                self.i += 1;
            }
            if self.peek() == Some(b'\t') {
                let tab = self.i;
                self.skip_ws();
                if !(self.at_eol() || self.peek() == Some(b'#')) && line_begin {
                    self.i = tab;
                    return Err(self.err("a tab in the indentation"));
                }
            }
            match self.peek() {
                Some(b'#') => self.skip_line_rest_raw(),
                Some(b'\n' | b'\r') => {
                    if !self.eat_eol() && self.at_eol() {
                        return Ok(());
                    }
                }
                _ => return Ok(()),
            }
        }
    }

    fn enter(&mut self) -> R<()> {
        self.depth += 1;
        if self.depth > MAX_DEPTH { Err(self.err("nesting too deep")) } else { Ok(()) }
    }

    /// `-` followed by a space or the line end: a block sequence item.
    fn at_seq_item(&self) -> bool {
        self.peek() == Some(b'-')
            && self.peek_at(1).is_none_or(|c| is_ws(c) || c == b'\n' || c == b'\r')
    }

    /// Whether the line from the cursor is `key:` (block context): a
    /// quoted key or plain text up to `:` followed by a space or the line
    /// end, before any comment.
    fn at_key(&self) -> bool {
        let b = self.b;
        let mut j = self.i;
        match b.get(j) {
            Some(&q @ (b'"' | b'\'')) => {
                j += 1;
                while j < b.len() && b[j] != b'\n' {
                    if b[j] == b'\\' && q == b'"' {
                        j += 2;
                        continue;
                    }
                    if b[j] == q {
                        if q == b'\'' && b.get(j + 1) == Some(&b'\'') {
                            j += 2;
                            continue;
                        }
                        j += 1;
                        while b.get(j).is_some_and(|&c| is_ws(c)) {
                            j += 1;
                        }
                        return b.get(j) == Some(&b':')
                            && b.get(j + 1).is_none_or(|&c| is_ws(c) || c == b'\n' || c == b'\r');
                    }
                    j += 1;
                }
                false
            }
            Some(
                b'[' | b'{' | b'#' | b'|' | b'>' | b'&' | b'*' | b'!' | b'?' | b'%' | b'@' | b'`',
            ) => false,
            _ => {
                while j < b.len() && b[j] != b'\n' {
                    if b[j] == b':'
                        && b.get(j + 1).is_none_or(|&c| is_ws(c) || c == b'\n' || c == b'\r')
                    {
                        return j > self.i;
                    }
                    if b[j] == b'#' && j > self.i && is_ws(b[j - 1]) {
                        return false;
                    }
                    j += 1;
                }
                false
            }
        }
    }

    /// The node at the cursor (a content character in column `col`),
    /// in a block whose parent is indented `parent` (None at the top).
    fn block_node(&mut self, col: usize, parent: Option<usize>) -> R<Node> {
        if self.at_seq_item() {
            return self.block_sequence(col);
        }
        if self.at_key() {
            return self.block_mapping(col);
        }
        self.inline_node(parent)
    }

    fn block_sequence(&mut self, indent: usize) -> R<Node> {
        self.enter()?;
        let at = self.pos();
        let mut items = Vec::new();
        loop {
            // At `-` in column `indent`.
            let item_at = self.pos();
            self.i += 1;
            self.skip_ws();
            let item = if self.peek() == Some(b'#') || self.at_eol() {
                self.skip_line_rest_raw();
                self.nested_block(indent, item_at)?
            } else {
                let col = self.col0();
                self.block_node(col, Some(indent))?
            };
            items.push(item);
            self.skip_to_content()?;
            if self.at_document_end() {
                break;
            }
            let col = self.col0();
            if col < indent {
                break;
            }
            if col > indent {
                return Err(self.err("unexpected indentation in a sequence"));
            }
            if !self.at_seq_item() {
                // A key at the sequence's own column ends a compact
                // sequence under a mapping key.
                break;
            }
        }
        self.depth -= 1;
        Ok(Node { value: Value::Seq(items), at })
    }

    /// The block under `key:` or `-` with nothing after it on its line:
    /// what follows indented more than `indent`, else null.
    fn nested_block(&mut self, indent: usize, at: Pos) -> R<Node> {
        self.skip_to_content()?;
        if self.at_document_end() || self.col0() <= indent {
            return Ok(Node { value: Value::Null, at });
        }
        let col = self.col0();
        self.block_node(col, Some(indent))
    }

    fn block_mapping(&mut self, indent: usize) -> R<Node> {
        self.enter()?;
        let at = self.pos();
        let mut entries: Vec<Entry> = Vec::new();
        loop {
            let key_at = self.pos();
            let key = self.key()?;
            if entries.iter().any(|e| e.key == key) {
                return Err(Error { at: key_at, message: "a key given twice" });
            }
            // At the `:`.
            self.i += 1;
            self.skip_ws();
            let value = if self.peek() == Some(b'#') || self.at_eol() {
                self.skip_line_rest_raw();
                self.skip_to_content()?;
                if !self.at_document_end() && self.col0() == indent && self.at_seq_item() {
                    // `Key:` then `- item` at the key's column.
                    self.block_sequence(indent)?
                } else {
                    self.nested_block(indent, key_at)?
                }
            } else {
                if self.at_seq_item() {
                    return Err(self.err("a block sequence cannot start on its key's line"));
                }
                if self.at_key() {
                    return Err(self.err("a mapping cannot start on its key's line"));
                }
                self.inline_node(Some(indent))?
            };
            entries.push(Entry { key, at: key_at, value });
            self.skip_to_content()?;
            if self.at_document_end() {
                break;
            }
            let col = self.col0();
            if col < indent {
                break;
            }
            if col > indent {
                return Err(self.err("unexpected indentation in a mapping"));
            }
            if self.at_seq_item() {
                break;
            }
            if !self.at_key() {
                return Err(self.err("expected `key: value`"));
            }
        }
        self.depth -= 1;
        Ok(Node { value: Value::Map(entries), at })
    }

    /// A block mapping key (the cursor is on it; `at_key` said so). Leaves
    /// the cursor at the `:`.
    fn key(&mut self) -> R<String> {
        match self.peek() {
            Some(q @ (b'"' | b'\'')) => {
                let s = self.quoted(q, None)?;
                self.skip_ws();
                Ok(s)
            }
            _ => {
                let start = self.i;
                while !(self.peek() == Some(b':')
                    && self.peek_at(1).is_none_or(|c| is_ws(c) || c == b'\n' || c == b'\r'))
                {
                    self.i += 1;
                }
                Ok(String::from(self.s[start..self.i].trim_end()))
            }
        }
    }

    /// A value that starts on the current line: a block scalar, a flow
    /// collection, a quoted or a plain scalar. Continuation lines must be
    /// indented more than `parent`.
    fn inline_node(&mut self, parent: Option<usize>) -> R<Node> {
        let at = self.pos();
        let min = parent.map_or(0, |p| p + 1);
        let value = match self.peek() {
            Some(b'|' | b'>') => self.block_scalar(parent)?,
            Some(b'[' | b'{') => {
                let v = self.flow_node()?;
                self.skip_line_rest()?;
                return Ok(v);
            }
            Some(q @ (b'"' | b'\'')) => {
                let s = self.quoted(q, Some(min))?;
                self.skip_line_rest()?;
                Value::Str(s)
            }
            Some(b'&') => return Err(self.err("anchors are not supported")),
            Some(b'*') => return Err(self.err("aliases are not supported")),
            Some(b'!') => return Err(self.err("tags are not supported")),
            Some(b'?') if self.peek_at(1).is_none_or(|c| is_ws(c) || c == b'\n' || c == b'\r') => {
                return Err(self.err("complex keys are not supported"));
            }
            Some(b'@' | b'`') => {
                return Err(self.err("`@` and `` ` `` cannot start a plain scalar"));
            }
            Some(b'%') if parent.is_none() => {
                return Err(self.err("a directive inside the document"));
            }
            _ => self.plain_block(min)?,
        };
        Ok(Node { value, at })
    }

    /// A plain scalar in block context; continuation lines indented at
    /// least `min` fold into it (a line break is a space, blank lines are
    /// line breaks).
    fn plain_block(&mut self, min: usize) -> R<Value> {
        let mut out = String::new();
        let mut first = true;
        loop {
            // One line's text, up to ` #` or the line end; `: ` is an error.
            let start = self.i;
            while !self.at_eol() {
                let c = self.b[self.i];
                if c == b'#' && self.i > start && is_ws(self.b[self.i - 1]) {
                    break;
                }
                if c == b':' && self.peek_at(1).is_none_or(|c| is_ws(c) || c == b'\n' || c == b'\r')
                {
                    return Err(self.err("`: ` inside a plain scalar (quote the value)"));
                }
                self.i += 1;
            }
            let text = self.s[start..self.i].trim_end_matches([' ', '\t']);
            if !first || !text.is_empty() {
                out.push_str(text);
            }
            first = false;
            let commented = self.peek() == Some(b'#');
            self.skip_line_rest_raw();
            if commented {
                break;
            }
            // Continuation: the next content line, if indented enough and
            // not a comment.
            let save = (self.i, self.line, self.line_start);
            let mut breaks = 0usize;
            loop {
                while self.peek() == Some(b' ') {
                    self.i += 1;
                }
                if self.at_eol() && self.i < self.b.len() {
                    breaks += 1;
                    if !self.eat_eol() {
                        break;
                    }
                    continue;
                }
                break;
            }
            let ok = self.i < self.b.len()
                && self.col0() >= min
                && self.peek() != Some(b'#')
                && !(self.col0() == 0 && (self.at_marker(b"---") || self.at_marker(b"...")))
                && !self.at_eol();
            if !ok {
                (self.i, self.line, self.line_start) = save;
                break;
            }
            if self.at_key() || self.at_seq_item() {
                if self.col0() == min.saturating_sub(1) || min == 0 {
                    (self.i, self.line, self.line_start) = save;
                    break;
                }
                return Err(self.err("unexpected indentation"));
            }
            if breaks == 0 {
                out.push(' ');
            } else {
                for _ in 0..breaks {
                    out.push('\n');
                }
            }
        }
        if out.is_empty() || matches!(out.as_str(), "~" | "null" | "Null" | "NULL") {
            return Ok(Value::Null);
        }
        Ok(Value::Str(out))
    }

    /// A quoted scalar at the cursor; may span lines (folded as YAML
    /// folds flow scalars). `min`: the indentation continuation lines need
    /// (None: the scalar must end on its line, as a key).
    fn quoted(&mut self, q: u8, min: Option<usize>) -> R<String> {
        let open = self.pos();
        self.i += 1;
        let mut out = String::new();
        loop {
            let Some(c) = self.peek() else {
                return Err(Error { at: open, message: "an unterminated quoted scalar" });
            };
            if c == q {
                if q == b'\'' && self.peek_at(1) == Some(b'\'') {
                    out.push('\'');
                    self.i += 2;
                    continue;
                }
                self.i += 1;
                return Ok(out);
            }
            if c == b'\n' || (c == b'\r' && self.peek_at(1) == Some(b'\n')) {
                let Some(min) = min else {
                    return Err(Error { at: open, message: "a quoted key spanning lines" });
                };
                // Folding: trailing spaces dropped, a single break is a
                // space, n blank lines are n breaks.
                while out.ends_with([' ', '\t']) {
                    out.pop();
                }
                self.fold_break(&mut out, min, open)?;
                continue;
            }
            if q == b'"' && c == b'\\' {
                self.i += 1;
                if self.at_eol() && self.i < self.b.len() {
                    // An escaped line break: joined without a space.
                    let Some(min) = min else {
                        return Err(Error { at: open, message: "a quoted key spanning lines" });
                    };
                    let mut ignored = String::new();
                    self.fold_break(&mut ignored, min, open)?;
                    if ignored.starts_with('\n') {
                        out.push_str(&ignored);
                    }
                    continue;
                }
                self.escape(&mut out)?;
                continue;
            }
            let ch = self.s[self.i..].chars().next().unwrap_or('\u{fffd}');
            out.push(ch);
            self.i += ch.len_utf8();
        }
    }

    /// At a line break inside a quoted scalar: consumes it, the blank
    /// lines after it and the next line's indentation; pushes a space or
    /// the blank lines' breaks.
    fn fold_break(&mut self, out: &mut String, min: usize, open: Pos) -> R<()> {
        self.eat_eol();
        let mut blank = 0usize;
        loop {
            self.skip_ws();
            if self.i >= self.b.len() {
                return Err(Error { at: open, message: "an unterminated quoted scalar" });
            }
            if self.at_eol() {
                blank += 1;
                self.eat_eol();
                continue;
            }
            break;
        }
        if self.col0() < min && min > 0 {
            return Err(self.err("a quoted scalar's continuation line is not indented enough"));
        }
        if blank == 0 {
            out.push(' ');
        } else {
            for _ in 0..blank {
                out.push('\n');
            }
        }
        Ok(())
    }

    /// A double-quoted scalar's escape after its `\` (YAML 1.2.2 5.7).
    fn escape(&mut self, out: &mut String) -> R<()> {
        let Some(e) = self.peek() else { return Err(self.err("an escape at the end")) };
        self.i += 1;
        let c = match e {
            b'0' => '\0',
            b'a' => '\x07',
            b'b' => '\x08',
            b't' | b'\t' => '\t',
            b'n' => '\n',
            b'v' => '\x0b',
            b'f' => '\x0c',
            b'r' => '\r',
            b'e' => '\x1b',
            b' ' => ' ',
            b'"' => '"',
            b'/' => '/',
            b'\\' => '\\',
            b'N' => '\u{85}',
            b'_' => '\u{a0}',
            b'L' => '\u{2028}',
            b'P' => '\u{2029}',
            b'x' | b'u' | b'U' => {
                let n = match e {
                    b'x' => 2,
                    b'u' => 4,
                    _ => 8,
                };
                let mut v = 0u32;
                for _ in 0..n {
                    let Some(d) = self.peek().and_then(|h| (h as char).to_digit(16)) else {
                        return Err(self.err("a bad hexadecimal escape"));
                    };
                    v = v * 16 + d;
                    self.i += 1;
                }
                match char::from_u32(v) {
                    Some(c) => c,
                    None => return Err(self.err("an escape that is not a character")),
                }
            }
            _ => {
                self.i -= 1;
                return Err(self.err("an unknown escape"));
            }
        };
        out.push(c);
        Ok(())
    }

    /// `|` or `>` and its block. `parent`: the indentation of the node
    /// holding it (the indentation indicator counts from there).
    fn block_scalar(&mut self, parent: Option<usize>) -> R<Value> {
        let header = self.pos();
        let folded = self.peek() == Some(b'>');
        self.i += 1;
        let (mut chomp, mut explicit) = (0i8, None::<usize>);
        for _ in 0..2 {
            match self.peek() {
                Some(b'-') if chomp == 0 => chomp = -1,
                Some(b'+') if chomp == 0 => chomp = 1,
                Some(d @ b'1'..=b'9') if explicit.is_none() => explicit = Some((d - b'0') as usize),
                _ => break,
            }
            self.i += 1;
        }
        if !(self.at_eol() || self.peek().is_some_and(is_ws)) {
            return Err(self.err("a bad block scalar header"));
        }
        self.skip_line_rest()?;
        let base = parent.map_or(0, |p| p + 1);
        let mut indent = explicit.map(|d| parent.map_or(0, |p| p) + d);
        // The lines: None for an empty (spaces only, not past the
        // indentation) line, Some(text after the indentation).
        let mut lines: Vec<Option<&str>> = Vec::new();
        loop {
            if self.i >= self.b.len() {
                break;
            }
            let line_begin = self.i;
            let mut sp = 0;
            while self.peek_at(sp) == Some(b' ') {
                sp += 1;
            }
            let eol_at = {
                let mut j = self.i + sp;
                while j < self.b.len() && self.b[j] != b'\n' {
                    j += 1;
                }
                j
            };
            let content = self.s[self.i + sp..eol_at].trim_end_matches('\r');
            let ind = match indent {
                Some(n) => n,
                None if content.is_empty() => {
                    lines.push(None);
                    self.i = eol_at;
                    if !self.eat_eol() {
                        break;
                    }
                    continue;
                }
                None => {
                    if sp < base {
                        break;
                    }
                    indent = Some(sp);
                    sp
                }
            };
            if content.is_empty() && sp <= ind {
                lines.push(None);
            } else if sp < ind {
                if content.starts_with('\t') && sp >= base {
                    self.i = line_begin + sp;
                    return Err(self.err("a tab in a block scalar's indentation"));
                }
                self.i = line_begin;
                break;
            } else {
                let text = &self.s[line_begin + ind..eol_at];
                lines.push(Some(text.trim_end_matches('\r')));
            }
            self.i = eol_at;
            if !self.eat_eol() {
                break;
            }
        }
        if self.i < self.b.len() && self.i != self.line_start {
            // Stopped mid-line: go back to its start.
            self.i = self.line_start;
        }
        let _ = header;
        // Trailing empty lines are the chomping's.
        let mut trailing = 0;
        while matches!(lines.last(), Some(None)) {
            lines.pop();
            trailing += 1;
        }
        let mut out = String::new();
        let mut prev_normal = false;
        let mut blank = 0usize;
        let mut first = true;
        for l in &lines {
            let Some(text) = l else {
                blank += 1;
                continue;
            };
            let normal = !text.starts_with([' ', '\t']);
            if first {
                for _ in 0..blank {
                    out.push('\n');
                }
            } else if folded && normal && prev_normal {
                if blank == 0 {
                    out.push(' ');
                } else {
                    for _ in 0..blank {
                        out.push('\n');
                    }
                }
            } else {
                for _ in 0..=blank {
                    out.push('\n');
                }
            }
            out.push_str(text);
            blank = 0;
            prev_normal = normal;
            first = false;
        }
        match chomp {
            -1 => {}
            0 => {
                if !first {
                    out.push('\n');
                }
            }
            _ => {
                if !first {
                    out.push('\n');
                }
                for _ in 0..trailing {
                    out.push('\n');
                }
            }
        }
        Ok(Value::Str(out))
    }

    /// Spaces, comments and line breaks inside a flow collection.
    fn flow_space(&mut self) -> R<()> {
        loop {
            self.skip_ws();
            match self.peek() {
                Some(b'#') => self.skip_line_rest_raw(),
                Some(b'\n' | b'\r') if self.at_eol() => {
                    if !self.eat_eol() {
                        return Ok(());
                    }
                }
                None => return Err(self.err("an unterminated flow collection")),
                _ => return Ok(()),
            }
        }
    }

    /// A flow sequence or mapping at the cursor.
    fn flow_node(&mut self) -> R<Node> {
        self.enter()?;
        let at = self.pos();
        let open = self.b[self.i];
        let close = if open == b'[' { b']' } else { b'}' };
        self.i += 1;
        let mut items = Vec::new();
        let mut entries: Vec<Entry> = Vec::new();
        loop {
            self.flow_space()?;
            if self.peek() == Some(close) {
                self.i += 1;
                break;
            }
            let key_at = self.pos();
            let first = self.flow_scalar_or_collection()?;
            self.flow_space()?;
            // `key: value` (in a flow sequence: a one-entry mapping).
            let pair = if self.peek() == Some(b':') {
                self.i += 1;
                self.flow_space()?;
                let value = if matches!(self.peek(), Some(b',')) || self.peek() == Some(close) {
                    Node { value: Value::Null, at: self.pos() }
                } else {
                    self.flow_scalar_or_collection()?
                };
                Some(value)
            } else {
                None
            };
            // A flow mapping's lone key has a null value.
            if open == b'{' || pair.is_some() {
                let key = match first.value {
                    Value::Str(key) => key,
                    Value::Null => {
                        return Err(Error { at: key_at, message: "an empty flow mapping key" });
                    }
                    _ => {
                        return Err(Error {
                            at: key_at,
                            message: "complex keys are not supported",
                        });
                    }
                };
                let value = pair.unwrap_or(Node { value: Value::Null, at: key_at });
                let e = Entry { key, at: key_at, value };
                if open == b'{' {
                    if entries.iter().any(|x| x.key == e.key) {
                        return Err(Error { at: key_at, message: "a key given twice" });
                    }
                    entries.push(e);
                } else {
                    items.push(Node { value: Value::Map(alloc::vec![e]), at: key_at });
                }
            } else {
                items.push(first);
            }
            self.flow_space()?;
            match self.peek() {
                Some(b',') => self.i += 1,
                Some(c) if c == close => {}
                _ => {
                    return Err(self.err(if open == b'[' {
                        "expected `,` or `]` in a flow sequence"
                    } else {
                        "expected `,` or `}` in a flow mapping"
                    }));
                }
            }
        }
        self.depth -= 1;
        let value = if open == b'[' { Value::Seq(items) } else { Value::Map(entries) };
        Ok(Node { value, at })
    }

    fn flow_scalar_or_collection(&mut self) -> R<Node> {
        let at = self.pos();
        match self.peek() {
            Some(b'[' | b'{') => self.flow_node(),
            Some(q @ (b'"' | b'\'')) => {
                Ok(Node { value: Value::Str(self.quoted(q, Some(0))?), at })
            }
            Some(b'&') => Err(self.err("anchors are not supported")),
            Some(b'*') => Err(self.err("aliases are not supported")),
            Some(b'!') => Err(self.err("tags are not supported")),
            _ => {
                // A plain scalar in flow context: up to `,`, a flow
                // indicator, `: `/`:` before an indicator, ` #` or the
                // line end (multi-line plain scalars in flow collections
                // fold like block ones).
                let mut out = String::new();
                loop {
                    let start = self.i;
                    while !self.at_eol() {
                        let c = self.b[self.i];
                        if is_flow_indicator(c) {
                            break;
                        }
                        if c == b'#' && self.i > start && is_ws(self.b[self.i - 1]) {
                            break;
                        }
                        if c == b':'
                            && self.peek_at(1).is_none_or(|n| {
                                is_ws(n) || n == b'\n' || n == b'\r' || is_flow_indicator(n)
                            })
                        {
                            break;
                        }
                        self.i += 1;
                    }
                    let text = self.s[start..self.i].trim_end_matches([' ', '\t']);
                    if !out.is_empty() && !text.is_empty() {
                        out.push(' ');
                    }
                    out.push_str(text);
                    if !self.at_eol() || self.i >= self.b.len() {
                        break;
                    }
                    // Continue on the next line unless it starts with an
                    // indicator.
                    let save = (self.i, self.line, self.line_start);
                    self.flow_space()?;
                    match self.peek() {
                        Some(c) if is_flow_indicator(c) || c == b':' => {
                            (self.i, self.line, self.line_start) = save;
                            break;
                        }
                        _ => {}
                    }
                }
                if out.is_empty() || matches!(out.as_str(), "~" | "null" | "Null" | "NULL") {
                    return Ok(Node { value: Value::Null, at });
                }
                Ok(Node { value: Value::Str(out), at })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::ToString;
    use alloc::vec;

    fn s(n: &Node) -> &str {
        n.as_str().unwrap()
    }

    fn strs(n: &Node) -> Vec<&str> {
        n.items().iter().map(s).collect()
    }

    #[test]
    fn manifest_shape() {
        let doc = "\u{feff}# Created with komac v2.6.0\n# yaml-language-server: $schema=https://aka.ms/winget-manifest.installer.1.9.0.schema.json\n\nPackageIdentifier: A.B # trailing\nTags:\n- cli\n- 'grep'\nInstallers:\n- Architecture: x64\n  NestedInstallerFiles:\n  - RelativeFilePath: d/rg.exe\n    PortableCommandAlias: rg\n  InstallerUrl: https://x/y.zip#frag\n-   Architecture: x86\nEmpty:\nManifestType: installer\nManifestVersion: 1.9.0\n";
        let v = parse(doc).unwrap();
        assert_eq!(s(v.get("PackageIdentifier").unwrap()), "A.B");
        assert_eq!(strs(v.get("Tags").unwrap()), ["cli", "grep"]);
        let inst = v.get("Installers").unwrap().items();
        assert_eq!(inst.len(), 2);
        assert_eq!(s(inst[0].get("InstallerUrl").unwrap()), "https://x/y.zip#frag");
        let nested = inst[0].get("NestedInstallerFiles").unwrap().items();
        assert_eq!(s(nested[0].get("PortableCommandAlias").unwrap()), "rg");
        assert_eq!(s(inst[1].get("Architecture").unwrap()), "x86");
        assert_eq!(inst[1].get("Architecture").unwrap().at, Pos { line: 14, col: 19 });
        assert_eq!(v.entry("Empty").unwrap().value.value, Value::Null);
        assert!(v.get("Empty").is_none());
        assert_eq!(v.entry("ManifestVersion").unwrap().at, Pos { line: 17, col: 1 });
        // Any key case.
        assert_eq!(s(v.get("manifestversion").unwrap()), "1.9.0");
    }

    #[test]
    fn scalars_and_escapes() {
        let v = parse(
            "a: \"q \\\"x\\\" \\u00e9 \\x41\\t\\\\ \\/ \\U0001F600\"\nb: 'it''s'\nc: plain text\n  continued here\n\n  and a paragraph\nd: ~\ne: null\nf: \"null\"\ng: a:b\nh: x #c\ni: 1.10\n",
        )
        .unwrap();
        assert_eq!(s(v.get("a").unwrap()), "q \"x\" \u{e9} A\t\\ / \u{1F600}");
        assert_eq!(s(v.get("b").unwrap()), "it's");
        assert_eq!(s(v.get("c").unwrap()), "plain text continued here\nand a paragraph");
        assert!(v.get("d").is_none() && v.get("e").is_none());
        assert_eq!(s(v.get("f").unwrap()), "null");
        assert_eq!(s(v.get("g").unwrap()), "a:b");
        assert_eq!(s(v.get("h").unwrap()), "x");
        assert_eq!(s(v.get("i").unwrap()), "1.10");
    }

    #[test]
    fn multi_line_quoted() {
        let v = parse("a: \"one\n  two\n\n  three\"\nb: 'x\n  y'\nc: \"join\\\n  ed\"\nd: k\n")
            .unwrap();
        assert_eq!(s(v.get("a").unwrap()), "one two\nthree");
        assert_eq!(s(v.get("b").unwrap()), "x y");
        assert_eq!(s(v.get("c").unwrap()), "joined");
        assert_eq!(s(v.get("d").unwrap()), "k");
    }

    #[test]
    fn flow_collections() {
        let v = parse(
            "a: [x, \"y z\", 'w', ]\nb: {}\nc: []\nd: { k: v, \"q\": [1, 2], e: {} }\ne: [\n  one, # c\n  two\n]\nf: [a: b]\n",
        )
        .unwrap();
        assert_eq!(strs(v.get("a").unwrap()), ["x", "y z", "w"]);
        assert_eq!(v.get("b").unwrap().value, Value::Map(vec![]));
        assert_eq!(v.get("c").unwrap().value, Value::Seq(vec![]));
        let d = v.get("d").unwrap();
        assert_eq!(s(d.get("k").unwrap()), "v");
        assert_eq!(strs(d.get("q").unwrap()), ["1", "2"]);
        assert_eq!(strs(v.get("e").unwrap()), ["one", "two"]);
        assert_eq!(s(v.get("f").unwrap().items()[0].get("a").unwrap()), "b");
    }

    #[test]
    fn block_scalars() {
        let v = parse(
            "lit: |\n  one\n   two\n\n  three\nstrip: |-\n  x\n  y\n\nfold: >-\n  a\n  b\n\n  c\n   more\n  d\nkeep: |+\n  k\n\nclip: >\n  z\n\n\nind: |2\n    four\n  two\nempty: |\nnext: 1\n",
        )
        .unwrap();
        assert_eq!(s(v.get("lit").unwrap()), "one\n two\n\nthree\n");
        assert_eq!(s(v.get("strip").unwrap()), "x\ny");
        assert_eq!(s(v.get("fold").unwrap()), "a b\nc\n more\nd");
        assert_eq!(s(v.get("keep").unwrap()), "k\n\n");
        assert_eq!(s(v.get("clip").unwrap()), "z\n");
        assert_eq!(s(v.get("ind").unwrap()), "  four\ntwo\n");
        assert_eq!(v.get("empty").unwrap().value, Value::Str(String::new()));
        assert_eq!(s(v.get("next").unwrap()), "1");
    }

    #[test]
    fn block_scalar_with_markdown_and_sequences() {
        let v = parse(
            "Notes: |-\n  # Title\n  - item: one\n  > quote\nAfter: x\nL:\n- |\n  in seq\n- k: >-\n    folded\n    text\n  j: 2\n",
        )
        .unwrap();
        assert_eq!(s(v.get("Notes").unwrap()), "# Title\n- item: one\n> quote");
        assert_eq!(s(v.get("After").unwrap()), "x");
        let l = v.get("L").unwrap().items();
        assert_eq!(s(&l[0]), "in seq\n");
        assert_eq!(s(l[1].get("k").unwrap()), "folded text");
        assert_eq!(s(l[1].get("j").unwrap()), "2");
    }

    #[test]
    fn nested_sequences_and_markers() {
        let v = parse("%YAML 1.2\n---\nk:\n  - - a\n    - b\n  -\n    x: 1\n  - # c\n  -\n...\n")
            .unwrap();
        let k = v.get("k").unwrap().items();
        assert_eq!(strs(&k[0]), ["a", "b"]);
        assert_eq!(s(k[1].get("x").unwrap()), "1");
        assert_eq!(k[2].value, Value::Null);
        assert_eq!(k[3].value, Value::Null);
        assert_eq!(parse("").unwrap().value, Value::Null);
        assert_eq!(parse("# only a comment\n").unwrap().value, Value::Null);
    }

    #[test]
    fn crlf_and_quoted_keys() {
        let v = parse("A: 1\r\n\"B c\":\r\n- x\r\n'd': |\r\n  t\r\n").unwrap();
        assert_eq!(s(v.get("A").unwrap()), "1");
        assert_eq!(strs(v.get("B c").unwrap()), ["x"]);
        assert_eq!(s(v.get("d").unwrap()), "t\n");
    }

    fn e(text: &str) -> (u32, u32, &'static str) {
        let e = parse(text).unwrap_err();
        (e.at.line, e.at.col, e.message)
    }

    #[test]
    fn errors_carry_line_and_column() {
        assert_eq!(e("a: 1\na: 2\n"), (2, 1, "a key given twice"));
        assert_eq!(e("a: *ref\n"), (1, 4, "aliases are not supported"));
        assert_eq!(e("a: &x 1\n"), (1, 4, "anchors are not supported"));
        assert_eq!(e("a: !!str 1\n"), (1, 4, "tags are not supported"));
        assert_eq!(e("a: 1\n\tb: 2\n"), (2, 1, "a tab in the indentation"));
        assert_eq!(e("a: 'open\n").0, 1);
        assert_eq!(e("a: 'open\n").1, 4);
        assert_eq!(e("a: 1\n   b: 2\n"), (2, 4, "unexpected indentation"));
        assert_eq!(e("a:\n  b: 1\n   c: 2\n"), (3, 4, "unexpected indentation"));
        assert_eq!(e("a:\n  - 1\n   - 2\n"), (3, 4, "unexpected indentation"));
        assert_eq!(e("a:\n  b:\n    - 1\n  - 2\n"), (4, 3, "unexpected indentation in a mapping"));
        assert_eq!(e("a: [x, y\n"), (2, 1, "an unterminated flow collection"));
        assert_eq!(e("a: b: c\n"), (1, 4, "a mapping cannot start on its key's line"));
        assert_eq!(e("a: \"\\q\"\n"), (1, 6, "an unknown escape"));
        assert_eq!(e("a: 1\n---\nb: 2\n").2, "a second document (one manifest is one document)");
        assert_eq!(e("- a\nb: 1\n").2, "text after the document's top-level node");
        assert_eq!(e("a: x\n  - y\n").2, "unexpected indentation");
        // Columns count characters.
        assert_eq!(e("\u{e9}\u{e9}: \"\\q\"\n"), (1, 7, "an unknown escape"));
        let deep = "[".repeat(100);
        assert_eq!(e(&deep).2, "nesting too deep");
    }

    #[test]
    fn error_display() {
        let err = parse("a: 1\na: 2\n").unwrap_err();
        assert_eq!(err.to_string(), "2:1: a key given twice");
    }
}
