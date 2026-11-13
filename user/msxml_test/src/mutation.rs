//! Changing the tree: what Click-to-Run's App-V orchestration does when
//! it merges Office's AppX manifests (ROADMAP OF), and the rest of the
//! DOM's mutation family. Expected values are Wine's msxml3 tests
//! (dlls/msxml3/tests/domdoc.c, run on Windows: test_removeChild,
//! test_replaceChild, test_insertBefore, test_appendChild,
//! test_createElement, test_createNode, test_create, test_setAttribute,
//! test_setAttributeNode, test_removeNamedItem, test_removeQualifiedItem,
//! test_element_setNamedItem, test_put_text, test_put_nodeValue,
//! test_default_namespace, test_doc_fragment, test_getElementsByTagName)
//! and MSXML's documentation; where neither settles a value, it is an
//! `info` line (removeAttribute and removeAttributeNode of what the
//! element does not have, normalize's empty text, importNode's NULL,
//! the bytes `save` writes).

use core::ptr::null_mut;

use super::*;

const VT_DISPATCH: u16 = 9;
const VT_UNKNOWN: u16 = 13;
const VT_I1: u16 = 16;

const PUT_NODE_VALUE: usize = 9;
const INSERT_BEFORE: usize = 18;
const REPLACE_CHILD: usize = 19;
const REMOVE_CHILD: usize = 20;
const APPEND_CHILD: usize = 21;
const PUT_TEXT: usize = 27;

const PUTREF_DOCUMENT_ELEMENT: usize = 46;
const CREATE_ELEMENT: usize = 47;
const CREATE_DOCUMENT_FRAGMENT: usize = 48;
const CREATE_TEXT_NODE: usize = 49;
const CREATE_COMMENT: usize = 50;
const CREATE_PROCESSING_INSTRUCTION: usize = 52;
const CREATE_ATTRIBUTE: usize = 53;
const DOC_GET_ELEMENTS_BY_TAG_NAME: usize = 55;
const CREATE_NODE: usize = 56;
const SAVE: usize = 66;
const IMPORT_NODE: usize = 83;

const SET_ATTRIBUTE: usize = 45;
const REMOVE_ATTRIBUTE: usize = 46;
const SET_ATTRIBUTE_NODE: usize = 48;
const REMOVE_ATTRIBUTE_NODE: usize = 49;
const EL_GET_ELEMENTS_BY_TAG_NAME: usize = 50;
const NORMALIZE: usize = 51;

const MAP_SET_NAMED_ITEM: usize = 8;
const MAP_REMOVE_NAMED_ITEM: usize = 9;
const MAP_REMOVE_QUALIFIED_ITEM: usize = 13;

// DOMNodeType.
const NODE_ELEMENT: i32 = 1;
const NODE_ATTRIBUTE: i32 = 2;
const NODE_TEXT: i32 = 3;
const NODE_CDATA_SECTION: i32 = 4;
const NODE_ENTITY: i32 = 6;
const NODE_PROCESSING_INSTRUCTION: i32 = 7;
const NODE_COMMENT: i32 = 8;
const NODE_DOCUMENT: i32 = 9;
const NODE_DOCUMENT_FRAGMENT: i32 = 11;

/// A pointer no call should write: an output left alone keeps it.
const UNTOUCHED: Obj = 0x1 as Obj;

const IID_IXMLDOMDOCUMENTFRAGMENT: Guid =
    guid(0x3efa_a413, 0x272f, 0x11d2, [0x83, 0x6f, 0x00, 0x00, 0xf8, 0x7a, 0x77, 0x82]);

fn null_variant() -> Variant {
    Variant { vt: VT_NULL, reserved: [0; 3], value: [0; 2] }
}

fn dispatch(obj: Obj) -> Variant {
    Variant { vt: VT_DISPATCH, reserved: [0; 3], value: [obj as usize, 0] }
}

fn new_doc(c: &mut Checks) -> Result<Obj, u32> {
    let mut doc: Obj = null_mut();
    // SAFETY: valid GUIDs and output.
    let hr = unsafe {
        CoCreateInstance(
            &CLSID_DOMDOCUMENT60,
            null_mut(),
            CLSCTX_INPROC_SERVER,
            &IID_IXMLDOMDOCUMENT2,
            &mut doc,
        )
    };
    c.hr(hr, S_OK)?;
    Ok(doc)
}

/// createElement, createTextNode, createComment or createAttribute.
fn create(c: &mut Checks, doc: Obj, slot: usize, name: &str) -> Result<Obj, u32> {
    let n = Wide::new(name);
    let mut out: Obj = null_mut();
    c.hr(call!(doc, slot, fn(Bstr, *mut Obj) -> HResult, n.0, &mut out), S_OK)?;
    c.ok(!out.is_null(), 0)?;
    Ok(out)
}

/// createNode(VT_I1 type, name, namespace) with the output starting as
/// UNTOUCHED.
fn create_node(doc: Obj, kind: i32, name: Option<&str>, ns: Option<&str>) -> (HResult, Obj) {
    let n = name.map(Wide::new);
    let u = ns.map(Wide::new);
    let ty = Variant { vt: VT_I1, reserved: [0; 3], value: [kind as u8 as usize, 0] };
    let mut out = UNTOUCHED;
    let hr = call!(
        doc,
        CREATE_NODE,
        fn(Variant, Bstr, Bstr, *mut Obj) -> HResult,
        ty,
        n.as_ref().map_or(null_mut(), |w| w.0),
        u.as_ref().map_or(null_mut(), |w| w.0),
        &mut out
    );
    (hr, out)
}

fn insert_before(parent: Obj, new: Obj, before: Variant) -> (HResult, Obj) {
    let mut out = UNTOUCHED;
    let hr =
        call!(parent, INSERT_BEFORE, fn(Obj, Variant, *mut Obj) -> HResult, new, before, &mut out);
    (hr, out)
}

fn append(parent: Obj, new: Obj) -> (HResult, Obj) {
    let mut out = UNTOUCHED;
    let hr = call!(parent, APPEND_CHILD, fn(Obj, *mut Obj) -> HResult, new, &mut out);
    (hr, out)
}

/// appendChild with no output.
fn append_only(parent: Obj, new: Obj) -> HResult {
    call!(parent, APPEND_CHILD, fn(Obj, *mut Obj) -> HResult, new, null_mut())
}

fn remove(parent: Obj, old: Obj) -> (HResult, Obj) {
    let mut out = UNTOUCHED;
    let hr = call!(parent, REMOVE_CHILD, fn(Obj, *mut Obj) -> HResult, old, &mut out);
    (hr, out)
}

fn replace(parent: Obj, new: Obj, old: Obj) -> (HResult, Obj) {
    let mut out = UNTOUCHED;
    let hr = call!(parent, REPLACE_CHILD, fn(Obj, Obj, *mut Obj) -> HResult, new, old, &mut out);
    (hr, out)
}

fn has_children(obj: Obj) -> HResult {
    let mut b: i16 = 0x55;
    call!(obj, HAS_CHILD_NODES, fn(*mut i16) -> HResult, &mut b)
}

fn xml_is(c: &mut Checks, obj: Obj, want: &str) -> Result<(), u32> {
    get_bstr(c, obj, GET_XML, want)
}

fn put_text(obj: Obj, s: &str) -> HResult {
    let w = Wide::new(s);
    call!(obj, PUT_TEXT, fn(Bstr) -> HResult, w.0)
}

fn set_attribute(el: Obj, name: &str, value: &str) -> HResult {
    let (n, v) = (Wide::new(name), Wide::new(value));
    call!(el, SET_ATTRIBUTE, fn(Bstr, Variant) -> HResult, n.0, Variant::bstr(&v))
}

fn put_node_value(obj: Obj, value: &str) -> HResult {
    let v = Wide::new(value);
    call!(obj, PUT_NODE_VALUE, fn(Variant) -> HResult, Variant::bstr(&v))
}

/// The IXMLDOMElement of a node (the node is released).
fn element(c: &mut Checks, node: Obj) -> Result<Obj, u32> {
    let (hr, el) = query(node, &IID_IXMLDOMELEMENT);
    release(node);
    c.hr(hr, S_OK)?;
    Ok(el)
}

