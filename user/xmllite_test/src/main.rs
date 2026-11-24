//! XmlLite's reader (xmllite.dll's `CreateXmlReader` and `IXmlReader`),
//! as Windows answers it. Word delay-loads `CreateXmlReader` and ends on
//! 0xc06d007f without it (ROADMAP OF), so this is the shape gala's
//! xmllite.dll has to have.
//!
//! The input is this program's own `IStream` over a byte buffer, and
//! every method but QueryInterface, AddRef, Release and Read answers
//! E_NOTIMPL: what the reader asks of a stream is then a result of the
//! run (the `stream` line) and not a guess, which is what gala's reader
//! may call in turn. The IIDs the reader asks the stream for are on the
//! same line.
//!
//! What the program asserts, measured on real Windows (the `node` lines
//! are the whole transcript, printed whether or not the checks hold):
//! - `CreateXmlReader(IID_IXmlReader)` gives a reader with no input:
//!   its node type is None, it is not at end of file, and its ReadState
//!   is Initial; ConformanceLevel is Document and DtdProcessing
//!   Prohibit;
//! - `SetInput` of a stream, then `Read` node by node to the end: the
//!   node type, depth, line and position, whether the element is empty,
//!   how many attributes it has, and its qualified name, prefix, local
//!   name, namespace URI and value -- for an XML declaration, white
//!   space, a comment, elements in a default and in a prefixed
//!   namespace, text, CDATA, a processing instruction and end tags;
//! - past the root: `Read` is S_FALSE with node type None, IsEOF is
//!   true and ReadState is EndOfFile;
//! - the root's attributes through MoveToFirstAttribute,
//!   MoveToNextAttribute and MoveToElement, with the namespace
//!   declarations among them, and MoveToAttributeByName by local name
//!   and namespace;
//! - every way a document can be refused: the code the `Read` that
//!   meets it answers, how many Reads it takes to get there and that
//!   the reader is then in the Error read state. The codes are
//!   Windows', and so are the counts but for the one row that says why
//!   (`Bad::win`).
//!
//! Prints `xmllite_test: ok (N checks)`, or `xmllite_test: check N got
//! X (line L)` and exits with N.

#![no_std]
#![no_main]

use core::ffi::c_void;
use core::ptr::null_mut;

use ntrt as _;

const NAME: &str = "xmllite_test";

/// Every check records its mismatch and goes on, so one run on Windows
/// reports every answer that differs from the table below. The committed
/// program stops at the first (the transcript is printed either way).
const MEASURE: bool = false;

type Handle = usize;
type HResult = i32;
type Guid = [u8; 16];
type Obj = *mut c_void;

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, overlapped: usize) -> i32;
    fn ExitProcess(code: u32) -> !;
}

#[link(name = "xmllite", kind = "raw-dylib")]
unsafe extern "system" {
    fn CreateXmlReader(iid: *const Guid, out: *mut Obj, malloc: Obj) -> HResult;
}

const S_OK: HResult = 0;
const S_FALSE: HResult = 1;
const E_NOTIMPL: HResult = 0x8000_4001u32 as HResult;
const E_NOINTERFACE: HResult = 0x8000_4002u32 as HResult;

/// `IXmlReader` (xmllite.idl, uuid 7279FC81-709D-4095-B63D-69FE4B0D9030).
const IID_IXML_READER: Guid = [
    0x81, 0xfc, 0x79, 0x72, 0x9d, 0x70, 0x95, 0x40, 0xb6, 0x3d, 0x69, 0xfe, 0x4b, 0x0d, 0x90, 0x30,
];
/// `IUnknown` (00000000-0000-0000-C000-000000000046).
const IID_IUNKNOWN: Guid = [0, 0, 0, 0, 0, 0, 0, 0, 0xc0, 0, 0, 0, 0, 0, 0, 0x46];
/// `ISequentialStream` (0c733a30-2a1c-11ce-ade5-00aa0044773d).
const IID_ISEQUENTIAL_STREAM: Guid = [
    0x30, 0x3a, 0x73, 0x0c, 0x1c, 0x2a, 0xce, 0x11, 0xad, 0xe5, 0x00, 0xaa, 0x00, 0x44, 0x77, 0x3d,
];
/// `IStream` (0000000c-0000-0000-C000-000000000046).
const IID_ISTREAM: Guid = [0x0c, 0, 0, 0, 0, 0, 0, 0, 0xc0, 0, 0, 0, 0, 0, 0, 0x46];

// IXmlReader's vtable, in the order of xmllite.idl.
const SET_INPUT: usize = 3;
const GET_PROPERTY: usize = 4;
const SET_PROPERTY: usize = 5;
const READ: usize = 6;
const GET_NODE_TYPE: usize = 7;
const MOVE_TO_FIRST_ATTRIBUTE: usize = 8;
const MOVE_TO_NEXT_ATTRIBUTE: usize = 9;
const MOVE_TO_ATTRIBUTE_BY_NAME: usize = 10;
const MOVE_TO_ELEMENT: usize = 11;
const GET_QUALIFIED_NAME: usize = 12;
const GET_NAMESPACE_URI: usize = 13;
const GET_LOCAL_NAME: usize = 14;
const GET_PREFIX: usize = 15;
const GET_VALUE: usize = 16;
const IS_DEFAULT: usize = 19;
const IS_EMPTY_ELEMENT: usize = 20;
const GET_LINE_NUMBER: usize = 21;
const GET_LINE_POSITION: usize = 22;
const GET_ATTRIBUTE_COUNT: usize = 23;
const GET_DEPTH: usize = 24;
const IS_EOF: usize = 25;
const RELEASE: usize = 2;

