//! The PNG and BMP decoders (IWICBitmapDecoder) and their one frame
//! (IWICBitmapFrameDecode), as Wine's pngformat and bmpformat tests (run
//! on Windows) and Microsoft's "PNG Format Overview" describe them.
//!
//! - Initialize reads the whole stream from its start and decodes it;
//!   once initialized (or after QueryCapability) a decoder is
//!   WINCODEC_ERR_WRONGSTATE to initialize or query again.
//!   QueryCapability answers WICBitmapDecoderCapabilityCanDecodeAllImages
//!   for an image of the decoder's format, leaving the stream where it
//!   was. Before Initialize, GetFrameCount and GetFrame are
//!   WINCODEC_ERR_NOTINITIALIZED.
//! - One frame: GetFrame(0); another index is E_INVALIDARG.
//! - The decoder has no palette, preview, thumbnail, metadata or colour
//!   contexts (WINCODEC_ERR_PALETTEUNAVAILABLE, ..._UNSUPPORTEDOPERATION,
//!   ..._CODECNOTHUMBNAIL); GetDecoderInfo is not implemented
//!   (recorded).
//! - The PNG frame's native formats: grey 1/2/4/8/16 bits as
//!   BlackWhite, 2/4/8bppGray and 16bppGray, or with tRNS as the indexed
//!   format of their depth (a grey palette whose tRNS level is
//!   transparent; 16 bits: 64bppRGBA); palette images as 1/2/4/8bppIndexed
//!   (PLTE's colours, tRNS's alphas; without PLTE the image is
//!   WINCODEC_ERR_UNKNOWNIMAGEFORMAT, as are depths PNG does not allow);
//!   RGB as 24bppBGR (tRNS left out) or
//!   48bppRGB; grey with alpha and RGBA as 32bppBGRA or 64bppRGBA. The
//!   resolution is pHYs's in dpi, else 96 x 96. Its colour contexts:
//!   none (an iCCP profile is not offered: recorded). Metadata readers
//!   are not there (recorded, WINCODEC_ERR_UNSUPPORTEDOPERATION).
//! - The BMP frame: `bmp`; no colour contexts or metadata
//!   (WINCODEC_ERR_UNSUPPORTEDOPERATION).
//! - Frames have no thumbnail (WINCODEC_ERR_CODECNOTHUMBNAIL).

use alloc::vec::Vec;
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, Ordering};

use crate::format::{self, Fmt};
use crate::png;
use crate::{
    E_INVALIDARG, E_OUTOFMEMORY, Guid, HResult, Header, IID_IWICBITMAPSOURCE, Obj, Rect, S_OK,
    Spin, VTable, WINCODEC_ERR_BADIMAGE, WINCODEC_ERR_CODECNOTHUMBNAIL,
    WINCODEC_ERR_NOTINITIALIZED, WINCODEC_ERR_PALETTEUNAVAILABLE, WINCODEC_ERR_UNKNOWNIMAGEFORMAT,
    WINCODEC_ERR_UNSUPPORTEDOPERATION, WINCODEC_ERR_WRONGSTATE, add_ref, bmp, copy_from_memory,
    gap, guid, not_implemented, palette, put, query, release_object, row_bytes, stream, try_box,
    zeroed,
};

pub const CONTAINER_PNG: Guid =
    guid(0x1b7c_faf4, 0x713f, 0x473c, [0xbb, 0xcd, 0x61, 0x37, 0x42, 0x5f, 0xae, 0xaf]);
pub const CONTAINER_BMP: Guid =
    guid(0x0af1_d87e, 0xfcfe, 0x4188, [0xbd, 0xeb, 0xa7, 0x90, 0x64, 0x71, 0xcb, 0xe3]);
const IID_IWICBITMAPDECODER: Guid =
    guid(0x9edd_e9e7, 0x8dee, 0x47ea, [0x99, 0xdf, 0xe6, 0xfa, 0xf2, 0xed, 0x44, 0xbf]);
const IID_IWICBITMAPFRAMEDECODE: Guid =
    guid(0x3b16_811b, 0x6a43, 0x4ec9, [0xa8, 0x13, 0x3d, 0x93, 0x0c, 0x13, 0xb9, 0x40]);
const CAN_DECODE_ALL_IMAGES: u32 = 2;

/// A decoded image: rows `stride` apart in format `fmt`.
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub fmt: &'static Fmt,
    pub stride: usize,
    pub data: Vec<u8>,
    pub palette: Option<Vec<u32>>,
    pub dpi: (f64, f64),
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Container {
    Png,
    Bmp,
}