/// The IUnknown of `obj`, released (a pointer to compare).
fn identity(obj: Obj) -> usize {
    let (_, unk) = query(obj, &IID_IUNKNOWN);
    release(unk);
    unk as usize
}

/// A numbered `info` line.
fn info_hr(what: &str, hr: HResult) {
    Line::new()
        .s(NAME.as_bytes())
        .s(b": info ")
        .s(what.as_bytes())
        .s(b" ")
        .hex(hr as u32 as u64)
        .s(b"\r\n")
        .print();
}

/// The complete4 document's root and its child number `i`.
fn complete4_child(c: &mut Checks, doc: Obj, i: i32) -> Result<(Obj, Obj, Obj), u32> {
    let (hr, ok) = load_xml(doc, COMPLETE4);
    c.hr(hr, S_OK)?;
    c.eq(ok as u16 as u64, VARIANT_TRUE as u16 as u64)?;
    let (hr, root) = get_obj(doc, GET_DOCUMENT_ELEMENT);
    c.hr(hr, S_OK)?;
    let (hr, list) = get_obj(root, GET_CHILD_NODES);
    c.hr(hr, S_OK)?;
    let mut node: Obj = null_mut();
    c.hr(call!(list, LIST_GET_ITEM, fn(i32, *mut Obj) -> HResult, i, &mut node), S_OK)?;
    Ok((root, list, node))
}

/// removeChild (Wine's test_removeChild): a NULL child is E_INVALIDARG
/// with the output left alone; a descendant that is not a child, or a
/// child already removed, E_INVALIDARG with the output NULL; the
/// removed node is the one passed (the same pointer) and has no parent;
/// a live childNodes list follows.
fn remove_child(c: &mut Checks, doc: Obj) -> Result<(), u32> {
    let (root, root_list, fo) = complete4_child(c, doc, 3)?;
    let (hr, ba) = get_obj(fo, GET_FIRST_CHILD);
    c.hr(hr, S_OK)?;
    let (hr, out) = remove(root, null_mut());
    c.code(hr, E_INVALIDARG)?;
    c.ok(out == UNTOUCHED, out as u64)?;
    let (hr, out) = remove(root, ba);
    c.code(hr, E_INVALIDARG)?;
    c.ok(out.is_null(), out as u64)?;
    c.hr(has_children(fo), S_OK)?;
    let (hr, out) = remove(root, fo);
    c.hr(hr, S_OK)?;
    c.ok(out == fo, out as u64)?;
    release(out);
    let (hr, out) = remove(root, fo);
    c.code(hr, E_INVALIDARG)?;
    c.ok(out.is_null(), out as u64)?;
    let (hr, parent) = get_obj(fo, GET_PARENT_NODE);
    c.hr(hr, S_FALSE)?;
    c.ok(parent.is_null(), parent as u64)?;
    c.eq(list_length(root_list), 3)?;
    // The removed subtree is whole.
    count(c, fo, GET_CHILD_NODES, LIST_GET_LENGTH, 1)?;
    // Another interface of the child works too (Wine: "MS quirk").
    let mut first: Obj = null_mut();
    c.hr(call!(root_list, LIST_GET_ITEM, fn(i32, *mut Obj) -> HResult, 0, &mut first), S_OK)?;
    let first = element(c, first)?;
    c.hr(call!(root, REMOVE_CHILD, fn(Obj, *mut Obj) -> HResult, first, null_mut()), S_OK)?;
    let (hr, parent) = get_obj(first, GET_PARENT_NODE);
    c.hr(hr, S_FALSE)?;
    c.ok(parent.is_null(), parent as u64)?;
    c.eq(list_length(root_list), 2)?;
    for o in [first, ba, fo, root_list, root] {
        release(o);
    }
    Ok(())
}

/// replaceChild (Wine's test_replaceChild): NULL for either node is
/// E_INVALIDARG with the output left alone; an old node that is not a
/// child E_INVALIDARG with the output NULL; a new node that is the
/// parent or its ancestor E_FAIL; a node replacing its parent's child
/// leaves its own parent; the output is the old node.
fn replace_child(c: &mut Checks, doc: Obj) -> Result<(), u32> {
    let (root, root_list, fo) = complete4_child(c, doc, 3)?;
    let mut bs: Obj = null_mut();
    c.hr(call!(root_list, LIST_GET_ITEM, fn(i32, *mut Obj) -> HResult, 0, &mut bs), S_OK)?;
    let (hr, ba) = get_obj(fo, GET_FIRST_CHILD);
    c.hr(hr, S_OK)?;
    let (hr, out) = replace(root, ba, null_mut());
    c.code(hr, E_INVALIDARG)?;
    c.ok(out == UNTOUCHED, out as u64)?;
    let (hr, out) = replace(root, null_mut(), fo);
    c.code(hr, E_INVALIDARG)?;
    c.ok(out == UNTOUCHED, out as u64)?;
    let (hr, out) = replace(root, bs, ba);
    c.code(hr, E_INVALIDARG)?;
    c.ok(out.is_null(), out as u64)?;
    let (hr, out) = replace(fo, fo, ba);
    c.code(hr, E_FAIL)?;
    c.ok(out.is_null(), out as u64)?;
    let (hr, out) = replace(root, ba, fo);
    c.hr(hr, S_OK)?;
    c.ok(out == fo, out as u64)?;
    release(out);
    let mut fourth: Obj = null_mut();
    c.hr(call!(root_list, LIST_GET_ITEM, fn(i32, *mut Obj) -> HResult, 3, &mut fourth), S_OK)?;
    get_bstr(c, fourth, GET_NODE_NAME, "ba")?;
    release(fourth);
    count(c, fo, GET_CHILD_NODES, LIST_GET_LENGTH, 0)?;
    let (hr, parent) = get_obj(fo, GET_PARENT_NODE);
    c.hr(hr, S_FALSE)?;
    c.ok(parent.is_null(), parent as u64)?;
    // A node replacing itself.
    let (hr, out) = replace(root, ba, ba);
    c.hr(hr, S_OK)?;
    c.ok(!out.is_null() && out != UNTOUCHED, out as u64)?;
    release(out);
    c.eq(list_length(root_list), 4)?;
    for o in [ba, bs, fo, root_list, root] {
        release(o);
    }
    Ok(())
}