// XmlNodeType.
const NODE_NONE: u32 = 0;
const NODE_ELEMENT: u32 = 1;
const NODE_TEXT: u32 = 3;
const NODE_CDATA: u32 = 4;
const NODE_PI: u32 = 7;
const NODE_COMMENT: u32 = 8;
const NODE_WHITESPACE: u32 = 13;
const NODE_END_ELEMENT: u32 = 15;
const NODE_XML_DECLARATION: u32 = 17;

// XmlReaderProperty.
const PROP_CONFORMANCE_LEVEL: u32 = 1;
const PROP_DTD_PROCESSING: u32 = 4;
const PROP_READ_STATE: u32 = 5;

// XmlReadState.
const READ_STATE_INITIAL: isize = 0;
const READ_STATE_INTERACTIVE: isize = 1;
const READ_STATE_ERROR: isize = 2;
const READ_STATE_END_OF_FILE: isize = 3;
const READ_STATE_CLOSED: isize = 4;

// The errors XmlLite answers a Read with, named as the XmlError enum
// of the Windows SDK's xmllite.h names them (where it gives a base and
// counts up from it). Each is measured below on a document that
// provokes it.
const MX_E_INPUTEND: HResult = 0xc00c_ee01u32 as HResult;
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

/// A document that is not well formed, the error the Read that meets it
/// answers, and how many Reads it takes to get there (the failing one
/// counted). A reader that has failed stays in the Error read state, and
/// setting its input again starts it over.
struct Bad {
    doc: &'static [u8],
    hr: HResult,
    /// The Reads gala's reader takes to reach the error.
    reads: u32,
    /// The Reads Windows' reader takes, the same in every row but the
    /// one that says why: there Windows hands out a node made of text
    /// the document ends in the middle of, where gala's reader refuses
    /// it. Both answer the same error, one Read apart.
    win: u32,
    /// What is wrong with it, for the line this prints.
    what: &'static str,
}

const fn b(doc: &'static [u8], hr: HResult, reads: u32, what: &'static str) -> Bad {
    Bad { doc, hr, reads, win: reads, what }
}

/// As [`b`], where the two readers take a different number of Reads.
const fn bw(doc: &'static [u8], hr: HResult, reads: u32, win: u32, what: &'static str) -> Bad {
    Bad { doc, hr, reads, win, what }
}

/// The documents, each refused for a different reason, so that the code
/// gala's own reader must answer with is Windows' and not a guess. The
/// conformance level is Document and DTDs are prohibited, both the
/// defaults.
const BADS: &[Bad] = &[
    b(b"<a><b></a>", WC_E_ELEMENTMATCH, 3, "an end tag that does not close the open element"),
    b(b"</a>", NC_E_QNAMECHARACTER, 1, "an end tag with no start tag"),
    b(b"<a/><b/>", WC_E_ROOTELEMENT, 2, "a second root element"),
    b(b"text<a/>", WC_E_SYNTAX, 1, "text before the root element"),
    b(b"<a/>text", WC_E_SYNTAX, 2, "text after the root element"),
    b(b"<!DOCTYPE a><a/>", WC_E_DTDPROHIBITED, 1, "a document type declaration"),
    b(b"<a>", MX_E_INPUTEND, 2, "the text ends inside an element"),
    b(b"<a", NC_E_QNAMECHARACTER, 1, "the text ends inside a start tag"),
    b(b"<a b='1'c='2'/>", WC_E_WHITESPACE, 1, "no white space between two attributes"),
    b(b"<a>&e;</a>", WC_E_UNDECLAREDENTITY, 2, "an entity no DTD declares"),
    b(b"<a b='<'/>", WC_E_LESSTHAN, 1, "'<' in an attribute value"),
    b(b"<a x='1' x='2'/>", WC_E_UNIQUEATTRIBUTE, 1, "an attribute given twice"),
    b(b"<p:a/>", NC_E_UNDECLAREDPREFIX, 1, "a prefix nothing declares"),
    b(b"<a><!FOO></a>", WC_E_SYNTAX, 2, "a markup declaration in content"),
    b(b"<a b=1/>", WC_E_QUOTE, 1, "an attribute value that is not quoted"),
    b(
        b"<?xml version=\"1.0\"?><?xml version=\"1.0\"?>",
        WC_E_TEXTXMLDECL,
        2,
        "a second declaration",
    ),
    b(b"<a b/>", WC_E_EQUAL, 1, "no '=' after an attribute's name"),
    b(b"<a>&e</a>", WC_E_SEMICOLON, 2, "a reference with no ';'"),
    b(b"<a>&#xZZ;</a>", WC_E_SEMICOLON, 2, "a character reference that is not a number"),
    b(b"<!--x", WC_E_COMMENT, 1, "a comment that is not closed"),
    // Windows hands out the text of a section the document ends inside
    // as a CDATA node (measured: type 4) and answers the error at the
    // Read after it; gala's reader refuses a section it finds no end of,
    // so its error comes one Read earlier. Both answer WC_E_CDSECT and
    // both end in the Error read state.
    bw(b"<a><![CDATA[x</a>", WC_E_CDSECT, 2, 3, "a CDATA section that is not closed"),
    b(b"<?pi x", WC_E_PI, 1, "a processing instruction that is not closed"),
    b(b"<a xmlns:p=\"\"/>", NC_E_EMPTYURI, 1, "a prefix declared as the empty URI"),
    b(b"<?pi,x?><a/>", WC_E_PI, 1, "no white space after a processing instruction's target"),
];

