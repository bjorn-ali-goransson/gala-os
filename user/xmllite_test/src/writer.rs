//! XmlLite's writer (xmllite.dll's `CreateXmlWriter` and `IXmlWriter`),
//! as Windows answers it. Word delay-loads `CreateXmlWriter` and ends on
//! 0xc06d007f without it (ROADMAP OF), as it did without the reader.
//!
//! The output is this program's own stream, which keeps the bytes and
//! records every method it is called on, one bit per vtable slot.
//! Everything but QueryInterface, AddRef, Release, Write and Commit
//! answers E_NOTIMPL, so what the writer asks of a stream is a result of
//! the run (the `out` line) and not a guess: a program may hand it a
//! stream that can only write.
//!
//! Every case prints what it left on the stream as a `doc` line, whether
//! or not the bytes match, and is then checked byte for byte against
//! what Windows wrote. Measured, all of it:
//! - the object: a wrong IID is E_NOINTERFACE with the out pointer
//!   NULLed, no out pointer at all is E_INVALIDARG, and the writer is an
//!   IUnknown and an IXmlWriter and nothing else;
//! - the properties: MultiLanguage 0, Indent 0, ByteOrderMark 1,
//!   OmitXmlDeclaration 0, ConformanceLevel 2 and an undocumented fifth
//!   that reads 0, while a sixth is E_NOTIMPL both ways and leaves the
//!   out value alone; nowhere to put the answer is E_INVALIDARG; and a
//!   property set once bytes have been written is E_ACCESSDENIED, so a
//!   program sets them before the first write and not after;
//! - the output: a write before SetOutput is E_UNEXPECTED while Flush is
//!   S_OK, SetOutput(NULL) is S_OK and leaves it that way, an object that
//!   is no stream is E_NOINTERFACE and keeps no reference, and either
//!   IStream or ISequentialStream alone is enough;
//! - the bytes: the declaration, an empty element, a full end element,
//!   what indenting looks like, what an attribute value escapes and what
//!   text escapes, the character and entity references, and that nothing
//!   reaches the stream until Flush, WriteEndDocument or Release -- which
//!   closes what is open, where Flush does not and will write half a
//!   start tag;
//! - every call a writer refuses, with the code Windows answers, and
//!   which of those refusals leave the writer dead.
//!
//! Three slots take an `IXmlReader` (WriteAttributes, WriteNode and
//! WriteNodeShallow) and are not measured here; gala's writer records
//! them as the gap they are.

use core::ptr::null_mut;

use crate::used;
use crate::{Checks, Guid, HResult, Line, Obj, StreamVtable, Wide};
use crate::{E_NOINTERFACE, E_NOTIMPL, NAME, RELEASE, S_OK};
use crate::{IID_ISEQUENTIAL_STREAM, IID_ISTREAM, IID_IUNKNOWN, IID_IXML_READER};

#[link(name = "xmllite", kind = "raw-dylib")]
unsafe extern "system" {
    fn CreateXmlWriter(iid: *const Guid, out: *mut Obj, malloc: Obj) -> HResult;
}

const E_INVALIDARG: HResult = 0x8007_0057u32 as HResult;
const E_ACCESSDENIED: HResult = 0x8007_0005u32 as HResult;
const E_UNEXPECTED: HResult = 0x8000_ffffu32 as HResult;

// The writer's own codes, from xmllite.h.
const WR_E_NONWHITESPACE: HResult = 0xc00c_ef01u32 as HResult;
const WR_E_NSPREFIXDECLARED: HResult = 0xc00c_ef02u32 as HResult;
const WR_E_NSPREFIXWITHEMPTYNSURI: HResult = 0xc00c_ef03u32 as HResult;
const WR_E_DUPLICATEATTRIBUTE: HResult = 0xc00c_ef04u32 as HResult;
const WR_E_NAMESPACEUNDECLARED: HResult = 0xc00c_ef09u32 as HResult;
const WR_E_INVALIDXMLSPACE: HResult = 0xc00c_ef0au32 as HResult;
const WR_E_INVALIDACTION: HResult = 0xc00c_ef0bu32 as HResult;
const WC_E_XMLCHARACTER: HResult = 0xc00c_ee2bu32 as HResult;
const WC_E_NAMECHARACTER: HResult = 0xc00c_ee2cu32 as HResult;

/// `IXmlWriter` (xmllite.idl, uuid 7279FC88-709D-4095-B63D-69FE4B0D9030).
const IID_IXML_WRITER: Guid = [
    0x88, 0xfc, 0x79, 0x72, 0x9d, 0x70, 0x95, 0x40, 0xb6, 0x3d, 0x69, 0xfe, 0x4b, 0x0d, 0x90, 0x30,
];
/// `IMarshal` (00000003-0000-0000-C000-000000000046), which it refuses.
const IID_IMARSHAL: Guid = [3, 0, 0, 0, 0, 0, 0, 0, 0xc0, 0, 0, 0, 0, 0, 0, 0x46];

// IXmlWriter's vtable, in the order of xmllite.idl (gala's writer lists
// every slot, and this program calls the ones that take no reader).
const SET_OUTPUT: usize = 3;
const GET_PROPERTY: usize = 4;
const SET_PROPERTY: usize = 5;
const WRITE_ATTRIBUTE_STRING: usize = 7;
const WRITE_CDATA: usize = 8;
const WRITE_CHAR_ENTITY: usize = 9;
const WRITE_CHARS: usize = 10;
const WRITE_COMMENT: usize = 11;
const WRITE_DOC_TYPE: usize = 12;
const WRITE_ELEMENT_STRING: usize = 13;
const WRITE_END_DOCUMENT: usize = 14;
const WRITE_END_ELEMENT: usize = 15;
const WRITE_ENTITY_REF: usize = 16;
const WRITE_FULL_END_ELEMENT: usize = 17;
const WRITE_NAME: usize = 18;
const WRITE_NMTOKEN: usize = 19;
const WRITE_PI: usize = 22;
const WRITE_QUALIFIED_NAME: usize = 23;
const WRITE_RAW: usize = 24;
const WRITE_RAW_CHARS: usize = 25;
const WRITE_START_DOCUMENT: usize = 26;
const WRITE_START_ELEMENT: usize = 27;
const WRITE_STRING: usize = 28;
const WRITE_SURROGATE_CHAR_ENTITY: usize = 29;
const WRITE_WHITESPACE: usize = 30;
const FLUSH: usize = 31;

