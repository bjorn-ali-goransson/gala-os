//! Changing the tree (`msxml_test`'s `mutation`): the create methods,
//! insertBefore, appendChild, removeChild, replaceChild, the attribute
//! methods and the attribute map's changes, put_text and put_nodeValue,
//! normalize, getElementsByTagName, importNode, putref_documentElement
//! and save, with MSXML's answers as Wine's msxml3 tests show them on
//! Windows.
//!
//! A node of another document that is inserted moves: it is copied into
//! this document's tree, taken from its old parent, and the object the
//! caller passed is pointed at the copy (it holds this document now), so
//! the caller's pointer is the inserted node, as with MSXML. Other
//! objects the program holds for that node still show the old copy, now
//! of no parent (MSXML has one object per node).

use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::Ordering;

use gala_xml::{XML_NS, XMLNS_NS};

use super::*;
use crate::{VT_DISPATCH, VT_EMPTY};

/// The node object behind `p` when it is one of this DLL's (a program
/// may pass any IXMLDOMNode; one gala did not make is not supported).
pub(super) fn ours<'a>(p: Obj) -> Option<&'a NodeObj> {
    if p.is_null() {
        return None;
    }
    // SAFETY: an interface pointer: its first field is a vtable pointer.
    let vtable = unsafe { *(p as *const *const ()) };
    let known = [
        &DOC_VTABLE as *const VTable<84> as *const (),
        &ELEMENT_VTABLE as *const VTable<52> as *const (),
        &ATTRIBUTE_VTABLE as *const VTable<46> as *const (),
        &TEXT_VTABLE as *const VTable<52> as *const (),
        &COMMENT_VTABLE as *const VTable<51> as *const (),
        &PI_VTABLE as *const VTable<46> as *const (),
        &FRAGMENT_VTABLE as *const VTable<43> as *const (),
    ];
    if known.contains(&vtable) {
        Some(node(p))
    } else {
        record(b"IXMLDOMNode(a node msxml6 did not make)", LEDGER_UNSUPPORTED, 0);
        None
    }
}

/// The node behind an object of ours: its tree and index.
fn tree_of(n: &NodeObj) -> &Tree {
    // SAFETY: a node object holds a reference on its document.
    unsafe { &(*n.doc).tree }
}

fn kind_of(n: &NodeObj) -> Kind {
    tree_of(n).node(n.idx).kind
}

/// Whether a node of kind `parent` may have a child of kind `child`.
fn may_contain(parent: Kind, child: Kind) -> bool {
    match parent {
        Kind::Element | Kind::Fragment => matches!(
            child,
            Kind::Element | Kind::Text | Kind::CData | Kind::Comment | Kind::Pi | Kind::Fragment
        ),
        Kind::Document => {
            matches!(child, Kind::Element | Kind::Comment | Kind::Pi | Kind::Fragment)
        }
        Kind::Attribute => child == Kind::Text,
        _ => false,
    }
}

/// The index of `p`'s node in document `d`: a node of another document
/// is copied into `d` (deep), taken from its old parent, and `p` is
/// pointed at the copy.
fn adopt(d: *mut Doc, p: Obj) -> u32 {
    let n = node(p);
    if n.doc == d {
        return n.idx;
    }
    let old = n.doc;
    // SAFETY: two live documents (each held: `d` by the caller's object,
    // `old` by `p`); one thread at a time (rental).
    let (to, from) = unsafe { (&mut *d, &mut *old) };
    let copy = to.tree.import(&from.tree, n.idx, true);
    from.tree.detach(n.idx);
    let w = p as *mut NodeObj;
    to.obj.refs.fetch_add(1, Ordering::Relaxed);
    // SAFETY: our node object; nothing else reads it while it changes.
    unsafe {
        (*w).doc = d;
        (*w).idx = copy;
    }
    node_release(old as Obj);
    copy
}

