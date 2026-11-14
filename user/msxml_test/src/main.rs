//! MSXML 6's DOM (msxml6.dll's DOMDocument60), the part the Office
//! Deployment Tool's setup.exe uses (ROADMAP OF), as Windows answers it.
//! Expected values are those of Wine's conformance tests
//! (dlls/msxml3/tests/domdoc.c, run against Windows, where MSXML 6 is
//! part of the system) and of real Windows on CI's runner (this program's
//! run there):
//! - CoCreateInstance(CLSID_DOMDocument60) (and
//!   CLSID_FreeThreadedDOMDocument60) gives IXMLDOMDocument2, which
//!   answers QueryInterface for IXMLDOMDocument3, IXMLDOMDocument,
//!   IXMLDOMNode, IDispatch and IUnknown; on a thread that never
//!   initialized COM it is CO_E_NOTINITIALIZED while the process has no
//!   MTA and succeeds (the implicit MTA) while another thread holds it;
//! - the defaults: async VARIANT_TRUE, validateOnParse VARIANT_TRUE,
//!   preserveWhiteSpace and resolveExternals VARIANT_FALSE; the
//!   properties SelectionLanguage "XPath", SelectionNamespaces "",
//!   ProhibitDTD VARIANT_TRUE (MSXML 6); an unknown property is E_FAIL to
//!   get and to set, a NULL output E_INVALIDARG, a SelectionLanguage that
//!   is not a string E_FAIL;
//! - an empty document has no documentElement (S_FALSE, NULL);
//! - loadXML of a well-formed document: S_OK and VARIANT_TRUE,
//!   readyState 4, parseError's errorCode 0; white space between elements
//!   is not kept (preserveWhiteSpace off); the XML declaration is the
//!   document's first child, a processing instruction named "xml";
//! - elements: nodeName/tagName, nodeType 1, baseName, prefix and
//!   namespaceURI (S_FALSE and NULL without a prefix or namespace),
//!   parentNode (the document's is S_FALSE and NULL), first/last child
//!   and siblings, text (references replaced), xml of an empty element
//!   (`<Display Level="None"/>`), getAttribute (S_FALSE and VT_NULL when
//!   absent), attributes (a named node map: length, getNamedItem with its
//!   nodeValue, get_item);
//! - selectSingleNode and selectNodes in XPath: child and descendant
//!   paths from the document and from an element, `@attribute` steps,
//!   predicates by position and by attribute value, contains(), `*`;
//!   prefixes from the SelectionNamespaces property; nothing found is
//!   S_FALSE and NULL (selectSingleNode) or an empty list; an undeclared
//!   prefix, a syntax error and an expression that is no node-set are
//!   E_FAIL; not(), position(), last(), count(), starts-with(),
//!   local-name(), `and`, a filter expression, a union, the
//!   following-sibling axis; node lists: length, get_item (S_FALSE and
//!   NULL past the end), nextNode to the end and reset; what setup.exe
//!   does with its configuration: selectNodes("@ID") from an element and
//!   each attribute's nodeValue;
//! - nodeValue (S_FALSE and VT_NULL for an element, the text of text and
//!   attributes), nodeTypeString, hasChildNodes (S_FALSE and
//!   VARIANT_FALSE without children), ownerDocument, a text node
//!   ("#text", its data and length), getAttributeNode and the
//!   attribute's value (an attribute has no parentNode), the attribute
//!   map's getQualifiedItem and nextNode;
//! - with preserveWhiteSpace on, white space between elements is kept as
//!   text nodes;
//! - a document that is not well formed (an end tag that does not match,
//!   an empty string, a DTD, prohibited in MSXML 6): S_FALSE and
//!   VARIANT_FALSE, parseError's errorCode an error (negative) with the
//!   line, and an empty document after; with no error, errorCode is
//!   S_FALSE and 0;
//! - load of a file (with async off): S_OK and VARIANT_TRUE and the same
//!   tree; a file that does not exist is S_FALSE and VARIANT_FALSE;
//! - cloneNode (`clone`: Wine's test_cloneNode and MSXML's documentation;
//!   setup.exe clones from its version descriptor);
//! - the xml of an element whose prefixes or default namespace are
//!   declared on its ancestors loads again, its names in their
//!   namespaces (`inherited_namespaces`: Wine's namespace tests; the
//!   exact texts are `info` lines);
//! - changing the tree (`mutation`: removeChild, replaceChild,
//!   insertBefore, appendChild with fragments and nodes of another
//!   document, the create methods, setAttribute, setAttributeNode,
//!   removeAttribute(Node), the attribute map's changes, put_text,
//!   put_nodeValue, normalize, getElementsByTagName, importNode, save;
//!   Click-to-Run's App-V orchestration changes the AppX manifests it
//!   merges);
//! - XSD schemas and validation (`schema`: XMLSchemaCache60, the
//!   document's `schemas`, validate, validateNode, validateOnParse, a
//!   schema from a `res:` URL; Click-to-Run's App-V pieces configure
//!   Office's package with them);
//! - XSLT (`xslt`: XSLTemplate60 and its processor, transformNode,
//!   transformNodeToObject into a document and streams, and transforms
//!   shaped like the ones App-V runs on a package manifest before it
//!   validates it);
//! - typed values (`typed`: nodeTypedValue and dataType of elements
//!   typed by `dt:dt`, the values Click-to-Run's App-V pieces read from
//!   a package's deployment configuration).
//!
//! The same source builds `wow64_msxml_test`, the 32-bit (x86) program.
//! Prints `msxml_test: ok` (`wow64_msxml_test: ok`), or `<name>: check N
//! got X (line L)` and exits with N.

#![no_std]
#![no_main]

use core::ffi::c_void;
use core::ptr::null_mut;

use ntrt as _;

#[cfg(target_arch = "x86_64")]
const NAME: &str = "msxml_test";
#[cfg(target_arch = "x86")]
const NAME: &str = "wow64_msxml_test";

type Handle = usize;
type HResult = i32;
type Guid = [u8; 16];
type Bstr = *mut u16;
type Obj = *mut c_void;

#[cfg_attr(
    target_arch = "x86",
    link(name = "kernel32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "kernel32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, overlapped: usize) -> i32;
    fn ExitProcess(code: u32) -> !;
    fn GetTempPathW(size: u32, buf: *mut u16) -> u32;
    fn CreateFileW(
        path: *const u16,
        access: u32,
        share: u32,
        security: usize,
        disposition: u32,
        flags: u32,
        template: usize,
    ) -> Handle;
    fn CloseHandle(h: Handle) -> i32;
    fn ReadFile(h: Handle, buf: *mut u8, len: u32, read: *mut u32, overlapped: usize) -> i32;
    fn DeleteFileW(path: *const u16) -> i32;
    fn CreateThread(
        attributes: *mut c_void,
        stack: usize,
        start: extern "system" fn(*mut c_void) -> u32,
        parameter: *mut c_void,
        flags: u32,
        id: *mut u32,
    ) -> Handle;
    fn WaitForSingleObject(h: Handle, ms: u32) -> u32;
    fn GetProcessHeaps(count: u32, heaps: *mut Handle) -> u32;
    fn HeapValidate(heap: Handle, flags: u32, block: *const c_void) -> i32;
}

/// Whether every heap of the process is valid (HeapValidate on each), so
/// a corruption is placed by the first section that ends with one.
fn heaps_valid() -> bool {
    let mut heaps = [0 as Handle; 64];
    // SAFETY: a buffer of 64 handles.
    let n = unsafe { GetProcessHeaps(heaps.len() as u32, heaps.as_mut_ptr()) } as usize;
    heaps[..n.min(heaps.len())]
        .iter()
        // SAFETY: a heap handle GetProcessHeaps returned; NULL block checks it all.
        .all(|&h| unsafe { HeapValidate(h, 0, core::ptr::null()) } != 0)
}

#[cfg_attr(
    target_arch = "x86",
    link(name = "ole32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "ole32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn CoInitializeEx(reserved: *mut c_void, flags: u32) -> HResult;
    fn CoCreateInstance(
        clsid: *const Guid,
        outer: Obj,
        context: u32,
        iid: *const Guid,
        out: *mut Obj,
    ) -> HResult;
    fn CoUninitialize();
}

