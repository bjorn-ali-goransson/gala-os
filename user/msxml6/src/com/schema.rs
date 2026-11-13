//! XMLSchemaCache60: MSXML 6's schema cache (IXMLDOMSchemaCollection
//! and IXMLDOMSchemaCollection2), and what the document does with one
//! (the `schemas` property, validate, validateNode, validation while
//! loading). The schemas are XSD only, compiled by `xsd.rs`; the
//! behaviour is MSXML's documentation's ("add Method", "Improvements in
//! Schema Cache Behavior in MSXML 6.0", "validate Method") and Wine's
//! msxml3 tests run on Windows (schema.c's collection tests, domdoc.c's
//! test_putref_schemas):
//! - add(namespace, schema): a DOMDocument (any node: its whole
//!   document) or a BSTR naming a file or `res:` URL, loaded without
//!   validation; VT_NULL removes the namespace; E_FAIL for a document
//!   that is not an XSD schema (XDR is not in MSXML 6), one that does
//!   not compile, a namespace that is not the schema's target namespace,
//!   and, with validateOnLoad (the default), a schema naming a type,
//!   element or attribute no schema in the cache declares; a namespace
//!   added again merges ("partial schemas"); an add is atomic;
//! - get is E_NOTIMPL in MSXML 6 (getSchema replaces it); remove of a
//!   namespace not there is S_OK; length and namespaceURI (an index
//!   past the end E_FAIL, the output untouched; NULL outputs E_POINTER);
//!   addCollection merges another cache's schemas; validate checks that
//!   every name resolves (what validateOnLoad false postponed).
//!
//! getSchema, getDeclaration (the Schema Object Model) and the
//! enumerator are not implemented (E_NOTIMPL, recorded). Each add, each
//! validation and its result is recorded in the ledger as "other" (what
//! a program does with schemas is seen in its run without a debugger).

use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, Ordering};

use super::*;
use crate::xsd;

const IID_IXMLDOMSCHEMACOLLECTION: Guid =
    guid(0x3739_84c8, 0xb845, 0x449b, [0x91, 0xe7, 0x45, 0xac, 0x83, 0x03, 0x6a, 0xde]);
const IID_IXMLDOMSCHEMACOLLECTION2: Guid =
    guid(0x50ea_08b0, 0xdd1b, 0x4664, [0x9a, 0x50, 0xc2, 0xf4, 0x0f, 0x4b, 0xd7, 0x9a]);

/// A namespace's schemas (MSXML 6 merges the documents of one target
/// namespace).
#[derive(Clone)]
struct Entry {
    ns: String,
    schemas: Vec<xsd::Schema>,
}

#[repr(C)]
pub(super) struct CacheObj {
    vtable: *const (),
    refs: AtomicU32,
    entries: Vec<Entry>,
    validate_on_load: bool,
}

fn cache<'a>(this: Obj) -> &'a mut CacheObj {
    // SAFETY: our cache objects' interface pointers; a cache is used by
    // one thread at a time (as the rental document is).
    unsafe { &mut *(this as *mut CacheObj) }
}

/// The cache behind `p`, when it is one of ours.
pub(super) fn ours_cache(p: Obj) -> Option<Obj> {
    if p.is_null() {
        return None;
    }
    // SAFETY: an interface pointer: its first field is a vtable pointer.
    let vtable = unsafe { *(p as *const *const ()) };
    (vtable == &CACHE_VTABLE as *const VTable<19> as *const ()).then_some(p)
}

/// Makes a schema cache (the class factory's CreateInstance).
pub fn new_cache(riid: *const Guid, out: *mut Obj) -> HResult {
    let c = CacheObj {
        vtable: &CACHE_VTABLE as *const VTable<19> as *const (),
        refs: AtomicU32::new(1),
        entries: Vec::new(),
        validate_on_load: true,
    };
    let Some(p) = try_box(c) else { return E_OUTOFMEMORY };
    let hr = cache_query(p as Obj, riid, out);
    cache_release(p as Obj);
    hr
}