/// insertBefore (Wine's test_insertBefore, MSXML 6's answers measured
/// on Windows where they differ): a document under itself is E_FAIL,
/// under anything else E_INVALIDARG; what each parent takes
/// (E_INVALIDARG otherwise, where Wine's msxml3 tests expect E_FAIL;
/// the output NULL), a reference node that is not a child
/// (E_INVALIDARG), a node moved from another parent, a node
/// from another document, and the namespaces the moved nodes' xml
/// declares.
fn insert(c: &mut Checks) -> Result<(), u32> {
    let doc = new_doc(c)?;
    let doc3 = new_doc(c)?;
    let (hr, out) = insert_before(doc, null_mut(), null_variant());
    c.code(hr, E_INVALIDARG)?;
    c.ok(out == UNTOUCHED, out as u64)?;
    let (hr, out) = insert_before(doc, doc3, null_variant());
    c.code(hr, E_INVALIDARG)?;
    c.ok(out.is_null(), out as u64)?;
    let (hr, out) = insert_before(doc, doc, null_variant());
    c.code(hr, E_FAIL)?;
    c.ok(out.is_null(), out as u64)?;

    let (hr, attr) = create_node(doc, NODE_ATTRIBUTE, Some("attr"), None);
    c.hr(hr, S_OK)?;
    let (hr, out) = insert_before(doc3, attr, null_variant());
    c.code(hr, E_INVALIDARG)?;
    c.ok(out.is_null(), out as u64)?;
    let (hr, cdata) = create_node(doc3, NODE_CDATA_SECTION, Some("cdata"), None);
    c.hr(hr, S_OK)?;
    c.hr(has_children(cdata), S_FALSE)?;
    for (parent, new) in [(cdata, attr), (doc3, cdata)] {
        let (hr, out) = insert_before(parent, new, null_variant());
        c.code(hr, E_INVALIDARG)?;
        c.ok(out.is_null(), out as u64)?;
    }
    let (hr, out) = insert_before(cdata, doc, null_variant());
    c.code(hr, E_INVALIDARG)?;
    c.ok(out.is_null(), out as u64)?;
    release(cdata);
    release(doc3);

    // An attribute takes text only.
    let (hr, attr2) = create_node(doc, NODE_ATTRIBUTE, Some("attr2"), None);
    c.hr(hr, S_OK)?;
    let (hr, out) = insert_before(attr, attr2, null_variant());
    c.code(hr, E_INVALIDARG)?;
    c.ok(out.is_null(), out as u64)?;
    let (hr, out) = insert_before(attr, attr2, Variant { vt: VT_UNKNOWN, ..dispatch(attr) });
    c.code(hr, E_INVALIDARG)?;
    c.ok(out.is_null(), out as u64)?;
    release(attr2);
    let text = create(c, doc, CREATE_TEXT_NODE, "text")?;
    c.hr(
        call!(
            attr,
            INSERT_BEFORE,
            fn(Obj, Variant, *mut Obj) -> HResult,
            text,
            null_variant(),
            null_mut()
        ),
        S_OK,
    )?;
    get_bstr(c, attr, GET_TEXT, "text")?;
    release(text);
    for kind in [NODE_CDATA_SECTION, NODE_COMMENT, NODE_ELEMENT, NODE_PROCESSING_INSTRUCTION] {
        let (hr, node) = create_node(doc, kind, Some("cdata"), None);
        c.hr(hr, S_OK)?;
        let (hr, out) = insert_before(attr, node, null_variant());
        c.code(hr, E_INVALIDARG)?;
        c.ok(out.is_null(), out as u64)?;
        release(node);
    }
    release(attr);

    // Elements.
    let elem1 = create(c, doc, CREATE_ELEMENT, "elem")?;
    let elem2 = create(c, doc, CREATE_ELEMENT, "elem2")?;
    let elem3 = create(c, doc, CREATE_ELEMENT, "elem3")?;
    let elem4 = create(c, doc, CREATE_ELEMENT, "elem4")?;
    c.hr(has_children(elem1), S_FALSE)?;
    let (hr, out) = insert_before(elem1, null_mut(), null_variant());
    c.code(hr, E_INVALIDARG)?;
    c.ok(out == UNTOUCHED, out as u64)?;
    let hr = call!(
        elem1,
        INSERT_BEFORE,
        fn(Obj, Variant, *mut Obj) -> HResult,
        doc,
        dispatch(null_mut()),
        null_mut()
    );
    c.code(hr, E_INVALIDARG)?;
    let (hr, out) = insert_before(elem1, elem4, dispatch(null_mut()));
    c.hr(hr, S_OK)?;
    c.ok(out == elem4, out as u64)?;
    release(out);
    c.hr(has_children(elem1), S_OK)?;
    c.hr(call!(elem1, REMOVE_CHILD, fn(Obj, *mut Obj) -> HResult, elem4, null_mut()), S_OK)?;
    release(elem4);
    c.hr(has_children(elem1), S_FALSE)?;
    for _ in 0..2 {
        let (hr, out) = insert_before(elem1, elem2, null_variant());
        c.hr(hr, S_OK)?;
        c.ok(out == elem2, out as u64)?;
        release(out);
        c.hr(has_children(elem1), S_OK)?;
    }
    // To another parent: it leaves the first.
    let (hr, out) = insert_before(elem3, elem2, null_variant());
    c.hr(hr, S_OK)?;
    c.ok(out == elem2, out as u64)?;
    release(out);
    c.hr(has_children(elem3), S_OK)?;
    c.hr(has_children(elem1), S_FALSE)?;
    // Before a reference child.
    let first = create(c, doc, CREATE_ELEMENT, "first")?;
    let (hr, out) = insert_before(elem3, first, dispatch(elem2));
    c.hr(hr, S_OK)?;
    release(out);
    xml_is(c, elem3, "<elem3><first/><elem2/></elem3>")?;
    release(first);

    // Nodes of another document.
    let doc2 = new_doc(c)?;
    let elem4 = create(c, doc2, CREATE_ELEMENT, "elem4")?;
    let elem5 = create(c, doc2, CREATE_ELEMENT, "elem4")?;
    for e in [elem4, elem5] {
        let (hr, out) = insert_before(elem3, e, null_variant());
        c.hr(hr, S_OK)?;
        c.ok(out == e, out as u64)?;
        release(out);
    }
    release(doc2);
    xml_is(c, elem3, "<elem3><first/><elem2/><elem4/><elem4/></elem3>")?;
    let (hr, parent) = get_obj(elem4, GET_PARENT_NODE);
    c.hr(hr, S_OK)?;
    get_bstr(c, parent, GET_NODE_NAME, "elem3")?;
    release(parent);
    for e in [elem1, elem2, elem3, elem4, elem5] {
        release(e);
    }

    // The same default namespace: the child's xml alone declares it,
    // within its parent's not.
    let (hr, elem1) =
        create_node(doc, NODE_ELEMENT, Some("elem1"), Some("http://winehq.org/default"));
    c.hr(hr, S_OK)?;
    let (hr, elem2) =
        create_node(doc, NODE_ELEMENT, Some("elem2"), Some("http://winehq.org/default"));
    c.hr(hr, S_OK)?;
    xml_is(c, elem1, "<elem1 xmlns=\"http://winehq.org/default\"/>")?;
    xml_is(c, elem2, "<elem2 xmlns=\"http://winehq.org/default\"/>")?;
    c.hr(
        call!(
            elem1,
            INSERT_BEFORE,
            fn(Obj, Variant, *mut Obj) -> HResult,
            elem2,
            null_variant(),
            null_mut()
        ),
        S_OK,
    )?;
    xml_is(c, elem1, "<elem1 xmlns=\"http://winehq.org/default\"><elem2/></elem1>")?;
    xml_is(c, elem2, "<elem2 xmlns=\"http://winehq.org/default\"/>")?;
    release(elem1);
    release(elem2);
    // A child in no namespace under a default namespace.
    let (hr, elem1) =
        create_node(doc, NODE_ELEMENT, Some("elem1"), Some("http://winehq.org/default"));
    c.hr(hr, S_OK)?;
    let (hr, elem2) = create_node(doc, NODE_ELEMENT, Some("elem2"), None);
    c.hr(hr, S_OK)?;
    c.hr(
        call!(
            elem1,
            INSERT_BEFORE,
            fn(Obj, Variant, *mut Obj) -> HResult,
            elem2,
            null_variant(),
            null_mut()
        ),
        S_OK,
    )?;
    xml_is(c, elem2, "<elem2/>")?;
    xml_is(c, elem1, "<elem1 xmlns=\"http://winehq.org/default\"><elem2 xmlns=\"\"/></elem1>")?;
    c.hr(call!(elem1, REMOVE_CHILD, fn(Obj, *mut Obj) -> HResult, elem2, null_mut()), S_OK)?;
    xml_is(c, elem2, "<elem2/>")?;
    release(elem1);
    release(elem2);
    release(doc);
    Ok(())
}

