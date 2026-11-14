//! XSLTemplate60 (IXSLTemplate), its processor (IXSLProcessor) and
//! IXMLDOMNode's transformNode and transformNodeToObject: XSLT
//! (`xslt.rs`) as MSXML hands it to programs. Click-to-Run's App-V
//! pieces transform a package manifest before validating it (ROADMAP
//! OF). The behaviour is MSXML's documentation's and Wine's msxml3
//! tests run on Windows (domdoc.c's test_xsltemplate, test_xsltext,
//! test_transformNodeToObject):
//! - IXSLTemplate: putref_stylesheet(NULL) resets; the stylesheet must
//!   hold a stylesheet that compiles (else E_FAIL), in a free-threaded
//!   or a rental document (MSXML 6 takes both, measured on Windows;
//!   msxml3 refuses a rental one); the template holds a
//!   reference on it; get_stylesheet(NULL) is E_INVALIDARG;
//!   createProcessor(NULL) E_INVALIDARG, and the processor holds a
//!   reference on its template;
//! - IXSLProcessor: input (a document or node of ours; VT_EMPTY before
//!   one is set), output (VT_EMPTY or NULL: a string, read back from
//!   get_output as VT_BSTR; a document of ours, whose tree the result
//!   replaces; an IStream or ISequentialStream, written in the output's
//!   encoding: UTF-16 with a byte order mark, or UTF-8), addParameter
//!   (strings, numbers, booleans; VT_EMPTY or VT_NULL removes it),
//!   setStartMode, transform (NULL E_INVALIDARG);
//! - transformNode gives the result as a string; transformNodeToObject
//!   writes it to a document or stream (anything else E_INVALIDARG).
//!
//! A string output's declaration names UTF-16 (the string's
//! encoding). Each transform is recorded in the ledger ("other", with
//! where its output went), and a stylesheet or transform that fails
//! with its reason ("XSLT: ...", or "XSLT not supported: ..." for what
//! gala's XSLT lacks), so a program's run shows what it asked of XSLT.

use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, Ordering};

use super::*;
use crate::xpath::Value;
use crate::xslt::{self, Sheet};

const IID_IXSLPROCESSOR: Guid = dom_iid(0x2933_bf92);
const IID_IXSLTEMPLATE: Guid = dom_iid(0x2933_bf93);
const IID_ISTREAM: Guid = guid(0x0000_000c, 0, 0, [0xc0, 0, 0, 0, 0, 0, 0, 0x46]);
const IID_ISEQUENTIALSTREAM: Guid =
    guid(0x0c73_3a30, 0x2a1c, 0x11ce, [0xad, 0xe5, 0x00, 0xaa, 0x00, 0x44, 0x77, 0x3d]);
const VT_R4: u16 = 4;
const VT_R8: u16 = 5;

// ---- Calls on other objects (a program's stream) ----

/// Calls IUnknown's slot `slot` (AddRef 1, Release 2) of `p`.
fn unknown_call(p: Obj, slot: usize) -> u32 {
    // SAFETY: a live COM object: its vtable starts with IUnknown's.
    unsafe {
        let vtable = *(p as *const *const usize);
        let f: extern "system" fn(Obj) -> u32 = core::mem::transmute(*vtable.add(slot));
        f(p)
    }
}

fn add_ref(p: Obj) {
    if !p.is_null() {
        unknown_call(p, 1);
    }
}

fn release(p: Obj) {
    if !p.is_null() {
        unknown_call(p, 2);
    }
}

/// QueryInterface on `p` (NULL when it does not answer).
fn query(p: Obj, iid: &Guid) -> Obj {
    let mut out: Obj = core::ptr::null_mut();
    // SAFETY: a live COM object: slot 0 is QueryInterface.
    unsafe {
        let vtable = *(p as *const *const usize);
        let f: extern "system" fn(Obj, *const Guid, *mut Obj) -> HResult =
            core::mem::transmute(*vtable.add(0));
        if f(p, iid, &mut out) < 0 {
            return core::ptr::null_mut();
        }
    }
    out
}

