//! Typed values: IXMLDOMNode's nodeTypedValue and dataType. Click-to-Run's
//! App-V pieces read typed values from the package's deployment
//! configuration when they configure Office's package (ROADMAP OF).
//!
//! Expected values are Wine's msxml3 tests run on Windows (domdoc.c:
//! test_nodeTypedValue on its szTypeValueXML, test_put_nodeTypedValue,
//! test_put_dataType, test_get_dataType, test_xmlns_attribute) and
//! MSXML's documentation ("nodeTypedValue Property", "dataType
//! Property"): an element typed by a `dt:dt` attribute in
//! urn:schemas-microsoft-com:datatypes gives its content as that type's
//! VARIANT (VT_I4 for int and i4, VT_CY for fixed.14.4, VT_DATE for the
//! dates and times with a zone's offset added, VT_ARRAY|VT_UI1 for
//! bin.hex and bin.base64, ...), an untyped element its text, a PI,
//! CDATA section or comment its value, a document or fragment S_FALSE
//! and VT_NULL; put_dataType takes a type name in any case. Where
//! neither settles the value, it is a non-stopping check (`note`, a type
//! name's spelling) or MSXML 6's answer on Windows (build 26100), which
//! differs from Wine's msxml3 numbers where a comment says so; what a
//! put_dataType MSXML 6 takes leaves is printed as `info` lines.

use core::ptr::null_mut;

use super::mutation::{create, new_doc, xml_is};
use super::*;

#[cfg_attr(
    target_arch = "x86",
    link(name = "oleaut32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "oleaut32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn SafeArrayCreateVector(vt: u16, lbound: i32, count: u32) -> SafeArray;
    fn SafeArrayAccessData(sa: SafeArray, data: *mut *mut u8) -> HResult;
    fn SafeArrayUnaccessData(sa: SafeArray) -> HResult;
    fn SafeArrayGetDim(sa: SafeArray) -> u32;
    fn SafeArrayGetUBound(sa: SafeArray, dim: u32, bound: *mut i32) -> HResult;
    fn SafeArrayGetLBound(sa: SafeArray, dim: u32, bound: *mut i32) -> HResult;
    fn SafeArrayDestroy(sa: SafeArray) -> HResult;
}

type SafeArray = *mut c_void;

const VT_I2: u16 = 2;
const VT_R4: u16 = 4;
const VT_R8: u16 = 5;
const VT_CY: u16 = 6;
const VT_DATE: u16 = 7;
const VT_I1: u16 = 16;
const VT_UI1: u16 = 17;
const VT_UI2: u16 = 18;
const VT_UI4: u16 = 19;
const VT_ARRAY: u16 = 0x2000;
const VT_BYTES: u16 = VT_ARRAY | VT_UI1;

const PUT_NODE_VALUE: usize = 9;
const REMOVE_CHILD: usize = 20;
const APPEND_CHILD: usize = 21;
const GET_NODE_TYPED_VALUE: usize = 30;
const PUT_NODE_TYPED_VALUE: usize = 31;
const GET_DATA_TYPE: usize = 32;
const PUT_DATA_TYPE: usize = 33;
const CREATE_ELEMENT: usize = 47;
const CREATE_DOCUMENT_FRAGMENT: usize = 48;
const CREATE_COMMENT: usize = 50;
const CREATE_TEXT_NODE: usize = 49;
const CREATE_CDATA_SECTION: usize = 51;
const CREATE_PROCESSING_INSTRUCTION: usize = 52;
const CREATE_ATTRIBUTE: usize = 53;
const CREATE_NODE: usize = 56;
const SET_ATTRIBUTE_NODE: usize = 48;

const DT_NS: &str = "urn:schemas-microsoft-com:datatypes";

