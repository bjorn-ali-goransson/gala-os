//! A small XML 1.0 reader with namespaces (W3C XML 1.0 and Namespaces
//! in XML 1.0), shared by galapkg (MSIX's AppxManifest.xml,
//! AppxBlockMap.xml, AppxBundleManifest.xml and OPC's
//! [Content_Types].xml), msxml6.dll's DOM and webservices.dll's
//! WS_XML_READER: elements, attributes, text,
//! CDATA, comments, processing instructions (the XML declaration among
//! them), the five predefined entities and character references; UTF-8
//! (with or without a byte order mark) and UTF-16 with a byte order mark.
//! Line ends are normalized to `\n` (XML 1.0 section 2.11).
//!
//! Refused (an error names the line and column): document type
//! declarations (no DTDs, so no entity expansion: MSIX packages carry
//! none, and MSXML 6 prohibits them by default), undeclared prefixes,
//! mismatched or unclosed tags, duplicate attributes, text or a second
//! element outside the root, nesting deeper than [`MAX_DEPTH`].
//!
//! The result is a tree of [`Element`]s, each with its namespace URI
//! and local name resolved, the line its start tag is on, its child
//! elements and its content in document order ([`Content`]); namespace
//! declarations are attributes in [`XMLNS_NS`], as the DOM has them.
//! [`parse_document`] also gives what comes before and after the root.
//!
//! [`Scan`] is the same reader with the other face: it hands out one
//! [`Event`] at a time for a caller that pulls, over the same
//! character-level rules, so it refuses what [`parse`] refuses -- at
//! the node that is wrong, which is where a reader whose caller pulls
//! has to fail. The document is passed to each [`Scan::next_event`]
//! call and not held, so a scan is an offset into it and borrows
//! nothing.
//!
//! Allocation is infallible (`alloc`'s String and Vec): a document is
//! read whole into memory, its size the caller's to bound.

#![no_std]

extern crate alloc;
#[cfg(test)]
extern crate std;

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

/// Deeper nesting is refused (manifests are a few levels deep).
pub const MAX_DEPTH: usize = 256;

/// The namespace the `xml` prefix is bound to.
pub const XML_NS: &str = "http://www.w3.org/XML/1998/namespace";

/// The namespace of namespace declarations (`xmlns`, `xmlns:p`), as the
/// DOM gives their attributes.
pub const XMLNS_NS: &str = "http://www.w3.org/2000/xmlns/";

/// What the reader was looking for where it stopped, which is more than
/// the message says: a caller hands its own caller a different code for
/// each, and picking that code by matching substrings of the message is
/// how it was done before this existed.
///
/// The names are the XML grammar's own terms for what is missing, which
/// is why they line up with the codes XmlLite answers a read with
/// (`user/xmllite`'s reader maps them one to one, measured in
/// `user/xmllite_test`); msxml6 groups them differently.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Reason {
    /// The grammar is broken and nothing more precise fits.
    #[default]
    Syntax,
    /// The text ran out inside an element.
    InputEnd,
    /// White space the grammar requires, between two attributes.
    Whitespace,
    /// The `;` that ends a reference, or a reference that is no number.
    Semicolon,
    /// The `=` after an attribute's name.
    Equal,
    /// The quotes around an attribute's value.
    Quote,
    /// A `<` where no `<` may stand (an attribute's value).
    LessThan,
    /// A name, or the rest of one: a tag the text ends inside of.
    Name,
    /// An end tag where no element is open, which in a document means
    /// it stands where the root element belongs.
    NoStartTag,
    /// A comment that is not closed.
    Comment,
    /// A CDATA section that is not closed.
    CdSect,
    /// A processing instruction that is not closed, or whose target is
    /// not followed by white space.
    Pi,
    /// No root element where a document must have one.
    NoRoot,
    /// A second root element, where a document ends after the first.
    RootElement,
    /// An end tag naming an element other than the open one.
    ElementMatch,
    /// One attribute given twice in a tag.
    UniqueAttribute,
    /// An XML declaration that is not the document's first node.
    XmlDecl,
    /// An entity no DTD declares (and these readers read no DTDs).
    UndeclaredEntity,
    /// A document type declaration, which these readers refuse.
    Dtd,
    /// A namespace prefix nothing binds.
    UndeclaredPrefix,
    /// A namespace declaration binding a prefix to nothing.
    EmptyUri,
    /// Nesting past [`MAX_DEPTH`], which is gala's own limit.
    MaxDepth,
    /// Bytes that are not the encoding they are taken for.
    Encoding,
}

/// Why a document could not be read: the line (1-based), the column
/// (1-based, in characters) where reading stopped, what the reader was
/// looking for, and what to say about it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    pub line: u32,
    pub column: u32,
    pub msg: String,
    pub reason: Reason,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "line {}: {}", self.line, self.msg)
    }
}

/// An attribute, its namespace resolved (empty for an unprefixed one;
/// [`XMLNS_NS`] for a namespace declaration, whose local name is the
/// prefix it declares, or `xmlns` for the default namespace).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Attr {
    pub ns: String,
    pub name: String,
    /// The name as written (with its prefix).
    pub qname: String,
    pub value: String,
}

/// A piece of an element's content (or of the document outside the
/// root), in document order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Content {
    /// The child element at this index of `children`.
    Element(usize),
    /// Character data, references replaced (one run between markup).
    Text(String),
    /// A CDATA section's text.
    CData(String),
    Comment(String),
    /// A processing instruction (the XML declaration is one, target
    /// `xml`): its target and its data, leading white space dropped.
    Pi {
        target: String,
        data: String,
    },
}

/// A whole document: what precedes the root (the XML declaration,
/// comments, processing instructions), the root, and what follows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Document {
    pub prolog: Vec<Content>,
    pub root: Element,
    pub epilog: Vec<Content>,
}

/// An element.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Element {
    /// Namespace URI (empty when in no namespace).
    pub ns: String,
    /// Local name.
    pub name: String,
    /// The name as written (with its prefix), for messages.
    pub qname: String,
    pub attrs: Vec<Attr>,
    pub children: Vec<Element>,
    /// The element's own text (character data and CDATA, concatenated).
    pub text: String,
    /// The content in document order: text, CDATA, comments, processing
    /// instructions and the child elements (by index).
    pub content: Vec<Content>,
    /// The line of the start tag.
    pub line: u32,
}