/// Puts `new` under `this` before child `before` (None: at the end),
/// in place of child `old` when one is given. The checks and their
/// order are MSXML 6's, measured on Windows (Wine's msxml3 tests expect
/// E_FAIL for each kind refusal): a node that is `this` or its ancestor
/// is E_FAIL (a document under itself too), then a document anywhere
/// and a kind `this` cannot hold are E_INVALIDARG; a document's second
/// element is E_FAIL.
fn place(this: Obj, new: Obj, before: Option<u32>, old: Option<u32>) -> HResult {
    let Some(nn) = ours(new) else { return E_INVALIDARG };
    let d_ptr = node(this).doc;
    let p = idx(this);
    let pk = doc(this).tree.node(p).kind;
    let nk = kind_of(nn);
    if nn.doc == d_ptr && doc(this).tree.contains(nn.idx, p) {
        return E_FAIL;
    }
    if nk == Kind::Document {
        return E_INVALIDARG;
    }
    // The kinds that arrive: a fragment's children, or the node.
    let src = tree_of(nn);
    let mut kinds: Vec<Kind> = Vec::new();
    if nk == Kind::Fragment {
        kinds.extend(src.node(nn.idx).children.iter().map(|&c| src.node(c).kind));
    } else {
        kinds.push(nk);
    }
    if !kinds.iter().all(|&k| may_contain(pk, k)) {
        return E_INVALIDARG;
    }
    if pk == Kind::Document {
        let t = &doc(this).tree;
        let moving = (nn.doc == d_ptr).then_some(nn.idx);
        let present = t
            .node(0)
            .children
            .iter()
            .any(|&c| t.node(c).kind == Kind::Element && Some(c) != old && Some(c) != moving);
        let arriving = kinds.iter().filter(|&&k| k == Kind::Element).count();
        if arriving > 1 || (arriving == 1 && present) {
            return E_FAIL;
        }
    }
    if pk == Kind::Attribute {
        // An attribute's value is its text: the text node's is added to
        // it, and the text node is taken from where it was.
        let text = src.node(nn.idx).value.clone();
        // SAFETY: the new node's document, held by `new`.
        unsafe { (*nn.doc).tree.detach(nn.idx) };
        doc(this).tree.nodes[p as usize].value.push_str(&text);
        return S_OK;
    }
    let ni = adopt(d_ptr, new);
    let t = &mut doc(this).tree;
    if old == Some(ni) || before == Some(ni) {
        return S_OK;
    }
    if t.node(ni).kind == Kind::Fragment {
        let kids = t.node(ni).children.clone();
        for k in kids {
            t.detach(k);
            t.insert(p, k, before);
        }
    } else {
        t.detach(ni);
        t.insert(p, ni, before);
    }
    if let Some(o) = old {
        t.detach(o);
    }
    S_OK
}

/// `obj` with one more reference (the output of a method that returns
/// a node it was given).
fn again(obj: Obj) -> Obj {
    node_add_ref(obj);
    obj
}

/// The child of `this` that `p` is (None when it is not one).
fn child_of(this: Obj, p: Obj) -> Option<u32> {
    let c = ours(p)?;
    let t = &doc(this).tree;
    (c.doc == node(this).doc
        && c.idx != 0
        && t.node(c.idx).kind != Kind::Attribute
        && t.node(c.idx).parent == idx(this))
    .then_some(c.idx)
}

/// IXMLDOMNode::insertBefore(new, before, out): NULL is E_INVALIDARG
/// (the output left alone); a reference that is not a child
/// E_INVALIDARG; otherwise `place`'s answer, the output `new`.
pub(super) extern "system" fn insert_before(
    this: Obj,
    new: Obj,
    before: Variant,
    out: *mut Obj,
) -> HResult {
    if new.is_null() {
        return E_INVALIDARG;
    }
    put(out, core::ptr::null_mut());
    let reference = match before.vt {
        VT_EMPTY | VT_NULL => None,
        VT_DISPATCH | VT_UNKNOWN if before.value[0] == 0 => None,
        VT_DISPATCH | VT_UNKNOWN => match child_of(this, before.value[0] as Obj) {
            Some(i) => Some(i),
            None => return E_INVALIDARG,
        },
        other => {
            record(b"IXMLDOMNode::insertBefore(reference type)", LEDGER_UNSUPPORTED, other.into());
            return E_INVALIDARG;
        }
    };
    let hr = place(this, new, reference, None);
    if hr == S_OK {
        put(out, again(new));
    }
    hr
}