/// Wine's szTypeValueXML.
const TYPED: &str = "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n\
<root xmlns:dt=\"urn:schemas-microsoft-com:datatypes\">\n\
   <string>Wine</string>\n\
   <string2 dt:dt=\"string\">String </string2>\n\
   <string3> Wine Wine </string3>\n\
   <string4>\nWine Wine\t</string4>\n\
   <string5>   </string5>\n\
   <nested><s1> s1 </s1><s2>\ns2 s2\t</s2></nested>\n\
   <number dt:dt=\"number\">12.44</number>\n\
   <number2 dt:dt=\"NUMbEr\">-3.71e3</number2>\n\
   <int dt:dt=\"int\">-13</int>\n\
   <fixed dt:dt=\"fixed.14.4\">7322.9371</fixed>\n\
   <bool dt:dt=\"boolean\">1</bool>\n\
   <datetime dt:dt=\"datetime\">2009-11-18T03:21:33.12</datetime>\n\
   <datetimetz dt:dt=\"datetime.tz\">2003-07-11T11:13:57+03:00</datetimetz>\n\
   <date dt:dt=\"date\">3721-11-01</date>\n\
   <time dt:dt=\"time\">13:57:12.31321</time>\n\
   <timetz dt:dt=\"time.tz\">23:21:01.13+03:21</timetz>\n\
   <i1 dt:dt=\"i1\">-13</i1>\n\
   <i2 dt:dt=\"i2\">31915</i2>\n\
   <i4 dt:dt=\"i4\">-312232</i4>\n\
   <ui1 dt:dt=\"ui1\">123</ui1>\n\
   <ui2 dt:dt=\"ui2\">48282</ui2>\n\
   <ui4 dt:dt=\"ui4\">949281</ui4>\n\
   <r4 dt:dt=\"r4\">213124.0</r4>\n\
   <r8 dt:dt=\"r8\">0.412</r8>\n\
   <float dt:dt=\"float\">41221.421</float>\n\
   <uuid dt:dt=\"uuid\">333C7BC4-460F-11D0-BC04-0080C7055a83</uuid>\n\
   <binhex dt:dt=\"bin.hex\">fffca012003c</binhex>\n\
   <binbase64 dt:dt=\"bin.base64\">YmFzZTY0IHRlc3Q=</binbase64>\n\
   <binbase64_1 dt:dt=\"bin.base64\">\nYmFzZTY0\nIHRlc3Q=\n</binbase64_1>\n\
   <binbase64_2 dt:dt=\"bin.base64\">\nYmF\r\t z  ZTY0\nIHRlc3Q=\n</binbase64_2>\n\
</root>";

pub fn typed(c: &mut Checks) -> Result<(), u32> {
    let doc = new_doc(c)?;
    get_values(c, doc)?;
    untyped_strings(c, doc)?;
    data_types(c, doc)?;
    other_nodes(c, doc)?;
    release(doc);
    put_typed_value(c)?;
    put_data_type(c)?;
    get_data_type(c)?;
    xmlns_attribute(c)?;
    unsettled(c)?;
    Ok(())
}

fn get_typed(node: Obj) -> (HResult, Variant) {
    let mut v = Variant::empty();
    let hr = call!(node, GET_NODE_TYPED_VALUE, fn(*mut Variant) -> HResult, &mut v);
    (hr, v)
}

fn put_typed(node: Obj, v: Variant) -> HResult {
    call!(node, PUT_NODE_TYPED_VALUE, fn(Variant) -> HResult, v)
}

fn put_type(node: Obj, name: Option<&str>) -> HResult {
    let w = name.map(Wide::new);
    call!(node, PUT_DATA_TYPE, fn(Bstr) -> HResult, w.as_ref().map_or(null_mut(), |w| w.0))
}

fn get_type(node: Obj) -> (HResult, Variant) {
    let mut v = Variant::empty();
    let hr = call!(node, GET_DATA_TYPE, fn(*mut Variant) -> HResult, &mut v);
    (hr, v)
}

/// The typed value of the node `path` selects in `doc`: S_OK, of type
/// `vt`.
fn value_of(c: &mut Checks, doc: Obj, path: &str, vt: u16) -> Result<Variant, u32> {
    let (hr, node) = select_single(doc, path);
    c.hr(hr, S_OK)?;
    let (hr, v) = get_typed(node);
    release(node);
    c.hr(hr, S_OK)?;
    c.eq(u64::from(v.vt), u64::from(vt))?;
    Ok(v)
}

/// The VARIANT's 8 bytes of value (an i64, a double, a CY or a DATE).
fn bits64(v: &Variant) -> u64 {
    // SAFETY: the value field holds at least 8 bytes on x86 and x64.
    unsafe { core::ptr::read_unaligned(v.value.as_ptr() as *const u64) }
}

fn int(c: &mut Checks, doc: Obj, path: &str, vt: u16, want: i64) -> Result<(), u32> {
    let v = value_of(c, doc, path, vt)?;
    let raw = v.value[0];
    let got = match vt {
        VT_I1 => i64::from(raw as u8 as i8),
        VT_I2 => i64::from(raw as u16 as i16),
        VT_I4 => i64::from(raw as u32 as i32),
        VT_UI1 => i64::from(raw as u8),
        VT_UI2 => i64::from(raw as u16),
        VT_UI4 => i64::from(raw as u32),
        VT_BOOL => i64::from(raw as u16 as i16),
        VT_CY => bits64(&v) as i64,
        _ => i64::MIN,
    };
    c.eq(got as u64, want as u64)
}

fn abs(d: f64) -> f64 {
    if d < 0.0 { -d } else { d }
}

/// A floating value: within `tolerance` of `want`.
fn real(
    c: &mut Checks,
    doc: Obj,
    path: &str,
    vt: u16,
    want: f64,
    tolerance: f64,
) -> Result<f64, u32> {
    let v = value_of(c, doc, path, vt)?;
    let got = if vt == VT_R4 {
        f64::from(f32::from_bits(v.value[0] as u32))
    } else {
        f64::from_bits(bits64(&v))
    };
    c.ok(abs(got - want) <= tolerance, got.to_bits())?;
    Ok(got)
}