impl Element {
    /// Appends a child element (to `children` and `content`).
    fn add_child(&mut self, child: Element) {
        self.content.push(Content::Element(self.children.len()));
        self.children.push(child);
    }

    /// An unprefixed attribute's value.
    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attr_ns("", name)
    }

    /// An attribute's value by namespace and local name.
    pub fn attr_ns(&self, ns: &str, name: &str) -> Option<&str> {
        self.attrs.iter().find(|a| a.ns == ns && a.name == name).map(|a| a.value.as_str())
    }

    /// Whether this element is `{ns}name`.
    pub fn is(&self, ns: &str, name: &str) -> bool {
        self.ns == ns && self.name == name
    }

    /// The first child `{ns}name`.
    pub fn child(&self, ns: &str, name: &str) -> Option<&Element> {
        self.children.iter().find(|c| c.is(ns, name))
    }

    /// The children `{ns}name`.
    pub fn children_named<'s>(
        &'s self,
        ns: &'s str,
        name: &'s str,
    ) -> impl Iterator<Item = &'s Element> + 's {
        self.children.iter().filter(move |c| c.is(ns, name))
    }
}

/// Turns the document's bytes into text: UTF-8 (a BOM skipped) or
/// UTF-16 LE/BE with a BOM; line ends become `\n`.
pub fn decode(bytes: &[u8]) -> Result<String, Error> {
    let text = decode_raw(bytes)?;
    if !text.contains('\r') {
        return Ok(text);
    }
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\r' {
            if chars.peek() == Some(&'\n') {
                chars.next();
            }
            out.push('\n');
        } else {
            out.push(c);
        }
    }
    Ok(out)
}

fn decode_raw(bytes: &[u8]) -> Result<String, Error> {
    let bad = |msg: &str| Error { line: 1, column: 1, msg: msg.into(), reason: Reason::Encoding };
    if let Some(rest) = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]) {
        return core::str::from_utf8(rest).map(String::from).map_err(|_| bad("not valid UTF-8"));
    }
    let le = bytes.starts_with(&[0xff, 0xfe]);
    if le || bytes.starts_with(&[0xfe, 0xff]) {
        let units = bytes[2..].chunks(2).map(|c| {
            let pair = [c[0], *c.get(1).unwrap_or(&0)];
            if le { u16::from_le_bytes(pair) } else { u16::from_be_bytes(pair) }
        });
        if !bytes.len().is_multiple_of(2) {
            return Err(bad("UTF-16 text of an odd number of bytes"));
        }
        return char::decode_utf16(units)
            .collect::<Result<String, _>>()
            .map_err(|_| bad("not valid UTF-16"));
    }
    core::str::from_utf8(bytes).map(String::from).map_err(|_| bad("not valid UTF-8"))
}

/// Parses a whole document; the root element is returned.
pub fn parse(bytes: &[u8]) -> Result<Element, Error> {
    parse_document(bytes).map(|d| d.root)
}

/// Parses a whole document given as bytes (see [`decode`]).
pub fn parse_document(bytes: &[u8]) -> Result<Document, Error> {
    let text = decode(bytes)?;
    parse_text(&text)
}

/// Parses a whole document already decoded (for UTF-16 text a program
/// hands over, such as MSXML's loadXML). Line ends are normalized.
pub fn parse_text(text: &str) -> Result<Document, Error> {
    if text.contains('\r') {
        return parse_document(text.as_bytes());
    }
    Parser { s: text.as_bytes(), text, pos: 0, line: 1, line_start: 0 }.document()
}

/// An element being read, with the namespace bindings its tag made.
#[derive(Debug)]
struct Open {
    el: Element,
    /// (prefix, uri); the empty prefix is the default namespace.
    bindings: Vec<(String, String)>,
}

struct Parser<'t> {
    s: &'t [u8],
    text: &'t str,
    pos: usize,
    line: u32,
    /// Where the current line starts (for the column).
    line_start: usize,
}

fn is_name_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_' || c == ':' || !c.is_ascii()
}

fn is_name_char(c: char) -> bool {
    is_name_start(c) || c.is_ascii_digit() || c == '-' || c == '.'
}

impl<'t> Parser<'t> {
    fn err<T>(&self, msg: impl Into<String>) -> Result<T, Error> {
        Err(self.error_at(self.line, msg))
    }

    /// As [`Parser::err`], saying what was looked for.
    fn err_for<T>(&self, reason: Reason, msg: impl Into<String>) -> Result<T, Error> {
        Err(self.error_for(self.line, reason, msg))
    }

    /// As [`Parser::error_at`], saying what was looked for.
    fn error_for(&self, line: u32, reason: Reason, msg: impl Into<String>) -> Error {
        Error { reason, ..self.error_at(line, msg) }
    }

    /// An error on `line` (the column is where reading stopped when that
    /// is the current line, else 1).
    fn error_at(&self, line: u32, msg: impl Into<String>) -> Error {
        let column = if line == self.line {
            let end = self.pos.min(self.text.len());
            self.text.get(self.line_start..end).map_or(1, |s| s.chars().count() as u32 + 1)
        } else {
            1
        };
        Error { line, column, msg: msg.into(), reason: Reason::Syntax }
    }

    fn starts(&self, p: &str) -> bool {
        self.s[self.pos..].starts_with(p.as_bytes())
    }

    /// Moves `n` bytes on, counting lines.
    fn advance(&mut self, n: usize) {
        let end = (self.pos + n).min(self.s.len());
        for i in self.pos..end {
            if self.s[i] == b'\n' {
                self.line += 1;
                self.line_start = i + 1;
            }
        }
        self.pos = end;
    }

    fn skip_space(&mut self) {
        while self.pos < self.s.len() && matches!(self.s[self.pos], b' ' | b'\t' | b'\r' | b'\n') {
            self.advance(1);
        }
    }

