//! XSLT: XSLTemplate60 (IXSLTemplate), its processor (IXSLProcessor),
//! and IXMLDOMNode's transformNode and transformNodeToObject.
//! Click-to-Run's App-V pieces ask COM for XSLTemplate60 and transform
//! a package manifest before validating it (ROADMAP OF): a clean-up that
//! drops what is not in the namespaces a parameter lists, a
//! normalization of older namespaces into the newest, and an alias step
//! that declares a prefix through `msxsl:node-set()` and the namespace
//! axis. The stylesheets here are this test's own, shaped like those:
//! the same instructions and functions, not App-V's text.
//!
//! Expected values are Wine's msxml3 tests run on Windows (domdoc.c:
//! test_xsltemplate: putref_stylesheet(NULL) resets, get_stylesheet(NULL)
//! E_INVALIDARG and NULL after a reset, a document that is not a
//! stylesheet and a stylesheet that is not free-threaded E_FAIL, the
//! stylesheet given back, createProcessor(NULL) and
//! get_ownerTemplate(NULL) E_INVALIDARG, the owner template given back,
//! get_input VT_EMPTY before an input, get_output(NULL) and
//! transform(NULL) E_INVALIDARG, a transform with no output object an
//! empty VT_BSTR, a document as the output (VT_UNKNOWN from get_output,
//! the document filled); test_testTransforms and test_xsltext:
//! transformNode's text for an html, a text-only and an
//! omit-xml-declaration result; test_transformNodeToObject: an object
//! that is neither a document nor a stream E_INVALIDARG, a stream gets
//! the output in its encoding, UTF-8 without and with the declaration
//! (`encoding="utf-8"` as the stylesheet names it), UTF-16 with a byte
//! order mark) and XSLT 1.0's rules for what a transform makes (W3C,
//! 1999: templates by priority, the built-in rules, variables and
//! params, result tree fragments, the namespace axis), checked on the
//! result's tree: names, namespaces, values. Where the text of a result
//! is not settled by those (which prefix a namespace gets, where its
//! declaration goes, a string output's declaration), it is an `info`
//! line until Windows' run.

use core::ptr::null_mut;

use super::mutation::{info_hr, section};
use super::*;

const VT_UNKNOWN: u16 = 13;
const VT_DISPATCH: u16 = 9;

const CLSID_XSLTEMPLATE60: Guid =
    guid(0x88d9_6a08, 0xf192, 0x11d4, [0xa6, 0x5f, 0x00, 0x40, 0x96, 0x32, 0x51, 0xe5]);
const IID_IXSLTEMPLATE: Guid = guid(0x2933_bf93, 0x7b36, 0x11d2, DOM_D4);
const IID_IXSLPROCESSOR: Guid = guid(0x2933_bf92, 0x7b36, 0x11d2, DOM_D4);

// IXSLTemplate's slots (msxml6.h).
const T_PUTREF_STYLESHEET: usize = 7;
const T_GET_STYLESHEET: usize = 8;
const T_CREATE_PROCESSOR: usize = 9;
// IXSLProcessor's.
const P_PUT_INPUT: usize = 7;
const P_GET_INPUT: usize = 8;
const P_GET_OWNER_TEMPLATE: usize = 9;
const P_PUT_OUTPUT: usize = 13;
const P_GET_OUTPUT: usize = 14;
const P_TRANSFORM: usize = 15;
const P_ADD_PARAMETER: usize = 18;
// IXMLDOMNode's.
const TRANSFORM_NODE: usize = 35;
const TRANSFORM_NODE_TO_OBJECT: usize = 42;
// IStream's.
const STREAM_READ: usize = 3;
const STREAM_SEEK: usize = 5;

#[cfg_attr(
    target_arch = "x86",
    link(name = "ole32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "ole32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn CreateStreamOnHGlobal(global: usize, delete: i32, out: *mut Obj) -> HResult;
}

/// Wine's szTransformXML and szTransformSSXML (an html output) and the
/// text transformNode gives (compared without line ends).
const GREETING: &str = "<?xml version=\"1.0\"?>\n<greeting>\nHello World\n</greeting>";
const GREETING_XSL: &str = "<xsl:stylesheet xmlns:xsl=\"http://www.w3.org/1999/XSL/Transform\" \
version=\"1.0\">\n   <xsl:output method=\"html\"/>\n   <xsl:template match=\"/\">\n\
       <xsl:apply-templates select=\"greeting\"/>\n   </xsl:template>\n\
   <xsl:template match=\"greeting\">\n       <html>\n           <body>\n\
               <h1>\n                   <xsl:value-of select=\".\"/>\n\
               </h1>\n           </body>\n       </html>\n   </xsl:template>\n\
</xsl:stylesheet>";
const GREETING_OUT: &str = "<html><body><h1>Hello World</h1></body></html>";

