//! The reader's properties: every one of them, what it defaults to,
//! what it refuses, when it stops taking values, and what the values it
//! keeps go on to do. Word sets `XmlReaderProperty_MultiLanguage` on a
//! reader before it reads its first part, and a reader that refuses it
//! ends the process (ROADMAP OF), so the whole enumeration is measured
//! here rather than the one property that stopped it.
//!
//! Measured on real Windows, all of it:
//! - the defaults: MultiLanguage 0, ConformanceLevel Document,
//!   RandomAccess 0, XmlResolver 0, DtdProcessing Prohibit, ReadState
//!   Closed, MaxElementDepth 256 and MaxEntityExpansion 100000, while
//!   the two past the end of the enumeration are E_NOTIMPL both ways;
//!   nowhere to put the answer is E_INVALIDARG, and the pointer is
//!   checked before the property is (a property that is E_NOTIMPL with
//!   somewhere to put it is E_INVALIDARG without);
//! - what each one takes: a conformance level and a DTD setting are
//!   refused with E_INVALIDARG outside their enumerations and the old
//!   value stays, ReadState is read-only (E_ACCESSDENIED), and the two
//!   limits take any value and read back as set;
//! - the two properties that are objects: the reader AddRefs what it is
//!   given and never asks it for an interface, GetProperty hands the
//!   pointer back with another reference, setting the property to NULL
//!   gives the reader's reference back, and so does releasing the
//!   reader -- which is what makes it safe for a program to let go of
//!   its own reference the moment it has set the property;
//! - when they lock: not at SetInput, which leaves the reader Initial
//!   with every property still free, but at the first Read, after which
//!   every SetProperty is E_ACCESSDENIED (the object properties
//!   included, and a refused set keeps no reference) while GetProperty
//!   still answers. A reader in the Error read state takes them again,
//!   and so does one whose input was taken away with SetInput(NULL),
//!   which leaves it Initial and not Closed;
//! - what the kept values do: MaxElementDepth 2 lets two elements
//!   through and answers SC_E_MAXELEMENTDEPTH at the third, leaving the
//!   reader in the Error state, while 0 is no limit at all;
//!   ConformanceLevel Auto reads exactly as Fragment does, both of them
//!   taking documents that Document refuses (those are in the `BADS`
//!   table of the reader's own cases); and DtdProcessing Prohibit
//!   answers WC_E_DTDPROHIBITED at the first Read of a document with an
//!   internal subset.
//!
//! Two measured behaviours are deliberately not asserted here, because
//! no implementation can match them:
//! - a Read on a reader that never had an input, or whose input was
//!   taken away with SetInput(NULL), faults inside real xmllite. gala's
//!   reader answers S_FALSE, and there is no Windows answer to hold it
//!   to.
//! - MaxEntityExpansion is not enforced by Windows at all: a limit of 4
//!   against two entities worth six characters each expands both and
//!   reports nothing. gala keeps the value and does the same.

use core::ptr::null_mut;

use crate::NODE_WHITESPACE;
use crate::{Checks, Guid, HResult, Line, Obj, Stream};
use crate::{CreateXmlReader, GET_DEPTH, GET_PROPERTY, GET_VALUE, READ, RELEASE};
use crate::{E_NOINTERFACE, E_NOTIMPL, NAME, NODE_ELEMENT, NODE_NONE, NODE_TEXT};
use crate::{IID_IUNKNOWN, IID_IXML_READER, WC_E_DTDPROHIBITED};
use crate::{PROP_CONFORMANCE_LEVEL, PROP_DTD_PROCESSING, PROP_READ_STATE};
use crate::{READ_STATE_CLOSED, READ_STATE_END_OF_FILE, READ_STATE_ERROR};
use crate::{READ_STATE_INITIAL, READ_STATE_INTERACTIVE};
use crate::{S_FALSE, S_OK};
use crate::{SET_INPUT, SET_PROPERTY};
use crate::{property, u32_at};

const E_INVALIDARG: HResult = 0x8007_0057u32 as HResult;
const E_ACCESSDENIED: HResult = 0x8007_0005u32 as HResult;
/// What the Read that would go past MaxElementDepth answers.
const SC_E_MAXELEMENTDEPTH: HResult = 0xc00c_ee81u32 as HResult;
/// What a byte that is not the encoding the document is taken for is to
/// XmlLite: a character XML does not allow, not an encoding it cannot
/// read.
const WC_E_XMLCHARACTER: HResult = 0xc00c_ee2bu32 as HResult;