/// ISequentialStream::Write (slot 4, IStream's too) of all of `bytes`.
fn stream_write(p: Obj, bytes: &[u8]) -> HResult {
    let mut written = 0u32;
    // SAFETY: an ISequentialStream (or IStream) we hold a reference on;
    // our buffer.
    let hr = unsafe {
        let vtable = *(p as *const *const usize);
        let f: extern "system" fn(Obj, *const u8, u32, *mut u32) -> HResult =
            core::mem::transmute(*vtable.add(4));
        f(p, bytes.as_ptr(), bytes.len() as u32, &mut written)
    };
    if hr >= 0 && written as usize != bytes.len() { E_FAIL } else { hr }
}

/// Whether `p` is one of our documents (not any node).
fn our_document(p: Obj) -> bool {
    !p.is_null()
        // SAFETY: an interface pointer: its first field is a vtable
        // pointer.
        && unsafe { *(p as *const *const ()) } == &DOC_VTABLE as *const VTable<84> as *const ()
}

/// Where a transform's output goes.
enum Dest {
    /// A string (get_output's VT_BSTR, transformNode's result).
    Text,
    /// One of our documents (a reference held): its tree is replaced.
    Doc(Obj),
    /// A program's stream (a reference held).
    Stream(Obj),
}

impl Dest {
    /// The destination a VARIANT names: VT_EMPTY or NULL a string (when
    /// `text_ok`), a document of ours, or a stream; None for anything
    /// else (recorded).
    fn from(v: &Variant, text_ok: bool) -> Option<Dest> {
        match v.vt {
            VT_EMPTY if text_ok => Some(Dest::Text),
            VT_UNKNOWN | VT_DISPATCH => {
                let p = v.value[0] as Obj;
                if p.is_null() {
                    return text_ok.then_some(Dest::Text);
                }
                if our_document(p) {
                    add_ref(p);
                    return Some(Dest::Doc(p));
                }
                let s = query(p, &IID_ISTREAM);
                let s = if s.is_null() { query(p, &IID_ISEQUENTIALSTREAM) } else { s };
                if s.is_null() {
                    record(
                        b"XSLT output(an object neither ours nor a stream)",
                        LEDGER_UNSUPPORTED,
                        0,
                    );
                    return None;
                }
                Some(Dest::Stream(s))
            }
            other => {
                record(b"XSLT output(variant type)", LEDGER_UNSUPPORTED, other.into());
                None
            }
        }
    }

    fn release(&self) {
        match *self {
            Dest::Doc(p) | Dest::Stream(p) => release(p),
            Dest::Text => {}
        }
    }

    fn name(&self) -> &'static [u8] {
        match self {
            Dest::Text => b"XSLT transform (output: a string)",
            Dest::Doc(_) => b"XSLT transform (output: a document)",
            Dest::Stream(_) => b"XSLT transform (output: a stream)",
        }
    }
}

/// Records a stylesheet's or transform's failure.
fn record_failure(f: &xslt::Failure) {
    let mut what = String::from(if f.unsupported { "XSLT not supported: " } else { "XSLT: " });
    what.push_str(&f.what);
    record(what.as_bytes(), if f.unsupported { LEDGER_UNSUPPORTED } else { LEDGER_OTHER }, 0);
}

/// The stylesheet compiled from node object `n` (E_FAIL, recorded, when
/// it does not compile).
fn compile_node(n: &NodeObj) -> Result<Sheet, HResult> {
    // SAFETY: a node object holds a reference on its document.
    let tree = unsafe { &(*n.doc).tree };
    xslt::compile(tree, n.idx).map_err(|f| {
        record_failure(&f);
        E_FAIL
    })
}