/// appendChild (Wine's test_appendChild): NULL is E_INVALIDARG; a node
/// of another document moves; a document takes one element (a second
/// is E_FAIL), comments and processing instructions; a fragment gives
/// its children and is left empty (the output is the fragment).
fn append_child(c: &mut Checks) -> Result<(), u32> {
    let doc = new_doc(c)?;
    let doc2 = new_doc(c)?;
    let elem = create(c, doc, CREATE_ELEMENT, "elem")?;
    c.hr(has_children(doc), S_FALSE)?;
    c.hr(append_only(doc2, null_mut()), E_INVALIDARG)?;
    c.hr(append_only(elem, null_mut()), E_INVALIDARG)?;
    c.hr(append_only(doc2, elem), S_OK)?;
    c.hr(has_children(doc), S_FALSE)?;
    c.hr(has_children(doc2), S_OK)?;
    release(elem);
    release(doc2);

    let elem = create(c, doc, CREATE_ELEMENT, "elem")?;
    let elem2 = create(c, doc, CREATE_ELEMENT, "elem2")?;
    c.hr(append_only(doc, elem), S_OK)?;
    c.code(append_only(doc, elem2), E_FAIL)?;
    let comment1 = create(c, doc, CREATE_COMMENT, "Comment1")?;
    let comment2 = create(c, doc, CREATE_COMMENT, "Comment2")?;
    c.hr(append_only(doc, comment1), S_OK)?;
    let hr = call!(
        doc,
        INSERT_BEFORE,
        fn(Obj, Variant, *mut Obj) -> HResult,
        comment2,
        dispatch(elem),
        null_mut()
    );
    c.hr(hr, S_OK)?;
    let target = Wide::new("xml");
    let data = Wide::new("version=\"1.0\"");
    let mut pi: Obj = null_mut();
    let hr = call!(
        doc,
        CREATE_PROCESSING_INSTRUCTION,
        fn(Bstr, Bstr, *mut Obj) -> HResult,
        target.0,
        data.0,
        &mut pi
    );
    c.hr(hr, S_OK)?;
    c.hr(append_only(doc, pi), S_OK)?;
    count(c, doc, GET_CHILD_NODES, LIST_GET_LENGTH, 4)?;
    let (hr, first) = get_obj(doc, GET_FIRST_CHILD);
    c.hr(hr, S_OK)?;
    get_bstr(c, first, GET_TEXT, "Comment2")?;
    release(first);
    for o in [pi, comment1, comment2, elem, elem2] {
        release(o);
    }

    // A fragment's children.
    let (hr, fragment) = get_obj(doc, CREATE_DOCUMENT_FRAGMENT);
    c.hr(hr, S_OK)?;
    c.hr(call!(doc, CREATE_DOCUMENT_FRAGMENT, fn(*mut Obj) -> HResult, null_mut()), E_INVALIDARG)?;
    c.eq(u64::from(node_type(fragment)), NODE_DOCUMENT_FRAGMENT as u64)?;
    get_bstr(c, fragment, GET_NODE_NAME, "#document-fragment")?;
    get_bstr(c, fragment, GET_NODE_TYPE_STRING, "documentfragment")?;
    let (hr, other) = query(fragment, &IID_IXMLDOMDOCUMENTFRAGMENT);
    c.hr(hr, S_OK)?;
    release(other);
    let text1 = create(c, doc, CREATE_TEXT_NODE, "text1")?;
    let text2 = create(c, doc, CREATE_TEXT_NODE, "text2")?;
    c.hr(has_children(fragment), S_FALSE)?;
    c.hr(append_only(fragment, text1), S_OK)?;
    c.hr(append_only(fragment, text2), S_OK)?;
    c.hr(has_children(fragment), S_OK)?;
    let elem = create(c, doc, CREATE_ELEMENT, "elem")?;
    let (hr, out) = append(elem, fragment);
    c.hr(hr, S_OK)?;
    c.ok(out == fragment, out as u64)?;
    release(out);
    c.hr(has_children(fragment), S_FALSE)?;
    let (hr, list) = get_obj(elem, GET_CHILD_NODES);
    c.hr(hr, S_OK)?;
    c.eq(list_length(list), 2)?;
    let mut node: Obj = null_mut();
    while call!(list, LIST_NEXT_NODE, fn(*mut Obj) -> HResult, &mut node) == S_OK {
        c.eq(u64::from(node_type(node)), NODE_TEXT as u64)?;
        release(node);
    }
    for o in [list, elem, text1, text2, fragment] {
        release(o);
    }

    // Fragments in fragments.
    let (hr, fragment) = get_obj(doc, CREATE_DOCUMENT_FRAGMENT);
    c.hr(hr, S_OK)?;
    let (hr, fragment2) = get_obj(doc, CREATE_DOCUMENT_FRAGMENT);
    c.hr(hr, S_OK)?;
    let elem = create(c, doc, CREATE_ELEMENT, "elem")?;
    c.hr(append_only(fragment, elem), S_OK)?;
    xml_is(c, fragment, "<elem/>")?;
    let elem2 = create(c, doc, CREATE_ELEMENT, "elem2")?;
    c.hr(append_only(fragment2, elem2), S_OK)?;
    xml_is(c, fragment2, "<elem2/>")?;
    c.hr(append_only(fragment, fragment2), S_OK)?;
    c.hr(has_children(fragment2), S_FALSE)?;
    xml_is(c, fragment, "<elem/><elem2/>")?;
    let elem3 = create(c, doc, CREATE_ELEMENT, "elem3")?;
    c.hr(append_only(elem3, fragment), S_OK)?;
    c.hr(has_children(fragment), S_FALSE)?;
    xml_is(c, elem3, "<elem3><elem/><elem2/></elem3>")?;
    let (hr, parent) = get_obj(fragment, GET_PARENT_NODE);
    c.hr(hr, S_FALSE)?;
    c.ok(parent.is_null(), parent as u64)?;
    for o in [elem, elem2, elem3, fragment2, fragment] {
        release(o);
    }
    release(doc);
    Ok(())
}

/// createElement, createNode and the other create methods (Wine's
/// test_createElement, test_createNode, test_create, test_doc_fragment):
/// a NULL name is E_INVALIDARG and an empty one E_FAIL, the output
/// NULL; createNode refuses the document, entities (E_INVALIDARG, the
/// output NULL); a
/// prefix without a namespace stays undeclared; a namespace given to
/// createNode is declared by the node's xml and is not an attribute.
fn creation(c: &mut Checks) -> Result<(), u32> {
    let doc = new_doc(c)?;
    for (name, want) in [(None, E_INVALIDARG), (Some(""), E_FAIL)] {
        let n = name.map(Wide::new);
        let mut out = UNTOUCHED;
        let hr = call!(
            doc,
            CREATE_ELEMENT,
            fn(Bstr, *mut Obj) -> HResult,
            n.as_ref().map_or(null_mut(), |w| w.0),
            &mut out
        );
        c.hr(hr, want)?;
        // The output is NULL (MSXML 6 on Windows for a NULL name; Wine's
        // msxml3 tests expect it left alone).
        c.note(out.is_null(), out as u64)?;
    }
    for kind in [NODE_DOCUMENT, NODE_ENTITY] {
        let (hr, out) = create_node(doc, kind, Some("name"), None);
        c.code(hr, E_INVALIDARG)?;
        c.note(out.is_null(), out as u64)?;
    }
    section("creation: a prefixed element");
    let el = create(c, doc, CREATE_ELEMENT, "prefix:name")?;
    xml_is(c, el, "<prefix:name/>")?;
    let mut s: Bstr = null_mut();
    c.hr(call!(el, GET_NAMESPACE_URI, fn(*mut Bstr) -> HResult, &mut s), S_FALSE)?;
    c.hr(append_only(doc, el), S_OK)?;
    xml_is(c, doc, "<prefix:name/>\r\n")?;
    release(el);

    section("creation: createNode with a namespace");
    let (hr, node) = create_node(doc, NODE_ELEMENT, Some("ns1:test"), Some("http://winehq.org"));
    c.hr(hr, S_OK)?;
    get_bstr(c, node, GET_PREFIX, "ns1")?;
    get_bstr(c, node, GET_BASE_NAME, "test")?;
    get_bstr(c, node, GET_NAMESPACE_URI, "http://winehq.org")?;
    xml_is(c, node, "<ns1:test xmlns:ns1=\"http://winehq.org\"/>")?;
    release(node);
    let (hr, node) =
        create_node(doc, NODE_ELEMENT, Some("test"), Some("http://winehq.org/default"));
    c.hr(hr, S_OK)?;
    let mut prefix: Bstr = 0x1 as Bstr;
    c.hr(call!(node, GET_PREFIX, fn(*mut Bstr) -> HResult, &mut prefix), S_FALSE)?;
    c.ok(prefix.is_null(), prefix as u64)?;
    xml_is(c, node, "<test xmlns=\"http://winehq.org/default\"/>")?;
    let el = element(c, node)?;
    let x = Wide::new("xmlns");
    let mut v = Variant::empty();
    c.hr(call!(el, GET_ATTRIBUTE, fn(Bstr, *mut Variant) -> HResult, x.0, &mut v), S_FALSE)?;
    c.eq(u64::from(v.vt), u64::from(VT_NULL))?;
    get_bstr(c, el, GET_NAMESPACE_URI, "http://winehq.org/default")?;
    release(el);

    section("creation: comment and fragment");
    let (hr, comment) = create_node(doc, NODE_COMMENT, None, None);
    c.hr(hr, S_OK)?;
    c.eq(u64::from(node_type(comment)), NODE_COMMENT as u64)?;
    get_bstr(c, comment, TEXT_GET_DATA, "")?;
    release(comment);
    let (hr, fragment) = create_node(doc, NODE_DOCUMENT_FRAGMENT, Some("p:text"), Some("uri"));
    c.hr(hr, S_OK)?;
    for slot in [GET_NAMESPACE_URI, GET_BASE_NAME, GET_PREFIX] {
        let mut s: Bstr = 0x1 as Bstr;
        c.hr(call!(fragment, slot, fn(*mut Bstr) -> HResult, &mut s), S_FALSE)?;
        c.ok(s.is_null(), s as u64)?;
    }
    release(fragment);
    section("creation: text");
    let text = create(c, doc, CREATE_TEXT_NODE, "a < b")?;
    get_bstr(c, text, GET_NODE_NAME, "#text")?;
    xml_is(c, text, "a &lt; b")?;
    release(text);
    release(doc);
    Ok(())
}