// XmlWriterProperty.
const PROP_MULTI_LANGUAGE: u32 = 0;
const PROP_INDENT: u32 = 1;
const PROP_BYTE_ORDER_MARK: u32 = 2;
const PROP_OMIT_XML_DECLARATION: u32 = 3;
const PROP_CONFORMANCE_LEVEL: u32 = 4;
/// Neither of these is a documented property: the first reads 0 and the
/// second is refused, which is where the enumeration ends.
const PROP_UNDOCUMENTED: u32 = 5;
const PROP_PAST_THE_END: u32 = 6;

// XmlStandalone.
const STANDALONE_OMIT: u32 = 0;
const STANDALONE_YES: u32 = 1;
const STANDALONE_NO: u32 = 2;

// XmlConformanceLevel.
const LEVEL_FRAGMENT: isize = 1;
const LEVEL_DOCUMENT: isize = 2;

// ---------------------------------------------------------------------
// The output: our own stream, which keeps what it is given.
// ---------------------------------------------------------------------

/// Enough for every case here, the long string included.
const CAP: usize = 8 * 1024;

/// The slots a writer must never call, so that a program may hand it a
/// stream that can only write: Seek, SetSize, CopyTo, Revert,
/// LockRegion, UnlockRegion, Stat and Clone.
const NEVER: u32 =
    (1 << 5) | (1 << 6) | (1 << 7) | (1 << 9) | (1 << 10) | (1 << 11) | (1 << 12) | (1 << 13);

/// A stream that records the bytes written to it and which of its
/// methods were called.
#[repr(C)]
struct Out {
    vtable: *const StreamVtable,
    buf: [u8; CAP],
    /// Every byte written, even past what `buf` keeps.
    len: usize,
    writes: u32,
    commits: u32,
    refs: i32,
    used: u32,
    /// The first IID the writer asked for that is neither IUnknown nor a
    /// stream: `SetOutput` has one of its own, and a stream may refuse
    /// it, as this one does.
    unknown: Guid,
    /// Which stream interface this one admits to, so a writer given only
    /// one of the two is a case of its own.
    allow_stream: bool,
    allow_sequential: bool,
}

/// `this` as our stream.
///
/// # Safety
/// `this` is a pointer to a live [`Out`] (we handed it to the writer).
unsafe fn mark(this: Obj, slot: u32) -> &'static mut Out {
    // SAFETY: the caller guarantees a live Out, and the program is
    // single-threaded.
    let s = unsafe { &mut *(this as *mut Out) };
    s.used |= 1 << slot;
    s
}

extern "system" fn ow_query(this: Obj, iid: *const Guid, out: *mut Obj) -> HResult {
    // SAFETY: the writer calls this on the object we handed it.
    let s = unsafe { mark(this, 0) };
    // SAFETY: an IID from the caller.
    let iid = unsafe { &*iid };
    let sequential = *iid == IID_ISEQUENTIAL_STREAM;
    let stream = *iid == IID_ISTREAM;
    if sequential {
        s.used |= used::SEQUENTIAL_IID;
    }
    if stream {
        s.used |= used::STREAM_IID;
    }
    let known =
        *iid == IID_IUNKNOWN || (sequential && s.allow_sequential) || (stream && s.allow_stream);
    if !known {
        if !sequential && !stream && s.used & used::UNKNOWN_IID == 0 {
            s.unknown = *iid;
            s.used |= used::UNKNOWN_IID;
        }
        // SAFETY: an out pointer from the caller.
        unsafe { *out = null_mut() };
        return E_NOINTERFACE;
    }
    s.refs += 1;
    // SAFETY: as above.
    unsafe { *out = this };
    S_OK
}

extern "system" fn ow_add_ref(this: Obj) -> u32 {
    // SAFETY: see `ow_query`.
    let s = unsafe { mark(this, 1) };
    s.refs += 1;
    s.refs as u32
}

extern "system" fn ow_release(this: Obj) -> u32 {
    // SAFETY: see `ow_query`. The object is on the case's stack and
    // outlives the writer, so the last release frees nothing.
    let s = unsafe { mark(this, 2) };
    s.refs -= 1;
    s.refs as u32
}

extern "system" fn ow_write(this: Obj, buf: *const u8, len: u32, wrote: *mut u32) -> HResult {
    // SAFETY: see `ow_query`.
    let s = unsafe { mark(this, 4) };
    s.writes += 1;
    for i in 0..len as usize {
        if s.len + i < CAP {
            // SAFETY: a buffer of at least `len` bytes from the caller.
            s.buf[s.len + i] = unsafe { *buf.add(i) };
        }
    }
    s.len += len as usize;
    if !wrote.is_null() {
        // SAFETY: an out pointer from the caller.
        unsafe { *wrote = len };
    }
    S_OK
}

extern "system" fn ow_commit(this: Obj, _: u32) -> HResult {
    // SAFETY: see `ow_query`.
    let s = unsafe { mark(this, 8) };
    s.commits += 1;
    S_OK
}

/// Every other method: E_NOTIMPL, so what the writer needs of a stream
/// is what the run shows it used.
macro_rules! notimpl {
    ($name:ident, $slot:expr, fn($($t:ty),*)) => {
        extern "system" fn $name(this: Obj $(, _: $t)*) -> HResult {
            // SAFETY: see `ow_query`.
            unsafe { mark(this, $slot) };
            E_NOTIMPL
        }
    };
}

notimpl!(ow_read, 3, fn(*mut u8, u32, *mut u32));
notimpl!(ow_seek, 5, fn(i64, u32, *mut u64));
notimpl!(ow_set_size, 6, fn(u64));
notimpl!(ow_copy_to, 7, fn(Obj, u64, *mut u64, *mut u64));
notimpl!(ow_revert, 9, fn());
notimpl!(ow_lock, 10, fn(u64, u64, u32));
notimpl!(ow_unlock, 11, fn(u64, u64, u32));
notimpl!(ow_stat, 12, fn(*mut u8, u32));
notimpl!(ow_clone, 13, fn(*mut Obj));

static OUT_VTABLE: StreamVtable = StreamVtable {
    query: ow_query,
    add_ref: ow_add_ref,
    release: ow_release,
    read: ow_read,
    write: ow_write,
    seek: ow_seek,
    set_size: ow_set_size,
    copy_to: ow_copy_to,
    commit: ow_commit,
    revert: ow_revert,
    lock: ow_lock,
    unlock: ow_unlock,
    stat: ow_stat,
    clone: ow_clone,
};

impl Out {
    fn new() -> Self {
        Self {
            vtable: &OUT_VTABLE,
            buf: [0; CAP],
            len: 0,
            writes: 0,
            commits: 0,
            refs: 1,
            used: 0,
            unknown: [0; 16],
            allow_stream: true,
            allow_sequential: true,
        }
    }

    fn obj(&mut self) -> Obj {
        self as *mut Out as Obj
    }