/// Runs `sheet` over node object `input` and delivers the result to
/// `dest`; a string result is returned for Dest::Text.
fn run(
    sheet: &Sheet,
    input: &NodeObj,
    params: &[(String, Value)],
    mode: Option<&str>,
    dest: &Dest,
) -> Result<String, HResult> {
    record(dest.name(), LEDGER_OTHER, params.len() as u64);
    // SAFETY: a node object holds a reference on its document.
    let src = unsafe { &(*input.doc).tree };
    let r = xslt::transform(sheet, src, input.idx, params, mode).map_err(|f| {
        record_failure(&f);
        E_FAIL
    })?;
    match *dest {
        Dest::Text => Ok(xslt::serialize(sheet, &r, "UTF-16")),
        Dest::Doc(p) => {
            let d = doc(p);
            d.tree = r.tree;
            d.error = ParseError::default();
            d.ready = 4;
            Ok(String::new())
        }
        Dest::Stream(p) => {
            let encoding = sheet.output.encoding.as_deref().unwrap_or("UTF-16");
            let text = xslt::serialize(sheet, &r, encoding);
            let lower = encoding.to_ascii_lowercase();
            let wide = lower.starts_with("utf-16") || lower == "ucs-2" || lower == "unicode";
            let mut bytes: Vec<u8> = Vec::new();
            if wide {
                if bytes.try_reserve(2 + 2 * text.len()).is_err() {
                    return Err(E_OUTOFMEMORY);
                }
                bytes.extend([0xff, 0xfe]);
                bytes.extend(text.encode_utf16().flat_map(u16::to_le_bytes));
            } else {
                if !(lower == "utf-8" || text.is_ascii()) {
                    record(b"XSLT output encoding (written as UTF-8)", LEDGER_UNSUPPORTED, 0);
                }
                bytes = text.into_bytes();
            }
            let hr = stream_write(p, &bytes);
            if hr < 0 { Err(hr) } else { Ok(String::new()) }
        }
    }
}

// ---- IXMLDOMNode::transformNode, transformNodeToObject ----

/// The stylesheet argument: a node of ours, compiled.
fn stylesheet_arg(stylesheet: Obj) -> Result<Sheet, HResult> {
    match mutate::ours(stylesheet) {
        Some(n) => compile_node(n),
        None => Err(E_INVALIDARG),
    }
}

pub(super) extern "system" fn transform_node(
    this: Obj,
    stylesheet: Obj,
    out: *mut Bstr,
) -> HResult {
    if out.is_null() || stylesheet.is_null() {
        return E_INVALIDARG;
    }
    let sheet = match stylesheet_arg(stylesheet) {
        Ok(s) => s,
        Err(hr) => return hr,
    };
    match run(&sheet, node(this), &[], None, &Dest::Text) {
        Ok(s) => give_str(out, Some(&s)),
        Err(hr) => hr,
    }
}

pub(super) extern "system" fn transform_node_to_object(
    this: Obj,
    stylesheet: Obj,
    out: Variant,
) -> HResult {
    if stylesheet.is_null() {
        return E_INVALIDARG;
    }
    let Some(dest) = Dest::from(&out, false) else { return E_INVALIDARG };
    let r = stylesheet_arg(stylesheet).and_then(|s| run(&s, node(this), &[], None, &dest));
    dest.release();
    match r {
        Ok(_) => S_OK,
        Err(hr) => hr,
    }
}

// ---- XSLTemplate60 (IXSLTemplate) ----

#[repr(C)]
struct TemplateObj {
    vtable: *const (),
    refs: AtomicU32,
    /// The stylesheet set (a reference held) and its compiled form.
    stylesheet: Obj,
    sheet: Option<Arc<Sheet>>,
}

fn template<'a>(this: Obj) -> &'a mut TemplateObj {
    // SAFETY: our template objects' interface pointers; one thread at a
    // time (as the rental document).
    unsafe { &mut *(this as *mut TemplateObj) }
}

/// Makes an XSLTemplate60 (the class factory's CreateInstance).
pub fn new_template(riid: *const Guid, out: *mut Obj) -> HResult {
    let t = TemplateObj {
        vtable: &TEMPLATE_VTABLE as *const VTable<10> as *const (),
        refs: AtomicU32::new(1),
        stylesheet: core::ptr::null_mut(),
        sheet: None,
    };
    let Some(p) = try_box(t) else { return E_OUTOFMEMORY };
    let hr = template_query(p as Obj, riid, out);
    template_release(p as Obj);
    hr
}