/// Calls slot `slot` of `obj`'s vtable as `$ty`.
macro_rules! call {
    ($obj:expr, $slot:expr, fn($($t:ty),*) $(-> $r:ty)?, $($arg:expr),*) => {{
        let obj: Obj = $obj;
        // SAFETY: a live COM object whose vtable has this slot with this
        // signature (xmllite.idl).
        unsafe {
            let vtable = *(obj as *const *const usize);
            let f: extern "system" fn(Obj $(, $t)*) $(-> $r)? =
                core::mem::transmute(*vtable.add($slot));
            f(obj $(, $arg)*)
        }
    }};
}

struct Line {
    buf: [u8; 300],
    n: usize,
}

impl Line {
    fn new() -> Self {
        Self { buf: [0; 300], n: 0 }
    }

    fn s(mut self, s: &[u8]) -> Self {
        for &b in s {
            if self.n < self.buf.len() {
                self.buf[self.n] = b;
                self.n += 1;
            }
        }
        self
    }

    fn dec(self, v: u64) -> Self {
        let mut d = [0u8; 20];
        let mut i = d.len();
        let mut v = v;
        loop {
            i -= 1;
            d[i] = b'0' + (v % 10) as u8;
            v /= 10;
            if v == 0 {
                break;
            }
        }
        self.s(&d[i..])
    }

    fn hex(self, v: u64) -> Self {
        let mut d = [0u8; 10];
        d[0] = b'0';
        d[1] = b'x';
        for i in 0..8 {
            d[2 + i] = b"0123456789abcdef"[((v >> (28 - 4 * i)) & 0xf) as usize];
        }
        self.s(&d)
    }

    /// A NUL-terminated wide string, a null pointer as `<null>`: what the
    /// reader answered, to read in the transcript. White space is escaped
    /// (a node's value holds it, and a line end would end the line) and
    /// anything else that is not printable ASCII is `?`.
    fn ws(mut self, p: *const u16) -> Self {
        if p.is_null() {
            return self.s(b"<null>");
        }
        let mut i = 0;
        loop {
            // SAFETY: a NUL-terminated string from the reader, read up to
            // its NUL.
            let u = unsafe { *p.add(i) };
            if u == 0 {
                break;
            }
            self = match u {
                0x0a => self.s(b"\\n"),
                0x0d => self.s(b"\\r"),
                0x09 => self.s(b"\\t"),
                0x20..=0x7e => self.s(&[u as u8]),
                _ => self.s(b"?"),
            };
            i += 1;
        }
        self
    }

    /// A GUID as its 16 bytes: an IID the reader asked for, which is
    /// data for the record and not matched against a name here.
    fn guid(mut self, g: &Guid) -> Self {
        for (i, b) in g.iter().enumerate() {
            if i > 0 {
                self = self.s(b"-");
            }
            self = self.s(&[
                b"0123456789abcdef"[usize::from(b >> 4)],
                b"0123456789abcdef"[usize::from(b & 0xf)],
            ]);
        }
        self
    }

    fn print(self) {
        let mut written = 0;
        // SAFETY: our buffer; the standard output handle.
        unsafe {
            WriteFile(
                GetStdHandle(-11i32 as u32),
                self.buf.as_ptr(),
                self.n as u32,
                &mut written,
                0,
            )
        };
    }
}

/// The check counter and the first check that was wrong (0 when none).
struct Checks(u32, u32);

impl Checks {
    #[track_caller]
    fn ok(&mut self, cond: bool, got: u64) -> Result<(), u32> {
        self.0 += 1;
        if cond {
            return Ok(());
        }
        Line::new()
            .s(NAME.as_bytes())
            .s(b": check ")
            .dec(u64::from(self.0))
            .s(b" got ")
            .hex(got)
            .s(b" (line ")
            .dec(u64::from(core::panic::Location::caller().line()))
            .s(if MEASURE { b", continuing)\r\n" } else { b")\r\n" })
            .print();
        if self.1 == 0 {
            self.1 = self.0;
        }
        if MEASURE { Ok(()) } else { Err(self.0) }
    }

    #[track_caller]
    fn eq(&mut self, got: u64, want: u64) -> Result<(), u32> {
        self.ok(got == want, got)
    }

    #[track_caller]
    fn hr(&mut self, got: HResult, want: HResult) -> Result<(), u32> {
        self.ok(got == want, got as u32 as u64)
    }

    /// A string the reader answered, with the length it answered beside
    /// it: both are checked, since a caller may read either.
    #[track_caller]
    fn text(&mut self, got: *const u16, len: u32, want: &str) -> Result<(), u32> {
        let same = wstr_is(got, want) && len as usize == want.chars().count();
        self.ok(same, u64::from(len))
    }
}

/// Whether a NUL-terminated wide string holds `want` (ASCII); a null
/// pointer counts as empty, which is how the reader may answer for a
/// name or namespace a node has not got.
fn wstr_is(p: *const u16, want: &str) -> bool {
    if p.is_null() {
        return want.is_empty();
    }
    let mut i = 0;
    for c in want.chars() {
        // SAFETY: a NUL-terminated string, read up to its NUL.
        if unsafe { *p.add(i) } != c as u16 {
            return false;
        }
        i += 1;
    }
    // SAFETY: as above.
    unsafe { *p.add(i) == 0 }
}

/// A fixed wide buffer for a name passed to the reader.
struct Wide([u16; 64]);

