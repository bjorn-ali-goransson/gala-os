//! The imaging factory (IWICImagingFactory, IWICImagingFactory2), the
//! DLL's class objects and its two exported helpers.
//!
//! - One static factory serves the process (its references are not
//!   counted). WICCreateImagingFactory_Proxy gives it for
//!   WINCODEC_SDK_VERSION1 0x0236 and ...2 0x0237; version 0 is
//!   WINCODEC_ERR_UNSUPPORTEDVERSION (Windows); another version is
//!   served and recorded (what Windows answers is not measured).
//! - DllGetClassObject serves CLSID_WICImagingFactory (1 and 2), the
//!   PNG decoder (CLSID_WICPngDecoder1 and 2), the BMP decoder and
//!   CLSID_WICDefaultFormatConverter; another class is recorded,
//!   CLASS_E_CLASSNOTAVAILABLE.
//! - CreateDecoderFromStream, ...FromFilename and ...FromFileHandle read
//!   the image whole and pick the decoder by its first bytes; an image
//!   gala has no decoder for is WINCODEC_ERR_COMPONENTNOTFOUND, recorded
//!   with what it looks like (JPEG, GIF, TIFF, ICO, DDS, WebP, or
//!   unknown). CreateDecoder(NULL container) is E_INVALIDARG; another
//!   container than PNG or BMP is WINCODEC_ERR_COMPONENTNOTFOUND
//!   (recorded).
//! - WICConvertBitmapSource(format, source): the source itself (another
//!   reference) when it is in that format, else a format converter over
//!   it; an unknown destination format is WINCODEC_ERR_COMPONENTNOTFOUND
//!   (Wine's converter test, run on Windows).
//! - Not implemented (each recorded, E_NOTIMPL): component information
//!   and enumeration, encoders (CreateEncoder,
//!   IWICImagingFactory2::CreateImageEncoder), colour contexts and
//!   transforms, flip-rotators, bitmaps from HBITMAP and HICON, fast
//!   metadata encoders, query writers.

use core::ffi::c_void;

use crate::decoder::{CONTAINER_BMP, CONTAINER_PNG, Container};
use crate::transform::Kinds;
use crate::{
    CLASS_E_CLASSNOTAVAILABLE, CLASS_E_NOAGGREGATION, E_INVALIDARG, E_OUTOFMEMORY, E_POINTER, Guid,
    HResult, Header, Obj, S_FALSE, S_OK, Source, VTable, WINCODEC_ERR_COMPONENTNOTFOUND,
    WINCODEC_ERR_UNSUPPORTEDVERSION, bitmap, data1, decoder, format, gap, guid, not_implemented,
    put, read_guid, release_object, stream, transform,
};

const IID_ICLASSFACTORY: Guid = guid(1, 0, 0, [0xc0, 0, 0, 0, 0, 0, 0, 0x46]);
const IID_IWICIMAGINGFACTORY: Guid =
    guid(0xec5e_c8a9, 0xc395, 0x4314, [0x9c, 0x77, 0x54, 0xd7, 0xa9, 0x35, 0xff, 0x70]);
const IID_IWICIMAGINGFACTORY2: Guid =
    guid(0x7b81_6b45, 0x1996, 0x4476, [0xb1, 0x32, 0xde, 0x9e, 0x24, 0x7c, 0x8a, 0xf0]);
const CLSID_WICIMAGINGFACTORY1: Guid =
    guid(0xcaca_f262, 0x9370, 0x4615, [0xa1, 0x3b, 0x9f, 0x55, 0x39, 0xda, 0x4c, 0x0a]);
const CLSID_WICIMAGINGFACTORY2: Guid =
    guid(0x317d_06e8, 0x5f24, 0x433d, [0xbd, 0xf7, 0x79, 0xce, 0x68, 0xd8, 0xab, 0xc2]);
const CLSID_WICPNGDECODER1: Guid =
    guid(0x389e_a17b, 0x5078, 0x4cde, [0xb6, 0xef, 0x25, 0xc1, 0x51, 0x75, 0xc7, 0x51]);
