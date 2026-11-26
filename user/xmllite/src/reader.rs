//! `CreateXmlReader` and `IXmlReader`: the nodes of a document, one at
//! a time, over gala's XML scanner ([`gala_xml::Scan`]).
//!
//! What a reader hands its caller is the scan's events with a node type,
//! a depth and a position each. Everything here is measured on Windows
//! in `user/xmllite_test`, which reads the same document and the same
//! malformed ones against this:
//!
//! - an empty element is ONE node, with IsEmptyElement set, where the
//!   scan hands out a start and an end -- so that end is read and
//!   dropped;
//! - a start tag's depth is the element's, an end tag's is the depth of
//!   the content it closes (one more), and every other node's is the
//!   depth it sits at;
//! - the position is the column of the node's first character, counted
//!   from 1 and in characters, past the markup that opens it: the name
//!   of an element, an end tag, a processing instruction or the
//!   declaration, and the first character of a comment's or a CDATA
//!   section's content;
//! - a run of character data that is all white space is Whitespace
//!   wherever it stands, inside the root element as well as outside it;
//! - an element's namespace declarations are among its attributes, in
//!   the namespace `http://www.w3.org/2000/xmlns/`, and the XML
//!   declaration's `version` and `encoding` are attributes of it;
//! - a document that is not well formed fails the read that meets it
//!   with the code XmlLite answers for what the reader was looking for
//!   ([`error_code`]) and leaves the reader in the Error read state,
//!   which every later read answers again.
//!
//! The document is read whole from the input stream at SetInput and
//! decoded there (XmlLite reads it in chunks as its caller reads nodes;
//! gala's scanner works on the whole text). What this reader asks of a
//! stream is what Windows' reader asks, measured: QueryInterface for
//! IStream and ISequentialStream::Read until it is given nothing -- no
//! AddRef, no Release, no Seek -- so a program may hand it a stream that
//! can do no more.
//!
//! Limits, each recorded in the ledger when a program meets it:
//! ReadValueChunk and GetBaseUri are not implemented, a DTD is not
//! parsed (DtdProcessing stays Prohibit, which is XmlLite's default),
//! and the attribute getters answer with the position of the element's
//! own node, since the scan does not keep one per attribute.

use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, Ordering};

use gala_xml::{Attr, Event, Reason, Scan};

use crate::{
    E_INVALIDARG, E_NOINTERFACE, E_NOTIMPL, E_OUTOFMEMORY, E_POINTER, Guid, HResult,
    IID_ISEQUENTIAL_STREAM, IID_ISTREAM, IID_IUNKNOWN, LEDGER_NOT_IMPLEMENTED, LEDGER_OTHER,
    LEDGER_UNSUPPORTED, Obj, S_FALSE, S_OK, data1, free_box, guid, not_implemented, push, put,
    record, record_head, text, try_box,
};

/// `IXmlReader` (xmllite.idl).
const IID_IXML_READER: Guid =
    guid(0x7279_fc81, 0x709d, 0x4095, [0xb6, 0x3d, 0x69, 0xfe, 0x4b, 0x0d, 0x90, 0x30]);

// XmlNodeType.
const NODE_NONE: u32 = 0;
const NODE_ELEMENT: u32 = 1;
const NODE_ATTRIBUTE: u32 = 2;
const NODE_TEXT: u32 = 3;
const NODE_CDATA: u32 = 4;
const NODE_PI: u32 = 7;
const NODE_COMMENT: u32 = 8;
const NODE_WHITESPACE: u32 = 13;
const NODE_END_ELEMENT: u32 = 15;
const NODE_XML_DECLARATION: u32 = 17;

// XmlReadState.
const STATE_INITIAL: isize = 0;
const STATE_INTERACTIVE: isize = 1;
const STATE_ERROR: isize = 2;
const STATE_END_OF_FILE: isize = 3;
/// A reader with no input: what one is before SetInput, and what
/// SetInput(NULL) leaves it (measured).
const STATE_CLOSED: isize = 4;

// XmlReaderProperty.
const PROP_CONFORMANCE_LEVEL: u32 = 1;
const PROP_DTD_PROCESSING: u32 = 4;
const PROP_READ_STATE: u32 = 5;

