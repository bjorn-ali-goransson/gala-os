//! XSD schemas and validation: XMLSchemaCache60 (IXMLDOMSchemaCollection
//! and IXMLDOMSchemaCollection2) and what a document does with one
//! (`schemas`, validate, validateNode, validateOnParse). Click-to-Run's
//! App-V pieces ask COM for XMLSchemaCache60 when they configure
//! Office's package (ROADMAP OF).
//!
//! Expected values are MSXML's documentation ("add Method", "get
//! Method", "Improvements in Schema Cache Behavior in MSXML 6.0",
//! "validate Method", "validateOnLoad Property") and Wine's msxml3 tests
//! run on Windows (schema.c: get_length and get_namespaceURI's E_POINTER
//! and E_FAIL, remove of a namespace not there, VT_NULL removing;
//! domdoc.c: validate without a document element (XML_E_NOTWF) and
//! without a schema (XML_E_NODTD), a valid document's S_OK with an error
//! whose errorCode is S_FALSE and 0, putref_schemas and get_schemas'
//! answers), with XSD's rules (W3C XML Schema 1.0) deciding what is
//! valid. Where none settles the value, it is an `info` line: the exact
//! error code of each kind of invalid document (only "an error" is
//! checked), add's answer for a schema naming an element no schema in
//! the cache declares while validateOnLoad is on, for a namespace that
//! is not the schema's targetNamespace, the collection's validate while
//! a name is unresolved, the order of namespaceURI's entries, and
//! validate's code for a document whose namespace has no schema.

use core::ptr::null_mut;

use super::mutation::info_hr;
use super::*;

const VT_DISPATCH: u16 = 9;

const CLSID_XMLSCHEMACACHE60: Guid =
    guid(0x88d9_6a07, 0xf192, 0x11d4, [0xa6, 0x5f, 0x00, 0x40, 0x96, 0x32, 0x51, 0xe5]);
const IID_IXMLDOMSCHEMACOLLECTION: Guid =
    guid(0x3739_84c8, 0xb845, 0x449b, [0x91, 0xe7, 0x45, 0xac, 0x83, 0x03, 0x6a, 0xde]);
const IID_IXMLDOMSCHEMACOLLECTION2: Guid =
    guid(0x50ea_08b0, 0xdd1b, 0x4664, [0x9a, 0x50, 0xc2, 0xf4, 0x0f, 0x4b, 0xd7, 0x9a]);

// IXMLDOMSchemaCollection(2)'s slots (msxml6.h).
const SC_ADD: usize = 7;
const SC_GET: usize = 8;
const SC_REMOVE: usize = 9;
const SC_GET_LENGTH: usize = 10;
const SC_GET_NAMESPACE_URI: usize = 11;
const SC_ADD_COLLECTION: usize = 12;
const SC_VALIDATE: usize = 14;
const SC_PUT_VALIDATE_ON_LOAD: usize = 15;
const SC_GET_VALIDATE_ON_LOAD: usize = 16;
// IXMLDOMDocument2 and 3.
const PUT_VALIDATE_ON_PARSE: usize = 68;
const GET_SCHEMAS: usize = 77;
const PUTREF_SCHEMAS: usize = 78;
const VALIDATE: usize = 79;
const VALIDATE_NODE: usize = 82;
const PE_GET_REASON: usize = 9;

const E_POINTER: HResult = 0x8000_4003u32 as i32;
const E_NOTIMPL: HResult = 0x8000_4001u32 as i32;
/// IXMLDOMDocument2::validate's documented codes.
const XML_E_NOTWF: u32 = 0xc00c_e223;
const XML_E_NODTD: u32 = 0xc00c_e224;

const CFG_NS: &str = "urn:gala:cfg";
const EXT_NS: &str = "urn:gala:ext";

