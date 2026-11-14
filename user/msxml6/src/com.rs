//! The COM objects: the document (IXMLDOMDocument, 2 and 3), its nodes
//! (IXMLDOMNode and each node type's interface), node lists
//! (IXMLDOMNodeList, IXMLDOMSelection), attribute maps
//! (IXMLDOMNamedNodeMap) and the parse error (IXMLDOMParseError), with
//! vtables in msxml6.h's order.
//!
//! A node object is a wrapper: the document and an index into its tree.
//! Each wrapper holds a reference on the document, so the tree lives as
//! long as anything in it is held; the document node itself is the
//! document object (asking for it gives that object). Two wrappers of one
//! node are two objects (MSXML returns the same one; nothing gala runs
//! compares them yet).
//!
//! Documents follow the rental model (DOMDocument60's): one thread at a
//! time; nothing is locked.

use alloc::string::String;
use alloc::vec::Vec;
use core::ffi::c_void;
use core::sync::atomic::{AtomicU32, Ordering};

use crate::dom::{Kind, NONE, Tree};

mod mutate;
mod typed;
mod xslt;
use crate::{
    Bstr, E_FAIL, E_INVALIDARG, E_NOINTERFACE, E_OUTOFMEMORY, E_POINTER, Guid, HResult,
    IID_IUNKNOWN, LEDGER_OTHER, LEDGER_UNSUPPORTED, S_FALSE, S_OK, VARIANT_FALSE, VARIANT_TRUE,
    VT_BOOL, VT_BSTR, VT_BYREF, VT_DISPATCH, VT_EMPTY, VT_I2, VT_I4, VT_INT, VT_NULL, VT_UI4,
    VT_UNKNOWN, Variant, bstr, free_box, guid, not_implemented, put, record, text, try_box, xpath,
};
use mutate::*;
use typed::{get_data_type, get_node_typed_value, put_data_type, put_node_typed_value};
use xslt::{transform_node, transform_node_to_object};

type Obj = *mut c_void;

const DOM_D4: [u8; 8] = [0xb2, 0x0e, 0x00, 0xc0, 0x4f, 0x98, 0x3e, 0x60];
const fn dom_iid(d1: u32) -> Guid {
    guid(d1, 0x7b36, 0x11d2, DOM_D4)
}
const IID_IDISPATCH: Guid = guid(0x0002_0400, 0, 0, [0xc0, 0, 0, 0, 0, 0, 0, 0x46]);
const IID_IXMLDOMNODE: Guid = dom_iid(0x2933_bf80);
const IID_IXMLDOMDOCUMENT: Guid = dom_iid(0x2933_bf81);
const IID_IXMLDOMNODELIST: Guid = dom_iid(0x2933_bf82);
const IID_IXMLDOMNAMEDNODEMAP: Guid = dom_iid(0x2933_bf83);
const IID_IXMLDOMCHARACTERDATA: Guid = dom_iid(0x2933_bf84);
const IID_IXMLDOMATTRIBUTE: Guid = dom_iid(0x2933_bf85);
const IID_IXMLDOMELEMENT: Guid = dom_iid(0x2933_bf86);
const IID_IXMLDOMTEXT: Guid = dom_iid(0x2933_bf87);
const IID_IXMLDOMCOMMENT: Guid = dom_iid(0x2933_bf88);
const IID_IXMLDOMPROCESSINGINSTRUCTION: Guid = dom_iid(0x2933_bf89);
const IID_IXMLDOMCDATASECTION: Guid = dom_iid(0x2933_bf8a);
const IID_IXMLDOMDOCUMENT2: Guid = dom_iid(0x2933_bf95);
const IID_IXMLDOMDOCUMENT3: Guid = dom_iid(0x2933_bf96);
const IID_IXMLDOMDOCUMENTFRAGMENT: Guid =
    guid(0x3efa_a413, 0x272f, 0x11d2, [0x83, 0x6f, 0x00, 0x00, 0xf8, 0x7a, 0x77, 0x82]);
const IID_IXMLDOMSELECTION: Guid =
    guid(0xaa63_4fc7, 0x5888, 0x44a7, [0xa2, 0x57, 0x3a, 0x47, 0x15, 0x0d, 0x3a, 0x0e]);
const IID_IXMLDOMPARSEERROR: Guid =
    guid(0x3efa_a426, 0x272f, 0x11d2, [0x83, 0x6f, 0x00, 0x00, 0xf8, 0x7a, 0x77, 0x82]);

// MSXML's parse error codes.
const XML_E_INVALIDATROOTLEVEL: i32 = 0xc00c_e556u32 as i32;
const XML_E_MISSINGROOT: i32 = 0xc00c_e558u32 as i32;
const XML_E_ENDTAGMISMATCH: i32 = 0xc00c_e56du32 as i32;
/// INET_E_OBJECT_NOT_FOUND: "The system cannot locate the object
/// specified" (a file load).
const INET_E_OBJECT_NOT_FOUND: i32 = 0x800c_0006u32 as i32;
/// The largest file `load` reads.
const MAX_FILE: u64 = 64 << 20;

#[repr(C)]
struct VTable<const N: usize>([*const (); N]);
// SAFETY: function pointers only.
unsafe impl<const N: usize> Sync for VTable<N> {}

/// A node object: the vtable of its node type, its references, the
/// document and the node's index.
#[repr(C)]
struct NodeObj {
    vtable: *const (),
    refs: AtomicU32,
    doc: *mut Doc,
    idx: u32,
}

/// A property's value (setProperty/getProperty).
#[derive(Clone)]
enum Prop {
    Bool(bool),
    Int(i32),
    Str(String),
}

/// The document: its object (the document node), the tree, the state
/// programs set and the last parse's error.
#[repr(C)]
struct Doc {
    obj: NodeObj,
    tree: Tree,
    async_load: bool,
    validate: bool,
    resolve: bool,
    preserve: bool,
    ready: i32,
    url: String,
    /// SelectionNamespaces' text, and its declarations (None when the
    /// text did not parse: then prefixes resolve to nothing).
    ns_text: String,
    ns: Option<Vec<(String, String)>>,
    /// The other properties set (name, value).
    props: Vec<(String, Prop)>,
    error: ParseError,
    /// The schema cache set (`schemas`; a reference held), or NULL.
    schemas: Obj,
    /// Made as FreeThreadedDOMDocument60 (served as the rental document,
    /// ROADMAP OF; an XSLT template takes only such a stylesheet).
    free_threaded: bool,
}

impl Drop for Doc {
    fn drop(&mut self) {
        if !self.schemas.is_null() {
            schema::cache_release(self.schemas);
        }
    }
}

#[derive(Clone, Default)]
struct ParseError {
    code: i32,
    reason: String,
    src: String,
    url: String,
    line: i32,
    linepos: i32,
    filepos: i32,
}

/// The node object behind an interface pointer.
fn node<'a>(this: Obj) -> &'a NodeObj {
    // SAFETY: our own objects' interface pointers.
    unsafe { &*(this as *const NodeObj) }
}

/// The document of a node object.
fn doc<'a>(this: Obj) -> &'a mut Doc {
    // SAFETY: a node holds a reference on its document; the rental model
    // keeps one thread in a document at a time.
    unsafe { &mut *node(this).doc }
}

fn idx(this: Obj) -> u32 {
    node(this).idx
}

/// Makes a document (the class factory's CreateInstance); `free_threaded`
/// for FreeThreadedDOMDocument60.
pub fn new_document(riid: *const Guid, out: *mut *mut c_void, free_threaded: bool) -> HResult {
    let d = Doc {
        obj: NodeObj {
            vtable: &DOC_VTABLE as *const VTable<84> as *const (),
            refs: AtomicU32::new(1),
            doc: core::ptr::null_mut(),
            idx: 0,
        },
        tree: Tree::new(),
        async_load: true,
        validate: true,
        resolve: false,
        preserve: false,
        ready: 4,
        url: String::new(),
        ns_text: String::new(),
        ns: Some(Vec::new()),
        props: Vec::new(),
        error: ParseError::default(),
        schemas: core::ptr::null_mut(),
        free_threaded,
    };
    give_document(d, riid, out)
}