impl Wide {
    fn new(s: &str) -> Self {
        let mut a = [0u16; 64];
        for (slot, u) in a.iter_mut().zip(s.encode_utf16()) {
            *slot = u;
        }
        Self(a)
    }

    fn ptr(&self) -> *const u16 {
        self.0.as_ptr()
    }
}

// ---------------------------------------------------------------------
// The input: our own IStream over a byte buffer.
// ---------------------------------------------------------------------

/// Which of `IStream`'s methods the reader called, one bit per vtable
/// slot, and which IIDs it asked for.
mod used {
    pub const UNKNOWN_IID: u32 = 1 << 16;
    pub const SEQUENTIAL_IID: u32 = 1 << 17;
    pub const STREAM_IID: u32 = 1 << 18;
}

#[repr(C)]
struct StreamVtable {
    query: extern "system" fn(Obj, *const Guid, *mut Obj) -> HResult,
    add_ref: extern "system" fn(Obj) -> u32,
    release: extern "system" fn(Obj) -> u32,
    read: extern "system" fn(Obj, *mut u8, u32, *mut u32) -> HResult,
    write: extern "system" fn(Obj, *const u8, u32, *mut u32) -> HResult,
    seek: extern "system" fn(Obj, i64, u32, *mut u64) -> HResult,
    set_size: extern "system" fn(Obj, u64) -> HResult,
    copy_to: extern "system" fn(Obj, Obj, u64, *mut u64, *mut u64) -> HResult,
    commit: extern "system" fn(Obj, u32) -> HResult,
    revert: extern "system" fn(Obj) -> HResult,
    lock: extern "system" fn(Obj, u64, u64, u32) -> HResult,
    unlock: extern "system" fn(Obj, u64, u64, u32) -> HResult,
    stat: extern "system" fn(Obj, *mut u8, u32) -> HResult,
    clone: extern "system" fn(Obj, *mut Obj) -> HResult,
}

/// A read-only stream over `bytes`, which records what was asked of it.
#[repr(C)]
struct Stream {
    vtable: *const StreamVtable,
    bytes: &'static [u8],
    pos: usize,
    refs: u32,
    used: u32,
    reads: u32,
    /// The first IID the reader asked for that is not IUnknown,
    /// ISequentialStream or IStream, for the record.
    unknown: Guid,
}

/// `this` as our stream: it records the slot and is the only object
/// whose vtable is ours.
///
/// # Safety
/// `this` is a pointer to a live [`Stream`] (we handed it to the reader).
unsafe fn mark(this: Obj, slot: u32) -> &'static mut Stream {
    // SAFETY: the caller guarantees a live Stream, and the program is
    // single-threaded.
    let s = unsafe { &mut *(this as *mut Stream) };
    s.used |= 1 << slot;
    s
}

extern "system" fn st_query(this: Obj, iid: *const Guid, out: *mut Obj) -> HResult {
    // SAFETY: the reader calls this on the object we handed it.
    let s = unsafe { mark(this, 0) };
    // SAFETY: an IID from the caller.
    let iid = unsafe { &*iid };
    let known = *iid == IID_IUNKNOWN
        || (*iid == IID_ISEQUENTIAL_STREAM && {
            s.used |= used::SEQUENTIAL_IID;
            true
        })
        || (*iid == IID_ISTREAM && {
            s.used |= used::STREAM_IID;
            true
        });
    if !known {
        if s.used & used::UNKNOWN_IID == 0 {
            s.unknown = *iid;
        }
        s.used |= used::UNKNOWN_IID;
        // SAFETY: an out pointer from the caller.
        unsafe { *out = null_mut() };
        return E_NOINTERFACE;
    }
    s.refs += 1;
    // SAFETY: as above.
    unsafe { *out = this };
    S_OK
}

extern "system" fn st_add_ref(this: Obj) -> u32 {
    // SAFETY: see `st_query`.
    let s = unsafe { mark(this, 1) };
    s.refs += 1;
    s.refs
}

extern "system" fn st_release(this: Obj) -> u32 {
    // SAFETY: see `st_query`. The object is on `run`'s stack and outlives
    // the reader, so the last release frees nothing.
    let s = unsafe { mark(this, 2) };
    s.refs = s.refs.saturating_sub(1);
    s.refs
}

extern "system" fn st_read(this: Obj, buf: *mut u8, len: u32, read: *mut u32) -> HResult {
    // SAFETY: see `st_query`.
    let s = unsafe { mark(this, 3) };
    s.reads += 1;
    let n = (s.bytes.len() - s.pos).min(len as usize);
    for i in 0..n {
        // SAFETY: a buffer of at least `len` bytes from the caller.
        unsafe { *buf.add(i) = s.bytes[s.pos + i] };
    }
    s.pos += n;
    if !read.is_null() {
        // SAFETY: an out pointer from the caller.
        unsafe { *read = n as u32 };
    }
    S_OK
}

/// Every other method: E_NOTIMPL, so what the reader needs of a stream
/// is what the run shows it used.
macro_rules! notimpl {
    ($name:ident, $slot:expr, fn($($t:ty),*)) => {
        extern "system" fn $name(this: Obj $(, _: $t)*) -> HResult {
            // SAFETY: see `st_query`.
            unsafe { mark(this, $slot) };
            E_NOTIMPL
        }
    };
}