/// Calls `f` with the schemas of cache `c` as a set.
pub(super) fn with_set<R>(c: Obj, f: impl FnOnce(&xsd::Set) -> R) -> R {
    let entries = &cache(c).entries;
    let set = xsd::Set { schemas: entries.iter().flat_map(|e| e.schemas.iter()).collect() };
    f(&set)
}

extern "system" fn cache_query(this: Obj, riid: *const Guid, out: *mut Obj) -> HResult {
    if out.is_null() || riid.is_null() {
        return E_POINTER;
    }
    // SAFETY: the caller's IID.
    let iid = unsafe { *riid };
    // The class's own CLSID as an IID: App-V's ConfigurePackage creates
    // the cache asking for it (`ole32` passes its IID to CreateInstance;
    // the run's ledger had the miss), and the same code works on Windows,
    // so MSXML answers it; gala gives the collection (IXMLDOMSchemaCollection2,
    // the class's default interface). Recorded each time; probe issue
    // #125 asks Windows what the pointer is.
    let own_class = iid == crate::CLSID_XMLSCHEMACACHE60;
    if own_class {
        record(b"XMLSchemaCache60::QueryInterface(its CLSID)", LEDGER_OTHER, 0);
    }
    if iid == IID_IUNKNOWN
        || iid == IID_IDISPATCH
        || iid == IID_IXMLDOMSCHEMACOLLECTION
        || iid == IID_IXMLDOMSCHEMACOLLECTION2
        || own_class
    {
        cache(this).refs.fetch_add(1, Ordering::Relaxed);
        put(out, this);
        return S_OK;
    }
    put(out, core::ptr::null_mut());
    crate::record_iid("XMLSchemaCache60::QueryInterface: no", &iid);
    E_NOINTERFACE
}

pub(super) extern "system" fn cache_add_ref(this: Obj) -> u32 {
    cache(this).refs.fetch_add(1, Ordering::Relaxed) + 1
}

pub(super) extern "system" fn cache_release(this: Obj) -> u32 {
    let left = cache(this).refs.fetch_sub(1, Ordering::AcqRel) - 1;
    if left == 0 {
        // SAFETY: the object's last reference.
        unsafe { free_box(this as *mut CacheObj) };
    }
    left
}

/// A namespace argument: NULL is "", white space normalized as xmlns
/// values are (the documentation of add).
fn namespace(uri: Bstr) -> String {
    xsd::collapse(&text(uri).unwrap_or_default())
}

/// Records `<what> <namespace>` (as much as the ledger keeps).
fn record_ns(what: &str, ns: &str, kind: u32, detail: u64) {
    let mut s = String::from(what);
    s.push(' ');
    s.push_str(ns);
    record(s.as_bytes(), kind, detail);
}

/// The tree a schema argument gives: a document of ours (any node: its
/// document) or a file or `res:` URL loaded.
fn schema_tree(var: &Variant) -> Result<Tree, HResult> {
    match var.vt {
        VT_DISPATCH | VT_UNKNOWN => {
            let p = var.value[0] as Obj;
            // Any interface of our document or node objects starts with
            // the node's vtable.
            match mutate::ours(p) {
                Some(n) => {
                    // SAFETY: a node object holds a reference on its
                    // document.
                    Ok(unsafe { (*n.doc).tree.clone() })
                }
                None => Err(E_INVALIDARG),
            }
        }
        VT_BSTR => {
            let Some(url) = text(var.value[0] as *const u16) else { return Err(E_INVALIDARG) };
            record_ns("XMLSchemaCache60::add(from URL)", &url, LEDGER_OTHER, 0);
            let src = read_source(&url).map_err(|_| E_FAIL)?;
            match gala_xml::parse_text(&src) {
                Ok(parsed) => Ok(Tree::from_document(&parsed, false)),
                Err(e) => {
                    record_ns(
                        "XMLSchemaCache60::add(schema parse error)",
                        &e.msg,
                        LEDGER_OTHER,
                        u64::from(e.line),
                    );
                    Err(E_FAIL)
                }
            }
        }
        other => {
            record(
                b"XMLSchemaCache60::add(schema argument type)",
                LEDGER_UNSUPPORTED,
                other.into(),
            );
            Err(E_INVALIDARG)
        }
    }
}

