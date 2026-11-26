//! `CreateXmlWriter` and `IXmlWriter`: a document written out, byte for
//! byte as Windows' writer writes it.
//!
//! Everything here is measured in `user/xmllite_test` (its `writer`
//! module writes twenty-six documents and reads the bytes back), so
//! what follows is the contract and not a reading of the standard:
//!
//! - nothing reaches the output stream until it is asked for. Flush
//!   hands over what there is; Release hands over what there is after
//!   closing every element still open -- and a writer whose last call
//!   killed it keeps the unfinished document it had, closing nothing;
//! - WriteEndDocument closes the open elements and writes nothing
//!   itself, so a program that ends a document and never flushes gets
//!   the same bytes from Release;
//! - the declaration is written at the first call that produces bytes,
//!   with the standalone a *successful* WriteStartDocument asked for.
//!   That is what a fragment shows: it has no document to start, and
//!   the declaration still comes out when it was not omitted;
//! - a call that is out of place *and* has no output is answered for
//!   the place first;
//! - a refusal that says the call does not belong where the document
//!   now is (an attribute with no start tag open, a processing
//!   instruction named `xml`, a second root element, a second
//!   WriteStartDocument, a doctype after the root) kills the writer:
//!   every write after it answers that same code while Flush still
//!   answers S_OK, and only a new output revives it. A refusal about
//!   the call's own arguments (a bad name, an undeclared namespace, a
//!   duplicate attribute, text at the top of a document) does not;
//! - it is writing that locks the properties, not SetOutput:
//!   SetProperty answers E_ACCESSDENIED from the first byte on, and a
//!   locked property is still read back.
//!
//! Limits, each recorded in the ledger when a program meets it:
//! WriteAttributes, WriteNode and WriteNodeShallow copy from an
//! `IXmlReader` and are not implemented, the MultiLanguage property is
//! not acted on (the output is UTF-8), and a doctype with a public
//! identifier and no system identifier is a shape no measurement
//! covers.
//!
//! Indenting is measured for elements; where a document holds character
//! data, a CDATA section or white space, which never stand on a line of
//! their own, the element around them is left as it is (ROADMAP OF).

use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, Ordering};

use crate::{
    E_INVALIDARG, E_NOINTERFACE, E_NOTIMPL, E_OUTOFMEMORY, Guid, HResult, IID_ISEQUENTIAL_STREAM,
    IID_ISTREAM, IID_IUNKNOWN, LEDGER_OTHER, LEDGER_REACHED, LEDGER_UNSUPPORTED, Obj, S_OK, data1,
    free_box, guid, not_implemented, put, record, text, try_box,
};

/// `IXmlWriter` (xmllite.idl).
const IID_IXML_WRITER: Guid =
    guid(0x7279_fc88, 0x709d, 0x4095, [0xb6, 0x3d, 0x69, 0xfe, 0x4b, 0x0d, 0x90, 0x30]);

// The writer's own codes (xmllite.h), which the reader does not use.
const E_ACCESSDENIED: HResult = 0x8007_0005u32 as HResult;
const E_UNEXPECTED: HResult = 0x8000_ffffu32 as HResult;
const WR_E_NONWHITESPACE: HResult = 0xc00c_ef01u32 as HResult;
const WR_E_NSPREFIXDECLARED: HResult = 0xc00c_ef02u32 as HResult;
const WR_E_NSPREFIXWITHEMPTYNSURI: HResult = 0xc00c_ef03u32 as HResult;
const WR_E_DUPLICATEATTRIBUTE: HResult = 0xc00c_ef04u32 as HResult;
const WR_E_NAMESPACEUNDECLARED: HResult = 0xc00c_ef09u32 as HResult;
const WR_E_INVALIDXMLSPACE: HResult = 0xc00c_ef0au32 as HResult;
const WR_E_INVALIDACTION: HResult = 0xc00c_ef0bu32 as HResult;
const WC_E_XMLCHARACTER: HResult = 0xc00c_ee2bu32 as HResult;
const WC_E_NAMECHARACTER: HResult = 0xc00c_ee2cu32 as HResult;

// XmlWriterProperty, as the properties are kept: the value a program
// set is the value it reads back, whatever it was.
const PROP_MULTI_LANGUAGE: usize = 0;
const PROP_INDENT: usize = 1;
const PROP_OMIT_XML_DECLARATION: usize = 3;
const PROP_CONFORMANCE_LEVEL: usize = 4;
/// A property that is not documented and reads 0; the enumeration ends
/// here, and everything past it is E_NOTIMPL both ways (measured).
const PROP_UNDOCUMENTED: usize = 5;
const PROPS: usize = 6;

/// XmlConformanceLevel: Fragment allows as many root elements as a
/// program likes, and text outside them.
const LEVEL_FRAGMENT: isize = 1;
const LEVEL_DOCUMENT: isize = 2;

// XmlStandalone.
const STANDALONE_YES: u32 = 1;
const STANDALONE_NO: u32 = 2;

// ---------------------------------------------------------------------
// Names, as XML spells them.
// ---------------------------------------------------------------------

/// An XML name's first character. The letter classes are `char`'s own
/// (gala's scanner reads names the same way).
fn name_start(c: char) -> bool {
    c == '_' || c.is_alphabetic()
}

/// A character inside an XML name.
fn name_char(c: char) -> bool {
    c == '_' || c == '-' || c == '.' || c == '\u{b7}' || c.is_alphanumeric()
}

/// A name with no prefix in it: an element's or an attribute's local
/// name, a prefix, an entity's name.
fn ncname(s: &str) -> bool {
    let mut cs = s.chars();
    cs.next().is_some_and(name_start) && cs.all(name_char)
}

/// A name that may carry a prefix: WriteName's argument and a doctype's
/// name.
fn prefixed_name(s: &str) -> bool {
    match s.split_once(':') {
        Some((prefix, local)) => ncname(prefix) && ncname(local),
        None => ncname(s),
    }
}

/// An Nmtoken, which may start with any name character.
fn nmtoken(s: &str) -> bool {
    !s.is_empty() && s.chars().all(name_char)
}

/// A character that may stand in a document at all (XML 1.0), which is
/// what WriteCharEntity takes.
fn xml_char(c: u32) -> bool {
    c == 0x9
        || c == 0xa
        || c == 0xd
        || (0x20..=0xd7ff).contains(&c)
        || (0xe000..=0xfffd).contains(&c)
}

/// The caller's string: Ok(None) for NULL, and E_OUTOFMEMORY when there
/// is no room to hold it.
fn arg(s: *const u16) -> Result<Option<String>, HResult> {
    if s.is_null() {
        return Ok(None);
    }
    text(s).map(Some).ok_or(E_OUTOFMEMORY)
}