extern "system" fn template_query(this: Obj, riid: *const Guid, out: *mut Obj) -> HResult {
    if out.is_null() || riid.is_null() {
        return E_POINTER;
    }
    // SAFETY: the caller's IID.
    let iid = unsafe { *riid };
    if iid == IID_IUNKNOWN || iid == IID_IDISPATCH || iid == IID_IXSLTEMPLATE {
        template(this).refs.fetch_add(1, Ordering::Relaxed);
        put(out, this);
        return S_OK;
    }
    put(out, core::ptr::null_mut());
    crate::record_iid("QI? XSLTemplate", &iid);
    E_NOINTERFACE
}

extern "system" fn template_add_ref(this: Obj) -> u32 {
    template(this).refs.fetch_add(1, Ordering::Relaxed) + 1
}

extern "system" fn template_release(this: Obj) -> u32 {
    let t = template(this);
    let left = t.refs.fetch_sub(1, Ordering::AcqRel) - 1;
    if left == 0 {
        release(t.stylesheet);
        // SAFETY: the object's last reference.
        unsafe { free_box(this as *mut TemplateObj) };
    }
    left
}

extern "system" fn putref_stylesheet(this: Obj, node_: Obj) -> HResult {
    let t = template(this);
    if node_.is_null() {
        release(t.stylesheet);
        t.stylesheet = core::ptr::null_mut();
        t.sheet = None;
        return S_OK;
    }
    let Some(n) = mutate::ours(node_) else { return E_INVALIDARG };
    let sheet = match compile_node(n) {
        Ok(s) => s,
        Err(hr) => return hr,
    };
    let sheet = Arc::new(sheet);
    add_ref(node_);
    release(t.stylesheet);
    t.stylesheet = node_;
    t.sheet = Some(sheet);
    S_OK
}

extern "system" fn template_get_stylesheet(this: Obj, out: *mut Obj) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    let s = template(this).stylesheet;
    add_ref(s);
    put(out, s);
    S_OK
}

extern "system" fn create_processor(this: Obj, out: *mut Obj) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    put(out, core::ptr::null_mut());
    let Some(sheet) = template(this).sheet.clone() else {
        record(b"XSLTemplate60::createProcessor(no stylesheet)", LEDGER_OTHER, 0);
        return E_FAIL;
    };
    let p = ProcObj {
        vtable: &PROCESSOR_VTABLE as *const VTable<21> as *const (),
        refs: AtomicU32::new(1),
        template: this,
        sheet,
        input: core::ptr::null_mut(),
        dest: Dest::Text,
        params: Vec::new(),
        result: String::new(),
        mode: String::new(),
    };
    let Some(p) = try_box(p) else { return E_OUTOFMEMORY };
    template_add_ref(this);
    put(out, p as Obj);
    S_OK
}

static TEMPLATE_VTABLE: VTable<10> = VTable([
    template_query as *const (),
    template_add_ref as *const (),
    template_release as *const (),
    get_type_info_count as *const (),
    get_type_info as *const (),
    get_ids_of_names as *const (),
    invoke as *const (),
    putref_stylesheet as *const (),
    template_get_stylesheet as *const (),
    create_processor as *const (),
]);

// ---- IXSLProcessor ----

#[repr(C)]
struct ProcObj {
    vtable: *const (),
    refs: AtomicU32,
    /// The template (a reference held) and its stylesheet as compiled
    /// when the processor was made.
    template: Obj,
    sheet: Arc<Sheet>,
    /// The input node (a reference held), or NULL.
    input: Obj,
    dest: Dest,
    params: Vec<(String, Value)>,
    /// The last transform's string output.
    result: String,
    mode: String,
}

fn proc<'a>(this: Obj) -> &'a mut ProcObj {
    // SAFETY: our processor objects' interface pointers; one thread at a
    // time.
    unsafe { &mut *(this as *mut ProcObj) }
}