/// A configuration's schema: a sequence with occurrence bounds, a
/// choice, an element of another namespace (by `ref`, its schema
/// imported without a location), attributes required, defaulted and
/// typed, simple types restricted by pattern, length and enumeration, an
/// `all` group and a unique constraint.
const CFG_XSD: &str = "<xs:schema xmlns:xs='http://www.w3.org/2001/XMLSchema' \
xmlns:c='urn:gala:cfg' xmlns:e='urn:gala:ext' targetNamespace='urn:gala:cfg' \
elementFormDefault='qualified'>\
<xs:import namespace='urn:gala:ext'/>\
<xs:element name='Config'><xs:complexType><xs:sequence>\
<xs:element name='Name' type='c:ST_Name'/>\
<xs:element name='Item' type='c:CT_Item' minOccurs='0' maxOccurs='3'/>\
<xs:choice minOccurs='0'><xs:element name='A' type='xs:boolean'/>\
<xs:element name='B' type='xs:unsignedInt'/></xs:choice>\
<xs:element ref='e:Ext' minOccurs='0'/>\
</xs:sequence>\
<xs:attribute name='Version' type='xs:unsignedShort' use='required'/>\
<xs:attribute name='Mode' type='c:ST_Mode' default='on'/>\
</xs:complexType>\
<xs:unique name='ItemId'><xs:selector xpath='c:Item'/><xs:field xpath='@Id'/></xs:unique>\
</xs:element>\
<xs:simpleType name='ST_Name'><xs:restriction base='xs:string'>\
<xs:pattern value='[A-Za-z][-_. A-Za-z0-9]*'/><xs:maxLength value='16'/>\
</xs:restriction></xs:simpleType>\
<xs:simpleType name='ST_Mode'><xs:restriction base='xs:token'>\
<xs:enumeration value='on'/><xs:enumeration value='off'/></xs:restriction></xs:simpleType>\
<xs:complexType name='CT_Item'><xs:all><xs:element name='Path' type='xs:string'/>\
<xs:element name='Flag' type='xs:boolean' minOccurs='0'/></xs:all>\
<xs:attribute name='Id' type='xs:string' use='required'/></xs:complexType>\
</xs:schema>";

/// The other namespace's schema: simple content extended by an
/// attribute.
const EXT_XSD: &str = "<xs:schema xmlns:xs='http://www.w3.org/2001/XMLSchema' \
targetNamespace='urn:gala:ext' elementFormDefault='qualified'>\
<xs:element name='Ext'><xs:complexType><xs:simpleContent>\
<xs:extension base='xs:string'><xs:attribute name='k' type='xs:hexBinary'/>\
</xs:extension></xs:simpleContent></xs:complexType></xs:element>\
</xs:schema>";

/// An XDR schema (Wine's schema.c xdr_schema1_xml): MSXML 6 has no XDR.
const XDR: &str = "<?xml version='1.0'?><Schema xmlns='urn:schemas-microsoft-com:xml-data' \
xmlns:dt='urn:schemas-microsoft-com:datatypes' name='test1.xdr'>\
<ElementType name='x' dt:type='boolean'/></Schema>";

const HEAD: &str = "<Config xmlns='urn:gala:cfg' xmlns:e='urn:gala:ext' Version='3'";

/// Documents valid against the two schemas (the head's attributes, then
/// the content).
const VALID: [&str; 4] = [
    "><Name>first</Name></Config>",
    " Mode=' off '><Name>a.b-c d</Name><Item Id='1'><Path>p</Path></Item>\
<Item Id='2'><Flag>1</Flag><Path>q</Path></Item><Item Id='3'><Path/></Item>\
<A>true</A><e:Ext k='0aFF'>x</e:Ext></Config>",
    "><Name>n</Name><B>4294967295</B></Config>",
    ">\r\n  <Name>n</Name>\r\n  <!-- c --><e:Ext/>\r\n</Config>",
];