/// Wine's xsltext_xsl: text only, from a choose.
const XSLTEXT_XSL: &str = "<?xml version=\"1.0\"?><xsl:stylesheet version=\"1.0\" \
xmlns:xsl=\"http://www.w3.org/1999/XSL/Transform\" ><xsl:output method=\"html\" \
encoding=\"us-ascii\"/><xsl:template match=\"/\">    <xsl:choose>        \
<xsl:when test=\"testkey\">            <xsl:text>testdata</xsl:text>        </xsl:when>\
    </xsl:choose></xsl:template></xsl:stylesheet>";

/// Wine's omitxmldecl_xsl and omitxmldecl_doc: for-each, element.
const OMIT_XSL: &str = "<?xml version=\"1.0\"?><xsl:stylesheet version=\"1.0\" \
xmlns:xsl=\"http://www.w3.org/1999/XSL/Transform\" ><xsl:output method=\"xml\" \
omit-xml-declaration=\"yes\"/><xsl:template match=\"/\">    <xsl:for-each select=\"/a/item\">\
        <xsl:element name=\"node\">            <xsl:value-of select=\"@name\"/>        \
</xsl:element>    </xsl:for-each></xsl:template></xsl:stylesheet>";
const OMIT_DOC: &str = "<?xml version=\"1.0\"?><a>    <item name=\"item1\"/>    \
<item name=\"item2\"/></a>";

/// Wine's identity transforms with an output encoding (and the
/// declaration omitted).
const COPY_HEAD: &str = "<?xml version=\"1.0\"?>\n<xsl:stylesheet \
xmlns:xsl=\"http://www.w3.org/1999/XSL/Transform\" version=\"1.0\">\n";
const COPY_BODY: &str = "<xsl:template match=\"@* | node()\" >    <xsl:copy>        \
<xsl:apply-templates select=\"@* | node()\" />    </xsl:copy></xsl:template></xsl:stylesheet>";
const ITEMS_DOC: &str = "<?xml version=\"1.0\" encoding=\"utf-8\"?><a>    <item>item1</item>    \
<item>item2</item></a>";
const ITEMS_OUT: &str = "<a><item>item1</item><item>item2</item></a>";

const PKG_NS: &str = "urn:gala:pkg";
const NEW_NS: &str = "urn:gala:new";

/// A manifest shaped like App-V's: a root in one namespace listing
/// ignorable prefixes, attributes and elements in an older namespace
/// and in one to ignore, a comment, text.
const MANIFEST: &str = "<p:Package xmlns:p=\"urn:gala:pkg\" xmlns:o=\"urn:gala:old\" \
xmlns:x=\"urn:gala:x\" Ignore=\"o x\" Keep=\"1\" x:drop=\"d\">\
<p:Id Name=\"n\" o:Ver=\"2\" x:z=\"z\"/>\
<o:Ext o:Kind=\"k\" Plain=\"v\"><o:Item>one</o:Item><x:Skip/></o:Ext>\
<x:Gone><p:Inner/></x:Gone><!-- note --><p:Text>a &amp; b</p:Text></p:Package>";

/// The normalization: params, a top-level variable made by a recursive
/// named template (the prefixes `Ignore` lists turned into their
/// namespaces through the namespace axis), templates by priority
/// (`/*/@*` over `@*`), choose, element and attribute with namespace
/// value templates, copy, the core string functions.
const NORMALIZE_XSL: &str = "<xsl:transform version=\"1.0\" \
xmlns:xsl=\"http://www.w3.org/1999/XSL/Transform\">\
<xsl:param name=\"known\"/><xsl:param name=\"to\"/>\
<xsl:output method=\"xml\" omit-xml-declaration=\"no\" encoding=\"UTF-8\"/>\
<xsl:variable name=\"ignored\"><xsl:call-template name=\"prefixes\">\
<xsl:with-param name=\"list\" select=\"/*/@Ignore\"/></xsl:call-template></xsl:variable>\
<xsl:template name=\"prefixes\"><xsl:param name=\"list\"/>\
<xsl:variable name=\"norm\" select=\"normalize-space($list)\"/>\
<xsl:choose><xsl:when test=\"contains($norm, ' ')\">\
<xsl:variable name=\"ns\" select=\"/*/namespace::*[name() = substring-before($norm, ' ')]\"/>\
<xsl:if test=\"not(string-length($ns) = 0)\"><xsl:value-of select=\"concat('[', $ns, ']')\"/></xsl:if>\
<xsl:call-template name=\"prefixes\"><xsl:with-param name=\"list\" \
select=\"substring-after($norm, ' ')\"/></xsl:call-template></xsl:when>\
<xsl:when test=\"string-length($norm) != 0\">\
<xsl:variable name=\"ns\" select=\"/*/namespace::*[name() = $norm]\"/>\
<xsl:if test=\"$ns\"><xsl:value-of select=\"concat('[', $ns, ']')\"/></xsl:if></xsl:when>\
<xsl:otherwise/></xsl:choose></xsl:template>\
<xsl:template match=\"*\"><xsl:variable name=\"m\" select=\"concat('[', namespace-uri(), ']')\"/>\
<xsl:choose><xsl:when test=\"contains($known, $m)\">\
<xsl:element name=\"{local-name()}\" namespace=\"{$to}\">\
<xsl:apply-templates select=\"node() | @*\"/></xsl:element></xsl:when>\
<xsl:when test=\"not(contains($ignored, $m))\"><xsl:copy>\
<xsl:apply-templates select=\"node() | @*\"/></xsl:copy></xsl:when>\
<xsl:otherwise/></xsl:choose></xsl:template>\
<xsl:template match=\"@*\"><xsl:variable name=\"m\" select=\"concat('[', namespace-uri(), ']')\"/>\
<xsl:choose><xsl:when test=\"contains($known, $m)\"><xsl:choose>\
<xsl:when test=\"contains($known, concat('[', namespace-uri(parent::node()), ']'))\">\
<xsl:attribute name=\"{local-name()}\" namespace=\"\"><xsl:value-of select=\".\"/></xsl:attribute>\
</xsl:when><xsl:otherwise>\
<xsl:attribute name=\"{local-name()}\" namespace=\"{$to}\"><xsl:value-of select=\".\"/></xsl:attribute>\
</xsl:otherwise></xsl:choose></xsl:when>\
<xsl:when test=\"not(contains($ignored, $m))\"><xsl:copy/></xsl:when><xsl:otherwise/></xsl:choose>\
</xsl:template>\
<xsl:template match=\"/*/@*\"><xsl:if test=\"local-name() != 'Ignore'\"><xsl:copy/></xsl:if>\
</xsl:template></xsl:transform>";