pub(super) extern "system" fn append_child(this: Obj, new: Obj, out: *mut Obj) -> HResult {
    if new.is_null() {
        return E_INVALIDARG;
    }
    put(out, core::ptr::null_mut());
    let hr = place(this, new, None, None);
    if hr == S_OK {
        put(out, again(new));
    }
    hr
}

/// IXMLDOMNode::removeChild(old, out): NULL is E_INVALIDARG (the output
/// left alone), a node that is not a child E_INVALIDARG (the output
/// NULL); the output is `old`, now of no parent.
pub(super) extern "system" fn remove_child(this: Obj, old: Obj, out: *mut Obj) -> HResult {
    if old.is_null() {
        return E_INVALIDARG;
    }
    put(out, core::ptr::null_mut());
    let Some(i) = child_of(this, old) else { return E_INVALIDARG };
    doc(this).tree.detach(i);
    put(out, again(old));
    S_OK
}

/// IXMLDOMNode::replaceChild(new, old, out): NULL for either is
/// E_INVALIDARG (the output left alone), an `old` that is not a child
/// E_INVALIDARG (the output NULL), then `place`'s checks; the output is
/// `old`.
pub(super) extern "system" fn replace_child(
    this: Obj,
    new: Obj,
    old: Obj,
    out: *mut Obj,
) -> HResult {
    if new.is_null() || old.is_null() {
        return E_INVALIDARG;
    }
    put(out, core::ptr::null_mut());
    let Some(i) = child_of(this, old) else { return E_INVALIDARG };
    let hr = place(this, new, Some(i), Some(i));
    if hr == S_OK {
        put(out, again(old));
    }
    hr
}

/// A variant's value as text (strings and numbers; None for others).
pub(super) fn variant_text(v: &Variant) -> Option<String> {
    match v.vt {
        VT_BSTR => Some(text(v.value[0] as *const u16).unwrap_or_default()),
        // SAFETY: a VT_BSTR | VT_BYREF variant points at a BSTR.
        x if x == VT_BSTR | VT_BYREF => {
            Some(text(unsafe { *(v.value[0] as *const *const u16) }).unwrap_or_default())
        }
        VT_I2 | VT_I4 | VT_INT | VT_UI4 => {
            let n = super::variant_int(v)?;
            Some(alloc::format!("{n}"))
        }
        _ => None,
    }
}

/// Sets the value of node `i` as put_text and put_nodeValue do: E_FAIL
/// for a namespace declaration on an element (MSXML will not change one
/// in place); a declaration of no element yet takes its value (Wine's
/// test_xmlns_attribute, before setAttributeNode).
pub(super) fn set_value(d: &mut Doc, i: u32, value: String) -> HResult {
    let n = d.tree.node(i);
    if n.is_xmlns() && n.parent != NONE {
        return E_FAIL;
    }
    d.tree.nodes[i as usize].value = value;
    S_OK
}

/// The children of element or fragment `i` become one text node holding
/// `value` (none for an empty value; the old children are left of no
/// parent).
pub(super) fn replace_children(d: &mut Doc, i: u32, value: &str) {
    let kids = d.tree.node(i).children.clone();
    for k in kids {
        d.tree.detach(k);
    }
    if !value.is_empty() {
        let t = d.tree.create(Kind::Text, "", "", value);
        d.tree.insert(i, t, None);
    }
}