#[cfg_attr(
    target_arch = "x86",
    link(name = "oleaut32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "oleaut32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn SysAllocString(s: *const u16) -> Bstr;
    fn SysFreeString(s: Bstr);
    fn VariantClear(v: *mut Variant) -> HResult;
}

const S_OK: HResult = 0;
const S_FALSE: HResult = 1;
const E_FAIL: HResult = 0x8000_4005u32 as i32;
const E_INVALIDARG: HResult = 0x8007_0057u32 as i32;
const VARIANT_TRUE: i16 = -1;
const VARIANT_FALSE: i16 = 0;
const VT_EMPTY: u16 = 0;
const VT_NULL: u16 = 1;
const VT_I4: u16 = 3;
const VT_BSTR: u16 = 8;
const VT_BOOL: u16 = 11;
const COINIT_APARTMENTTHREADED: u32 = 2;
const COINIT_MULTITHREADED: u32 = 0;
const CO_E_NOTINITIALIZED: HResult = 0x8004_01f0u32 as i32;
const CLSCTX_INPROC_SERVER: u32 = 1;

const fn guid(d1: u32, d2: u16, d3: u16, d4: [u8; 8]) -> Guid {
    let a = d1.to_le_bytes();
    let b = d2.to_le_bytes();
    let c = d3.to_le_bytes();
    [
        a[0], a[1], a[2], a[3], b[0], b[1], c[0], c[1], d4[0], d4[1], d4[2], d4[3], d4[4], d4[5],
        d4[6], d4[7],
    ]
}

const DOM_D4: [u8; 8] = [0xb2, 0x0e, 0x00, 0xc0, 0x4f, 0x98, 0x3e, 0x60];
const CLSID_DOMDOCUMENT60: Guid =
    guid(0x88d9_6a05, 0xf192, 0x11d4, [0xa6, 0x5f, 0x00, 0x40, 0x96, 0x32, 0x51, 0xe5]);
const CLSID_FREETHREADEDDOMDOCUMENT60: Guid =
    guid(0x88d9_6a06, 0xf192, 0x11d4, [0xa6, 0x5f, 0x00, 0x40, 0x96, 0x32, 0x51, 0xe5]);
const IID_IUNKNOWN: Guid = guid(0, 0, 0, [0xc0, 0, 0, 0, 0, 0, 0, 0x46]);
const IID_IDISPATCH: Guid = guid(0x0002_0400, 0, 0, [0xc0, 0, 0, 0, 0, 0, 0, 0x46]);
const IID_IXMLDOMNODE: Guid = guid(0x2933_bf80, 0x7b36, 0x11d2, DOM_D4);
const IID_IXMLDOMDOCUMENT: Guid = guid(0x2933_bf81, 0x7b36, 0x11d2, DOM_D4);
const IID_IXMLDOMDOCUMENT2: Guid = guid(0x2933_bf95, 0x7b36, 0x11d2, DOM_D4);
const IID_IXMLDOMDOCUMENT3: Guid = guid(0x2933_bf96, 0x7b36, 0x11d2, DOM_D4);
const IID_IXMLDOMELEMENT: Guid = guid(0x2933_bf86, 0x7b36, 0x11d2, DOM_D4);
const IID_IXMLDOMATTRIBUTE: Guid = guid(0x2933_bf85, 0x7b36, 0x11d2, DOM_D4);
const IID_IXMLDOMTEXT: Guid = guid(0x2933_bf87, 0x7b36, 0x11d2, DOM_D4);
const IID_IXMLDOMCOMMENT: Guid = guid(0x2933_bf88, 0x7b36, 0x11d2, DOM_D4);

/// A VARIANT (24 bytes on x64, 16 on x86).
#[repr(C)]
struct Variant {
    vt: u16,
    reserved: [u16; 3],
    value: [usize; 2],
}

impl Variant {
    fn empty() -> Self {
        Variant { vt: VT_EMPTY, reserved: [0; 3], value: [0; 2] }
    }
    fn bstr(s: &Wide) -> Self {
        Variant { vt: VT_BSTR, reserved: [0; 3], value: [s.0 as usize, 0] }
    }
    fn i4(v: i32) -> Self {
        Variant { vt: VT_I4, reserved: [0; 3], value: [v as u32 as usize, 0] }
    }
    fn clear(&mut self) {
        // SAFETY: a VARIANT this program owns.
        unsafe { VariantClear(self) };
    }
}

// The DOM interfaces' vtable slots (msxml6.h's order).
const QUERY_INTERFACE: usize = 0;
const RELEASE: usize = 2;
const GET_NODE_NAME: usize = 7;
const GET_NODE_VALUE: usize = 8;
const GET_NODE_TYPE: usize = 10;
const GET_PARENT_NODE: usize = 11;
const GET_CHILD_NODES: usize = 12;
const GET_FIRST_CHILD: usize = 13;
const GET_LAST_CHILD: usize = 14;
const GET_PREVIOUS_SIBLING: usize = 15;
const GET_NEXT_SIBLING: usize = 16;
const GET_ATTRIBUTES: usize = 17;
const HAS_CHILD_NODES: usize = 22;
const GET_OWNER_DOCUMENT: usize = 23;
const CLONE_NODE: usize = 24;
const GET_NODE_TYPE_STRING: usize = 25;
const GET_TEXT: usize = 26;
const GET_XML: usize = 34;
const SELECT_NODES: usize = 36;
const SELECT_SINGLE_NODE: usize = 37;
const GET_NAMESPACE_URI: usize = 39;
const GET_PREFIX: usize = 40;
const GET_BASE_NAME: usize = 41;
// IXMLDOMDocument
const GET_DOCUMENT_ELEMENT: usize = 45;
const LOAD: usize = 58;
const GET_READY_STATE: usize = 59;
const GET_PARSE_ERROR: usize = 60;
const GET_ASYNC: usize = 62;
const PUT_ASYNC: usize = 63;
const LOAD_XML: usize = 65;
const GET_VALIDATE_ON_PARSE: usize = 67;
const GET_RESOLVE_EXTERNALS: usize = 69;
const GET_PRESERVE_WHITE_SPACE: usize = 71;
const PUT_PRESERVE_WHITE_SPACE: usize = 72;
// IXMLDOMDocument2
const SET_PROPERTY: usize = 80;
const GET_PROPERTY: usize = 81;
// IXMLDOMElement
const GET_TAG_NAME: usize = 43;
const GET_ATTRIBUTE: usize = 44;
const GET_ATTRIBUTE_NODE: usize = 47;
// IXMLDOMAttribute
const ATTR_GET_VALUE: usize = 44;
// IXMLDOMText (IXMLDOMCharacterData)
const TEXT_GET_DATA: usize = 43;
const TEXT_GET_LENGTH: usize = 45;
// IXMLDOMNodeList
const LIST_GET_ITEM: usize = 7;
const LIST_GET_LENGTH: usize = 8;
const LIST_NEXT_NODE: usize = 9;
const LIST_RESET: usize = 10;
// IXMLDOMNamedNodeMap
const MAP_GET_NAMED_ITEM: usize = 7;
const MAP_GET_ITEM: usize = 10;
const MAP_GET_LENGTH: usize = 11;
const MAP_GET_QUALIFIED_ITEM: usize = 12;
const MAP_NEXT_NODE: usize = 14;
// IXMLDOMParseError
const PE_GET_ERROR_CODE: usize = 7;
const PE_GET_LINE: usize = 11;

/// Calls slot `slot` of `obj`'s vtable as `$ty`.
macro_rules! call {
    ($obj:expr, $slot:expr, fn($($t:ty),*) $(-> $r:ty)?, $($arg:expr),*) => {{
        let obj: Obj = $obj;
        // SAFETY: a live COM object whose vtable has this slot with this
        // signature (msxml6.h).
        unsafe {
            let vtable = *(obj as *const *const usize);
            let f: extern "system" fn(Obj $(, $t)*) $(-> $r)? =
                core::mem::transmute(*vtable.add($slot));
            f(obj $(, $arg)*)
        }
    }};
}

mod mutation;
mod schema;
mod typed;
mod xslt;

fn release(obj: Obj) {
    if !obj.is_null() {
        call!(obj, RELEASE, fn() -> u32,);
    }
}