/// The caller's `n` characters, which is how WriteChars and
/// WriteRawChars hand text over (no terminator).
fn arg_n(s: *const u16, n: u32) -> Result<Option<String>, HResult> {
    if s.is_null() {
        return Ok(None);
    }
    let n = n as usize;
    // SAFETY: the caller's buffer of n characters.
    let units = unsafe { core::slice::from_raw_parts(s, n) };
    let mut out = String::new();
    if out.try_reserve(n).is_err() {
        return Err(E_OUTOFMEMORY);
    }
    for c in char::decode_utf16(units.iter().copied()) {
        out.push(c.unwrap_or('\u{fffd}'));
    }
    Ok(Some(out))
}

/// A copy of `s`, or None when there is no room for it.
fn owned(s: &str) -> Option<String> {
    let mut o = String::new();
    o.try_reserve(s.len()).ok()?;
    o.push_str(s);
    Some(o)
}

/// `prefix:local`, or None when there is no room for it.
fn join(prefix: &str, local: &str) -> Option<String> {
    let mut s = String::new();
    s.try_reserve(prefix.len() + 1 + local.len()).ok()?;
    s.push_str(prefix);
    s.push(':');
    s.push_str(local);
    Some(s)
}

// ---------------------------------------------------------------------
// The output stream.
// ---------------------------------------------------------------------

/// The output as a stream to write to: IStream, or ISequentialStream for
/// one that is no more than that -- either alone is enough, measured.
/// The reference the QueryInterface gives is kept until the output is
/// given up. The bool says whether it answered as an IStream, which has
/// Commit where ISequentialStream has nothing at that slot.
fn stream_of(output: Obj) -> Result<(Obj, bool), HResult> {
    for (iid, committable) in [(&IID_ISTREAM, true), (&IID_ISEQUENTIAL_STREAM, false)] {
        let mut out: Obj = core::ptr::null_mut();
        // SAFETY: slot 0 of a COM object's vtable is QueryInterface, and
        // the IID and the output are ours.
        let hr = unsafe {
            let vtable = *(output as *const *const usize);
            let f: extern "system" fn(Obj, *const Guid, *mut Obj) -> HResult =
                core::mem::transmute(*vtable);
            f(output, iid, &mut out)
        };
        if hr >= 0 && !out.is_null() {
            return Ok((out, committable));
        }
    }
    // An output that is no stream: XmlLite also takes an
    // IXmlWriterOutput (CreateXmlWriterOutputWithEncodingName), which
    // gala does not make.
    record(b"IXmlWriter::SetOutput(not a stream)", LEDGER_UNSUPPORTED, 0);
    Err(E_NOINTERFACE)
}

// ---------------------------------------------------------------------
// The object.
// ---------------------------------------------------------------------

#[repr(C)]
struct VTable<const N: usize>([*const (); N]);
// SAFETY: function pointers only.
unsafe impl<const N: usize> Sync for VTable<N> {}

/// An element the writer is inside.
struct Open {
    /// The qualified name as it was written, which the end tag repeats.
    name: String,
    /// How many namespace declarations this element added, to drop when
    /// it ends.
    scopes: usize,
    /// Character data has been written into it, so its end tag stays
    /// where it is when indenting.
    mixed: bool,
    /// A node has been written inside it, so its end tag goes on a line
    /// of its own when indenting.
    children: bool,
}

/// The writer object: its vtable, its references, the output it writes
/// to and where it has got to in the document.
#[repr(C)]
struct Writer {
    vtable: *const (),
    refs: AtomicU32,
    /// The stream SetOutput took, with one reference of ours.
    out: Obj,
    /// That stream answered as an IStream, so it has Commit.
    committable: bool,
    /// What is written and not handed over yet.
    buf: Vec<u8>,
    /// A buffer that would not grow, which the call in progress answers
    /// with E_OUTOFMEMORY.
    oom: bool,
    /// XmlWriterProperty, by its own number. Property 2 is
    /// ByteOrderMark, which is kept and not acted on: a stream output
    /// never gets a mark, whatever it says (measured).
    props: [isize; PROPS],
    /// Bytes have been produced, which locks the properties.
    begun: bool,
    /// The code every write answers until a new output revives the
    /// writer (S_OK while it is alive).
    dead: HResult,
    /// The declaration is settled: written, or left out.
    declared: bool,
    /// WriteStartDocument has been answered, so another is refused.
    started: bool,
    /// XmlStandalone, as WriteStartDocument asked for it.
    standalone: u32,
    /// A doctype has been written.
    dtd: bool,
    /// A root element has been written (a document allows one).
    rooted: bool,
    /// The elements that are open, outermost first.
    open: Vec<Open>,
    /// A start tag whose `>` is still to come.
    in_tag: bool,
    /// The attribute names written into that tag, for a duplicate.
    attrs: Vec<String>,
    /// The namespace declarations in scope, as (prefix, uri); the
    /// default namespace has an empty prefix.
    scope: Vec<(String, String)>,
    /// A declaration an attribute needed, to write at the end of the
    /// start tag (measured: that is where it goes).
    pending: Vec<u8>,
}

impl Writer {
    fn new() -> Self {
        Writer {
            vtable: &WRITER_VTABLE as *const VTable<32> as *const (),
            refs: AtomicU32::new(1),
            out: core::ptr::null_mut(),
            committable: false,
            buf: Vec::new(),
            oom: false,
            props: [0, 0, 1, 0, LEVEL_DOCUMENT, 0],
            begun: false,
            dead: S_OK,
            declared: false,
            started: false,
            standalone: 0,
            dtd: false,
            rooted: false,
            open: Vec::new(),
            in_tag: false,
            attrs: Vec::new(),
            scope: Vec::new(),
            pending: Vec::new(),
        }
    }

    /// Back to a writer with nothing written, which a new output does:
    /// the properties stay (they are the writer's), everything a
    /// document left behind goes.
    fn reset(&mut self) {
        self.buf = Vec::new();
        self.oom = false;
        self.begun = false;
        self.dead = S_OK;
        self.declared = false;
        self.started = false;
        self.standalone = 0;
        self.dtd = false;
        self.rooted = false;
        self.open = Vec::new();
        self.in_tag = false;
        self.attrs = Vec::new();
        self.scope = Vec::new();
        self.pending = Vec::new();
    }

    fn indenting(&self) -> bool {
        self.props[PROP_INDENT] != 0
    }

    fn omitting(&self) -> bool {
        self.props[PROP_OMIT_XML_DECLARATION] != 0
    }

    fn fragment(&self) -> bool {
        self.props[PROP_CONFORMANCE_LEVEL] == LEVEL_FRAGMENT
    }