// XmlConformanceLevel, XmlReaderDtdProcessing.
const LEVEL_FRAGMENT: isize = 1;
const LEVEL_DOCUMENT: isize = 2;
const DTD_PROHIBIT: isize = 0;

// The codes XmlLite answers a read with, named as the SDK's XmlError
// enum names them. `user/xmllite_test` measures each on the document
// that provokes it; the two that are not measured say so in
// `error_code`.
const MX_E_INPUTEND: HResult = 0xc00c_ee01u32 as HResult;
const MX_E_ENCODING: HResult = 0xc00c_ee02u32 as HResult;
const WC_E_WHITESPACE: HResult = 0xc00c_ee21u32 as HResult;
const WC_E_SEMICOLON: HResult = 0xc00c_ee22u32 as HResult;
const WC_E_QUOTE: HResult = 0xc00c_ee24u32 as HResult;
const WC_E_EQUAL: HResult = 0xc00c_ee25u32 as HResult;
const WC_E_LESSTHAN: HResult = 0xc00c_ee26u32 as HResult;
const WC_E_SYNTAX: HResult = 0xc00c_ee2du32 as HResult;
const WC_E_CDSECT: HResult = 0xc00c_ee2eu32 as HResult;
const WC_E_COMMENT: HResult = 0xc00c_ee2fu32 as HResult;
const WC_E_ROOTELEMENT: HResult = 0xc00c_ee3au32 as HResult;
const WC_E_ELEMENTMATCH: HResult = 0xc00c_ee3bu32 as HResult;
const WC_E_UNIQUEATTRIBUTE: HResult = 0xc00c_ee3cu32 as HResult;
const WC_E_TEXTXMLDECL: HResult = 0xc00c_ee3du32 as HResult;
const WC_E_UNDECLAREDENTITY: HResult = 0xc00c_ee47u32 as HResult;
const WC_E_PI: HResult = 0xc00c_ee4au32 as HResult;
const WC_E_DTDPROHIBITED: HResult = 0xc00c_ee4fu32 as HResult;
const NC_E_QNAMECHARACTER: HResult = 0xc00c_ee61u32 as HResult;
const NC_E_UNDECLAREDPREFIX: HResult = 0xc00c_ee65u32 as HResult;
const NC_E_EMPTYURI: HResult = 0xc00c_ee66u32 as HResult;
const SC_E_MAXELEMENTDEPTH: HResult = 0xc00c_ee81u32 as HResult;

/// What the reader was looking for where it stopped, as the code
/// XmlLite answers that read with.
///
/// Every row but two is measured on Windows in `user/xmllite_test`,
/// which refuses the same documents: the three are one table in three
/// places (the reasons are pinned in `gala_xml`'s own tests), so a row
/// added to one belongs in the others. The two that are not measured
/// are gala's own: nesting past the scanner's limit, which XmlLite
/// answers for its own MaxElementDepth, and bytes that are not the
/// encoding they are taken for, which it meets before any node.
fn error_code(reason: Reason) -> HResult {
    match reason {
        Reason::Syntax | Reason::NoRoot => WC_E_SYNTAX,
        Reason::InputEnd => MX_E_INPUTEND,
        Reason::Whitespace => WC_E_WHITESPACE,
        Reason::Semicolon => WC_E_SEMICOLON,
        Reason::Equal => WC_E_EQUAL,
        Reason::Quote => WC_E_QUOTE,
        Reason::LessThan => WC_E_LESSTHAN,
        // A name, or an end tag where no element is open: both are a
        // character that cannot be part of a qualified name there.
        Reason::Name | Reason::NoStartTag => NC_E_QNAMECHARACTER,
        Reason::Comment => WC_E_COMMENT,
        Reason::CdSect => WC_E_CDSECT,
        Reason::Pi => WC_E_PI,
        Reason::RootElement => WC_E_ROOTELEMENT,
        Reason::ElementMatch => WC_E_ELEMENTMATCH,
        Reason::UniqueAttribute => WC_E_UNIQUEATTRIBUTE,
        Reason::XmlDecl => WC_E_TEXTXMLDECL,
        Reason::UndeclaredEntity => WC_E_UNDECLAREDENTITY,
        Reason::Dtd => WC_E_DTDPROHIBITED,
        Reason::UndeclaredPrefix => NC_E_UNDECLAREDPREFIX,
        Reason::EmptyUri => NC_E_EMPTYURI,
        Reason::MaxDepth => SC_E_MAXELEMENTDEPTH,
        Reason::Encoding => MX_E_ENCODING,
    }
}