fn query(obj: Obj, iid: &Guid) -> (HResult, Obj) {
    let mut out = null_mut();
    let hr = call!(obj, QUERY_INTERFACE, fn(*const Guid, *mut Obj) -> HResult, iid, &mut out);
    (hr, out)
}

struct Line {
    buf: [u8; 200],
    n: usize,
}

impl Line {
    fn new() -> Self {
        Self { buf: [0; 200], n: 0 }
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
        let mut d = [0u8; 18];
        d[0] = b'0';
        d[1] = b'x';
        for i in 0..16 {
            d[2 + i] = b"0123456789abcdef"[((v >> (60 - 4 * i)) & 0xf) as usize];
        }
        self.s(&d)
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

/// The check counter, and the first check that `code` found wrong (0
/// when none): an error-code check records its mismatch and goes on, so
/// one run on Windows reports every code that differs.
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
            .s(b")\r\n")
            .print();
        Err(self.0)
    }

    #[track_caller]
    fn hr(&mut self, got: HResult, want: HResult) -> Result<(), u32> {
        self.ok(got == want, got as u32 as u64)
    }

    /// An error code whose mismatch does not stop the test: it is
    /// printed with the code expected, and the test fails at its end
    /// with the first such check's number.
    #[track_caller]
    fn code(&mut self, got: HResult, want: HResult) -> Result<(), u32> {
        self.later(got == want, got as u32 as u64, Some(want as u32 as u64))
    }

    /// A condition whose failure does not stop the test, as `code`.
    #[track_caller]
    fn note(&mut self, cond: bool, got: u64) -> Result<(), u32> {
        self.later(cond, got, None)
    }

    #[track_caller]
    fn later(&mut self, cond: bool, got: u64, want: Option<u64>) -> Result<(), u32> {
        self.0 += 1;
        if !cond {
            let mut line = Line::new()
                .s(NAME.as_bytes())
                .s(b": check ")
                .dec(u64::from(self.0))
                .s(b" got ")
                .hex(got);
            if let Some(w) = want {
                line = line.s(b" want ").hex(w);
            }
            line.s(b" (line ")
                .dec(u64::from(core::panic::Location::caller().line()))
                .s(b", continuing)\r\n")
                .print();
            if self.1 == 0 {
                self.1 = self.0;
            }
        }
        Ok(())
    }

    #[track_caller]
    fn eq(&mut self, got: u64, want: u64) -> Result<(), u32> {
        self.ok(got == want, got)
    }

    /// A BSTR that should hold `want` (it is freed).
    #[track_caller]
    fn text(&mut self, got: Bstr, want: &str) -> Result<(), u32> {
        let same = bstr_is(got, want);
        let first = if got.is_null() {
            0xdead
        } else {
            // SAFETY: a BSTR, NUL-terminated.
            u64::from(unsafe { *got })
        };
        // SAFETY: a BSTR from the DOM (or NULL).
        unsafe { SysFreeString(got) };
        self.ok(same, first)
    }

    /// A VARIANT that should be the string `want` (it is cleared).
    #[track_caller]
    fn var_text(&mut self, v: &mut Variant, want: &str) -> Result<(), u32> {
        let same = v.vt == VT_BSTR && bstr_is(v.value[0] as Bstr, want);
        let vt = v.vt;
        v.clear();
        self.ok(same, u64::from(vt))
    }
}

/// Whether two NUL-terminated wide strings are equal (both freed).
fn bstr_eq(a: Bstr, b: Bstr) -> bool {
    let mut equal = !a.is_null() && !b.is_null();
    let mut i = 0;
    // SAFETY: two NUL-terminated strings, read up to the first NUL.
    unsafe {
        while equal {
            let (x, y) = (*a.add(i), *b.add(i));
            equal = x == y;
            if x == 0 || y == 0 {
                break;
            }
            i += 1;
        }
        SysFreeString(a);
        SysFreeString(b);
    }
    equal
}

/// Whether `s` (a NUL-terminated wide string) is `want`.
fn bstr_is(s: Bstr, want: &str) -> bool {
    if s.is_null() {
        return false;
    }
    let mut i = 0;
    for u in want.encode_utf16() {
        // SAFETY: within the string (it ends at a NUL, which no unit of
        // `want` is).
        if unsafe { *s.add(i) } != u {
            return false;
        }
        i += 1;
    }
    // SAFETY: as above.
    unsafe { *s.add(i) == 0 }
}

/// A BSTR made from `text` (freed on drop).
struct Wide(Bstr);

impl Wide {
    fn new(text: &str) -> Self {
        let mut buf = [0u16; 4096];
        for (slot, u) in buf.iter_mut().zip(text.encode_utf16()) {
            *slot = u;
        }
        // SAFETY: a NUL-terminated string.
        Wide(unsafe { SysAllocString(buf.as_ptr()) })
    }
}

impl Drop for Wide {
    fn drop(&mut self) {
        // SAFETY: our BSTR.
        unsafe { SysFreeString(self.0) };
    }
}

const DOC: &str = "<?xml version=\"1.0\"?>\r\n\
<Configuration xmlns:x=\"urn:x\">\r\n\
  <Add Channel=\"Current\" OfficeClientEdition=\"64\">\r\n\
    <Product ID=\"O365ProPlusRetail\">\r\n\
      <Language ID=\"en-us\" />\r\n\
      <ExcludeApp ID=\"Access\" />\r\n\
      <ExcludeApp ID=\"Groove\" />\r\n\
    </Product>\r\n\
  </Add>\r\n\
  <x:Item x:Kind=\"k\">text &amp; more</x:Item>\r\n\
  <Display Level=\"None\"/>\r\n\
</Configuration>\r\n";

fn get_bstr(c: &mut Checks, obj: Obj, slot: usize, want: &str) -> Result<(), u32> {
    let mut s: Bstr = null_mut();
    c.hr(call!(obj, slot, fn(*mut Bstr) -> HResult, &mut s), S_OK)?;
    c.text(s, want)
}

fn get_obj(obj: Obj, slot: usize) -> (HResult, Obj) {
    let mut out: Obj = null_mut();
    let hr = call!(obj, slot, fn(*mut Obj) -> HResult, &mut out);
    (hr, out)
}

fn get_bool(c: &mut Checks, obj: Obj, slot: usize, want: i16) -> Result<(), u32> {
    let mut b: i16 = 0x55;
    c.hr(call!(obj, slot, fn(*mut i16) -> HResult, &mut b), S_OK)?;
    c.eq(b as u16 as u64, want as u16 as u64)
}

fn node_type(obj: Obj) -> u32 {
    let mut t = 0u32;
    call!(obj, GET_NODE_TYPE, fn(*mut u32) -> HResult, &mut t);
    t
}

fn select_single(doc_or_node: Obj, expr: &str) -> (HResult, Obj) {
    let e = Wide::new(expr);
    let mut out: Obj = 0x1 as Obj;
    let hr = call!(doc_or_node, SELECT_SINGLE_NODE, fn(Bstr, *mut Obj) -> HResult, e.0, &mut out);
    (hr, out)
}

fn select(doc_or_node: Obj, expr: &str) -> (HResult, Obj) {
    let e = Wide::new(expr);
    let mut out: Obj = 0x1 as Obj;
    let hr = call!(doc_or_node, SELECT_NODES, fn(Bstr, *mut Obj) -> HResult, e.0, &mut out);
    (hr, out)
}

fn list_length(list: Obj) -> u64 {
    let mut n = -1i32;
    call!(list, LIST_GET_LENGTH, fn(*mut i32) -> HResult, &mut n);
    n as u32 as u64
}

/// The attribute `name` of element `el` should be `want`.
fn attribute(c: &mut Checks, el: Obj, name: &str, want: &str) -> Result<(), u32> {
    let n = Wide::new(name);
    let mut v = Variant::empty();
    let hr = call!(el, GET_ATTRIBUTE, fn(Bstr, *mut Variant) -> HResult, n.0, &mut v);
    c.hr(hr, S_OK)?;
    c.var_text(&mut v, want)
}

/// The node `expr` selects from `ctx` should be an element whose ID
/// attribute is `want`.
fn id_of(c: &mut Checks, ctx: Obj, expr: &str, want: &str) -> Result<(), u32> {
    let (hr, node) = select_single(ctx, expr);
    c.hr(hr, S_OK)?;
    let (hr, el) = query(node, &IID_IXMLDOMELEMENT);
    release(node);
    c.hr(hr, S_OK)?;
    let r = attribute(c, el, "ID", want);
    release(el);
    r
}