    // -----------------------------------------------------------------
    // Bytes.
    // -----------------------------------------------------------------

    /// Appends to what is not handed over yet.
    fn emit(&mut self, bytes: &[u8]) {
        if self.buf.try_reserve(bytes.len()).is_err() {
            self.oom = true;
            return;
        }
        self.buf.extend_from_slice(bytes);
        self.begun = true;
    }

    fn emit_char(&mut self, c: char) {
        let mut b = [0u8; 4];
        let n = c.encode_utf8(&mut b).len();
        self.emit(&b[..n]);
    }

    /// A character reference, `&#x` and the code point in upper-case
    /// hexadecimal with no leading zeros (measured).
    fn emit_reference(&mut self, code: u32) {
        self.emit(b"&#x");
        let mut digits = [0u8; 8];
        let mut n = 0;
        let mut v = code;
        loop {
            digits[n] = b"0123456789ABCDEF"[(v & 0xf) as usize];
            n += 1;
            v >>= 4;
            if v == 0 {
                break;
            }
        }
        digits[..n].reverse();
        self.emit(&digits[..n]);
        self.emit(b";");
    }

    /// An attribute's value: `<`, `&` and `"` by name, a line end or a
    /// tab by reference, and `>` and `'` as they are (measured).
    fn emit_attr_value(&mut self, s: &str) {
        for c in s.chars() {
            match c {
                '<' => self.emit(b"&lt;"),
                '&' => self.emit(b"&amp;"),
                '"' => self.emit(b"&quot;"),
                '\t' => self.emit(b"&#x9;"),
                '\n' => self.emit(b"&#xA;"),
                '\r' => self.emit(b"&#xD;"),
                _ => self.emit_char(c),
            }
        }
    }

    /// Character data: `<`, `&` and `>` by name, every line end as
    /// CRLF, and `"`, `'` and a tab as they are (measured).
    fn emit_text(&mut self, s: &str) {
        let mut cs = s.chars().peekable();
        while let Some(c) = cs.next() {
            match c {
                '<' => self.emit(b"&lt;"),
                '&' => self.emit(b"&amp;"),
                '>' => self.emit(b"&gt;"),
                '\r' => {
                    if cs.peek() == Some(&'\n') {
                        cs.next();
                    }
                    self.emit(b"\r\n");
                }
                '\n' => self.emit(b"\r\n"),
                _ => self.emit_char(c),
            }
        }
    }

    /// The XML declaration, at the first call that produces bytes.
    fn declaration(&mut self) {
        if self.declared {
            return;
        }
        self.declared = true;
        if self.omitting() {
            return;
        }
        self.emit(b"<?xml version=\"1.0\" encoding=\"UTF-8\"");
        match self.standalone {
            STANDALONE_YES => self.emit(b" standalone=\"yes\""),
            STANDALONE_NO => self.emit(b" standalone=\"no\""),
            _ => {}
        }
        self.emit(b"?>");
    }

    /// CRLF and two spaces a level before a node that stands on a line
    /// of its own: when Indent is on, something is written already and
    /// the element around it holds no character data.
    fn indent_at(&mut self, depth: usize) {
        if !self.indenting() || !self.begun {
            return;
        }
        if self.open.last().is_some_and(|e| e.mixed) {
            return;
        }
        self.emit(b"\r\n");
        for _ in 0..depth {
            self.emit(b"  ");
        }
    }

    /// The namespace a prefix stands for here, innermost declaration
    /// first.
    fn uri_of(&self, prefix: &str) -> Option<&str> {
        self.scope.iter().rev().find(|(p, _)| p == prefix).map(|(_, u)| u.as_str())
    }

    /// A prefix that stands for `uri` here (never the default
    /// namespace, which an attribute cannot use).
    fn prefix_of(&self, uri: &str) -> Option<&str> {
        self.scope.iter().rev().find(|(p, u)| !p.is_empty() && u == uri).map(|(p, _)| p.as_str())
    }

    /// Remembers a declaration for the element now open.
    fn declared_here(&mut self, prefix: &str, uri: &str) -> bool {
        let (Some(p), Some(u)) = (owned(prefix), owned(uri)) else {
            return false;
        };
        if self.scope.try_reserve(1).is_err() {
            return false;
        }
        self.scope.push((p, u));
        if let Some(e) = self.open.last_mut() {
            e.scopes += 1;
        }
        true
    }

    /// ` xmlns="uri"` or ` xmlns:prefix="uri"`, in the start tag now
    /// open.
    fn emit_declaration(&mut self, prefix: &str, uri: &str) {
        if prefix.is_empty() {
            self.emit(b" xmlns=\"");
        } else {
            self.emit(b" xmlns:");
            self.emit(prefix.as_bytes());
            self.emit(b"=\"");
        }
        self.emit_attr_value(uri);
        self.emit(b"\"");
    }

    /// A prefix for an attribute's namespace that nothing declares:
    /// `p` and one more than the prefixes in scope (measured, `p1`
    /// where nothing is declared and `p2` beside one that is).
    fn invented_prefix(&self) -> Option<String> {
        let n = self.scope.iter().filter(|(p, _)| !p.is_empty()).count() + 1;
        let mut s = String::new();
        s.try_reserve(8).ok()?;
        s.push('p');
        // n is small; its decimal digits, most significant first.
        let mut place = 1;
        while place * 10 <= n {
            place *= 10;
        }
        let mut left = n;
        while place > 0 {
            s.push((b'0' + (left / place) as u8) as char);
            left %= place;
            place /= 10;
        }
        Some(s)
    }

    /// The `>` that ends a start tag, with the declarations an
    /// attribute needed, which go at the end of the tag (measured).
    fn finish_tag(&mut self) {
        if !self.in_tag {
            return;
        }
        self.in_tag = false;
        let pending = core::mem::take(&mut self.pending);
        self.emit(&pending);
        self.emit(b">");
    }

    /// The element that is open, closed: ` />` while its start tag is
    /// still open, `</name>` otherwise.
    fn close_element(&mut self) {
        let Some(e) = self.open.pop() else {
            return;
        };
        self.scope.truncate(self.scope.len().saturating_sub(e.scopes));
        if self.in_tag {
            self.in_tag = false;
            let pending = core::mem::take(&mut self.pending);
            self.emit(&pending);
            self.emit(b" />");
        } else {
            if e.children && !e.mixed {
                let depth = self.open.len();
                self.indent_at(depth);
            }
            self.emit(b"</");
            self.emit(e.name.as_bytes());
            self.emit(b">");
        }
    }

