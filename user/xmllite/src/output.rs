//! `CreateXmlWriterOutputWithEncodingName`,
//! `CreateXmlWriterOutputWithEncodingCodePage` and the object they
//! make: a stream together with the encoding to write it in, which is
//! the only way a program asks XmlLite for anything but UTF-8.
//!
//! Word delay-loads `CreateXmlWriterOutputWithEncodingName` and ends
//! with 0xc06d007f without it (ROADMAP OF).
//!
//! Measured in `user/xmllite_test` (the `writer` module's six `output_`
//! cases), so what follows is the contract:
//!
//! - a creator validates almost nothing. A NULL stream or nowhere to
//!   put the answer is E_INVALIDARG, with the answer left exactly as it
//!   was and the stream not asked one question and not referenced;
//!   an encoding nothing knows, and an object that is no stream, are
//!   both S_OK, and what they cost comes later;
//! - creating takes a reference to the stream's IUnknown and asks it
//!   nothing: what the stream is, SetOutput finds out;
//! - the object answers IUnknown with itself, and an IID of its own
//!   with something else; everything else is E_NOINTERFACE, and a
//!   refused question costs nothing;
//! - an output is not one writer's own. Two writers may hold the same
//!   one, and both append to its stream;
//! - what the declaration says is the name a name output was given,
//!   verbatim, case and all, while a code page is declared by XmlLite's
//!   own name for it;
//! - the byte order mark belongs to SetOutput rather than to the
//!   output, and only a wide encoding has one to write.
//!
//! The 22 code pages and 25 names XmlLite takes are the ones here;
//! `codepages` holds the single-byte tables, and an encoding outside
//! them leaves a writer that answers MX_E_ENCODING for every write
//! (`writer`).

use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, Ordering};

use crate::codepages::{
    CP_1250, CP_1251, CP_1252, CP_1253, CP_1254, CP_1255, CP_1256, CP_1257, CP_1258, CP_28591,
    CP_28592, CP_28593, CP_28594, CP_28595, CP_28596, CP_28597, CP_28598, CP_28599,
};
use crate::{
    E_INVALIDARG, E_NOINTERFACE, E_OUTOFMEMORY, Guid, HResult, IID_IUNKNOWN, LEDGER_REACHED,
    LEDGER_UNSUPPORTED, Obj, S_OK, VTable, data1, free_box, guid, put, record, record_head, text,
    try_box,
};

/// `IXmlWriterOutput`, which is in no public header: the writer is what
/// names it, and an output is the only object that answers it. What
/// Windows answers it with is not even an IUnknown (see [`Output`]).
const IID_IXML_WRITER_OUTPUT: Guid =
    guid(0xc113_1708, 0x0f59, 0x477f, [0x93, 0x59, 0x7d, 0x33, 0x24, 0x51, 0xbc, 0x1a]);

// ---------------------------------------------------------------------
// The encodings.
// ---------------------------------------------------------------------

/// What a writer's characters become on their way to the stream.
#[derive(Clone, Copy)]
pub enum Encoding {
    /// UTF-8, which is also what a writer with a plain stream writes.
    Utf8,
    /// UTF-16, little-endian, which is the only order XmlLite writes.
    Utf16,
    /// UCS-4, little-endian.
    Ucs4,
    /// The bytes below 0x80 and nothing else (us-ascii).
    Ascii,
    /// One byte a character, by the table of what each byte from 0x80
    /// up stands for.
    Single(&'static [u16; 128]),
    /// An encoding nothing here knows. The creator still makes an
    /// output of it, and every write through it is MX_E_ENCODING
    /// (measured).
    Unknown,
}

/// The byte a single-byte encoding writes a character as, if it has
/// one. 0xffff marks a byte the encoding does not use, so U+FFFF --
/// which no document may hold anyway -- is not one of its characters.
fn single(table: &[u16; 128], c: char) -> Option<u8> {
    let code = c as u32;
    if code < 0x80 {
        return Some(code as u8);
    }
    if code > 0xfffe {
        return None;
    }
    let want = code as u16;
    let i = table.iter().position(|&u| u == want)?;
    Some(0x80 + i as u8)
}

impl Encoding {
    /// Whether this encoding has a byte for the character. One it has
    /// not becomes a character reference where a reference is allowed,
    /// and ends the encoder where one is not (measured; `writer`).
    pub fn has(self, c: char) -> bool {
        match self {
            Encoding::Utf8 | Encoding::Utf16 | Encoding::Ucs4 => true,
            Encoding::Ascii => (c as u32) < 0x80,
            Encoding::Single(table) => single(table, c).is_some(),
            Encoding::Unknown => false,
        }
    }