/// An empty string for a getter whose node has none: a caller reads to
/// the NUL, and never gets NULL.
static EMPTY: [u16; 1] = [0];

/// A string a getter hands out: NUL-terminated UTF-16, which lives as
/// long as the node it belongs to.
#[derive(Default)]
struct Wide(Vec<u16>);

impl Wide {
    /// `s` as the string a getter hands out. False when out of memory,
    /// which leaves the string it held.
    fn set(&mut self, s: &str) -> bool {
        let mut units = Vec::new();
        // At most one unit per byte of UTF-8, plus the NUL.
        if units.try_reserve(s.len() + 1).is_err() {
            return false;
        }
        units.extend(s.encode_utf16());
        units.push(0);
        self.0 = units;
        true
    }

    fn ptr(&self) -> *const u16 {
        if self.0.is_empty() { EMPTY.as_ptr() } else { self.0.as_ptr() }
    }

    /// The length a getter reports: the units before the NUL.
    fn len(&self) -> u32 {
        self.0.len().saturating_sub(1) as u32
    }
}

/// The five strings a node's getters hand out, all valid at once (a
/// caller may hold every one of them before it reads the next node).
#[derive(Default)]
struct Names {
    qname: Wide,
    prefix: Wide,
    local: Wide,
    ns: Wide,
    value: Wide,
}

impl Names {
    /// The prefix is the qualified name's own, so a caller gets the five
    /// from four. False when out of memory.
    fn set(&mut self, qname: &str, local: &str, ns: &str, value: &str) -> bool {
        self.qname.set(qname)
            && self.prefix.set(prefix_of(qname))
            && self.local.set(local)
            && self.ns.set(ns)
            && self.value.set(value)
    }
}

/// The prefix of a qualified name, or "" when it has none.
fn prefix_of(qname: &str) -> &str {
    match qname.split_once(':') {
        Some((p, _)) => p,
        None => "",
    }
}

/// The node the reader is on.
struct Node {
    ty: u32,
    depth: u32,
    line: u32,
    pos: u32,
    empty: bool,
    names: Names,
    /// An element's attributes, or the XML declaration's
    /// pseudo-attributes, in the order they are written.
    attrs: Vec<Attr>,
}

impl Default for Node {
    fn default() -> Self {
        Node {
            ty: NODE_NONE,
            depth: 0,
            line: 0,
            pos: 0,
            empty: false,
            names: Names::default(),
            attrs: Vec::new(),
        }
    }
}

/// The reader object: its vtable, its references and where it has got
/// to in the document.
#[repr(C)]
struct Reader {
    vtable: *const (),
    refs: AtomicU32,
    /// The document, read whole from the input stream and decoded.
    text: String,
    scan: Scan,
    /// XmlReadState.
    state: isize,
    /// The code the read that failed answered, which every later read
    /// answers again until the input is set anew.
    failed: HResult,
    node: Node,
    /// The attribute the reader has moved to, an index into
    /// `node.attrs`.
    attr: Option<usize>,
    /// The strings that attribute's getters hand out.
    attr_names: Names,
    /// The elements the scan is inside, outermost first, as (namespace,
    /// qualified name, local name): an end tag's event carries no names
    /// and a reader answers for them.
    open: Vec<(String, String, String)>,
    /// The scan's end of an empty element, still to be read and dropped.
    drop_end: bool,
    /// ConformanceLevel Fragment: more than one root element is allowed.
    fragment: bool,
}

impl Reader {
    fn new() -> Self {
        Reader {
            vtable: &READER_VTABLE as *const VTable<26> as *const (),
            refs: AtomicU32::new(1),
            text: String::new(),
            scan: Scan::new(),
            state: STATE_CLOSED,
            failed: S_OK,
            node: Node::default(),
            attr: None,
            attr_names: Names::default(),
            open: Vec::new(),
            drop_end: false,
            fragment: false,
        }
    }