notimpl!(st_write, 4, fn(*const u8, u32, *mut u32));
notimpl!(st_seek, 5, fn(i64, u32, *mut u64));
notimpl!(st_set_size, 6, fn(u64));
notimpl!(st_copy_to, 7, fn(Obj, u64, *mut u64, *mut u64));
notimpl!(st_commit, 8, fn(u32));
notimpl!(st_revert, 9, fn());
notimpl!(st_lock, 10, fn(u64, u64, u32));
notimpl!(st_unlock, 11, fn(u64, u64, u32));
notimpl!(st_stat, 12, fn(*mut u8, u32));
notimpl!(st_clone, 13, fn(*mut Obj));

static STREAM_VTABLE: StreamVtable = StreamVtable {
    query: st_query,
    add_ref: st_add_ref,
    release: st_release,
    read: st_read,
    write: st_write,
    seek: st_seek,
    set_size: st_set_size,
    copy_to: st_copy_to,
    commit: st_commit,
    revert: st_revert,
    lock: st_lock,
    unlock: st_unlock,
    stat: st_stat,
    clone: st_clone,
};

impl Stream {
    fn new(bytes: &'static [u8]) -> Self {
        Self { vtable: &STREAM_VTABLE, bytes, pos: 0, refs: 1, used: 0, reads: 0, unknown: [0; 16] }
    }

    fn obj(&mut self) -> Obj {
        self as *mut Stream as Obj
    }
}

// ---------------------------------------------------------------------
// The document and the nodes it should give.
// ---------------------------------------------------------------------

/// The document, with a declaration, a comment, a default and a prefixed
/// namespace, attributes in both, text, an empty element, CDATA and a
/// processing instruction. Line ends are `\n`, and one element is
/// indented, so that a white-space node holding more than a line end is
/// read too.
///
/// It is one literal line because a `\` at the end of a Rust string
/// literal eats the line end *and* the next line's indentation, which
/// would quietly take the white space out of the document under test.
const DOC: &[u8] = b"<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<!--c-->\n<r xmlns=\"urn:d\" xmlns:a=\"urn:a\" k=\"v\" a:j=\"w\">\n  <a:p>text</a:p>\n<e/>\n<![CDATA[<raw>]]>\n<?pi data?>\n</r>\n";

/// One node of [`DOC`], as the reader answers for it.
struct Node {
    ty: u32,
    depth: u32,
    line: u32,
    pos: u32,
    empty: bool,
    attrs: u32,
    qname: &'static str,
    prefix: &'static str,
    local: &'static str,
    ns: &'static str,
    value: &'static str,
}

// One argument per column of the table below, which is the point of it.
#[allow(clippy::too_many_arguments)]
const fn n(
    ty: u32,
    depth: u32,
    line: u32,
    pos: u32,
    empty: bool,
    attrs: u32,
    qname: &'static str,
    prefix: &'static str,
    local: &'static str,
    ns: &'static str,
    value: &'static str,
) -> Node {
    Node { ty, depth, line, pos, empty, attrs, qname, prefix, local, ns, value }
}

/// [`DOC`]'s nodes in document order, as Windows answers for them.
///
/// The position is 1 at the start of a line and counts characters, and
/// what it points at depends on the node: the name of an element, an end
/// tag, a processing instruction or the declaration; the first character
/// of a comment's or a CDATA section's content, past its `<!--` or
/// `<![CDATA[`; and the start of a run of text or white space.
///
/// An end tag's depth is one more than its start tag's -- the depth of
/// the content it closes, not of the element -- and an empty element is
/// one node with no end tag of its own, where gala's own reader
/// (`gala_xml`'s [`Scan`](gala_xml::Scan)) hands out both.
const NODES: &[Node] = &[
    n(NODE_XML_DECLARATION, 0, 1, 3, false, 2, "xml", "", "xml", "", ""),
    n(NODE_WHITESPACE, 0, 1, 39, false, 0, "", "", "", "", "\n"),
    n(NODE_COMMENT, 0, 2, 5, false, 0, "", "", "", "", "c"),
    n(NODE_WHITESPACE, 0, 2, 9, false, 0, "", "", "", "", "\n"),
    n(NODE_ELEMENT, 0, 3, 2, false, 4, "r", "", "r", "urn:d", ""),
    n(NODE_WHITESPACE, 1, 3, 48, false, 0, "", "", "", "", "\n  "),
    n(NODE_ELEMENT, 1, 4, 4, false, 0, "a:p", "a", "p", "urn:a", ""),
    n(NODE_TEXT, 2, 4, 8, false, 0, "", "", "", "", "text"),
    n(NODE_END_ELEMENT, 2, 4, 14, false, 0, "a:p", "a", "p", "urn:a", ""),
    n(NODE_WHITESPACE, 1, 4, 18, false, 0, "", "", "", "", "\n"),
    n(NODE_ELEMENT, 1, 5, 2, true, 0, "e", "", "e", "urn:d", ""),
    n(NODE_WHITESPACE, 1, 5, 5, false, 0, "", "", "", "", "\n"),
    n(NODE_CDATA, 1, 6, 10, false, 0, "", "", "", "", "<raw>"),
    n(NODE_WHITESPACE, 1, 6, 18, false, 0, "", "", "", "", "\n"),
    n(NODE_PI, 1, 7, 3, false, 0, "pi", "", "pi", "", "data"),
    n(NODE_WHITESPACE, 1, 7, 12, false, 0, "", "", "", "", "\n"),
    n(NODE_END_ELEMENT, 1, 8, 3, false, 0, "r", "", "r", "urn:d", ""),
    n(NODE_WHITESPACE, 0, 8, 5, false, 0, "", "", "", "", "\n"),
];