    /// Appends the character's bytes; false when the buffer would not
    /// grow, or when the encoding has no byte for it (which [`has`]
    /// asks first).
    ///
    /// [`has`]: Encoding::has
    pub fn push(self, c: char, into: &mut Vec<u8>) -> bool {
        let mut bytes = [0u8; 4];
        let n = match self {
            Encoding::Utf8 => c.encode_utf8(&mut bytes).len(),
            Encoding::Utf16 => {
                let mut units = [0u16; 2];
                let units = c.encode_utf16(&mut units);
                for (i, u) in units.iter().enumerate() {
                    bytes[i * 2] = *u as u8;
                    bytes[i * 2 + 1] = (*u >> 8) as u8;
                }
                units.len() * 2
            }
            Encoding::Ucs4 => {
                bytes = (c as u32).to_le_bytes();
                4
            }
            Encoding::Ascii | Encoding::Single(_) => {
                let table: &[u16; 128] = match self {
                    Encoding::Single(table) => table,
                    _ => &ASCII_ONLY,
                };
                match single(table, c) {
                    Some(b) => {
                        bytes[0] = b;
                        1
                    }
                    None => return false,
                }
            }
            Encoding::Unknown => return false,
        };
        if into.try_reserve(n).is_err() {
            return false;
        }
        into.extend_from_slice(&bytes[..n]);
        true
    }

    /// The bytes that lead the stream when the ByteOrderMark property
    /// is on: only a wide encoding has any (measured).
    pub fn mark(self) -> &'static [u8] {
        match self {
            Encoding::Utf16 => &[0xff, 0xfe],
            Encoding::Ucs4 => &[0xff, 0xfe, 0x00, 0x00],
            _ => &[],
        }
    }

    /// Whether this is an encoding gala can write at all.
    pub fn known(self) -> bool {
        !matches!(self, Encoding::Unknown)
    }
}

/// us-ascii has no byte above 0x7f, which is a table of holes.
static ASCII_ONLY: [u16; 128] = [0xffff; 128];

/// The encoding's name as the declaration says it. The longest XmlLite
/// takes is `unicode-1-1-utf-8`, so a name needs no heap -- and a name
/// it does not take is never declared, only recorded.
#[derive(Clone, Copy)]
pub struct Says {
    bytes: [u8; 24],
    len: u8,
}

impl Says {
    /// The name, cut to what a declaration can need.
    pub const fn new(name: &str) -> Says {
        let mut bytes = [0u8; 24];
        let from = name.as_bytes();
        let mut n = 0;
        while n < from.len() && n < 24 {
            bytes[n] = from[n];
            n += 1;
        }
        Says { bytes, len: n as u8 }
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.len as usize]
    }

    /// The name for a ledger line; `?` for one that was cut through a
    /// character.
    pub fn as_str(&self) -> &str {
        core::str::from_utf8(self.as_bytes()).unwrap_or("?")
    }
}