/// IXMLDOMNode::put_text: an element's or fragment's children become one
/// text node (none for an empty text; the old children are left of no
/// parent); other nodes' value. The document's is not supported
/// (recorded, E_FAIL).
pub(super) extern "system" fn put_text(this: Obj, s: Bstr) -> HResult {
    let value = text(s).unwrap_or_default();
    let d = doc(this);
    let i = idx(this);
    match d.tree.node(i).kind {
        Kind::Element | Kind::Fragment => {
            replace_children(d, i, &value);
            S_OK
        }
        Kind::Document => {
            record(b"IXMLDOMNode::put_text(the document)", LEDGER_UNSUPPORTED, 0);
            E_FAIL
        }
        _ => set_value(d, i, value),
    }
}

/// IXMLDOMNode::put_nodeValue: documents, elements and fragments (they
/// have no value) are E_INVALIDARG (MSXML 6 measured on Windows; Wine's
/// msxml3 tests expect E_FAIL); strings and numbers otherwise.
pub(super) extern "system" fn put_node_value(this: Obj, v: Variant) -> HResult {
    let d = doc(this);
    let i = idx(this);
    if matches!(d.tree.node(i).kind, Kind::Element | Kind::Document | Kind::Fragment) {
        return E_INVALIDARG;
    }
    match variant_text(&v) {
        Some(value) => set_value(d, i, value),
        None => {
            record(b"IXMLDOMNode::put_nodeValue(variant type)", LEDGER_UNSUPPORTED, v.vt.into());
            E_INVALIDARG
        }
    }
}

/// IXMLDOMAttribute::put_value (as put_nodeValue).
pub(super) extern "system" fn put_value(this: Obj, v: Variant) -> HResult {
    put_node_value(this, v)
}

// ---- Creating nodes ----

/// The namespace an attribute named `name` has by its prefix alone.
fn attribute_ns(name: &str) -> &'static str {
    if name == "xmlns" || name.starts_with("xmlns:") {
        XMLNS_NS
    } else if name.starts_with("xml:") {
        XML_NS
    } else {
        ""
    }
}

/// A new node of no parent, given to the caller.
fn create(this: Obj, kind: Kind, name: &str, ns: &str, value: &str, out: *mut Obj) -> HResult {
    let d = doc(this);
    let i = d.tree.create(kind, name, ns, value);
    give(d, Some(i), out)
}

/// A name argument: NULL is E_INVALIDARG and empty E_FAIL (Wine's
/// test_create on Windows).
fn name_arg(s: Bstr) -> Result<String, HResult> {
    match text(s) {
        None if s.is_null() => Err(E_INVALIDARG),
        None => Err(E_OUTOFMEMORY),
        Some(n) if n.is_empty() => Err(E_FAIL),
        Some(n) => Ok(n),
    }
}

/// IXMLDOMDocument::createElement: a refused name sets the output to
/// NULL (MSXML 6 measured on Windows; Wine's msxml3 tests expect it left
/// alone).
pub(super) extern "system" fn create_element(this: Obj, name: Bstr, out: *mut Obj) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: checked non-NULL above; the caller's out pointer.
    unsafe { *out = core::ptr::null_mut() };
    match name_arg(name) {
        Ok(n) => create(this, Kind::Element, &n, "", "", out),
        Err(hr) => hr,
    }
}

pub(super) extern "system" fn create_document_fragment(this: Obj, out: *mut Obj) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    create(this, Kind::Fragment, "", "", "", out)
}

fn create_data(this: Obj, kind: Kind, data: Bstr, out: *mut Obj) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    let value = text(data).unwrap_or_default();
    create(this, kind, "", "", &value, out)
}

pub(super) extern "system" fn create_text_node(this: Obj, data: Bstr, out: *mut Obj) -> HResult {
    create_data(this, Kind::Text, data, out)
}

pub(super) extern "system" fn create_comment(this: Obj, data: Bstr, out: *mut Obj) -> HResult {
    create_data(this, Kind::Comment, data, out)
}

pub(super) extern "system" fn create_cdata_section(
    this: Obj,
    data: Bstr,
    out: *mut Obj,
) -> HResult {
    create_data(this, Kind::CData, data, out)
}