    /// Back to a reader with no input, which SetInput does before it
    /// takes one: everything a document left behind is dropped.
    fn reset(&mut self) {
        self.text = String::new();
        self.scan = Scan::new();
        self.state = STATE_CLOSED;
        self.failed = S_OK;
        self.node = Node::default();
        self.attr = None;
        self.attr_names = Names::default();
        self.open = Vec::new();
        self.drop_end = false;
    }

    /// The next node of the document, or the error the scan refuses it
    /// with.
    fn step(&mut self) -> HResult {
        loop {
            // Read before the event, this is where the node the event
            // describes starts: every character of the document belongs
            // to one node.
            let (line, pos) = self.scan.line_position(&self.text);
            let event = match self.scan.next_event(&self.text) {
                Ok(event) => event,
                Err(e) => {
                    let hr = error_code(e.reason);
                    // What the document was refused for: a program that
                    // stops on a document gala's reader will not read
                    // leaves this line behind.
                    record_head("IXmlReader::Read", &e.msg, LEDGER_OTHER, u64::from(e.line));
                    // Where the document was refused, as `gala_xml`
                    // counts it, so a caller that asks after a failed
                    // read is told the place and not nothing.
                    self.node = Node { line: e.line, pos: e.column, ..Node::default() };
                    self.attr = None;
                    self.state = STATE_ERROR;
                    self.failed = hr;
                    return hr;
                }
            };
            if self.drop_end && matches!(event, Event::EndElement) {
                // The second half of an empty element: one node to a
                // reader, which has had its start tag already.
                self.drop_end = false;
                self.open.pop();
                continue;
            }
            return self.on(event, line, pos);
        }
    }

    /// The node an event describes, as the reader answers for it.
    fn on(&mut self, event: Event, line: u32, pos: u32) -> HResult {
        self.attr = None;
        self.node = Node { line, ..Node::default() };
        let depth = self.scan.depth() as u32;
        let ok = match event {
            Event::StartElement { ns, name, qname, attrs, .. } => {
                self.node.ty = NODE_ELEMENT;
                // The scan has the element on its stack; the start tag's
                // own depth is one less.
                self.node.depth = depth.saturating_sub(1);
                // Past the '<', at the name.
                self.node.pos = pos + 1;
                self.node.empty = self.scan.empty_element();
                self.drop_end = self.node.empty;
                self.node.attrs = attrs;
                let named = self.node.names.set(&qname, &name, &ns, "");
                named && push(&mut self.open, (ns, qname, name))
            }
            Event::EndElement => {
                self.node.ty = NODE_END_ELEMENT;
                // The depth of the content it closes, not of the
                // element: one more than its start tag's.
                self.node.depth = depth + 1;
                // Past the '</', at the name.
                self.node.pos = pos + 2;
                let (ns, qname, name) = self.open.pop().unwrap_or_default();
                self.node.names.set(&qname, &name, &ns, "")
            }
            Event::Text(value) => {
                // A run that is all white space is its own node type,
                // inside the root element as well as outside it.
                self.node.ty = if value.trim().is_empty() { NODE_WHITESPACE } else { NODE_TEXT };
                self.node.depth = depth;
                self.node.pos = pos;
                self.node.names.set("", "", "", &value)
            }
            Event::CData(value) => {
                self.node.ty = NODE_CDATA;
                self.node.depth = depth;
                // Past the '<![CDATA[', at the content.
                self.node.pos = pos + 9;
                self.node.names.set("", "", "", &value)
            }
            Event::Comment(value) => {
                self.node.ty = NODE_COMMENT;
                self.node.depth = depth;
                // Past the '<!--', at the content.
                self.node.pos = pos + 4;
                self.node.names.set("", "", "", &value)
            }
            Event::Pi { target, data } => {
                // The scan refuses an `xml` instruction anywhere but at
                // the start of the document, so this one is the XML
                // declaration.
                let declaration = target.eq_ignore_ascii_case("xml");
                self.node.ty = if declaration { NODE_XML_DECLARATION } else { NODE_PI };
                self.node.depth = depth;
                // Past the '<?', at the target.
                self.node.pos = pos + 2;
                if declaration {
                    self.node.attrs = decl_attrs(&data);
                    self.node.names.set(&target, &target, "", "")
                } else {
                    self.node.names.set(&target, &target, "", &data)
                }
            }
            Event::End => {
                self.state = STATE_END_OF_FILE;
                return S_FALSE;
            }
        };
        if !ok {
            self.node = Node::default();
            self.state = STATE_ERROR;
            self.failed = E_OUTOFMEMORY;
            return E_OUTOFMEMORY;
        }
        self.state = STATE_INTERACTIVE;
        S_OK
    }