/// The 25 encoding names XmlLite's writer takes, case disregarded, and
/// what each one is. Every other name is [`Encoding::Unknown`]: there
/// are no further aliases, measured one by one (`utf8`, `cp1252`,
/// `latin1`, `iso-8859-15`, `utf-16le`, `utf-32`, `ascii` and ` utf-8`
/// among them are all refused).
const NAMES: [(&str, Encoding); 25] = [
    ("utf-8", Encoding::Utf8),
    ("unicode-1-1-utf-8", Encoding::Utf8),
    ("utf-16", Encoding::Utf16),
    ("ucs-2", Encoding::Utf16),
    ("iso-10646-ucs-2", Encoding::Utf16),
    ("ucs-4", Encoding::Ucs4),
    ("us-ascii", Encoding::Ascii),
    ("windows-1250", Encoding::Single(&CP_1250)),
    ("windows-1251", Encoding::Single(&CP_1251)),
    ("windows-1252", Encoding::Single(&CP_1252)),
    ("windows-1253", Encoding::Single(&CP_1253)),
    ("windows-1254", Encoding::Single(&CP_1254)),
    ("windows-1255", Encoding::Single(&CP_1255)),
    ("windows-1256", Encoding::Single(&CP_1256)),
    ("windows-1257", Encoding::Single(&CP_1257)),
    ("windows-1258", Encoding::Single(&CP_1258)),
    ("iso-8859-1", Encoding::Single(&CP_28591)),
    ("iso-8859-2", Encoding::Single(&CP_28592)),
    ("iso-8859-3", Encoding::Single(&CP_28593)),
    ("iso-8859-4", Encoding::Single(&CP_28594)),
    ("iso-8859-5", Encoding::Single(&CP_28595)),
    ("iso-8859-6", Encoding::Single(&CP_28596)),
    ("iso-8859-7", Encoding::Single(&CP_28597)),
    ("iso-8859-8", Encoding::Single(&CP_28598)),
    ("iso-8859-9", Encoding::Single(&CP_28599)),
];

/// Code pages 1250 to 1258, with the name each is declared by.
const WINDOWS_PAGES: [(&[u16; 128], &str); 9] = [
    (&CP_1250, "WINDOWS-1250"),
    (&CP_1251, "WINDOWS-1251"),
    (&CP_1252, "WINDOWS-1252"),
    (&CP_1253, "WINDOWS-1253"),
    (&CP_1254, "WINDOWS-1254"),
    (&CP_1255, "WINDOWS-1255"),
    (&CP_1256, "WINDOWS-1256"),
    (&CP_1257, "WINDOWS-1257"),
    (&CP_1258, "WINDOWS-1258"),
];

/// Code pages 28591 to 28599, with the name each is declared by.
const ISO_PAGES: [(&[u16; 128], &str); 9] = [
    (&CP_28591, "iso-8859-1"),
    (&CP_28592, "iso-8859-2"),
    (&CP_28593, "iso-8859-3"),
    (&CP_28594, "iso-8859-4"),
    (&CP_28595, "iso-8859-5"),
    (&CP_28596, "iso-8859-6"),
    (&CP_28597, "iso-8859-7"),
    (&CP_28598, "iso-8859-8"),
    (&CP_28599, "iso-8859-9"),
];

/// The encoding a name stands for. The name itself is what the
/// declaration will say, so it is kept as it was given.
fn by_name(name: &str) -> Encoding {
    for (known, encoding) in NAMES.iter() {
        if known.eq_ignore_ascii_case(name) {
            return *encoding;
        }
    }
    Encoding::Unknown
}

/// The encoding and the name a code page is declared by: a page has no
/// name of its own to echo, so XmlLite's own stands in (measured, for
/// every one of the 22 pages it takes).
fn by_page(page: u32) -> (Encoding, Says) {
    match page {
        65001 => (Encoding::Utf8, Says::new("UTF-8")),
        1200 => (Encoding::Utf16, Says::new("UTF-16")),
        12000 => (Encoding::Ucs4, Says::new("UCS-4")),
        20127 => (Encoding::Ascii, Says::new("us-ascii")),
        1250..=1258 => {
            let (table, says) = WINDOWS_PAGES[(page - 1250) as usize];
            (Encoding::Single(table), Says::new(says))
        }
        28591..=28599 => {
            let (table, says) = ISO_PAGES[(page - 28591) as usize];
            (Encoding::Single(table), Says::new(says))
        }
        _ => (Encoding::Unknown, Says::new("")),
    }
}

// ---------------------------------------------------------------------
// The object.
// ---------------------------------------------------------------------

/// An `IXmlWriterOutput`: the stream, with the encoding and the name to
/// declare it by.
#[repr(C)]
struct Output {
    /// IUnknown, which is all the object answers as itself.
    vtable: *const (),
    /// The second vtable, at an address of its own, which is what the
    /// output IID is answered with. Windows answers it with a pointer
    /// that is no IUnknown at all -- its third slot returns a number
    /// that is no count and gives nothing back, so a caller can never
    /// let go of what it asked for and no program can have been
    /// written against it (measured). gala hands out this field's
    /// address instead and counts it properly.
    tear: *const (),
    refs: AtomicU32,
    /// The stream, holding the reference the creator took.
    stream: Obj,
    encoding: Encoding,
    says: Says,
}