fn load_xml(doc: Obj, xml: &str) -> (HResult, i16) {
    let x = Wide::new(xml);
    let mut ok: i16 = 0x55;
    let hr = call!(doc, LOAD_XML, fn(Bstr, *mut i16) -> HResult, x.0, &mut ok);
    (hr, ok)
}

fn defaults(c: &mut Checks, doc: Obj) -> Result<(), u32> {
    for iid in [
        &IID_IXMLDOMDOCUMENT3,
        &IID_IXMLDOMDOCUMENT,
        &IID_IXMLDOMNODE,
        &IID_IDISPATCH,
        &IID_IUNKNOWN,
    ] {
        let (hr, other) = query(doc, iid);
        c.hr(hr, S_OK)?;
        c.ok(!other.is_null(), 0)?;
        release(other);
    }
    get_bool(c, doc, GET_ASYNC, VARIANT_TRUE)?;
    get_bool(c, doc, GET_VALIDATE_ON_PARSE, VARIANT_TRUE)?;
    get_bool(c, doc, GET_PRESERVE_WHITE_SPACE, VARIANT_FALSE)?;
    get_bool(c, doc, GET_RESOLVE_EXTERNALS, VARIANT_FALSE)?;
    let property = |c: &mut Checks, name: &str, want_vt: u16| -> Result<Variant, u32> {
        let n = Wide::new(name);
        let mut v = Variant::empty();
        c.hr(call!(doc, GET_PROPERTY, fn(Bstr, *mut Variant) -> HResult, n.0, &mut v), S_OK)?;
        c.eq(u64::from(v.vt), u64::from(want_vt))?;
        Ok(v)
    };
    let mut language = property(c, "SelectionLanguage", VT_BSTR)?;
    c.var_text(&mut language, "XPath")?;
    let mut namespaces = property(c, "SelectionNamespaces", VT_BSTR)?;
    c.var_text(&mut namespaces, "")?;
    let prohibit = property(c, "ProhibitDTD", VT_BOOL)?;
    c.eq(prohibit.value[0] as u16 as u64, VARIANT_TRUE as u16 as u64)?;
    // An unknown property, a NULL output, a language that is no string.
    let unknown = Wide::new("askldhfaklsdf");
    let mut v = Variant::i4(7);
    c.hr(call!(doc, GET_PROPERTY, fn(Bstr, *mut Variant) -> HResult, unknown.0, &mut v), E_FAIL)?;
    // Windows clears the output first (VT_EMPTY), the CI runner's answer.
    c.eq(u64::from(v.vt), 0)?;
    let language = Wide::new("SelectionLanguage");
    c.hr(
        call!(doc, GET_PROPERTY, fn(Bstr, *mut Variant) -> HResult, language.0, null_mut()),
        E_INVALIDARG,
    )?;
    c.hr(
        call!(doc, SET_PROPERTY, fn(Bstr, Variant) -> HResult, unknown.0, Variant::i4(1)),
        E_FAIL,
    )?;
    c.hr(
        call!(doc, SET_PROPERTY, fn(Bstr, Variant) -> HResult, language.0, Variant::i4(1)),
        E_FAIL,
    )?;
    let xpath = Wide::new("XPath");
    c.hr(
        call!(doc, SET_PROPERTY, fn(Bstr, Variant) -> HResult, language.0, Variant::bstr(&xpath)),
        S_OK,
    )?;
    // Nothing loaded: no document element.
    let (hr, root) = get_obj(doc, GET_DOCUMENT_ELEMENT);
    c.hr(hr, S_FALSE)?;
    c.ok(root.is_null(), root as u64)
}