/// A date: Wine's check (the value as an R4, printed with 7 digits:
/// within `tolerance` of `wine`), then, not stopping, the date with
/// whole seconds (`exact`).
fn date(
    c: &mut Checks,
    doc: Obj,
    path: &str,
    wine: f64,
    tolerance: f64,
    exact: f64,
) -> Result<(), u32> {
    let got = real(c, doc, path, VT_DATE, wine, tolerance)?;
    c.note(abs(got - exact) < 1e-9, got.to_bits())
}

/// A VT_ARRAY|VT_UI1 (cleared): one dimension from 0, holding `want`.
fn bytes(c: &mut Checks, v: &mut Variant, want: &[u8]) -> Result<(), u32> {
    c.eq(u64::from(v.vt), u64::from(VT_BYTES))?;
    let sa = v.value[0] as SafeArray;
    c.ok(!sa.is_null(), 0)?;
    if v.vt != VT_BYTES || sa.is_null() {
        v.clear();
        return Ok(());
    }
    // SAFETY: the SAFEARRAY of a VARIANT msxml6 gave.
    c.eq(u64::from(unsafe { SafeArrayGetDim(sa) }), 1)?;
    let (mut lower, mut upper) = (-5, -5);
    // SAFETY: as above; valid outputs.
    c.hr(unsafe { SafeArrayGetLBound(sa, 1, &mut lower) }, S_OK)?;
    c.eq(lower as u64, 0)?;
    // SAFETY: as above.
    c.hr(unsafe { SafeArrayGetUBound(sa, 1, &mut upper) }, S_OK)?;
    c.eq(upper as i64 as u64, want.len() as u64 - 1)?;
    let mut data: *mut u8 = null_mut();
    // SAFETY: as above.
    c.hr(unsafe { SafeArrayAccessData(sa, &mut data) }, S_OK)?;
    // SAFETY: the array holds `want.len()` bytes (its bounds, checked).
    let same = unsafe { core::slice::from_raw_parts(data, want.len()) } == want;
    // SAFETY: accessed above.
    unsafe { SafeArrayUnaccessData(sa) };
    v.clear();
    c.ok(same, 0)
}

/// A VT_ARRAY|VT_UI1 VARIANT holding `data` (SafeArrayDestroy it).
fn byte_array(c: &mut Checks, data: &[u8]) -> Result<Variant, u32> {
    // SAFETY: plain call.
    let sa = unsafe { SafeArrayCreateVector(VT_UI1, 0, data.len() as u32) };
    c.ok(!sa.is_null(), 0)?;
    let mut p: *mut u8 = null_mut();
    // SAFETY: a fresh vector; valid output.
    c.hr(unsafe { SafeArrayAccessData(sa, &mut p) }, S_OK)?;
    // SAFETY: the vector holds data.len() bytes.
    unsafe {
        core::ptr::copy_nonoverlapping(data.as_ptr(), p, data.len());
        SafeArrayUnaccessData(sa);
    }
    Ok(Variant { vt: VT_BYTES, reserved: [0; 3], value: [sa as usize, 0] })
}

fn destroy(v: Variant) {
    // SAFETY: an array byte_array made.
    unsafe { SafeArrayDestroy(v.value[0] as SafeArray) };
}