/// Puts `schema` into `entries` (merging into its namespace's entry);
/// E_FAIL when a global name is declared twice in one namespace.
fn merge(entries: &mut Vec<Entry>, schema: xsd::Schema) -> Result<(), String> {
    match entries.iter_mut().find(|e| e.ns == schema.target) {
        Some(e) => {
            for other in &e.schemas {
                let clash = schema
                    .global_elems
                    .iter()
                    .find(|(n, _)| other.element(n).is_some())
                    .map(|(n, _)| n)
                    .or_else(|| {
                        schema
                            .named_types
                            .iter()
                            .find(|(n, _)| other.named_type(n).is_some())
                            .map(|(n, _)| n)
                    })
                    .or_else(|| {
                        schema
                            .attrs
                            .iter()
                            .find(|a| other.attribute(&a.name.local).is_some())
                            .map(|a| &a.name.local)
                    });
                if let Some(n) = clash {
                    return Err(n.clone());
                }
            }
            e.schemas.push(schema);
        }
        None => entries.push(Entry { ns: schema.target.clone(), schemas: alloc::vec![schema] }),
    }
    Ok(())
}

/// Whether every name the schemas of `entries` use is declared there.
fn check_names(entries: &[Entry]) -> Result<(), String> {
    let set = xsd::Set { schemas: entries.iter().flat_map(|e| e.schemas.iter()).collect() };
    match set.unresolved() {
        Some(what) => Err(what),
        None => Ok(()),
    }
}

extern "system" fn cache_add(this: Obj, uri: Bstr, var: Variant) -> HResult {
    let c = cache(this);
    let ns = namespace(uri);
    record_ns("XMLSchemaCache60::add", &ns, LEDGER_OTHER, u64::from(var.vt));
    if var.vt == VT_NULL {
        c.entries.retain(|e| e.ns != ns);
        return S_OK;
    }
    let tree = match schema_tree(&var) {
        Ok(t) => t,
        Err(hr) => return hr,
    };
    let schema = match xsd::compile(&tree) {
        Ok(s) => s,
        Err(xsd::Error::Unsupported(what)) => {
            record_ns("XMLSchemaCache60::add(XSD not supported:", &what, LEDGER_UNSUPPORTED, 0);
            return E_FAIL;
        }
        Err(xsd::Error::Xdr) => {
            record(b"XMLSchemaCache60::add(an XDR schema: none in MSXML 6)", LEDGER_OTHER, 0);
            return E_FAIL;
        }
        Err(xsd::Error::NotSchema) => {
            record(b"XMLSchemaCache60::add(not a schema)", LEDGER_OTHER, 0);
            return E_FAIL;
        }
        Err(xsd::Error::Invalid(why)) => {
            record_ns("XMLSchemaCache60::add(schema invalid:", &why, LEDGER_OTHER, 0);
            return E_FAIL;
        }
    };
    if schema.target != ns {
        record_ns(
            "XMLSchemaCache60::add(namespace is not the targetNamespace)",
            &schema.target,
            LEDGER_OTHER,
            0,
        );
        return E_FAIL;
    }
    // Atomic: on a copy, kept only when everything holds.
    let mut entries = c.entries.clone();
    if let Err(name) = merge(&mut entries, schema) {
        record_ns("XMLSchemaCache60::add(declared twice:", &name, LEDGER_OTHER, 0);
        return E_FAIL;
    }
    if c.validate_on_load
        && let Err(what) = check_names(&entries)
    {
        record_ns("XMLSchemaCache60::add(unresolved:", &what, LEDGER_OTHER, 0);
        return E_FAIL;
    }
    c.entries = entries;
    S_OK
}