/// Gives the caller document `d` as interface `riid`.
fn give_document(d: Doc, riid: *const Guid, out: *mut *mut c_void) -> HResult {
    let Some(p) = try_box(d) else { return E_OUTOFMEMORY };
    // SAFETY: the new document, ours.
    unsafe { (*p).obj.doc = p };
    let this = p as Obj;
    let hr = node_query(this, riid, out);
    node_release(this);
    hr
}

/// IXMLDOMNode::cloneNode(deep, out): a node of no parent in the same
/// document, with its attributes and, deep, its children; the document
/// node's clone is a new document with the original's settings and
/// properties and, deep, its content (msxml_test's `clone`).
extern "system" fn clone_node(this: Obj, deep: i16, out: *mut Obj) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    let d = doc(this);
    let i = idx(this);
    if i != 0 {
        let copy = d.tree.copy(i, deep != 0, NONE);
        return give(d, Some(copy), out);
    }
    let copy = Doc {
        obj: NodeObj {
            vtable: d.obj.vtable,
            refs: AtomicU32::new(1),
            doc: core::ptr::null_mut(),
            idx: 0,
        },
        tree: if deep != 0 { d.tree.clone() } else { Tree::new() },
        async_load: d.async_load,
        validate: d.validate,
        resolve: d.resolve,
        preserve: d.preserve,
        ready: 4,
        url: d.url.clone(),
        ns_text: d.ns_text.clone(),
        ns: d.ns.clone(),
        props: d.props.clone(),
        error: ParseError::default(),
        schemas: core::ptr::null_mut(),
        free_threaded: d.free_threaded,
    };
    give_document(copy, &IID_IXMLDOMNODE, out.cast())
}

/// An object for node `i` of `d` (one reference, the caller's): the
/// document object for the document node. NULL when out of memory.
fn wrap(d: &mut Doc, i: u32) -> Obj {
    let d_ptr = d as *mut Doc;
    if i == 0 {
        d.obj.refs.fetch_add(1, Ordering::Relaxed);
        return d_ptr as Obj;
    }
    let vtable = match d.tree.node(i).kind {
        Kind::Element => &ELEMENT_VTABLE as *const VTable<52> as *const (),
        Kind::Attribute => &ATTRIBUTE_VTABLE as *const VTable<46> as *const (),
        Kind::Text | Kind::CData => &TEXT_VTABLE as *const VTable<52> as *const (),
        Kind::Comment => &COMMENT_VTABLE as *const VTable<51> as *const (),
        Kind::Pi => &PI_VTABLE as *const VTable<46> as *const (),
        Kind::Document => &DOC_VTABLE as *const VTable<84> as *const (),
        Kind::Fragment => &FRAGMENT_VTABLE as *const VTable<43> as *const (),
    };
    match try_box(NodeObj { vtable, refs: AtomicU32::new(1), doc: d_ptr, idx: i }) {
        Some(p) => {
            d.obj.refs.fetch_add(1, Ordering::Relaxed);
            p as Obj
        }
        None => core::ptr::null_mut(),
    }
}

/// Gives the caller an object for node `i` (or NULL and S_FALSE for
/// none).
fn give(d: &mut Doc, i: Option<u32>, out: *mut Obj) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    match i {
        None => {
            put(out, core::ptr::null_mut());
            S_FALSE
        }
        Some(i) => {
            let o = wrap(d, i);
            put(out, o);
            if o.is_null() { E_OUTOFMEMORY } else { S_OK }
        }
    }
}

/// Gives the caller a string (S_OK), or NULL and S_FALSE for None.
fn give_str(out: *mut Bstr, s: Option<&str>) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    match s {
        None => {
            put(out, core::ptr::null_mut());
            S_FALSE
        }
        Some(s) => {
            let b = bstr(s);
            put(out, b);
            if b.is_null() { E_OUTOFMEMORY } else { S_OK }
        }
    }
}

fn give_bool(out: *mut i16, v: bool) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    put(out, if v { VARIANT_TRUE } else { VARIANT_FALSE });
    S_OK
}

fn variant_str(out: *mut Variant, s: &str) -> HResult {
    let b = bstr(s);
    if b.is_null() {
        return E_OUTOFMEMORY;
    }
    put(out, Variant { vt: VT_BSTR, reserved: [0; 3], value: [b as usize, 0] });
    S_OK
}

fn variant_null(out: *mut Variant) {
    put(out, Variant { vt: VT_NULL, reserved: [0; 3], value: [0; 2] });
}

// ---- IUnknown, IDispatch ----

extern "system" fn node_query(this: Obj, riid: *const Guid, out: *mut Obj) -> HResult {
    if out.is_null() || riid.is_null() {
        return E_POINTER;
    }
    // SAFETY: the caller's IID.
    let iid = unsafe { *riid };
    let kind = doc(this).tree.node(idx(this)).kind;
    let ok = iid == IID_IUNKNOWN
        || iid == IID_IDISPATCH
        || iid == IID_IXMLDOMNODE
        || match kind {
            Kind::Document => {
                iid == IID_IXMLDOMDOCUMENT
                    || iid == IID_IXMLDOMDOCUMENT2
                    || iid == IID_IXMLDOMDOCUMENT3
            }
            Kind::Element => iid == IID_IXMLDOMELEMENT,
            Kind::Attribute => iid == IID_IXMLDOMATTRIBUTE,
            Kind::Text => iid == IID_IXMLDOMCHARACTERDATA || iid == IID_IXMLDOMTEXT,
            Kind::CData => {
                iid == IID_IXMLDOMCHARACTERDATA
                    || iid == IID_IXMLDOMTEXT
                    || iid == IID_IXMLDOMCDATASECTION
            }
            Kind::Comment => iid == IID_IXMLDOMCHARACTERDATA || iid == IID_IXMLDOMCOMMENT,
            Kind::Pi => iid == IID_IXMLDOMPROCESSINGINSTRUCTION,
            Kind::Fragment => iid == IID_IXMLDOMDOCUMENTFRAGMENT,
        };
    if !ok {
        put(out, core::ptr::null_mut());
        crate::record_iid("QI? node", &iid);
        return E_NOINTERFACE;
    }
    node(this).refs.fetch_add(1, Ordering::Relaxed);
    put(out, this);
    S_OK
}

extern "system" fn node_add_ref(this: Obj) -> u32 {
    node(this).refs.fetch_add(1, Ordering::Relaxed) + 1
}

extern "system" fn node_release(this: Obj) -> u32 {
    let n = node(this);
    let left = n.refs.fetch_sub(1, Ordering::AcqRel) - 1;
    if left == 0 {
        let d = n.doc;
        if this == d as Obj {
            // SAFETY: the document's last reference: nothing else holds it.
            unsafe { free_box(d) };
        } else {
            // SAFETY: a wrapper from `wrap`, now unreferenced.
            unsafe { free_box(this as *mut NodeObj) };
            // SAFETY: the document this wrapper held.
            node_release(d as Obj);
        }
    }
    left
}

/// IDispatch::GetTypeInfoCount: no type information (0).
extern "system" fn get_type_info_count(_this: Obj, n: *mut u32) -> HResult {
    if n.is_null() {
        return E_INVALIDARG;
    }
    put(n, 0);
    S_OK
}

extern "system" fn get_type_info(_this: Obj, _i: u32, _lcid: u32, _out: *mut Obj) -> HResult {
    not_implemented(b"IDispatch::GetTypeInfo")
}

extern "system" fn get_ids_of_names(
    _this: Obj,
    _iid: *const Guid,
    _names: *mut *mut u16,
    _n: u32,
    _lcid: u32,
    _ids: *mut i32,
) -> HResult {
    not_implemented(b"IDispatch::GetIDsOfNames")
}

extern "system" fn invoke(
    _this: Obj,
    _id: i32,
    _iid: *const Guid,
    _lcid: u32,
    _flags: u16,
    _params: *mut c_void,
    _result: *mut Variant,
    _exception: *mut c_void,
    _arg_err: *mut u32,
) -> HResult {
    not_implemented(b"IDispatch::Invoke")
}