    /// Skips up to and including `end`; `what` names the construct and
    /// `reason` is what a text that ends before `end` was missing.
    fn skip_past(&mut self, end: &str, what: &str, reason: Reason) -> Result<&'t str, Error> {
        let start = self.pos;
        match self.text[start..].find(end) {
            Some(i) => {
                self.advance(i + end.len());
                Ok(&self.text[start..start + i])
            }
            None => self.err_for(reason, alloc::format!("{what} not closed")),
        }
    }

    fn name(&mut self) -> Result<&'t str, Error> {
        let start = self.pos;
        let mut chars = self.text[start..].char_indices();
        match chars.next() {
            Some((_, c)) if is_name_start(c) => {}
            _ => return self.err_for(Reason::Name, "a name expected"),
        }
        let len =
            chars.find(|&(_, c)| !is_name_char(c)).map_or(self.text.len() - start, |(i, _)| i);
        self.advance(len);
        Ok(&self.text[start..start + len])
    }

    /// Character data up to `stop` with references replaced.
    fn chars_until(&mut self, stop: &[u8], out: &mut String) -> Result<(), Error> {
        while self.pos < self.s.len() && !stop.contains(&self.s[self.pos]) {
            if self.s[self.pos] == b'&' {
                self.reference(out)?;
                continue;
            }
            let start = self.pos;
            let mut end = start;
            while end < self.s.len() && !stop.contains(&self.s[end]) && self.s[end] != b'&' {
                end += 1;
            }
            out.push_str(&self.text[start..end]);
            self.advance(end - start);
        }
        Ok(())
    }

    fn reference(&mut self, out: &mut String) -> Result<(), Error> {
        self.advance(1);
        let start = self.pos;
        let Some(len) = self.text[start..].find(';').filter(|&l| l <= 12) else {
            return self.err_for(Reason::Semicolon, "an unterminated '&' reference");
        };
        let r = &self.text[start..start + len];
        let c = match r {
            "lt" => '<',
            "gt" => '>',
            "amp" => '&',
            "quot" => '"',
            "apos" => '\'',
            _ => {
                let code = if let Some(hex) = r.strip_prefix("#x") {
                    u32::from_str_radix(hex, 16).ok()
                } else if let Some(dec) = r.strip_prefix('#') {
                    dec.parse::<u32>().ok()
                } else {
                    return self.err_for(
                        Reason::UndeclaredEntity,
                        alloc::format!("unknown entity &{r}; (no DTDs)"),
                    );
                };
                match code.and_then(char::from_u32).filter(|&c| c != '\0') {
                    Some(c) => c,
                    None => {
                        return self.err_for(
                            Reason::Semicolon,
                            alloc::format!("a bad character reference &{r};"),
                        );
                    }
                }
            }
        };
        out.push(c);
        self.advance(len + 1);
        Ok(())
    }

    /// Comments, processing instructions and white space between
    /// markup outside the root; a DOCTYPE is refused.
    fn misc(&mut self, out: &mut Vec<Content>) -> Result<(), Error> {
        loop {
            self.skip_space();
            if self.starts("<!--") {
                out.push(Content::Comment(self.comment()?));
            } else if self.starts("<?") {
                let (target, data) = self.pi()?;
                out.push(Content::Pi { target, data });
            } else if self.starts("<!DOCTYPE") {
                return self
                    .err_for(Reason::Dtd, "a document type declaration (DTDs are not allowed)");
            } else {
                return Ok(());
            }
        }
    }

    /// A comment's text (at `<!--`).
    fn comment(&mut self) -> Result<String, Error> {
        self.advance(4);
        Ok(self.skip_past("-->", "a comment", Reason::Comment)?.into())
    }

    /// A processing instruction's target and data (at `<?`), its
    /// leading white space dropped.
    fn pi(&mut self) -> Result<(String, String), Error> {
        self.advance(2);
        let target = self.name()?;
        let body = self.skip_past("?>", "a processing instruction", Reason::Pi)?;
        if !body.is_empty() && !body.starts_with([' ', '\t', '\n']) {
            return self
                .err_for(Reason::Pi, alloc::format!("white space expected after <?{target}"));
        }
        let data = body.trim_start_matches([' ', '\t', '\n']);
        Ok((target.into(), data.into()))
    }

    fn document(mut self) -> Result<Document, Error> {
        let mut prolog = Vec::new();
        // The XML declaration may only come first.
        if self.starts("<?xml") && !self.starts("<?xml-") {
            let (target, data) = self.pi()?;
            prolog.push(Content::Pi { target, data });
        }
        self.misc(&mut prolog)?;
        if self.starts("</") {
            return self.err_for(Reason::NoStartTag, "no root element: an end tag opens the text");
        }
        if !self.starts("<") {
            return self.err_for(Reason::NoRoot, "no root element");
        }
        let root = self.element()?;
        let mut epilog = Vec::new();
        self.misc(&mut epilog)?;
        if self.pos != self.s.len() {
            // A second root element and anything else after the root
            // are different errors to a caller, which is why they are
            // different reasons: a reader answers WC_E_ROOTELEMENT for
            // the one and WC_E_SYNTAX for the other (both measured in
            // user/xmllite_test).
            return if self.starts("<") && !self.starts("</") {
                self.err_for(Reason::RootElement, "a second element after the root element")
            } else {
                self.err("content after the root element")
            };
        }
        let misplaced = |c: &Content| matches!(c, Content::Pi { target, .. } if target.eq_ignore_ascii_case("xml"));
        if prolog.iter().skip(1).chain(epilog.iter()).any(misplaced) {
            return Err(self.error_for(1, Reason::XmlDecl, "an XML declaration not at the start"));
        }
        Ok(Document { prolog, root, epilog })
    }

    /// Resolves a prefix against the open elements' bindings (innermost
    /// first) and the tag's own.
    fn resolve(
        &self,
        stack: &[Open],
        own: &[(String, String)],
        prefix: &str,
        line: u32,
    ) -> Result<String, Error> {
        if prefix == "xml" {
            return Ok(XML_NS.into());
        }
        let found = own
            .iter()
            .chain(stack.iter().rev().flat_map(|o| o.bindings.iter()))
            .find(|(p, _)| p == prefix)
            .map(|(_, uri)| uri.clone());
        match found {
            Some(uri) => Ok(uri),
            None if prefix.is_empty() => Ok(String::new()),
            None => Err(self.error_for(
                line,
                Reason::UndeclaredPrefix,
                alloc::format!("undeclared namespace prefix '{prefix}'"),
            )),
        }
    }

    /// The root element and everything in it (iteratively: the nesting
    /// is bounded by MAX_DEPTH, not by the stack).
    fn element(&mut self) -> Result<Element, Error> {
        let mut stack: Vec<Open> = Vec::new();
        loop {
            if self.starts("</") {
                let line = self.line;
                self.advance(2);
                let name = self.name()?;
                self.skip_space();
                if !self.starts(">") {
                    return self.err("'>' expected");
                }
                self.advance(1);
                let Some(open) = stack.pop() else {
                    return self.err_for(Reason::NoStartTag, "an end tag without a start tag");
                };
                if open.el.qname != name {
                    return Err(self.error_for(
                        line,
                        Reason::ElementMatch,
                        alloc::format!(
                            "</{name}> closes <{}> (line {})",
                            open.el.qname,
                            open.el.line
                        ),
                    ));
                }
                match stack.last_mut() {
                    Some(parent) => parent.el.add_child(open.el),
                    None => return Ok(open.el),
                }
            } else if self.starts("<!--") {
                let text = self.comment()?;
                if let Some(o) = stack.last_mut() {
                    o.el.content.push(Content::Comment(text));
                }
            } else if self.starts("<![CDATA[") {
                self.advance(9);
                let data = self.skip_past("]]>", "a CDATA section", Reason::CdSect)?;
                match stack.last_mut() {
                    Some(o) => {
                        o.el.text.push_str(data);
                        o.el.content.push(Content::CData(data.into()));
                    }
                    None => return self.err("CDATA outside the root"),
                }
            } else if self.starts("<?") {
                let (target, data) = self.pi()?;
                if target.eq_ignore_ascii_case("xml") {
                    return self.err_for(Reason::XmlDecl, "an XML declaration not at the start");
                }
                if let Some(o) = stack.last_mut() {
                    o.el.content.push(Content::Pi { target, data });
                }
            } else if self.starts("<!") {
                return self.err("markup declarations are not allowed");
            } else if self.starts("<") {
                if stack.len() >= MAX_DEPTH {
                    return self.err_for(Reason::MaxDepth, "elements nested too deeply");
                }
                let (open, empty) = self.start_tag(&stack)?;
                if empty {
                    match stack.last_mut() {
                        Some(parent) => parent.el.add_child(open.el),
                        None => return Ok(open.el),
                    }
                } else {
                    stack.push(open);
                }
            } else if self.pos >= self.s.len() {
                let open =
                    stack.last().map_or(("?".into(), 0), |o| (o.el.qname.clone(), o.el.line));
                return self.err_for(
                    Reason::InputEnd,
                    alloc::format!("<{}> (line {}) is not closed", open.0, open.1),
                );
            } else {
                let mut text = String::new();
                self.chars_until(b"<", &mut text)?;
                match stack.last_mut() {
                    Some(o) => {
                        o.el.text.push_str(&text);
                        o.el.content.push(Content::Text(text));
                    }
                    None => return self.err("text outside the root"),
                }
            }
        }
    }

    fn start_tag(&mut self, stack: &[Open]) -> Result<(Open, bool), Error> {
        let line = self.line;
        self.advance(1);
        let qname = self.name()?;
        let mut raw: Vec<(&'t str, String, u32)> = Vec::new();
        let empty = loop {
            let had_space = self.pos < self.s.len() && self.s[self.pos].is_ascii_whitespace();
            self.skip_space();
            if self.starts("/>") {
                self.advance(2);
                break true;
            }
            if self.starts(">") {
                self.advance(1);
                break false;
            }
            if self.pos >= self.s.len() {
                return self.err_for(
                    Reason::Name,
                    alloc::format!("the start tag <{qname}> is not closed"),
                );
            }
            if !had_space {
                return self.err_for(
                    Reason::Whitespace,
                    alloc::format!("white space expected in <{qname}>"),
                );
            }
            let aline = self.line;
            let an = self.name()?;
            self.skip_space();
            if !self.starts("=") {
                return self
                    .err_for(Reason::Equal, alloc::format!("'=' expected after attribute {an}"));
            }
            self.advance(1);
            self.skip_space();
            let quote = match self.s.get(self.pos) {
                Some(&q @ (b'"' | b'\'')) => q,
                _ => {
                    return self.err_for(
                        Reason::Quote,
                        alloc::format!("a quoted value expected for attribute {an}"),
                    );
                }
            };
            self.advance(1);
            let mut value = String::new();
            self.chars_until(&[quote, b'<'], &mut value)?;
            if self.s.get(self.pos) != Some(&quote) {
                return self.err_for(
                    Reason::LessThan,
                    alloc::format!("'<' in the value of attribute {an}"),
                );
            }
            self.advance(1);
            // Attribute values are normalized: white space becomes spaces.
            let value: String = value
                .chars()
                .map(|c| if matches!(c, '\t' | '\n' | '\r') { ' ' } else { c })
                .collect();
            if raw.iter().any(|(n, _, _)| *n == an) {
                return Err(self.error_for(
                    aline,
                    Reason::UniqueAttribute,
                    alloc::format!("attribute {an} given twice in <{qname}>"),
                ));
            }
            raw.push((an, value, aline));
        };
        let mut bindings = Vec::new();
        for (n, v, _) in &raw {
            if *n == "xmlns" {
                bindings.push((String::new(), v.clone()));
            } else if let Some(p) = n.strip_prefix("xmlns:") {
                if v.is_empty() {
                    return Err(self.error_for(
                        line,
                        Reason::EmptyUri,
                        alloc::format!("prefix {p} bound to nothing"),
                    ));
                }
                bindings.push((p.into(), v.clone()));
            }
        }
        let split = |q: &'t str| match q.split_once(':') {
            Some((p, l)) => (p, l),
            None => ("", q),
        };
        let (prefix, local) = split(qname);
        let ns = self.resolve(stack, &bindings, prefix, line)?;
        let mut attrs = Vec::new();
        for (n, value, aline) in raw {
            if n == "xmlns" || n.starts_with("xmlns:") {
                let local = n.strip_prefix("xmlns:").unwrap_or(n);
                attrs.push(Attr {
                    ns: XMLNS_NS.into(),
                    name: local.into(),
                    qname: n.into(),
                    value,
                });
                continue;
            }
            let (p, l) = split(n);
            // Unprefixed attributes are in no namespace.
            let ans = if p.is_empty() {
                String::new()
            } else {
                self.resolve(stack, &bindings, p, aline)?
            };
            if attrs.iter().any(|a: &Attr| a.ns == ans && a.name == l) {
                return Err(self.error_for(
                    aline,
                    Reason::UniqueAttribute,
                    alloc::format!("attribute {{{ans}}}{l} given twice in <{qname}>"),
                ));
            }
            attrs.push(Attr { ns: ans, name: l.into(), qname: n.into(), value });
        }
        let el = Element {
            ns,
            name: local.into(),
            qname: qname.into(),
            attrs,
            children: Vec::new(),
            text: String::new(),
            content: Vec::new(),
            line,
        };
        Ok((Open { el, bindings }, empty))
    }
}