pub(super) extern "system" fn create_processing_instruction(
    this: Obj,
    target: Bstr,
    data: Bstr,
    out: *mut Obj,
) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    match name_arg(target) {
        Ok(t) => create(this, Kind::Pi, &t, "", &text(data).unwrap_or_default(), out),
        Err(hr) => hr,
    }
}

pub(super) extern "system" fn create_attribute(this: Obj, name: Bstr, out: *mut Obj) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    match name_arg(name) {
        Ok(n) => create(this, Kind::Attribute, &n, attribute_ns(&n), "", out),
        Err(hr) => hr,
    }
}

/// The node types createNode takes as names (MSXML's nodeTypeString).
fn type_by_name(name: &str) -> Option<i64> {
    let types = [
        ("element", 1),
        ("attribute", 2),
        ("text", 3),
        ("cdatasection", 4),
        ("entityreference", 5),
        ("processinginstruction", 7),
        ("comment", 8),
        ("document", 9),
        ("documentfragment", 11),
    ];
    types.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|&(_, t)| t)
}

/// IXMLDOMDocument::createNode(type, name, namespaceURI, out): the type
/// a number (any integer variant) or its name; the document, document
/// types, entities and notations are E_INVALIDARG; a refusal sets the
/// output to NULL first, as createElement's does on Windows (Wine's
/// msxml3 tests expect it left alone); entity references are not
/// supported (recorded, E_NOTIMPL).
pub(super) extern "system" fn create_node(
    this: Obj,
    ty: Variant,
    name: Bstr,
    ns: Bstr,
    out: *mut Obj,
) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: checked non-NULL above; the caller's out pointer.
    unsafe { *out = core::ptr::null_mut() };
    let kind = match ty.vt {
        VT_BSTR => text(ty.value[0] as *const u16).as_deref().and_then(type_by_name),
        // VT_I1 and VT_UI1 (a byte), the wider integers.
        16 | 17 => Some(i64::from(ty.value[0] as u8 as i8)),
        _ => super::variant_int(&ty),
    };
    let uri = text(ns).unwrap_or_default();
    match kind {
        Some(1) => match name_arg(name) {
            Ok(n) => create(this, Kind::Element, &n, &uri, "", out),
            Err(hr) => hr,
        },
        Some(2) => match name_arg(name) {
            Ok(n) => {
                let ns = if uri.is_empty() { attribute_ns(&n) } else { uri.as_str() };
                create(this, Kind::Attribute, &n, ns, "", out)
            }
            Err(hr) => hr,
        },
        Some(3) => create(this, Kind::Text, "", "", "", out),
        Some(4) => create(this, Kind::CData, "", "", "", out),
        Some(7) => match name_arg(name) {
            Ok(t) => create(this, Kind::Pi, &t, "", "", out),
            Err(hr) => hr,
        },
        Some(8) => create(this, Kind::Comment, "", "", "", out),
        Some(11) => create(this, Kind::Fragment, "", "", "", out),
        Some(5) => {
            record(b"IXMLDOMDocument::createNode(entity reference)", LEDGER_UNSUPPORTED, 5);
            crate::E_NOTIMPL
        }
        _ => E_INVALIDARG,
    }
}

/// IXMLDOMDocument::putref_documentElement: the element replaces the
/// document element (or becomes it).
pub(super) extern "system" fn putref_document_element(this: Obj, el: Obj) -> HResult {
    if el.is_null() {
        return E_INVALIDARG;
    }
    match ours(el) {
        Some(n) if kind_of(n) == Kind::Element => {}
        _ => return E_INVALIDARG,
    }
    let root = doc(this).tree.root();
    place(this, el, root, root)
}

/// IXMLDOMDocument3::importNode(node, deep, out): a copy of the node
/// (of this document or another) owned by this document, of no parent.
/// The document itself is E_FAIL.
pub(super) extern "system" fn import_node(this: Obj, n: Obj, deep: i16, out: *mut Obj) -> HResult {
    if out.is_null() || n.is_null() {
        return E_INVALIDARG;
    }
    put(out, core::ptr::null_mut());
    let Some(src) = ours(n) else { return E_INVALIDARG };
    if src.idx == 0 {
        return E_FAIL;
    }
    let d = doc(this);
    let copy = if src.doc == node(this).doc {
        d.tree.copy(src.idx, deep != 0, NONE)
    } else {
        d.tree.import(tree_of(src), src.idx, deep != 0)
    };
    give(d, Some(copy), out)
}