    /// What it kept, which is everything up to [`CAP`].
    fn bytes(&self) -> &[u8] {
        let n = if self.len < CAP { self.len } else { CAP };
        &self.buf[..n]
    }
}

/// An object that is no stream at all: `SetOutput` must refuse it and
/// keep no reference to it.
#[repr(C)]
struct Plain {
    vtable: *const PlainVtable,
    refs: i32,
}

#[repr(C)]
struct PlainVtable {
    query: extern "system" fn(Obj, *const Guid, *mut Obj) -> HResult,
    add_ref: extern "system" fn(Obj) -> u32,
    release: extern "system" fn(Obj) -> u32,
}

extern "system" fn pl_query(this: Obj, iid: *const Guid, out: *mut Obj) -> HResult {
    // SAFETY: an IID and an out pointer from the caller.
    unsafe {
        if *iid != IID_IUNKNOWN {
            *out = null_mut();
            return E_NOINTERFACE;
        }
        let p = &mut *(this as *mut Plain);
        p.refs += 1;
        *out = this;
    }
    S_OK
}

extern "system" fn pl_add_ref(this: Obj) -> u32 {
    // SAFETY: a live Plain.
    let p = unsafe { &mut *(this as *mut Plain) };
    p.refs += 1;
    p.refs as u32
}

extern "system" fn pl_release(this: Obj) -> u32 {
    // SAFETY: a live Plain, which outlives every reference to it.
    let p = unsafe { &mut *(this as *mut Plain) };
    p.refs -= 1;
    p.refs as u32
}

static PLAIN_VTABLE: PlainVtable =
    PlainVtable { query: pl_query, add_ref: pl_add_ref, release: pl_release };

// ---------------------------------------------------------------------
// The writer's methods.
// ---------------------------------------------------------------------

/// A string argument that may be null, which most of the writer's
/// methods take and answer E_INVALIDARG or S_OK for.
struct Arg(Wide, bool);

impl Arg {
    fn new(v: Option<&str>) -> Self {
        match v {
            Some(t) => Arg(Wide::new(t), true),
            None => Arg(Wide::new(""), false),
        }
    }

    fn ptr(&self) -> *const u16 {
        if self.1 { self.0.ptr() } else { core::ptr::null() }
    }
}

/// One of the writer's methods that takes nothing but strings, of
/// which there is one with one, one with two, two with three and two
/// with four (a repetition cannot spell out the types, which hold no
/// name of their own to repeat on).
macro_rules! writes {
    ($name:ident, $slot:expr, $a:ident) => {
        fn $name(w: Obj, $a: Option<&str>) -> HResult {
            let $a = Arg::new($a);
            call!(w, $slot, fn(*const u16) -> HResult, $a.ptr())
        }
    };
    ($name:ident, $slot:expr, $a:ident, $b:ident) => {
        fn $name(w: Obj, $a: Option<&str>, $b: Option<&str>) -> HResult {
            let ($a, $b) = (Arg::new($a), Arg::new($b));
            call!(w, $slot, fn(*const u16, *const u16) -> HResult, $a.ptr(), $b.ptr())
        }
    };
    ($name:ident, $slot:expr, $a:ident, $b:ident, $c:ident) => {
        fn $name(w: Obj, $a: Option<&str>, $b: Option<&str>, $c: Option<&str>) -> HResult {
            let ($a, $b, $c) = (Arg::new($a), Arg::new($b), Arg::new($c));
            call!(
                w,
                $slot,
                fn(*const u16, *const u16, *const u16) -> HResult,
                $a.ptr(),
                $b.ptr(),
                $c.ptr()
            )
        }
    };
    ($name:ident, $slot:expr, $a:ident, $b:ident, $c:ident, $d:ident) => {
        fn $name(
            w: Obj,
            $a: Option<&str>,
            $b: Option<&str>,
            $c: Option<&str>,
            $d: Option<&str>,
        ) -> HResult {
            let ($a, $b, $c, $d) = (Arg::new($a), Arg::new($b), Arg::new($c), Arg::new($d));
            call!(
                w,
                $slot,
                fn(*const u16, *const u16, *const u16, *const u16) -> HResult,
                $a.ptr(),
                $b.ptr(),
                $c.ptr(),
                $d.ptr()
            )
        }
    };
}

writes!(start_element, WRITE_START_ELEMENT, prefix, local, ns);
writes!(attr, WRITE_ATTRIBUTE_STRING, prefix, local, ns, value);
writes!(element_string, WRITE_ELEMENT_STRING, prefix, local, ns, value);
writes!(string, WRITE_STRING, value);
writes!(raw, WRITE_RAW, value);
writes!(comment, WRITE_COMMENT, value);
writes!(cdata, WRITE_CDATA, value);
writes!(pi, WRITE_PI, name, value);
writes!(whitespace, WRITE_WHITESPACE, value);
writes!(entity_ref, WRITE_ENTITY_REF, value);
writes!(name, WRITE_NAME, value);
writes!(nmtoken, WRITE_NMTOKEN, value);
writes!(qualified_name, WRITE_QUALIFIED_NAME, local, ns);
writes!(doc_type, WRITE_DOC_TYPE, name, public, system, subset);

fn start_document(w: Obj, standalone: u32) -> HResult {
    call!(w, WRITE_START_DOCUMENT, fn(u32) -> HResult, standalone)
}

fn end_element(w: Obj) -> HResult {
    call!(w, WRITE_END_ELEMENT, fn() -> HResult,)
}

fn full_end_element(w: Obj) -> HResult {
    call!(w, WRITE_FULL_END_ELEMENT, fn() -> HResult,)
}

fn end_document(w: Obj) -> HResult {
    call!(w, WRITE_END_DOCUMENT, fn() -> HResult,)
}

fn flush(w: Obj) -> HResult {
    call!(w, FLUSH, fn() -> HResult,)
}

fn char_entity(w: Obj, ch: u16) -> HResult {
    call!(w, WRITE_CHAR_ENTITY, fn(u16) -> HResult, ch)
}

fn surrogate(w: Obj, low: u16, high: u16) -> HResult {
    call!(w, WRITE_SURROGATE_CHAR_ENTITY, fn(u16, u16) -> HResult, low, high)
}

fn chars(w: Obj, s: &str) -> HResult {
    let a = Wide::new(s);
    call!(w, WRITE_CHARS, fn(*const u16, u32) -> HResult, a.ptr(), s.encode_utf16().count() as u32)
}

fn raw_chars(w: Obj, s: &str) -> HResult {
    let a = Wide::new(s);
    let n = s.encode_utf16().count() as u32;
    call!(w, WRITE_RAW_CHARS, fn(*const u16, u32) -> HResult, a.ptr(), n)
}

fn set_output(w: Obj, out: Obj) -> HResult {
    call!(w, SET_OUTPUT, fn(Obj) -> HResult, out)
}