    /// Every element still open, closed: what WriteEndDocument does,
    /// and Release after it on a writer that is still alive.
    fn close_all(&mut self) {
        if self.dead != S_OK {
            return;
        }
        while !self.open.is_empty() {
            self.close_element();
        }
    }

    /// A node is being written inside the element that is open: its
    /// start tag ends here, and it has a child now.
    fn open_content(&mut self, character_data: bool) {
        self.declaration();
        self.finish_tag();
        if let Some(e) = self.open.last_mut() {
            if character_data {
                e.mixed = true;
            } else {
                e.children = true;
            }
        }
    }

    // -----------------------------------------------------------------
    // What a call answers before it looks at its arguments.
    // -----------------------------------------------------------------

    /// The code that killed the writer, or S_OK.
    fn alive(&self) -> HResult {
        self.dead
    }

    /// A refusal that says the call does not belong where the document
    /// now is: the writer keeps it and answers it for every write
    /// after.
    fn poison(&mut self, hr: HResult) -> HResult {
        self.dead = hr;
        hr
    }

    /// Where character data may go: inside an element, or anywhere in a
    /// fragment. A document's top level takes white space only.
    fn text_place(&self) -> HResult {
        if self.open.is_empty() && !self.fragment() { WR_E_INVALIDACTION } else { S_OK }
    }

    /// E_OUTOFMEMORY when the buffer would not grow, S_OK otherwise.
    fn written(&self) -> HResult {
        if self.oom { E_OUTOFMEMORY } else { S_OK }
    }

    // -----------------------------------------------------------------
    // The stream.
    // -----------------------------------------------------------------

    /// Hands the bytes over: Write, and Commit for a stream that has
    /// one. Nothing buffered is nothing to hand over.
    fn hand_over(&mut self) -> HResult {
        if self.out.is_null() || self.buf.is_empty() {
            return S_OK;
        }
        let mut wrote: u32 = 0;
        // SAFETY: slot 4 is ISequentialStream::Write (IStream's too);
        // the buffer and the count are ours.
        let hr = unsafe {
            let vtable = *(self.out as *const *const usize);
            let f: extern "system" fn(Obj, *const u8, u32, *mut u32) -> HResult =
                core::mem::transmute(*vtable.add(4));
            f(self.out, self.buf.as_ptr(), self.buf.len() as u32, &mut wrote)
        };
        let handed = self.buf.len() as u64;
        self.buf.clear();
        if hr < 0 {
            record(b"IXmlWriter::Flush(stream write)", LEDGER_OTHER, hr as u32 as u64);
            return hr;
        }
        // Not a gap: what a program actually got out of the writer, so a
        // run that ends elsewhere still says a document was written and
        // how long it was.
        record(b"IXmlWriter hands its bytes over", LEDGER_REACHED, handed);
        if self.committable {
            // SAFETY: slot 8 is IStream::Commit, which the output
            // answered QueryInterface for.
            unsafe {
                let vtable = *(self.out as *const *const usize);
                let f: extern "system" fn(Obj, u32) -> HResult =
                    core::mem::transmute(*vtable.add(8));
                f(self.out, 0)
            };
        }
        S_OK
    }

    /// Gives the output back, which SetOutput and the last Release do.
    fn drop_output(&mut self) {
        if self.out.is_null() {
            return;
        }
        let out = core::mem::replace(&mut self.out, core::ptr::null_mut());
        self.committable = false;
        // SAFETY: slot 2 is IUnknown::Release, and the reference is the
        // one our QueryInterface took.
        unsafe {
            let vtable = *(out as *const *const usize);
            let f: extern "system" fn(Obj) -> u32 = core::mem::transmute(*vtable.add(2));
            f(out)
        };
    }

    // -----------------------------------------------------------------
    // The writes.
    // -----------------------------------------------------------------

    fn start_document(&mut self, standalone: u32) -> HResult {
        if self.alive() != S_OK {
            return self.alive();
        }
        // A fragment has no document to start, and that is the answer
        // even with nowhere to write it (measured).
        if self.fragment() {
            return WR_E_INVALIDACTION;
        }
        if self.started || self.declared || self.begun {
            return self.poison(WR_E_INVALIDACTION);
        }
        if self.out.is_null() {
            return E_UNEXPECTED;
        }
        self.started = true;
        self.standalone = standalone;
        S_OK
    }

    fn start_element(
        &mut self,
        prefix: Option<&str>,
        local: Option<&str>,
        uri: Option<&str>,
    ) -> HResult {
        if self.alive() != S_OK {
            return self.alive();
        }
        // A document has one root element.
        if !self.fragment() && self.open.is_empty() && self.rooted {
            return self.poison(WR_E_INVALIDACTION);
        }
        if self.out.is_null() {
            return E_UNEXPECTED;
        }
        let Some(local) = local.filter(|s| !s.is_empty()) else {
            return E_INVALIDARG;
        };
        if !ncname(local) {
            return WC_E_NAMECHARACTER;
        }
        let prefix = prefix.filter(|s| !s.is_empty());
        if prefix.is_some_and(|p| !ncname(p)) {
            return WC_E_NAMECHARACTER;
        }
        let uri = uri.filter(|s| !s.is_empty());
        // The name as it will be written, and the declaration it needs.
        let (name, declare) = match (prefix, uri) {
            (Some(p), Some(u)) => {
                let known = self.uri_of(p) == Some(u);
                (join(p, local), if known { None } else { Some(u) })
            }
            (Some(p), None) => {
                // A prefix with no namespace, and none in scope for it.
                if self.uri_of(p).is_none() {
                    return WR_E_NSPREFIXWITHEMPTYNSURI;
                }
                (join(p, local), None)
            }
            (None, Some(u)) => {
                let known = self.uri_of("") == Some(u);
                (owned(local), if known { None } else { Some(u) })
            }
            (None, None) => (owned(local), None),
        };
        let Some(name) = name else {
            return E_OUTOFMEMORY;
        };
        if self.open.try_reserve(1).is_err() {
            return E_OUTOFMEMORY;
        }
        if self.open.is_empty() {
            self.rooted = true;
        }
        self.open_content(false);
        let depth = self.open.len();
        self.indent_at(depth);
        self.emit(b"<");
        self.emit(name.as_bytes());
        self.open.push(Open { name, scopes: 0, mixed: false, children: false });
        self.in_tag = true;
        self.attrs = Vec::new();
        self.pending = Vec::new();
        if let Some(u) = declare {
            let p = prefix.unwrap_or("");
            self.emit_declaration(p, u);
            if !self.declared_here(p, u) {
                return E_OUTOFMEMORY;
            }
        }
        self.written()
    }