// ---- Attributes ----

/// IXMLDOMElement::setAttribute(name, value): an attribute there gets
/// the value (a namespace declaration cannot change: E_FAIL, Wine's
/// test_setAttribute); a new one's namespace is its prefix's in scope.
pub(super) extern "system" fn set_attribute(this: Obj, name: Bstr, value: Variant) -> HResult {
    let name = match name_arg(name) {
        Ok(n) => n,
        Err(hr) => return hr,
    };
    let Some(value) = variant_text(&value) else {
        record(b"IXMLDOMElement::setAttribute(variant type)", LEDGER_UNSUPPORTED, value.vt.into());
        return E_INVALIDARG;
    };
    let d = doc(this);
    let el = idx(this);
    if let Some(a) = find_attr(&d.tree, el, &name) {
        return set_value(d, a, value);
    }
    let mut ns: String = attribute_ns(&name).into();
    if ns.is_empty()
        && let Some((prefix, _)) = name.split_once(':')
    {
        ns = d.tree.lookup(el, prefix).into();
    }
    let a = d.tree.create(Kind::Attribute, &name, &ns, &value);
    d.tree.insert(el, a, None);
    S_OK
}

/// IXMLDOMElement::removeAttribute(name): S_OK when removed, S_FALSE
/// when the element has no such attribute.
pub(super) extern "system" fn remove_attribute(this: Obj, name: Bstr) -> HResult {
    let name = match name_arg(name) {
        Ok(n) => n,
        Err(hr) => return hr,
    };
    let d = doc(this);
    match find_attr(&d.tree, idx(this), &name) {
        Some(a) => {
            d.tree.detach(a);
            S_OK
        }
        None => S_FALSE,
    }
}

/// Puts attribute `attr` on element `el` of `d`, in place of one of the
/// same name (returned).
fn set_attribute_at(d: *mut Doc, el: u32, attr: Obj) -> Option<u32> {
    let a = adopt(d, attr);
    // SAFETY: the element's document, held by the caller's object.
    let t = unsafe { &mut (*d).tree };
    let name = t.node(a).name.clone();
    let old = find_attr(t, el, &name);
    if let Some(o) = old {
        let pos = t.node(el).attrs.iter().position(|&x| x == o).unwrap_or(0);
        t.detach(o);
        t.nodes[el as usize].attrs.insert(pos, a);
        t.nodes[a as usize].parent = el;
    } else {
        t.insert(el, a, None);
    }
    old
}

/// The attribute object `p` when it is one of ours: Err(E_INVALIDARG)
/// for another node, Err(E_FAIL) for an attribute on an element already
/// (Wine's test_setAttributeNode).
fn free_attribute(p: Obj) -> Result<(), HResult> {
    let Some(n) = ours(p) else { return Err(E_INVALIDARG) };
    if kind_of(n) != Kind::Attribute {
        return Err(E_INVALIDARG);
    }
    if tree_of(n).node(n.idx).parent != NONE {
        return Err(E_FAIL);
    }
    Ok(())
}

/// IXMLDOMElement::setAttributeNode(attr, out): NULL is E_INVALIDARG and
/// an attribute on an element already E_FAIL (the output left alone);
/// the output is the attribute it replaced, or NULL.
pub(super) extern "system" fn set_attribute_node(this: Obj, attr: Obj, out: *mut Obj) -> HResult {
    if attr.is_null() {
        return E_INVALIDARG;
    }
    if let Err(hr) = free_attribute(attr) {
        return hr;
    }
    put(out, core::ptr::null_mut());
    let old = set_attribute_at(node(this).doc, idx(this), attr);
    if old.is_some() && !out.is_null() {
        return give(doc(this), old, out);
    }
    S_OK
}