/// Methods gala does not implement: each records itself and is
/// E_NOTIMPL (with its own parameters, as stdcall callees pop them).
macro_rules! not_implemented {
    ($($f:ident($($t:ty),*) = $name:literal;)*) => {
        $(
            extern "system" fn $f(_this: Obj $(, _: $t)*) -> HResult {
                not_implemented($name)
            }
        )*
    };
}

not_implemented! {
    get_specified(*mut i16) = b"IXMLDOMNode::get_specified";
    get_definition(*mut Obj) = b"IXMLDOMNode::get_definition";
    get_parsed(*mut i16) = b"IXMLDOMNode::get_parsed";
    get_doctype(*mut Obj) = b"IXMLDOMDocument::get_doctype";
    get_implementation(*mut Obj) = b"IXMLDOMDocument::get_implementation";
    create_entity_reference(Bstr, *mut Obj) = b"IXMLDOMDocument::createEntityReference";
    node_from_id(Bstr, *mut Obj) = b"IXMLDOMDocument::nodeFromID";
    abort() = b"IXMLDOMDocument::abort";
    put_onreadystatechange(Variant) = b"IXMLDOMDocument::put_onreadystatechange";
    put_ondataavailable(Variant) = b"IXMLDOMDocument::put_ondataavailable";
    put_ontransformnode(Variant) = b"IXMLDOMDocument::put_ontransformnode";
    get_namespaces(*mut Obj) = b"IXMLDOMDocument2::get_namespaces";
    put_data(Bstr) = b"IXMLDOMCharacterData::put_data";
    substring_data(i32, i32, *mut Bstr) = b"IXMLDOMCharacterData::substringData";
    append_data(Bstr) = b"IXMLDOMCharacterData::appendData";
    insert_data(i32, Bstr) = b"IXMLDOMCharacterData::insertData";
    delete_data(i32, i32) = b"IXMLDOMCharacterData::deleteData";
    replace_data(i32, i32, Bstr) = b"IXMLDOMCharacterData::replaceData";
    split_text(i32, *mut Obj) = b"IXMLDOMText::splitText";
    pi_put_data(Bstr) = b"IXMLDOMProcessingInstruction::put_data";
}

// ---- IXMLDOMNode ----

extern "system" fn get_node_name(this: Obj, out: *mut Bstr) -> HResult {
    let n = doc(this).tree.node(idx(this));
    let name = match n.kind {
        Kind::Document => "#document",
        Kind::Text => "#text",
        Kind::CData => "#cdata-section",
        Kind::Comment => "#comment",
        Kind::Fragment => "#document-fragment",
        _ => n.name.as_str(),
    };
    give_str(out, Some(name))
}

/// nodeValue: the value of attributes, text, comments and processing
/// instructions; VT_NULL (S_FALSE) for elements and the document.
extern "system" fn get_node_value(this: Obj, out: *mut Variant) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    let n = doc(this).tree.node(idx(this));
    match n.kind {
        Kind::Element | Kind::Document | Kind::Fragment => {
            variant_null(out);
            S_FALSE
        }
        _ => variant_str(out, &n.value),
    }
}

extern "system" fn get_node_type(this: Obj, out: *mut u32) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    put(out, doc(this).tree.node(idx(this)).kind as u32);
    S_OK
}

/// parentNode: none for the document and for attributes (as the DOM
/// has it).
extern "system" fn get_parent_node(this: Obj, out: *mut Obj) -> HResult {
    let d = doc(this);
    let n = d.tree.node(idx(this));
    let p = (n.kind != Kind::Attribute && n.parent != NONE).then_some(n.parent);
    give(d, p, out)
}

extern "system" fn get_child_nodes(this: Obj, out: *mut Obj) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    new_list(this, Items::Children(idx(this)), out)
}

extern "system" fn get_first_child(this: Obj, out: *mut Obj) -> HResult {
    let d = doc(this);
    let c = d.tree.node(idx(this)).children.first().copied();
    give(d, c, out)
}

extern "system" fn get_last_child(this: Obj, out: *mut Obj) -> HResult {
    let d = doc(this);
    let c = d.tree.node(idx(this)).children.last().copied();
    give(d, c, out)
}

extern "system" fn get_previous_sibling(this: Obj, out: *mut Obj) -> HResult {
    let d = doc(this);
    let s = d.tree.siblings(idx(this)).0;
    give(d, s, out)
}

extern "system" fn get_next_sibling(this: Obj, out: *mut Obj) -> HResult {
    let d = doc(this);
    let s = d.tree.siblings(idx(this)).1;
    give(d, s, out)
}

/// attributes: an element's (a named node map); none for other nodes.
extern "system" fn get_attributes(this: Obj, out: *mut Obj) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    if doc(this).tree.node(idx(this)).kind != Kind::Element {
        put(out, core::ptr::null_mut());
        return S_FALSE;
    }
    let d = node(this).doc;
    let map = MapObj {
        vtable: &MAP_VTABLE as *const VTable<17> as *const (),
        refs: AtomicU32::new(1),
        doc: d,
        el: idx(this),
        cursor: 0,
    };
    match try_box(map) {
        Some(p) => {
            node_add_ref(d as Obj);
            put(out, p as Obj);
            S_OK
        }
        None => E_OUTOFMEMORY,
    }
}

extern "system" fn has_child_nodes(this: Obj, out: *mut i16) -> HResult {
    let has = !doc(this).tree.node(idx(this)).children.is_empty();
    put(out, if has { VARIANT_TRUE } else { VARIANT_FALSE });
    if has { S_OK } else { S_FALSE }
}

/// ownerDocument: the document (none for the document itself).
extern "system" fn get_owner_document(this: Obj, out: *mut Obj) -> HResult {
    let d = doc(this);
    let i = (idx(this) != 0).then_some(0);
    give(d, i, out)
}

extern "system" fn get_node_type_string(this: Obj, out: *mut Bstr) -> HResult {
    let s = match doc(this).tree.node(idx(this)).kind {
        Kind::Element => "element",
        Kind::Attribute => "attribute",
        Kind::Text => "text",
        Kind::CData => "cdatasection",
        Kind::Pi => "processinginstruction",
        Kind::Comment => "comment",
        Kind::Document => "document",
        Kind::Fragment => "documentfragment",
    };
    give_str(out, Some(s))
}

/// text: an element's or the document's text (every text and CDATA node
/// in it), white space at either end dropped unless preserveWhiteSpace;
/// other nodes' value.
extern "system" fn get_text(this: Obj, out: *mut Bstr) -> HResult {
    let d = doc(this);
    let mut s = String::new();
    d.tree.text(idx(this), &mut s);
    let kind = d.tree.node(idx(this)).kind;
    let s = if !d.preserve && matches!(kind, Kind::Element | Kind::Document) {
        s.trim_matches([' ', '\t', '\n', '\r'])
    } else {
        s.as_str()
    };
    give_str(out, Some(s))
}

extern "system" fn get_xml(this: Obj, out: *mut Bstr) -> HResult {
    let mut s = String::new();
    doc(this).tree.xml(idx(this), &mut s);
    give_str(out, Some(&s))
}

/// Compiles and evaluates `expr` from this node: the node-set, or the
/// failure (E_FAIL).
fn select(this: Obj, expr: Bstr) -> Result<Vec<u32>, HResult> {
    let d = doc(this);
    let Some(e) = text(expr) else { return Err(E_INVALIDARG) };
    let empty: Vec<(String, String)> = Vec::new();
    let ns = d.ns.as_ref().unwrap_or(&empty);
    let q = xpath::compile(&e, ns).map_err(|why| {
        if why.contains("not supported") || why.contains("namespace axis") {
            record(
                b"selectNodes(XPath: id(), lang() or the namespace axis)",
                LEDGER_UNSUPPORTED,
                0,
            );
        }
        E_FAIL
    })?;
    match q.eval(&d.tree, idx(this)) {
        Ok(xpath::Value::Nodes(n)) => Ok(n),
        _ => Err(E_FAIL),
    }
}

extern "system" fn select_nodes(this: Obj, expr: Bstr, out: *mut Obj) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    put(out, core::ptr::null_mut());
    match select(this, expr) {
        Ok(nodes) => new_list(this, Items::Fixed(nodes), out),
        Err(hr) => hr,
    }
}