fn tree(c: &mut Checks, doc: Obj) -> Result<(), u32> {
    let (hr, ok) = load_xml(doc, DOC);
    c.hr(hr, S_OK)?;
    c.eq(ok as u16 as u64, VARIANT_TRUE as u16 as u64)?;
    let mut state = 0i32;
    c.hr(call!(doc, GET_READY_STATE, fn(*mut i32) -> HResult, &mut state), S_OK)?;
    c.eq(state as u64, 4)?;
    let (hr, error) = get_obj(doc, GET_PARSE_ERROR);
    c.hr(hr, S_OK)?;
    let mut code = 1i32;
    c.hr(call!(error, PE_GET_ERROR_CODE, fn(*mut i32) -> HResult, &mut code), S_FALSE)?;
    release(error);
    c.eq(code as u32 as u64, 0)?;

    // The document: the XML declaration, then the root.
    c.eq(u64::from(node_type(doc)), 9)?;
    let (hr, kids) = get_obj(doc, GET_CHILD_NODES);
    c.hr(hr, S_OK)?;
    c.eq(list_length(kids), 2)?;
    release(kids);
    let (hr, decl) = get_obj(doc, GET_FIRST_CHILD);
    c.hr(hr, S_OK)?;
    c.eq(u64::from(node_type(decl)), 7)?;
    get_bstr(c, decl, GET_NODE_NAME, "xml")?;
    release(decl);

    let (hr, root) = get_obj(doc, GET_DOCUMENT_ELEMENT);
    c.hr(hr, S_OK)?;
    c.eq(u64::from(node_type(root)), 1)?;
    get_bstr(c, root, GET_NODE_NAME, "Configuration")?;
    get_bstr(c, root, GET_TAG_NAME, "Configuration")?;
    let mut none: Bstr = 0x1 as Bstr;
    c.hr(call!(root, GET_PREFIX, fn(*mut Bstr) -> HResult, &mut none), S_FALSE)?;
    c.ok(none.is_null(), none as u64)?;
    none = 0x1 as Bstr;
    c.hr(call!(root, GET_NAMESPACE_URI, fn(*mut Bstr) -> HResult, &mut none), S_FALSE)?;
    c.ok(none.is_null(), none as u64)?;
    let (hr, parent) = get_obj(root, GET_PARENT_NODE);
    c.hr(hr, S_OK)?;
    c.eq(u64::from(node_type(parent)), 9)?;
    let (hr, nothing) = get_obj(parent, GET_PARENT_NODE);
    c.hr(hr, S_FALSE)?;
    c.ok(nothing.is_null(), 0)?;
    release(parent);
    // White space between elements is gone: three children.
    let (hr, kids) = get_obj(root, GET_CHILD_NODES);
    c.hr(hr, S_OK)?;
    c.eq(list_length(kids), 3)?;
    let mut item: Obj = null_mut();
    c.hr(call!(kids, LIST_GET_ITEM, fn(i32, *mut Obj) -> HResult, 1, &mut item), S_OK)?;
    get_bstr(c, item, GET_NODE_NAME, "x:Item")?;
    get_bstr(c, item, GET_BASE_NAME, "Item")?;
    get_bstr(c, item, GET_PREFIX, "x")?;
    get_bstr(c, item, GET_NAMESPACE_URI, "urn:x")?;
    get_bstr(c, item, GET_TEXT, "text & more")?;
    get_bstr(c, item, GET_NODE_TYPE_STRING, "element")?;
    let mut v = Variant::i4(9);
    c.hr(call!(item, GET_NODE_VALUE, fn(*mut Variant) -> HResult, &mut v), S_FALSE)?;
    c.eq(u64::from(v.vt), u64::from(VT_NULL))?;
    get_bool(c, item, HAS_CHILD_NODES, VARIANT_TRUE)?;
    let (hr, owner) = get_obj(item, GET_OWNER_DOCUMENT);
    c.hr(hr, S_OK)?;
    c.eq(u64::from(node_type(owner)), 9)?;
    release(owner);
    // Its text node.
    let (hr, t) = get_obj(item, GET_FIRST_CHILD);
    c.hr(hr, S_OK)?;
    c.eq(u64::from(node_type(t)), 3)?;
    get_bstr(c, t, GET_NODE_NAME, "#text")?;
    let (hr, text) = query(t, &IID_IXMLDOMTEXT);
    release(t);
    c.hr(hr, S_OK)?;
    get_bstr(c, text, TEXT_GET_DATA, "text & more")?;
    let mut n = 0i32;
    c.hr(call!(text, TEXT_GET_LENGTH, fn(*mut i32) -> HResult, &mut n), S_OK)?;
    c.eq(n as u64, 11)?;
    let mut v = Variant::empty();
    c.hr(call!(text, GET_NODE_VALUE, fn(*mut Variant) -> HResult, &mut v), S_OK)?;
    c.var_text(&mut v, "text & more")?;
    release(text);
    // Its attribute, by qualified name and by namespace.
    let (hr, map) = get_obj(item, GET_ATTRIBUTES);
    c.hr(hr, S_OK)?;
    let (local, uri) = (Wide::new("Kind"), Wide::new("urn:x"));
    let mut attr: Obj = null_mut();
    let hr = call!(
        map,
        MAP_GET_QUALIFIED_ITEM,
        fn(Bstr, Bstr, *mut Obj) -> HResult,
        local.0,
        uri.0,
        &mut attr
    );
    c.hr(hr, S_OK)?;
    get_bstr(c, attr, GET_TEXT, "k")?;
    release(attr);
    release(map);
    let (hr, prev) = get_obj(item, GET_PREVIOUS_SIBLING);
    c.hr(hr, S_OK)?;
    get_bstr(c, prev, GET_NODE_NAME, "Add")?;
    release(prev);
    let (hr, next) = get_obj(item, GET_NEXT_SIBLING);
    c.hr(hr, S_OK)?;
    get_bstr(c, next, GET_NODE_NAME, "Display")?;
    get_bstr(c, next, GET_XML, "<Display Level=\"None\"/>")?;
    let (hr, last) = get_obj(next, GET_NEXT_SIBLING);
    c.hr(hr, S_FALSE)?;
    c.ok(last.is_null(), 0)?;
    release(next);
    let (hr, el) = query(item, &IID_IXMLDOMELEMENT);
    c.hr(hr, S_OK)?;
    attribute(c, el, "x:Kind", "k")?;
    let kind = Wide::new("x:Kind");
    let mut attr: Obj = null_mut();
    c.hr(call!(el, GET_ATTRIBUTE_NODE, fn(Bstr, *mut Obj) -> HResult, kind.0, &mut attr), S_OK)?;
    let (hr, a) = query(attr, &IID_IXMLDOMATTRIBUTE);
    release(attr);
    c.hr(hr, S_OK)?;
    let mut v = Variant::empty();
    c.hr(call!(a, ATTR_GET_VALUE, fn(*mut Variant) -> HResult, &mut v), S_OK)?;
    c.var_text(&mut v, "k")?;
    get_bstr(c, a, GET_NODE_TYPE_STRING, "attribute")?;
    // An attribute has no parent node.
    let (hr, none) = get_obj(a, GET_PARENT_NODE);
    c.hr(hr, S_FALSE)?;
    c.ok(none.is_null(), 0)?;
    release(a);
    release(el);
    release(item);
    item = 0x1 as Obj;
    c.hr(call!(kids, LIST_GET_ITEM, fn(i32, *mut Obj) -> HResult, 3, &mut item), S_FALSE)?;
    c.ok(item.is_null(), 0)?;
    release(kids);
    let (hr, last) = get_obj(root, GET_LAST_CHILD);
    c.hr(hr, S_OK)?;
    get_bstr(c, last, GET_NODE_NAME, "Display")?;
    release(last);

    // Attributes: getAttribute, the named node map.
    let (hr, add) = get_obj(root, GET_FIRST_CHILD);
    c.hr(hr, S_OK)?;
    let (hr, add_el) = query(add, &IID_IXMLDOMELEMENT);
    c.hr(hr, S_OK)?;
    attribute(c, add_el, "OfficeClientEdition", "64")?;
    let missing = Wide::new("Missing");
    let mut v = Variant::i4(5);
    c.hr(
        call!(add_el, GET_ATTRIBUTE, fn(Bstr, *mut Variant) -> HResult, missing.0, &mut v),
        S_FALSE,
    )?;
    c.eq(u64::from(v.vt), u64::from(VT_NULL))?;
    release(add_el);
    let (hr, map) = get_obj(add, GET_ATTRIBUTES);
    c.hr(hr, S_OK)?;
    let mut n = 0i32;
    c.hr(call!(map, MAP_GET_LENGTH, fn(*mut i32) -> HResult, &mut n), S_OK)?;
    c.eq(n as u64, 2)?;
    let channel = Wide::new("Channel");
    let mut attr: Obj = null_mut();
    c.hr(
        call!(map, MAP_GET_NAMED_ITEM, fn(Bstr, *mut Obj) -> HResult, channel.0, &mut attr),
        S_OK,
    )?;
    c.eq(u64::from(node_type(attr)), 2)?;
    let mut v = Variant::empty();
    c.hr(call!(attr, GET_NODE_VALUE, fn(*mut Variant) -> HResult, &mut v), S_OK)?;
    c.var_text(&mut v, "Current")?;
    release(attr);
    c.hr(call!(map, MAP_GET_ITEM, fn(i32, *mut Obj) -> HResult, 1, &mut attr), S_OK)?;
    get_bstr(c, attr, GET_NODE_NAME, "OfficeClientEdition")?;
    release(attr);
    for want in ["Channel", "OfficeClientEdition"] {
        c.hr(call!(map, MAP_NEXT_NODE, fn(*mut Obj) -> HResult, &mut attr), S_OK)?;
        get_bstr(c, attr, GET_NODE_NAME, want)?;
        release(attr);
    }
    attr = 0x1 as Obj;
    c.hr(call!(map, MAP_NEXT_NODE, fn(*mut Obj) -> HResult, &mut attr), S_FALSE)?;
    c.ok(attr.is_null(), 0)?;
    release(map);
    // An empty element has no children.
    let (hr, language) = select_single(add, "Product/Language");
    c.hr(hr, S_OK)?;
    let mut b: i16 = 0x55;
    c.hr(call!(language, HAS_CHILD_NODES, fn(*mut i16) -> HResult, &mut b), S_FALSE)?;
    c.eq(b as u16 as u64, VARIANT_FALSE as u16 as u64)?;
    release(language);
    release(add);
    release(root);
    Ok(())
}