const CLSID_WICPNGDECODER2: Guid =
    guid(0xe018_945b, 0xaa86, 0x4008, [0x9b, 0xd4, 0x67, 0x77, 0xa1, 0xe4, 0x0c, 0x11]);
const CLSID_WICBMPDECODER: Guid =
    guid(0x6b46_2062, 0x7cbf, 0x400d, [0x9f, 0xdb, 0x81, 0x3d, 0xd1, 0x0f, 0x27, 0x78]);
const CLSID_WICDEFAULTFORMATCONVERTER: Guid =
    guid(0x1a3f_11dc, 0xb514, 0x4b17, [0x8c, 0x5f, 0x21, 0x54, 0x51, 0x38, 0x52, 0xf1]);
const SDK_VERSION1: u32 = 0x0236;
const SDK_VERSION2: u32 = 0x0237;

/// The factory: static, its references not counted.
static FACTORY: Header = Header::new(FACTORY_VTBL.addr());

fn factory() -> Obj {
    (&raw const FACTORY) as Obj
}

static FACTORY_VTBL: VTable<29> = VTable([
    factory_query as *const (),
    static_add_ref as *const (),
    static_release as *const (),
    create_decoder_from_filename as *const (),
    create_decoder_from_stream as *const (),
    create_decoder_from_file_handle as *const (),
    create_component_info as *const (),
    create_decoder as *const (),
    create_encoder as *const (),
    create_palette as *const (),
    create_format_converter as *const (),
    create_bitmap_scaler as *const (),
    create_bitmap_clipper as *const (),
    create_bitmap_flip_rotator as *const (),
    create_stream as *const (),
    create_color_context as *const (),
    create_color_transformer as *const (),
    create_bitmap as *const (),
    create_bitmap_from_source as *const (),
    create_bitmap_from_source_rect as *const (),
    create_bitmap_from_memory as *const (),
    create_bitmap_from_hbitmap as *const (),
    create_bitmap_from_hicon as *const (),
    create_component_enumerator as *const (),
    create_fast_metadata_encoder_from_decoder as *const (),
    create_fast_metadata_encoder_from_frame_decode as *const (),
    create_query_writer as *const (),
    create_query_writer_from_reader as *const (),
    create_image_encoder as *const (),
]);

extern "system" fn static_add_ref(_this: Obj) -> u32 {
    2
}

extern "system" fn static_release(_this: Obj) -> u32 {
    1
}

/// QueryInterface of the static objects (no reference taken).
fn static_query(this: Obj, iids: &[Guid], riid: *const Guid, out: *mut Obj) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    match read_guid(riid) {
        Some(iid) if iid == crate::IID_IUNKNOWN || iids.contains(&iid) => {
            put(out, this);
            S_OK
        }
        Some(_) => {
            put(out, core::ptr::null_mut());
            crate::E_NOINTERFACE
        }
        None => {
            put(out, core::ptr::null_mut());
            E_INVALIDARG
        }
    }
}

extern "system" fn factory_query(this: Obj, riid: *const Guid, out: *mut Obj) -> HResult {
    let r = static_query(this, &[IID_IWICIMAGINGFACTORY, IID_IWICIMAGINGFACTORY2], riid, out);
    if r == crate::E_NOINTERFACE {
        let d1 = read_guid(riid).map_or(0, |g| data1(&g));
        gap(b"IWICImagingFactory::QueryInterface(interface)", d1);
    }
    r
}

/// What data starting with `d` looks like, for the ledger (gala has no
/// decoder for it).
fn other_format(d: &[u8]) -> &'static [u8] {
    if d.starts_with(&[0xff, 0xd8, 0xff]) {
        b"CreateDecoder(JPEG)"
    } else if d.starts_with(b"GIF8") {
        b"CreateDecoder(GIF)"
    } else if d.starts_with(b"II*\0") || d.starts_with(b"MM\0*") {
        b"CreateDecoder(TIFF)"
    } else if d.starts_with(&[0, 0, 1, 0]) || d.starts_with(&[0, 0, 2, 0]) {
        b"CreateDecoder(ICO or CUR)"
    } else if d.starts_with(b"DDS ") {
        b"CreateDecoder(DDS)"
    } else if d.starts_with(b"RIFF") && d.get(8..12) == Some(b"WEBP") {
        b"CreateDecoder(WebP)"
    } else {
        b"CreateDecoder(unknown format)"
    }
}