/// What a scan sees, in document order ([`Scan`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// A start tag, its namespace and its attributes' resolved as
    /// [`Element`]'s are. An empty element is a start followed at once
    /// by an [`Event::EndElement`], which [`Scan::empty_element`] says
    /// in advance.
    StartElement {
        /// Namespace URI (empty when in no namespace).
        ns: String,
        /// Local name.
        name: String,
        /// The name as written (with its prefix).
        qname: String,
        attrs: Vec<Attr>,
        /// The line the start tag is on.
        line: u32,
    },
    /// The end of the element the scan was in (an end tag, or the
    /// second half of an empty element).
    EndElement,
    /// Character data, references replaced: one run between markup,
    /// and the white space outside the root element.
    Text(String),
    /// A CDATA section's text. It is character data like
    /// [`Event::Text`] -- a caller with no use for the difference
    /// treats the two alike -- but a reader has to be able to say
    /// which of the two it read.
    CData(String),
    Comment(String),
    /// A processing instruction (the XML declaration is one, target
    /// `xml`): its target and its data, leading white space dropped.
    Pi {
        target: String,
        data: String,
    },
    /// Past the root element, nothing left: every later call gives this
    /// again.
    End,
}

/// A document read node by node: [`Event`]s in document order, with the
/// errors [`parse`] gives and at the point where reading meets them --
/// which is what a program that is handed one node at a time needs
/// (webservices.dll's WS_XML_READER, ARCHITECTURE "Other DLLs"), and
/// what a tree cannot say, since it is built or refused whole.
///
/// The text is passed to each [`Scan::next_event`] instead of held, so
/// a scan is a position and a stack of open elements and borrows
/// nothing: the document stays the caller's, to own as it likes.
///
/// Both faces of this crate are dispatch loops over the same character
/// rules: [`parse`] builds elements out of them, a scan hands each node
/// out.
#[derive(Debug)]
pub struct Scan {
    pos: usize,
    line: u32,
    line_start: usize,
    /// The elements the scan is inside, innermost last: their names (to
    /// match end tags against) and their namespace bindings.
    stack: Vec<Open>,
    /// The root element has been closed.
    root_done: bool,
    /// More than one root element is allowed: the text is a fragment,
    /// not a document (webservices.dll's
    /// WS_XML_READER_PROPERTY_ALLOW_FRAGMENT, ARCHITECTURE "Other
    /// DLLs"). Everything else holds: each root is well-formed, and
    /// nothing but white space, comments and processing instructions
    /// may stand between them.
    fragment: bool,
    /// An empty element's tag was handed out as a start, so its end is
    /// the next event.
    pending_end: bool,
    /// An event has been handed out, so an XML declaration is no longer
    /// at the start of the document.
    started: bool,
}