// XmlReaderProperty, the whole enumeration (xmllite.h); the three the
// reader's own cases use are in the crate root.
const PROP_MULTI_LANGUAGE: u32 = 0;
const PROP_RANDOM_ACCESS: u32 = 2;
const PROP_XML_RESOLVER: u32 = 3;
const PROP_MAX_ELEMENT_DEPTH: u32 = 6;
const PROP_MAX_ENTITY_EXPANSION: u32 = 7;
/// The two past the end of the enumeration, which answer E_NOTIMPL.
const PROP_PAST_THE_END: u32 = 8;
const PROP_WELL_PAST_THE_END: u32 = 9;

// XmlConformanceLevel and DtdProcessing.
const LEVEL_AUTO: isize = 0;
const LEVEL_FRAGMENT: isize = 1;
const LEVEL_DOCUMENT: isize = 2;
const DTD_PROHIBIT: isize = 0;
const DTD_PARSE: isize = 1;

/// Four elements deep, for the depth limit.
const DEEP: &[u8] = b"<a><b><c><d>x</d></c></b></a>";
/// Two roots, and text before the root: a fragment either way, which
/// Document refuses and Auto and Fragment take.
const TWO_ROOTS: &[u8] = b"<a/><b/>";
/// A document and the nodes a reader is to read it as, each the type
/// of a node and the value it holds.
type Nodes<'a> = (&'a [u8], &'a [(u32, &'a str)]);
/// Character data outside the root elements, which a fragment allows
/// and a document does not, and the nodes a fragment reads it as: the
/// type and the value of each (measured).
const LOOSE: &[Nodes] = &[
    (b"text<a/>", &[(NODE_TEXT, "text"), (NODE_ELEMENT, "")]),
    (b" text<a/>", &[(NODE_TEXT, " text"), (NODE_ELEMENT, "")]),
    (b"<a/> x<b/>", &[(NODE_ELEMENT, ""), (NODE_TEXT, " x"), (NODE_ELEMENT, "")]),
    (b"<a/>\n<b/>", &[(NODE_ELEMENT, ""), (NODE_WHITESPACE, "\n"), (NODE_ELEMENT, "")]),
    (b"a&amp;b<a/>", &[(NODE_TEXT, "a&b"), (NODE_ELEMENT, "")]),
];
/// A document type declaration with an internal subset.
const SUBSET: &[u8] = b"<!DOCTYPE a [<!ENTITY e \"xy\">]><a>&e;</a>";

/// An object with nothing but IUnknown, which counts its references and
/// records being asked for an interface: what a program hands the
/// MultiLanguage and XmlResolver properties. Windows' reader AddRefs it
/// and never calls QueryInterface on it (the interface it assumes is
/// measured in the mlang cases), so `queried` must stay 0.
#[repr(C)]
pub(crate) struct Spy {
    vtable: *const SpyVtable,
    pub(crate) refs: i32,
    pub(crate) queried: u32,
}

#[repr(C)]
struct SpyVtable {
    query: extern "system" fn(Obj, *const Guid, *mut Obj) -> HResult,
    add_ref: extern "system" fn(Obj) -> u32,
    release: extern "system" fn(Obj) -> u32,
}

extern "system" fn spy_query(this: Obj, iid: *const Guid, out: *mut Obj) -> HResult {
    // SAFETY: a live Spy, and an IID and an out pointer from the caller.
    unsafe {
        let s = &mut *(this as *mut Spy);
        s.queried += 1;
        if *iid != IID_IUNKNOWN {
            *out = null_mut();
            return E_NOINTERFACE;
        }
        s.refs += 1;
        *out = this;
    }
    S_OK
}

extern "system" fn spy_add_ref(this: Obj) -> u32 {
    // SAFETY: a live Spy.
    let s = unsafe { &mut *(this as *mut Spy) };
    s.refs += 1;
    s.refs as u32
}

extern "system" fn spy_release(this: Obj) -> u32 {
    // SAFETY: a live Spy, which outlives every reference to it.
    let s = unsafe { &mut *(this as *mut Spy) };
    s.refs -= 1;
    s.refs as u32
}