extern "system" fn proc_query(this: Obj, riid: *const Guid, out: *mut Obj) -> HResult {
    if out.is_null() || riid.is_null() {
        return E_POINTER;
    }
    // SAFETY: the caller's IID.
    let iid = unsafe { *riid };
    if iid == IID_IUNKNOWN || iid == IID_IDISPATCH || iid == IID_IXSLPROCESSOR {
        proc(this).refs.fetch_add(1, Ordering::Relaxed);
        put(out, this);
        return S_OK;
    }
    put(out, core::ptr::null_mut());
    crate::record_iid("QI? XSLProcessor", &iid);
    E_NOINTERFACE
}

extern "system" fn proc_add_ref(this: Obj) -> u32 {
    proc(this).refs.fetch_add(1, Ordering::Relaxed) + 1
}

extern "system" fn proc_release(this: Obj) -> u32 {
    let p = proc(this);
    let left = p.refs.fetch_sub(1, Ordering::AcqRel) - 1;
    if left == 0 {
        release(p.input);
        p.dest.release();
        let t = p.template;
        // SAFETY: the object's last reference.
        unsafe { free_box(this as *mut ProcObj) };
        template_release(t);
    }
    left
}

extern "system" fn put_input(this: Obj, v: Variant) -> HResult {
    let p = proc(this);
    let n = match v.vt {
        VT_UNKNOWN | VT_DISPATCH => v.value[0] as Obj,
        other => {
            record(b"IXSLProcessor::put_input(variant type)", LEDGER_UNSUPPORTED, other.into());
            return E_INVALIDARG;
        }
    };
    if !n.is_null() && mutate::ours(n).is_none() {
        return E_INVALIDARG;
    }
    add_ref(n);
    release(p.input);
    p.input = n;
    S_OK
}

extern "system" fn get_input(this: Obj, out: *mut Variant) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    let n = proc(this).input;
    let v = if n.is_null() {
        Variant { vt: VT_EMPTY, reserved: [0; 3], value: [0; 2] }
    } else {
        add_ref(n);
        Variant { vt: VT_UNKNOWN, reserved: [0; 3], value: [n as usize, 0] }
    };
    put(out, v);
    S_OK
}

extern "system" fn get_owner_template(this: Obj, out: *mut Obj) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    let t = proc(this).template;
    template_add_ref(t);
    put(out, t);
    S_OK
}

extern "system" fn set_start_mode(this: Obj, mode: Bstr, uri: Bstr) -> HResult {
    if text(uri).is_some_and(|u| !u.is_empty()) {
        record(b"IXSLProcessor::setStartMode(a namespace)", LEDGER_UNSUPPORTED, 0);
        return E_INVALIDARG;
    }
    proc(this).mode = text(mode).unwrap_or_default();
    S_OK
}

extern "system" fn get_start_mode(this: Obj, out: *mut Bstr) -> HResult {
    give_str(out, Some(&proc(this).mode))
}

extern "system" fn get_start_mode_uri(_this: Obj, out: *mut Bstr) -> HResult {
    give_str(out, Some(""))
}

extern "system" fn put_output(this: Obj, v: Variant) -> HResult {
    let Some(dest) = Dest::from(&v, true) else { return E_INVALIDARG };
    let p = proc(this);
    p.dest.release();
    p.dest = dest;
    S_OK
}

extern "system" fn get_output(this: Obj, out: *mut Variant) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    let p = proc(this);
    match p.dest {
        Dest::Doc(o) | Dest::Stream(o) => {
            add_ref(o);
            put(out, Variant { vt: VT_UNKNOWN, reserved: [0; 3], value: [o as usize, 0] });
            S_OK
        }
        Dest::Text => variant_str(out, &p.result),
    }
}

extern "system" fn transform(this: Obj, done: *mut i16) -> HResult {
    if done.is_null() {
        return E_INVALIDARG;
    }
    put(done, VARIANT_FALSE);
    let p = proc(this);
    let Some(input) = (!p.input.is_null()).then(|| node(p.input)) else {
        record(b"IXSLProcessor::transform(no input)", LEDGER_OTHER, 0);
        return E_FAIL;
    };
    let mode = (!p.mode.is_empty()).then_some(p.mode.as_str());
    match run(&p.sheet, input, &p.params, mode, &p.dest) {
        Ok(s) => {
            p.result = s;
            put(done, VARIANT_TRUE);
            S_OK
        }
        Err(hr) => hr,
    }
}