fn set_prop(w: Obj, which: u32, value: isize) -> HResult {
    call!(w, SET_PROPERTY, fn(u32, isize) -> HResult, which, value)
}

/// A property and the value it answered, which is left at the sentinel
/// when the writer refuses the call.
fn get_prop(w: Obj, which: u32) -> (HResult, isize) {
    let mut v: isize = 0x7ead_bee5;
    let hr = call!(w, GET_PROPERTY, fn(u32, *mut isize) -> HResult, which, &mut v);
    (hr, v)
}

fn release(w: Obj) -> u32 {
    call!(w, RELEASE, fn() -> u32,)
}

// ---------------------------------------------------------------------
// The cases.
// ---------------------------------------------------------------------

/// A writer with no output yet.
fn writer(c: &mut Checks) -> Result<Obj, u32> {
    let mut w: Obj = null_mut();
    // SAFETY: a plain call; the IID and the out pointer are ours.
    c.hr(unsafe { CreateXmlWriter(&IID_IXML_WRITER, &mut w, null_mut()) }, S_OK)?;
    c.ok(!w.is_null(), 0)?;
    Ok(w)
}

/// A writer whose properties are set and whose output is `out`. The
/// order is the one a program has to use: writing locks the properties,
/// so they go in before the first call that produces bytes.
fn writing(c: &mut Checks, props: &[(u32, isize)], out: &mut Out) -> Result<Obj, u32> {
    let w = writer(c)?;
    for &(which, value) in props {
        c.hr(set_prop(w, which, value), S_OK)?;
    }
    c.hr(set_output(w, out.obj()), S_OK)?;
    Ok(w)
}

/// What a case left on its stream, as a `doc` line -- printed whether or
/// not it is what Windows wrote, since a writer that differs is read
/// here and not guessed at -- and then checked byte for byte.
fn doc(c: &mut Checks, what: &str, out: &Out, want: &[u8]) -> Result<(), u32> {
    Line::new()
        .s(NAME.as_bytes())
        .s(b": doc ")
        .s(what.as_bytes())
        .s(b" ")
        .dec(out.len as u64)
        .s(b" bytes \"")
        .esc(out.bytes())
        .s(b"\"\r\n")
        .print();
    c.eq(out.len as u64, want.len() as u64)?;
    c.ok(out.bytes() == want, out.len as u64)
}

/// The object, its properties, and what it answers with no output.
fn object(c: &mut Checks) -> Result<(), u32> {
    // A writer is asked for by IID, and the reader's is not it.
    let mut p: Obj = usize::MAX as Obj;
    // SAFETY: a plain call; the IID and the out pointer are ours.
    c.hr(unsafe { CreateXmlWriter(&IID_IXML_READER, &mut p, null_mut()) }, E_NOINTERFACE)?;
    c.ok(p.is_null(), 1)?;
    // SAFETY: as above, with nowhere to put the answer.
    c.hr(unsafe { CreateXmlWriter(&IID_IXML_WRITER, null_mut(), null_mut()) }, E_INVALIDARG)?;

    let w = writer(c)?;
    // It is an IUnknown and an IXmlWriter and nothing else.
    for (iid, want) in [
        (&IID_IUNKNOWN, S_OK),
        (&IID_IXML_WRITER, S_OK),
        (&IID_IXML_READER, E_NOINTERFACE),
        (&IID_IMARSHAL, E_NOINTERFACE),
    ] {
        let mut q: Obj = usize::MAX as Obj;
        c.hr(call!(w, 0, fn(*const Guid, *mut Obj) -> HResult, iid, &mut q), want)?;
        if want == S_OK {
            c.ok(!q.is_null(), 2)?;
            release(q);
        } else {
            c.ok(q.is_null(), 3)?;
        }
    }

    // The defaults. ByteOrderMark is TRUE and no case ever sees a BOM:
    // it is for an output made with an encoding, not for a stream.
    for (which, want) in [
        (PROP_MULTI_LANGUAGE, 0),
        (PROP_INDENT, 0),
        (PROP_BYTE_ORDER_MARK, 1),
        (PROP_OMIT_XML_DECLARATION, 0),
        (PROP_CONFORMANCE_LEVEL, LEVEL_DOCUMENT),
        (PROP_UNDOCUMENTED, 0),
    ] {
        let (hr, v) = get_prop(w, which);
        c.hr(hr, S_OK)?;
        c.eq(v as u64, want as u64)?;
    }
    // Past the end of the enumeration: refused both ways, and the out
    // value is left as it was.
    let (hr, v) = get_prop(w, PROP_PAST_THE_END);
    c.hr(hr, E_NOTIMPL)?;
    c.eq(v as u64, 0x7ead_bee5)?;
    c.hr(set_prop(w, PROP_PAST_THE_END, 1), E_NOTIMPL)?;
    // Nowhere to put the answer.
    c.hr(
        call!(w, GET_PROPERTY, fn(u32, *mut isize) -> HResult, PROP_INDENT, null_mut::<isize>()),
        E_INVALIDARG,
    )?;
    // Every documented property is taken, and a level is read back.
    for (which, value) in [
        (PROP_INDENT, 1),
        (PROP_BYTE_ORDER_MARK, 0),
        (PROP_OMIT_XML_DECLARATION, 1),
        (PROP_CONFORMANCE_LEVEL, LEVEL_FRAGMENT),
    ] {
        c.hr(set_prop(w, which, value), S_OK)?;
        let (hr, v) = get_prop(w, which);
        c.hr(hr, S_OK)?;
        c.eq(v as u64, value as u64)?;
    }

    // With no output, a write is E_UNEXPECTED and Flush is S_OK --
    // except where the call is also out of place, which is answered
    // first: the level set just above is Fragment, which has no
    // document to start, and that is the code even with nowhere to
    // write it.
    c.hr(start_document(w, STANDALONE_OMIT), WR_E_INVALIDACTION)?;
    c.hr(start_element(w, None, Some("a"), None), E_UNEXPECTED)?;
    c.hr(string(w, Some("text")), E_UNEXPECTED)?;
    c.hr(flush(w), S_OK)?;
    // And SetOutput(NULL) is S_OK and leaves it that way.
    c.hr(set_output(w, null_mut()), S_OK)?;
    c.hr(start_element(w, None, Some("a"), None), E_UNEXPECTED)?;
    c.hr(end_document(w), E_UNEXPECTED)?;
    c.hr(flush(w), S_OK)?;

    // An object that is no stream is refused, and no reference is kept.
    let mut plain = Plain { vtable: &PLAIN_VTABLE, refs: 1 };
    c.hr(set_output(w, &mut plain as *mut Plain as Obj), E_NOINTERFACE)?;
    c.eq(plain.refs as u64, 1)?;

    // A property is still free right after the output is set: it is
    // writing that locks them, so a program may set them up to the
    // first call that produces bytes and not after.
    let mut out = Out::new();
    c.hr(set_output(w, out.obj()), S_OK)?;
    c.hr(set_prop(w, PROP_INDENT, 1), S_OK)?;
    c.hr(start_element(w, None, Some("r"), None), S_OK)?;
    c.hr(set_prop(w, PROP_INDENT, 0), E_ACCESSDENIED)?;
    c.hr(set_prop(w, PROP_CONFORMANCE_LEVEL, LEVEL_DOCUMENT), E_ACCESSDENIED)?;
    // And a property is read back whether or not it may still be set.
    let (hr, v) = get_prop(w, PROP_INDENT);
    c.hr(hr, S_OK)?;
    c.eq(v as u64, 1)?;
    c.eq(u64::from(release(w)), 0)?;
    Ok(())
}