static SPY_VTABLE: SpyVtable =
    SpyVtable { query: spy_query, add_ref: spy_add_ref, release: spy_release };

impl Spy {
    pub(crate) fn new() -> Self {
        Self { vtable: &SPY_VTABLE, refs: 1, queried: 0 }
    }

    pub(crate) fn obj(&mut self) -> Obj {
        self as *mut Spy as Obj
    }
}

fn reader(c: &mut Checks) -> Result<Obj, u32> {
    let mut r: Obj = null_mut();
    // SAFETY: a plain call; the IID and the out pointer are ours.
    c.hr(unsafe { CreateXmlReader(&IID_IXML_READER, &mut r, null_mut()) }, S_OK)?;
    c.ok(!r.is_null(), 0)?;
    Ok(r)
}

fn set(r: Obj, which: u32, value: isize) -> HResult {
    call!(r, SET_PROPERTY, fn(u32, isize) -> HResult, which, value)
}

fn read(r: Obj) -> (HResult, u32) {
    let mut ty = 0xdead_beefu32;
    let hr = call!(r, READ, fn(*mut u32) -> HResult, &mut ty);
    (hr, ty)
}

fn input(r: Obj, stream: &mut Stream) -> HResult {
    call!(r, SET_INPUT, fn(Obj) -> HResult, stream.obj())
}

fn no_input(r: Obj) -> HResult {
    call!(r, SET_INPUT, fn(Obj) -> HResult, null_mut())
}

fn release(obj: Obj) -> u32 {
    call!(obj, RELEASE, fn() -> u32,)
}

/// The read state, which every case here looks at.
fn state(c: &mut Checks, r: Obj, want: isize) -> Result<(), u32> {
    let (hr, got) = property(r, PROP_READ_STATE);
    c.hr(hr, S_OK)?;
    c.eq(got as u64, want as u64)
}

/// A property and the value it answered, for the transcript: a reader
/// that answers one of them differently is read here and not guessed at.
fn line(which: u32, hr: HResult, v: isize) {
    Line::new()
        .s(NAME.as_bytes())
        .s(b": prop ")
        .dec(u64::from(which))
        .s(b" hr=")
        .hex(hr as u32 as u64)
        .s(b" value=")
        .hex(v as u64)
        .s(b"\r\n")
        .print();
}

fn get(c: &mut Checks, r: Obj, which: u32, want: isize) -> Result<(), u32> {
    let (hr, v) = property(r, which);
    line(which, hr, v);
    c.hr(hr, S_OK)?;
    c.eq(v as u64, want as u64)
}

pub fn check(c: &mut Checks) -> Result<(), u32> {
    defaults(c)?;
    taken(c)?;
    objects(c)?;
    locking(c)?;
    limits(c)?;
    fragments(c)?;
    dtd(c)?;
    past_the_end(c)?;
    undecodable(c)
}

/// The defaults, the two past the end of the enumeration, and nowhere to
/// put the answer.
fn defaults(c: &mut Checks) -> Result<(), u32> {
    let r = reader(c)?;
    for (which, want) in [
        (PROP_MULTI_LANGUAGE, 0),
        (PROP_CONFORMANCE_LEVEL, LEVEL_DOCUMENT),
        (PROP_RANDOM_ACCESS, 0),
        (PROP_XML_RESOLVER, 0),
        (PROP_DTD_PROCESSING, DTD_PROHIBIT),
        (PROP_READ_STATE, READ_STATE_CLOSED),
        (PROP_MAX_ELEMENT_DEPTH, 256),
        (PROP_MAX_ENTITY_EXPANSION, 100_000),
    ] {
        get(c, r, which, want)?;
    }
    // Past the end of the enumeration: E_NOTIMPL both ways.
    for which in [PROP_PAST_THE_END, PROP_WELL_PAST_THE_END] {
        let (hr, v) = property(r, which);
        line(which, hr, v);
        c.hr(hr, E_NOTIMPL)?;
        c.hr(set(r, which, 1), E_NOTIMPL)?;
    }
    // Nowhere to put the answer, for a property that is answered and for
    // one that is not: the pointer is checked first, so both are
    // E_INVALIDARG.
    for which in [PROP_CONFORMANCE_LEVEL, PROP_PAST_THE_END] {
        c.hr(
            call!(r, GET_PROPERTY, fn(u32, *mut isize) -> HResult, which, null_mut::<isize>()),
            E_INVALIDARG,
        )?;
    }
    c.eq(u64::from(release(r)), 0)
}