/// setAttribute, put_text and put_nodeValue (Wine's test_setAttribute,
/// test_put_text, test_put_nodeValue, test_default_namespace): a
/// namespace declaration set as an attribute declares the element's
/// prefix in its xml without changing its namespace; a declaration
/// already there cannot be changed (E_FAIL, through setAttribute or
/// put_text); put_text on an element replaces its children with the
/// text, the old text node left as it was; put_nodeValue is E_FAIL for
/// the document, fragments and elements.
fn attributes(c: &mut Checks) -> Result<(), u32> {
    let doc = new_doc(c)?;
    let el = create(c, doc, CREATE_ELEMENT, "prefix:name")?;
    c.hr(set_attribute(el, "xmlns:prefix", "uri"), S_OK)?;
    let mut s: Bstr = null_mut();
    c.hr(call!(el, GET_NAMESPACE_URI, fn(*mut Bstr) -> HResult, &mut s), S_FALSE)?;
    xml_is(c, el, "<prefix:name xmlns:prefix=\"uri\"/>")?;
    c.hr(set_attribute(el, "xmlns:prefix", "uri2"), E_FAIL)?;
    xml_is(c, el, "<prefix:name xmlns:prefix=\"uri\"/>")?;
    let name = Wide::new("xmlns:prefix");
    let mut attr: Obj = null_mut();
    c.hr(call!(el, GET_ATTRIBUTE_NODE, fn(Bstr, *mut Obj) -> HResult, name.0, &mut attr), S_OK)?;
    c.hr(put_text(attr, "uri3"), E_FAIL)?;
    release(attr);
    c.hr(set_attribute(el, "attr1", "value"), S_OK)?;
    let name = Wide::new("attr1");
    c.hr(call!(el, GET_ATTRIBUTE_NODE, fn(Bstr, *mut Obj) -> HResult, name.0, &mut attr), S_OK)?;
    c.hr(put_text(attr, "value2"), S_OK)?;
    release(attr);
    xml_is(c, el, "<prefix:name xmlns:prefix=\"uri\" attr1=\"value2\"/>")?;
    c.hr(set_attribute(el, "attr1", "value3"), S_OK)?;
    attribute(c, el, "attr1", "value3")?;
    count(c, el, GET_ATTRIBUTES, MAP_GET_LENGTH, 2)?;
    release(el);
    let (hr, node) = create_node(doc, NODE_ELEMENT, Some("prefix:name"), Some("uri"));
    c.hr(hr, S_OK)?;
    let el = element(c, node)?;
    c.hr(set_attribute(el, "prefix", "uri2"), S_OK)?;
    xml_is(c, el, "<prefix:name xmlns:prefix=\"uri\" prefix=\"uri2\"/>")?;
    release(el);

    // An element in no namespace under a default namespace, with its
    // own declaration and attribute.
    let (hr, e1) = create_node(doc, NODE_ELEMENT, Some("e1"), Some("default-uri"));
    c.hr(hr, S_OK)?;
    let e2 = create(c, doc, CREATE_ELEMENT, "e2")?;
    c.hr(set_attribute(e2, "xmlns:ns", "uri"), S_OK)?;
    c.hr(set_attribute(e2, "attr", "value"), S_OK)?;
    c.hr(append_only(e1, e2), S_OK)?;
    xml_is(
        c,
        e1,
        "<e1 xmlns=\"default-uri\"><e2 xmlns=\"\" xmlns:ns=\"uri\" attr=\"value\"/></e1>",
    )?;
    release(e2);
    release(e1);

    // put_text.
    let src = "<?xml version='1.0' encoding='UTF-16'?>\n<open what=\"door\">sesame</open>\n";
    let (hr, _) = load_xml(doc, src);
    c.hr(hr, S_OK)?;
    let (hr, el) = get_obj(doc, GET_DOCUMENT_ELEMENT);
    c.hr(hr, S_OK)?;
    let (hr, text) = get_obj(el, GET_FIRST_CHILD);
    c.hr(hr, S_OK)?;
    c.hr(put_text(text, "SESAME"), S_OK)?;
    xml_is(c, el, "<open what=\"door\">SESAME</open>")?;
    let (hr, map) = get_obj(el, GET_ATTRIBUTES);
    c.hr(hr, S_OK)?;
    let mut what: Obj = null_mut();
    c.hr(call!(map, MAP_GET_ITEM, fn(i32, *mut Obj) -> HResult, 0, &mut what), S_OK)?;
    c.hr(put_text(what, "window"), S_OK)?;
    c.hr(put_text(el, "sesame"), S_OK)?;
    xml_is(c, el, "<open what=\"window\">sesame</open>")?;
    c.hr(put_text(text, "SeSaMe"), S_OK)?;
    get_bstr(c, text, GET_TEXT, "SeSaMe")?;
    xml_is(c, el, "<open what=\"window\">sesame</open>")?;
    for o in [what, map, text, el] {
        release(o);
    }
    let src = "<open>\r\n\t<inside>sesame</inside>\r\n</open>\n";
    let (hr, _) = load_xml(doc, src);
    c.hr(hr, S_OK)?;
    let (hr, el) = get_obj(doc, GET_DOCUMENT_ELEMENT);
    c.hr(hr, S_OK)?;
    c.hr(put_text(el, "sesame"), S_OK)?;
    xml_is(c, el, "<open>sesame</open>")?;
    release(el);

    // put_nodeValue.
    c.hr(put_node_value(doc, "one two three"), E_FAIL)?;
    let (hr, fragment) = create_node(doc, NODE_DOCUMENT_FRAGMENT, Some("test"), None);
    c.hr(hr, S_OK)?;
    c.hr(put_node_value(fragment, "one two three"), E_FAIL)?;
    release(fragment);
    let el = create(c, doc, CREATE_ELEMENT, "test")?;
    c.hr(put_node_value(el, "one two three"), E_FAIL)?;
    release(el);
    let text = create(c, doc, CREATE_TEXT_NODE, "")?;
    c.hr(put_node_value(text, "Jeeves & Wooster"), S_OK)?;
    xml_is(c, text, "Jeeves &amp; Wooster")?;
    release(text);
    let attr = create(c, doc, CREATE_ATTRIBUTE, "attr")?;
    c.hr(put_node_value(attr, "Jeeves & Wooster"), S_OK)?;
    let mut v = Variant::empty();
    c.hr(call!(attr, GET_NODE_VALUE, fn(*mut Variant) -> HResult, &mut v), S_OK)?;
    c.var_text(&mut v, "Jeeves & Wooster")?;
    release(attr);
    release(doc);
    Ok(())
}