extern "system" fn select_single_node(this: Obj, expr: Bstr, out: *mut Obj) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    put(out, core::ptr::null_mut());
    match select(this, expr) {
        Ok(nodes) => give(doc(this), nodes.first().copied(), out),
        Err(hr) => hr,
    }
}

extern "system" fn get_namespace_uri(this: Obj, out: *mut Bstr) -> HResult {
    let n = doc(this).tree.node(idx(this));
    give_str(out, (!n.ns.is_empty()).then_some(n.ns.as_str()))
}

extern "system" fn get_prefix(this: Obj, out: *mut Bstr) -> HResult {
    let n = doc(this).tree.node(idx(this));
    let p = n.prefix();
    give_str(out, (!p.is_empty()).then_some(p))
}

extern "system" fn get_base_name(this: Obj, out: *mut Bstr) -> HResult {
    let n = doc(this).tree.node(idx(this));
    match n.kind {
        Kind::Element | Kind::Attribute | Kind::Pi => give_str(out, Some(n.local())),
        _ => give_str(out, None),
    }
}

// ---- IXMLDOMDocument ----

extern "system" fn get_document_element(this: Obj, out: *mut Obj) -> HResult {
    let d = doc(this);
    let r = d.tree.root();
    give(d, r, out)
}

/// Replaces the tree with `src` parsed (or an empty one and the error).
fn parse_into(d: &mut Doc, src: Result<String, ParseError>, url: &str) -> bool {
    d.tree = Tree::new();
    d.ready = 4;
    d.url = url.into();
    let src = match src {
        Ok(s) => s,
        Err(e) => {
            d.error = e;
            return false;
        }
    };
    match gala_xml::parse_text(&src) {
        Ok(parsed) => {
            d.tree = Tree::from_document(&parsed, d.preserve);
            d.error = ParseError::default();
            // validateOnParse with a schema cache: an invalid document
            // fails to load and leaves an empty one.
            if let Some(mut e) = schema::on_load(d) {
                e.url = url.into();
                d.tree = Tree::new();
                d.error = e;
                return false;
            }
            true
        }
        Err(e) => {
            let code = if e.msg.contains("no root") {
                XML_E_MISSINGROOT
            } else if e.msg.contains(" closes ") {
                XML_E_ENDTAGMISMATCH
            } else {
                XML_E_INVALIDATROOTLEVEL
            };
            let line_text = src.split('\n').nth(e.line.saturating_sub(1) as usize).unwrap_or("");
            let before: usize = src
                .split('\n')
                .take(e.line.saturating_sub(1) as usize)
                .map(|l| l.chars().count() + 1)
                .sum();
            // In the ledger with where it is (a program's load or loadXML
            // that fails says no more than S_FALSE).
            let mut what = String::new();
            if what.try_reserve(14 + e.msg.len()).is_ok() {
                what.push_str("parse error: ");
                what.push_str(&e.msg);
                crate::record(
                    what.as_bytes(),
                    crate::LEDGER_OTHER,
                    u64::from(e.line) << 32 | u64::from(e.column),
                );
            }
            // And the text there (the ledger keeps 64 characters of a
            // name: 40 of the line, up to a few past the error), so the
            // input a program built is seen without a debugger.
            let mut near = String::new();
            if near.try_reserve(16 + 4 * 40).is_ok() {
                near.push_str("parse error at: ");
                let from = (e.column as usize).saturating_sub(33);
                near.extend(line_text.chars().skip(from).take(40));
                crate::record(
                    near.as_bytes(),
                    crate::LEDGER_OTHER,
                    u64::from(e.line) << 32 | u64::from(e.column),
                );
            }
            let mut reason = e.msg.clone();
            reason.push_str(".\r\n");
            d.error = ParseError {
                code,
                reason,
                src: line_text.into(),
                url: url.into(),
                line: e.line as i32,
                linepos: e.column as i32,
                filepos: (before + e.column.saturating_sub(1) as usize) as i32,
            };
            false
        }
    }
}

extern "system" fn load_xml(this: Obj, xml: Bstr, ok: *mut i16) -> HResult {
    let d = doc(this);
    let src = match text(xml) {
        Some(s) => Ok(s),
        None if xml.is_null() => Ok(String::new()),
        None => return E_OUTOFMEMORY,
    };
    let loaded = parse_into(d, src, "");
    put(ok, if loaded { VARIANT_TRUE } else { VARIANT_FALSE });
    if loaded { S_OK } else { S_FALSE }
}

#[cfg_attr(
    target_arch = "x86",
    link(name = "kernel32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "kernel32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn CreateFileW(
        path: *const u16,
        access: u32,
        share: u32,
        security: usize,
        disposition: u32,
        flags: u32,
        template: usize,
    ) -> usize;
    fn GetFileSizeEx(h: usize, size: *mut i64) -> i32;
    fn ReadFile(h: usize, buf: *mut u8, len: u32, read: *mut u32, overlapped: usize) -> i32;
    fn CloseHandle(h: usize) -> i32;
    fn GetModuleHandleW(name: *const u16) -> usize;
    fn LoadLibraryExW(name: *const u16, file: usize, flags: u32) -> usize;
    fn FindResourceW(module: usize, name: *const u16, kind: *const u16) -> usize;
    fn LoadResource(module: usize, res: usize) -> *const u8;
    fn SizeofResource(module: usize, res: usize) -> u32;
}

/// A source's text: a `res:` URL (a module's resource) or a local file.
fn read_source(url: &str) -> Result<String, ParseError> {
    if url.len() > 6 && url[..6].eq_ignore_ascii_case("res://") {
        read_resource(url)
    } else {
        read_file(url)
    }
}

/// `s` with its %XX escapes decoded (as a URL's path).
fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out: Vec<u8> = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let hex = |c: u8| (c as char).to_digit(16);
        if b[i] == b'%'
            && i + 2 < b.len()
            && let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2]))
        {
            out.push((h * 16 + l) as u8);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).unwrap_or_else(|_| s.into())
}

/// A NUL-terminated wide copy of `s`.
fn wide(s: &str) -> Vec<u16> {
    let mut w: Vec<u16> = s.encode_utf16().collect();
    w.push(0);
    w
}

/// A `res:` URL's text (urlmon's res protocol, as MSXML loads one):
/// `res://<module>[/<type>]/<name>`, the type RT_HTML (23) when not
/// given, `#<number>` naming an integer type or name (FindResource's
/// rule). The module is the one loaded under that name, else the file
/// mapped as data. Each load is recorded ("other": what a program loads
/// from where).
fn read_resource(url: &str) -> Result<String, ParseError> {
    let not_found = || ParseError {
        code: INET_E_OBJECT_NOT_FOUND,
        reason: "The system cannot locate the resource specified.\r\n".into(),
        url: url.into(),
        ..ParseError::default()
    };
    record(b"load(res: URL)", LEDGER_OTHER, 0);
    let rest = percent_decode(&url[6..]);
    // The module ends at the last ".dll/", ".exe/", ".ocx/" or ".mui/".
    let lower = rest.to_ascii_lowercase();
    let end = [".dll/", ".exe/", ".ocx/", ".mui/"]
        .iter()
        .filter_map(|ext| lower.rfind(ext).map(|i| i + ext.len() - 1))
        .max();
    let Some(end) = end else {
        record(b"load(res: URL without a module)", LEDGER_UNSUPPORTED, 0);
        return Err(not_found());
    };
    let (module, path) = (&rest[..end], &rest[end + 1..]);
    let (kind, name) = match path.split_once('/') {
        Some((k, n)) => (Some(k), n),
        None => (None, path),
    };
    let wmodule = wide(module);
    // SAFETY: a NUL-terminated name.
    let mut h = unsafe { GetModuleHandleW(wmodule.as_ptr()) };
    if h == 0 {
        // SAFETY: as above; LOAD_LIBRARY_AS_DATAFILE | AS_IMAGE_RESOURCE.
        h = unsafe { LoadLibraryExW(wmodule.as_ptr(), 0, 0x22) };
    }
    if h == 0 {
        return Err(not_found());
    }
    let wname = wide(name);
    let wkind = kind.map(wide);
    let kind_ptr = match &wkind {
        Some(k) => k.as_ptr(),
        None => 23 as *const u16,
    };
    // SAFETY: a module handle and NUL-terminated (or integer) names.
    let res = unsafe { FindResourceW(h, wname.as_ptr(), kind_ptr) };
    if res == 0 {
        return Err(not_found());
    }
    // SAFETY: a resource of that module.
    let (data, size) = unsafe { (LoadResource(h, res), SizeofResource(h, res)) };
    if data.is_null() {
        return Err(not_found());
    }
    // SAFETY: the resource's bytes, mapped with its module.
    let bytes = unsafe { core::slice::from_raw_parts(data, size as usize) };
    gala_xml::decode(bytes).map_err(|e| ParseError {
        code: XML_E_INVALIDATROOTLEVEL,
        reason: e.msg,
        url: url.into(),
        line: 1,
        linepos: 1,
        ..ParseError::default()
    })
}