/// What each property takes, and what it refuses.
fn taken(c: &mut Checks) -> Result<(), u32> {
    let r = reader(c)?;
    // A conformance level is read back as set, Auto included.
    for level in [LEVEL_AUTO, LEVEL_FRAGMENT, LEVEL_DOCUMENT] {
        c.hr(set(r, PROP_CONFORMANCE_LEVEL, level), S_OK)?;
        get(c, r, PROP_CONFORMANCE_LEVEL, level)?;
    }
    // Outside the enumeration it is refused and the old value stays.
    for level in [-1, 3] {
        c.hr(set(r, PROP_CONFORMANCE_LEVEL, level), E_INVALIDARG)?;
        get(c, r, PROP_CONFORMANCE_LEVEL, LEVEL_DOCUMENT)?;
    }
    // DtdProcessing has two values and no more.
    c.hr(set(r, PROP_DTD_PROCESSING, DTD_PARSE), S_OK)?;
    get(c, r, PROP_DTD_PROCESSING, DTD_PARSE)?;
    c.hr(set(r, PROP_DTD_PROCESSING, 2), E_INVALIDARG)?;
    get(c, r, PROP_DTD_PROCESSING, DTD_PARSE)?;
    c.hr(set(r, PROP_DTD_PROCESSING, DTD_PROHIBIT), S_OK)?;
    // RandomAccess takes either way round.
    for v in [1, 0] {
        c.hr(set(r, PROP_RANDOM_ACCESS, v), S_OK)?;
    }
    // ReadState is the reader's to say.
    c.hr(set(r, PROP_READ_STATE, READ_STATE_INITIAL), E_ACCESSDENIED)?;
    get(c, r, PROP_READ_STATE, READ_STATE_CLOSED)?;
    // The two limits take any value and read back as set.
    for (which, v) in [(PROP_MAX_ELEMENT_DEPTH, 9), (PROP_MAX_ENTITY_EXPANSION, 4)] {
        c.hr(set(r, which, v), S_OK)?;
        get(c, r, which, v)?;
    }
    c.eq(u64::from(release(r)), 0)
}

/// The two properties that are objects, and whose reference is whose.
fn objects(c: &mut Checks) -> Result<(), u32> {
    for which in [PROP_MULTI_LANGUAGE, PROP_XML_RESOLVER] {
        let mut spy = Spy::new();
        let r = reader(c)?;
        // Setting it takes a reference, and the object is never asked
        // for an interface.
        c.hr(set(r, which, spy.obj() as isize), S_OK)?;
        c.eq(spy.refs as u64, 2)?;
        c.eq(u64::from(spy.queried), 0)?;
        // Reading it back hands out another one, which is the caller's.
        let (hr, v) = property(r, which);
        line(which, hr, v);
        c.hr(hr, S_OK)?;
        c.eq(v as usize as u64, spy.obj() as usize as u64)?;
        c.eq(spy.refs as u64, 3)?;
        c.eq(u64::from(release(spy.obj())), 2)?;
        // Setting it to NULL gives the reader's reference back.
        c.hr(set(r, which, 0), S_OK)?;
        c.eq(spy.refs as u64, 1)?;
        get(c, r, which, 0)?;
        // And so does releasing a reader that still holds one, which is
        // what lets a program let go of its own reference at once.
        c.hr(set(r, which, spy.obj() as isize), S_OK)?;
        c.eq(spy.refs as u64, 2)?;
        c.eq(u64::from(release(r)), 0)?;
        c.eq(spy.refs as u64, 1)?;
        c.eq(u64::from(spy.queried), 0)?;
    }
    Ok(())
}