/// get: not implemented in MSXML 6 (its documentation: use getSchema).
extern "system" fn cache_get(_this: Obj, _uri: Bstr, out: *mut Obj) -> HResult {
    record(b"XMLSchemaCache60::get (E_NOTIMPL, as in MSXML 6)", LEDGER_OTHER, 0);
    if out.is_null() {
        return E_POINTER;
    }
    crate::E_NOTIMPL
}

extern "system" fn cache_remove(this: Obj, uri: Bstr) -> HResult {
    let ns = namespace(uri);
    record_ns("XMLSchemaCache60::remove", &ns, LEDGER_OTHER, 0);
    cache(this).entries.retain(|e| e.ns != ns);
    S_OK
}

extern "system" fn cache_get_length(this: Obj, out: *mut i32) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, cache(this).entries.len() as i32);
    S_OK
}

extern "system" fn cache_get_namespace_uri(this: Obj, index: i32, out: *mut Bstr) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    match usize::try_from(index).ok().and_then(|i| cache(this).entries.get(i)) {
        Some(e) => give_str(out, Some(&e.ns)),
        None => E_FAIL,
    }
}

extern "system" fn cache_add_collection(this: Obj, other: Obj) -> HResult {
    if other.is_null() {
        return E_POINTER;
    }
    record(b"XMLSchemaCache60::addCollection", LEDGER_OTHER, 0);
    let Some(o) = ours_cache(other) else {
        record(
            b"XMLSchemaCache60::addCollection(a collection msxml6 did not make)",
            LEDGER_UNSUPPORTED,
            0,
        );
        return E_INVALIDARG;
    };
    if o == this {
        return S_OK;
    }
    let mut entries = cache(this).entries.clone();
    for e in cache(o).entries.clone() {
        for s in e.schemas {
            if let Err(name) = merge(&mut entries, s) {
                record_ns(
                    "XMLSchemaCache60::addCollection(declared twice:",
                    &name,
                    LEDGER_OTHER,
                    0,
                );
                return E_FAIL;
            }
        }
    }
    let c = cache(this);
    if c.validate_on_load
        && let Err(what) = check_names(&entries)
    {
        record_ns("XMLSchemaCache60::addCollection(unresolved:", &what, LEDGER_OTHER, 0);
        return E_FAIL;
    }
    c.entries = entries;
    S_OK
}

/// validate: the compile steps validateOnLoad false postponed (every name
/// resolves).
extern "system" fn cache_validate(this: Obj) -> HResult {
    match check_names(&cache(this).entries) {
        Ok(()) => {
            record(b"XMLSchemaCache60::validate (valid)", LEDGER_OTHER, 0);
            S_OK
        }
        Err(what) => {
            record_ns("XMLSchemaCache60::validate(unresolved:", &what, LEDGER_OTHER, 0);
            E_FAIL
        }
    }
}

extern "system" fn cache_put_validate_on_load(this: Obj, v: i16) -> HResult {
    record(b"XMLSchemaCache60::put_validateOnLoad", LEDGER_OTHER, (v != 0).into());
    cache(this).validate_on_load = v != 0;
    S_OK
}

extern "system" fn cache_get_validate_on_load(this: Obj, out: *mut i16) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, if cache(this).validate_on_load { VARIANT_TRUE } else { VARIANT_FALSE });
    S_OK
}

not_implemented! {
    cache_new_enum(*mut Obj) = b"IXMLDOMSchemaCollection::get__newEnum";
    cache_get_schema(Bstr, *mut Obj) = b"IXMLDOMSchemaCollection2::getSchema";
    cache_get_declaration(Obj, *mut Obj) = b"IXMLDOMSchemaCollection2::getDeclaration";
}