fn xpath(c: &mut Checks, doc: Obj) -> Result<(), u32> {
    id_of(c, doc, "Configuration/Add/Product", "O365ProPlusRetail")?;
    id_of(c, doc, "/Configuration/Add/Product/Language", "en-us")?;
    id_of(c, doc, "//ExcludeApp[2]", "Groove")?;
    id_of(c, doc, "//ExcludeApp[@ID='Groove']", "Groove")?;
    id_of(c, doc, "//Product[ExcludeApp/@ID=\"Access\"]", "O365ProPlusRetail")?;
    id_of(c, doc, "//ExcludeApp[contains(@ID, 'cce')]", "Access")?;
    id_of(c, doc, "//ExcludeApp[not(@ID='Access')]", "Groove")?;
    id_of(c, doc, "//ExcludeApp[position()=last()]", "Groove")?;
    id_of(c, doc, "(//ExcludeApp)[1]", "Access")?;
    id_of(
        c,
        doc,
        "//Product[count(ExcludeApp)=2 and starts-with(@ID, 'O365')]",
        "O365ProPlusRetail",
    )?;
    id_of(c, doc, "//Language/following-sibling::ExcludeApp", "Access")?;
    let (hr, list) = select(doc, "//Language | //Display");
    c.hr(hr, S_OK)?;
    c.eq(list_length(list), 2)?;
    release(list);
    let (hr, item) = select_single(doc, "//*[local-name()='Item']");
    c.hr(hr, S_OK)?;
    get_bstr(c, item, GET_NODE_NAME, "x:Item")?;
    release(item);
    // An expression that is not a node-set.
    let (hr, _) = select(doc, "count(//ExcludeApp)");
    c.hr(hr, E_FAIL)?;
    // As setup.exe reads its configuration: a product's attributes by
    // a relative "@ID", each one's nodeValue.
    let (hr, product) = select_single(doc, "Configuration/Add/Product");
    c.hr(hr, S_OK)?;
    let (hr, ids) = select(product, "@ID");
    c.hr(hr, S_OK)?;
    c.eq(list_length(ids), 1)?;
    let mut id: Obj = null_mut();
    c.hr(call!(ids, LIST_GET_ITEM, fn(i32, *mut Obj) -> HResult, 0, &mut id), S_OK)?;
    let mut v = Variant::empty();
    c.hr(call!(id, GET_NODE_VALUE, fn(*mut Variant) -> HResult, &mut v), S_OK)?;
    c.var_text(&mut v, "O365ProPlusRetail")?;
    release(id);
    release(ids);
    let (hr, langs) = select(product, "Language");
    c.hr(hr, S_OK)?;
    c.eq(list_length(langs), 1)?;
    release(langs);
    release(product);
    // From an element, relative paths.
    let (hr, add) = select_single(doc, "/Configuration/Add");
    c.hr(hr, S_OK)?;
    id_of(c, add, "Product/ExcludeApp", "Access")?;
    id_of(c, add, "./Product", "O365ProPlusRetail")?;
    let (hr, up) = select_single(add, "..");
    c.hr(hr, S_OK)?;
    get_bstr(c, up, GET_NODE_NAME, "Configuration")?;
    release(up);
    release(add);
    // An attribute.
    let (hr, attr) = select_single(doc, "/Configuration/Add/@Channel");
    c.hr(hr, S_OK)?;
    c.eq(u64::from(node_type(attr)), 2)?;
    get_bstr(c, attr, GET_NODE_NAME, "Channel")?;
    get_bstr(c, attr, GET_TEXT, "Current")?;
    release(attr);
    // Nothing found.
    let (hr, none) = select_single(doc, "//NoSuch");
    c.hr(hr, S_FALSE)?;
    c.ok(none.is_null(), none as u64)?;
    let (hr, list) = select(doc, "//NoSuch");
    c.hr(hr, S_OK)?;
    c.eq(list_length(list), 0)?;
    release(list);
    // Lists.
    let (hr, list) = select(doc, "//ExcludeApp");
    c.hr(hr, S_OK)?;
    c.eq(list_length(list), 2)?;
    for want in ["Access", "Groove"] {
        let mut node: Obj = null_mut();
        c.hr(call!(list, LIST_NEXT_NODE, fn(*mut Obj) -> HResult, &mut node), S_OK)?;
        let (hr, el) = query(node, &IID_IXMLDOMELEMENT);
        release(node);
        c.hr(hr, S_OK)?;
        attribute(c, el, "ID", want)?;
        release(el);
    }
    let mut node: Obj = 0x1 as Obj;
    c.hr(call!(list, LIST_NEXT_NODE, fn(*mut Obj) -> HResult, &mut node), S_FALSE)?;
    c.ok(node.is_null(), 0)?;
    c.hr(call!(list, LIST_RESET, fn() -> HResult,), S_OK)?;
    c.hr(call!(list, LIST_NEXT_NODE, fn(*mut Obj) -> HResult, &mut node), S_OK)?;
    release(node);
    release(list);
    let (hr, list) = select(doc, "/Configuration/*");
    c.hr(hr, S_OK)?;
    c.eq(list_length(list), 3)?;
    release(list);
    // Prefixes: only those SelectionNamespaces declares.
    let (hr, _) = select_single(doc, "//y:Item");
    c.hr(hr, E_FAIL)?;
    let namespaces = Wide::new("SelectionNamespaces");
    let decl = Wide::new("xmlns:y='urn:x'");
    c.hr(
        call!(doc, SET_PROPERTY, fn(Bstr, Variant) -> HResult, namespaces.0, Variant::bstr(&decl)),
        S_OK,
    )?;
    let (hr, item) = select_single(doc, "//y:Item");
    c.hr(hr, S_OK)?;
    get_bstr(c, item, GET_TEXT, "text & more")?;
    release(item);
    let (hr, attr) = select_single(doc, "//y:Item/@y:Kind");
    c.hr(hr, S_OK)?;
    get_bstr(c, attr, GET_TEXT, "k")?;
    release(attr);
    // A syntax error.
    let (hr, _) = select(doc, "//[");
    c.hr(hr, E_FAIL)
}

fn errors(c: &mut Checks, doc: Obj) -> Result<(), u32> {
    // (The line of an empty document's error is not checked.)
    for (xml, line) in
        [("<a>\n<b></a>", Some(2)), ("", None), ("<!DOCTYPE a [<!ELEMENT a ANY>]><a/>", Some(1))]
    {
        let (hr, ok) = load_xml(doc, xml);
        c.hr(hr, S_FALSE)?;
        c.eq(ok as u16 as u64, VARIANT_FALSE as u16 as u64)?;
        let (hr, error) = get_obj(doc, GET_PARSE_ERROR);
        c.hr(hr, S_OK)?;
        let mut code = 0i32;
        call!(error, PE_GET_ERROR_CODE, fn(*mut i32) -> HResult, &mut code);
        let mut at = -1i32;
        call!(error, PE_GET_LINE, fn(*mut i32) -> HResult, &mut at);
        release(error);
        c.ok(code < 0, code as u32 as u64)?;
        if let Some(line) = line {
            c.eq(at as u32 as u64, line)?;
        }
        let (hr, root) = get_obj(doc, GET_DOCUMENT_ELEMENT);
        c.hr(hr, S_FALSE)?;
        c.ok(root.is_null(), 0)?;
    }
    Ok(())
}

/// With preserveWhiteSpace on, the white space between elements is kept
/// as text nodes.
fn white_space(c: &mut Checks, doc: Obj) -> Result<(), u32> {
    c.hr(call!(doc, PUT_PRESERVE_WHITE_SPACE, fn(i16) -> HResult, VARIANT_TRUE), S_OK)?;
    get_bool(c, doc, GET_PRESERVE_WHITE_SPACE, VARIANT_TRUE)?;
    let (hr, ok) = load_xml(doc, DOC);
    c.hr(hr, S_OK)?;
    c.eq(ok as u16 as u64, VARIANT_TRUE as u16 as u64)?;
    let (_, root) = get_obj(doc, GET_DOCUMENT_ELEMENT);
    let (hr, kids) = get_obj(root, GET_CHILD_NODES);
    c.hr(hr, S_OK)?;
    c.eq(list_length(kids), 7)?;
    release(kids);
    let (_, first) = get_obj(root, GET_FIRST_CHILD);
    c.eq(u64::from(node_type(first)), 3)?;
    release(first);
    release(root);
    c.hr(call!(doc, PUT_PRESERVE_WHITE_SPACE, fn(i16) -> HResult, VARIANT_FALSE), S_OK)
}

fn file(c: &mut Checks, doc: Obj) -> Result<(), u32> {
    let mut path = [0u16; 300];
    // SAFETY: a buffer of 300 characters.
    let n = unsafe { GetTempPathW(260, path.as_mut_ptr()) } as usize;
    c.ok(n > 0 && n < 260, n as u64)?;
    for (slot, u) in path[n..].iter_mut().zip("gala_msxml_test.xml".encode_utf16()) {
        *slot = u;
    }
    // SAFETY: a NUL-terminated path.
    let h = unsafe { CreateFileW(path.as_ptr(), 0x4000_0000, 0, 0, 2, 0x80, 0) };
    c.ok(h != usize::MAX, 0)?;
    let mut written = 0;
    // SAFETY: our handle and text.
    unsafe {
        WriteFile(h, DOC.as_ptr(), DOC.len() as u32, &mut written, 0);
        CloseHandle(h);
    }
    c.hr(call!(doc, PUT_ASYNC, fn(i16) -> HResult, VARIANT_FALSE), S_OK)?;
    get_bool(c, doc, GET_ASYNC, VARIANT_FALSE)?;
    // SAFETY: a NUL-terminated path.
    let source = Wide(unsafe { SysAllocString(path.as_ptr()) });
    let mut ok: i16 = 0x55;
    let hr = call!(doc, LOAD, fn(Variant, *mut i16) -> HResult, Variant::bstr(&source), &mut ok);
    c.hr(hr, S_OK)?;
    c.eq(ok as u16 as u64, VARIANT_TRUE as u16 as u64)?;
    id_of(c, doc, "//Language", "en-us")?;
    // SAFETY: a NUL-terminated path.
    unsafe { DeleteFileW(path.as_ptr()) };
    let hr = call!(doc, LOAD, fn(Variant, *mut i16) -> HResult, Variant::bstr(&source), &mut ok);
    c.hr(hr, S_FALSE)?;
    c.eq(ok as u16 as u64, VARIANT_FALSE as u16 as u64)
}