/// IXMLDOMElement::removeAttributeNode(attr, out): the output is the
/// attribute, now of no element; one the element does not have is
/// E_INVALIDARG.
pub(super) extern "system" fn remove_attribute_node(
    this: Obj,
    attr: Obj,
    out: *mut Obj,
) -> HResult {
    if attr.is_null() {
        return E_INVALIDARG;
    }
    put(out, core::ptr::null_mut());
    let Some(a) = ours(attr) else { return E_INVALIDARG };
    let t = &doc(this).tree;
    if a.doc != node(this).doc
        || t.node(a.idx).kind != Kind::Attribute
        || t.node(a.idx).parent != idx(this)
    {
        return E_INVALIDARG;
    }
    doc(this).tree.detach(a.idx);
    put(out, again(attr));
    S_OK
}

/// Element nodes under `from` (not itself) named `name` ("*": any), in
/// document order.
fn elements_named(t: &Tree, from: u32, name: &str) -> Vec<u32> {
    let mut found = Vec::new();
    let mut stack: Vec<u32> = t.node(from).children.iter().rev().copied().collect();
    while let Some(i) = stack.pop() {
        let n = t.node(i);
        if n.kind != Kind::Element {
            continue;
        }
        if name == "*" || n.name == name {
            found.push(i);
        }
        stack.extend(n.children.iter().rev());
    }
    found
}

/// getElementsByTagName (the document's and an element's): the elements
/// below, a list made when asked (MSXML's is live).
fn elements_by_tag_name(this: Obj, name: Bstr, out: *mut Obj) -> HResult {
    if out.is_null() || name.is_null() {
        return E_INVALIDARG;
    }
    let Some(name) = text(name) else { return E_OUTOFMEMORY };
    let found = elements_named(&doc(this).tree, idx(this), &name);
    new_list(this, Items::Fixed(found), out)
}

pub(super) extern "system" fn doc_get_elements_by_tag_name(
    this: Obj,
    name: Bstr,
    out: *mut Obj,
) -> HResult {
    elements_by_tag_name(this, name, out)
}

pub(super) extern "system" fn el_get_elements_by_tag_name(
    this: Obj,
    name: Bstr,
    out: *mut Obj,
) -> HResult {
    elements_by_tag_name(this, name, out)
}

/// IXMLDOMElement::normalize: adjacent text nodes in the element's
/// subtree become one; empty ones go (as the W3C DOM's normalize).
pub(super) extern "system" fn normalize(this: Obj) -> HResult {
    doc(this).tree.normalize(idx(this));
    S_OK
}

// ---- The attribute map's changes ----

/// IXMLDOMNamedNodeMap::setNamedItem(attr, out): the attribute goes on
/// the map's element in place of one of its name; the output is the
/// attribute set (Wine's test_element_setNamedItem).
pub(super) extern "system" fn map_set_named_item(this: Obj, attr: Obj, out: *mut Obj) -> HResult {
    if attr.is_null() {
        return E_INVALIDARG;
    }
    if let Err(hr) = free_attribute(attr) {
        return hr;
    }
    let m = map(this);
    set_attribute_at(m.doc, m.el, attr);
    put(out, again(attr));
    S_OK
}

/// Takes attribute `a` (if any) off the map's element and gives it:
/// S_FALSE and NULL when there is none.
fn take(this: Obj, a: Option<u32>, out: *mut Obj) -> HResult {
    let m = map(this);
    let d = m.doc();
    if let Some(a) = a {
        d.tree.detach(a);
    }
    if out.is_null() {
        return if a.is_some() { S_OK } else { S_FALSE };
    }
    give(d, a, out)
}

pub(super) extern "system" fn map_remove_named_item(
    this: Obj,
    name: Bstr,
    out: *mut Obj,
) -> HResult {
    if name.is_null() {
        return E_INVALIDARG;
    }
    let Some(name) = text(name) else { return E_OUTOFMEMORY };
    let m = map(this);
    let a = find_attr(&m.doc().tree, m.el, &name);
    take(this, a, out)
}