/// The root's attributes, in the order MoveToNextAttribute gives them:
/// the namespace declarations among them, as the reader has them.
struct Attr {
    qname: &'static str,
    prefix: &'static str,
    local: &'static str,
    ns: &'static str,
    value: &'static str,
}

const ATTRS: &[Attr] = &[
    Attr {
        qname: "xmlns",
        prefix: "",
        local: "xmlns",
        ns: "http://www.w3.org/2000/xmlns/",
        value: "urn:d",
    },
    Attr {
        qname: "xmlns:a",
        prefix: "xmlns",
        local: "a",
        ns: "http://www.w3.org/2000/xmlns/",
        value: "urn:a",
    },
    Attr { qname: "k", prefix: "", local: "k", ns: "", value: "v" },
    Attr { qname: "a:j", prefix: "a", local: "j", ns: "urn:a", value: "w" },
];

// ---------------------------------------------------------------------
// The reader's getters.
// ---------------------------------------------------------------------

type Getter = extern "system" fn(Obj, *mut *const u16, *mut u32) -> HResult;

/// One of the reader's string getters: the string and its length.
fn string(reader: Obj, slot: usize) -> (HResult, *const u16, u32) {
    let mut p: *const u16 = null_mut();
    let mut len = 0xdead_beefu32;
    let obj = reader;
    // SAFETY: a live reader whose vtable has this getter at this slot
    // (xmllite.idl).
    let hr = unsafe {
        let vtable = *(obj as *const *const usize);
        let f: Getter = core::mem::transmute(*vtable.add(slot));
        f(obj, &mut p, &mut len)
    };
    (hr, p, len)
}

fn u32_at(reader: Obj, slot: usize) -> (HResult, u32) {
    let mut v = 0xdead_beefu32;
    let hr = call!(reader, slot, fn(*mut u32) -> HResult, &mut v);
    (hr, v)
}

fn property(reader: Obj, which: u32) -> (HResult, isize) {
    let mut v: isize = -1;
    let hr = call!(reader, GET_PROPERTY, fn(u32, *mut isize) -> HResult, which, &mut v);
    (hr, v)
}

fn flag(reader: Obj, slot: usize) -> bool {
    call!(reader, slot, fn() -> i32,) != 0
}

/// The node the reader is on, as a `node` line: the transcript of a run,
/// printed whether or not the checks hold, since a reader that answers
/// one field differently is read here and not guessed at.
fn transcript(reader: Obj, i: usize) {
    let (_, qname, _) = string(reader, GET_QUALIFIED_NAME);
    let (_, prefix, _) = string(reader, GET_PREFIX);
    let (_, local, _) = string(reader, GET_LOCAL_NAME);
    let (_, ns, _) = string(reader, GET_NAMESPACE_URI);
    let (_, value, _) = string(reader, GET_VALUE);
    let (_, ty) = u32_at(reader, GET_NODE_TYPE);
    let (_, depth) = u32_at(reader, GET_DEPTH);
    let (_, line) = u32_at(reader, GET_LINE_NUMBER);
    let (_, pos) = u32_at(reader, GET_LINE_POSITION);
    let (_, attrs) = u32_at(reader, GET_ATTRIBUTE_COUNT);
    Line::new()
        .s(NAME.as_bytes())
        .s(b": node ")
        .dec(i as u64)
        .s(b" type=")
        .dec(u64::from(ty))
        .s(b" depth=")
        .dec(u64::from(depth))
        .s(b" line=")
        .dec(u64::from(line))
        .s(b" pos=")
        .dec(u64::from(pos))
        .s(b" empty=")
        .dec(u64::from(flag(reader, IS_EMPTY_ELEMENT)))
        .s(b" default=")
        .dec(u64::from(flag(reader, IS_DEFAULT)))
        .s(b" eof=")
        .dec(u64::from(flag(reader, IS_EOF)))
        .s(b" attrs=")
        .dec(u64::from(attrs))
        .s(b" qname=[")
        .ws(qname)
        .s(b"] prefix=[")
        .ws(prefix)
        .s(b"] local=[")
        .ws(local)
        .s(b"] ns=[")
        .ws(ns)
        .s(b"] value=[")
        .ws(value)
        .s(b"]\r\n")
        .print();
}

/// The node the reader is on against the row that describes it.
fn check_node(c: &mut Checks, reader: Obj, want: &Node) -> Result<(), u32> {
    let (hr, ty) = u32_at(reader, GET_NODE_TYPE);
    c.hr(hr, S_OK)?;
    c.eq(u64::from(ty), u64::from(want.ty))?;
    let (hr, depth) = u32_at(reader, GET_DEPTH);
    c.hr(hr, S_OK)?;
    c.eq(u64::from(depth), u64::from(want.depth))?;
    let (hr, line) = u32_at(reader, GET_LINE_NUMBER);
    c.hr(hr, S_OK)?;
    c.eq(u64::from(line), u64::from(want.line))?;
    let (hr, pos) = u32_at(reader, GET_LINE_POSITION);
    c.hr(hr, S_OK)?;
    c.eq(u64::from(pos), u64::from(want.pos))?;
    let (hr, attrs) = u32_at(reader, GET_ATTRIBUTE_COUNT);
    c.hr(hr, S_OK)?;
    c.eq(u64::from(attrs), u64::from(want.attrs))?;
    c.eq(u64::from(flag(reader, IS_EMPTY_ELEMENT)), u64::from(want.empty))?;
    c.ok(!flag(reader, IS_DEFAULT), 1)?;
    c.ok(!flag(reader, IS_EOF), 1)?;
    for (slot, want) in [
        (GET_QUALIFIED_NAME, want.qname),
        (GET_PREFIX, want.prefix),
        (GET_LOCAL_NAME, want.local),
        (GET_NAMESPACE_URI, want.ns),
        (GET_VALUE, want.value),
    ] {
        let (hr, p, len) = string(reader, slot);
        c.hr(hr, S_OK)?;
        c.text(p, len, want)?;
    }
    Ok(())
}