impl Container {
    fn guid(self) -> Guid {
        match self {
            Container::Png => CONTAINER_PNG,
            Container::Bmp => CONTAINER_BMP,
        }
    }

    fn sniff(self, data: &[u8]) -> bool {
        match self {
            Container::Png => data.starts_with(b"\x89PNG\r\n\x1a\n"),
            Container::Bmp => bmp::sniff(data),
        }
    }

    fn decode(self, data: &[u8]) -> Result<Image, HResult> {
        match self {
            Container::Png => decode_png(data),
            Container::Bmp => bmp::decode(data),
        }
    }
}

/// Fresh, queried (QueryCapability), being initialized, or ready.
#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Fresh,
    Queried,
    Busy,
    Ready,
}

#[repr(C)]
pub struct Decoder {
    h: Header,
    container: Container,
    state: Spin<State>,
    /// Written once, before `ready` is set; read only after.
    image: UnsafeCell<Option<Image>>,
    ready: AtomicBool,
}
// SAFETY: `image` is written once before `ready` (Release) and only read
// after `ready` (Acquire).
unsafe impl Sync for Decoder {}

static DECODER_VTBL: VTable<14> = VTable([
    decoder_query as *const (),
    add_ref as *const (),
    decoder_release as *const (),
    query_capability as *const (),
    initialize as *const (),
    get_container_format as *const (),
    get_decoder_info as *const (),
    decoder_copy_palette as *const (),
    decoder_get_metadata_query_reader as *const (),
    get_preview as *const (),
    decoder_get_color_contexts as *const (),
    decoder_get_thumbnail as *const (),
    get_frame_count as *const (),
    get_frame as *const (),
]);

/// A new decoder of `container` (one reference).
pub fn new_decoder(container: Container) -> Option<Obj> {
    try_box(Decoder {
        h: Header::new(DECODER_VTBL.addr()),
        container,
        state: Spin::new(State::Fresh),
        image: UnsafeCell::new(None),
        ready: AtomicBool::new(false),
    })
    .map(|d| d as Obj)
}

/// The decoder for data starting like `data`, if gala has one.
pub fn container_of(data: &[u8]) -> Option<Container> {
    [Container::Png, Container::Bmp].into_iter().find(|c| c.sniff(data))
}

/// Initializes a fresh decoder from data already read (the factory's
/// CreateDecoderFromStream and ...FromFilename).
pub fn initialize_with(d: Obj, data: &[u8]) -> HResult {
    let dec = this(d);
    match dec.container.decode(data) {
        Ok(img) => {
            dec.state.with(|s| *s = State::Busy);
            // SAFETY: not ready yet: nobody reads the image.
            unsafe { *dec.image.get() = Some(img) };
            dec.ready.store(true, Ordering::Release);
            dec.state.with(|s| *s = State::Ready);
            S_OK
        }
        Err(e) => e,
    }
}

fn this<'a>(o: Obj) -> &'a Decoder {
    // SAFETY: one of our decoders.
    unsafe { &*(o as *const Decoder) }
}

impl Decoder {
    fn image(&self) -> Option<&Image> {
        if !self.ready.load(Ordering::Acquire) {
            return None;
        }
        // SAFETY: written once before `ready`.
        unsafe { (*self.image.get()).as_ref() }
    }
}

extern "system" fn decoder_query(this: Obj, riid: *const Guid, out: *mut Obj) -> HResult {
    query(this, &[IID_IWICBITMAPDECODER], riid, out)
}

extern "system" fn decoder_release(this: Obj) -> u32 {
    release_object::<Decoder>(this)
}

extern "system" fn query_capability(this_: Obj, stream_: Obj, out: *mut u32) -> HResult {
    if stream_.is_null() || out.is_null() {
        return E_INVALIDARG;
    }
    let d = this(this_);
    let fresh = d.state.with(|s| {
        let was = *s;
        if was == State::Fresh {
            *s = State::Queried;
        }
        was == State::Fresh
    });
    if !fresh {
        return WINCODEC_ERR_WRONGSTATE;
    }
    // Where the stream stands, to leave it there.
    let seek: extern "system" fn(Obj, i64, u32, *mut u64) -> HResult =
        // SAFETY: IStream::Seek.
        unsafe { core::mem::transmute(crate::slot(stream_, 5)) };
    let mut at = 0u64;
    let _ = seek(stream_, 0, 1, &mut at);
    let data = stream::read_all(stream_);
    let _ = seek(stream_, at as i64, 0, core::ptr::null_mut());
    let data = match data {
        Ok(d) => d,
        Err(e) => return e,
    };
    put(out, if d.container.sniff(&data) { CAN_DECODE_ALL_IMAGES } else { 0 });
    S_OK
}