/// Documents that are not valid: (what is wrong, the document).
const INVALID: [(&str, &str); 15] = [
    ("required attribute", "<Config xmlns='urn:gala:cfg'><Name>n</Name></Config>"),
    ("enumeration", " Mode='maybe'><Name>n</Name></Config>"),
    ("pattern", "><Name>1abc</Name></Config>"),
    ("maxLength", "><Name>abcdefghijklmnopq</Name></Config>"),
    (
        "maxOccurs",
        "><Name>n</Name><Item Id='1'><Path/></Item><Item Id='2'><Path/></Item>\
<Item Id='3'><Path/></Item><Item Id='4'><Path/></Item></Config>",
    ),
    ("all incomplete", "><Name>n</Name><Item Id='1'><Flag>0</Flag></Item></Config>"),
    ("unknown element", "><Name>n</Name><Zap/></Config>"),
    ("order", "><Item Id='1'><Path/></Item><Name>n</Name></Config>"),
    ("unsignedInt", "><Name>n</Name><B>-1</B></Config>"),
    ("undeclared attribute", " Foo='1'><Name>n</Name></Config>"),
    ("unique", "><Name>n</Name><Item Id='1'><Path/></Item><Item Id='1'><Path/></Item></Config>"),
    ("text", ">text<Name>n</Name></Config>"),
    ("hexBinary", "><Name>n</Name><e:Ext k='zz'/></Config>"),
    ("incomplete", "></Config>"),
    ("undeclared root", "<c:Other xmlns:c='urn:gala:cfg'/>"),
];

/// HEAD + `rest`, or `rest` when it is a whole document.
fn document(rest: &str, buf: &mut [u8; 1024]) -> usize {
    let mut n = 0;
    let parts: [&str; 2] = if rest.starts_with('<') { ["", rest] } else { [HEAD, rest] };
    for p in parts {
        for &b in p.as_bytes() {
            buf[n] = b;
            n += 1;
        }
    }
    n
}

fn with_document<T>(rest: &str, f: impl FnOnce(&str) -> T) -> T {
    let mut buf = [0u8; 1024];
    let n = document(rest, &mut buf);
    f(core::str::from_utf8(&buf[..n]).unwrap_or(""))
}

fn new_cache(c: &mut Checks) -> Result<Obj, u32> {
    let mut cache: Obj = null_mut();
    // SAFETY: valid GUIDs and output.
    let hr = unsafe {
        CoCreateInstance(
            &CLSID_XMLSCHEMACACHE60,
            null_mut(),
            CLSCTX_INPROC_SERVER,
            &IID_IXMLDOMSCHEMACOLLECTION2,
            &mut cache,
        )
    };
    c.hr(hr, S_OK)?;
    c.ok(!cache.is_null(), 0)?;
    Ok(cache)
}