/// test_nodeTypedValue: each typed element of szTypeValueXML.
fn get_values(c: &mut Checks, doc: Obj) -> Result<(), u32> {
    c.eq(load_xml(doc, TYPED).1 as u16 as u64, VARIANT_TRUE as u16 as u64)?;
    // The document: E_INVALIDARG without an output, S_FALSE and VT_NULL.
    c.hr(call!(doc, GET_NODE_TYPED_VALUE, fn(*mut Variant) -> HResult, null_mut()), E_INVALIDARG)?;
    let (hr, v) = get_typed(doc);
    c.hr(hr, S_FALSE)?;
    c.eq(u64::from(v.vt), u64::from(VT_NULL))?;
    let (hr, node) = select_single(doc, "root/string");
    c.hr(hr, S_OK)?;
    c.hr(call!(node, GET_NODE_TYPED_VALUE, fn(*mut Variant) -> HResult, null_mut()), E_INVALIDARG)?;
    release(node);

    let mut v = value_of(c, doc, "root/string2", VT_BSTR)?;
    // MSXML 6 trims a string type's text as `text` does (Wine's msxml3
    // test has "String ").
    c.var_text(&mut v, "String")?;
    let mut v = value_of(c, doc, "root/number", VT_BSTR)?;
    c.var_text(&mut v, "12.44")?;
    let mut v = value_of(c, doc, "root/number2", VT_BSTR)?;
    c.var_text(&mut v, "-3.71e3")?;
    let mut v = value_of(c, doc, "root/uuid", VT_BSTR)?;
    c.var_text(&mut v, "333C7BC4-460F-11D0-BC04-0080C7055a83")?;

    int(c, doc, "root/int", VT_I4, -13)?;
    int(c, doc, "root/fixed", VT_CY, 73_229_371)?;
    int(c, doc, "root/bool", VT_BOOL, -1)?;
    int(c, doc, "root/i1", VT_I1, -13)?;
    int(c, doc, "root/i2", VT_I2, 31915)?;
    int(c, doc, "root/i4", VT_I4, -312_232)?;
    int(c, doc, "root/ui1", VT_UI1, 123)?;
    int(c, doc, "root/ui2", VT_UI2, 48282)?;
    int(c, doc, "root/ui4", VT_UI4, 949_281)?;
    real(c, doc, "root/r4", VT_R4, 213_124.0, 0.0)?;
    real(c, doc, "root/r8", VT_R8, 0.412, 1e-15)?;
    real(c, doc, "root/float", VT_R8, 41221.421, 1e-9)?;

    // Wine's values are the dates as R4s printed with 7 digits. MSXML 6
    // takes a zoned time to UTC (11:13:57+03:00 is 08:13:57; Wine's
    // msxml3 test adds the offset) and drops a fraction of a second.
    date(c, doc, "root/datetime", 40135.14, 0.005, 40135.0 + 12093.0 / 86400.0)?;
    let utc = 37813.0 + 29637.0 / 86400.0;
    date(c, doc, "root/datetimetz", utc, 1e-9, utc)?;
    date(c, doc, "root/date", 665_413.0, 0.5, 665_413.0)?;
    date(c, doc, "root/time", 0.581_388_9, 5e-8, 50232.0 / 86400.0)?;
    // 23:21:01.13+03:21 is 20:00:01.
    date(c, doc, "root/timetz", 72001.0 / 86400.0, 1e-9, 72001.0 / 86400.0)?;

    let mut v = value_of(c, doc, "root/binhex", VT_BYTES)?;
    bytes(c, &mut v, &[0xff, 0xfc, 0xa0, 0x12, 0x00, 0x3c])?;
    for path in ["root/binbase64", "root/binbase64_1", "root/binbase64_2"] {
        let mut v = value_of(c, doc, path, VT_BYTES)?;
        bytes(c, &mut v, b"base64 test")?;
    }

    // A text node in a typed element: its value, untyped (MSXML 6).
    let mut v = value_of(c, doc, "root/int/text()", VT_BSTR)?;
    c.var_text(&mut v, "-13")?;
    // An attribute (dt:dt itself): its value, untyped.
    let (hr, node) = select_single(doc, "root/string2/@*");
    c.hr(hr, S_OK)?;
    let (hr, mut v) = get_typed(node);
    c.hr(hr, S_OK)?;
    c.var_text(&mut v, "string")?;
    let (hr, v) = get_type(node);
    release(node);
    c.hr(hr, S_FALSE)?;
    c.eq(u64::from(v.vt), u64::from(VT_NULL))
}

/// Untyped elements: their text, trimmed while preserveWhiteSpace is off,
/// as is when it is on.
fn untyped_strings(c: &mut Checks, doc: Obj) -> Result<(), u32> {
    let cases = [
        ("root/string", "Wine", "Wine"),
        ("root/string3", "Wine Wine", " Wine Wine "),
        ("root/string4", "Wine Wine", "\nWine Wine\t"),
        ("root/string5", "", ""),
        ("root/nested", "s1 \ns2 s2", " s1 \ns2 s2\t"),
    ];
    for (path, trimmed, raw) in cases {
        let (hr, node) = select_single(doc, path);
        c.hr(hr, S_OK)?;
        let (hr, mut v) = get_typed(node);
        c.hr(hr, S_OK)?;
        c.var_text(&mut v, trimmed)?;
        get_bstr(c, node, GET_TEXT, trimmed)?;
        c.hr(call!(doc, PUT_PRESERVE_WHITE_SPACE, fn(i16) -> HResult, VARIANT_TRUE), S_OK)?;
        let (hr, mut v) = get_typed(node);
        c.hr(hr, S_OK)?;
        if path == "root/string5" {
            // White space alone: whether loading kept it (Wine's msxml3
            // test has it kept; not settled for MSXML 6, which drops white
            // space between elements).
            c.var_text(&mut v, "")?;
        } else {
            c.var_text(&mut v, raw)?;
            get_bstr(c, node, GET_TEXT, raw)?;
        }
        c.hr(call!(doc, PUT_PRESERVE_WHITE_SPACE, fn(i16) -> HResult, VARIANT_FALSE), S_OK)?;
        // No type: dataType is S_FALSE and VT_NULL.
        let (hr, v) = get_type(node);
        release(node);
        c.hr(hr, S_FALSE)?;
        c.eq(u64::from(v.vt), u64::from(VT_NULL))?;
    }
    Ok(())
}