extern "system" fn initialize(this_: Obj, stream_: Obj, _options: u32) -> HResult {
    if stream_.is_null() {
        return E_INVALIDARG;
    }
    let d = this(this_);
    let fresh = d.state.with(|s| {
        let was = *s;
        if was == State::Fresh {
            *s = State::Busy;
        }
        was == State::Fresh
    });
    if !fresh {
        return WINCODEC_ERR_WRONGSTATE;
    }
    let r = match stream::read_all(stream_) {
        Ok(data) => d.container.decode(&data),
        Err(e) => Err(e),
    };
    match r {
        Ok(img) => {
            // SAFETY: Busy: nobody else writes or reads the image.
            unsafe { *d.image.get() = Some(img) };
            d.ready.store(true, Ordering::Release);
            d.state.with(|s| *s = State::Ready);
            S_OK
        }
        Err(e) => {
            d.state.with(|s| *s = State::Fresh);
            e
        }
    }
}

extern "system" fn get_container_format(this_: Obj, out: *mut Guid) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    put(out, this(this_).container.guid());
    S_OK
}

extern "system" fn get_decoder_info(_this: Obj, _out: *mut Obj) -> HResult {
    not_implemented(b"IWICBitmapDecoder::GetDecoderInfo")
}

extern "system" fn decoder_copy_palette(_this: Obj, palette: Obj) -> HResult {
    if palette.is_null() {
        return E_INVALIDARG;
    }
    WINCODEC_ERR_PALETTEUNAVAILABLE
}

extern "system" fn decoder_get_metadata_query_reader(this_: Obj, _out: *mut Obj) -> HResult {
    if this(this_).container == Container::Png {
        gap(b"IWICBitmapDecoder::GetMetadataQueryReader(PNG)", 0);
    }
    WINCODEC_ERR_UNSUPPORTEDOPERATION
}

extern "system" fn get_preview(_this: Obj, _out: *mut Obj) -> HResult {
    WINCODEC_ERR_UNSUPPORTEDOPERATION
}

extern "system" fn decoder_get_color_contexts(
    _this: Obj,
    _count: u32,
    _contexts: *mut Obj,
    _actual: *mut u32,
) -> HResult {
    WINCODEC_ERR_UNSUPPORTEDOPERATION
}

extern "system" fn decoder_get_thumbnail(_this: Obj, _out: *mut Obj) -> HResult {
    WINCODEC_ERR_CODECNOTHUMBNAIL
}

extern "system" fn get_frame_count(this_: Obj, out: *mut u32) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    if this(this_).image().is_none() {
        return WINCODEC_ERR_NOTINITIALIZED;
    }
    put(out, 1);
    S_OK
}

extern "system" fn get_frame(this_: Obj, index: u32, out: *mut Obj) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    if this(this_).image().is_none() {
        return WINCODEC_ERR_NOTINITIALIZED;
    }
    if index != 0 {
        return E_INVALIDARG;
    }
    add_ref(this_);
    match try_box(Frame { h: Header::new(FRAME_VTBL.addr()), decoder: this_ }) {
        Some(f) => {
            put(out, f as Obj);
            S_OK
        }
        // The frame try_box could not place was dropped, releasing the
        // decoder.
        None => E_OUTOFMEMORY,
    }
}

/// A decoder's frame (holds a reference on the decoder).
#[repr(C)]
struct Frame {
    h: Header,
    decoder: Obj,
}

impl Drop for Frame {
    fn drop(&mut self) {
        release_object::<Decoder>(self.decoder);
    }
}

static FRAME_VTBL: VTable<11> = VTable([
    frame_query as *const (),
    add_ref as *const (),
    frame_release as *const (),
    frame_get_size as *const (),
    frame_get_pixel_format as *const (),
    frame_get_resolution as *const (),
    frame_copy_palette as *const (),
    frame_copy_pixels as *const (),
    frame_get_metadata_query_reader as *const (),
    frame_get_color_contexts as *const (),
    frame_get_thumbnail as *const (),
]);

fn frame<'a>(o: Obj) -> (&'a Decoder, &'a Image) {
    // SAFETY: one of our frames, made only of a ready decoder.
    let d = this(unsafe { (*(o as *const Frame)).decoder });
    match d.image() {
        Some(i) => (d, i),
        None => unreachable!("a frame of a decoder that is not ready"),
    }
}