fn new_document(c: &mut Checks) -> Result<Obj, u32> {
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

/// A document holding `xml` (loaded with validateOnParse off).
fn loaded(c: &mut Checks, xml: &str) -> Result<Obj, u32> {
    let doc = new_document(c)?;
    c.hr(call!(doc, PUT_VALIDATE_ON_PARSE, fn(i16) -> HResult, VARIANT_FALSE), S_OK)?;
    let (hr, ok) = load_xml(doc, xml);
    c.hr(hr, S_OK)?;
    c.eq(ok as u16 as u64, VARIANT_TRUE as u16 as u64)?;
    Ok(doc)
}

fn dispatch(obj: Obj) -> Variant {
    Variant { vt: VT_DISPATCH, reserved: [0; 3], value: [obj as usize, 0] }
}

fn add(cache: Obj, ns: &str, schema: Variant) -> HResult {
    let n = Wide::new(ns);
    call!(cache, SC_ADD, fn(Bstr, Variant) -> HResult, n.0, schema)
}

/// Adds the schema text `xsd` for `ns`.
fn add_text(c: &mut Checks, cache: Obj, ns: &str, xsd: &str) -> Result<HResult, u32> {
    let doc = loaded(c, xsd)?;
    let hr = add(cache, ns, dispatch(doc));
    release(doc);
    Ok(hr)
}

fn expect_len(c: &mut Checks, cache: Obj, want: u64) -> Result<(), u32> {
    let n = length(c, cache)?;
    c.eq(n, want)
}

fn expect_add(c: &mut Checks, cache: Obj, ns: &str, xsd: &str, want: HResult) -> Result<(), u32> {
    let hr = add_text(c, cache, ns, xsd)?;
    c.hr(hr, want)
}

fn length(c: &mut Checks, cache: Obj) -> Result<u64, u32> {
    let mut n = -1i32;
    c.hr(call!(cache, SC_GET_LENGTH, fn(*mut i32) -> HResult, &mut n), S_OK)?;
    Ok(n as u32 as u64)
}

/// validate: its result and the error's code (the error released).
fn validate(c: &mut Checks, doc: Obj) -> Result<(HResult, u32), u32> {
    let mut err: Obj = null_mut();
    let hr = call!(doc, VALIDATE, fn(*mut Obj) -> HResult, &mut err);
    c.ok(!err.is_null(), hr as u32 as u64)?;
    let mut code = 0x55i32;
    let code_hr = call!(err, PE_GET_ERROR_CODE, fn(*mut i32) -> HResult, &mut code);
    if hr == S_OK {
        // A valid document's error: errorCode S_FALSE and 0, no reason.
        c.hr(code_hr, S_FALSE)?;
        let mut reason: Bstr = 0x1 as Bstr;
        c.hr(call!(err, PE_GET_REASON, fn(*mut Bstr) -> HResult, &mut reason), S_FALSE)?;
        c.ok(reason.is_null(), reason as u64)?;
    } else {
        c.hr(code_hr, S_OK)?;
    }
    release(err);
    Ok((hr, code as u32))
}

fn info_code(what: &str, code: u32) {
    Line::new()
        .s(NAME.as_bytes())
        .s(b": info ")
        .s(what.as_bytes())
        .s(b" ")
        .hex(code.into())
        .s(b"\r\n")
        .print();
}

pub fn schema(c: &mut Checks) -> Result<(), u32> {
    let cache = new_cache(c)?;
    collection(c, cache)?;
    documents(c, cache)?;
    on_load(c, cache)?;
    resources(c)?;
    deferred(c)?;
    other_collections(c, cache)?;
    release(cache);
    Ok(())
}

/// The collection itself: interfaces, length, namespaces, add's refusals,
/// get, remove.
fn collection(c: &mut Checks, cache: Obj) -> Result<(), u32> {
    for iid in [&IID_IXMLDOMSCHEMACOLLECTION, &IID_IDISPATCH, &IID_IUNKNOWN] {
        let (hr, o) = query(cache, iid);
        c.hr(hr, S_OK)?;
        release(o);
    }
    // The class's CLSID as an IID (App-V's ConfigurePackage asks for it
    // when it creates the cache): what Windows answers, and whether the
    // pointer is the collection's, are info lines.
    let (hr, o) = query(cache, &CLSID_XMLSCHEMACACHE60);
    info_hr("QueryInterface(CLSID_XMLSchemaCache60)", hr);
    info_hr("its pointer is the collection's", i32::from(o == cache));
    release(o);
    c.hr(call!(cache, SC_GET_LENGTH, fn(*mut i32) -> HResult, null_mut()), E_POINTER)?;
    expect_len(c, cache, 0)?;
    let none = Wide::new("urn:none");
    c.hr(call!(cache, SC_REMOVE, fn(Bstr) -> HResult, none.0), S_OK)?;
    get_bool(c, cache, SC_GET_VALIDATE_ON_LOAD, VARIANT_TRUE)?;
    // XDR is gone from MSXML 6; a document that is no schema is refused.
    expect_add(c, cache, "", XDR, E_FAIL)?;
    expect_add(c, cache, "", "<a/>", E_FAIL)?;
    expect_len(c, cache, 0)?;
    // A namespace that is not the schema's target.
    let hr = add_text(c, cache, "urn:other", EXT_XSD)?;
    info_hr("add under another namespace", hr);
    let other = Wide::new("urn:other");
    c.hr(call!(cache, SC_REMOVE, fn(Bstr) -> HResult, other.0), S_OK)?;
    // The other namespace first, then the schema that refers to it.
    expect_add(c, cache, EXT_NS, EXT_XSD, S_OK)?;
    expect_len(c, cache, 1)?;
    expect_add(c, cache, CFG_NS, CFG_XSD, S_OK)?;
    expect_len(c, cache, 2)?;
    // namespaceURI: both, in some order; past the end E_FAIL with the
    // output untouched; a NULL output E_POINTER (checked first).
    let mut seen = [false; 2];
    for i in 0..2 {
        let mut s: Bstr = null_mut();
        c.hr(call!(cache, SC_GET_NAMESPACE_URI, fn(i32, *mut Bstr) -> HResult, i, &mut s), S_OK)?;
        info_text("namespaceURI", s);
        let is_cfg = bstr_is(s, CFG_NS);
        let is_ext = bstr_is(s, EXT_NS);
        // SAFETY: our BSTR.
        unsafe { SysFreeString(s) };
        c.ok(is_cfg || is_ext, i as u64)?;
        seen[usize::from(is_cfg)] = true;
    }
    c.ok(seen[0] && seen[1], 0)?;
    let mut s: Bstr = 0x1 as Bstr;
    c.hr(call!(cache, SC_GET_NAMESPACE_URI, fn(i32, *mut Bstr) -> HResult, 2, &mut s), E_FAIL)?;
    c.ok(s == 0x1 as Bstr, s as u64)?;
    c.hr(
        call!(cache, SC_GET_NAMESPACE_URI, fn(i32, *mut Bstr) -> HResult, 0, null_mut()),
        E_POINTER,
    )?;
    c.hr(
        call!(cache, SC_GET_NAMESPACE_URI, fn(i32, *mut Bstr) -> HResult, 2, null_mut()),
        E_POINTER,
    )?;
    // get is not implemented in MSXML 6.
    let ns = Wide::new(CFG_NS);
    let mut node: Obj = null_mut();
    c.hr(call!(cache, SC_GET, fn(Bstr, *mut Obj) -> HResult, ns.0, &mut node), E_NOTIMPL)?;
    // A namespace added again merges: a second document declaring the
    // same element is a conflict.
    expect_add(c, cache, EXT_NS, EXT_XSD, E_FAIL)?;
    expect_len(c, cache, 2)
}

/// validate and validateNode against the cache, the `schemas` property.
fn documents(c: &mut Checks, cache: Obj) -> Result<(), u32> {
    // No document element: XML_E_NOTWF; no schema: XML_E_NODTD.
    let empty = new_document(c)?;
    let (hr, code) = validate(c, empty)?;
    c.hr(hr, S_FALSE)?;
    c.eq(code.into(), XML_E_NOTWF.into())?;
    release(empty);
    let doc = with_document(VALID[0], |x| loaded(c, x))?;
    let (hr, code) = validate(c, doc)?;
    c.hr(hr, S_FALSE)?;
    c.eq(code.into(), XML_E_NODTD.into())?;
    // schemas: none is VT_NULL and S_FALSE; set, it is the cache.
    let mut v = Variant::i4(7);
    c.hr(call!(doc, GET_SCHEMAS, fn(*mut Variant) -> HResult, &mut v), S_FALSE)?;
    c.eq(v.vt.into(), VT_NULL.into())?;
    c.hr(call!(doc, PUTREF_SCHEMAS, fn(Variant) -> HResult, Variant::i4(1)), E_FAIL)?;
    c.hr(call!(doc, PUTREF_SCHEMAS, fn(Variant) -> HResult, dispatch(cache)), S_OK)?;
    let mut v = Variant::empty();
    c.hr(call!(doc, GET_SCHEMAS, fn(*mut Variant) -> HResult, &mut v), S_OK)?;
    c.eq(v.vt.into(), VT_DISPATCH.into())?;
    c.ok(v.value[0] == cache as usize, v.value[0] as u64)?;
    v.clear();
    release(doc);
    for (i, rest) in VALID.iter().enumerate() {
        let doc = with_document(rest, |x| loaded(c, x))?;
        c.hr(call!(doc, PUTREF_SCHEMAS, fn(Variant) -> HResult, dispatch(cache)), S_OK)?;
        let (hr, code) = validate(c, doc)?;
        c.ok(hr == S_OK, (i as u64) << 32 | u64::from(code))?;
        // validateNode of the document element by its declaration.
        let (hr, root) = get_obj(doc, GET_DOCUMENT_ELEMENT);
        c.hr(hr, S_OK)?;
        let mut err: Obj = null_mut();
        let hr = call!(doc, VALIDATE_NODE, fn(Obj, *mut Obj) -> HResult, root, &mut err);
        c.hr(hr, S_OK)?;
        release(err);
        release(root);
        release(doc);
    }
    for (what, rest) in INVALID {
        let doc = with_document(rest, |x| loaded(c, x))?;
        c.hr(call!(doc, PUTREF_SCHEMAS, fn(Variant) -> HResult, dispatch(cache)), S_OK)?;
        let (hr, code) = validate(c, doc)?;
        info_code(what, code);
        c.hr(hr, S_FALSE)?;
        c.ok(code & 0x8000_0000 != 0, code.into())?;
        release(doc);
    }
    // A document whose namespace has no schema.
    let doc = loaded(c, "<r xmlns='urn:nothing'/>")?;
    c.hr(call!(doc, PUTREF_SCHEMAS, fn(Variant) -> HResult, dispatch(cache)), S_OK)?;
    let (hr, code) = validate(c, doc)?;
    info_code("no schema for the namespace", code);
    c.hr(hr, S_FALSE)?;
    // VT_NULL clears schemas.
    let null = Variant { vt: VT_NULL, reserved: [0; 3], value: [0; 2] };
    c.hr(call!(doc, PUTREF_SCHEMAS, fn(Variant) -> HResult, null), S_OK)?;
    let mut v = Variant::empty();
    c.hr(call!(doc, GET_SCHEMAS, fn(*mut Variant) -> HResult, &mut v), S_FALSE)?;
    c.eq(v.vt.into(), VT_NULL.into())?;
    release(doc);
    Ok(())
}

/// validateOnParse with a cache: an invalid document does not load (the
/// parse error says why, the document is empty); with validateOnParse
/// off it loads; a namespace with no schema is not validated.
fn on_load(c: &mut Checks, cache: Obj) -> Result<(), u32> {
    let doc = new_document(c)?;
    c.hr(call!(doc, PUTREF_SCHEMAS, fn(Variant) -> HResult, dispatch(cache)), S_OK)?;
    let (hr, ok) = with_document(INVALID[2].1, |x| load_xml(doc, x));
    c.hr(hr, S_FALSE)?;
    c.eq(ok as u16 as u64, VARIANT_FALSE as u16 as u64)?;
    let (hr, err) = get_obj(doc, GET_PARSE_ERROR);
    c.hr(hr, S_OK)?;
    let mut code = 0i32;
    c.hr(call!(err, PE_GET_ERROR_CODE, fn(*mut i32) -> HResult, &mut code), S_OK)?;
    info_code("invalid on load", code as u32);
    c.ok(code < 0, code as u32 as u64)?;
    release(err);
    let (hr, root) = get_obj(doc, GET_DOCUMENT_ELEMENT);
    c.hr(hr, S_FALSE)?;
    c.ok(root.is_null(), 0)?;
    let (hr, ok) = with_document(VALID[1], |x| load_xml(doc, x));
    c.hr(hr, S_OK)?;
    c.eq(ok as u16 as u64, VARIANT_TRUE as u16 as u64)?;
    let (hr, ok) = load_xml(doc, "<r xmlns='urn:nothing'><x/></r>");
    c.hr(hr, S_OK)?;
    c.eq(ok as u16 as u64, VARIANT_TRUE as u16 as u64)?;
    c.hr(call!(doc, PUT_VALIDATE_ON_PARSE, fn(i16) -> HResult, VARIANT_FALSE), S_OK)?;
    let (hr, ok) = with_document(INVALID[2].1, |x| load_xml(doc, x));
    c.hr(hr, S_OK)?;
    c.eq(ok as u16 as u64, VARIANT_TRUE as u16 as u64)?;
    release(doc);
    Ok(())
}

#[cfg_attr(
    target_arch = "x86",
    link(name = "kernel32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "kernel32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn GetModuleFileNameW(module: Handle, buf: *mut u16, size: u32) -> u32;
}

/// `res://<this program>/<rest>`.
fn res_url(rest: &str) -> Wide {
    let mut path = [0u16; 512];
    // SAFETY: our buffer.
    let n = unsafe { GetModuleFileNameW(0, path.as_mut_ptr(), 400) } as usize;
    let mut buf = [0u16; 1024];
    let units = "res://"
        .encode_utf16()
        .chain(path[..n].iter().copied())
        .chain("/".encode_utf16())
        .chain(rest.encode_utf16());
    for (slot, u) in buf.iter_mut().zip(units) {
        *slot = u;
    }
    // SAFETY: a NUL-terminated string.
    Wide(unsafe { SysAllocString(buf.as_ptr()) })
}

/// A schema from a `res:` URL (urlmon's res protocol, which MSXML loads
/// through): this program's RT_HTML resource 1 (build.rs: EXT_XSD in
/// UTF-16 with a BOM), by the default type and by type #23.
fn resources(c: &mut Checks) -> Result<(), u32> {
    for rest in ["#1", "#23/#1"] {
        let cache = new_cache(c)?;
        let url = res_url(rest);
        let v = Variant::bstr(&url);
        c.hr(add(cache, EXT_NS, v), S_OK)?;
        expect_len(c, cache, 1)?;
        release(cache);
    }
    let doc = new_document(c)?;
    c.hr(call!(doc, PUT_ASYNC, fn(i16) -> HResult, VARIANT_FALSE), S_OK)?;
    let url = res_url("#1");
    let mut ok: i16 = 0x55;
    c.hr(call!(doc, LOAD, fn(Variant, *mut i16) -> HResult, Variant::bstr(&url), &mut ok), S_OK)?;
    c.eq(ok as u16 as u64, VARIANT_TRUE as u16 as u64)?;
    release(doc);
    Ok(())
}

/// validateOnLoad: on, a schema naming an element of a namespace not in
/// the cache (info); off, it is added and the collection's validate
/// compiles later (info while unresolved, S_OK once the other namespace
/// is there).
fn deferred(c: &mut Checks) -> Result<(), u32> {
    let cache = new_cache(c)?;
    let hr = add_text(c, cache, CFG_NS, CFG_XSD)?;
    info_hr("add with an unresolved reference", hr);
    release(cache);
    let cache = new_cache(c)?;
    c.hr(call!(cache, SC_PUT_VALIDATE_ON_LOAD, fn(i16) -> HResult, VARIANT_FALSE), S_OK)?;
    get_bool(c, cache, SC_GET_VALIDATE_ON_LOAD, VARIANT_FALSE)?;
    expect_add(c, cache, CFG_NS, CFG_XSD, S_OK)?;
    info_hr("validate while unresolved", call!(cache, SC_VALIDATE, fn() -> HResult,));
    expect_add(c, cache, EXT_NS, EXT_XSD, S_OK)?;
    c.hr(call!(cache, SC_VALIDATE, fn() -> HResult,), S_OK)?;
    expect_len(c, cache, 2)?;
    // Adding VT_NULL removes the namespace.
    let null = Variant { vt: VT_NULL, reserved: [0; 3], value: [0; 2] };
    c.hr(add(cache, CFG_NS, null), S_OK)?;
    expect_len(c, cache, 1)?;
    release(cache);
    Ok(())
}

/// addCollection: NULL is E_POINTER; another cache's schemas join.
fn other_collections(c: &mut Checks, cache: Obj) -> Result<(), u32> {
    let other = new_cache(c)?;
    c.hr(call!(other, SC_ADD_COLLECTION, fn(Obj) -> HResult, null_mut()), E_POINTER)?;
    c.hr(call!(other, SC_ADD_COLLECTION, fn(Obj) -> HResult, cache), S_OK)?;
    expect_len(c, other, 2)?;
    let doc = with_document(VALID[1], |x| loaded(c, x))?;
    c.hr(call!(doc, PUTREF_SCHEMAS, fn(Variant) -> HResult, dispatch(other)), S_OK)?;
    let (hr, _) = validate(c, doc)?;
    c.hr(hr, S_OK)?;
    release(doc);
    release(other);
    Ok(())
}