/// When the properties lock, and what unlocks them again.
fn locking(c: &mut Checks) -> Result<(), u32> {
    let mut spy = Spy::new();
    let r = reader(c)?;
    let mut stream = Stream::new(DEEP);
    state(c, r, READ_STATE_CLOSED)?;
    // An input leaves the reader Initial, with every property free: a
    // program sets them up to the first Read and not after.
    c.hr(input(r, &mut stream), S_OK)?;
    state(c, r, READ_STATE_INITIAL)?;
    c.hr(set(r, PROP_CONFORMANCE_LEVEL, LEVEL_FRAGMENT), S_OK)?;
    c.hr(set(r, PROP_MAX_ELEMENT_DEPTH, 9), S_OK)?;
    c.hr(set(r, PROP_MULTI_LANGUAGE, spy.obj() as isize), S_OK)?;
    c.eq(spy.refs as u64, 2)?;
    // The first Read locks them all.
    let (hr, ty) = read(r);
    c.hr(hr, S_OK)?;
    c.eq(u64::from(ty), u64::from(NODE_ELEMENT))?;
    state(c, r, READ_STATE_INTERACTIVE)?;
    for (which, v) in [
        (PROP_CONFORMANCE_LEVEL, LEVEL_DOCUMENT),
        (PROP_MAX_ELEMENT_DEPTH, 4),
        (PROP_MULTI_LANGUAGE, 0),
    ] {
        c.hr(set(r, which, v), E_ACCESSDENIED)?;
    }
    // A refused set changes nothing: no reference given back, and the
    // values still read back.
    c.eq(spy.refs as u64, 2)?;
    get(c, r, PROP_MAX_ELEMENT_DEPTH, 9)?;
    get(c, r, PROP_CONFORMANCE_LEVEL, LEVEL_FRAGMENT)?;
    // Taking the input away leaves the reader Initial -- not Closed,
    // which is only what it is before it ever had one -- and its
    // properties free again.
    c.hr(no_input(r), S_OK)?;
    state(c, r, READ_STATE_INITIAL)?;
    c.hr(set(r, PROP_CONFORMANCE_LEVEL, LEVEL_DOCUMENT), S_OK)?;
    c.eq(u64::from(release(r)), 0)?;
    c.eq(spy.refs as u64, 1)
}

/// What MaxElementDepth does with the value it kept.
fn limits(c: &mut Checks) -> Result<(), u32> {
    let r = reader(c)?;
    let mut stream = Stream::new(DEEP);
    c.hr(set(r, PROP_MAX_ELEMENT_DEPTH, 2), S_OK)?;
    c.hr(input(r, &mut stream), S_OK)?;
    // Two elements come through, at depth 0 and depth 1.
    for depth in [0, 1] {
        let (hr, ty) = read(r);
        c.hr(hr, S_OK)?;
        c.eq(u64::from(ty), u64::from(NODE_ELEMENT))?;
        let (hr, got) = u32_at(r, GET_DEPTH);
        c.hr(hr, S_OK)?;
        c.eq(u64::from(got), depth)?;
    }
    // The third is one too deep, and the reader is left in the Error
    // state -- where its properties may be set again.
    let (hr, _) = read(r);
    c.hr(hr, SC_E_MAXELEMENTDEPTH)?;
    state(c, r, READ_STATE_ERROR)?;
    c.hr(set(r, PROP_MAX_ELEMENT_DEPTH, 0), S_OK)?;
    c.eq(u64::from(release(r)), 0)?;

    // And 0 is no limit at all: the same document reads whole, nine
    // nodes of it.
    let r = reader(c)?;
    let mut stream = Stream::new(DEEP);
    c.hr(set(r, PROP_MAX_ELEMENT_DEPTH, 0), S_OK)?;
    c.hr(input(r, &mut stream), S_OK)?;
    for _ in 0..9 {
        let (hr, _) = read(r);
        c.hr(hr, S_OK)?;
    }
    let (hr, _) = read(r);
    c.hr(hr, S_FALSE)?;
    c.eq(u64::from(release(r)), 0)
}

