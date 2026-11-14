//! A small XML 1.0 reader with namespaces (W3C XML 1.0 and Namespaces
//! in XML 1.0), shared by galapkg (MSIX's AppxManifest.xml,
//! AppxBlockMap.xml, AppxBundleManifest.xml and OPC's
//! [Content_Types].xml) and msxml6.dll's DOM: elements, attributes, text,
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

/// Why a document could not be read: the line (1-based), the column
/// (1-based, in characters) where reading stopped, and what.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    pub line: u32,
    pub column: u32,
    pub msg: String,
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
    let bad = |msg: &str| Error { line: 1, column: 1, msg: msg.into() };
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

    /// An error on `line` (the column is where reading stopped when that
    /// is the current line, else 1).
    fn error_at(&self, line: u32, msg: impl Into<String>) -> Error {
        let column = if line == self.line {
            let end = self.pos.min(self.text.len());
            self.text.get(self.line_start..end).map_or(1, |s| s.chars().count() as u32 + 1)
        } else {
            1
        };
        Error { line, column, msg: msg.into() }
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

    /// Skips up to and including `end`; `what` names the construct.
    fn skip_past(&mut self, end: &str, what: &str) -> Result<&'t str, Error> {
        let start = self.pos;
        match self.text[start..].find(end) {
            Some(i) => {
                self.advance(i + end.len());
                Ok(&self.text[start..start + i])
            }
            None => self.err(alloc::format!("{what} not closed")),
        }
    }

    fn name(&mut self) -> Result<&'t str, Error> {
        let start = self.pos;
        let mut chars = self.text[start..].char_indices();
        match chars.next() {
            Some((_, c)) if is_name_start(c) => {}
            _ => return self.err("a name expected"),
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
            return self.err("an unterminated '&' reference");
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
                    return self.err(alloc::format!("unknown entity &{r}; (no DTDs)"));
                };
                match code.and_then(char::from_u32).filter(|&c| c != '\0') {
                    Some(c) => c,
                    None => return self.err(alloc::format!("a bad character reference &{r};")),
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
                out.push(self.comment()?);
            } else if self.starts("<?") {
                out.push(self.pi()?);
            } else if self.starts("<!DOCTYPE") {
                return self.err("a document type declaration (DTDs are not allowed)");
            } else {
                return Ok(());
            }
        }
    }

    /// A comment (at `<!--`).
    fn comment(&mut self) -> Result<Content, Error> {
        self.advance(4);
        let text = self.skip_past("-->", "a comment")?;
        Ok(Content::Comment(text.into()))
    }

    /// A processing instruction (at `<?`).
    fn pi(&mut self) -> Result<Content, Error> {
        self.advance(2);
        let target = self.name()?;
        let body = self.skip_past("?>", "a processing instruction")?;
        if !body.is_empty() && !body.starts_with([' ', '\t', '\n']) {
            return self.err(alloc::format!("white space expected after <?{target}"));
        }
        let data = body.trim_start_matches([' ', '\t', '\n']);
        Ok(Content::Pi { target: target.into(), data: data.into() })
    }

    fn document(mut self) -> Result<Document, Error> {
        let mut prolog = Vec::new();
        // The XML declaration may only come first.
        if self.starts("<?xml") && !self.starts("<?xml-") {
            prolog.push(self.pi()?);
        }
        self.misc(&mut prolog)?;
        if !self.starts("<") || self.starts("</") {
            return self.err("no root element");
        }
        let root = self.element()?;
        let mut epilog = Vec::new();
        self.misc(&mut epilog)?;
        if self.pos != self.s.len() {
            return self.err("content after the root element");
        }
        let misplaced = |c: &Content| matches!(c, Content::Pi { target, .. } if target.eq_ignore_ascii_case("xml"));
        if prolog.iter().skip(1).chain(epilog.iter()).any(misplaced) {
            return Err(self.error_at(1, "an XML declaration not at the start"));
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
            None => {
                Err(self.error_at(line, alloc::format!("undeclared namespace prefix '{prefix}'")))
            }
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
                    return self.err("an end tag without a start tag");
                };
                if open.el.qname != name {
                    return Err(self.error_at(
                        line,
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
                let c = self.comment()?;
                if let Some(o) = stack.last_mut() {
                    o.el.content.push(c);
                }
            } else if self.starts("<![CDATA[") {
                self.advance(9);
                let data = self.skip_past("]]>", "a CDATA section")?;
                match stack.last_mut() {
                    Some(o) => {
                        o.el.text.push_str(data);
                        o.el.content.push(Content::CData(data.into()));
                    }
                    None => return self.err("CDATA outside the root"),
                }
            } else if self.starts("<?") {
                let pi = self.pi()?;
                if matches!(&pi, Content::Pi { target, .. } if target.eq_ignore_ascii_case("xml")) {
                    return self.err("an XML declaration not at the start");
                }
                if let Some(o) = stack.last_mut() {
                    o.el.content.push(pi);
                }
            } else if self.starts("<!") {
                return self.err("markup declarations are not allowed");
            } else if self.starts("<") {
                if stack.len() >= MAX_DEPTH {
                    return self.err("elements nested too deeply");
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
                return self.err(alloc::format!("<{}> (line {}) is not closed", open.0, open.1));
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
                return self.err(alloc::format!("the start tag <{qname}> is not closed"));
            }
            if !had_space {
                return self.err(alloc::format!("white space expected in <{qname}>"));
            }
            let aline = self.line;
            let an = self.name()?;
            self.skip_space();
            if !self.starts("=") {
                return self.err(alloc::format!("'=' expected after attribute {an}"));
            }
            self.advance(1);
            self.skip_space();
            let quote = match self.s.get(self.pos) {
                Some(&q @ (b'"' | b'\'')) => q,
                _ => return self.err(alloc::format!("a quoted value expected for attribute {an}")),
            };
            self.advance(1);
            let mut value = String::new();
            self.chars_until(&[quote, b'<'], &mut value)?;
            if self.s.get(self.pos) != Some(&quote) {
                return self.err(alloc::format!("'<' in the value of attribute {an}"));
            }
            self.advance(1);
            // Attribute values are normalized: white space becomes spaces.
            let value: String = value
                .chars()
                .map(|c| if matches!(c, '\t' | '\n' | '\r') { ' ' } else { c })
                .collect();
            if raw.iter().any(|(n, _, _)| *n == an) {
                return Err(
                    self.error_at(aline, alloc::format!("attribute {an} given twice in <{qname}>"))
                );
            }
            raw.push((an, value, aline));
        };
        let mut bindings = Vec::new();
        for (n, v, _) in &raw {
            if *n == "xmlns" {
                bindings.push((String::new(), v.clone()));
            } else if let Some(p) = n.strip_prefix("xmlns:") {
                if v.is_empty() {
                    return Err(self.error_at(line, alloc::format!("prefix {p} bound to nothing")));
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
                return Err(self.error_at(
                    aline,
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

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

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
}