    /// The strings the getters answer with: the attribute the reader has
    /// moved to, or the node itself.
    fn names(&self) -> &Names {
        if self.attr.is_some() { &self.attr_names } else { &self.node.names }
    }

    /// Moves to attribute `i`, whose strings the getters then answer
    /// with.
    fn move_to(&mut self, i: usize) -> HResult {
        let Some(a) = self.node.attrs.get(i) else {
            return S_FALSE;
        };
        let (qname, name, ns, value) = (&a.qname, &a.name, &a.ns, &a.value);
        let mut names = Names::default();
        if !names.set(qname, name, ns, value) {
            return E_OUTOFMEMORY;
        }
        self.attr_names = names;
        self.attr = Some(i);
        S_OK
    }
}

/// The XML declaration's pseudo-attributes (`version`, `encoding`,
/// `standalone`), which a reader hands out as the attributes of the
/// declaration node: its data as the scan read it, `name="value"` pairs.
///
/// A declaration gala's scanner accepted but cannot be read as pairs
/// yields none rather than failing the read: the scanner does not check
/// what a declaration says, which XmlLite does (it has codes of its own
/// for a declaration's version and encoding; none is measured).
fn decl_attrs(data: &str) -> Vec<Attr> {
    let mut out = Vec::new();
    let mut rest = data.trim();
    while !rest.is_empty() {
        let Some((name, tail)) = rest.split_once('=') else {
            break;
        };
        let tail = tail.trim_start();
        let Some(quote) = tail.chars().next() else {
            break;
        };
        if quote != '"' && quote != '\'' {
            break;
        }
        let Some((value, tail)) = tail[1..].split_once(quote) else {
            break;
        };
        let name = name.trim();
        let mut attr = Attr {
            ns: String::new(),
            name: String::new(),
            qname: String::new(),
            value: String::new(),
        };
        // An attribute whose strings do not fit in memory is left out,
        // as is everything after it.
        if attr.name.try_reserve(name.len()).is_err()
            || attr.qname.try_reserve(name.len()).is_err()
            || attr.value.try_reserve(value.len()).is_err()
        {
            break;
        }
        attr.name.push_str(name);
        // A pseudo-attribute has no prefix, so the two are the same.
        attr.qname.push_str(name);
        attr.value.push_str(value);
        if !push(&mut out, attr) {
            break;
        }
        rest = tail.trim_start();
    }
    out
}

// ---------------------------------------------------------------------
// The input stream.
// ---------------------------------------------------------------------

/// The input as a stream to read from: IStream, or ISequentialStream for
/// one that is no more than that. The reference the QueryInterface
/// gives is the caller's own object (a stream is not addrefed or
/// released, as Windows' reader does not), and the document is read
/// before SetInput returns, so the pointer is not kept.
fn stream_of(input: Obj) -> Result<Obj, HResult> {
    for iid in [&IID_ISTREAM, &IID_ISEQUENTIAL_STREAM] {
        let mut out: Obj = core::ptr::null_mut();
        // SAFETY: slot 0 of a COM object's vtable is QueryInterface, and
        // the IID and the output are ours.
        let hr = unsafe {
            let vtable = *(input as *const *const usize);
            let f: extern "system" fn(Obj, *const Guid, *mut Obj) -> HResult =
                core::mem::transmute(*vtable);
            f(input, iid, &mut out)
        };
        if hr >= 0 && !out.is_null() {
            return Ok(out);
        }
    }
    // An input that is not a stream: XmlLite also takes an
    // IXmlReaderInput (CreateXmlReaderInputWithEncodingName), which
    // gala does not make.
    record(b"IXmlReader::SetInput(not a stream)", LEDGER_UNSUPPORTED, 0);
    Err(E_NOINTERFACE)
}