/// dataType of typed elements: the type's name (its spelling as MSXML
/// names it is not settled).
fn data_types(c: &mut Checks, doc: Obj) -> Result<(), u32> {
    let cases = [
        ("root/int", "int", true),
        ("root/string2", "string", true),
        ("root/binhex", "bin.hex", true),
        ("root/number2", "number", false),
        ("root/datetime", "dateTime", false),
        ("root/datetimetz", "dateTime.tz", false),
    ];
    for (path, want, sure) in cases {
        let (hr, node) = select_single(doc, path);
        c.hr(hr, S_OK)?;
        let (hr, mut v) = get_type(node);
        release(node);
        c.hr(hr, S_OK)?;
        if sure {
            c.var_text(&mut v, want)?;
        } else {
            c.eq(u64::from(v.vt), u64::from(VT_BSTR))?;
            let same = v.vt == VT_BSTR && bstr_is(v.value[0] as Bstr, want);
            if !same && v.vt == VT_BSTR {
                info_text("dataType", v.value[0] as Bstr);
            }
            v.clear();
            c.note(same, 0)?;
        }
    }
    Ok(())
}

/// A PI, CDATA section and comment: S_OK and their value; a fragment
/// S_FALSE and VT_NULL.
fn other_nodes(c: &mut Checks, doc: Obj) -> Result<(), u32> {
    let (t, d) = (Wide::new("foo"), Wide::new("value"));
    let mut pi: Obj = null_mut();
    let hr = call!(
        doc,
        CREATE_PROCESSING_INSTRUCTION,
        fn(Bstr, Bstr, *mut Obj) -> HResult,
        t.0,
        d.0,
        &mut pi
    );
    c.hr(hr, S_OK)?;
    let (hr, mut v) = get_typed(pi);
    release(pi);
    c.hr(hr, S_OK)?;
    c.var_text(&mut v, "value")?;
    let text = "[1]*2=3; &gee that's not right!";
    let cdata = create(c, doc, CREATE_CDATA_SECTION, text)?;
    let (hr, mut v) = get_typed(cdata);
    release(cdata);
    c.hr(hr, S_OK)?;
    c.var_text(&mut v, text)?;
    let comment = create(c, doc, CREATE_COMMENT, "comment")?;
    let (hr, mut v) = get_typed(comment);
    release(comment);
    c.hr(hr, S_OK)?;
    c.var_text(&mut v, "comment")?;
    let (hr, frag) = get_obj(doc, CREATE_DOCUMENT_FRAGMENT);
    c.hr(hr, S_OK)?;
    let (hr, v) = get_typed(frag);
    release(frag);
    c.hr(hr, S_FALSE)?;
    c.eq(u64::from(v.vt), u64::from(VT_NULL))
}