/// The alias step: a result tree fragment holding an element in the new
/// namespace with prefix `n`, whose namespace nodes `msxsl:node-set()`
/// and the namespace axis copy onto the root; every element and
/// attribute made again by name and namespace; a pattern with a
/// predicate for one attribute.
const ALIAS_XSL: &str = "<xsl:transform version=\"1.0\" \
xmlns:xsl=\"http://www.w3.org/1999/XSL/Transform\" xmlns:msxsl=\"urn:schemas-microsoft-com:xslt\" \
xmlns:p=\"urn:gala:pkg\">\
<xsl:output method=\"xml\" version=\"1.0\" omit-xml-declaration=\"no\" encoding=\"UTF-8\"/>\
<xsl:variable name=\"ns\" select=\"'urn:gala:new'\"/>\
<xsl:variable name=\"dummy\"><xsl:element name=\"n:dummy\" namespace=\"{$ns}\"/></xsl:variable>\
<xsl:template match=\"/*\"><xsl:element name=\"{local-name()}\" namespace=\"{namespace-uri()}\">\
<xsl:copy-of select=\"msxsl:node-set($dummy)//namespace::*\"/>\
<xsl:apply-templates select=\"node() | @*\"/></xsl:element></xsl:template>\
<xsl:template match=\"*\"><xsl:element name=\"{local-name()}\" namespace=\"{namespace-uri()}\">\
<xsl:apply-templates select=\"node() | @*\"/></xsl:element></xsl:template>\
<xsl:template match=\"@*\"><xsl:attribute name=\"{local-name()}\" namespace=\"{namespace-uri()}\">\
<xsl:value-of select=\".\"/></xsl:attribute></xsl:template>\
<xsl:template match=\"/p:Package/@Keep[namespace-uri() = '']\">\
<xsl:attribute name=\"{local-name()}\" namespace=\"{namespace-uri()}\">\
<xsl:value-of select=\"'n'\"/></xsl:attribute></xsl:template></xsl:transform>";

/// Literal result elements with attribute value templates, for-each
/// with position() and last(), if, text, a variable, a comment, modes
/// and the built-in rules (a text node copied).
const LITERAL_XSL: &str = "<xsl:stylesheet version=\"1.0\" \
xmlns:xsl=\"http://www.w3.org/1999/XSL/Transform\">\
<xsl:output omit-xml-declaration=\"yes\"/>\
<xsl:template match=\"/\"><r n=\"{count(//i)}\">\
<xsl:for-each select=\"//i\"><xsl:variable name=\"k\" select=\"concat(@k, position())\"/>\
<i k=\"{$k}\"><xsl:value-of select=\"translate(., 'abc', 'ABC')\"/></i>\
<xsl:if test=\"position() != last()\"><xsl:text>,</xsl:text></xsl:if></xsl:for-each>\
<xsl:comment>c</xsl:comment><xsl:apply-templates select=\"//t\" mode=\"m\"/></r></xsl:template>\
<xsl:template match=\"t\" mode=\"m\">[<xsl:apply-templates/>]</xsl:template>\
</xsl:stylesheet>";
const LITERAL_DOC: &str = "<d><i k=\"a\">abc</i><i k=\"b\">cab</i><t>x<u>y</u></t></d>";
const LITERAL_OUT: &str = "<r n=\"2\"><i k=\"a1\">ABC</i>,<i k=\"b2\">CAB</i><!--c-->[xy]</r>";

fn new_doc(c: &mut Checks, clsid: &Guid) -> Result<Obj, u32> {
    let mut doc: Obj = null_mut();
    // SAFETY: valid GUIDs and output.
    let hr = unsafe {
        CoCreateInstance(clsid, null_mut(), CLSCTX_INPROC_SERVER, &IID_IXMLDOMDOCUMENT2, &mut doc)
    };
    c.hr(hr, S_OK)?;
    Ok(doc)
}