    fn attribute(
        &mut self,
        prefix: Option<&str>,
        local: Option<&str>,
        uri: Option<&str>,
        value: Option<&str>,
    ) -> HResult {
        if self.alive() != S_OK {
            return self.alive();
        }
        // An attribute wants a start tag that is still open, and asking
        // for one anywhere else kills the writer (measured).
        if !self.in_tag {
            return self.poison(WR_E_INVALIDACTION);
        }
        if self.out.is_null() {
            return E_UNEXPECTED;
        }
        let Some(local) = local.filter(|s| !s.is_empty()) else {
            return E_INVALIDARG;
        };
        if !ncname(local) {
            return WC_E_NAMECHARACTER;
        }
        let prefix = prefix.filter(|s| !s.is_empty());
        if prefix.is_some_and(|p| !ncname(p)) {
            return WC_E_NAMECHARACTER;
        }
        let uri = uri.filter(|s| !s.is_empty());
        let value = value.unwrap_or("");

        // The reserved prefixes, which are not namespaces to resolve.
        if prefix == Some("xmlns") {
            let Some(name) = join("xmlns", local) else {
                return E_OUTOFMEMORY;
            };
            if let Some(hr) = self.duplicate(&name) {
                return hr;
            }
            let declared = uri.unwrap_or(value);
            self.emit(b" ");
            self.emit(name.as_bytes());
            self.emit(b"=\"");
            self.emit_attr_value(declared);
            self.emit(b"\"");
            if !self.declared_here(local, declared) || !self.keep(name) {
                return E_OUTOFMEMORY;
            }
            return self.written();
        }
        if prefix.is_none() && local == "xmlns" {
            // The default namespace is declared by WriteStartElement's
            // namespace argument, not by an attribute.
            return WR_E_NSPREFIXDECLARED;
        }
        if prefix == Some("xml") && local == "space" && value != "default" && value != "preserve" {
            return WR_E_INVALIDXMLSPACE;
        }

        // The name as it will be written, and the declaration it needs,
        // which goes at the end of the start tag.
        let mut declare: Option<(String, &str)> = None;
        let name = match (prefix, uri) {
            // The xml prefix is declared everywhere.
            (Some("xml"), _) => join("xml", local),
            (Some(p), Some(u)) => {
                if self.uri_of(p) != Some(u) {
                    match owned(p) {
                        Some(p) => declare = Some((p, u)),
                        None => return E_OUTOFMEMORY,
                    }
                }
                join(p, local)
            }
            // A prefix nothing declares, with no namespace to declare
            // it with, is dropped (measured).
            (Some(p), None) => {
                if self.uri_of(p).is_some() {
                    join(p, local)
                } else {
                    owned(local)
                }
            }
            (None, Some(u)) => match self.prefix_of(u) {
                Some(p) => join(p, local),
                None => match self.invented_prefix() {
                    Some(p) => {
                        let name = join(&p, local);
                        declare = Some((p, u));
                        name
                    }
                    None => return E_OUTOFMEMORY,
                },
            },
            (None, None) => owned(local),
        };
        let Some(name) = name else {
            return E_OUTOFMEMORY;
        };
        if let Some(hr) = self.duplicate(&name) {
            return hr;
        }
        self.emit(b" ");
        self.emit(name.as_bytes());
        self.emit(b"=\"");
        self.emit_attr_value(value);
        self.emit(b"\"");
        if !self.keep(name) {
            return E_OUTOFMEMORY;
        }
        if let Some((p, u)) = declare {
            let mut bytes = core::mem::take(&mut self.pending);
            core::mem::swap(&mut self.buf, &mut bytes);
            self.emit_declaration(&p, u);
            core::mem::swap(&mut self.buf, &mut bytes);
            self.pending = bytes;
            if !self.declared_here(&p, u) {
                return E_OUTOFMEMORY;
            }
        }
        self.written()
    }

    /// The code for an attribute this start tag already has.
    fn duplicate(&self, name: &str) -> Option<HResult> {
        self.attrs.iter().any(|a| a == name).then_some(WR_E_DUPLICATEATTRIBUTE)
    }

    /// Keeps an attribute's name, to answer the next one that repeats
    /// it.
    fn keep(&mut self, name: String) -> bool {
        if self.attrs.try_reserve(1).is_err() {
            return false;
        }
        self.attrs.push(name);
        true
    }

    fn end_element(&mut self, full: bool) -> HResult {
        if self.alive() != S_OK {
            return self.alive();
        }
        // Nothing to end: the call is wrong, and the writer is not
        // (measured).
        if self.open.is_empty() {
            return E_INVALIDARG;
        }
        if self.out.is_null() {
            return E_UNEXPECTED;
        }
        if full {
            self.finish_tag();
        }
        self.close_element();
        self.written()
    }

    fn end_document(&mut self) -> HResult {
        if self.alive() != S_OK {
            return self.alive();
        }
        // Where WriteStartDocument answers for the place before the
        // output, this one answers for the output first (measured,
        // both).
        if self.out.is_null() {
            return E_UNEXPECTED;
        }
        // A fragment has no document to end (measured); a document
        // that is finished already has nothing left to close, which is
        // not an error.
        if self.fragment() {
            return WR_E_INVALIDACTION;
        }
        while !self.open.is_empty() {
            self.close_element();
        }
        self.written()
    }

    /// WriteString and WriteChars: character data.
    fn string(&mut self, value: Option<&str>) -> HResult {
        if self.alive() != S_OK {
            return self.alive();
        }
        let place = self.text_place();
        if place != S_OK {
            return place;
        }
        if self.out.is_null() {
            return E_UNEXPECTED;
        }
        // Nothing to write is not an error (measured).
        let Some(value) = value else {
            return S_OK;
        };
        self.open_content(true);
        self.emit_text(value);
        self.written()
    }

    /// WriteRaw and WriteRawChars: markup of the caller's own, which is
    /// written as it is.
    fn raw(&mut self, value: Option<&str>) -> HResult {
        if self.alive() != S_OK {
            return self.alive();
        }
        if self.out.is_null() {
            return E_UNEXPECTED;
        }
        let Some(value) = value else {
            return S_OK;
        };
        self.open_content(true);
        self.emit(value.as_bytes());
        self.written()
    }

    fn whitespace(&mut self, value: Option<&str>) -> HResult {
        if self.alive() != S_OK {
            return self.alive();
        }
        if self.out.is_null() {
            return E_UNEXPECTED;
        }
        let Some(value) = value else {
            return S_OK;
        };
        // White space is allowed at the top of a document where text is
        // not, and anything else here is refused without killing the
        // writer (measured).
        if !value.chars().all(|c| c == ' ' || c == '\t' || c == '\r' || c == '\n') {
            return WR_E_NONWHITESPACE;
        }
        self.open_content(true);
        self.emit(value.as_bytes());
        self.written()
    }