/// The root's attributes, through MoveToFirstAttribute and
/// MoveToNextAttribute, and back to the element.
fn attributes(c: &mut Checks, reader: Obj) -> Result<(), u32> {
    c.hr(call!(reader, MOVE_TO_FIRST_ATTRIBUTE, fn() -> HResult,), S_OK)?;
    for (i, want) in ATTRS.iter().enumerate() {
        if i > 0 {
            c.hr(call!(reader, MOVE_TO_NEXT_ATTRIBUTE, fn() -> HResult,), S_OK)?;
        }
        let (_, qname, _) = string(reader, GET_QUALIFIED_NAME);
        let (_, value, _) = string(reader, GET_VALUE);
        Line::new()
            .s(NAME.as_bytes())
            .s(b": attr ")
            .dec(i as u64)
            .s(b" qname=[")
            .ws(qname)
            .s(b"] value=[")
            .ws(value)
            .s(b"] default=")
            .dec(u64::from(flag(reader, IS_DEFAULT)))
            .s(b"\r\n")
            .print();
        for (slot, want) in [
            (GET_QUALIFIED_NAME, want.qname),
            (GET_PREFIX, want.prefix),
            (GET_LOCAL_NAME, want.local),
            (GET_NAMESPACE_URI, want.ns),
            (GET_VALUE, want.value),
        ] {
            let (hr, p, len) = string(reader, slot);
            c.hr(hr, S_OK)?;
            c.text(p, len, want)?;
        }
    }
    // Past the last one.
    c.hr(call!(reader, MOVE_TO_NEXT_ATTRIBUTE, fn() -> HResult,), S_FALSE)?;
    // By name: the one in a namespace by its local name and URI, the
    // plain one by its name alone.
    let (local, ns) = (Wide::new("j"), Wide::new("urn:a"));
    c.hr(
        call!(
            reader,
            MOVE_TO_ATTRIBUTE_BY_NAME,
            fn(*const u16, *const u16) -> HResult,
            local.ptr(),
            ns.ptr()
        ),
        S_OK,
    )?;
    let (hr, p, len) = string(reader, GET_VALUE);
    c.hr(hr, S_OK)?;
    c.text(p, len, "w")?;
    let k = Wide::new("k");
    c.hr(
        call!(
            reader,
            MOVE_TO_ATTRIBUTE_BY_NAME,
            fn(*const u16, *const u16) -> HResult,
            k.ptr(),
            null_mut()
        ),
        S_OK,
    )?;
    let (hr, p, len) = string(reader, GET_VALUE);
    c.hr(hr, S_OK)?;
    c.text(p, len, "v")?;
    let absent = Wide::new("nope");
    c.hr(
        call!(
            reader,
            MOVE_TO_ATTRIBUTE_BY_NAME,
            fn(*const u16, *const u16) -> HResult,
            absent.ptr(),
            null_mut()
        ),
        S_FALSE,
    )?;
    c.hr(call!(reader, MOVE_TO_ELEMENT, fn() -> HResult,), S_OK)?;
    let (_, ty) = u32_at(reader, GET_NODE_TYPE);
    c.eq(u64::from(ty), u64::from(NODE_ELEMENT))?;
    let (hr, p, len) = string(reader, GET_LOCAL_NAME);
    c.hr(hr, S_OK)?;
    c.text(p, len, "r")
}