/// A local file's text: `path` is a path or a `file:` URL.
fn read_file(path: &str) -> Result<String, ParseError> {
    let not_found = || ParseError {
        code: INET_E_OBJECT_NOT_FOUND,
        reason: "The system cannot locate the object specified.\r\n".into(),
        url: path.into(),
        ..ParseError::default()
    };
    let mut p = path;
    if p.len() > 5 && p[..5].eq_ignore_ascii_case("file:") {
        p = p[5..].trim_start_matches('/');
    }
    let mut wide: Vec<u16> = Vec::new();
    if wide.try_reserve(p.len() + 1).is_err() {
        return Err(not_found());
    }
    wide.extend(p.encode_utf16().map(|c| if c == u16::from(b'/') { u16::from(b'\\') } else { c }));
    wide.push(0);
    // SAFETY: a NUL-terminated path; GENERIC_READ, FILE_SHARE_READ,
    // OPEN_EXISTING.
    let h = unsafe { CreateFileW(wide.as_ptr(), 0x8000_0000, 1, 0, 3, 0x80, 0) };
    if h == usize::MAX {
        return Err(not_found());
    }
    let mut size = 0i64;
    let mut bytes: Vec<u8> = Vec::new();
    // SAFETY: our handle and outputs.
    let ok = unsafe { GetFileSizeEx(h, &mut size) } != 0
        && (size as u64) <= MAX_FILE
        && bytes.try_reserve(size as usize).is_ok();
    let mut got = 0u32;
    // SAFETY: a buffer of `size` bytes reserved above.
    let ok = ok
        && unsafe { ReadFile(h, bytes.as_mut_ptr(), size as u32, &mut got, 0) } != 0
        && got as i64 == size;
    // SAFETY: our handle.
    unsafe { CloseHandle(h) };
    if !ok {
        record(b"load(file not read: too large or no memory)", LEDGER_OTHER, size as u64);
        return Err(not_found());
    }
    // SAFETY: ReadFile filled `got` bytes.
    unsafe { bytes.set_len(got as usize) };
    match gala_xml::decode(&bytes) {
        Ok(s) => Ok(s),
        Err(e) => Err(ParseError {
            code: XML_E_INVALIDATROOTLEVEL,
            reason: e.msg,
            url: path.into(),
            line: 1,
            linepos: 1,
            ..ParseError::default()
        }),
    }
}

/// load: a local file (a path or `file:` URL), read whole and parsed
/// before load returns, whatever `async` says (recorded when async is
/// on). URLs of other schemes and streams are not supported (recorded:
/// S_FALSE, or E_NOTIMPL for a source that is not a string).
extern "system" fn load(this: Obj, source: Variant, ok: *mut i16) -> HResult {
    put(ok, VARIANT_FALSE);
    let d = doc(this);
    let p = match source.vt {
        VT_BSTR => source.value[0] as *const u16,
        // SAFETY: a VT_BSTR | VT_BYREF variant points at a BSTR.
        v if v == VT_BSTR | VT_BYREF => unsafe { *(source.value[0] as *const *const u16) },
        VT_UNKNOWN => {
            record(b"IXMLDOMDocument::load(a stream)", LEDGER_UNSUPPORTED, 0);
            return crate::E_NOTIMPL;
        }
        other => {
            record(b"IXMLDOMDocument::load(source type)", LEDGER_UNSUPPORTED, other.into());
            return E_INVALIDARG;
        }
    };
    let Some(path) = text(p) else { return E_INVALIDARG };
    let lower = path.get(..8).map(|s| s.to_ascii_lowercase()).unwrap_or_default();
    if lower.starts_with("http://") || lower.starts_with("https://") || lower.starts_with("ftp://")
    {
        record(b"IXMLDOMDocument::load(URL)", LEDGER_UNSUPPORTED, 0);
        let e = ParseError {
            code: INET_E_OBJECT_NOT_FOUND,
            reason: "The system cannot locate the object specified.\r\n".into(),
            url: path.clone(),
            ..ParseError::default()
        };
        parse_into(d, Err(e), &path);
        return S_FALSE;
    }
    if d.async_load {
        record(b"IXMLDOMDocument::load(async: loaded before returning)", LEDGER_OTHER, 0);
    }
    let src = read_source(&path);
    let loaded = parse_into(d, src, &path);
    put(ok, if loaded { VARIANT_TRUE } else { VARIANT_FALSE });
    if loaded { S_OK } else { S_FALSE }
}

extern "system" fn get_ready_state(this: Obj, out: *mut i32) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    put(out, doc(this).ready);
    S_OK
}

extern "system" fn get_parse_error(this: Obj, out: *mut Obj) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    let e = ErrorObj {
        vtable: &ERROR_VTABLE as *const VTable<14> as *const (),
        refs: AtomicU32::new(1),
        error: doc(this).error.clone(),
    };
    match try_box(e) {
        Some(p) => {
            put(out, p as Obj);
            S_OK
        }
        None => E_OUTOFMEMORY,
    }
}

extern "system" fn get_url(this: Obj, out: *mut Bstr) -> HResult {
    let d = doc(this);
    give_str(out, (!d.url.is_empty()).then_some(d.url.as_str()))
}

extern "system" fn get_async(this: Obj, out: *mut i16) -> HResult {
    give_bool(out, doc(this).async_load)
}

extern "system" fn put_async(this: Obj, v: i16) -> HResult {
    doc(this).async_load = v != 0;
    S_OK
}

extern "system" fn get_validate_on_parse(this: Obj, out: *mut i16) -> HResult {
    give_bool(out, doc(this).validate)
}

/// validateOnParse: kept (gala's reader validates nothing: no DTDs or
/// schemas reach it).
extern "system" fn put_validate_on_parse(this: Obj, v: i16) -> HResult {
    doc(this).validate = v != 0;
    S_OK
}

extern "system" fn get_resolve_externals(this: Obj, out: *mut i16) -> HResult {
    give_bool(out, doc(this).resolve)
}

/// resolveExternals: kept; turning it on is recorded (no DTDs, so there
/// is nothing external to resolve).
extern "system" fn put_resolve_externals(this: Obj, v: i16) -> HResult {
    if v != 0 {
        record(b"IXMLDOMDocument::put_resolveExternals(TRUE)", LEDGER_UNSUPPORTED, 0);
    }
    doc(this).resolve = v != 0;
    S_OK
}

extern "system" fn get_preserve_white_space(this: Obj, out: *mut i16) -> HResult {
    give_bool(out, doc(this).preserve)
}

extern "system" fn put_preserve_white_space(this: Obj, v: i16) -> HResult {
    doc(this).preserve = v != 0;
    S_OK
}

// ---- IXMLDOMDocument2's properties ----

/// The properties MSXML 6 knows, with their defaults.
fn default_prop(name: &str) -> Option<Prop> {
    let is = |n: &str| name.eq_ignore_ascii_case(n);
    Some(if is("ProhibitDTD") || is("ValidateOnParse") || is("ForcedResync") {
        Prop::Bool(true)
    } else if is("ServerHTTPRequest")
        || is("AllowDocumentFunction")
        || is("AllowXsltScript")
        || is("ResolveExternals")
        || is("UseInlineSchema")
        || is("MultipleErrorMessages")
        || is("NormalizeAttributeValues")
        || is("NewParser")
    {
        Prop::Bool(false)
    } else if is("MaxElementDepth") {
        Prop::Int(256)
    } else if is("MaxXMLSize") {
        Prop::Int(0)
    } else {
        return None;
    })
}