    fn cdata(&mut self, value: Option<&str>) -> HResult {
        if self.alive() != S_OK {
            return self.alive();
        }
        let place = self.text_place();
        if place != S_OK {
            return place;
        }
        if self.out.is_null() {
            return E_UNEXPECTED;
        }
        self.open_content(true);
        self.emit(b"<![CDATA[");
        // A section cannot hold what ends it, so the text is cut in two
        // around it (measured).
        let mut left = value.unwrap_or("");
        while let Some(at) = left.find("]]>") {
            self.emit(&left.as_bytes()[..at]);
            self.emit(b"]]]]><![CDATA[>");
            left = &left[at + 3..];
        }
        self.emit(left.as_bytes());
        self.emit(b"]]>");
        self.written()
    }

    fn comment(&mut self, value: Option<&str>) -> HResult {
        if self.alive() != S_OK {
            return self.alive();
        }
        if self.out.is_null() {
            return E_UNEXPECTED;
        }
        self.open_content(false);
        let depth = self.open.len();
        self.indent_at(depth);
        self.emit(b"<!--");
        // A comment keeps what would end it apart instead of refusing
        // it: a hyphen after a hyphen, and one at the end, gets a space
        // before the next (measured).
        let mut last = '\0';
        for c in value.unwrap_or("").chars() {
            if c == '-' && last == '-' {
                self.emit(b" ");
            }
            self.emit_char(c);
            last = c;
        }
        if last == '-' {
            self.emit(b" ");
        }
        self.emit(b"-->");
        self.written()
    }

    fn pi(&mut self, name: Option<&str>, value: Option<&str>) -> HResult {
        if self.alive() != S_OK {
            return self.alive();
        }
        if self.out.is_null() {
            return E_UNEXPECTED;
        }
        let Some(name) = name.filter(|s| !s.is_empty()) else {
            return E_INVALIDARG;
        };
        if !prefixed_name(name) {
            return WC_E_NAMECHARACTER;
        }
        // The one reserved name, which kills the writer (measured; XML
        // reserves it however it is spelled).
        if name.eq_ignore_ascii_case("xml") {
            return self.poison(WR_E_INVALIDACTION);
        }
        self.open_content(false);
        let depth = self.open.len();
        self.indent_at(depth);
        self.emit(b"<?");
        self.emit(name.as_bytes());
        if let Some(value) = value.filter(|s| !s.is_empty()) {
            self.emit(b" ");
            self.emit(value.as_bytes());
        }
        self.emit(b"?>");
        self.written()
    }

    fn doc_type(
        &mut self,
        name: Option<&str>,
        public: Option<&str>,
        system: Option<&str>,
        subset: Option<&str>,
    ) -> HResult {
        if self.alive() != S_OK {
            return self.alive();
        }
        // A doctype goes before the root element and there is one of
        // them; asking for another kills the writer (measured).
        if self.dtd || self.rooted || !self.open.is_empty() {
            return self.poison(WR_E_INVALIDACTION);
        }
        if self.out.is_null() {
            return E_UNEXPECTED;
        }
        let Some(name) = name.filter(|s| !s.is_empty()) else {
            return E_INVALIDARG;
        };
        if !prefixed_name(name) {
            return WC_E_NAMECHARACTER;
        }
        self.declaration();
        let depth = self.open.len();
        self.indent_at(depth);
        self.emit(b"<!DOCTYPE ");
        self.emit(name.as_bytes());
        match (public.filter(|s| !s.is_empty()), system.filter(|s| !s.is_empty())) {
            (Some(public), system) => {
                if system.is_none() {
                    // A public identifier comes with a system one in
                    // every document this was measured against.
                    record(b"IXmlWriter::WriteDocType(public, no system)", LEDGER_OTHER, 0);
                }
                self.emit(b" PUBLIC \"");
                self.emit(public.as_bytes());
                self.emit(b"\" \"");
                self.emit(system.unwrap_or("").as_bytes());
                self.emit(b"\"");
            }
            (None, Some(system)) => {
                self.emit(b" SYSTEM \"");
                self.emit(system.as_bytes());
                self.emit(b"\"");
            }
            (None, None) => {}
        }
        if let Some(subset) = subset.filter(|s| !s.is_empty()) {
            self.emit(b" [");
            self.emit(subset.as_bytes());
            self.emit(b"]");
        }
        self.emit(b">");
        self.dtd = true;
        self.written()
    }

    fn char_entity(&mut self, ch: u16) -> HResult {
        if self.alive() != S_OK {
            return self.alive();
        }
        let place = self.text_place();
        if place != S_OK {
            return place;
        }
        if self.out.is_null() {
            return E_UNEXPECTED;
        }
        if !xml_char(u32::from(ch)) {
            return WC_E_XMLCHARACTER;
        }
        self.open_content(true);
        self.emit_reference(u32::from(ch));
        self.written()
    }

    fn surrogate(&mut self, low: u16, high: u16) -> HResult {
        if self.alive() != S_OK {
            return self.alive();
        }
        let place = self.text_place();
        if place != S_OK {
            return place;
        }
        if self.out.is_null() {
            return E_UNEXPECTED;
        }
        if !(0xdc00..=0xdfff).contains(&low) || !(0xd800..=0xdbff).contains(&high) {
            return WC_E_XMLCHARACTER;
        }
        let code = 0x1_0000 + ((u32::from(high) - 0xd800) << 10) + (u32::from(low) - 0xdc00);
        self.open_content(true);
        self.emit_reference(code);
        self.written()
    }

    fn entity_ref(&mut self, name: Option<&str>) -> HResult {
        if self.alive() != S_OK {
            return self.alive();
        }
        let place = self.text_place();
        if place != S_OK {
            return place;
        }
        if self.out.is_null() {
            return E_UNEXPECTED;
        }
        let Some(name) = name.filter(|s| !s.is_empty()) else {
            return E_INVALIDARG;
        };
        // Whether anything declared it is the document's business, not
        // the writer's (measured: an unknown entity is written).
        if !prefixed_name(name) {
            return WC_E_NAMECHARACTER;
        }
        self.open_content(true);
        self.emit(b"&");
        self.emit(name.as_bytes());
        self.emit(b";");
        self.written()
    }

    /// WriteName and WriteNmToken: text that is a name, written as it
    /// is.
    fn name(&mut self, value: Option<&str>, token: bool) -> HResult {
        if self.alive() != S_OK {
            return self.alive();
        }
        let place = self.text_place();
        if place != S_OK {
            return place;
        }
        if self.out.is_null() {
            return E_UNEXPECTED;
        }
        let Some(value) = value.filter(|s| !s.is_empty()) else {
            return E_INVALIDARG;
        };
        let ok = if token { nmtoken(value) } else { prefixed_name(value) };
        if !ok {
            return WC_E_NAMECHARACTER;
        }
        self.open_content(true);
        self.emit(value.as_bytes());
        self.written()
    }