/// A document with the properties left alone: the declaration, an
/// attribute, a prefixed element, an empty element, a comment, a
/// processing instruction, CDATA and a full end element. And what the
/// writer asked of the stream, which is the `out` line.
fn document(c: &mut Checks) -> Result<(), u32> {
    let mut out = Out::new();
    let w = writing(c, &[], &mut out)?;
    c.hr(start_document(w, STANDALONE_OMIT), S_OK)?;
    c.hr(start_element(w, None, Some("root"), None), S_OK)?;
    c.hr(attr(w, None, Some("id"), None, Some("7")), S_OK)?;
    c.hr(start_element(w, Some("p"), Some("child"), Some("urn:p")), S_OK)?;
    c.hr(string(w, Some("hello")), S_OK)?;
    c.hr(end_element(w), S_OK)?;
    c.hr(element_string(w, None, Some("e"), None, None), S_OK)?;
    c.hr(element_string(w, None, Some("f"), None, Some("body")), S_OK)?;
    c.hr(comment(w, Some("a note")), S_OK)?;
    c.hr(pi(w, Some("pi"), Some("go")), S_OK)?;
    c.hr(cdata(w, Some("raw < text")), S_OK)?;
    c.hr(full_end_element(w), S_OK)?;
    c.hr(end_document(w), S_OK)?;
    c.hr(flush(w), S_OK)?;
    doc(
        c,
        "document",
        &out,
        b"<?xml version=\"1.0\" encoding=\"UTF-8\"?><root id=\"7\"><p:child xmlns:p=\"urn:p\">hello</p:child><e /><f>body</f><!--a note--><?pi go?><![CDATA[raw < text]]></root>",
    )?;

    Line::new()
        .s(NAME.as_bytes())
        .s(b": out used=")
        .hex(u64::from(out.used))
        .s(b" writes=")
        .dec(u64::from(out.writes))
        .s(b" commits=")
        .dec(u64::from(out.commits))
        .s(b" refs=")
        .dec(out.refs as u64)
        .s(b" unknown=")
        .guid(&out.unknown)
        .s(b"\r\n")
        .print();
    // What the writer asks of a stream: QueryInterface, AddRef, Write
    // and Commit, and nothing else -- never Seek, SetSize, Stat or
    // Clone, so a stream that can only write is enough. The IID
    // SetOutput asks for first is one of its own and may be refused.
    c.eq(u64::from(out.used & NEVER), 0)?;
    c.ok(out.used & used::STREAM_IID != 0, u64::from(out.used))?;
    c.ok(out.commits > 0, u64::from(out.commits))?;
    c.ok(out.writes > 0, u64::from(out.writes))?;
    // The reference SetOutput took is given back with the writer.
    c.eq(u64::from(release(w)), 0)?;
    c.eq(out.refs as u64, 1)
}

/// Indenting: CRLF and two spaces a level, and no line end at the end.
fn indent(c: &mut Checks) -> Result<(), u32> {
    let mut out = Out::new();
    let w = writing(c, &[(PROP_INDENT, 1)], &mut out)?;
    c.hr(start_document(w, STANDALONE_OMIT), S_OK)?;
    c.hr(start_element(w, None, Some("root"), None), S_OK)?;
    c.hr(start_element(w, None, Some("child"), None), S_OK)?;
    c.hr(string(w, Some("hello")), S_OK)?;
    c.hr(end_element(w), S_OK)?;
    c.hr(start_element(w, None, Some("empty"), None), S_OK)?;
    c.hr(end_element(w), S_OK)?;
    c.hr(end_document(w), S_OK)?;
    c.hr(flush(w), S_OK)?;
    release(w);
    doc(
        c,
        "indent",
        &out,
        b"<?xml version=\"1.0\" encoding=\"UTF-8\"?>\r\n<root>\r\n  <child>hello</child>\r\n  <empty />\r\n</root>",
    )
}

/// What an attribute value escapes and what text escapes, which are not
/// the same, and every way a character can be written raw or by
/// reference. A line end in text becomes CRLF; in an attribute value it
/// becomes a reference.
fn escapes(c: &mut Checks) -> Result<(), u32> {
    let mut out = Out::new();
    let w = writing(c, &[(PROP_OMIT_XML_DECLARATION, 1)], &mut out)?;
    let awkward = "<&>\"'\ttab\nnl\rcr";
    c.hr(start_element(w, None, Some("a"), None), S_OK)?;
    c.hr(attr(w, None, Some("v"), None, Some(awkward)), S_OK)?;
    c.hr(string(w, Some(awkward)), S_OK)?;
    c.hr(raw(w, Some("<raw/>")), S_OK)?;
    c.hr(char_entity(w, 0x41), S_OK)?;
    c.hr(entity_ref(w, Some("amp")), S_OK)?;
    c.hr(whitespace(w, Some("  ")), S_OK)?;
    c.hr(surrogate(w, 0xdc00, 0xd800), S_OK)?;
    c.hr(chars(w, "chars"), S_OK)?;
    c.hr(raw_chars(w, "<rawchars/>"), S_OK)?;
    c.hr(name(w, Some("nm")), S_OK)?;
    c.hr(nmtoken(w, Some("tok")), S_OK)?;
    c.hr(qualified_name(w, Some("q"), None), S_OK)?;
    c.hr(end_document(w), S_OK)?;
    c.hr(flush(w), S_OK)?;
    release(w);
    doc(
        c,
        "escapes",
        &out,
        b"<a v=\"&lt;&amp;>&quot;'&#x9;tab&#xA;nl&#xD;cr\">&lt;&amp;&gt;\"'\ttab\r\nnl\r\ncr<raw/>&#x41;&amp;  &#x10000;chars<rawchars/>nmtokq</a>",
    )
}