/// Returns how many checks held, which the `ok` line prints.
fn run() -> Result<u32, u32> {
    let mut c = Checks(0, 0);
    let mut reader: Obj = null_mut();
    // SAFETY: a plain call; the IID and the out pointer are ours.
    c.hr(unsafe { CreateXmlReader(&IID_IXML_READER, &mut reader, null_mut()) }, S_OK)?;
    c.ok(!reader.is_null(), 0)?;

    // With no input: nothing read, and the defaults. The node type is
    // None and GetNodeType says so with S_FALSE, and the read state is
    // Closed and not Initial -- a reader is Initial once it has an input
    // it has not read.
    let (hr, ty) = u32_at(reader, GET_NODE_TYPE);
    c.hr(hr, S_FALSE)?;
    c.eq(u64::from(ty), u64::from(NODE_NONE))?;
    c.ok(!flag(reader, IS_EOF), 1)?;
    let (hr, state) = property(reader, PROP_READ_STATE);
    c.hr(hr, S_OK)?;
    c.eq(state as u64, READ_STATE_CLOSED as u64)?;
    let (hr, level) = property(reader, PROP_CONFORMANCE_LEVEL);
    c.hr(hr, S_OK)?;
    c.eq(level as u64, 2)?;
    let (hr, dtd) = property(reader, PROP_DTD_PROCESSING);
    c.hr(hr, S_OK)?;
    c.eq(dtd as u64, 0)?;
    c.hr(call!(reader, SET_PROPERTY, fn(u32, isize) -> HResult, PROP_DTD_PROCESSING, 0), S_OK)?;

    // The document, node by node.
    let mut stream = Stream::new(DOC);
    c.hr(call!(reader, SET_INPUT, fn(Obj) -> HResult, stream.obj()), S_OK)?;
    let (hr, state) = property(reader, PROP_READ_STATE);
    c.hr(hr, S_OK)?;
    c.eq(state as u64, READ_STATE_INITIAL as u64)?;
    for (i, want) in NODES.iter().enumerate() {
        let mut ty = 0xdead_beefu32;
        c.hr(call!(reader, READ, fn(*mut u32) -> HResult, &mut ty), S_OK)?;
        transcript(reader, i);
        c.eq(u64::from(ty), u64::from(want.ty))?;
        check_node(&mut c, reader, want)?;
        if want.ty == NODE_ELEMENT && want.attrs > 0 {
            attributes(&mut c, reader)?;
        }
    }
    // Past the root.
    let mut ty = 0xdead_beefu32;
    c.hr(call!(reader, READ, fn(*mut u32) -> HResult, &mut ty), S_FALSE)?;
    c.eq(u64::from(ty), u64::from(NODE_NONE))?;
    c.ok(flag(reader, IS_EOF), 0)?;
    let (hr, state) = property(reader, PROP_READ_STATE);
    c.hr(hr, S_OK)?;
    c.eq(state as u64, READ_STATE_END_OF_FILE as u64)?;
    // Again: the same answer, and nothing read.
    c.hr(call!(reader, READ, fn(*mut u32) -> HResult, &mut ty), S_FALSE)?;
    c.eq(u64::from(ty), u64::from(NODE_NONE))?;

    Line::new()
        .s(NAME.as_bytes())
        .s(b": stream used=")
        .hex(u64::from(stream.used))
        .s(b" reads=")
        .dec(u64::from(stream.reads))
        .s(b" refs=")
        .dec(u64::from(stream.refs))
        .s(b" left=")
        .dec((stream.bytes.len() - stream.pos) as u64)
        .s(b" unknown=")
        .guid(&stream.unknown)
        .s(b"\r\n")
        .print();
    // What the reader asks of a stream, measured: QueryInterface (for
    // IStream, and for one IID of its own that a stream may refuse) and
    // Read, which it calls until it is given nothing -- never AddRef or
    // Release, and never Seek. gala's reader needs no more of one, and a
    // program may hand it no more.
    c.eq(u64::from(stream.used & 0xffff), 0b1001)?;
    c.eq(u64::from(stream.reads), 2)?;
    c.ok(stream.used & used::STREAM_IID != 0, u64::from(stream.used))?;

    // Documents that are not well formed: the Read that meets the error
    // answers it, and the reader then stays in the Error read state.
    // Setting the input again is what starts it over, which every entry
    // after the first relies on.
    for bad in BADS {
        let mut input = Stream::new(bad.doc);
        c.hr(call!(reader, SET_INPUT, fn(Obj) -> HResult, input.obj()), S_OK)?;
        let mut hr = S_OK;
        let mut reads: u32 = 0;
        // The node each read hands out, kept for the measuring print:
        // where gala's reader and Windows' part company it is a node one
        // of them makes and the other refuses, and the count alone does
        // not say which.
        let mut types = [0u32; 16];
        while hr == S_OK && reads < 16 {
            let mut ty = 0xdead_beefu32;
            hr = call!(reader, READ, fn(*mut u32) -> HResult, &mut ty);
            types[reads as usize] = ty;
            reads += 1;
        }
        if MEASURE {
            let mut line = Line::new();
            line = line
                .s(NAME.as_bytes())
                .s(b": bad hr=")
                .hex(hr as u32 as u64)
                .s(b" reads=")
                .dec(u64::from(reads))
                .s(b" types=");
            for (i, &ty) in types.iter().enumerate().take(reads as usize) {
                if i > 0 {
                    line = line.s(b",");
                }
                line = line.dec(u64::from(ty));
            }
            line.s(b" doc=").s(bad.doc).s(b" -- ").s(bad.what.as_bytes()).s(b"\r\n").print();
        }
        c.hr(hr, bad.hr)?;
        // Either reader's count, since one row has one of each.
        c.ok(reads == bad.reads || reads == bad.win, u64::from(reads))?;
        let (hr, state) = property(reader, PROP_READ_STATE);
        c.hr(hr, S_OK)?;
        c.eq(state as u64, READ_STATE_ERROR as u64)?;
    }

    // A reader whose input is set again reads from the start.
    let mut again = Stream::new(b"<x>1</x>");
    c.hr(call!(reader, SET_INPUT, fn(Obj) -> HResult, again.obj()), S_OK)?;
    let mut ty = 0xdead_beefu32;
    c.hr(call!(reader, READ, fn(*mut u32) -> HResult, &mut ty), S_OK)?;
    c.eq(u64::from(ty), u64::from(NODE_ELEMENT))?;
    let (hr, state) = property(reader, PROP_READ_STATE);
    c.hr(hr, S_OK)?;
    c.eq(state as u64, READ_STATE_INTERACTIVE as u64)?;
    let (hr, p, len) = string(reader, GET_LOCAL_NAME);
    c.hr(hr, S_OK)?;
    c.text(p, len, "x")?;

    // Release gives up the last reference.
    c.eq(u64::from(call!(reader, RELEASE, fn() -> u32,)), 0)?;
    if c.1 != 0 {
        return Err(c.1);
    }
    Ok(c.0)
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    let code = match run() {
        Ok(checks) => {
            Line::new()
                .s(NAME.as_bytes())
                .s(b": ok (")
                .dec(u64::from(checks))
                .s(b" checks)\r\n")
                .print();
            0
        }
        Err(n) => n,
    };
    // SAFETY: ends the process.
    unsafe { ExitProcess(code) }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xc000_0409) }
}