extern "system" fn reset(this: Obj) -> HResult {
    proc(this).result = String::new();
    S_OK
}

extern "system" fn add_parameter(this: Obj, name: Bstr, v: Variant, uri: Bstr) -> HResult {
    let Some(name) = text(name) else { return E_INVALIDARG };
    if text(uri).is_some_and(|u| !u.is_empty()) {
        record(b"IXSLProcessor::addParameter(a namespace)", LEDGER_UNSUPPORTED, 0);
        return E_INVALIDARG;
    }
    let (vt, byref) = (v.vt & !VT_BYREF, v.vt & VT_BYREF != 0);
    // A VT_BYREF variant points at its value.
    let word = |size: usize| -> u64 {
        if byref {
            // SAFETY: a VT_BYREF variant's pointer to a value of its type.
            unsafe {
                match size {
                    2 => u64::from(*(v.value[0] as *const u16)),
                    4 => u64::from(*(v.value[0] as *const u32)),
                    _ => *(v.value[0] as *const u64),
                }
            }
        } else {
            match size {
                2 => v.value[0] as u64 & 0xffff,
                4 => v.value[0] as u64 & 0xffff_ffff,
                // SAFETY: a VARIANT's 8 value bytes (the first two words on x86).
                _ => unsafe { *(v.value.as_ptr() as *const u64) },
            }
        }
    };
    let value = match vt {
        VT_EMPTY | VT_NULL => None,
        VT_BSTR => {
            let s = if byref {
                // SAFETY: a VT_BSTR | VT_BYREF variant points at a BSTR.
                unsafe { *(v.value[0] as *const *const u16) }
            } else {
                v.value[0] as *const u16
            };
            Some(Value::Str(text(s).unwrap_or_default()))
        }
        VT_BOOL => Some(Value::Bool(word(2) != 0)),
        VT_I2 => Some(Value::Num(f64::from(word(2) as u16 as i16))),
        VT_I4 | VT_INT => Some(Value::Num(f64::from(word(4) as u32 as i32))),
        VT_UI4 => Some(Value::Num(f64::from(word(4) as u32))),
        VT_R4 => Some(Value::Num(f64::from(f32::from_bits(word(4) as u32)))),
        VT_R8 => Some(Value::Num(f64::from_bits(word(8)))),
        other => {
            record(b"IXSLProcessor::addParameter(value type)", LEDGER_UNSUPPORTED, other.into());
            return E_INVALIDARG;
        }
    };
    let p = proc(this);
    p.params.retain(|(n, _)| *n != name);
    if let Some(value) = value {
        p.params.push((name, value));
    }
    S_OK
}

extern "system" fn proc_get_stylesheet(this: Obj, out: *mut Obj) -> HResult {
    template_get_stylesheet(proc(this).template, out)
}

extern "system" fn get_ready_state(_this: Obj, _out: *mut i32) -> HResult {
    not_implemented(b"IXSLProcessor::get_readyState")
}

extern "system" fn add_object(_this: Obj, _obj: Obj, _uri: Bstr) -> HResult {
    not_implemented(b"IXSLProcessor::addObject")
}

static PROCESSOR_VTABLE: VTable<21> = VTable([
    proc_query as *const (),
    proc_add_ref as *const (),
    proc_release as *const (),
    get_type_info_count as *const (),
    get_type_info as *const (),
    get_ids_of_names as *const (),
    invoke as *const (),
    put_input as *const (),
    get_input as *const (),
    get_owner_template as *const (),
    set_start_mode as *const (),
    get_start_mode as *const (),
    get_start_mode_uri as *const (),
    put_output as *const (),
    get_output as *const (),
    transform as *const (),
    reset as *const (),
    get_ready_state as *const (),
    add_parameter as *const (),
    add_object as *const (),
    proc_get_stylesheet as *const (),
]);