/// test_put_nodeTypedValue.
fn put_typed_value(c: &mut Checks) -> Result<(), u32> {
    let doc = new_doc(c)?;
    let el = create(c, doc, CREATE_ELEMENT, "Element")?;
    let (hr, v) = get_type(el);
    c.hr(hr, S_FALSE)?;
    c.eq(u64::from(v.vt), u64::from(VT_NULL))?;
    // An untyped element takes any value as text.
    let one = Variant { vt: VT_I1, reserved: [0; 3], value: [1, 0] };
    c.hr(put_typed(el, one), S_OK)?;
    let (hr, v) = get_type(el);
    c.hr(hr, S_FALSE)?;
    c.eq(u64::from(v.vt), u64::from(VT_NULL))?;
    let (hr, mut v) = get_typed(el);
    c.hr(hr, S_OK)?;
    c.var_text(&mut v, "1")?;

    // int: a string and an I2 put, an I4 read.
    c.hr(put_type(el, Some("int")), S_OK)?;
    let s = Wide::new("1");
    c.hr(put_typed(el, Variant::bstr(&s)), S_OK)?;
    let (hr, v) = get_typed(el);
    c.hr(hr, S_OK)?;
    c.eq(u64::from(v.vt), u64::from(VT_I4))?;
    c.eq(v.value[0] as u32 as u64, 1)?;
    let minus = Variant { vt: VT_I2, reserved: [0; 3], value: [0xffff, 0] };
    c.hr(put_typed(el, minus), S_OK)?;
    let (hr, v) = get_typed(el);
    c.hr(hr, S_OK)?;
    c.eq(u64::from(v.vt), u64::from(VT_I4))?;
    c.eq(v.value[0] as u32 as u64, 0xffff_ffff)?;
    get_bstr(c, el, GET_TEXT, "-1")?;
    // Not a number: the value cannot be coerced.
    let s = Wide::new("abc");
    c.code(put_typed(el, Variant::bstr(&s)), E_FAIL)?;

    let (hr, first) = get_obj(el, GET_FIRST_CHILD);
    c.hr(hr, S_OK)?;
    c.hr(call!(el, REMOVE_CHILD, fn(Obj, *mut Obj) -> HResult, first, null_mut()), S_OK)?;
    release(first);
    c.hr(call!(doc, APPEND_CHILD, fn(Obj, *mut Obj) -> HResult, el, null_mut()), S_OK)?;

    // bin.base64: a string is kept as is, bytes are encoded.
    c.hr(put_type(el, Some("bin.base64")), S_OK)?;
    let s = Wide::new("ABCD");
    c.hr(put_typed(el, Variant::bstr(&s)), S_OK)?;
    let (hr, mut v) = get_typed(el);
    c.hr(hr, S_OK)?;
    bytes(c, &mut v, &[0x00, 0x10, 0x83])?;
    let (hr, first) = get_obj(el, GET_FIRST_CHILD);
    c.hr(hr, S_OK)?;
    get_bstr(c, first, GET_TEXT, "ABCD")?;
    release(first);
    let array = byte_array(c, b"dGVzdA=")?;
    c.hr(put_typed(el, Variant { ..array }), S_OK)?;
    destroy(array);
    let (hr, mut v) = get_typed(el);
    c.hr(hr, S_OK)?;
    bytes(c, &mut v, b"dGVzdA=")?;
    let (hr, first) = get_obj(el, GET_FIRST_CHILD);
    c.hr(hr, S_OK)?;
    get_bstr(c, first, GET_TEXT, "ZEdWemRBPQ==")?;
    release(first);

    // bin.hex: lower-case digits.
    let s = Wide::new("");
    c.hr(put_typed(el, Variant::bstr(&s)), S_OK)?;
    c.hr(put_type(el, Some("bin.hex")), S_OK)?;
    let data: [u8; 16] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15];
    let array = byte_array(c, &data)?;
    c.hr(put_typed(el, Variant { ..array }), S_OK)?;
    destroy(array);
    let (hr, mut v) = get_typed(el);
    c.hr(hr, S_OK)?;
    bytes(c, &mut v, &data)?;
    let (hr, first) = get_obj(el, GET_FIRST_CHILD);
    c.hr(hr, S_OK)?;
    get_bstr(c, first, GET_TEXT, "000102030405060708090a0b0c0d0e0f")?;
    release(first);
    release(el);

    // The documentation's example: put_dataType declares the prefix.
    let el = create(c, doc, CREATE_ELEMENT, "Test")?;
    c.hr(put_type(el, Some("bin.hex")), S_OK)?;
    let s = Wide::new("ffab123d");
    c.hr(put_typed(el, Variant::bstr(&s)), S_OK)?;
    let (hr, mut v) = get_type(el);
    c.hr(hr, S_OK)?;
    c.var_text(&mut v, "bin.hex")?;
    let xml = xml_of(c, el)?;
    let want =
        "<Test xmlns:dt=\"urn:schemas-microsoft-com:datatypes\" dt:dt=\"bin.hex\">ffab123d</Test>";
    let same = bstr_is(xml, want);
    if !same {
        info_text("put_dataType xml", xml);
    }
    // SAFETY: the BSTR xml_of gave.
    unsafe { SysFreeString(xml) };
    c.note(same, 0)?;
    // A type the content is not of: S_OK (what it leaves is measured
    // by the info lines).
    c.hr(put_type(el, Some("boolean")), S_OK)?;
    after_put("boolean on bin.hex content", el);
    release(el);
    // Typed values of other types are written in the type's form.
    let el = create(c, doc, CREATE_ELEMENT, "b")?;
    c.hr(put_type(el, Some("boolean")), S_OK)?;
    let t = Variant { vt: VT_BOOL, reserved: [0; 3], value: [0xffff, 0] };
    c.hr(put_typed(el, t), S_OK)?;
    get_bstr(c, el, GET_TEXT, "1")?;
    release(el);

    // A document or fragment has no value.
    let s = Wide::new("x");
    c.code(put_typed(doc, Variant::bstr(&s)), E_INVALIDARG)?;
    release(doc);
    Ok(())
}