/// Wine's complete4A (domdoc.c), test_cloneNode's document.
const COMPLETE4: &str = "<?xml version='1.0'?>\n<lc dl='str1'>\n<bs vr='str2' sz='1234'>fn1.txt\n</bs>\n\
<pr id='str3' vr='1.2.3' pn='wine 20050804'>\nfn2.txt\n</pr>\n<empty></empty>\n\
<fo>\n<ba>\nf1\n</ba>\n</fo>\n</lc>\n";

fn clone_of(obj: Obj, deep: i16) -> (HResult, Obj) {
    let mut out: Obj = 0x1 as Obj;
    let hr = call!(obj, CLONE_NODE, fn(i16, *mut Obj) -> HResult, deep, &mut out);
    (hr, out)
}

/// The length of `obj`'s childNodes (or attributes, `slot`).
fn count(c: &mut Checks, obj: Obj, slot: usize, length: usize, want: u64) -> Result<(), u32> {
    let (hr, list) = get_obj(obj, slot);
    c.hr(hr, S_OK)?;
    let mut n = -1i32;
    c.hr(call!(list, length, fn(*mut i32) -> HResult, &mut n), S_OK)?;
    release(list);
    c.eq(n as u32 as u64, want)
}

/// cloneNode, as Wine's test_cloneNode checks it on Windows: a NULL
/// output is E_INVALIDARG; the document cloned deep has its two children
/// and the original's properties (Wine: SelectionLanguage), shallow no
/// documentElement; an element cloned deep has its children, its three
/// attributes (getNamedItem("id") finds one) and an ownerDocument,
/// shallow no children but the attributes. From MSXML's documentation of
/// cloneNode: the clone has no parent (parentNode S_FALSE and NULL); its
/// xml is the original's.
fn clone(c: &mut Checks, doc: Obj) -> Result<(), u32> {
    let (hr, ok) = load_xml(doc, COMPLETE4);
    c.hr(hr, S_OK)?;
    c.eq(ok as u16 as u64, VARIANT_TRUE as u16 as u64)?;
    let name = Wide::new("SelectionNamespaces");
    let value = Wide::new("xmlns:w='urn:wine'");
    let hr = call!(doc, SET_PROPERTY, fn(Bstr, Variant) -> HResult, name.0, Variant::bstr(&value));
    c.hr(hr, S_OK)?;
    c.hr(
        call!(doc, CLONE_NODE, fn(i16, *mut Obj) -> HResult, VARIANT_FALSE, null_mut()),
        E_INVALIDARG,
    )?;

    let (hr, copy) = clone_of(doc, VARIANT_TRUE);
    c.hr(hr, S_OK)?;
    c.eq(u64::from(node_type(copy)), 9)?;
    count(c, copy, GET_CHILD_NODES, LIST_GET_LENGTH, 2)?;
    let (hr, copy2) = query(copy, &IID_IXMLDOMDOCUMENT2);
    c.hr(hr, S_OK)?;
    let mut v = Variant::empty();
    c.hr(call!(copy2, GET_PROPERTY, fn(Bstr, *mut Variant) -> HResult, name.0, &mut v), S_OK)?;
    c.var_text(&mut v, "xmlns:w='urn:wine'")?;
    let (hr, el) = get_obj(copy2, GET_DOCUMENT_ELEMENT);
    c.hr(hr, S_OK)?;
    get_bstr(c, el, GET_NODE_NAME, "lc")?;
    release(el);
    release(copy2);
    release(copy);
    let (hr, copy) = clone_of(doc, VARIANT_FALSE);
    c.hr(hr, S_OK)?;
    // documentElement is IXMLDOMDocument's, so it is asked through the
    // clone's IXMLDOMDocument2 (through the IXMLDOMNode cloneNode returns,
    // Windows' CI run answered E_INVALIDARG): S_FALSE and no element.
    let (hr, copy2) = query(copy, &IID_IXMLDOMDOCUMENT2);
    c.hr(hr, S_OK)?;
    let (hr, el) = get_obj(copy2, GET_DOCUMENT_ELEMENT);
    c.hr(hr, S_FALSE)?;
    c.ok(el.is_null(), el as u64)?;
    release(copy2);
    release(copy);

    let (hr, pr) = select_single(doc, "lc/pr");
    c.hr(hr, S_OK)?;
    c.hr(
        call!(pr, CLONE_NODE, fn(i16, *mut Obj) -> HResult, VARIANT_TRUE, null_mut()),
        E_INVALIDARG,
    )?;
    let mut xml: Bstr = null_mut();
    c.hr(call!(pr, GET_XML, fn(*mut Bstr) -> HResult, &mut xml), S_OK)?;
    let (hr, copy) = clone_of(pr, VARIANT_TRUE);
    c.hr(hr, S_OK)?;
    let (hr, first) = get_obj(copy, GET_FIRST_CHILD);
    c.hr(hr, S_OK)?;
    release(first);
    let (hr, owner) = get_obj(copy, GET_OWNER_DOCUMENT);
    c.hr(hr, S_OK)?;
    release(owner);
    let (hr, parent) = get_obj(copy, GET_PARENT_NODE);
    c.hr(hr, S_FALSE)?;
    c.ok(parent.is_null(), parent as u64)?;
    count(c, copy, GET_CHILD_NODES, LIST_GET_LENGTH, 1)?;
    count(c, copy, GET_ATTRIBUTES, MAP_GET_LENGTH, 3)?;
    let (hr, map) = get_obj(copy, GET_ATTRIBUTES);
    c.hr(hr, S_OK)?;
    let id = Wide::new("id");
    let mut attr: Obj = null_mut();
    c.hr(call!(map, MAP_GET_NAMED_ITEM, fn(Bstr, *mut Obj) -> HResult, id.0, &mut attr), S_OK)?;
    release(attr);
    release(map);
    let mut copy_xml: Bstr = null_mut();
    c.hr(call!(copy, GET_XML, fn(*mut Bstr) -> HResult, &mut copy_xml), S_OK)?;
    c.ok(bstr_eq(xml, copy_xml), 0)?;
    release(copy);
    let (hr, copy) = clone_of(pr, VARIANT_FALSE);
    c.hr(hr, S_OK)?;
    let (hr, first) = get_obj(copy, GET_FIRST_CHILD);
    c.hr(hr, S_FALSE)?;
    c.ok(first.is_null(), first as u64)?;
    count(c, copy, GET_CHILD_NODES, LIST_GET_LENGTH, 0)?;
    count(c, copy, GET_ATTRIBUTES, MAP_GET_LENGTH, 3)?;
    release(copy);
    release(pr);
    let empty = Wide::new("");
    let hr = call!(doc, SET_PROPERTY, fn(Bstr, Variant) -> HResult, name.0, Variant::bstr(&empty));
    c.hr(hr, S_OK)
}

/// An `info` line with a string: what Windows answers, for the record
/// (not checked; non-ASCII as `?`).
fn info_text(what: &str, s: Bstr) {
    let mut line = Line::new().s(NAME.as_bytes()).s(b": info ").s(what.as_bytes()).s(b" ");
    let mut i = 0;
    while !s.is_null() {
        // SAFETY: a NUL-terminated string, read up to its NUL.
        let u = unsafe { *s.add(i) };
        if u == 0 {
            break;
        }
        line = line.s(&[if (0x20..0x7f).contains(&u) { u as u8 } else { b'?' }]);
        i += 1;
    }
    line.s(b"\r\n").print();
}

const NS_DOC: &str = "<r xmlns=\"urn:d\" xmlns:a=\"urn:a\" xmlns:u=\"urn:u\">\
<a:p a:k=\"v\"><a:c/></a:p><q/><t v=\"u:n\"/></r>";

/// The xml of `node`.
fn xml_of(c: &mut Checks, node: Obj) -> Result<Bstr, u32> {
    let mut s: Bstr = null_mut();
    c.hr(call!(node, GET_XML, fn(*mut Bstr) -> HResult, &mut s), S_OK)?;
    c.ok(!s.is_null(), 0)?;
    Ok(s)
}