/// A standalone declaration, with ByteOrderMark asked for: a stream
/// output never gets one.
fn declaration(c: &mut Checks) -> Result<(), u32> {
    let mut out = Out::new();
    let w = writing(c, &[(PROP_BYTE_ORDER_MARK, 1)], &mut out)?;
    c.hr(start_document(w, STANDALONE_YES), S_OK)?;
    c.hr(start_element(w, None, Some("r"), None), S_OK)?;
    c.hr(end_document(w), S_OK)?;
    c.hr(flush(w), S_OK)?;
    release(w);
    doc(
        c,
        "declaration",
        &out,
        b"<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><r />",
    )
}

/// Names a writer refuses, and the arguments it takes as nothing at all.
fn names(c: &mut Checks) -> Result<(), u32> {
    let mut out = Out::new();
    let w = writing(c, &[(PROP_OMIT_XML_DECLARATION, 1)], &mut out)?;
    c.hr(start_element(w, None, None, None), E_INVALIDARG)?;
    c.hr(start_element(w, None, Some(""), None), E_INVALIDARG)?;
    c.hr(start_element(w, None, Some("1bad"), None), WC_E_NAMECHARACTER)?;
    c.hr(start_element(w, None, Some("a b"), None), WC_E_NAMECHARACTER)?;
    // A prefix with no namespace to go with it.
    c.hr(start_element(w, Some("p"), Some("ok"), None), WR_E_NSPREFIXWITHEMPTYNSURI)?;
    // A namespace with no prefix is the default one.
    c.hr(start_element(w, None, Some("ok"), Some("urn:x")), S_OK)?;
    // Nothing to write is not an error.
    c.hr(string(w, None), S_OK)?;
    c.hr(comment(w, None), S_OK)?;
    // A comment keeps what would end it apart instead of refusing it.
    c.hr(comment(w, Some("no -- allowed?")), S_OK)?;
    // The one reserved processing instruction name, which kills it.
    c.hr(pi(w, Some("xml"), Some("bad")), WR_E_INVALIDACTION)?;
    c.hr(cdata(w, Some("]]>")), WR_E_INVALIDACTION)?;
    c.hr(flush(w), S_OK)?;
    release(w);
    doc(c, "names", &out, b"<ok xmlns=\"urn:x\"><!----><!--no - - allowed?-->")
}

/// Attributes and the namespaces they declare: a duplicate, the two
/// reserved prefixes, an undeclared prefix (which is dropped) and a
/// namespace with no prefix (which gets one invented, declared at the
/// end of the start tag).
fn attributes(c: &mut Checks) -> Result<(), u32> {
    let mut out = Out::new();
    let w = writing(c, &[(PROP_OMIT_XML_DECLARATION, 1)], &mut out)?;
    c.hr(start_element(w, None, Some("r"), None), S_OK)?;
    c.hr(attr(w, None, Some("a"), None, Some("1")), S_OK)?;
    c.hr(attr(w, None, Some("a"), None, Some("2")), WR_E_DUPLICATEATTRIBUTE)?;
    c.hr(attr(w, Some("xmlns"), Some("p"), None, Some("urn:p")), S_OK)?;
    c.hr(attr(w, Some("p"), Some("b"), None, Some("3")), S_OK)?;
    c.hr(attr(w, Some("q"), Some("c"), None, Some("4")), S_OK)?;
    c.hr(attr(w, None, Some("d"), Some("urn:d"), Some("5")), S_OK)?;
    c.hr(attr(w, None, Some("xmlns"), None, Some("urn:def")), WR_E_NSPREFIXDECLARED)?;
    c.hr(attr(w, Some("xml"), Some("space"), None, Some("preserve")), S_OK)?;
    c.hr(attr(w, Some("xml"), Some("space"), None, Some("nonsense")), WR_E_INVALIDXMLSPACE)?;
    c.hr(attr(w, None, Some("e"), None, None), S_OK)?;
    c.hr(end_document(w), S_OK)?;
    c.hr(flush(w), S_OK)?;
    release(w);
    doc(
        c,
        "attributes",
        &out,
        b"<r a=\"1\" xmlns:p=\"urn:p\" p:b=\"3\" c=\"4\" p2:d=\"5\" xml:space=\"preserve\" e=\"\" xmlns:p2=\"urn:d\" />",
    )
}

/// A qualified name wants its namespace declared, and the refusal does
/// not kill the writer.
fn qualified(c: &mut Checks) -> Result<(), u32> {
    let mut out = Out::new();
    let w = writing(c, &[(PROP_OMIT_XML_DECLARATION, 1)], &mut out)?;
    c.hr(start_element(w, Some("p"), Some("r"), Some("urn:p")), S_OK)?;
    c.hr(qualified_name(w, Some("name"), Some("urn:p")), S_OK)?;
    c.hr(qualified_name(w, Some("name"), Some("urn:nowhere")), WR_E_NAMESPACEUNDECLARED)?;
    c.hr(end_document(w), S_OK)?;
    c.hr(flush(w), S_OK)?;
    release(w);
    doc(c, "qualified", &out, b"<p:r xmlns:p=\"urn:p\">p:name</p:r>")
}

/// The conformance level: a document has one root and no text outside
/// it, a fragment has as many as it likes and no declaration of its own
/// -- though the declaration is still written when it was not omitted.
fn levels(c: &mut Checks) -> Result<(), u32> {
    let mut out = Out::new();
    let w = writing(c, &[(PROP_OMIT_XML_DECLARATION, 1)], &mut out)?;
    c.hr(start_element(w, None, Some("one"), None), S_OK)?;
    c.hr(end_element(w), S_OK)?;
    c.hr(start_element(w, None, Some("two"), None), WR_E_INVALIDACTION)?;
    c.hr(end_document(w), WR_E_INVALIDACTION)?;
    c.hr(flush(w), S_OK)?;
    release(w);
    doc(c, "document-level", &out, b"<one />")?;

    let mut out = Out::new();
    let w = writing(
        c,
        &[(PROP_OMIT_XML_DECLARATION, 1), (PROP_CONFORMANCE_LEVEL, LEVEL_FRAGMENT)],
        &mut out,
    )?;
    c.hr(string(w, Some("top text")), S_OK)?;
    c.hr(start_element(w, None, Some("one"), None), S_OK)?;
    c.hr(end_element(w), S_OK)?;
    c.hr(start_element(w, None, Some("two"), None), S_OK)?;
    // A fragment has no document to start or end.
    c.hr(end_document(w), WR_E_INVALIDACTION)?;
    c.hr(flush(w), S_OK)?;
    release(w);
    doc(c, "fragment-level", &out, b"top text<one /><two />")?;

    let mut out = Out::new();
    let w = writing(c, &[(PROP_CONFORMANCE_LEVEL, LEVEL_FRAGMENT)], &mut out)?;
    c.hr(start_document(w, STANDALONE_OMIT), WR_E_INVALIDACTION)?;
    c.hr(start_element(w, None, Some("r"), None), S_OK)?;
    c.hr(end_document(w), WR_E_INVALIDACTION)?;
    c.hr(flush(w), S_OK)?;
    release(w);
    doc(c, "fragment-declaration", &out, b"<?xml version=\"1.0\" encoding=\"UTF-8\"?><r />")?;

    // White space at the top of a document is allowed where text is not,
    // and neither refusal kills the writer: the next call is answered
    // for what it is.
    let mut out = Out::new();
    let w = writing(c, &[(PROP_OMIT_XML_DECLARATION, 1)], &mut out)?;
    c.hr(whitespace(w, Some("  \t")), S_OK)?;
    c.hr(string(w, Some("text at the top")), WR_E_INVALIDACTION)?;
    c.hr(whitespace(w, Some("x")), WR_E_NONWHITESPACE)?;
    c.hr(flush(w), S_OK)?;
    release(w);
    doc(c, "top-level", &out, b"  \t")
}