/// A new document holding `xml`.
fn loaded(c: &mut Checks, clsid: &Guid, xml: &str) -> Result<Obj, u32> {
    let doc = new_doc(c, clsid)?;
    let (hr, ok) = load_xml(doc, xml);
    c.hr(hr, S_OK)?;
    c.eq(ok as u16 as u64, VARIANT_TRUE as u16 as u64)?;
    Ok(doc)
}

fn unknown(p: Obj) -> Variant {
    Variant { vt: VT_UNKNOWN, reserved: [0; 3], value: [p as usize, 0] }
}

/// Whether `s` (freed) is `want` with its line ends (CR, LF) left out.
fn same_without_returns(s: Bstr, want: &str) -> bool {
    let mut out = [0u16; 512];
    let mut n = 0;
    let mut i = 0;
    while !s.is_null() && n < out.len() {
        // SAFETY: a NUL-terminated string, read up to its NUL.
        let u = unsafe { *s.add(i) };
        if u == 0 {
            break;
        }
        if u != 13 && u != 10 {
            out[n] = u;
            n += 1;
        }
        i += 1;
    }
    // SAFETY: our BSTR (or NULL).
    unsafe { SysFreeString(s) };
    want.encode_utf16().eq(out[..n].iter().copied())
}

/// transformNode of `doc` with `stylesheet`: S_OK and the text.
fn transform_node(c: &mut Checks, doc: Obj, stylesheet: Obj) -> Result<Bstr, u32> {
    let mut s: Bstr = null_mut();
    let hr = call!(doc, TRANSFORM_NODE, fn(Obj, *mut Bstr) -> HResult, stylesheet, &mut s);
    c.hr(hr, S_OK)?;
    c.ok(!s.is_null(), 0)?;
    Ok(s)
}

/// The node `expr` selects from `doc` (with the test's prefixes): its
/// namespace URI should be `ns` (and its text `text`, when given).
fn node_is(c: &mut Checks, doc: Obj, expr: &str, ns: &str, text: Option<&str>) -> Result<(), u32> {
    let (hr, n) = select_single(doc, expr);
    c.hr(hr, S_OK)?;
    if ns.is_empty() {
        // No namespace: S_FALSE and NULL (or an empty string).
        let mut s: Bstr = null_mut();
        let hr = call!(n, GET_NAMESPACE_URI, fn(*mut Bstr) -> HResult, &mut s);
        let empty = s.is_null() || bstr_is(s, "");
        // SAFETY: our BSTR (or NULL).
        unsafe { SysFreeString(s) };
        c.ok(hr >= 0 && empty, hr as u32 as u64)?;
    } else {
        get_bstr(c, n, GET_NAMESPACE_URI, ns)?;
    }
    if let Some(t) = text {
        get_bstr(c, n, GET_TEXT, t)?;
    }
    release(n);
    Ok(())
}

/// `expr` should select nothing from `doc`.
fn absent(c: &mut Checks, doc: Obj, expr: &str) -> Result<(), u32> {
    let (hr, n) = select_single(doc, expr);
    c.hr(hr, S_FALSE)?;
    c.ok(n.is_null(), n as u64)
}

fn set_namespaces(c: &mut Checks, doc: Obj) -> Result<(), u32> {
    let name = Wide::new("SelectionNamespaces");
    let value = Wide::new("xmlns:p='urn:gala:pkg' xmlns:n='urn:gala:new' xmlns:x='urn:gala:x'");
    let hr = call!(doc, SET_PROPERTY, fn(Bstr, Variant) -> HResult, name.0, Variant::bstr(&value));
    c.hr(hr, S_OK)
}

pub fn xslt(c: &mut Checks) -> Result<(), u32> {
    section("xslt: transformNode");
    transform_nodes(c)?;
    section("xslt: transformNodeToObject");
    to_objects(c)?;
    section("xslt: template");
    template(c)?;
    section("xslt: App-V's shape");
    app_v(c)
}

