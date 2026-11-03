//! A small XML 1.0 reader with namespaces (W3C XML 1.0 and Namespaces
//! in XML 1.0), enough for MSIX's AppxManifest.xml, AppxBlockMap.xml,
//! AppxBundleManifest.xml and OPC's [Content_Types].xml: elements,
//! attributes, text, CDATA, comments, processing instructions, the five
//! predefined entities and character references; UTF-8 (with or without
//! a byte order mark) and UTF-16 with a byte order mark.
//!
//! Refused (an error names the line): document type declarations (no
//! DTDs, so no entity expansion: MSIX packages carry none), undeclared
//! prefixes, mismatched or unclosed tags, duplicate attributes, text or
//! a second element outside the root, nesting deeper than [`MAX_DEPTH`].
//!
//! The result is a tree of [`Element`]s, each with its namespace URI
//! and local name resolved, and the line its start tag is on.

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

/// Deeper nesting is refused (manifests are a few levels deep).
pub const MAX_DEPTH: usize = 256;

/// The namespace the `xml` prefix is bound to.
pub const XML_NS: &str = "http://www.w3.org/XML/1998/namespace";

/// Why a document could not be read: the line (1-based) and what.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    pub line: u32,
    pub msg: String,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "line {}: {}", self.line, self.msg)
    }
}

/// An attribute, its namespace resolved (empty for an unprefixed one).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Attr {
    pub ns: String,
    pub name: String,
    pub value: String,
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
    /// The line of the start tag.
    pub line: u32,
}

impl Element {
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
/// UTF-16 LE/BE with a BOM.
fn decode(bytes: &[u8]) -> Result<String, Error> {
    let bad = |msg: &str| Error { line: 1, msg: msg.into() };
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
    let text = decode(bytes)?;
    Parser { s: text.as_bytes(), text: &text, pos: 0, line: 1 }.document()
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
}

fn is_name_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_' || c == ':' || !c.is_ascii()
}

fn is_name_char(c: char) -> bool {
    is_name_start(c) || c.is_ascii_digit() || c == '-' || c == '.'
}

impl<'t> Parser<'t> {
    fn err<T>(&self, msg: impl Into<String>) -> Result<T, Error> {
        Err(Error { line: self.line, msg: msg.into() })
    }

    fn starts(&self, p: &str) -> bool {
        self.s[self.pos..].starts_with(p.as_bytes())
    }

    /// Moves `n` bytes on, counting lines.
    fn advance(&mut self, n: usize) {
        let end = (self.pos + n).min(self.s.len());
        self.line += self.s[self.pos..end].iter().filter(|&&b| b == b'\n').count() as u32;
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
    fn misc(&mut self) -> Result<(), Error> {
        loop {
            self.skip_space();
            if self.starts("<!--") {
                self.skip_past("-->", "a comment")?;
            } else if self.starts("<?") {
                self.skip_past("?>", "a processing instruction")?;
            } else if self.starts("<!DOCTYPE") {
                return self.err("a document type declaration (DTDs are not allowed)");
            } else {
                return Ok(());
            }
        }
    }

    fn document(mut self) -> Result<Element, Error> {
        self.misc()?;
        if !self.starts("<") || self.starts("</") {
            return self.err("no root element");
        }
        let root = self.element()?;
        self.misc()?;
        if self.pos != self.s.len() {
            return self.err("content after the root element");
        }
        Ok(root)
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
                Err(Error { line, msg: alloc::format!("undeclared namespace prefix '{prefix}'") })
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
                    return Err(Error {
                        line,
                        msg: alloc::format!(
                            "</{name}> closes <{}> (line {})",
                            open.el.qname,
                            open.el.line
                        ),
                    });
                }
                match stack.last_mut() {
                    Some(parent) => parent.el.children.push(open.el),
                    None => return Ok(open.el),
                }
            } else if self.starts("<!--") {
                self.skip_past("-->", "a comment")?;
            } else if self.starts("<![CDATA[") {
                self.advance(9);
                let data = self.skip_past("]]>", "a CDATA section")?;
                match stack.last_mut() {
                    Some(o) => o.el.text.push_str(data),
                    None => return self.err("CDATA outside the root"),
                }
            } else if self.starts("<?") {
                self.skip_past("?>", "a processing instruction")?;
            } else if self.starts("<!") {
                return self.err("markup declarations are not allowed");
            } else if self.starts("<") {
                if stack.len() >= MAX_DEPTH {
                    return self.err("elements nested too deeply");
                }
                let (open, empty) = self.start_tag(&stack)?;
                if empty {
                    match stack.last_mut() {
                        Some(parent) => parent.el.children.push(open.el),
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
                    Some(o) => o.el.text.push_str(&text),
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
                return Err(Error {
                    line: aline,
                    msg: alloc::format!("attribute {an} given twice in <{qname}>"),
                });
            }
            raw.push((an, value, aline));
        };
        let mut bindings = Vec::new();
        for (n, v, _) in &raw {
            if *n == "xmlns" {
                bindings.push((String::new(), v.clone()));
            } else if let Some(p) = n.strip_prefix("xmlns:") {
                if v.is_empty() {
                    return Err(Error { line, msg: alloc::format!("prefix {p} bound to nothing") });
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
                return Err(Error {
                    line: aline,
                    msg: alloc::format!("attribute {{{ans}}}{l} given twice in <{qname}>"),
                });
            }
            attrs.push(Attr { ns: ans, name: l.into(), value });
        }
        let el = Element {
            ns,
            name: local.into(),
            qname: qname.into(),
            attrs,
            children: Vec::new(),
            text: String::new(),
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
}