/// A variant's value as a boolean or number (None for other types).
fn variant_int(v: &Variant) -> Option<i64> {
    match v.vt {
        VT_BOOL | VT_I2 => Some(i64::from(v.value[0] as u16 as i16)),
        VT_I4 | VT_INT => Some(i64::from(v.value[0] as u32 as i32)),
        VT_UI4 => Some(i64::from(v.value[0] as u32)),
        _ => None,
    }
}

extern "system" fn set_property(this: Obj, name: Bstr, value: Variant) -> HResult {
    let d = doc(this);
    let Some(name) = text(name) else { return E_INVALIDARG };
    let as_text =
        || (value.vt == VT_BSTR).then(|| text(value.value[0] as *const u16).unwrap_or_default());
    if name.eq_ignore_ascii_case("SelectionLanguage") {
        // MSXML 6 selects in XPath only.
        return match as_text() {
            Some(s) if s.eq_ignore_ascii_case("XPath") => S_OK,
            Some(_) => {
                record(
                    b"IXMLDOMDocument2::setProperty(SelectionLanguage other than XPath)",
                    LEDGER_OTHER,
                    0,
                );
                E_FAIL
            }
            None => E_FAIL,
        };
    }
    if name.eq_ignore_ascii_case("SelectionNamespaces") {
        let Some(s) = as_text() else { return E_FAIL };
        // A text that does not parse is kept, and prefixes stop
        // resolving (Wine's tests show Windows doing so).
        d.ns = xpath::parse_namespaces(&s);
        d.ns_text = s;
        return if d.ns.is_some() { S_OK } else { E_FAIL };
    }
    let Some(default) = default_prop(&name) else {
        record(b"IXMLDOMDocument2::setProperty(unknown property)", LEDGER_OTHER, 0);
        return E_FAIL;
    };
    let v = match default {
        Prop::Bool(_) => match variant_int(&value) {
            Some(x) => Prop::Bool(x != 0),
            None => return E_FAIL,
        },
        Prop::Int(_) => match variant_int(&value) {
            Some(x) => Prop::Int(x as i32),
            None => return E_FAIL,
        },
        Prop::Str(_) => return E_FAIL,
    };
    if name.eq_ignore_ascii_case("ProhibitDTD") && matches!(v, Prop::Bool(false)) {
        // gala's reader takes no DTD: documents with one still fail.
        record(
            b"IXMLDOMDocument2::setProperty(ProhibitDTD false: DTDs still refused)",
            LEDGER_UNSUPPORTED,
            0,
        );
    }
    if d.props.try_reserve(1).is_err() {
        return E_OUTOFMEMORY;
    }
    d.props.retain(|(n, _)| !n.eq_ignore_ascii_case(&name));
    d.props.push((name, v));
    S_OK
}

extern "system" fn get_property(this: Obj, name: Bstr, out: *mut Variant) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    // The output is cleared (VT_EMPTY) before the name is looked at: an
    // unknown property leaves it empty, as Windows does (msxml_test).
    put(out, Variant { vt: 0, reserved: [0; 3], value: [0, 0] });
    let d = doc(this);
    let Some(name) = text(name) else { return E_INVALIDARG };
    let v = if name.eq_ignore_ascii_case("SelectionLanguage") {
        Prop::Str("XPath".into())
    } else if name.eq_ignore_ascii_case("SelectionNamespaces") {
        Prop::Str(d.ns_text.clone())
    } else if let Some((_, v)) = d.props.iter().find(|(n, _)| n.eq_ignore_ascii_case(&name)) {
        v.clone()
    } else if let Some(v) = default_prop(&name) {
        v
    } else {
        return E_FAIL;
    };
    match v {
        Prop::Str(s) => variant_str(out, &s),
        Prop::Bool(b) => {
            let raw = if b { VARIANT_TRUE } else { VARIANT_FALSE };
            put(out, Variant { vt: VT_BOOL, reserved: [0; 3], value: [raw as u16 as usize, 0] });
            S_OK
        }
        Prop::Int(i) => {
            put(out, Variant { vt: VT_I4, reserved: [0; 3], value: [i as u32 as usize, 0] });
            S_OK
        }
    }
}

// ---- IXMLDOMElement ----

/// The attribute of element `el` named `name` (its qualified name).
fn find_attr(t: &Tree, el: u32, name: &str) -> Option<u32> {
    t.node(el).attrs.iter().copied().find(|&a| t.node(a).name == name)
}

extern "system" fn get_attribute(this: Obj, name: Bstr, out: *mut Variant) -> HResult {
    if out.is_null() || name.is_null() {
        return E_INVALIDARG;
    }
    let d = doc(this);
    let Some(name) = text(name) else { return E_OUTOFMEMORY };
    match find_attr(&d.tree, idx(this), &name) {
        Some(a) => variant_str(out, &d.tree.node(a).value),
        None => {
            variant_null(out);
            S_FALSE
        }
    }
}

extern "system" fn get_attribute_node(this: Obj, name: Bstr, out: *mut Obj) -> HResult {
    if out.is_null() || name.is_null() {
        return E_INVALIDARG;
    }
    let d = doc(this);
    let Some(name) = text(name) else { return E_OUTOFMEMORY };
    let a = find_attr(&d.tree, idx(this), &name);
    give(d, a, out)
}

// ---- IXMLDOMAttribute, IXMLDOMCharacterData, IXMLDOMProcessingInstruction ----

extern "system" fn get_value(this: Obj, out: *mut Variant) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    variant_str(out, &doc(this).tree.node(idx(this)).value)
}

extern "system" fn get_data(this: Obj, out: *mut Bstr) -> HResult {
    give_str(out, Some(&doc(this).tree.node(idx(this)).value))
}

extern "system" fn get_length(this: Obj, out: *mut i32) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    let n = doc(this).tree.node(idx(this)).value.encode_utf16().count();
    put(out, n as i32);
    S_OK
}

// ---- The vtables ----

/// IXMLDOMNode's 43 slots (IUnknown and IDispatch first).
const NODE_SLOTS: [*const (); 43] = [
    node_query as *const (),
    node_add_ref as *const (),
    node_release as *const (),
    get_type_info_count as *const (),
    get_type_info as *const (),
    get_ids_of_names as *const (),
    invoke as *const (),
    get_node_name as *const (),
    get_node_value as *const (),
    put_node_value as *const (),
    get_node_type as *const (),
    get_parent_node as *const (),
    get_child_nodes as *const (),
    get_first_child as *const (),
    get_last_child as *const (),
    get_previous_sibling as *const (),
    get_next_sibling as *const (),
    get_attributes as *const (),
    insert_before as *const (),
    replace_child as *const (),
    remove_child as *const (),
    append_child as *const (),
    has_child_nodes as *const (),
    get_owner_document as *const (),
    clone_node as *const (),
    get_node_type_string as *const (),
    get_text as *const (),
    put_text as *const (),
    get_specified as *const (),
    get_definition as *const (),
    get_node_typed_value as *const (),
    put_node_typed_value as *const (),
    get_data_type as *const (),
    put_data_type as *const (),
    get_xml as *const (),
    transform_node as *const (),
    select_nodes as *const (),
    select_single_node as *const (),
    get_parsed as *const (),
    get_namespace_uri as *const (),
    get_prefix as *const (),
    get_base_name as *const (),
    transform_node_to_object as *const (),
];

/// IXMLDOMNode's slots followed by an interface's own.
const fn with<const N: usize, const M: usize>(own: [*const (); M]) -> VTable<N> {
    let mut t = [core::ptr::null::<()>(); N];
    let mut i = 0;
    while i < 43 {
        t[i] = NODE_SLOTS[i];
        i += 1;
    }
    while i < N {
        t[i] = own[i - 43];
        i += 1;
    }
    VTable(t)
}