/// transformNode (Wine's test_testTransforms and test_xsltext; a rental
/// stylesheet will do) and literal result elements.
fn transform_nodes(c: &mut Checks) -> Result<(), u32> {
    let doc = loaded(c, &CLSID_DOMDOCUMENT60, GREETING)?;
    let ss = loaded(c, &CLSID_DOMDOCUMENT60, GREETING_XSL)?;
    let s = transform_node(c, doc, ss)?;
    c.ok(same_without_returns(s, GREETING_OUT), 0)?;
    let mut s: Bstr = null_mut();
    let hr = call!(doc, TRANSFORM_NODE, fn(Obj, *mut Bstr) -> HResult, null_mut(), &mut s);
    c.hr(hr, E_INVALIDARG)?;
    let hr = call!(doc, TRANSFORM_NODE, fn(Obj, *mut Bstr) -> HResult, ss, null_mut());
    c.hr(hr, E_INVALIDARG)?;
    release(ss);
    release(doc);

    let ss = loaded(c, &CLSID_DOMDOCUMENT60, XSLTEXT_XSL)?;
    let doc = loaded(c, &CLSID_DOMDOCUMENT60, "<testkey/>")?;
    let s = transform_node(c, doc, ss)?;
    c.text(s, "testdata")?;
    release(doc);
    release(ss);

    let ss = loaded(c, &CLSID_DOMDOCUMENT60, OMIT_XSL)?;
    let doc = loaded(c, &CLSID_DOMDOCUMENT60, OMIT_DOC)?;
    let s = transform_node(c, doc, ss)?;
    c.text(s, "<node>item1</node><node>item2</node>")?;
    release(doc);
    release(ss);

    let ss = loaded(c, &CLSID_DOMDOCUMENT60, LITERAL_XSL)?;
    let doc = loaded(c, &CLSID_DOMDOCUMENT60, LITERAL_DOC)?;
    let s = transform_node(c, doc, ss)?;
    c.text(s, LITERAL_OUT)?;
    // An xml output's declaration in a string (the method given, the
    // declaration not omitted).
    let sheet = joined(&[COPY_HEAD, "<xsl:output method=\"xml\"/>", COPY_BODY]);
    let ss2 = loaded(c, &CLSID_DOMDOCUMENT60, sheet.as_str())?;
    let items = loaded(c, &CLSID_DOMDOCUMENT60, ITEMS_DOC)?;
    let s = transform_node(c, items, ss2)?;
    info_long("transformNode xml output", s);
    release(items);
    release(ss2);
    release(doc);
    release(ss);
    Ok(())
}

/// An `info` line with a string of any length (freed): 120 characters
/// a line, non-ASCII as `?`.
fn info_long(what: &str, s: Bstr) {
    let mut i = 0;
    let mut part = 0u64;
    loop {
        let mut line = Line::new()
            .s(NAME.as_bytes())
            .s(b": info ")
            .s(what.as_bytes())
            .s(b" [")
            .dec(part)
            .s(b"] ");
        let mut n = 0;
        let mut end = s.is_null();
        while !end && n < 120 {
            // SAFETY: a NUL-terminated string, read up to its NUL.
            let u = unsafe { *s.add(i) };
            if u == 0 {
                end = true;
                break;
            }
            line = line.s(&[if (0x20..0x7f).contains(&u) { u as u8 } else { b'?' }]);
            i += 1;
            n += 1;
        }
        line.s(b"\r\n").print();
        // SAFETY: as above (one past the last unit read is in the string).
        if end || unsafe { *s.add(i) } == 0 {
            break;
        }
        part += 1;
    }
    // SAFETY: our BSTR (or NULL).
    unsafe { SysFreeString(s) };
}

/// A small text buffer (the stylesheets put together from parts).
struct Text([u8; 600], usize);

impl Text {
    fn as_str(&self) -> &str {
        core::str::from_utf8(&self.0[..self.1]).unwrap_or("")
    }
}

fn joined(parts: &[&str]) -> Text {
    let mut t = Text([0; 600], 0);
    for p in parts {
        for &b in p.as_bytes() {
            if t.1 < t.0.len() {
                t.0[t.1] = b;
                t.1 += 1;
            }
        }
    }
    t
}

/// The bytes a stream holds (from its start), at most 512.
fn stream_bytes(stream: Obj, buf: &mut [u8; 512]) -> usize {
    call!(stream, STREAM_SEEK, fn(i64, u32, *mut u64) -> HResult, 0, 0, null_mut());
    let mut read = 0u32;
    call!(
        stream,
        STREAM_READ,
        fn(*mut u8, u32, *mut u32) -> HResult,
        buf.as_mut_ptr(),
        512,
        &mut read
    );
    read as usize
}