/// A doctype, and a declaration and a doctype where they cannot go.
/// Flush writes a start tag it has not finished.
fn doctype(c: &mut Checks) -> Result<(), u32> {
    let mut out = Out::new();
    let w = writing(c, &[], &mut out)?;
    c.hr(start_document(w, STANDALONE_NO), S_OK)?;
    c.hr(doc_type(w, Some("r"), Some("urn:pub"), Some("r.dtd"), None), S_OK)?;
    c.hr(start_element(w, None, Some("r"), None), S_OK)?;
    c.hr(start_document(w, STANDALONE_OMIT), WR_E_INVALIDACTION)?;
    c.hr(doc_type(w, Some("late"), None, None, None), WR_E_INVALIDACTION)?;
    c.hr(end_document(w), WR_E_INVALIDACTION)?;
    c.hr(flush(w), S_OK)?;
    release(w);
    doc(
        c,
        "doctype",
        &out,
        b"<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"no\"?><!DOCTYPE r PUBLIC \"urn:pub\" \"r.dtd\"><r",
    )
}

/// Characters and entities: a surrogate pair that is none, a character
/// that cannot be written at all, an entity nobody declared (which is
/// written anyway) and text past ASCII, which comes out as UTF-8.
fn entities(c: &mut Checks) -> Result<(), u32> {
    let mut out = Out::new();
    let w = writing(c, &[(PROP_OMIT_XML_DECLARATION, 1)], &mut out)?;
    c.hr(start_element(w, None, Some("r"), None), S_OK)?;
    c.hr(surrogate(w, 0x0041, 0x0042), WC_E_XMLCHARACTER)?;
    c.hr(char_entity(w, 0), WC_E_XMLCHARACTER)?;
    c.hr(char_entity(w, 0x0a), S_OK)?;
    c.hr(entity_ref(w, Some("unknown")), S_OK)?;
    c.hr(entity_ref(w, Some("1bad")), WC_E_NAMECHARACTER)?;
    c.hr(string(w, Some("\u{e9}\u{4e2d}")), S_OK)?;
    c.hr(end_document(w), S_OK)?;
    c.hr(flush(w), S_OK)?;
    release(w);
    doc(c, "entities", &out, b"<r>&#xA;&unknown;\xc3\xa9\xe4\xb8\xad</r>")
}

/// Which refusals leave the writer dead. A bad argument does not: the
/// call is the one that is wrong. A call that cannot go where the
/// document now is does, and every call after it is refused the same way
/// while Flush still answers S_OK.
fn poison(c: &mut Checks) -> Result<(), u32> {
    let mut out = Out::new();
    let w = writing(c, &[(PROP_OMIT_XML_DECLARATION, 1)], &mut out)?;
    c.hr(end_element(w), E_INVALIDARG)?;
    c.hr(start_element(w, None, Some("r"), None), S_OK)?;
    c.hr(end_document(w), S_OK)?;
    c.hr(flush(w), S_OK)?;
    release(w);
    doc(c, "after-bad-end", &out, b"<r />")?;

    let mut out = Out::new();
    let w = writing(c, &[(PROP_OMIT_XML_DECLARATION, 1)], &mut out)?;
    c.hr(start_element(w, None, Some("1bad"), None), WC_E_NAMECHARACTER)?;
    c.hr(start_element(w, None, Some("r"), None), S_OK)?;
    c.hr(end_document(w), S_OK)?;
    c.hr(flush(w), S_OK)?;
    release(w);
    doc(c, "after-bad-name", &out, b"<r />")?;

    let mut out = Out::new();
    let w = writing(c, &[(PROP_OMIT_XML_DECLARATION, 1)], &mut out)?;
    c.hr(attr(w, None, Some("a"), None, Some("1")), WR_E_INVALIDACTION)?;
    c.hr(start_element(w, None, Some("r"), None), WR_E_INVALIDACTION)?;
    c.hr(flush(w), S_OK)?;
    release(w);
    doc(c, "after-stray-attribute", &out, b"")?;

    let mut out = Out::new();
    let w = writing(c, &[(PROP_OMIT_XML_DECLARATION, 1)], &mut out)?;
    c.hr(start_element(w, None, Some("r"), None), S_OK)?;
    c.hr(pi(w, Some("xml"), Some("x")), WR_E_INVALIDACTION)?;
    c.hr(comment(w, Some("after")), WR_E_INVALIDACTION)?;
    c.hr(end_document(w), WR_E_INVALIDACTION)?;
    c.hr(flush(w), S_OK)?;
    release(w);
    // Flush writes what there is, which is a start tag with no end: a
    // dead writer keeps the document it had, unfinished.
    doc(c, "after-reserved-pi", &out, b"<r")
}