/// A decoder initialized from the image `data`.
fn decoder_for(data: &[u8], out: *mut Obj) -> HResult {
    let Some(c) = decoder::container_of(data) else {
        let head =
            data.get(..4).map_or(0, |b| u64::from(u32::from_be_bytes([b[0], b[1], b[2], b[3]])));
        gap(other_format(data), head);
        return WINCODEC_ERR_COMPONENTNOTFOUND;
    };
    let Some(d) = decoder::new_decoder(c) else { return E_OUTOFMEMORY };
    match decoder::initialize_with(d, data) {
        S_OK => {
            put(out, d);
            S_OK
        }
        e => {
            release_object::<decoder::Decoder>(d);
            e
        }
    }
}

extern "system" fn create_decoder_from_stream(
    _this: Obj,
    stream_: Obj,
    _vendor: *const Guid,
    _options: u32,
    out: *mut Obj,
) -> HResult {
    if stream_.is_null() || out.is_null() {
        return E_INVALIDARG;
    }
    put(out, core::ptr::null_mut());
    match stream::read_all(stream_) {
        Ok(data) => decoder_for(&data, out),
        Err(e) => e,
    }
}

extern "system" fn create_decoder_from_filename(
    _this: Obj,
    path: *const u16,
    _vendor: *const Guid,
    access: u32,
    _options: u32,
    out: *mut Obj,
) -> HResult {
    if path.is_null() || out.is_null() {
        return E_INVALIDARG;
    }
    put(out, core::ptr::null_mut());
    if access & !0x8000_0000 != 0 {
        gap(b"CreateDecoderFromFilename(access)", access.into());
    }
    match stream::read_file(path) {
        Ok(data) => decoder_for(&data, out),
        Err(e) => e,
    }
}

extern "system" fn create_decoder_from_file_handle(
    _this: Obj,
    file: usize,
    _vendor: *const Guid,
    _options: u32,
    out: *mut Obj,
) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    put(out, core::ptr::null_mut());
    match stream::read_handle(file) {
        Ok(data) => decoder_for(&data, out),
        Err(e) => e,
    }
}

extern "system" fn create_decoder(
    _this: Obj,
    container: *const Guid,
    _vendor: *const Guid,
    out: *mut Obj,
) -> HResult {
    let Some(c) = read_guid(container) else { return E_INVALIDARG };
    if out.is_null() {
        return E_INVALIDARG;
    }
    let kind = match c {
        CONTAINER_PNG => Container::Png,
        CONTAINER_BMP => Container::Bmp,
        _ => {
            gap(b"CreateDecoder(container)", data1(&c));
            return WINCODEC_ERR_COMPONENTNOTFOUND;
        }
    };
    match decoder::new_decoder(kind) {
        Some(d) => {
            put(out, d);
            S_OK
        }
        None => E_OUTOFMEMORY,
    }
}

extern "system" fn create_palette(_this: Obj, out: *mut Obj) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    match crate::palette::new_palette() {
        Some(p) => {
            put(out, p);
            S_OK
        }
        None => E_OUTOFMEMORY,
    }
}

extern "system" fn create_format_converter(_this: Obj, out: *mut Obj) -> HResult {
    transform::create(Kinds::Converter, out)
}

extern "system" fn create_bitmap_scaler(_this: Obj, out: *mut Obj) -> HResult {
    transform::create(Kinds::Scaler, out)
}

extern "system" fn create_bitmap_clipper(_this: Obj, out: *mut Obj) -> HResult {
    transform::create(Kinds::Clipper, out)
}