/// test_put_dataType: the names in any case; E_INVALIDARG for NULL,
/// E_FAIL for an unknown name and on nodes that cannot be typed.
fn put_data_type(c: &mut Checks) -> Result<(), u32> {
    let doc = new_doc(c)?;
    c.hr(
        call!(
            doc,
            CREATE_ELEMENT,
            fn(Bstr, *mut Obj) -> HResult,
            Wide::new("Testing").0,
            null_mut()
        ),
        E_INVALIDARG,
    )?;
    let root = create(c, doc, CREATE_ELEMENT, "Testing")?;
    c.hr(call!(doc, APPEND_CHILD, fn(Obj, *mut Obj) -> HResult, root, null_mut()), S_OK)?;
    c.hr(put_type(root, None), E_INVALIDARG)?;
    let names = [
        // MSXML 6 takes an unknown name (Wine's msxml3 test has E_FAIL).
        ("abcdefg", S_OK),
        ("Boolean", S_OK),
        ("String", S_OK),
        ("number", S_OK),
        ("InT", S_OK),
        ("fixed.14.4", S_OK),
        ("DateTime", S_OK),
        ("DateTime.tz", S_OK),
        ("Date", S_OK),
        ("Time", S_OK),
        ("Time.tz", S_OK),
        ("I1", S_OK),
        ("I2", S_OK),
        ("I4", S_OK),
        ("UI1", S_OK),
        ("UI2", S_OK),
        ("UI4", S_OK),
        ("r4", S_OK),
        ("r8", S_OK),
        ("float", S_OK),
        ("UuId", S_OK),
        ("bin.hex", S_OK),
        ("bin.base64", S_OK),
    ];
    for (name, want) in names {
        let el = create(c, doc, CREATE_ELEMENT, "test")?;
        c.hr(call!(root, APPEND_CHILD, fn(Obj, *mut Obj) -> HResult, el, null_mut()), S_OK)?;
        c.hr(put_type(el, Some(name)), want)?;
        release(el);
    }
    let el = create(c, doc, CREATE_ELEMENT, "Testing_Change")?;
    c.hr(call!(root, APPEND_CHILD, fn(Obj, *mut Obj) -> HResult, el, null_mut()), S_OK)?;
    c.hr(put_type(el, Some("DateTime.tz")), S_OK)?;
    c.hr(put_type(el, Some("string")), S_OK)?;
    release(el);

    // Nodes without a type (createNode with a VT_I2 type).
    for kind in [7u16, 11, 4, 8] {
        let ty = Variant { vt: VT_I2, reserved: [0; 3], value: [usize::from(kind), 0] };
        let n = Wide::new("testname");
        let mut node: Obj = null_mut();
        let hr = call!(
            doc,
            CREATE_NODE,
            fn(Variant, Bstr, Bstr, *mut Obj) -> HResult,
            ty,
            n.0,
            null_mut(),
            &mut node
        );
        c.hr(hr, S_OK)?;
        c.hr(call!(root, APPEND_CHILD, fn(Obj, *mut Obj) -> HResult, node, null_mut()), S_OK)?;
        c.hr(put_type(node, None), E_INVALIDARG)?;
        // MSXML 6: E_INVALIDARG (Wine's msxml3 test has E_FAIL).
        c.hr(put_type(node, Some("string")), E_INVALIDARG)?;
        c.hr(put_type(node, Some("number")), E_INVALIDARG)?;
        release(node);
    }
    // A text node: E_INVALIDARG; an attribute: S_OK (what it leaves is
    // measured by the info lines).
    let mut t: Obj = null_mut();
    let w = Wide::new("text");
    c.hr(call!(doc, CREATE_TEXT_NODE, fn(Bstr, *mut Obj) -> HResult, w.0, &mut t), S_OK)?;
    c.hr(put_type(t, Some("int")), E_INVALIDARG)?;
    release(t);
    let a = create(c, doc, CREATE_ATTRIBUTE, "a")?;
    c.hr(put_type(a, Some("int")), S_OK)?;
    after_put("int on an attribute", a);
    release(a);
    // An unknown name on an element: S_OK (as above).
    let el = create(c, doc, CREATE_ELEMENT, "u")?;
    c.hr(put_type(el, Some("abcdefg")), S_OK)?;
    after_put("abcdefg", el);
    release(el);
    release(root);
    release(doc);
    Ok(())
}

/// test_get_dataType: nodes made by createNode (VT_I4 types) and the
/// document have no type.
fn get_data_type(c: &mut Checks) -> Result<(), u32> {
    let doc = new_doc(c)?;
    for (kind, name) in [
        (1, "element"),
        (2, "attr"),
        (3, "text"),
        (4, "cdata"),
        (7, "pi"),
        (8, "comment"),
        (11, "docfrag"),
    ] {
        let n = Wide::new(name);
        let mut node: Obj = null_mut();
        let hr = call!(
            doc,
            CREATE_NODE,
            fn(Variant, Bstr, Bstr, *mut Obj) -> HResult,
            Variant::i4(kind),
            n.0,
            null_mut(),
            &mut node
        );
        c.hr(hr, S_OK)?;
        c.hr(call!(node, GET_DATA_TYPE, fn(*mut Variant) -> HResult, null_mut()), E_INVALIDARG)?;
        let (hr, v) = get_type(node);
        release(node);
        c.hr(hr, S_FALSE)?;
        c.eq(u64::from(v.vt), u64::from(VT_NULL))?;
    }
    let (hr, v) = get_type(doc);
    c.hr(hr, S_FALSE)?;
    c.eq(u64::from(v.vt), u64::from(VT_NULL))?;
    release(doc);
    Ok(())
}