extern "system" fn frame_query(this: Obj, riid: *const Guid, out: *mut Obj) -> HResult {
    query(this, &[IID_IWICBITMAPFRAMEDECODE, IID_IWICBITMAPSOURCE], riid, out)
}

extern "system" fn frame_release(this: Obj) -> u32 {
    release_object::<Frame>(this)
}

extern "system" fn frame_get_size(this_: Obj, w: *mut u32, h: *mut u32) -> HResult {
    if w.is_null() || h.is_null() {
        return E_INVALIDARG;
    }
    let (_, i) = frame(this_);
    put(w, i.width);
    put(h, i.height);
    S_OK
}

extern "system" fn frame_get_pixel_format(this_: Obj, out: *mut Guid) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    put(out, frame(this_).1.fmt.guid);
    S_OK
}

extern "system" fn frame_get_resolution(this_: Obj, x: *mut f64, y: *mut f64) -> HResult {
    if x.is_null() || y.is_null() {
        return E_INVALIDARG;
    }
    let (_, i) = frame(this_);
    put(x, i.dpi.0);
    put(y, i.dpi.1);
    S_OK
}

extern "system" fn frame_copy_palette(this_: Obj, pal: Obj) -> HResult {
    if pal.is_null() {
        return E_INVALIDARG;
    }
    match &frame(this_).1.palette {
        None => WINCODEC_ERR_PALETTEUNAVAILABLE,
        Some(colors) => palette::assign(pal, palette::CUSTOM, colors),
    }
}

extern "system" fn frame_copy_pixels(
    this_: Obj,
    rect: *const Rect,
    stride: u32,
    size: u32,
    buf: *mut u8,
) -> HResult {
    let (_, i) = frame(this_);
    copy_from_memory(&i.data, i.stride, i.fmt.bpp, i.width, i.height, rect, stride, size, buf)
}

extern "system" fn frame_get_metadata_query_reader(this_: Obj, _out: *mut Obj) -> HResult {
    if frame(this_).0.container == Container::Png {
        gap(b"IWICBitmapFrameDecode::GetMetadataQueryReader(PNG)", 0);
    }
    WINCODEC_ERR_UNSUPPORTEDOPERATION
}

extern "system" fn frame_get_color_contexts(
    this_: Obj,
    _count: u32,
    _contexts: *mut Obj,
    actual: *mut u32,
) -> HResult {
    if frame(this_).0.container != Container::Png {
        return WINCODEC_ERR_UNSUPPORTEDOPERATION;
    }
    if actual.is_null() {
        return E_INVALIDARG;
    }
    put(actual, 0);
    S_OK
}

extern "system" fn frame_get_thumbnail(_this: Obj, _out: *mut Obj) -> HResult {
    WINCODEC_ERR_CODECNOTHUMBNAIL
}