/// `xml` (freed) loaded again: it should parse, its document element
/// named `local` in namespace `ns`, with an attribute `attr` in `ns`
/// when one is given.
fn reload(
    c: &mut Checks,
    doc: Obj,
    xml: Bstr,
    local: &str,
    ns: &str,
    attr: Option<&str>,
) -> Result<(), u32> {
    let mut ok: i16 = 0x55;
    let hr = call!(doc, LOAD_XML, fn(Bstr, *mut i16) -> HResult, xml, &mut ok);
    // SAFETY: our BSTR.
    unsafe { SysFreeString(xml) };
    c.hr(hr, S_OK)?;
    c.eq(ok as u16 as u64, VARIANT_TRUE as u16 as u64)?;
    let (hr, el) = get_obj(doc, GET_DOCUMENT_ELEMENT);
    c.hr(hr, S_OK)?;
    get_bstr(c, el, GET_BASE_NAME, local)?;
    get_bstr(c, el, GET_NAMESPACE_URI, ns)?;
    if let Some(attr) = attr {
        let (hr, map) = get_obj(el, GET_ATTRIBUTES);
        c.hr(hr, S_OK)?;
        let (name, uri) = (Wide::new(attr), Wide::new(ns));
        let mut node: Obj = null_mut();
        let hr = call!(
            map,
            MAP_GET_QUALIFIED_ITEM,
            fn(Bstr, Bstr, *mut Obj) -> HResult,
            name.0,
            uri.0,
            &mut node
        );
        release(map);
        c.hr(hr, S_OK)?;
        release(node);
    }
    release(el);
    Ok(())
}

/// The xml of an element whose prefixes (or default namespace) are
/// declared on its ancestors. MSXML serializes so that the text is
/// well formed with its namespaces (Wine's domdoc.c, run on Windows: an
/// element in a namespace no declaration in the text binds gets one on
/// itself, `<elem2 xmlns="http://winehq.org/default"/>` for a child
/// asked alone, and `<a><ns:b xmlns:ns="uri" .../></a>` once a's
/// declaration is gone, while declarations already in the text are kept
/// as they are; Click-to-Run's App-V orchestration loads such a node's
/// xml again). Checked: the document element's xml is the source as is,
/// and each subtree's xml loads again with its names in their
/// namespaces. The exact text of each (whether declarations the subtree
/// does not use come along: `u` for `<t v="u:n"/>`) is an `info` line
/// until Windows' run settles it.
fn inherited_namespaces(c: &mut Checks, doc: Obj) -> Result<(), u32> {
    let (hr, ok) = load_xml(doc, NS_DOC);
    c.hr(hr, S_OK)?;
    c.eq(ok as u16 as u64, VARIANT_TRUE as u16 as u64)?;
    let (hr, root) = get_obj(doc, GET_DOCUMENT_ELEMENT);
    c.hr(hr, S_OK)?;
    get_bstr(c, root, GET_XML, NS_DOC)?;
    let (hr, p) = get_obj(root, GET_FIRST_CHILD);
    c.hr(hr, S_OK)?;
    release(root);
    let (hr, inner) = get_obj(p, GET_FIRST_CHILD);
    c.hr(hr, S_OK)?;
    let (hr, q) = get_obj(p, GET_NEXT_SIBLING);
    c.hr(hr, S_OK)?;
    let (hr, t) = get_obj(q, GET_NEXT_SIBLING);
    c.hr(hr, S_OK)?;
    let p_xml = xml_of(c, p)?;
    let inner_xml = xml_of(c, inner)?;
    let q_xml = xml_of(c, q)?;
    let t_xml = xml_of(c, t)?;
    for node in [p, inner, q, t] {
        release(node);
    }
    for (what, s) in
        [("a:p xml", p_xml), ("a:c xml", inner_xml), ("q xml", q_xml), ("t xml", t_xml)]
    {
        info_text(what, s);
    }
    reload(c, doc, p_xml, "p", "urn:a", Some("k"))?;
    reload(c, doc, inner_xml, "c", "urn:a", None)?;
    reload(c, doc, q_xml, "q", "urn:d", None)?;
    reload(c, doc, t_xml, "t", "urn:d", None)
}

fn run() -> Result<(), u32> {
    let mut c = Checks(0, 0);
    // SAFETY: plain call.
    c.hr(unsafe { CoInitializeEx(null_mut(), COINIT_APARTMENTTHREADED) }, S_OK)?;
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
    c.ok(!doc.is_null(), 0)?;
    defaults(&mut c, doc)?;
    tree(&mut c, doc)?;
    xpath(&mut c, doc)?;
    errors(&mut c, doc)?;
    white_space(&mut c, doc)?;
    file(&mut c, doc)?;
    clone(&mut c, doc)?;
    inherited_namespaces(&mut c, doc)?;
    mutation::mutation(&mut c, doc)?;
    mutation::section("schema");
    schema::schema(&mut c)?;
    mutation::section("typed");
    typed::typed(&mut c)?;
    mutation::section("free-threaded");
    free_threaded(&mut c)?;
    xslt::xslt(&mut c)?;
    release(doc);
    // SAFETY: plain call.
    unsafe { CoUninitialize() };
    implicit_mta(&mut c)?;
    match c.1 {
        0 => Ok(()),
        first => Err(first),
    }
}

/// FreeThreadedDOMDocument60 (msxml6.h's CLSID; Click-to-Run's App-V
/// pieces create it): the same document interfaces, loading and
/// selecting as DOMDocument60's.
fn free_threaded(c: &mut Checks) -> Result<(), u32> {
    let mut doc: Obj = null_mut();
    // SAFETY: valid GUIDs and output.
    let hr = unsafe {
        CoCreateInstance(
            &CLSID_FREETHREADEDDOMDOCUMENT60,
            null_mut(),
            CLSCTX_INPROC_SERVER,
            &IID_IXMLDOMDOCUMENT2,
            &mut doc,
        )
    };
    c.hr(hr, S_OK)?;
    let (hr, doc3) = query(doc, &IID_IXMLDOMDOCUMENT3);
    c.hr(hr, S_OK)?;
    release(doc3);
    let (hr, ok) = load_xml(doc, "<a><b/></a>");
    c.hr(hr, S_OK)?;
    c.eq(ok as u16 as u64, VARIANT_TRUE as u16 as u64)?;
    let (hr, b) = select_single(doc, "a/b");
    c.hr(hr, S_OK)?;
    release(b);
    release(doc);
    Ok(())
}

/// What CoCreateInstance(CLSID_DOMDocument60) gave on a thread that
/// never initialized COM.
static mut IMPLICIT_HR: HResult = 0;

extern "system" fn uninitialized_thread(_: *mut c_void) -> u32 {
    let mut doc: Obj = null_mut();
    // SAFETY: valid GUIDs and output; the static is read after the
    // thread ends.
    unsafe {
        IMPLICIT_HR = CoCreateInstance(
            &CLSID_DOMDOCUMENT60,
            null_mut(),
            CLSCTX_INPROC_SERVER,
            &IID_IXMLDOMDOCUMENT2,
            &mut doc,
        );
    }
    if !doc.is_null() {
        release(doc);
    }
    0
}

/// CoCreateInstance on a new thread that never initializes COM.
fn on_uninitialized_thread() -> HResult {
    // SAFETY: plain calls; the thread has ended when the static is read.
    unsafe {
        let t = CreateThread(null_mut(), 0, uninitialized_thread, null_mut(), 0, null_mut());
        if t == 0 {
            return -1;
        }
        WaitForSingleObject(t, 0xffff_ffff);
        CloseHandle(t);
        IMPLICIT_HR
    }
}

/// A thread that never initialized COM creates objects as a member of the
/// process's MTA when there is one (the implicit MTA, as Microsoft's
/// documentation of CoGetApartmentType's APTTYPEQUALIFIER_IMPLICIT_MTA
/// describes; the Click-to-Run service's process pool does so), and
/// gets CO_E_NOTINITIALIZED when there is none.
fn implicit_mta(c: &mut Checks) -> Result<(), u32> {
    c.hr(on_uninitialized_thread(), CO_E_NOTINITIALIZED)?;
    // SAFETY: plain call.
    c.hr(unsafe { CoInitializeEx(null_mut(), COINIT_MULTITHREADED) }, S_OK)?;
    c.hr(on_uninitialized_thread(), S_OK)?;
    // SAFETY: plain call.
    unsafe { CoUninitialize() };
    Ok(())
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    let code = match run() {
        Ok(()) => {
            Line::new().s(NAME.as_bytes()).s(b": ok\r\n").print();
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