impl Default for Scan {
    fn default() -> Self {
        Self::new()
    }
}

impl Scan {
    /// A scan at the start of a document.
    pub fn new() -> Self {
        Self {
            pos: 0,
            line: 1,
            line_start: 0,
            stack: Vec::new(),
            root_done: false,
            fragment: false,
            pending_end: false,
            started: false,
        }
    }

    /// A scan over a fragment: more than one root element, with the
    /// same rules otherwise (each root well-formed, and nothing but
    /// white space, comments and processing instructions between them).
    pub fn allow_fragment(mut self) -> Self {
        self.fragment = true;
        self
    }

    /// How many elements the scan is inside: 0 outside the root, 1 on
    /// the root's content.
    pub fn depth(&self) -> usize {
        self.stack.len()
    }

    /// Where the scan has got to: the line, counted from 1, and how
    /// many bytes into it, counted from 0. It is the end of the node
    /// last handed out, which is where the next one starts.
    pub fn position(&self) -> (u32, usize) {
        (self.line, self.pos - self.line_start)
    }

    /// Where the scan has got to as a reader reports it: the line and
    /// the character within it, both counted from 1, which is how
    /// XmlLite counts them. [`Scan::position`] counts bytes from 0
    /// instead, which is what webservices' reader hands its caller.
    ///
    /// Called before [`Scan::next_event`] it is where the next node
    /// starts, since every character of the document belongs to one
    /// node.
    pub fn line_position(&self, text: &str) -> (u32, u32) {
        let end = self.pos.min(text.len());
        let chars = text.get(self.line_start..end).map_or(0, |s| s.chars().count());
        (self.line, chars as u32 + 1)
    }

    /// The start tag last handed out was an empty element (`<e/>`), so
    /// the next event is the end it has no tag for. A reader that
    /// reports an empty element as one node reads that end and drops
    /// it; XmlLite's IsEmptyElement is this.
    pub fn empty_element(&self) -> bool {
        self.pending_end
    }

    /// The local name of the element the scan is inside, innermost
    /// first, or None outside the root.
    pub fn open_name(&self) -> Option<&str> {
        self.stack.last().map(|o| o.el.name.as_str())
    }

    /// The next node of `text`, which is the same document every call
    /// (the scan holds a byte offset into it, not the text).
    pub fn next_event(&mut self, text: &str) -> Result<Event, Error> {
        let mut p = Parser {
            s: text.as_bytes(),
            text,
            pos: self.pos,
            line: self.line,
            line_start: self.line_start,
        };
        let event = self.step(&mut p);
        self.pos = p.pos;
        self.line = p.line;
        self.line_start = p.line_start;
        if event.is_ok() {
            self.started = true;
        }
        event
    }

    /// Leaves the innermost open element; the root's end ends the
    /// document's content.
    fn close(&mut self) {
        self.stack.pop();
        if self.stack.is_empty() {
            self.root_done = true;
        }
    }