/// test_xmlns_attribute: with xmlns:dt declared by an attribute node,
/// put_dataType adds only dt:dt.
fn xmlns_attribute(c: &mut Checks) -> Result<(), u32> {
    let doc = new_doc(c)?;
    let root = create(c, doc, CREATE_ELEMENT, "Testing")?;
    c.hr(call!(doc, APPEND_CHILD, fn(Obj, *mut Obj) -> HResult, root, null_mut()), S_OK)?;
    let a = create(c, doc, CREATE_ATTRIBUTE, "xmlns:dt")?;
    let ns = Wide::new(DT_NS);
    c.hr(call!(a, PUT_NODE_VALUE, fn(Variant) -> HResult, Variant::bstr(&ns)), S_OK)?;
    c.hr(call!(root, SET_ATTRIBUTE_NODE, fn(Obj, *mut Obj) -> HResult, a, null_mut()), S_OK)?;
    release(a);
    c.hr(put_type(root, Some("bin.base64")), S_OK)?;
    xml_is(
        c,
        root,
        "<Testing xmlns:dt=\"urn:schemas-microsoft-com:datatypes\" dt:dt=\"bin.base64\"/>",
    )?;
    release(root);
    release(doc);
    Ok(())
}

/// What neither Wine's tests nor the documentation settle: `info` lines.
/// What a put_dataType that MSXML 6 takes leaves: `info typed <case>`
/// lines with the node's dataType and typed value.
fn after_put(case: &str, node: Obj) {
    let (hr, mut v) = get_type(node);
    info(case, "dataType", hr as u32 as u64);
    if v.vt == VT_BSTR {
        info_text("dataType", v.value[0] as Bstr);
    }
    v.clear();
    let (hr, mut v) = get_typed(node);
    info(case, "nodeTypedValue", hr as u32 as u64);
    info(case, "nodeTypedValue vt", u64::from(v.vt));
    if v.vt == VT_BSTR {
        info_text("nodeTypedValue", v.value[0] as Bstr);
    }
    v.clear();
}

/// Typed content Wine's tests do not cover, as MSXML 6 answers on
/// Windows: i8 and ui8 read as their text, char as a VT_I4, an empty
/// int as VT_NULL; malformed content and an unknown type fail the load.
fn unsettled(c: &mut Checks) -> Result<(), u32> {
    let doc = new_doc(c)?;
    const X: &str = "<x xmlns:dt=\"urn:schemas-microsoft-com:datatypes\" dt:dt=";
    for (ty, xml, vt, text) in [
        ("i8", "\"i8\">5</x>", VT_BSTR, Some("5")),
        ("ui8", "\"ui8\">5</x>", VT_BSTR, Some("5")),
        ("char", "\"char\">u</x>", VT_I4, None),
        ("int", "\"int\"/>", VT_NULL, None),
    ] {
        c.eq(load_xml2(doc, X, xml).1 as u16 as u64, VARIANT_TRUE as u16 as u64)?;
        let (hr, root) = get_obj(doc, GET_DOCUMENT_ELEMENT);
        c.hr(hr, S_OK)?;
        let (hr, mut v) = get_typed(root);
        c.hr(hr, S_OK)?;
        c.eq(u64::from(v.vt), u64::from(vt))?;
        match text {
            Some(t) => c.var_text(&mut v, t)?,
            None if vt == VT_I4 => {
                info("char", "value", v.value[0] as u32 as u64);
                v.clear();
            }
            None => v.clear(),
        }
        let (hr, mut v) = get_type(root);
        c.hr(hr, S_OK)?;
        c.var_text(&mut v, ty)?;
        release(root);
    }
    for xml in ["\"int\">abc</x>", "\"nosuch\">x</x>"] {
        c.eq(load_xml2(doc, X, xml).1 as u16 as u64, VARIANT_FALSE as u16 as u64)?;
    }
    release(doc);
    Ok(())
}

/// loadXML of `head` and `tail` joined.
fn load_xml2(doc: Obj, head: &str, tail: &str) -> (HResult, i16) {
    let mut buf = [0u8; 160];
    let n = head.len() + tail.len();
    buf[..head.len()].copy_from_slice(head.as_bytes());
    buf[head.len()..n].copy_from_slice(tail.as_bytes());
    // SAFETY: two str's bytes, so UTF-8.
    load_xml(doc, unsafe { core::str::from_utf8_unchecked(&buf[..n]) })
}

/// `<name>: info typed <case> <what> <value>`.
fn info(case: &str, what: &str, value: u64) {
    Line::new()
        .s(NAME.as_bytes())
        .s(b": info typed ")
        .s(case.as_bytes())
        .s(b" ")
        .s(what.as_bytes())
        .s(b" ")
        .hex(value)
        .s(b"\r\n")
        .print();
}