/// The PNG's samples in WIC's native format for its colour type and
/// depth.
fn decode_png(data: &[u8]) -> Result<Image, HResult> {
    let raw = match png::decode_raw(data) {
        Ok(r) => r,
        Err(png::Error::NotPng | png::Error::Corrupt) => {
            return Err(WINCODEC_ERR_UNKNOWNIMAGEFORMAT);
        }
        Err(png::Error::OutOfMemory) => return Err(E_OUTOFMEMORY),
        // png.rs recorded it.
        Err(png::Error::Unsupported(_)) => return Err(WINCODEC_ERR_UNSUPPORTEDOPERATION),
    };
    if data.windows(4).any(|w| w == b"iCCP") {
        gap(b"PNG decoder(iCCP colour context)", 0);
    }
    let (w, h, depth) = (raw.width, raw.height, raw.depth);
    let has_trns = !raw.trns.is_empty();
    let fmt_of = |g: Guid| format::find(&g).ok_or(WINCODEC_ERR_BADIMAGE);
    let key = raw.trns.get(..2).map(|b| u32::from(u16::from_be_bytes([b[0], b[1]])));
    // (format, how each row's samples become the format's bytes)
    let (fmt, palette): (&'static Fmt, Option<Vec<u32>>) = match (raw.color, depth) {
        (0, 1) if !has_trns => (fmt_of(format::BLACK_WHITE)?, None),
        (0, 2) if !has_trns => (fmt_of(format::GRAY_2)?, None),
        (0, 4) if !has_trns => (fmt_of(format::GRAY_4)?, None),
        (0, 8) if !has_trns => (fmt_of(format::GRAY_8)?, None),
        (0, 16) if !has_trns => (fmt_of(format::GRAY_16)?, None),
        (0, 1 | 2 | 4 | 8) => {
            let n = 1u32 << depth;
            let mut p = Vec::new();
            p.try_reserve_exact(n as usize).map_err(|_| E_OUTOFMEMORY)?;
            for i in 0..n {
                let g = i * 255 / (n - 1);
                let a = if Some(i) == key.map(|k| k & (n - 1)) { 0 } else { 0xff };
                p.push(a << 24 | g << 16 | g << 8 | g);
            }
            (indexed(depth)?, Some(p))
        }
        (3, _) => {
            if raw.palette.len() < 3 {
                return Err(WINCODEC_ERR_UNKNOWNIMAGEFORMAT);
            }
            let mut p = Vec::new();
            let n = raw.palette.len() / 3;
            p.try_reserve_exact(n).map_err(|_| E_OUTOFMEMORY)?;
            for i in 0..n {
                let c = &raw.palette[3 * i..3 * i + 3];
                let a = u32::from(raw.trns.get(i).copied().unwrap_or(0xff));
                p.push(a << 24 | u32::from(c[0]) << 16 | u32::from(c[1]) << 8 | u32::from(c[2]));
            }
            (indexed(depth)?, Some(p))
        }
        (2, 8) => (fmt_of(format::BGR24)?, None),
        (2, 16) => (fmt_of(format::RGB48)?, None),
        (4 | 6, 8) => (fmt_of(format::BGRA32)?, None),
        (0 | 4 | 6, 16) => (fmt_of(format::RGBA64)?, None),
        _ => return Err(WINCODEC_ERR_UNKNOWNIMAGEFORMAT),
    };
    let stride = row_bytes(w, fmt.bpp);
    let size = stride.checked_mul(h as usize).ok_or(E_OUTOFMEMORY)?;
    let mut out = zeroed(size).ok_or(E_OUTOFMEMORY)?;
    for y in 0..h as usize {
        let src = raw.line(y);
        let dst = &mut out[y * stride..(y + 1) * stride];
        match (raw.color, depth) {
            (2, 8) => {
                for x in 0..w as usize {
                    dst[3 * x..3 * x + 3].copy_from_slice(&[
                        src[3 * x + 2],
                        src[3 * x + 1],
                        src[3 * x],
                    ]);
                }
            }
            (6, 8) => {
                for x in 0..w as usize {
                    let p = &src[4 * x..4 * x + 4];
                    dst[4 * x..4 * x + 4].copy_from_slice(&[p[2], p[1], p[0], p[3]]);
                }
            }
            (4, 8) => {
                for x in 0..w as usize {
                    let (g, a) = (src[2 * x], src[2 * x + 1]);
                    dst[4 * x..4 * x + 4].copy_from_slice(&[g, g, g, a]);
                }
            }
            (0, 16) if has_trns => {
                for x in 0..w as usize {
                    let (hi, lo) = (src[2 * x], src[2 * x + 1]);
                    let a =
                        if Some(u32::from(u16::from_be_bytes([hi, lo]))) == key { 0 } else { 0xff };
                    dst[8 * x..8 * x + 8].copy_from_slice(&[lo, hi, lo, hi, lo, hi, a, a]);
                }
            }
            (4, 16) => {
                for x in 0..w as usize {
                    let p = &src[4 * x..4 * x + 4];
                    dst[8 * x..8 * x + 8]
                        .copy_from_slice(&[p[1], p[0], p[1], p[0], p[1], p[0], p[3], p[2]]);
                }
            }
            (_, 16) => {
                // Big-endian samples to little-endian, in their order.
                for (d, s) in dst.chunks_exact_mut(2).zip(src.chunks_exact(2)) {
                    d.copy_from_slice(&[s[1], s[0]]);
                }
            }
            _ => dst.copy_from_slice(&src[..stride]),
        }
    }
    let dpi = match raw.phys {
        Some((x, y, 1)) if x != 0 && y != 0 => (f64::from(x) * 0.0254, f64::from(y) * 0.0254),
        _ => (96.0, 96.0),
    };
    Ok(Image { width: w, height: h, fmt, stride, data: out, palette, dpi })
}

fn indexed(depth: u8) -> Result<&'static Fmt, HResult> {
    let g = match depth {
        1 => format::INDEXED_1,
        2 => format::INDEXED_2,
        4 => format::INDEXED_4,
        8 => format::INDEXED_8,
        _ => return Err(WINCODEC_ERR_UNKNOWNIMAGEFORMAT),
    };
    format::find(&g).ok_or(WINCODEC_ERR_BADIMAGE)
}