/// transformNodeToObject (Wine's test_transformNodeToObject): into a
/// document and into streams, in the output's encoding.
fn to_objects(c: &mut Checks) -> Result<(), u32> {
    let doc = loaded(c, &CLSID_DOMDOCUMENT60, GREETING)?;
    let ss = loaded(c, &CLSID_DOMDOCUMENT60, GREETING_XSL)?;
    let hr =
        call!(doc, TRANSFORM_NODE_TO_OBJECT, fn(Obj, Variant) -> HResult, ss, unknown(null_mut()));
    c.hr(hr, E_INVALIDARG)?;
    let target = new_doc(c, &CLSID_DOMDOCUMENT60)?;
    let v = Variant { vt: VT_DISPATCH, reserved: [0; 3], value: [target as usize, 0] };
    let hr = call!(doc, TRANSFORM_NODE_TO_OBJECT, fn(Obj, Variant) -> HResult, ss, v);
    c.hr(hr, S_OK)?;
    let (hr, el) = get_obj(target, GET_DOCUMENT_ELEMENT);
    c.hr(hr, S_OK)?;
    get_bstr(c, el, GET_NODE_NAME, "html")?;
    release(el);
    release(target);
    // A stream gets UTF-16 with a byte order mark (no encoding named).
    let mut stream: Obj = null_mut();
    // SAFETY: a valid output.
    c.hr(unsafe { CreateStreamOnHGlobal(0, 1, &mut stream) }, S_OK)?;
    let hr = call!(doc, TRANSFORM_NODE_TO_OBJECT, fn(Obj, Variant) -> HResult, ss, unknown(stream));
    c.hr(hr, S_OK)?;
    let mut buf = [0u8; 512];
    let n = stream_bytes(stream, &mut buf);
    c.ok(n >= 2 && buf[0] == 0xff && buf[1] == 0xfe, n as u64)?;
    let mut wide = [0u16; 256];
    let mut k = 0;
    for pair in buf[2..n].chunks(2) {
        let u = u16::from_le_bytes([pair[0], pair.get(1).copied().unwrap_or(0)]);
        if u != 13 && u != 10 && k < wide.len() {
            wide[k] = u;
            k += 1;
        }
    }
    c.ok(GREETING_OUT.encode_utf16().eq(wide[..k].iter().copied()), k as u64)?;
    release(stream);
    release(ss);
    release(doc);

    // UTF-8: the declaration omitted, then kept (as the stylesheet
    // names the encoding).
    let doc = loaded(c, &CLSID_DOMDOCUMENT60, ITEMS_DOC)?;
    for (output, want) in [
        (
            "<xsl:output method=\"xml\" version=\"1.0\" encoding=\"utf-8\" omit-xml-declaration=\"yes\"/>",
            ITEMS_OUT,
        ),
        ("<xsl:output method=\"xml\" version=\"1.0\" encoding=\"utf-8\"/>", ""),
    ] {
        let sheet = joined(&[COPY_HEAD, output, COPY_BODY]);
        let ss = loaded(c, &CLSID_DOMDOCUMENT60, sheet.as_str())?;
        let mut stream: Obj = null_mut();
        // SAFETY: a valid output.
        c.hr(unsafe { CreateStreamOnHGlobal(0, 1, &mut stream) }, S_OK)?;
        let hr =
            call!(doc, TRANSFORM_NODE_TO_OBJECT, fn(Obj, Variant) -> HResult, ss, unknown(stream));
        c.hr(hr, S_OK)?;
        let mut buf = [0u8; 512];
        let n = stream_bytes(stream, &mut buf);
        let got = &buf[..n];
        let ok = if want.is_empty() {
            let decl = b"<?xml version=\"1.0\" encoding=\"utf-8\"?>";
            got.starts_with(decl) && &got[decl.len()..] == ITEMS_OUT.as_bytes()
        } else {
            got == want.as_bytes()
        };
        c.ok(ok, n as u64)?;
        release(stream);
        release(ss);
    }
    release(doc);
    Ok(())
}