    fn qualified_name(&mut self, local: Option<&str>, uri: Option<&str>) -> HResult {
        if self.alive() != S_OK {
            return self.alive();
        }
        let place = self.text_place();
        if place != S_OK {
            return place;
        }
        if self.out.is_null() {
            return E_UNEXPECTED;
        }
        let Some(local) = local.filter(|s| !s.is_empty()) else {
            return E_INVALIDARG;
        };
        if !ncname(local) {
            return WC_E_NAMECHARACTER;
        }
        // A name in a namespace nothing declared cannot be written, and
        // the refusal does not kill the writer (measured).
        let prefix = match uri.filter(|s| !s.is_empty()) {
            None => None,
            Some(uri) => {
                if self.uri_of("") == Some(uri) {
                    None
                } else {
                    match self.prefix_of(uri) {
                        Some(p) => match owned(p) {
                            Some(p) => Some(p),
                            None => return E_OUTOFMEMORY,
                        },
                        None => return WR_E_NAMESPACEUNDECLARED,
                    }
                }
            }
        };
        self.open_content(true);
        if let Some(prefix) = prefix {
            self.emit(prefix.as_bytes());
            self.emit(b":");
        }
        self.emit(local.as_bytes());
        self.written()
    }

    /// WriteElementString: a start tag, a value if there is one, and an
    /// end tag -- an empty element where there is none (measured).
    fn element_string(
        &mut self,
        prefix: Option<&str>,
        local: Option<&str>,
        uri: Option<&str>,
        value: Option<&str>,
    ) -> HResult {
        let hr = self.start_element(prefix, local, uri);
        if hr != S_OK {
            return hr;
        }
        if value.is_some() {
            let hr = self.string(value);
            if hr != S_OK {
                return hr;
            }
        }
        self.end_element(false)
    }
}

/// `this` as the writer we handed out.
fn obj<'a>(this: Obj) -> &'a mut Writer {
    // SAFETY: a live writer of ours; a writer is used by one thread at a
    // time (XmlLite's own is not thread safe either).
    unsafe { &mut *(this as *mut Writer) }
}

/// CreateXmlWriter: a writer with no output, whose properties are the
/// defaults and whose writes are E_UNEXPECTED until one is set.
#[unsafe(no_mangle)]
pub extern "system" fn CreateXmlWriter(riid: *const Guid, out: *mut Obj, malloc: Obj) -> HResult {
    // A writer with nowhere to put it is E_INVALIDARG, where the reader
    // answers E_POINTER (measured, both of them).
    if out.is_null() || riid.is_null() {
        return E_INVALIDARG;
    }
    put(out, core::ptr::null_mut());
    // SAFETY: the caller's IID.
    let iid = unsafe { *riid };
    if iid != IID_IXML_WRITER && iid != IID_IUNKNOWN {
        record(b"CreateXmlWriter(iid)", LEDGER_UNSUPPORTED, data1(&iid));
        return E_NOINTERFACE;
    }
    if !malloc.is_null() {
        // A caller's IMalloc: the writer allocates from gala's heap
        // instead, which the ledger says rather than leaving it to be
        // guessed at.
        record(b"CreateXmlWriter(IMalloc)", LEDGER_UNSUPPORTED, 0);
    }
    match try_box(Writer::new()) {
        Some(p) => {
            put(out, p.cast());
            // Not a gap: a writer was made. Without this row a run says
            // nothing about a program that asked for one and then wrote
            // nothing, which is the shape a failure here takes.
            record(b"CreateXmlWriter", LEDGER_REACHED, 0);
            S_OK
        }
        None => E_OUTOFMEMORY,
    }
}

extern "system" fn query_interface(this: Obj, riid: *const Guid, out: *mut Obj) -> HResult {
    if out.is_null() || riid.is_null() {
        return E_INVALIDARG;
    }
    put(out, core::ptr::null_mut());
    // SAFETY: the caller's IID.
    let iid = unsafe { *riid };
    if iid != IID_IXML_WRITER && iid != IID_IUNKNOWN {
        record(b"IXmlWriter::QueryInterface(iid)", LEDGER_UNSUPPORTED, data1(&iid));
        return E_NOINTERFACE;
    }
    add_ref(this);
    put(out, this);
    S_OK
}

extern "system" fn add_ref(this: Obj) -> u32 {
    obj(this).refs.fetch_add(1, Ordering::Relaxed) + 1
}

/// Release gives up a reference, and the last one ends the document:
/// what is open is closed and the bytes are handed over, where Flush
/// closes nothing (measured). A writer that was killed keeps the
/// unfinished document it had.
extern "system" fn release(this: Obj) -> u32 {
    let left = obj(this).refs.fetch_sub(1, Ordering::AcqRel) - 1;
    if left == 0 {
        let w = obj(this);
        w.close_all();
        w.hand_over();
        w.drop_output();
        // SAFETY: the last reference; nothing uses the object after.
        unsafe { free_box(this as *mut Writer) };
    }
    left
}

/// SetOutput: the stream to write to, which starts the writer over --
/// and NULL gives the output up, leaving the writes E_UNEXPECTED
/// (measured). What was buffered and not flushed is dropped with it.
extern "system" fn set_output(this: Obj, output: Obj) -> HResult {
    let w = obj(this);
    w.drop_output();
    w.reset();
    if output.is_null() {
        return S_OK;
    }
    match stream_of(output) {
        Ok((stream, committable)) => {
            w.out = stream;
            w.committable = committable;
            S_OK
        }
        Err(hr) => hr,
    }
}

extern "system" fn get_property(this: Obj, property: u32, value: *mut isize) -> HResult {
    // Past the end of the enumeration the out value is left as it was
    // (measured).
    let Some(&v) = obj(this).props.get(property as usize) else {
        return E_NOTIMPL;
    };
    if value.is_null() {
        return E_INVALIDARG;
    }
    put(value, v);
    S_OK
}

extern "system" fn set_property(this: Obj, property: u32, value: isize) -> HResult {
    let w = obj(this);
    if property as usize >= PROPS {
        return E_NOTIMPL;
    }
    // It is writing that locks the properties, not the output.
    if w.begun {
        return E_ACCESSDENIED;
    }
    match property as usize {
        PROP_MULTI_LANGUAGE if value != 0 => {
            // gala writes UTF-8 and nothing else.
            record(b"IXmlWriter::SetProperty(MultiLanguage)", LEDGER_UNSUPPORTED, value as u64);
        }
        PROP_UNDOCUMENTED if value != 0 => {
            record(b"IXmlWriter::SetProperty(5)", LEDGER_UNSUPPORTED, value as u64);
        }
        _ => {}
    }
    w.props[property as usize] = value;
    S_OK
}