extern "system" fn create_stream(_this: Obj, out: *mut Obj) -> HResult {
    stream::create(out)
}

extern "system" fn create_bitmap(
    _this: Obj,
    w: u32,
    h: u32,
    format: *const Guid,
    _option: u32,
    out: *mut Obj,
) -> HResult {
    bitmap::create(w, h, format, out)
}

extern "system" fn create_bitmap_from_source(
    _this: Obj,
    source: Obj,
    option: u32,
    out: *mut Obj,
) -> HResult {
    bitmap::create_from_source(source, option, None, out)
}

extern "system" fn create_bitmap_from_source_rect(
    _this: Obj,
    source: Obj,
    x: u32,
    y: u32,
    w: u32,
    h: u32,
    out: *mut Obj,
) -> HResult {
    bitmap::create_from_source(source, 2, Some((x, y, w, h)), out)
}

#[allow(clippy::too_many_arguments)]
extern "system" fn create_bitmap_from_memory(
    _this: Obj,
    w: u32,
    h: u32,
    format: *const Guid,
    stride: u32,
    size: u32,
    buffer: *const u8,
    out: *mut Obj,
) -> HResult {
    bitmap::create_from_memory(w, h, format, stride, size, buffer, out)
}

/// The factory methods not implemented yet (they all return an HRESULT;
/// the caller's arguments are left alone, outputs included); each records
/// its own name.
macro_rules! factory_not_impl {
    ($($name:ident $method:literal)*) => {
        $(
            extern "system" fn $name(_this: Obj) -> HResult {
                not_implemented(concat!("IWICImagingFactory::", $method).as_bytes())
            }
        )*
    };
}

factory_not_impl! {
    create_component_info "CreateComponentInfo"
    create_encoder "CreateEncoder"
    create_bitmap_flip_rotator "CreateBitmapFlipRotator"
    create_color_context "CreateColorContext"
    create_color_transformer "CreateColorTransformer"
    create_bitmap_from_hbitmap "CreateBitmapFromHBITMAP"
    create_bitmap_from_hicon "CreateBitmapFromHICON"
    create_component_enumerator "CreateComponentEnumerator"
    create_fast_metadata_encoder_from_decoder "CreateFastMetadataEncoderFromDecoder"
    create_fast_metadata_encoder_from_frame_decode "CreateFastMetadataEncoderFromFrameDecode"
    create_query_writer "CreateQueryWriter"
    create_query_writer_from_reader "CreateQueryWriterFromReader"
    create_image_encoder "CreateImageEncoder"
}

/// WICCreateImagingFactory_Proxy(SDKVersion, &factory): the process's
/// factory.
#[unsafe(no_mangle)]
pub extern "system" fn WICCreateImagingFactory_Proxy(version: u32, out: *mut Obj) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    if version == 0 {
        return WINCODEC_ERR_UNSUPPORTEDVERSION;
    }
    if version != SDK_VERSION1 && version != SDK_VERSION2 {
        gap(b"WICCreateImagingFactory_Proxy(SDK version)", version.into());
    }
    put(out, factory());
    S_OK
}

/// WICConvertBitmapSource(format, source, &result).
#[unsafe(no_mangle)]
pub extern "system" fn WICConvertBitmapSource(
    to: *const Guid,
    source: Obj,
    out: *mut Obj,
) -> HResult {
    if out.is_null() || source.is_null() {
        return E_INVALIDARG;
    }
    put(out, core::ptr::null_mut());
    let Some(t) = read_guid(to) else { return E_INVALIDARG };
    if format::find(&t).is_none() {
        gap(b"WICConvertBitmapSource(format)", data1(&t));
        return WINCODEC_ERR_COMPONENTNOTFOUND;
    }
    let src = Source(source);
    if src.format() == Ok(t) {
        src.add_ref();
        put(out, source);
        return S_OK;
    }
    let mut c: Obj = core::ptr::null_mut();
    let r = transform::create(Kinds::Converter, &mut c);
    if r != S_OK {
        return r;
    }
    match transform::converter_init(c, source, to) {
        S_OK => {
            put(out, c);
            S_OK
        }
        e => {
            release_object::<transform::Wrapper>(c);
            e
        }
    }
}