/// The output a call was made on.
#[allow(clippy::mut_from_ref)]
fn obj(this: Obj) -> &'static mut Output {
    // SAFETY: the object is one of ours, alive while the caller holds a
    // reference to it.
    unsafe { &mut *(this as *mut Output) }
}

/// The output a tear-off pointer belongs to: the field's address, less
/// where the field sits.
fn outer(tear: Obj) -> Obj {
    (tear as usize - core::mem::offset_of!(Output, tear)) as Obj
}

/// Takes a reference to the stream, which is all the creator asks of
/// it: no QueryInterface (measured).
fn add_ref_stream(stream: Obj) {
    // SAFETY: slot 1 of a COM object's vtable is AddRef, and the
    // caller gave us the object.
    unsafe {
        let vtable = *(stream as *const *const usize);
        let f: extern "system" fn(Obj) -> u32 = core::mem::transmute(*vtable.add(1));
        f(stream)
    };
}

/// Gives the stream back when the last reference to the output goes.
fn release_stream(stream: Obj) {
    // SAFETY: slot 2 is Release, and the reference is the one the
    // creator took.
    unsafe {
        let vtable = *(stream as *const *const usize);
        let f: extern "system" fn(Obj) -> u32 = core::mem::transmute(*vtable.add(2));
        f(stream)
    };
}

extern "system" fn query_interface(this: Obj, iid: *const Guid, out: *mut Obj) -> HResult {
    if iid.is_null() || out.is_null() {
        return E_INVALIDARG;
    }
    put(out, core::ptr::null_mut());
    // SAFETY: the caller's IID, which it says it has.
    let iid = unsafe { *iid };
    let o = obj(this);
    if iid == IID_IUNKNOWN {
        o.refs.fetch_add(1, Ordering::Relaxed);
        put(out, this);
        return S_OK;
    }
    if iid == IID_IXML_WRITER_OUTPUT {
        o.refs.fetch_add(1, Ordering::Relaxed);
        put(out, (&raw const o.tear) as Obj);
        return S_OK;
    }
    record(b"IXmlWriterOutput::QueryInterface(iid)", LEDGER_UNSUPPORTED, data1(&iid));
    E_NOINTERFACE
}

extern "system" fn add_ref(this: Obj) -> u32 {
    obj(this).refs.fetch_add(1, Ordering::Relaxed) + 1
}

extern "system" fn release(this: Obj) -> u32 {
    let o = obj(this);
    let left = o.refs.fetch_sub(1, Ordering::Release) - 1;
    if left == 0 {
        release_stream(o.stream);
        // SAFETY: the last reference is gone, and the box is the one
        // the creator made.
        unsafe { free_box(this as *mut Output) };
    }
    left
}

/// The tear-off's three slots: the same object, reached at another
/// address, so each one goes to the object itself.
extern "system" fn tear_query_interface(this: Obj, iid: *const Guid, out: *mut Obj) -> HResult {
    query_interface(outer(this), iid, out)
}

extern "system" fn tear_add_ref(this: Obj) -> u32 {
    add_ref(outer(this))
}

extern "system" fn tear_release(this: Obj) -> u32 {
    release(outer(this))
}

static OUTPUT_VTABLE: VTable<3> =
    VTable([query_interface as *const (), add_ref as *const (), release as *const ()]);

static OUTPUT_TEAR_VTABLE: VTable<3> = VTable([
    tear_query_interface as *const (),
    tear_add_ref as *const (),
    tear_release as *const (),
]);

// ---------------------------------------------------------------------
// The creators.
// ---------------------------------------------------------------------