pub(super) extern "system" fn map_remove_qualified_item(
    this: Obj,
    name: Bstr,
    ns: Bstr,
    out: *mut Obj,
) -> HResult {
    if name.is_null() {
        return E_INVALIDARG;
    }
    let (Some(name), ns) = (text(name), text(ns).unwrap_or_default()) else {
        return E_OUTOFMEMORY;
    };
    let m = map(this);
    let t = &m.doc().tree;
    let a = t.node(m.el).attrs.iter().copied().find(|&a| {
        let n = t.node(a);
        n.local() == name && n.ns == ns
    });
    take(this, a, out)
}

// ---- save ----

#[cfg_attr(
    target_arch = "x86",
    link(name = "kernel32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "kernel32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn WriteFile(h: usize, buf: *const u8, len: u32, written: *mut u32, overlapped: usize) -> i32;
    fn GetLastError() -> u32;
}

/// Whether the document's XML declaration names UTF-16.
fn declares_utf16(t: &Tree) -> bool {
    t.node(0).children.first().is_some_and(|&c| {
        let n = t.node(c);
        n.kind == Kind::Pi && n.name == "xml" && {
            let v = n.value.to_ascii_lowercase();
            v.contains("encoding=\"utf-16\"") || v.contains("encoding='utf-16'")
        }
    })
}

/// IXMLDOMDocument::save(destination): to a file named by a string, the
/// document's xml in UTF-8, or in UTF-16 (little-endian, with a BOM)
/// when its declaration names UTF-16. Other destinations (streams, other
/// documents) are not supported (recorded, E_NOTIMPL).
pub(super) extern "system" fn save(this: Obj, dest: Variant) -> HResult {
    let p = match dest.vt {
        VT_BSTR => dest.value[0] as *const u16,
        // SAFETY: a VT_BSTR | VT_BYREF variant points at a BSTR.
        v if v == VT_BSTR | VT_BYREF => unsafe { *(dest.value[0] as *const *const u16) },
        other => {
            record(b"IXMLDOMDocument::save(destination type)", LEDGER_UNSUPPORTED, other.into());
            return crate::E_NOTIMPL;
        }
    };
    let Some(path) = text(p) else { return E_INVALIDARG };
    let t = &doc(this).tree;
    let mut xml = String::new();
    t.xml(0, &mut xml);
    let mut bytes: Vec<u8> = Vec::new();
    if declares_utf16(t) {
        if bytes.try_reserve(2 + 2 * xml.len()).is_err() {
            return E_OUTOFMEMORY;
        }
        bytes.extend([0xff, 0xfe]);
        bytes.extend(xml.encode_utf16().flat_map(u16::to_le_bytes));
    } else {
        bytes = xml.into_bytes();
    }
    let mut wide: Vec<u16> = Vec::new();
    if wide.try_reserve(path.len() + 1).is_err() {
        return E_OUTOFMEMORY;
    }
    wide.extend(path.encode_utf16());
    wide.push(0);
    // SAFETY: a NUL-terminated path; GENERIC_WRITE, no sharing,
    // CREATE_ALWAYS.
    let h = unsafe { CreateFileW(wide.as_ptr(), 0x4000_0000, 0, 0, 2, 0x80, 0) };
    if h == usize::MAX {
        // SAFETY: plain call.
        let e = unsafe { GetLastError() };
        return (0x8007_0000 | (e & 0xffff)) as i32;
    }
    let mut written = 0u32;
    // SAFETY: our handle and buffer.
    let ok = unsafe { WriteFile(h, bytes.as_ptr(), bytes.len() as u32, &mut written, 0) } != 0
        && written as usize == bytes.len();
    // SAFETY: plain call.
    let e = if ok { 0 } else { unsafe { GetLastError() } };
    // SAFETY: our handle.
    unsafe { CloseHandle(h) };
    if ok { S_OK } else { (0x8007_0000 | (e & 0xffff)) as i32 }
}