// ---- The class objects ----

/// A class object: its vtable and what it makes.
#[repr(C)]
struct ClassObject {
    vtbl: *const (),
    makes: Class,
}
// SAFETY: immutable statics.
unsafe impl Sync for ClassObject {}

#[derive(Clone, Copy)]
enum Class {
    Factory,
    Decoder(Container),
    Converter,
}

static CLASS_VTBL: VTable<5> = VTable([
    class_query as *const (),
    static_add_ref as *const (),
    static_release as *const (),
    class_create as *const (),
    class_lock as *const (),
]);

static FACTORY_CLASS: ClassObject = ClassObject { vtbl: CLASS_VTBL.addr(), makes: Class::Factory };
static PNG_CLASS: ClassObject =
    ClassObject { vtbl: CLASS_VTBL.addr(), makes: Class::Decoder(Container::Png) };
static BMP_CLASS: ClassObject =
    ClassObject { vtbl: CLASS_VTBL.addr(), makes: Class::Decoder(Container::Bmp) };
static CONVERTER_CLASS: ClassObject =
    ClassObject { vtbl: CLASS_VTBL.addr(), makes: Class::Converter };

extern "system" fn class_query(this: Obj, riid: *const Guid, out: *mut Obj) -> HResult {
    static_query(this, &[IID_ICLASSFACTORY], riid, out)
}

extern "system" fn class_create(
    this: Obj,
    outer: *mut c_void,
    riid: *const Guid,
    out: *mut Obj,
) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, core::ptr::null_mut());
    if !outer.is_null() {
        return CLASS_E_NOAGGREGATION;
    }
    // SAFETY: one of our class objects.
    let makes = unsafe { (*(this as *const ClassObject)).makes };
    let mut obj: Obj = core::ptr::null_mut();
    match makes {
        Class::Factory => return factory_query(factory(), riid, out),
        Class::Decoder(c) => match decoder::new_decoder(c) {
            Some(d) => obj = d,
            None => return E_OUTOFMEMORY,
        },
        Class::Converter => {
            let r = transform::create(Kinds::Converter, &mut obj);
            if r != S_OK {
                return r;
            }
        }
    }
    // Hand out the asked-for interface; our reference goes.
    // SAFETY: IUnknown::QueryInterface of our own object.
    let qi: extern "system" fn(Obj, *const Guid, *mut Obj) -> HResult =
        unsafe { core::mem::transmute(crate::slot(obj, 0)) };
    let r = qi(obj, riid, out);
    Source(obj).release();
    r
}

extern "system" fn class_lock(_this: Obj, _lock: i32) -> HResult {
    S_OK
}

/// DllGetClassObject: the class objects of the factory, the PNG and BMP
/// decoders and the default format converter.
#[unsafe(no_mangle)]
pub extern "system" fn DllGetClassObject(
    clsid: *const Guid,
    riid: *const Guid,
    out: *mut Obj,
) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, core::ptr::null_mut());
    let Some(c) = read_guid(clsid) else { return E_INVALIDARG };
    let class = match c {
        CLSID_WICIMAGINGFACTORY1 | CLSID_WICIMAGINGFACTORY2 => &FACTORY_CLASS,
        CLSID_WICPNGDECODER1 | CLSID_WICPNGDECODER2 => &PNG_CLASS,
        CLSID_WICBMPDECODER => &BMP_CLASS,
        CLSID_WICDEFAULTFORMATCONVERTER => &CONVERTER_CLASS,
        _ => {
            gap(b"DllGetClassObject(class)", data1(&c));
            return CLASS_E_CLASSNOTAVAILABLE;
        }
    };
    class_query((class as *const ClassObject) as Obj, riid, out)
}

/// DllCanUnloadNow: S_FALSE (gala does not unload it).
#[unsafe(no_mangle)]
pub extern "system" fn DllCanUnloadNow() -> HResult {
    S_FALSE
}