static CACHE_VTABLE: VTable<19> = VTable([
    cache_query as *const (),
    cache_add_ref as *const (),
    cache_release as *const (),
    get_type_info_count as *const (),
    get_type_info as *const (),
    get_ids_of_names as *const (),
    invoke as *const (),
    cache_add as *const (),
    cache_get as *const (),
    cache_remove as *const (),
    cache_get_length as *const (),
    cache_get_namespace_uri as *const (),
    cache_add_collection as *const (),
    cache_new_enum as *const (),
    cache_validate as *const (),
    cache_put_validate_on_load as *const (),
    cache_get_validate_on_load as *const (),
    cache_get_schema as *const (),
    cache_get_declaration as *const (),
]);

// ---- The document's side ----

/// IXMLDOMDocument2::get_schemas: the cache set (VT_DISPATCH, a
/// reference for the caller), or VT_NULL and S_FALSE.
pub(super) extern "system" fn get_schemas(this: Obj, out: *mut Variant) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    let s = doc(this).schemas;
    if s.is_null() {
        variant_null(out);
        return S_FALSE;
    }
    cache_add_ref(s);
    put(out, Variant { vt: VT_DISPATCH, reserved: [0; 3], value: [s as usize, 0] });
    S_OK
}

/// IXMLDOMDocument2::putref_schemas: a cache of ours (VT_DISPATCH or
/// VT_UNKNOWN; NULL, VT_NULL and VT_EMPTY clear it); other variant types
/// E_FAIL (Wine's tests).
pub(super) extern "system" fn putref_schemas(this: Obj, v: Variant) -> HResult {
    let d = doc(this);
    let new = match v.vt {
        VT_DISPATCH | VT_UNKNOWN => {
            let p = v.value[0] as Obj;
            if p.is_null() {
                None
            } else {
                match ours_cache(p) {
                    Some(c) => Some(c),
                    None => {
                        record(
                            b"IXMLDOMDocument2::putref_schemas(a collection msxml6 did not make)",
                            LEDGER_UNSUPPORTED,
                            0,
                        );
                        return E_FAIL;
                    }
                }
            }
        }
        VT_NULL | VT_EMPTY => None,
        _ => return E_FAIL,
    };
    record(b"IXMLDOMDocument2::putref_schemas", LEDGER_OTHER, new.is_some().into());
    if let Some(c) = new {
        cache_add_ref(c);
    }
    let old = core::mem::replace(&mut d.schemas, new.unwrap_or(core::ptr::null_mut()));
    if !old.is_null() {
        cache_release(old);
    }
    S_OK
}

/// A validation's error as a parse error (the code and reason; the
/// place: the line of nothing, as validate's error has only those set).
fn invalid_error(e: &xsd::validate::Invalid, tree: &Tree) -> ParseError {
    if let Some(what) = e.unsupported {
        let mut s = String::from("validation: not supported: ");
        s.push_str(what);
        record(s.as_bytes(), LEDGER_UNSUPPORTED, 0);
    }
    let mut s = String::from("validation failed: ");
    s.push_str(e.reason.trim_end());
    record(s.as_bytes(), LEDGER_OTHER, e.code as u32 as u64);
    // And where: the node's path, its end kept (the ledger keeps 96
    // bytes of a name).
    let mut names: Vec<&str> = Vec::new();
    let mut x = e.node;
    while x != crate::dom::NONE && x != 0 && names.len() < 32 {
        let n = tree.node(x);
        names.push(&n.name);
        x = n.parent;
    }
    let mut path = String::new();
    for (i, n) in names.iter().rev().enumerate() {
        path.push(if i + 1 == names.len() && tree.node(e.node).kind == Kind::Attribute {
            '@'
        } else {
            '/'
        });
        path.push_str(n);
    }
    let tail: String = {
        let chars: Vec<char> = path.chars().collect();
        chars[chars.len().saturating_sub(60)..].iter().collect()
    };
    let mut s = String::from("validation failed at: ");
    s.push_str(&tail);
    record(s.as_bytes(), LEDGER_OTHER, e.code as u32 as u64);
    ParseError { code: e.code, reason: e.reason.clone(), ..ParseError::default() }
}

fn report_defaults(o: &xsd::validate::Outcome) {
    if o.defaults > 0 {
        record(
            b"validation: schema default attribute values not added to the tree",
            LEDGER_UNSUPPORTED,
            o.defaults.into(),
        );
    }
}