/// setAttributeNode and removeAttributeNode (Wine's
/// test_setAttributeNode): NULL is E_INVALIDARG, the output left alone;
/// a new attribute gives NULL (no attribute replaced); an attribute on
/// an element already, of this document or another, is E_FAIL; the
/// attribute set is the element's (its value changed later shows in the
/// element's xml). removeAttributeNode gives the attribute back (MSXML's
/// documentation); what it answers for one the element does not have
/// is an `info` line, and so is removeAttribute's for a name it lacks.
fn attribute_nodes(c: &mut Checks) -> Result<(), u32> {
    let doc = new_doc(c)?;
    let (hr, ok) = load_xml(doc, COMPLETE4);
    c.hr(hr, S_OK)?;
    c.eq(ok as u16 as u64, VARIANT_TRUE as u16 as u64)?;
    let (hr, el) = get_obj(doc, GET_DOCUMENT_ELEMENT);
    c.hr(hr, S_OK)?;
    let mut out = UNTOUCHED;
    let hr = call!(el, SET_ATTRIBUTE_NODE, fn(Obj, *mut Obj) -> HResult, null_mut(), &mut out);
    c.code(hr, E_INVALIDARG)?;
    c.ok(out == UNTOUCHED, out as u64)?;
    let attr = create(c, doc, CREATE_ATTRIBUTE, "attr")?;
    let hr = call!(el, SET_ATTRIBUTE_NODE, fn(Obj, *mut Obj) -> HResult, attr, &mut out);
    c.hr(hr, S_OK)?;
    c.ok(out.is_null(), out as u64)?;
    let name = Wide::new("attr");
    let mut attr2: Obj = null_mut();
    c.hr(call!(el, GET_ATTRIBUTE_NODE, fn(Bstr, *mut Obj) -> HResult, name.0, &mut attr2), S_OK)?;
    release(attr2);
    out = UNTOUCHED;
    let hr = call!(el, SET_ATTRIBUTE_NODE, fn(Obj, *mut Obj) -> HResult, attr, &mut out);
    c.code(hr, E_FAIL)?;
    c.ok(out == UNTOUCHED, out as u64)?;
    release(el);
    let doc2 = new_doc(c)?;
    let (hr, _) = load_xml(doc2, COMPLETE4);
    c.hr(hr, S_OK)?;
    let (hr, el2) = get_obj(doc2, GET_DOCUMENT_ELEMENT);
    c.hr(hr, S_OK)?;
    c.hr(call!(el2, SET_ATTRIBUTE_NODE, fn(Obj, *mut Obj) -> HResult, attr, null_mut()), E_FAIL)?;
    release(el2);
    release(doc2);
    release(attr);

    let el = create(c, doc, CREATE_ELEMENT, "test")?;
    let attr = create(c, doc, CREATE_ATTRIBUTE, "attr")?;
    c.hr(put_node_value(attr, "attrvalue1"), S_OK)?;
    xml_is(c, attr, "attr=\"attrvalue1\"")?;
    out = UNTOUCHED;
    c.hr(call!(el, SET_ATTRIBUTE_NODE, fn(Obj, *mut Obj) -> HResult, attr, &mut out), S_OK)?;
    c.ok(out.is_null(), out as u64)?;
    xml_is(c, el, "<test attr=\"attrvalue1\"/>")?;
    c.hr(put_node_value(attr, "attrvalue2"), S_OK)?;
    xml_is(c, el, "<test attr=\"attrvalue2\"/>")?;
    // A second attribute of the same name replaces the first.
    let other = create(c, doc, CREATE_ATTRIBUTE, "attr")?;
    c.hr(put_node_value(other, "other"), S_OK)?;
    out = null_mut();
    c.hr(call!(el, SET_ATTRIBUTE_NODE, fn(Obj, *mut Obj) -> HResult, other, &mut out), S_OK)?;
    c.ok(!out.is_null(), 0)?;
    get_bstr(c, out, GET_TEXT, "attrvalue2")?;
    release(out);
    xml_is(c, el, "<test attr=\"other\"/>")?;
    out = UNTOUCHED;
    c.hr(call!(el, REMOVE_ATTRIBUTE_NODE, fn(Obj, *mut Obj) -> HResult, other, &mut out), S_OK)?;
    c.ok(out == other, out as u64)?;
    release(out);
    xml_is(c, el, "<test/>")?;
    let hr = call!(el, REMOVE_ATTRIBUTE_NODE, fn(Obj, *mut Obj) -> HResult, other, null_mut());
    info_hr("removeAttributeNode(not the element's)", hr);
    let hr = call!(el, REMOVE_ATTRIBUTE_NODE, fn(Obj, *mut Obj) -> HResult, null_mut(), null_mut());
    c.code(hr, E_INVALIDARG)?;
    release(other);
    release(attr);
    release(el);

    // removeAttribute.
    let src = "<a xmlns:ns=\"uri\" ns:attr1=\"attr1-value\"><ns:b ns:attr=\"attr-value\"/></a>";
    let (hr, _) = load_xml(doc, src);
    c.hr(hr, S_OK)?;
    let (hr, el) = get_obj(doc, GET_DOCUMENT_ELEMENT);
    c.hr(hr, S_OK)?;
    let name = Wide::new("ns:attr1");
    c.hr(call!(el, REMOVE_ATTRIBUTE, fn(Bstr) -> HResult, name.0), S_OK)?;
    xml_is(c, el, "<a xmlns:ns=\"uri\"><ns:b ns:attr=\"attr-value\"/></a>")?;
    let mut attr: Obj = UNTOUCHED;
    c.hr(call!(el, GET_ATTRIBUTE_NODE, fn(Bstr, *mut Obj) -> HResult, name.0, &mut attr), S_FALSE)?;
    let hr = call!(el, REMOVE_ATTRIBUTE, fn(Bstr) -> HResult, name.0);
    info_hr("removeAttribute(absent)", hr);
    release(el);
    release(doc);
    Ok(())
}