static DOC_VTABLE: VTable<84> = with::<84, 41>([
    get_doctype as *const (),
    get_implementation as *const (),
    get_document_element as *const (),
    putref_document_element as *const (),
    create_element as *const (),
    create_document_fragment as *const (),
    create_text_node as *const (),
    create_comment as *const (),
    create_cdata_section as *const (),
    create_processing_instruction as *const (),
    create_attribute as *const (),
    create_entity_reference as *const (),
    doc_get_elements_by_tag_name as *const (),
    create_node as *const (),
    node_from_id as *const (),
    load as *const (),
    get_ready_state as *const (),
    get_parse_error as *const (),
    get_url as *const (),
    get_async as *const (),
    put_async as *const (),
    abort as *const (),
    load_xml as *const (),
    save as *const (),
    get_validate_on_parse as *const (),
    put_validate_on_parse as *const (),
    get_resolve_externals as *const (),
    put_resolve_externals as *const (),
    get_preserve_white_space as *const (),
    put_preserve_white_space as *const (),
    put_onreadystatechange as *const (),
    put_ondataavailable as *const (),
    put_ontransformnode as *const (),
    get_namespaces as *const (),
    schema::get_schemas as *const (),
    schema::putref_schemas as *const (),
    schema::validate as *const (),
    set_property as *const (),
    get_property as *const (),
    schema::validate_node as *const (),
    import_node as *const (),
]);

static ELEMENT_VTABLE: VTable<52> = with::<52, 9>([
    get_node_name as *const (), // get_tagName
    get_attribute as *const (),
    set_attribute as *const (),
    remove_attribute as *const (),
    get_attribute_node as *const (),
    set_attribute_node as *const (),
    remove_attribute_node as *const (),
    el_get_elements_by_tag_name as *const (),
    normalize as *const (),
]);

/// IXMLDOMDocumentFragment adds nothing to IXMLDOMNode.
static FRAGMENT_VTABLE: VTable<43> = with::<43, 0>([]);

static ATTRIBUTE_VTABLE: VTable<46> = with::<46, 3>([
    get_node_name as *const (), // get_name
    get_value as *const (),
    put_value as *const (),
]);

/// IXMLDOMText (and IXMLDOMCDATASection, the same layout).
static TEXT_VTABLE: VTable<52> = with::<52, 9>([
    get_data as *const (),
    put_data as *const (),
    get_length as *const (),
    substring_data as *const (),
    append_data as *const (),
    insert_data as *const (),
    delete_data as *const (),
    replace_data as *const (),
    split_text as *const (),
]);

static COMMENT_VTABLE: VTable<51> = with::<51, 8>([
    get_data as *const (),
    put_data as *const (),
    get_length as *const (),
    substring_data as *const (),
    append_data as *const (),
    insert_data as *const (),
    delete_data as *const (),
    replace_data as *const (),
]);

static PI_VTABLE: VTable<46> = with::<46, 3>([
    get_node_name as *const (), // get_target
    get_data as *const (),
    pi_put_data as *const (),
]);

// ---- Node lists (IXMLDOMNodeList, IXMLDOMSelection) ----

enum Items {
    /// A node's children, live.
    Children(u32),
    /// A selection's nodes.
    Fixed(Vec<u32>),
}

#[repr(C)]
struct ListObj {
    vtable: *const (),
    refs: AtomicU32,
    doc: *mut Doc,
    items: Items,
    cursor: usize,
}

fn list<'a>(this: Obj) -> &'a mut ListObj {
    // SAFETY: our list objects' interface pointers.
    unsafe { &mut *(this as *mut ListObj) }
}

fn new_list(owner: Obj, items: Items, out: *mut Obj) -> HResult {
    let d = node(owner).doc;
    let l = ListObj {
        vtable: &LIST_VTABLE as *const VTable<23> as *const (),
        refs: AtomicU32::new(1),
        doc: d,
        items,
        cursor: 0,
    };
    match try_box(l) {
        Some(p) => {
            node_add_ref(d as Obj);
            put(out, p as Obj);
            S_OK
        }
        None => E_OUTOFMEMORY,
    }
}

impl ListObj {
    fn doc(&mut self) -> &mut Doc {
        // SAFETY: the list holds a reference on its document.
        unsafe { &mut *self.doc }
    }

    fn get(&mut self, i: usize) -> Option<u32> {
        match &self.items {
            Items::Children(p) => {
                let p = *p;
                self.doc().tree.node(p).children.get(i).copied()
            }
            Items::Fixed(v) => v.get(i).copied(),
        }
    }

    fn len(&mut self) -> usize {
        match &self.items {
            Items::Children(p) => {
                let p = *p;
                self.doc().tree.node(p).children.len()
            }
            Items::Fixed(v) => v.len(),
        }
    }
}

extern "system" fn list_query(this: Obj, riid: *const Guid, out: *mut Obj) -> HResult {
    if out.is_null() || riid.is_null() {
        return E_POINTER;
    }
    // SAFETY: the caller's IID.
    let iid = unsafe { *riid };
    let selection = matches!(list(this).items, Items::Fixed(_));
    if iid == IID_IUNKNOWN
        || iid == IID_IDISPATCH
        || iid == IID_IXMLDOMNODELIST
        || (selection && iid == IID_IXMLDOMSELECTION)
    {
        list(this).refs.fetch_add(1, Ordering::Relaxed);
        put(out, this);
        return S_OK;
    }
    put(out, core::ptr::null_mut());
    crate::record_iid("QI? list", &iid);
    E_NOINTERFACE
}

extern "system" fn list_add_ref(this: Obj) -> u32 {
    list(this).refs.fetch_add(1, Ordering::Relaxed) + 1
}

extern "system" fn list_release(this: Obj) -> u32 {
    let l = list(this);
    let left = l.refs.fetch_sub(1, Ordering::AcqRel) - 1;
    if left == 0 {
        let d = l.doc;
        // SAFETY: the list's last reference.
        unsafe { free_box(this as *mut ListObj) };
        node_release(d as Obj);
    }
    left
}

extern "system" fn list_get_item(this: Obj, i: i32, out: *mut Obj) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    let l = list(this);
    let n = usize::try_from(i).ok().and_then(|i| l.get(i));
    give(l.doc(), n, out)
}

extern "system" fn list_get_length(this: Obj, out: *mut i32) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    put(out, list(this).len() as i32);
    S_OK
}

extern "system" fn list_next_node(this: Obj, out: *mut Obj) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    let l = list(this);
    let n = l.get(l.cursor);
    if n.is_some() {
        l.cursor += 1;
    }
    give(l.doc(), n, out)
}

extern "system" fn list_reset(this: Obj) -> HResult {
    list(this).cursor = 0;
    S_OK
}

not_implemented! {
    list_new_enum(*mut Obj) = b"IXMLDOMNodeList::get__newEnum";
    sel_get_expr(*mut Bstr) = b"IXMLDOMSelection::get_expr";
    sel_put_expr(Bstr) = b"IXMLDOMSelection::put_expr";
    sel_get_context(*mut Obj) = b"IXMLDOMSelection::get_context";
    sel_putref_context(Obj) = b"IXMLDOMSelection::putref_context";
    sel_peek_node(*mut Obj) = b"IXMLDOMSelection::peekNode";
    sel_matches(Obj, *mut Obj) = b"IXMLDOMSelection::matches";
    sel_remove_next(*mut Obj) = b"IXMLDOMSelection::removeNext";
    sel_remove_all() = b"IXMLDOMSelection::removeAll";
    sel_clone(*mut Obj) = b"IXMLDOMSelection::clone";
    sel_get_property(Bstr, *mut Variant) = b"IXMLDOMSelection::getProperty";
    sel_set_property(Bstr, Variant) = b"IXMLDOMSelection::setProperty";
}

static LIST_VTABLE: VTable<23> = VTable([
    list_query as *const (),
    list_add_ref as *const (),
    list_release as *const (),
    get_type_info_count as *const (),
    get_type_info as *const (),
    get_ids_of_names as *const (),
    invoke as *const (),
    list_get_item as *const (),
    list_get_length as *const (),
    list_next_node as *const (),
    list_reset as *const (),
    list_new_enum as *const (),
    sel_get_expr as *const (),
    sel_put_expr as *const (),
    sel_get_context as *const (),
    sel_putref_context as *const (),
    sel_peek_node as *const (),
    sel_matches as *const (),
    sel_remove_next as *const (),
    sel_remove_all as *const (),
    sel_clone as *const (),
    sel_get_property as *const (),
    sel_set_property as *const (),
]);