/// Validates the document or element `el` of `d` against its cache:
/// None when valid.
fn run(d: &Doc, el: Option<u32>) -> Option<ParseError> {
    if d.schemas.is_null() {
        let code = if d.tree.root().is_none() {
            xsd::validate::XML_E_NOTWF
        } else {
            xsd::validate::XML_E_NODTD
        };
        let reason = if code == xsd::validate::XML_E_NOTWF {
            "The document has no document element.\r\n"
        } else {
            "No schema is associated with the document.\r\n"
        };
        return Some(ParseError { code, reason: reason.into(), ..ParseError::default() });
    }
    let result = with_set(d.schemas, |set| match el {
        None => xsd::validate::document(set, &d.tree),
        Some(e) => xsd::validate::element(set, &d.tree, e),
    });
    match result {
        Ok(o) => {
            report_defaults(&o);
            None
        }
        Err(e) => Some(invalid_error(&e, &d.tree)),
    }
}

/// Hands the caller a parse error object for `e` (code 0: no error).
fn give_error(e: ParseError, out: *mut Obj) -> HResult {
    if out.is_null() {
        return S_OK;
    }
    let o = ErrorObj {
        vtable: &ERROR_VTABLE as *const VTable<14> as *const (),
        refs: AtomicU32::new(1),
        error: e,
    };
    match try_box(o) {
        Some(p) => {
            put(out, p as Obj);
            S_OK
        }
        None => E_OUTOFMEMORY,
    }
}

/// IXMLDOMDocument2::validate: S_OK when the document is valid against
/// its cache, S_FALSE with the error otherwise (no document element:
/// XML_E_NOTWF; no schema: XML_E_NODTD).
pub(super) extern "system" fn validate(this: Obj, out: *mut Obj) -> HResult {
    let d = doc(this);
    let e = run(d, None);
    record(
        b"IXMLDOMDocument2::validate",
        LEDGER_OTHER,
        e.as_ref().map_or(0, |e| e.code as u32 as u64),
    );
    let valid = e.is_none();
    let hr = give_error(e.unwrap_or_default(), out);
    if hr != S_OK {
        return hr;
    }
    if valid { S_OK } else { S_FALSE }
}

/// IXMLDOMDocument3::validateNode: an element of this document, by its
/// global declaration.
pub(super) extern "system" fn validate_node(this: Obj, node: Obj, out: *mut Obj) -> HResult {
    let d = doc(this);
    let Some(n) = mutate::ours(node) else { return E_INVALIDARG };
    if !core::ptr::eq(n.doc, d) {
        return E_FAIL;
    }
    if d.tree.node(n.idx).kind != Kind::Element {
        record(b"IXMLDOMDocument3::validateNode(not an element)", LEDGER_UNSUPPORTED, 0);
        return E_FAIL;
    }
    let e = run(d, Some(n.idx));
    record(
        b"IXMLDOMDocument3::validateNode",
        LEDGER_OTHER,
        e.as_ref().map_or(0, |e| e.code as u32 as u64),
    );
    let valid = e.is_none();
    let hr = give_error(e.unwrap_or_default(), out);
    if hr != S_OK {
        return hr;
    }
    if valid { S_OK } else { S_FALSE }
}

/// Validation while loading (validateOnParse with a cache that has a
/// schema for the document element's namespace): the error, when the
/// document is not valid.
pub(super) fn on_load(d: &Doc) -> Option<ParseError> {
    if !d.validate || d.schemas.is_null() {
        return None;
    }
    let root = d.tree.root()?;
    let ns = &d.tree.node(root).ns;
    if !with_set(d.schemas, |set| set.has_namespace(ns)) {
        return None;
    }
    let e = run(d, None);
    record(
        b"load: validated against the schema cache",
        LEDGER_OTHER,
        e.as_ref().map_or(0, |e| e.code as u32 as u64),
    );
    e
}