/// WriteAttributes, WriteNode and WriteNodeShallow copy a reader's node
/// into the document, which gala does not do: the ledger names the
/// method and the program gets E_NOTIMPL.
extern "system" fn write_attributes(_: Obj, _: Obj, _: i32) -> HResult {
    not_implemented(b"IXmlWriter::WriteAttributes")
}

extern "system" fn write_node(_: Obj, _: Obj, _: i32) -> HResult {
    not_implemented(b"IXmlWriter::WriteNode")
}

extern "system" fn write_node_shallow(_: Obj, _: Obj, _: i32) -> HResult {
    not_implemented(b"IXmlWriter::WriteNodeShallow")
}

/// The strings a method takes, as the arguments the writer works on.
macro_rules! strings {
    ($($name:ident),+) => {
        $(let $name = match arg($name) { Ok(s) => s, Err(hr) => return hr };)+
    };
}

extern "system" fn write_attribute_string(
    this: Obj,
    prefix: *const u16,
    local: *const u16,
    uri: *const u16,
    value: *const u16,
) -> HResult {
    strings!(prefix, local, uri, value);
    obj(this).attribute(prefix.as_deref(), local.as_deref(), uri.as_deref(), value.as_deref())
}

extern "system" fn write_cdata(this: Obj, value: *const u16) -> HResult {
    strings!(value);
    obj(this).cdata(value.as_deref())
}

extern "system" fn write_char_entity(this: Obj, ch: u16) -> HResult {
    obj(this).char_entity(ch)
}

extern "system" fn write_chars(this: Obj, chars: *const u16, count: u32) -> HResult {
    let value = match arg_n(chars, count) {
        Ok(s) => s,
        Err(hr) => return hr,
    };
    obj(this).string(value.as_deref())
}

extern "system" fn write_comment(this: Obj, value: *const u16) -> HResult {
    strings!(value);
    obj(this).comment(value.as_deref())
}

extern "system" fn write_doc_type(
    this: Obj,
    name: *const u16,
    public: *const u16,
    system: *const u16,
    subset: *const u16,
) -> HResult {
    strings!(name, public, system, subset);
    obj(this).doc_type(name.as_deref(), public.as_deref(), system.as_deref(), subset.as_deref())
}

extern "system" fn write_element_string(
    this: Obj,
    prefix: *const u16,
    local: *const u16,
    uri: *const u16,
    value: *const u16,
) -> HResult {
    strings!(prefix, local, uri, value);
    obj(this).element_string(prefix.as_deref(), local.as_deref(), uri.as_deref(), value.as_deref())
}

extern "system" fn write_end_document(this: Obj) -> HResult {
    obj(this).end_document()
}

extern "system" fn write_end_element(this: Obj) -> HResult {
    obj(this).end_element(false)
}

extern "system" fn write_entity_ref(this: Obj, name: *const u16) -> HResult {
    strings!(name);
    obj(this).entity_ref(name.as_deref())
}

extern "system" fn write_full_end_element(this: Obj) -> HResult {
    obj(this).end_element(true)
}

extern "system" fn write_name(this: Obj, value: *const u16) -> HResult {
    strings!(value);
    obj(this).name(value.as_deref(), false)
}

extern "system" fn write_nmtoken(this: Obj, value: *const u16) -> HResult {
    strings!(value);
    obj(this).name(value.as_deref(), true)
}

extern "system" fn write_pi(this: Obj, name: *const u16, value: *const u16) -> HResult {
    strings!(name, value);
    obj(this).pi(name.as_deref(), value.as_deref())
}

extern "system" fn write_qualified_name(this: Obj, local: *const u16, uri: *const u16) -> HResult {
    strings!(local, uri);
    obj(this).qualified_name(local.as_deref(), uri.as_deref())
}

extern "system" fn write_raw(this: Obj, value: *const u16) -> HResult {
    strings!(value);
    obj(this).raw(value.as_deref())
}

extern "system" fn write_raw_chars(this: Obj, chars: *const u16, count: u32) -> HResult {
    let value = match arg_n(chars, count) {
        Ok(s) => s,
        Err(hr) => return hr,
    };
    obj(this).raw(value.as_deref())
}

extern "system" fn write_start_document(this: Obj, standalone: u32) -> HResult {
    obj(this).start_document(standalone)
}

extern "system" fn write_start_element(
    this: Obj,
    prefix: *const u16,
    local: *const u16,
    uri: *const u16,
) -> HResult {
    strings!(prefix, local, uri);
    obj(this).start_element(prefix.as_deref(), local.as_deref(), uri.as_deref())
}

extern "system" fn write_string(this: Obj, value: *const u16) -> HResult {
    strings!(value);
    obj(this).string(value.as_deref())
}

extern "system" fn write_surrogate_char_entity(this: Obj, low: u16, high: u16) -> HResult {
    obj(this).surrogate(low, high)
}

extern "system" fn write_whitespace(this: Obj, value: *const u16) -> HResult {
    strings!(value);
    obj(this).whitespace(value.as_deref())
}

/// Flush hands over what is written, and closes nothing: a document
/// that is not finished is flushed as it stands, a dead writer
/// included (measured).
extern "system" fn flush(this: Obj) -> HResult {
    obj(this).hand_over()
}

/// IXmlWriter's vtable, in xmllite.h's order (`user/xmllite_test` calls
/// every slot that takes no reader by its number).
static WRITER_VTABLE: VTable<32> = VTable([
    query_interface as *const (),
    add_ref as *const (),
    release as *const (),
    set_output as *const (),
    get_property as *const (),
    set_property as *const (),
    write_attributes as *const (),
    write_attribute_string as *const (),
    write_cdata as *const (),
    write_char_entity as *const (),
    write_chars as *const (),
    write_comment as *const (),
    write_doc_type as *const (),
    write_element_string as *const (),
    write_end_document as *const (),
    write_end_element as *const (),
    write_entity_ref as *const (),
    write_full_end_element as *const (),
    write_name as *const (),
    write_nmtoken as *const (),
    write_node as *const (),
    write_node_shallow as *const (),
    write_pi as *const (),
    write_qualified_name as *const (),
    write_raw as *const (),
    write_raw_chars as *const (),
    write_start_document as *const (),
    write_start_element as *const (),
    write_string as *const (),
    write_surrogate_char_entity as *const (),
    write_whitespace as *const (),
    flush as *const (),
]);