// ---- Attribute maps (IXMLDOMNamedNodeMap) ----

#[repr(C)]
struct MapObj {
    vtable: *const (),
    refs: AtomicU32,
    doc: *mut Doc,
    el: u32,
    cursor: usize,
}

fn map<'a>(this: Obj) -> &'a mut MapObj {
    // SAFETY: our map objects' interface pointers.
    unsafe { &mut *(this as *mut MapObj) }
}

impl MapObj {
    fn doc<'a>(&self) -> &'a mut Doc {
        // SAFETY: the map holds a reference on its document.
        unsafe { &mut *self.doc }
    }
    fn get(&self, i: usize) -> Option<u32> {
        self.doc().tree.node(self.el).attrs.get(i).copied()
    }
}

extern "system" fn map_query(this: Obj, riid: *const Guid, out: *mut Obj) -> HResult {
    if out.is_null() || riid.is_null() {
        return E_POINTER;
    }
    // SAFETY: the caller's IID.
    let iid = unsafe { *riid };
    if iid == IID_IUNKNOWN || iid == IID_IDISPATCH || iid == IID_IXMLDOMNAMEDNODEMAP {
        map(this).refs.fetch_add(1, Ordering::Relaxed);
        put(out, this);
        return S_OK;
    }
    put(out, core::ptr::null_mut());
    crate::record_iid("QI? map", &iid);
    E_NOINTERFACE
}

extern "system" fn map_add_ref(this: Obj) -> u32 {
    map(this).refs.fetch_add(1, Ordering::Relaxed) + 1
}

extern "system" fn map_release(this: Obj) -> u32 {
    let m = map(this);
    let left = m.refs.fetch_sub(1, Ordering::AcqRel) - 1;
    if left == 0 {
        let d = m.doc;
        // SAFETY: the map's last reference.
        unsafe { free_box(this as *mut MapObj) };
        node_release(d as Obj);
    }
    left
}

extern "system" fn map_get_named_item(this: Obj, name: Bstr, out: *mut Obj) -> HResult {
    if out.is_null() || name.is_null() {
        return E_INVALIDARG;
    }
    let m = map(this);
    let Some(name) = text(name) else { return E_OUTOFMEMORY };
    let a = find_attr(&m.doc().tree, m.el, &name);
    give(m.doc(), a, out)
}

extern "system" fn map_get_item(this: Obj, i: i32, out: *mut Obj) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    let m = map(this);
    let a = usize::try_from(i).ok().and_then(|i| m.get(i));
    give(m.doc(), a, out)
}

extern "system" fn map_get_length(this: Obj, out: *mut i32) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    let m = map(this);
    put(out, m.doc().tree.node(m.el).attrs.len() as i32);
    S_OK
}

extern "system" fn map_get_qualified_item(
    this: Obj,
    name: Bstr,
    ns: Bstr,
    out: *mut Obj,
) -> HResult {
    if out.is_null() || name.is_null() {
        return E_INVALIDARG;
    }
    let m = map(this);
    let (Some(name), ns) = (text(name), text(ns).unwrap_or_default()) else {
        return E_OUTOFMEMORY;
    };
    let t = &m.doc().tree;
    let a = t.node(m.el).attrs.iter().copied().find(|&a| {
        let n = t.node(a);
        n.local() == name && n.ns == ns
    });
    give(m.doc(), a, out)
}

extern "system" fn map_next_node(this: Obj, out: *mut Obj) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    let m = map(this);
    let a = m.get(m.cursor);
    if a.is_some() {
        m.cursor += 1;
    }
    give(m.doc(), a, out)
}

extern "system" fn map_reset(this: Obj) -> HResult {
    map(this).cursor = 0;
    S_OK
}

not_implemented! {
    map_new_enum(*mut Obj) = b"IXMLDOMNamedNodeMap::get__newEnum";
}

static MAP_VTABLE: VTable<17> = VTable([
    map_query as *const (),
    map_add_ref as *const (),
    map_release as *const (),
    get_type_info_count as *const (),
    get_type_info as *const (),
    get_ids_of_names as *const (),
    invoke as *const (),
    map_get_named_item as *const (),
    map_set_named_item as *const (),
    map_remove_named_item as *const (),
    map_get_item as *const (),
    map_get_length as *const (),
    map_get_qualified_item as *const (),
    map_remove_qualified_item as *const (),
    map_next_node as *const (),
    map_reset as *const (),
    map_new_enum as *const (),
]);

// ---- IXMLDOMParseError ----

/// The parse error as it was when asked for.
#[repr(C)]
struct ErrorObj {
    vtable: *const (),
    refs: AtomicU32,
    error: ParseError,
}

fn err<'a>(this: Obj) -> &'a ErrorObj {
    // SAFETY: our parse error objects' interface pointers.
    unsafe { &*(this as *const ErrorObj) }
}

extern "system" fn error_query(this: Obj, riid: *const Guid, out: *mut Obj) -> HResult {
    if out.is_null() || riid.is_null() {
        return E_POINTER;
    }
    // SAFETY: the caller's IID.
    let iid = unsafe { *riid };
    if iid == IID_IUNKNOWN || iid == IID_IDISPATCH || iid == IID_IXMLDOMPARSEERROR {
        err(this).refs.fetch_add(1, Ordering::Relaxed);
        put(out, this);
        return S_OK;
    }
    put(out, core::ptr::null_mut());
    crate::record_iid("QI? error", &iid);
    E_NOINTERFACE
}

extern "system" fn error_add_ref(this: Obj) -> u32 {
    err(this).refs.fetch_add(1, Ordering::Relaxed) + 1
}

extern "system" fn error_release(this: Obj) -> u32 {
    let left = err(this).refs.fetch_sub(1, Ordering::AcqRel) - 1;
    if left == 0 {
        // SAFETY: the object's last reference.
        unsafe { free_box(this as *mut ErrorObj) };
    }
    left
}

/// A number of the error: S_FALSE (and 0) when there was none.
fn error_number(this: Obj, out: *mut i32, v: i32) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    put(out, v);
    if err(this).error.code == 0 { S_FALSE } else { S_OK }
}

extern "system" fn error_code(this: Obj, out: *mut i32) -> HResult {
    error_number(this, out, err(this).error.code)
}

extern "system" fn error_line(this: Obj, out: *mut i32) -> HResult {
    error_number(this, out, err(this).error.line)
}

extern "system" fn error_linepos(this: Obj, out: *mut i32) -> HResult {
    error_number(this, out, err(this).error.linepos)
}

extern "system" fn error_filepos(this: Obj, out: *mut i32) -> HResult {
    error_number(this, out, err(this).error.filepos)
}

/// A string of the error: S_FALSE and NULL when there was none.
fn error_string(this: Obj, out: *mut Bstr, s: &str) -> HResult {
    if err(this).error.code == 0 {
        return give_str(out, None);
    }
    give_str(out, Some(s))
}

extern "system" fn error_url(this: Obj, out: *mut Bstr) -> HResult {
    error_string(this, out, &err(this).error.url)
}

extern "system" fn error_reason(this: Obj, out: *mut Bstr) -> HResult {
    error_string(this, out, &err(this).error.reason)
}

extern "system" fn error_src_text(this: Obj, out: *mut Bstr) -> HResult {
    error_string(this, out, &err(this).error.src)
}

static ERROR_VTABLE: VTable<14> = VTable([
    error_query as *const (),
    error_add_ref as *const (),
    error_release as *const (),
    get_type_info_count as *const (),
    get_type_info as *const (),
    get_ids_of_names as *const (),
    invoke as *const (),
    error_code as *const (),
    error_url as *const (),
    error_reason as *const (),
    error_src_text as *const (),
    error_line as *const (),
    error_linepos as *const (),
    error_filepos as *const (),
]);

mod schema;
pub use schema::new_cache;
pub use xslt::new_template;