/// The attribute map's changes (Wine's test_removeNamedItem,
/// test_removeQualifiedItem, test_element_setNamedItem): a NULL name is
/// E_INVALIDARG with the output left alone; a name the element lacks
/// S_FALSE with the output NULL; the output is optional; setNamedItem
/// gives the attribute set and replaces one of the same name.
fn named_map(c: &mut Checks) -> Result<(), u32> {
    let doc = new_doc(c)?;
    let (pr_root, pr_list, pr) = complete4_child(c, doc, 1)?;
    let (hr, map) = get_obj(pr, GET_ATTRIBUTES);
    c.hr(hr, S_OK)?;
    let length = |c: &mut Checks, want: u64| -> Result<(), u32> {
        let mut n = -1i32;
        c.hr(call!(map, MAP_GET_LENGTH, fn(*mut i32) -> HResult, &mut n), S_OK)?;
        c.eq(n as u32 as u64, want)
    };
    length(c, 3)?;
    let remove_named = |name: Option<&Wide>, out: *mut Obj| -> HResult {
        let n = name.map_or(null_mut(), |w| w.0);
        call!(map, MAP_REMOVE_NAMED_ITEM, fn(Bstr, *mut Obj) -> HResult, n, out)
    };
    let mut out = UNTOUCHED;
    c.hr(remove_named(None, &mut out), E_INVALIDARG)?;
    c.ok(out == UNTOUCHED, out as u64)?;
    let vr = Wide::new("vr");
    let mut removed: Obj = null_mut();
    c.hr(remove_named(Some(&vr), &mut removed), S_OK)?;
    out = UNTOUCHED;
    c.hr(remove_named(Some(&vr), &mut out), S_FALSE)?;
    c.ok(out.is_null(), out as u64)?;
    length(c, 2)?;
    let hr = call!(map, MAP_SET_NAMED_ITEM, fn(Obj, *mut Obj) -> HResult, removed, null_mut());
    c.hr(hr, S_OK)?;
    release(removed);
    length(c, 3)?;
    c.hr(remove_named(Some(&vr), null_mut()), S_OK)?;
    length(c, 2)?;
    c.hr(remove_named(Some(&vr), null_mut()), S_FALSE)?;
    let remove_qualified = |name: Option<&str>, out: *mut Obj| -> HResult {
        let n = name.map(Wide::new);
        let n = n.as_ref().map_or(null_mut(), |w| w.0);
        call!(
            map,
            MAP_REMOVE_QUALIFIED_ITEM,
            fn(Bstr, Bstr, *mut Obj) -> HResult,
            n,
            null_mut(),
            out
        )
    };
    c.hr(remove_qualified(None, null_mut()), E_INVALIDARG)?;
    out = UNTOUCHED;
    c.hr(remove_qualified(None, &mut out), E_INVALIDARG)?;
    c.ok(out == UNTOUCHED, out as u64)?;
    c.hr(remove_qualified(Some("id"), null_mut()), S_OK)?;
    c.hr(remove_qualified(Some("id"), null_mut()), S_FALSE)?;
    c.hr(remove_qualified(Some("pn"), &mut out), S_OK)?;
    get_bstr(c, out, GET_NODE_NAME, "pn")?;
    release(out);
    length(c, 0)?;
    for o in [map, pr, pr_list, pr_root] {
        release(o);
    }

    // With namespaces.
    let src = "<?xml version=\"1.0\"?><a xmlns:ns=\"nshref\" xml:lang=\"ru\" ns:b=\"b attr\" \
               xml:c=\"c attr\"     d=\"d attr\" />";
    let (hr, ok) = load_xml(doc, src);
    c.hr(hr, S_OK)?;
    c.eq(ok as u16 as u64, VARIANT_TRUE as u16 as u64)?;
    let (hr, a) = select_single(doc, "a");
    c.hr(hr, S_OK)?;
    let (hr, map) = get_obj(a, GET_ATTRIBUTES);
    c.hr(hr, S_OK)?;
    let absent = Wide::new("foo:bar");
    out = UNTOUCHED;
    let hr = call!(map, MAP_REMOVE_NAMED_ITEM, fn(Bstr, *mut Obj) -> HResult, absent.0, &mut out);
    c.hr(hr, S_FALSE)?;
    c.ok(out.is_null(), out as u64)?;
    for name in ["d", "ns:b", "xml:lang"] {
        let n = Wide::new(name);
        let mut out: Obj = null_mut();
        let hr = call!(map, MAP_REMOVE_NAMED_ITEM, fn(Bstr, *mut Obj) -> HResult, n.0, &mut out);
        c.hr(hr, S_OK)?;
        release(out);
    }
    let mut n = -1i32;
    c.hr(call!(map, MAP_GET_LENGTH, fn(*mut i32) -> HResult, &mut n), S_OK)?;
    c.eq(n as u32 as u64, 2)?;
    release(map);
    release(a);

    // setNamedItem on a new element.
    let el = create(c, doc, CREATE_ELEMENT, "e")?;
    let (hr, map) = get_obj(el, GET_ATTRIBUTES);
    c.hr(hr, S_OK)?;
    let attr1 = create(c, doc, CREATE_ATTRIBUTE, "attr")?;
    c.hr(put_text(attr1, "text1"), S_OK)?;
    let attr2 = create(c, doc, CREATE_ATTRIBUTE, "attr")?;
    c.hr(put_text(attr2, "text2"), S_OK)?;
    for (attr, want) in [(attr1, "text1"), (attr2, "text2")] {
        let mut node: Obj = null_mut();
        let hr = call!(map, MAP_SET_NAMED_ITEM, fn(Obj, *mut Obj) -> HResult, attr, &mut node);
        c.hr(hr, S_OK)?;
        get_bstr(c, node, GET_TEXT, want)?;
        release(node);
    }
    xml_is(c, el, "<e attr=\"text2\"/>")?;
    for o in [attr1, attr2, map, el] {
        release(o);
    }
    release(doc);
    Ok(())
}

/// normalize (MSXML's documentation: adjacent text nodes in the
/// element's subtree become one), getElementsByTagName (Wine's
/// test_getElementsByTagName: "*" from the document is every element,
/// from an element its descendants; NULL arguments are E_INVALIDARG),
/// importNode (MSXML's documentation: a copy owned by this document,
/// with no parent), selectNodes with SelectionNamespaces over nodes
/// made and moved, and putref_documentElement (the documentation).
fn more(c: &mut Checks) -> Result<(), u32> {
    let doc = new_doc(c)?;
    let el = create(c, doc, CREATE_ELEMENT, "n")?;
    let inner = create(c, doc, CREATE_ELEMENT, "i")?;
    for s in ["a", "b"] {
        let t = create(c, doc, CREATE_TEXT_NODE, s)?;
        c.hr(append_only(inner, t), S_OK)?;
        release(t);
    }
    c.hr(append_only(el, inner), S_OK)?;
    for (slot, s) in [(CREATE_TEXT_NODE, "c"), (CREATE_TEXT_NODE, "d"), (CREATE_COMMENT, "x")] {
        let t = create(c, doc, slot, s)?;
        c.hr(append_only(el, t), S_OK)?;
        release(t);
    }
    let t = create(c, doc, CREATE_TEXT_NODE, "e")?;
    c.hr(append_only(el, t), S_OK)?;
    release(t);
    count(c, el, GET_CHILD_NODES, LIST_GET_LENGTH, 5)?;
    c.hr(call!(el, NORMALIZE, fn() -> HResult,), S_OK)?;
    count(c, el, GET_CHILD_NODES, LIST_GET_LENGTH, 4)?;
    count(c, inner, GET_CHILD_NODES, LIST_GET_LENGTH, 1)?;
    xml_is(c, el, "<n><i>ab</i>cd<!--x-->e</n>")?;
    let empty = create(c, doc, CREATE_TEXT_NODE, "")?;
    c.hr(append_only(el, empty), S_OK)?;
    release(empty);
    c.hr(call!(el, NORMALIZE, fn() -> HResult,), S_OK)?;
    let (hr, list) = get_obj(el, GET_CHILD_NODES);
    c.hr(hr, S_OK)?;
    info_hr("normalize(empty text last: children)", list_length(list) as i32);
    release(list);
    release(inner);

    // getElementsByTagName.
    let (hr, ok) = load_xml(doc, COMPLETE4);
    c.hr(hr, S_OK)?;
    c.eq(ok as u16 as u64, VARIANT_TRUE as u16 as u64)?;
    let star = Wide::new("*");
    let mut list = UNTOUCHED;
    let hr = call!(
        doc,
        DOC_GET_ELEMENTS_BY_TAG_NAME,
        fn(Bstr, *mut Obj) -> HResult,
        null_mut(),
        &mut list
    );
    c.code(hr, E_INVALIDARG)?;
    let hr =
        call!(doc, DOC_GET_ELEMENTS_BY_TAG_NAME, fn(Bstr, *mut Obj) -> HResult, star.0, null_mut());
    c.code(hr, E_INVALIDARG)?;
    for (name, want) in [("*", 6), ("bs", 1), ("ba", 1), ("x", 0)] {
        let n = Wide::new(name);
        let hr =
            call!(doc, DOC_GET_ELEMENTS_BY_TAG_NAME, fn(Bstr, *mut Obj) -> HResult, n.0, &mut list);
        c.hr(hr, S_OK)?;
        c.eq(list_length(list), want)?;
        release(list);
    }
    let (hr, root) = get_obj(doc, GET_DOCUMENT_ELEMENT);
    c.hr(hr, S_OK)?;
    let hr =
        call!(root, EL_GET_ELEMENTS_BY_TAG_NAME, fn(Bstr, *mut Obj) -> HResult, star.0, &mut list);
    c.hr(hr, S_OK)?;
    c.eq(list_length(list), 5)?;
    let mut first: Obj = null_mut();
    c.hr(call!(list, LIST_GET_ITEM, fn(i32, *mut Obj) -> HResult, 0, &mut first), S_OK)?;
    get_bstr(c, first, GET_NODE_NAME, "bs")?;
    release(first);
    release(list);

    // importNode into another document.
    let doc2 = new_doc(c)?;
    let (hr, doc3) = query(doc2, &IID_IXMLDOMDOCUMENT3);
    c.hr(hr, S_OK)?;
    let mut copy: Obj = null_mut();
    let hr =
        call!(doc3, IMPORT_NODE, fn(Obj, i16, *mut Obj) -> HResult, root, VARIANT_TRUE, &mut copy);
    c.hr(hr, S_OK)?;
    let (hr, parent) = get_obj(copy, GET_PARENT_NODE);
    c.hr(hr, S_FALSE)?;
    c.ok(parent.is_null(), parent as u64)?;
    let (hr, owner) = get_obj(copy, GET_OWNER_DOCUMENT);
    c.hr(hr, S_OK)?;
    c.ok(identity(owner) == identity(doc2), 0)?;
    release(owner);
    let mut a: Bstr = null_mut();
    c.hr(call!(root, GET_XML, fn(*mut Bstr) -> HResult, &mut a), S_OK)?;
    let mut b: Bstr = null_mut();
    c.hr(call!(copy, GET_XML, fn(*mut Bstr) -> HResult, &mut b), S_OK)?;
    c.ok(bstr_eq(a, b), 0)?;
    c.hr(append_only(doc2, copy), S_OK)?;
    let (hr, found) = select_single(doc2, "lc/fo/ba");
    c.hr(hr, S_OK)?;
    release(found);
    // The original stays where it was.
    let (hr, found) = select_single(doc, "lc/fo/ba");
    c.hr(hr, S_OK)?;
    release(found);
    let hr = call!(
        doc3,
        IMPORT_NODE,
        fn(Obj, i16, *mut Obj) -> HResult,
        null_mut(),
        VARIANT_TRUE,
        &mut copy
    );
    info_hr("importNode(NULL)", hr);
    release(doc3);
    release(doc2);

    // Nodes made in a namespace and moved are selected by prefix.
    let (hr, item) = create_node(doc, NODE_ELEMENT, Some("x:item"), Some("urn:x"));
    c.hr(hr, S_OK)?;
    let (hr, fo) = select_single(doc, "lc/fo");
    c.hr(hr, S_OK)?;
    c.hr(append_only(fo, item), S_OK)?;
    let name = Wide::new("SelectionNamespaces");
    let value = Wide::new("xmlns:y='urn:x'");
    let hr = call!(doc, SET_PROPERTY, fn(Bstr, Variant) -> HResult, name.0, Variant::bstr(&value));
    c.hr(hr, S_OK)?;
    let (hr, found) = select_single(doc, "//y:item");
    c.hr(hr, S_OK)?;
    let (hr, parent) = get_obj(found, GET_PARENT_NODE);
    c.hr(hr, S_OK)?;
    get_bstr(c, parent, GET_NODE_NAME, "fo")?;
    release(parent);
    release(found);
    let (hr, list) = select(doc, "/lc/*[2] | //y:item");
    c.hr(hr, S_OK)?;
    c.eq(list_length(list), 2)?;
    release(list);
    let empty = Wide::new("");
    let hr = call!(doc, SET_PROPERTY, fn(Bstr, Variant) -> HResult, name.0, Variant::bstr(&empty));
    c.hr(hr, S_OK)?;
    count(c, fo, GET_CHILD_NODES, LIST_GET_LENGTH, 2)?;
    xml_is(c, item, "<x:item xmlns:x=\"urn:x\"/>")?;
    for o in [item, fo, root, el] {
        release(o);
    }
    release(doc);

    // putref_documentElement: the element becomes the document element,
    // in place of the one there.
    let doc = new_doc(c)?;
    for name in ["r", "s"] {
        let e = create(c, doc, CREATE_ELEMENT, name)?;
        c.hr(call!(doc, PUTREF_DOCUMENT_ELEMENT, fn(Obj) -> HResult, e), S_OK)?;
        release(e);
        let (hr, root) = get_obj(doc, GET_DOCUMENT_ELEMENT);
        c.hr(hr, S_OK)?;
        get_bstr(c, root, GET_NODE_NAME, name)?;
        release(root);
        count(c, doc, GET_CHILD_NODES, LIST_GET_LENGTH, 1)?;
    }
    release(doc);
    Ok(())
}