/// What both creators do once the encoding is settled. `detail` is the
/// code page, or 0 for a name.
fn create(
    stream: Obj,
    malloc: Obj,
    encoding: Encoding,
    says: Says,
    detail: u64,
    out: *mut Obj,
) -> HResult {
    // Neither creator touches the answer it cannot give, or the stream
    // it would have nowhere to hand back (measured: the out value is
    // left exactly as it was, and the stream is asked nothing and
    // referenced not at all).
    if stream.is_null() || out.is_null() {
        return E_INVALIDARG;
    }
    if !malloc.is_null() {
        // The allocator a program offers, which gala's own heap stands
        // in for.
        record(b"CreateXmlWriterOutput(IMalloc)", LEDGER_UNSUPPORTED, 0);
    }
    if !encoding.known() {
        // Not a refusal -- XmlLite makes an output of an encoding it
        // does not know either -- but every write through it will be
        // MX_E_ENCODING, so the ledger says which encoding was asked
        // for before the program gets there.
        record_head("CreateXmlWriterOutput(encoding)", says.as_str(), LEDGER_UNSUPPORTED, detail);
    }
    let Some(o) = try_box(Output {
        vtable: &OUTPUT_VTABLE as *const VTable<3> as *const (),
        tear: &OUTPUT_TEAR_VTABLE as *const VTable<3> as *const (),
        refs: AtomicU32::new(1),
        stream,
        encoding,
        says,
    }) else {
        return E_OUTOFMEMORY;
    };
    add_ref_stream(stream);
    put(out, o as Obj);
    // Not a gap: what encoding a program asked a document to be written
    // in, which a run that ends elsewhere still says.
    record_head("CreateXmlWriterOutput", says.as_str(), LEDGER_REACHED, detail);
    S_OK
}

/// `CreateXmlWriterOutputWithEncodingName(stream, malloc, encoding,
/// output)` (xmllite.h): the stream to write to and the name the
/// declaration will say, which is kept as it was given.
#[unsafe(no_mangle)]
pub extern "system" fn CreateXmlWriterOutputWithEncodingName(
    stream: Obj,
    malloc: Obj,
    encoding: *const u16,
    out: *mut Obj,
) -> HResult {
    let name = text(encoding);
    let name = name.as_deref().unwrap_or("");
    create(stream, malloc, by_name(name), Says::new(name), 0, out)
}

/// `CreateXmlWriterOutputWithEncodingCodePage(stream, malloc, codepage,
/// output)` (xmllite.h): the stream to write to and one of the 22 code
/// pages XmlLite writes, each with a name of XmlLite's own.
#[unsafe(no_mangle)]
pub extern "system" fn CreateXmlWriterOutputWithEncodingCodePage(
    stream: Obj,
    malloc: Obj,
    codepage: u32,
    out: *mut Obj,
) -> HResult {
    let (encoding, says) = by_page(codepage);
    create(stream, malloc, encoding, says, u64::from(codepage), out)
}

// ---------------------------------------------------------------------
// What the writer asks of an output.
// ---------------------------------------------------------------------

/// The stream, the encoding and the name behind an output: the first
/// thing SetOutput asks any object it is given, before it asks for a
/// stream at all (measured -- a raw stream is asked the output IID
/// first and its refusal is the answer).
///
/// `None` for anything that is not an output of gala's own making, which
/// the writer then asks to be a stream itself.
pub fn taken(output: Obj) -> Option<(Obj, Encoding, Says)> {
    let mut q: Obj = core::ptr::null_mut();
    // SAFETY: slot 0 is QueryInterface, and the IID and the answer are
    // ours.
    let hr = unsafe {
        let vtable = *(output as *const *const usize);
        let f: extern "system" fn(Obj, *const Guid, *mut Obj) -> HResult =
            core::mem::transmute(*vtable);
        f(output, &IID_IXML_WRITER_OUTPUT, &mut q)
    };
    if hr < 0 || q.is_null() {
        return None;
    }
    // SAFETY: whatever answered, what it handed back begins with a
    // vtable pointer, which is what COM means.
    let vtable = unsafe { *(q as *const *const ()) };
    if vtable != &OUTPUT_TEAR_VTABLE as *const VTable<3> as *const () {
        // Somebody else's IXmlWriterOutput. What it handed back cannot
        // be given up -- Windows' own answer there is no IUnknown (see
        // [`Output`]) -- so the reference stays where it is and the
        // writer goes on to ask this object for a stream.
        record(b"IXmlWriter::SetOutput(another output)", LEDGER_UNSUPPORTED, 0);
        return None;
    }
    let o = obj(outer(q));
    let got = (o.stream, o.encoding, o.says);
    release(outer(q));
    Some(got)
}