/// XSLTemplate60 and its processor (Wine's test_xsltemplate).
fn template(c: &mut Checks) -> Result<(), u32> {
    let mut t: Obj = null_mut();
    // SAFETY: valid GUIDs and output.
    let hr = unsafe {
        CoCreateInstance(
            &CLSID_XSLTEMPLATE60,
            null_mut(),
            CLSCTX_INPROC_SERVER,
            &IID_IXSLTEMPLATE,
            &mut t,
        )
    };
    c.hr(hr, S_OK)?;
    let (hr, d) = query(t, &IID_IDISPATCH);
    c.hr(hr, S_OK)?;
    release(d);
    c.hr(call!(t, T_PUTREF_STYLESHEET, fn(Obj) -> HResult, null_mut()), S_OK)?;
    c.hr(call!(t, T_GET_STYLESHEET, fn(*mut Obj) -> HResult, null_mut()), E_INVALIDARG)?;
    let (hr, n) = get_obj(t, T_GET_STYLESHEET);
    c.hr(hr, S_OK)?;
    c.ok(n.is_null(), n as u64)?;
    // Not a stylesheet: E_FAIL. A stylesheet in a rental document: MSXML 6
    // takes it (measured on Windows; Wine's msxml3 tests have E_FAIL).
    let not_xsl = loaded(c, &CLSID_FREETHREADEDDOMDOCUMENT60, "<a>test</a>")?;
    c.hr(call!(t, T_PUTREF_STYLESHEET, fn(Obj) -> HResult, not_xsl), E_FAIL)?;
    release(not_xsl);
    let rental = loaded(c, &CLSID_DOMDOCUMENT60, GREETING_XSL)?;
    c.hr(call!(t, T_PUTREF_STYLESHEET, fn(Obj) -> HResult, rental), S_OK)?;
    release(rental);
    let ss = loaded(c, &CLSID_FREETHREADEDDOMDOCUMENT60, GREETING_XSL)?;
    c.hr(call!(t, T_PUTREF_STYLESHEET, fn(Obj) -> HResult, ss), S_OK)?;
    let (hr, n) = get_obj(t, T_GET_STYLESHEET);
    c.hr(hr, S_OK)?;
    c.ok(n == ss, n as u64)?;
    release(n);
    c.hr(call!(t, T_CREATE_PROCESSOR, fn(*mut Obj) -> HResult, null_mut()), E_INVALIDARG)?;
    let (hr, p) = get_obj(t, T_CREATE_PROCESSOR);
    c.hr(hr, S_OK)?;
    let (hr, pq) = query(p, &IID_IXSLPROCESSOR);
    c.hr(hr, S_OK)?;
    release(pq);
    c.hr(call!(p, P_GET_OWNER_TEMPLATE, fn(*mut Obj) -> HResult, null_mut()), E_INVALIDARG)?;
    let (hr, owner) = get_obj(p, P_GET_OWNER_TEMPLATE);
    c.hr(hr, S_OK)?;
    c.ok(owner == t, owner as u64)?;
    release(owner);
    let mut v = Variant { vt: VT_BSTR, reserved: [0; 3], value: [0; 2] };
    c.hr(call!(p, P_GET_INPUT, fn(*mut Variant) -> HResult, &mut v), S_OK)?;
    c.eq(u64::from(v.vt), u64::from(VT_EMPTY))?;
    c.hr(call!(p, P_GET_OUTPUT, fn(*mut Variant) -> HResult, null_mut()), E_INVALIDARG)?;
    c.hr(call!(p, P_PUT_OUTPUT, fn(Variant) -> HResult, Variant::empty()), S_OK)?;
    c.hr(call!(p, P_PUT_OUTPUT, fn(Variant) -> HResult, unknown(null_mut())), S_OK)?;
    c.hr(call!(p, P_TRANSFORM, fn(*mut i16) -> HResult, null_mut()), E_INVALIDARG)?;
    // No output object: an empty string (the html stylesheet selects
    // nothing in this input).
    let input = loaded(c, &CLSID_DOMDOCUMENT60, "<a>test</a>")?;
    c.hr(call!(p, P_PUT_INPUT, fn(Variant) -> HResult, unknown(input)), S_OK)?;
    let mut done: i16 = 0x55;
    c.hr(call!(p, P_TRANSFORM, fn(*mut i16) -> HResult, &mut done), S_OK)?;
    let mut out = Variant::empty();
    c.hr(call!(p, P_GET_OUTPUT, fn(*mut Variant) -> HResult, &mut out), S_OK)?;
    c.var_text(&mut out, "")?;
    release(input);
    // A document as the output.
    let input = loaded(c, &CLSID_DOMDOCUMENT60, GREETING)?;
    c.hr(call!(p, P_PUT_INPUT, fn(Variant) -> HResult, unknown(input)), S_OK)?;
    let target = new_doc(c, &CLSID_DOMDOCUMENT60)?;
    c.hr(call!(p, P_PUT_OUTPUT, fn(Variant) -> HResult, unknown(target)), S_OK)?;
    get_bstr(c, target, GET_XML, "")?;
    let mut done: i16 = 0x55;
    c.hr(call!(p, P_TRANSFORM, fn(*mut i16) -> HResult, &mut done), S_OK)?;
    c.eq(done as u16 as u64, VARIANT_TRUE as u16 as u64)?;
    let mut out = Variant::empty();
    c.hr(call!(p, P_GET_OUTPUT, fn(*mut Variant) -> HResult, &mut out), S_OK)?;
    c.eq(u64::from(out.vt), u64::from(VT_UNKNOWN))?;
    out.clear();
    let (hr, el) = get_obj(target, GET_DOCUMENT_ELEMENT);
    c.hr(hr, S_OK)?;
    get_bstr(c, el, GET_NODE_NAME, "html")?;
    release(el);
    release(target);
    release(input);
    release(p);
    c.hr(call!(t, T_PUTREF_STYLESHEET, fn(Obj) -> HResult, null_mut()), S_OK)?;
    release(ss);
    release(t);
    Ok(())
}

/// A processor of `xsl` (free-threaded) over `input` with string
/// params: the transform's string output, loaded into a new document.
fn run_processor(
    c: &mut Checks,
    xsl: &str,
    input: Obj,
    params: &[(&str, &str)],
) -> Result<Obj, u32> {
    let mut t: Obj = null_mut();
    // SAFETY: valid GUIDs and output.
    let hr = unsafe {
        CoCreateInstance(
            &CLSID_XSLTEMPLATE60,
            null_mut(),
            CLSCTX_INPROC_SERVER,
            &IID_IXSLTEMPLATE,
            &mut t,
        )
    };
    c.hr(hr, S_OK)?;
    let ss = loaded(c, &CLSID_FREETHREADEDDOMDOCUMENT60, xsl)?;
    c.hr(call!(t, T_PUTREF_STYLESHEET, fn(Obj) -> HResult, ss), S_OK)?;
    let (hr, p) = get_obj(t, T_CREATE_PROCESSOR);
    c.hr(hr, S_OK)?;
    c.hr(call!(p, P_PUT_INPUT, fn(Variant) -> HResult, unknown(input)), S_OK)?;
    for (name, value) in params {
        let (n, v) = (Wide::new(name), Wide::new(value));
        let hr = call!(
            p,
            P_ADD_PARAMETER,
            fn(Bstr, Variant, Bstr) -> HResult,
            n.0,
            Variant::bstr(&v),
            null_mut()
        );
        c.hr(hr, S_OK)?;
    }
    let mut done: i16 = 0x55;
    c.hr(call!(p, P_TRANSFORM, fn(*mut i16) -> HResult, &mut done), S_OK)?;
    c.eq(done as u16 as u64, VARIANT_TRUE as u16 as u64)?;
    let mut out = Variant::empty();
    c.hr(call!(p, P_GET_OUTPUT, fn(*mut Variant) -> HResult, &mut out), S_OK)?;
    c.eq(u64::from(out.vt), u64::from(VT_BSTR))?;
    let text = out.value[0] as Bstr;
    let result = new_doc(c, &CLSID_DOMDOCUMENT60)?;
    let mut ok: i16 = 0x55;
    let hr = call!(result, LOAD_XML, fn(Bstr, *mut i16) -> HResult, text, &mut ok);
    c.hr(hr, S_OK)?;
    c.eq(ok as u16 as u64, VARIANT_TRUE as u16 as u64)?;
    out.clear();
    release(p);
    release(ss);
    release(t);
    set_namespaces(c, result)?;
    Ok(result)
}