/// The document's bytes, read from the stream until it gives nothing
/// (ISequentialStream::Read, slot 3 of IStream's vtable too).
fn read_all(stream: Obj) -> Result<Vec<u8>, HResult> {
    /// What one read asks for. A document shorter than this is two
    /// reads: its bytes, then none.
    const CHUNK: usize = 64 << 10;
    let mut bytes: Vec<u8> = Vec::new();
    loop {
        let had = bytes.len();
        if bytes.try_reserve(CHUNK).is_err() {
            return Err(E_OUTOFMEMORY);
        }
        bytes.resize(had + CHUNK, 0);
        let mut got: u32 = 0;
        // SAFETY: slot 3 is ISequentialStream::Read; the buffer holds
        // CHUNK bytes from `had` and the count is ours.
        let hr = unsafe {
            let vtable = *(stream as *const *const usize);
            let f: extern "system" fn(Obj, *mut u8, u32, *mut u32) -> HResult =
                core::mem::transmute(*vtable.add(3));
            f(stream, bytes.as_mut_ptr().add(had), CHUNK as u32, &mut got)
        };
        bytes.truncate(had + (got as usize).min(CHUNK));
        if hr < 0 {
            record(b"IXmlReader::SetInput(stream read)", LEDGER_OTHER, hr as u32 as u64);
            return Err(hr);
        }
        if got == 0 {
            return Ok(bytes);
        }
    }
}

// ---------------------------------------------------------------------
// The object.
// ---------------------------------------------------------------------

#[repr(C)]
struct VTable<const N: usize>([*const (); N]);
// SAFETY: function pointers only.
unsafe impl<const N: usize> Sync for VTable<N> {}

/// `this` as the reader we handed out.
fn obj<'a>(this: Obj) -> &'a mut Reader {
    // SAFETY: a live reader of ours; a reader is used by one thread at a
    // time (XmlLite's own is not thread safe either).
    unsafe { &mut *(this as *mut Reader) }
}

/// CreateXmlReader: a reader with no input, whose read state is Closed
/// until one is set (measured).
#[unsafe(no_mangle)]
pub extern "system" fn CreateXmlReader(riid: *const Guid, out: *mut Obj, malloc: Obj) -> HResult {
    if out.is_null() || riid.is_null() {
        return E_POINTER;
    }
    put(out, core::ptr::null_mut());
    // SAFETY: the caller's IID.
    let iid = unsafe { *riid };
    if iid != IID_IXML_READER && iid != IID_IUNKNOWN {
        record(b"CreateXmlReader(iid)", LEDGER_UNSUPPORTED, data1(&iid));
        return E_NOINTERFACE;
    }
    if !malloc.is_null() {
        // A caller's IMalloc: the reader allocates from gala's heap
        // instead, which the ledger says rather than leaving it to be
        // guessed at.
        record(b"CreateXmlReader(IMalloc)", LEDGER_UNSUPPORTED, 0);
    }
    match try_box(Reader::new()) {
        Some(p) => {
            put(out, p.cast());
            S_OK
        }
        None => E_OUTOFMEMORY,
    }
}

extern "system" fn query_interface(this: Obj, riid: *const Guid, out: *mut Obj) -> HResult {
    if out.is_null() || riid.is_null() {
        return E_POINTER;
    }
    put(out, core::ptr::null_mut());
    // SAFETY: the caller's IID.
    let iid = unsafe { *riid };
    if iid != IID_IXML_READER && iid != IID_IUNKNOWN {
        record(b"IXmlReader::QueryInterface(iid)", LEDGER_UNSUPPORTED, data1(&iid));
        return E_NOINTERFACE;
    }
    add_ref(this);
    put(out, this);
    S_OK
}

extern "system" fn add_ref(this: Obj) -> u32 {
    obj(this).refs.fetch_add(1, Ordering::Relaxed) + 1
}