/// save to a file: what is saved loads again to the same tree (MSXML's
/// documentation of save; the bytes it writes, its encoding and BOM,
/// are an `info` line).
fn save(c: &mut Checks) -> Result<(), u32> {
    let doc = new_doc(c)?;
    let (hr, _) = load_xml(doc, COMPLETE4);
    c.hr(hr, S_OK)?;
    let el = create(c, doc, CREATE_ELEMENT, "added")?;
    c.hr(set_attribute(el, "v", "\u{e9}&<"), S_OK)?;
    let (hr, root) = get_obj(doc, GET_DOCUMENT_ELEMENT);
    c.hr(hr, S_OK)?;
    c.hr(append_only(root, el), S_OK)?;
    release(el);
    release(root);
    let mut before: Bstr = null_mut();
    c.hr(call!(doc, GET_XML, fn(*mut Bstr) -> HResult, &mut before), S_OK)?;

    let mut path = [0u16; 300];
    // SAFETY: a buffer of 300 characters.
    let n = unsafe { GetTempPathW(260, path.as_mut_ptr()) } as usize;
    c.ok(n > 0 && n < 260, n as u64)?;
    for (slot, u) in path[n..].iter_mut().zip("gala_msxml_save.xml".encode_utf16()) {
        *slot = u;
    }
    // SAFETY: a NUL-terminated path.
    let target = Wide(unsafe { SysAllocString(path.as_ptr()) });
    c.hr(call!(doc, SAVE, fn(Variant) -> HResult, Variant::bstr(&target)), S_OK)?;
    // SAFETY: a NUL-terminated path; GENERIC_READ, OPEN_EXISTING.
    let h = unsafe { CreateFileW(path.as_ptr(), 0x8000_0000, 1, 0, 3, 0x80, 0) };
    c.ok(h != usize::MAX, 0)?;
    let mut head = [0u8; 8];
    let mut got = 0u32;
    // SAFETY: our handle and buffer.
    unsafe {
        ReadFile(h, head.as_mut_ptr(), 8, &mut got, 0);
        CloseHandle(h);
    }
    let mut line = Line::new().s(NAME.as_bytes()).s(b": info save's first bytes");
    for b in &head[..got as usize] {
        line = line.s(b" ").dec(u64::from(*b));
    }
    line.s(b"\r\n").print();

    let doc2 = new_doc(c)?;
    c.hr(call!(doc2, PUT_ASYNC, fn(i16) -> HResult, VARIANT_FALSE), S_OK)?;
    let mut ok: i16 = 0x55;
    let hr = call!(doc2, LOAD, fn(Variant, *mut i16) -> HResult, Variant::bstr(&target), &mut ok);
    c.hr(hr, S_OK)?;
    let mut after: Bstr = null_mut();
    c.hr(call!(doc2, GET_XML, fn(*mut Bstr) -> HResult, &mut after), S_OK)?;
    c.ok(bstr_eq(before, after), 0)?;
    // SAFETY: a NUL-terminated path.
    unsafe { DeleteFileW(path.as_ptr()) };
    release(doc2);
    release(doc);
    Ok(())
}

pub fn mutation(c: &mut Checks, doc: Obj) -> Result<(), u32> {
    section("removeChild");
    remove_child(c, doc)?;
    section("replaceChild");
    replace_child(c, doc)?;
    section("insertBefore");
    insert(c)?;
    section("appendChild");
    append_child(c)?;
    section("creation");
    creation(c)?;
    section("attributes");
    attributes(c)?;
    section("attribute nodes");
    attribute_nodes(c)?;
    section("named map");
    named_map(c)?;
    section("more");
    more(c)?;
    section("save");
    save(c)
}

/// Names the part of the test that starts and whether the process's
/// heaps are still valid, so a heap corruption (Windows' heap reports it
/// later, at some free) is placed by the first part that ends with one.
pub fn section(name: &str) {
    let state: &[u8] = if heaps_valid() { b" (heaps valid)" } else { b" (heaps CORRUPT)" };
    Line::new().s(NAME.as_bytes()).s(b": section ").s(name.as_bytes()).s(state).s(b"\r\n").print();
}