    fn step(&mut self, p: &mut Parser<'_>) -> Result<Event, Error> {
        if self.pending_end {
            self.pending_end = false;
            self.close();
            return Ok(Event::EndElement);
        }
        // Outside the root: white space, comments and processing
        // instructions, of which the XML declaration may only be the
        // document's first node.
        if self.stack.is_empty() {
            let at_start = !self.started && p.pos == 0;
            // The white space between the markup outside the root is a
            // node of its own, as the text inside it is: a reader hands
            // its caller every character of the document, and one with
            // no use for it drops a text node that is only white space
            // (as webservices' reader does). A declaration still has to
            // be the document's first node, which `at_start` holds from
            // before this.
            let from = p.pos;
            p.skip_space();
            if p.pos > from {
                return Ok(Event::Text(p.text[from..p.pos].into()));
            }
            if p.starts("<!--") {
                return Ok(Event::Comment(p.comment()?));
            }
            if p.starts("<?") {
                let (target, data) = p.pi()?;
                if target.eq_ignore_ascii_case("xml") && !at_start {
                    return p.err_for(Reason::XmlDecl, "an XML declaration not at the start");
                }
                return Ok(Event::Pi { target, data });
            }
            if p.starts("<!DOCTYPE") {
                return p
                    .err_for(Reason::Dtd, "a document type declaration (DTDs are not allowed)");
            }
            if self.root_done {
                if p.pos == p.s.len() {
                    return Ok(Event::End);
                }
                if !self.fragment {
                    // A second root element and anything else after the
                    // root are different errors to a caller (see
                    // `Parser::document`).
                    return if p.starts("<") && !p.starts("</") {
                        p.err_for(Reason::RootElement, "a second element after the root element")
                    } else {
                        p.err("content after the root element")
                    };
                }
                // A fragment's next root element, read like the first.
                self.root_done = false;
            }
            if p.starts("</") {
                return p.err_for(Reason::NoStartTag, "no root element: an end tag opens the text");
            }
            if !p.starts("<") {
                return p.err_for(Reason::NoRoot, "no root element");
            }
        }
        // Inside the root, or on its start tag.
        if p.starts("</") {
            let line = p.line;
            p.advance(2);
            let name = p.name()?;
            p.skip_space();
            if !p.starts(">") {
                return p.err("'>' expected");
            }
            p.advance(1);
            let open = &self.stack.last().expect("inside an element").el;
            if open.qname != name {
                let (qname, oline) = (open.qname.clone(), open.line);
                return Err(p.error_for(
                    line,
                    Reason::ElementMatch,
                    alloc::format!("</{name}> closes <{qname}> (line {oline})"),
                ));
            }
            self.close();
            return Ok(Event::EndElement);
        }
        if p.starts("<!--") {
            return Ok(Event::Comment(p.comment()?));
        }
        if p.starts("<![CDATA[") {
            p.advance(9);
            let data = p.skip_past("]]>", "a CDATA section", Reason::CdSect)?;
            return Ok(Event::CData(data.into()));
        }
        if p.starts("<?") {
            let (target, data) = p.pi()?;
            if target.eq_ignore_ascii_case("xml") {
                return p.err_for(Reason::XmlDecl, "an XML declaration not at the start");
            }
            return Ok(Event::Pi { target, data });
        }
        if p.starts("<!") {
            return p.err("markup declarations are not allowed");
        }
        if p.starts("<") {
            if self.stack.len() >= MAX_DEPTH {
                return p.err_for(Reason::MaxDepth, "elements nested too deeply");
            }
            let (open, empty) = p.start_tag(&self.stack)?;
            let event = Event::StartElement {
                ns: open.el.ns.clone(),
                name: open.el.name.clone(),
                qname: open.el.qname.clone(),
                attrs: open.el.attrs.clone(),
                line: open.el.line,
            };
            self.stack.push(open);
            self.pending_end = empty;
            return Ok(event);
        }
        if p.pos >= p.s.len() {
            let (qname, line) =
                self.stack.last().map_or(("?".into(), 0), |o| (o.el.qname.clone(), o.el.line));
            return p.err_for(
                Reason::InputEnd,
                alloc::format!("<{qname}> (line {line}) is not closed"),
            );
        }
        let mut text = String::new();
        p.chars_until(b"<", &mut text)?;
        Ok(Event::Text(text))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    /// A second root element is content after the root element, unless
    /// the scan was told the text is a fragment.
    #[test]
    fn fragments_need_the_flag() {
        let text = "<a>1</a>\n<b>2</b>";
        let names = |scan: &mut Scan| -> Result<Vec<String>, Error> {
            let mut names = Vec::new();
            loop {
                match scan.next_event(text)? {
                    Event::StartElement { name, .. } => names.push(name),
                    Event::End => return Ok(names),
                    _ => {}
                }
            }
        };

        let whole = names(&mut Scan::new()).unwrap_err();
        assert_eq!(whole.reason, Reason::RootElement);
        assert!(alloc::format!("{whole}").contains("after the root element"), "{whole}");
        assert_eq!(names(&mut Scan::new().allow_fragment()).unwrap(), vec!["a", "b"]);

        // What a fragment does not allow: text between its roots.
        let mut scan = Scan::new().allow_fragment();
        assert!(matches!(scan.next_event("<a/>x<b/>"), Ok(Event::StartElement { .. })));
        assert!(matches!(scan.next_event("<a/>x<b/>"), Ok(Event::EndElement)));
        let loose = alloc::format!("{}", scan.next_event("<a/>x<b/>").unwrap_err());
        assert!(loose.contains("no root element"), "{loose}");
    }

    #[test]
    fn namespaces_resolve() {
        let doc = br#"<?xml version="1.0" encoding="utf-8"?>
<!-- a comment -->
<Package xmlns="urn:f" xmlns:uap="urn:u" IgnorableNamespaces="uap">
  <Identity Name="A&amp;B" uap:Extra='x&#x41;&#66;'/>
  <uap:Thing><Inner xmlns="urn:other">text &lt;here&gt;<![CDATA[ <raw> ]]></Inner></uap:Thing>
</Package>
"#;
        let root = parse(doc).unwrap();
        assert!(root.is("urn:f", "Package"));
        assert_eq!(root.line, 3);
        let id = root.child("urn:f", "Identity").unwrap();
        assert_eq!(id.attr("Name"), Some("A&B"));
        assert_eq!(id.attr_ns("urn:u", "Extra"), Some("xAB"));
        assert_eq!(id.attr("Extra"), None);
        assert_eq!(id.line, 4);
        let thing = root.child("urn:u", "Thing").unwrap();
        let inner = thing.child("urn:other", "Inner").unwrap();
        assert_eq!(inner.text, "text <here> <raw> ");
        assert_eq!(inner.line, 5);
    }

    #[test]
    fn utf16_and_bom() {
        let mut d = vec![0xff, 0xfe];
        for u in "<a b=\"é\"/>".encode_utf16() {
            d.extend_from_slice(&u.to_le_bytes());
        }
        assert_eq!(parse(&d).unwrap().attr("b"), Some("é"));
        assert_eq!(parse(b"\xef\xbb\xbf<a/>").unwrap().name, "a");
    }

    fn err(doc: &str) -> Error {
        parse(doc.as_bytes()).unwrap_err()
    }

    #[test]
    fn errors_name_the_line() {
        let e = err("<a>\n<b>\n</c>\n</a>");
        assert_eq!(e.line, 3);
        assert_eq!(e.msg, "</c> closes <b> (line 2)");
        assert_eq!(err("<a>\n  <p:b/>\n</a>").msg, "undeclared namespace prefix 'p'");
        assert_eq!(err("<a>\n  <p:b/>\n</a>").line, 2);
        assert_eq!(err("<a x='1'\n x=\"2\"/>").line, 2);
        assert!(err("<!DOCTYPE a [<!ENTITY e 'x'>]><a>&e;</a>").msg.contains("DTD"));
        assert!(err("<a>&e;</a>").msg.contains("unknown entity"));
        assert!(err("<a>\n<b>").msg.contains("not closed"));
        assert!(err("<a/><b/>").msg.contains("after the root"));
        assert!(err("text<a/>").msg.contains("no root"));
        assert!(err("<a b='<'/>").msg.contains("'<'"));
        assert!(err("<a b='1'c='2'/>").msg.contains("white space"));
        assert!(err("<a x:b='1' xmlns:x='u' y:b='2' xmlns:y='u'/>").msg.contains("twice"));
        let deep = "<a>".repeat(MAX_DEPTH + 1);
        assert!(err(&deep).msg.contains("too deeply"));
        assert!(parse(b"").is_err());
    }

    #[test]
    fn content_in_order() {
        let doc = b"<?xml version=\"1.0\"?>\r\n<!--c--><r xmlns='urn:d' xmlns:p='urn:p' p:a='1'>x<p:e/>\r\ny<![CDATA[z]]><?t d ?><!--k--></r><?after?>";
        let d = parse_document(doc).unwrap();
        assert_eq!(
            d.prolog,
            vec![
                Content::Pi { target: "xml".into(), data: "version=\"1.0\"".into() },
                Content::Comment("c".into())
            ]
        );
        assert_eq!(d.epilog, vec![Content::Pi { target: "after".into(), data: "".into() }]);
        let r = &d.root;
        assert_eq!(
            r.content,
            vec![
                Content::Text("x".into()),
                Content::Element(0),
                Content::Text("\ny".into()),
                Content::CData("z".into()),
                Content::Pi { target: "t".into(), data: "d ".into() },
                Content::Comment("k".into()),
            ]
        );
        assert_eq!(r.children[0].ns, "urn:p");
        let names: std::vec::Vec<_> =
            r.attrs.iter().map(|a| (a.ns.as_str(), a.name.as_str(), a.qname.as_str())).collect();
        assert_eq!(
            names,
            vec![(XMLNS_NS, "xmlns", "xmlns"), (XMLNS_NS, "p", "xmlns:p"), ("urn:p", "a", "p:a")]
        );
        assert!(err("<a/><?xml version='1.0'?>").msg.contains("XML declaration"));
        let e = err("<a>\n  <b></c></a>");
        assert_eq!((e.line, e.column), (2, 10));
    }

    /// Scans `text` to its first error.
    fn scan_err(text: &str) -> Error {
        let mut scan = Scan::new();
        loop {
            match scan.next_event(text) {
                Ok(Event::End) => panic!("{text}: no error"),
                Ok(_) => {}
                Err(e) => return e,
            }
        }
    }

    /// What the reader was looking for, for every way it refuses a
    /// document. `user/xmllite_test` measures the code Windows answers
    /// for each of these same documents, and `user/xmllite`'s reader
    /// maps these reasons to those codes, so the three are one table in
    /// three places: a row added here belongs there too.
    #[test]
    fn reasons_name_what_was_looked_for() {
        let rows: &[(&str, Reason)] = &[
            ("<a><b></a>", Reason::ElementMatch),
            ("</a>", Reason::NoStartTag),
            ("<a/><b/>", Reason::RootElement),
            ("text<a/>", Reason::NoRoot),
            ("<a/>text", Reason::Syntax),
            ("<!DOCTYPE a><a/>", Reason::Dtd),
            ("<a>", Reason::InputEnd),
            ("<a", Reason::Name),
            ("<a b='1'c='2'/>", Reason::Whitespace),
            ("<a>&e;</a>", Reason::UndeclaredEntity),
            ("<a b='<'/>", Reason::LessThan),
            ("<a x='1' x='2'/>", Reason::UniqueAttribute),
            ("<p:a/>", Reason::UndeclaredPrefix),
            ("<a><!FOO></a>", Reason::Syntax),
            ("<a b=1/>", Reason::Quote),
            ("<?xml version='1.0'?><?xml version='1.0'?>", Reason::XmlDecl),
            ("<a b/>", Reason::Equal),
            ("<a>&e</a>", Reason::Semicolon),
            ("<a>&#xZZ;</a>", Reason::Semicolon),
            ("<!--x", Reason::Comment),
            ("<a><![CDATA[x</a>", Reason::CdSect),
            ("<?pi x", Reason::Pi),
            ("<a xmlns:p=''/>", Reason::EmptyUri),
            ("<?pi,x?><a/>", Reason::Pi),
        ];
        for (doc, want) in rows {
            let e = scan_err(doc);
            assert_eq!(e.reason, *want, "{doc}: {e}");
        }
        // The tree builder answers for the same documents, and MSXML
        // groups three of them its own way (msxml6's parse error code).
        assert_eq!(err("<a><b></a>").reason, Reason::ElementMatch);
        assert_eq!(err("</a>").reason, Reason::NoStartTag);
        assert_eq!(err("text<a/>").reason, Reason::NoRoot);
        assert_eq!(err("<a/><b/>").reason, Reason::RootElement);
        // Bytes that are not the encoding they are taken for.
        assert_eq!(decode(&[0xff, 0xff]).unwrap_err().reason, Reason::Encoding);
    }

    /// Nesting past MAX_DEPTH is gala's own limit, not the grammar's.
    #[test]
    fn too_deep_is_its_own_reason() {
        let deep = "<a>".repeat(MAX_DEPTH + 1);
        assert_eq!(scan_err(&deep).reason, Reason::MaxDepth);
    }

    /// Every character of the document belongs to a node, the white
    /// space outside the root element included: a reader hands its
    /// caller the lot, and one with no use for it (webservices') drops
    /// a text node that is only white space.
    #[test]
    fn white_space_outside_the_root_is_a_node() {
        let text = "\n<!--c--> \n<r/>\n\n";
        let mut scan = Scan::new();
        let mut events = Vec::new();
        loop {
            let event = scan.next_event(text).expect("a well-formed document");
            let done = event == Event::End;
            events.push(event);
            if done {
                break;
            }
        }
        assert_eq!(
            events,
            vec![
                Event::Text("\n".into()),
                Event::Comment("c".into()),
                Event::Text(" \n".into()),
                Event::StartElement {
                    ns: String::new(),
                    name: "r".into(),
                    qname: "r".into(),
                    attrs: Vec::new(),
                    line: 3,
                },
                Event::EndElement,
                Event::Text("\n\n".into()),
                Event::End,
            ]
        );
    }

    /// An empty element is a start tag whose end has no tag of its own,
    /// which the scan says before handing that end out.
    #[test]
    fn an_empty_element_is_announced() {
        let text = "<r><e/><f></f></r>";
        let mut scan = Scan::new();
        assert!(matches!(scan.next_event(text), Ok(Event::StartElement { .. })));
        assert!(!scan.empty_element(), "<r> is not empty");
        assert!(matches!(scan.next_event(text), Ok(Event::StartElement { .. })));
        assert!(scan.empty_element(), "<e/> is empty");
        assert_eq!(scan.next_event(text), Ok(Event::EndElement));
        assert!(!scan.empty_element(), "the end was handed out");
        assert!(matches!(scan.next_event(text), Ok(Event::StartElement { .. })));
        assert!(!scan.empty_element(), "<f> has an end tag of its own");
    }

    /// Where the scan is, the two ways its callers count it: bytes from
    /// 0 into the line (webservices') and characters from 1 (XmlLite's).
    /// Read before a call, it is where the next node starts.
    #[test]
    fn line_position_counts_characters() {
        // The two-byte 'é' makes the byte and the character counts
        // differ, which is the whole point of having both.
        let text = "<r>\n  <é a='1'/>\n</r>";
        let mut scan = Scan::new();
        assert_eq!(scan.line_position(text), (1, 1));
        assert!(matches!(scan.next_event(text), Ok(Event::StartElement { .. })));
        // After <r>: line 1, three characters in.
        assert_eq!(scan.line_position(text), (1, 4));
        assert_eq!(scan.position(), (1, 3));
        // The white space node, then the element: the position before
        // the call is the first character of what the call returns.
        assert_eq!(scan.next_event(text), Ok(Event::Text("\n  ".into())));
        assert_eq!(scan.line_position(text), (2, 3));
        assert_eq!(scan.position(), (2, 2));
        assert!(matches!(scan.next_event(text), Ok(Event::StartElement { .. })));
        // Past `<é a='1'/>`: ten characters, eleven bytes.
        assert_eq!(scan.line_position(text), (2, 13));
        assert_eq!(scan.position(), (2, 13));
        assert_eq!(scan.next_event(text), Ok(Event::EndElement));
    }

    /// Every node of a document, in order, as a scan hands them out.
    #[test]
    fn scan_events_in_order() {
        let text = "<?xml version='1.0'?><!--before-->\n\
                    <r xmlns='u' a='1'><b/>hi<![CDATA[<raw>]]><?p d?></r>";
        let mut scan = Scan::new();
        let mut events = Vec::new();
        loop {
            let event = scan.next_event(text).expect("a well-formed document");
            let done = event == Event::End;
            events.push(event);
            if done {
                break;
            }
        }
        assert_eq!(
            events,
            vec![
                Event::Pi { target: "xml".into(), data: "version='1.0'".into() },
                Event::Comment("before".into()),
                // The line end between the comment and the root: white
                // space outside the root is a node like any other.
                Event::Text("\n".into()),
                Event::StartElement {
                    ns: "u".into(),
                    name: "r".into(),
                    qname: "r".into(),
                    attrs: vec![
                        Attr {
                            ns: XMLNS_NS.into(),
                            name: "xmlns".into(),
                            qname: "xmlns".into(),
                            value: "u".into(),
                        },
                        Attr {
                            ns: String::new(),
                            name: "a".into(),
                            qname: "a".into(),
                            value: "1".into(),
                        },
                    ],
                    line: 2,
                },
                // The empty element is a start and an end.
                Event::StartElement {
                    ns: "u".into(),
                    name: "b".into(),
                    qname: "b".into(),
                    attrs: Vec::new(),
                    line: 2,
                },
                Event::EndElement,
                Event::Text("hi".into()),
                Event::CData("<raw>".into()),
                Event::Pi { target: "p".into(), data: "d".into() },
                Event::EndElement,
                Event::End,
            ]
        );
        // Past the end, every call says so again.
        assert_eq!(scan.next_event(text), Ok(Event::End));
    }

    /// What a tree cannot do: the nodes before a malformed one are
    /// handed out, and the error comes at the node that is wrong.
    #[test]
    fn scan_fails_where_reading_meets_it() {
        let text = "<r><a></r>";
        let mut scan = Scan::new();
        assert!(
            matches!(scan.next_event(text), Ok(Event::StartElement { ref name, .. }) if name == "r")
        );
        assert!(
            matches!(scan.next_event(text), Ok(Event::StartElement { ref name, .. }) if name == "a")
        );
        assert_eq!(scan.depth(), 2);
        assert_eq!(scan.open_name(), Some("a"));
        let err = scan.next_event(text).expect_err("</r> closes <a>");
        assert_eq!(err.msg, "</r> closes <a> (line 1)");
        // The whole document is refused by `parse`, which cannot say
        // which nodes were good.
        assert!(parse(text.as_bytes()).is_err());
    }

    /// A scan refuses what `parse` refuses, where it meets it.
    #[test]
    fn scan_refuses_what_parse_refuses() {
        for (doc, msg) in [
            ("<!DOCTYPE r><r/>", "a document type declaration (DTDs are not allowed)"),
            ("text", "no root element"),
            ("<r/>junk", "content after the root element"),
            ("<r>", "<r> (line 1) is not closed"),
            ("<r><p:a/></r>", "undeclared namespace prefix 'p'"),
            ("<r><?xml v?></r>", "an XML declaration not at the start"),
        ] {
            let mut scan = Scan::new();
            let err = loop {
                match scan.next_event(doc) {
                    Ok(Event::End) => panic!("{doc} was read whole"),
                    Ok(_) => {}
                    Err(e) => break e,
                }
            };
            assert_eq!(err.msg, msg, "{doc}");
        }
    }
}