/// App-V's shape: the normalization with its params, then the alias
/// step on its result; each result's tree checked, its text an `info`
/// line.
fn app_v(c: &mut Checks) -> Result<(), u32> {
    let input = loaded(c, &CLSID_DOMDOCUMENT60, MANIFEST)?;
    let params = [("known", "[urn:gala:old]"), ("to", NEW_NS)];
    let norm = run_processor(c, NORMALIZE_XSL, input, &params)?;
    let (hr, root) = get_obj(norm, GET_DOCUMENT_ELEMENT);
    c.hr(hr, S_OK)?;
    get_bstr(c, root, GET_BASE_NAME, "Package")?;
    get_bstr(c, root, GET_NAMESPACE_URI, PKG_NS)?;
    // The root's attributes but Ignore stay (the higher priority
    // template); elsewhere an ignorable namespace's attributes go.
    attribute(c, root, "Keep", "1")?;
    attribute(c, root, "x:drop", "d")?;
    let name = Wide::new("Ignore");
    let mut v = Variant::empty();
    let hr = call!(root, GET_ATTRIBUTE, fn(Bstr, *mut Variant) -> HResult, name.0, &mut v);
    c.hr(hr, S_FALSE)?;
    let xml = xml_of(c, root)?;
    info_long("normalized", xml);
    release(root);
    absent(c, norm, "/p:Package/p:Id/@x:z")?;
    node_is(c, norm, "/p:Package/p:Id/@Name", "", Some("n"))?;
    // An old attribute on an element that stays: into the new namespace.
    node_is(c, norm, "/p:Package/p:Id/@n:Ver", NEW_NS, Some("2"))?;
    // An old element: into the new namespace, its old attributes in no
    // namespace.
    node_is(c, norm, "/p:Package/n:Ext", NEW_NS, None)?;
    node_is(c, norm, "/p:Package/n:Ext/@Kind", "", Some("k"))?;
    node_is(c, norm, "/p:Package/n:Ext/@Plain", "", Some("v"))?;
    node_is(c, norm, "/p:Package/n:Ext/n:Item", NEW_NS, Some("one"))?;
    // Ignorable elements go, with what is in them.
    absent(c, norm, "//x:Skip")?;
    absent(c, norm, "//x:Gone")?;
    absent(c, norm, "//p:Inner")?;
    // The comment goes (no template copies it); text stays.
    absent(c, norm, "//comment()")?;
    node_is(c, norm, "/p:Package/p:Text", PKG_NS, Some("a & b"))?;
    let (hr, list) = select(norm, "/p:Package/*");
    c.hr(hr, S_OK)?;
    c.eq(list_length(list), 3)?;
    release(list);

    let alias = run_processor(c, ALIAS_XSL, norm, &[])?;
    let (hr, root) = get_obj(alias, GET_DOCUMENT_ELEMENT);
    c.hr(hr, S_OK)?;
    get_bstr(c, root, GET_BASE_NAME, "Package")?;
    get_bstr(c, root, GET_NAMESPACE_URI, PKG_NS)?;
    // The namespace node copied from the fragment: a declaration of n.
    attribute(c, root, "xmlns:n", NEW_NS)?;
    attribute(c, root, "Keep", "n")?;
    let xml = xml_of(c, root)?;
    info_long("aliased", xml);
    release(root);
    node_is(c, alias, "/p:Package/n:Ext/n:Item", NEW_NS, Some("one"))?;
    node_is(c, alias, "/p:Package/p:Id/@n:Ver", NEW_NS, Some("2"))?;
    // Which prefix the new namespace's elements get (App-V counts on
    // the one declared on the root).
    let (hr, ext) = select_single(alias, "/p:Package/n:Ext");
    c.hr(hr, S_OK)?;
    let mut s: Bstr = null_mut();
    let hr = call!(ext, GET_NODE_NAME, fn(*mut Bstr) -> HResult, &mut s);
    info_hr("aliased Ext nodeName hr", hr);
    info_long("aliased Ext nodeName", s);
    release(ext);
    release(alias);
    release(norm);
    release(input);
    Ok(())
}