/// ConformanceLevel Auto reads as Fragment does. What Document answers
/// on the same two documents is in the reader's own `BADS` table.
fn fragments(c: &mut Checks) -> Result<(), u32> {
    for level in [LEVEL_AUTO, LEVEL_FRAGMENT] {
        let r = reader(c)?;
        let mut stream = Stream::new(TWO_ROOTS);
        c.hr(set(r, PROP_CONFORMANCE_LEVEL, level), S_OK)?;
        get(c, r, PROP_CONFORMANCE_LEVEL, level)?;
        c.hr(input(r, &mut stream), S_OK)?;
        // Two roots, and nothing after them.
        for _ in 0..2 {
            let (hr, ty) = read(r);
            c.hr(hr, S_OK)?;
            c.eq(u64::from(ty), u64::from(NODE_ELEMENT))?;
        }
        let (hr, _) = read(r);
        c.hr(hr, S_FALSE)?;
        c.eq(u64::from(release(r)), 0)?;

        // And character data where a document would have none: a
        // node like the text inside an element, entities expanded,
        // with the white space in front of it part of it -- while a
        // run of white space with markup after it is a node of its
        // own, and white space rather than text.
        for (doc, nodes) in LOOSE {
            let r = reader(c)?;
            let mut stream = Stream::new(doc);
            c.hr(set(r, PROP_CONFORMANCE_LEVEL, level), S_OK)?;
            c.hr(input(r, &mut stream), S_OK)?;
            for (ty, value) in *nodes {
                let (hr, got) = read(r);
                c.hr(hr, S_OK)?;
                c.eq(u64::from(got), u64::from(*ty))?;
                let (hr, text, len) = crate::string(r, GET_VALUE);
                c.hr(hr, S_OK)?;
                c.text(text, len, value)?;
            }
            let (hr, _) = read(r);
            c.hr(hr, S_FALSE)?;
            c.eq(u64::from(release(r)), 0)?;
        }
    }
    Ok(())
}

/// DtdProcessing Prohibit, the default, on a document type declaration
/// with an internal subset. What Parse makes of one -- the declaration
/// as a node, and its entities expanded in the text -- is a gap of
/// gala's own, which the reader records; the property itself is taken
/// either way (`taken`).
fn dtd(c: &mut Checks) -> Result<(), u32> {
    let r = reader(c)?;
    let mut stream = Stream::new(SUBSET);
    c.hr(input(r, &mut stream), S_OK)?;
    let (hr, _) = read(r);
    c.hr(hr, WC_E_DTDPROHIBITED)?;
    state(c, r, READ_STATE_ERROR)?;
    c.eq(u64::from(release(r)), 0)
}

/// A document read to its end: the properties are free again there, as
/// they are in the Initial and Error states, and only the Interactive
/// state locks them.
fn past_the_end(c: &mut Checks) -> Result<(), u32> {
    let r = reader(c)?;
    let mut stream = Stream::new(b"<a/>");
    c.hr(input(r, &mut stream), S_OK)?;
    let (hr, ty) = read(r);
    c.hr(hr, S_OK)?;
    c.eq(u64::from(ty), u64::from(NODE_ELEMENT))?;
    let (hr, _) = read(r);
    c.hr(hr, S_FALSE)?;
    state(c, r, READ_STATE_END_OF_FILE)?;
    c.hr(set(r, PROP_CONFORMANCE_LEVEL, LEVEL_FRAGMENT), S_OK)?;
    c.hr(set(r, PROP_MAX_ELEMENT_DEPTH, 9), S_OK)?;
    get(c, r, PROP_MAX_ELEMENT_DEPTH, 9)?;
    c.eq(u64::from(release(r)), 0)
}

/// A byte that is not the encoding the document is taken for: the input
/// is taken all the same -- S_OK, and the Initial state -- and the first
/// read is what answers for it, with the code for a character XML does
/// not allow and the Error state. The encoding a declaration names is a
/// gap of gala's own (the reader records it): an encoding its decoder
/// does not know is refused where Windows asks the MultiLanguage object
/// for the character set, and answers MX_E_ENCODING or
/// MX_E_ENCODINGSIGNATURE.
fn undecodable(c: &mut Checks) -> Result<(), u32> {
    let r = reader(c)?;
    let mut stream = Stream::new(b"<a>\x80</a>");
    c.hr(input(r, &mut stream), S_OK)?;
    state(c, r, READ_STATE_INITIAL)?;
    let (hr, ty) = read(r);
    c.hr(hr, WC_E_XMLCHARACTER)?;
    c.eq(u64::from(ty), u64::from(NODE_NONE))?;
    state(c, r, READ_STATE_ERROR)?;
    // And again, for every read after it.
    let (hr, _) = read(r);
    c.hr(hr, WC_E_XMLCHARACTER)?;
    c.eq(u64::from(release(r)), 0)
}