/// Either stream interface alone is enough, and a second SetOutput
/// starts the writer over.
fn outputs(c: &mut Checks) -> Result<(), u32> {
    let mut out = Out::new();
    out.allow_stream = false;
    let w = writing(c, &[(PROP_OMIT_XML_DECLARATION, 1)], &mut out)?;
    c.hr(start_element(w, None, Some("seq"), None), S_OK)?;
    c.hr(end_document(w), S_OK)?;
    c.hr(flush(w), S_OK)?;
    doc(c, "sequential-only", &out, b"<seq />")?;
    c.ok(out.used & used::SEQUENTIAL_IID != 0, u64::from(out.used))?;

    let mut out = Out::new();
    out.allow_sequential = false;
    c.hr(set_output(w, out.obj()), S_OK)?;
    c.hr(start_element(w, None, Some("istr"), None), S_OK)?;
    c.hr(end_document(w), S_OK)?;
    c.hr(flush(w), S_OK)?;
    doc(c, "stream-only", &out, b"<istr />")?;
    c.ok(out.used & used::STREAM_IID != 0, u64::from(out.used))?;

    // A writer that has finished a document writes another after a new
    // output, and the old stream's reference is given back.
    let mut again = Out::new();
    c.hr(set_output(w, again.obj()), S_OK)?;
    c.eq(out.refs as u64, 1)?;
    c.hr(start_element(w, None, Some("again"), None), S_OK)?;
    c.hr(end_document(w), S_OK)?;
    c.hr(flush(w), S_OK)?;
    doc(c, "second-output", &again, b"<again />")?;
    c.eq(u64::from(release(w)), 0)
}

/// Nothing reaches the stream until it is asked for, and Release asks:
/// it closes what is open where Flush would not.
fn release_flushes(c: &mut Checks) -> Result<(), u32> {
    let mut out = Out::new();
    let w = writing(c, &[(PROP_OMIT_XML_DECLARATION, 1)], &mut out)?;
    c.hr(start_element(w, None, Some("r"), None), S_OK)?;
    c.hr(string(w, Some("x")), S_OK)?;
    c.eq(out.len as u64, 0)?;
    c.eq(u64::from(release(w)), 0)?;
    doc(c, "release", &out, b"<r>x</r>")
}

/// A string longer than anything the writer can hold at once. How it
/// breaks the bytes up is its own business (the `doc` line says), so
/// only what came out is checked.
fn long(c: &mut Checks) -> Result<(), u32> {
    const N: usize = 4000;
    let mut x = [0u16; N + 1];
    for slot in x.iter_mut().take(N) {
        *slot = u16::from(b'x');
    }
    let mut out = Out::new();
    let w = writing(c, &[(PROP_OMIT_XML_DECLARATION, 1)], &mut out)?;
    c.hr(start_element(w, None, Some("r"), None), S_OK)?;
    c.hr(call!(w, WRITE_STRING, fn(*const u16) -> HResult, x.as_ptr()), S_OK)?;
    c.hr(end_document(w), S_OK)?;
    c.hr(flush(w), S_OK)?;
    release(w);
    Line::new()
        .s(NAME.as_bytes())
        .s(b": doc long ")
        .dec(out.len as u64)
        .s(b" bytes in ")
        .dec(u64::from(out.writes))
        .s(b" write(s)\r\n")
        .print();
    c.eq(out.len as u64, (N + 7) as u64)?;
    c.ok(&out.bytes()[..3] == b"<r>", 0)?;
    c.ok(&out.bytes()[N + 3..] == b"</r>", 1)?;
    c.ok(out.bytes()[3..N + 3].iter().all(|&b| b == b'x'), 2)
}

/// The places an implementation would otherwise have to guess: the
/// prefix a writer invents when nothing at all is declared, a comment
/// whose text ends in the character its own end begins with, a CDATA
/// section holding the one thing it cannot hold, an attribute that
/// comes after its element's content, and the two shapes of a doctype
/// the document above does not use.
fn more(c: &mut Checks) -> Result<(), u32> {
    let mut out = Out::new();
    let w = writing(c, &[(PROP_OMIT_XML_DECLARATION, 1)], &mut out)?;
    c.hr(start_element(w, None, Some("r"), None), S_OK)?;
    c.hr(attr(w, None, Some("a"), Some("urn:a"), Some("1")), S_OK)?;
    c.hr(end_document(w), S_OK)?;
    c.hr(flush(w), S_OK)?;
    release(w);
    doc(c, "invented-prefix", &out, b"<r p1:a=\"1\" xmlns:p1=\"urn:a\" />")?;

    let mut out = Out::new();
    let w = writing(c, &[(PROP_OMIT_XML_DECLARATION, 1)], &mut out)?;
    c.hr(comment(w, Some("ends in -")), S_OK)?;
    c.hr(start_element(w, None, Some("r"), None), S_OK)?;
    c.hr(cdata(w, Some("a ]]> inside")), S_OK)?;
    c.hr(string(w, Some("a\r\nb")), S_OK)?;
    c.hr(end_document(w), S_OK)?;
    c.hr(flush(w), S_OK)?;
    release(w);
    doc(
        c,
        "awkward-content",
        &out,
        b"<!--ends in - --><r><![CDATA[a ]]]]><![CDATA[> inside]]>a\r\nb</r>",
    )?;

    let mut out = Out::new();
    let w = writing(c, &[(PROP_OMIT_XML_DECLARATION, 1)], &mut out)?;
    c.hr(start_element(w, None, Some("r"), None), S_OK)?;
    c.hr(string(w, Some("x")), S_OK)?;
    c.hr(attr(w, None, Some("a"), None, Some("1")), WR_E_INVALIDACTION)?;
    c.hr(start_element(w, None, Some("late"), None), WR_E_INVALIDACTION)?;
    c.hr(flush(w), S_OK)?;
    release(w);
    doc(c, "late-attribute", &out, b"<r>x")?;

    let mut out = Out::new();
    let w = writing(c, &[(PROP_OMIT_XML_DECLARATION, 1)], &mut out)?;
    c.hr(doc_type(w, Some("r"), None, Some("r.dtd"), None), S_OK)?;
    c.hr(start_element(w, None, Some("r"), None), S_OK)?;
    c.hr(end_document(w), S_OK)?;
    c.hr(flush(w), S_OK)?;
    release(w);
    doc(c, "doctype-system", &out, b"<!DOCTYPE r SYSTEM \"r.dtd\"><r />")?;

    let mut out = Out::new();
    let w = writing(c, &[(PROP_OMIT_XML_DECLARATION, 1)], &mut out)?;
    c.hr(doc_type(w, Some("r"), None, None, Some("<!ENTITY x \"y\">")), S_OK)?;
    c.hr(start_element(w, None, Some("r"), None), S_OK)?;
    c.hr(end_document(w), S_OK)?;
    c.hr(flush(w), S_OK)?;
    release(w);
    doc(c, "doctype-subset", &out, b"<!DOCTYPE r [<!ENTITY x \"y\">]><r />")
}

/// Every case, in the order they are written above.
pub fn check(c: &mut Checks) -> Result<(), u32> {
    object(c)?;
    document(c)?;
    indent(c)?;
    escapes(c)?;
    declaration(c)?;
    names(c)?;
    attributes(c)?;
    qualified(c)?;
    levels(c)?;
    doctype(c)?;
    entities(c)?;
    poison(c)?;
    outputs(c)?;
    release_flushes(c)?;
    long(c)?;
    more(c)
}