extern "system" fn release(this: Obj) -> u32 {
    let left = obj(this).refs.fetch_sub(1, Ordering::AcqRel) - 1;
    if left == 0 {
        // SAFETY: the last reference; nothing uses the object after.
        unsafe { free_box(this as *mut Reader) };
    }
    left
}

extern "system" fn set_input(this: Obj, input: Obj) -> HResult {
    let r = obj(this);
    r.reset();
    if input.is_null() {
        // SetInput(NULL) gives up the input, leaving the reader as
        // CreateXmlReader made it.
        return S_OK;
    }
    let stream = match stream_of(input) {
        Ok(stream) => stream,
        Err(hr) => return hr,
    };
    let bytes = match read_all(stream) {
        Ok(bytes) => bytes,
        Err(hr) => return hr,
    };
    match gala_xml::decode(&bytes) {
        Ok(text) => r.text = text,
        Err(e) => {
            // Bytes that are not the encoding they are taken for: the
            // input is refused here, before any node.
            record_head("IXmlReader::SetInput", &e.msg, LEDGER_OTHER, u64::from(e.line));
            return error_code(e.reason);
        }
    }
    r.scan = if r.fragment { Scan::new().allow_fragment() } else { Scan::new() };
    r.state = STATE_INITIAL;
    S_OK
}

extern "system" fn get_property(this: Obj, property: u32, value: *mut isize) -> HResult {
    if value.is_null() {
        return E_INVALIDARG;
    }
    let r = obj(this);
    let v = match property {
        PROP_CONFORMANCE_LEVEL => {
            if r.fragment {
                LEVEL_FRAGMENT
            } else {
                LEVEL_DOCUMENT
            }
        }
        // A DTD is never parsed, which is XmlLite's own default.
        PROP_DTD_PROCESSING => DTD_PROHIBIT,
        PROP_READ_STATE => r.state,
        _ => {
            record(
                b"IXmlReader::GetProperty(property)",
                LEDGER_NOT_IMPLEMENTED,
                u64::from(property),
            );
            return E_NOTIMPL;
        }
    };
    put(value, v);
    S_OK
}

extern "system" fn set_property(this: Obj, property: u32, value: isize) -> HResult {
    let r = obj(this);
    match (property, value) {
        // The level the next SetInput reads at: a document has one root
        // element, a fragment any number.
        (PROP_CONFORMANCE_LEVEL, LEVEL_DOCUMENT) => {
            r.fragment = false;
            S_OK
        }
        (PROP_CONFORMANCE_LEVEL, LEVEL_FRAGMENT) => {
            r.fragment = true;
            S_OK
        }
        (PROP_DTD_PROCESSING, DTD_PROHIBIT) => S_OK,
        _ => {
            // The property and the value it was given, as one detail.
            let detail = (u64::from(property) << 32) | (value as u64 & 0xffff_ffff);
            record(b"IXmlReader::SetProperty(property)", LEDGER_UNSUPPORTED, detail);
            E_NOTIMPL
        }
    }
}

extern "system" fn read(this: Obj, ty: *mut u32) -> HResult {
    let r = obj(this);
    let hr = match r.state {
        // A reader with no input, and one past the end: nothing more,
        // every time it is asked.
        STATE_CLOSED | STATE_END_OF_FILE => S_FALSE,
        // A document that was refused answers the same code again.
        STATE_ERROR => r.failed,
        _ => r.step(),
    };
    put(ty, r.node.ty);
    hr
}

extern "system" fn get_node_type(this: Obj, ty: *mut u32) -> HResult {
    let r = obj(this);
    let t = if r.attr.is_some() { NODE_ATTRIBUTE } else { r.node.ty };
    put(ty, t);
    // None is not a node, which the reader says with S_FALSE (measured
    // on one that has not read anything).
    if t == NODE_NONE { S_FALSE } else { S_OK }
}

extern "system" fn move_to_first_attribute(this: Obj) -> HResult {
    obj(this).move_to(0)
}

extern "system" fn move_to_next_attribute(this: Obj) -> HResult {
    let r = obj(this);
    let next = r.attr.map_or(0, |i| i + 1);
    r.move_to(next)
}

extern "system" fn move_to_attribute_by_name(
    this: Obj,
    local: *const u16,
    ns: *const u16,
) -> HResult {
    let r = obj(this);
    let Some(local) = text(local) else {
        return E_INVALIDARG;
    };
    // No namespace asked for means the attributes in none, which is
    // where an unprefixed attribute is.
    let ns = text(ns).unwrap_or_default();
    let found = r.node.attrs.iter().position(|a| a.name == local && a.ns == ns);
    match found {
        Some(i) => r.move_to(i),
        None => S_FALSE,
    }
}

extern "system" fn move_to_element(this: Obj) -> HResult {
    let r = obj(this);
    if r.attr.take().is_none() {
        // Not on an attribute: nothing to go back from.
        return S_FALSE;
    }
    S_OK
}

/// One of the five string getters: the string of the attribute the
/// reader has moved to, or of the node itself.
macro_rules! getter {
    ($name:ident, $field:ident) => {
        extern "system" fn $name(this: Obj, out: *mut *const u16, len: *mut u32) -> HResult {
            if out.is_null() {
                return E_INVALIDARG;
            }
            let names = obj(this).names();
            put(out, names.$field.ptr());
            put(len, names.$field.len());
            S_OK
        }
    };
}

getter!(get_qualified_name, qname);
getter!(get_namespace_uri, ns);
getter!(get_local_name, local);
getter!(get_prefix, prefix);
getter!(get_value, value);

extern "system" fn read_value_chunk(
    _this: Obj,
    _buf: *mut u16,
    _len: u32,
    _read: *mut u32,
) -> HResult {
    // A value is handed out whole by GetValue; nothing gala runs reads
    // one in chunks.
    not_implemented(b"IXmlReader::ReadValueChunk")
}

extern "system" fn get_base_uri(_this: Obj, _out: *mut *const u16, _len: *mut u32) -> HResult {
    // The reader is given a stream, not a URI to resolve against.
    not_implemented(b"IXmlReader::GetBaseUri")
}

extern "system" fn is_default(_this: Obj) -> i32 {
    // An attribute a DTD gave its value, and a DTD is never parsed.
    0
}

extern "system" fn is_empty_element(this: Obj) -> i32 {
    i32::from(obj(this).node.empty)
}

extern "system" fn get_line_number(this: Obj, line: *mut u32) -> HResult {
    put(line, obj(this).node.line);
    S_OK
}

extern "system" fn get_line_position(this: Obj, position: *mut u32) -> HResult {
    put(position, obj(this).node.pos);
    S_OK
}

extern "system" fn get_attribute_count(this: Obj, count: *mut u32) -> HResult {
    put(count, obj(this).node.attrs.len() as u32);
    S_OK
}

extern "system" fn get_depth(this: Obj, depth: *mut u32) -> HResult {
    let r = obj(this);
    // An attribute is one deeper than the element it is on.
    put(depth, if r.attr.is_some() { r.node.depth + 1 } else { r.node.depth });
    S_OK
}

extern "system" fn is_eof(this: Obj) -> i32 {
    i32::from(obj(this).state == STATE_END_OF_FILE)
}

/// IXmlReader's vtable, in xmllite.h's order (`user/xmllite_test` calls
/// every slot by its number).
static READER_VTABLE: VTable<26> = VTable([
    query_interface as *const (),
    add_ref as *const (),
    release as *const (),
    set_input as *const (),
    get_property as *const (),
    set_property as *const (),
    read as *const (),
    get_node_type as *const (),
    move_to_first_attribute as *const (),
    move_to_next_attribute as *const (),
    move_to_attribute_by_name as *const (),
    move_to_element as *const (),
    get_qualified_name as *const (),
    get_namespace_uri as *const (),
    get_local_name as *const (),
    get_prefix as *const (),
    get_value as *const (),
    read_value_chunk as *const (),
    get_base_uri as *const (),
    is_default as *const (),
    is_empty_element as *const (),
    get_line_number as *const (),
    get_line_position as *const (),
    get_attribute_count as *const (),
    get_depth as *const (),
    is_eof as *const (),
]);
