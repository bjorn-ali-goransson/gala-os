//! The Windows Imaging Component (WindowsCodecs.dll, wincodec.h) as
//! Office prepares its images with it (ROADMAP OF): the imaging factory,
//! bitmaps and locks, palettes, streams, the PNG and BMP decoders, the
//! format converter, the scaler and the clipper. Expected values are
//! those of Wine's windowscodecs conformance tests, which run on Windows
//! (dlls/windowscodecs/tests: bitmap.c, palette.c, stream.c,
//! bmpformat.c, pngformat.c, converter.c; their data reused here), and
//! of Microsoft's documentation (the PNG decoder's native pixel formats):
//!
//! - The factory: CoCreateInstance of CLSID_WICImagingFactory2 (and
//!   ...Factory1) for IWICImagingFactory, which answers
//!   IWICImagingFactory2; WICCreateImagingFactory_Proxy with
//!   WINCODEC_SDK_VERSION2.
//! - CreateBitmap (bitmap.c's test_createbitmap): zeroed pixels, no
//!   palette until SetPalette (CopyPalette keeps the palette's type),
//!   lock rectangles that leave the bitmap E_INVALIDARG, NULL the whole
//!   bitmap, stride 12 for 3 pixels of 24 bpp, the lock's size without
//!   the last row's padding (33, a 1x2 part 15, 6 bytes in), two read
//!   locks at once but not a write lock beside them
//!   (WINCODEC_ERR_ALREADYLOCKED), writes through a read lock seen by
//!   CopyPixels, resolution 0 x 0 until SetResolution.
//! - CreateBitmapFromMemory (test_CreateBitmapFromMemory): its argument
//!   errors, a copy of the caller's rows (a later change to them is not
//!   seen), a stride of 13 for 9-byte rows.
//! - CreateBitmapFromSource and ...FromSourceRect
//!   (test_createbitmapfromsource): WICBitmapNoCache hands back the
//!   bitmap itself; a copy keeps pixels, format and resolution, and
//!   no palette for a non-indexed format; rectangles clamped to the
//!   source, empty or outside it E_INVALIDARG.
//! - Palettes (palette.c): a new one is custom and empty; custom colours
//!   back as given (fewer when asked for fewer); predefined Custom or
//!   MedianCut E_INVALIDARG; FixedGray4 and FixedBW colours, grey and
//!   black-and-white answers.
//! - IWICStream over memory (stream.c's test_StreamOnMemory): a second
//!   initialization WINCODEC_ERR_WRONGSTATE, NULL memory E_INVALIDARG,
//!   reads short at the end, Seek's errors (past the end E_INVALIDARG,
//!   before the start or above 4 GB
//!   HRESULT_FROM_WIN32(ERROR_ARITHMETIC_OVERFLOW)), writes past the end
//!   STG_E_MEDIUMFULL, Stat's size and type.
//! - The BMP decoder (bmpformat.c): a 2x3 24 bpp image (24bppBGR, 96 dpi
//!   when one of the header's resolutions is 0, rows bottom-up, the
//!   CopyPixels argument errors, one frame (another index
//!   WINCODEC_ERR_FRAMEMISSING, Windows), no palette, metadata,
//!   colour contexts or thumbnail, a second Initialize
//!   WINCODEC_ERR_WRONGSTATE); a 1 bpp image with a core header
//!   (1bppIndexed and its colour table); 32 bpp bit fields (32bppBGR,
//!   72 dpi); CreateDecoder's arguments.
//! - The PNG decoder through SHCreateMemStream (pngformat.c, and the
//!   native formats of Microsoft's "PNG Format Overview" and pngformat's
//!   table): RGBA as 32bppBGRA, 8-bit grey as 8bppGray, a 4-bit palette
//!   image with tRNS as 4bppIndexed with PLTE's colours and tRNS's
//!   alphas, 8-bit RGB with tRNS as 24bppBGR; pHYs as the resolution;
//!   the frame has no colour contexts (count 0, a NULL count
//!   E_INVALIDARG), the decoder none (WINCODEC_ERR_UNSUPPORTEDOPERATION).
//! - Conversion (converter.c): WICConvertBitmapSource BGRA -> PBGRA
//!   premultiplied (rounded to nearest: Wine's data), BGR -> BGRA opaque,
//!   24bppRGB -> 24bppBGR, a non-format GUID
//!   WINCODEC_ERR_COMPONENTNOTFOUND; CLSID_WICDefaultFormatConverter
//!   through COM, CanConvert BGRA -> BGR; the converter's resolution is
//!   the source's.
//! - The scaler (test_bitmap_scaler): every state and argument error,
//!   the source's format and resolution, its own size; nearest neighbour
//!   doubling a 2x2 image repeats each pixel; scaling a uniform image
//!   keeps its colour in every mode.
//! - The clipper (test_clipper): rectangles that leave the source
//!   E_INVALIDARG; its size; CopyPixels relative to its rectangle.
//!
//! What Windows does that the documentation leaves open is printed as
//! `info` lines and not checked: a PNG's resolution without pHYs, PBGRA
//! -> BGRA's rounding, where nearest neighbour samples when it halves an
//! image, Linear's values between two pixels, and a PNG's GetFrame(1).
//! WICCreateImagingFactory_Proxy with SDK version 0 is
//! WINCODEC_ERR_UNSUPPORTEDVERSION (Windows).
//!
//! Prints `windowscodecs_test: ok`, or `windowscodecs_test: check N got X
//! (line L)` and exits with N.

#![no_std]
#![no_main]

use core::ffi::c_void;
use core::ptr::{null, null_mut};

use ntrt as _;

const NAME: &str = "windowscodecs_test";

type Handle = usize;
type Hr = i32;
type Guid = [u8; 16];
type Obj = *mut c_void;

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, overlapped: usize) -> i32;
    fn ExitProcess(code: u32) -> !;
}

#[link(name = "ole32", kind = "raw-dylib")]
unsafe extern "system" {
    fn CoInitializeEx(reserved: *mut c_void, flags: u32) -> Hr;
    fn CoCreateInstance(
        clsid: *const Guid,
        outer: Obj,
        context: u32,
        iid: *const Guid,
        out: *mut Obj,
    ) -> Hr;
}

#[link(name = "shlwapi", kind = "raw-dylib")]
unsafe extern "system" {
    fn SHCreateMemStream(data: *const u8, size: u32) -> Obj;
}

#[link(name = "windowscodecs", kind = "raw-dylib")]
unsafe extern "system" {
    fn WICCreateImagingFactory_Proxy(version: u32, out: *mut Obj) -> Hr;
    fn WICConvertBitmapSource(to: *const Guid, source: Obj, out: *mut Obj) -> Hr;
}

const S_OK: Hr = 0;
const E_NOTIMPL: Hr = 0x8000_4001_u32 as i32;
const E_INVALIDARG: Hr = 0x8007_0057_u32 as i32;
const STG_E_MEDIUMFULL: Hr = 0x8003_0070_u32 as i32;
const ARITHMETIC_OVERFLOW: Hr = 0x8007_0216_u32 as i32;
const WINCODEC_ERR_WRONGSTATE: Hr = 0x8898_2f04_u32 as i32;
const WINCODEC_ERR_UNSUPPORTEDVERSION: Hr = 0x8898_2f0b_u32 as i32;
const WINCODEC_ERR_NOTINITIALIZED: Hr = 0x8898_2f0c_u32 as i32;
const WINCODEC_ERR_FRAMEMISSING: Hr = 0x8898_2f62_u32 as i32;
const WINCODEC_ERR_ALREADYLOCKED: Hr = 0x8898_2f0d_u32 as i32;
const WINCODEC_ERR_CODECNOTHUMBNAIL: Hr = 0x8898_2f44_u32 as i32;
const WINCODEC_ERR_PALETTEUNAVAILABLE: Hr = 0x8898_2f45_u32 as i32;
const WINCODEC_ERR_COMPONENTNOTFOUND: Hr = 0x8898_2f50_u32 as i32;
const WINCODEC_ERR_UNSUPPORTEDOPERATION: Hr = 0x8898_2f81_u32 as i32;
const WINCODEC_ERR_INSUFFICIENTBUFFER: Hr = 0x8898_2f8c_u32 as i32;

const COINIT_APARTMENTTHREADED: u32 = 2;
const CLSCTX_INPROC_SERVER: u32 = 1;
const LOCK_READ: u32 = 1;
const LOCK_WRITE: u32 = 2;
const CACHE_ON_LOAD: u32 = 2;
const NO_CACHE: u32 = 0;
const NEAREST: u32 = 0;
const LINEAR: u32 = 1;
const FANT: u32 = 3;
const PALETTE_CUSTOM: u32 = 0;
const PALETTE_MEDIAN_CUT: u32 = 1;
const PALETTE_FIXED_BW: u32 = 2;
const PALETTE_FIXED_GRAY4: u32 = 10;
const PALETTE_FIXED_GRAY256: u32 = 12;
const STREAM_SEEK_SET: u32 = 0;
const STREAM_SEEK_END: u32 = 2;

const fn guid(d1: u32, d2: u16, d3: u16, d4: [u8; 8]) -> Guid {
    let a = d1.to_le_bytes();
    let b = d2.to_le_bytes();
    let c = d3.to_le_bytes();
    [
        a[0], a[1], a[2], a[3], b[0], b[1], c[0], c[1], d4[0], d4[1], d4[2], d4[3], d4[4], d4[5],
        d4[6], d4[7],
    ]
}

const fn pf(last: u8) -> Guid {
    guid(0x6fdd_c324, 0x4e03, 0x4bfe, [0xb1, 0x85, 0x3d, 0x77, 0x76, 0x8d, 0xc9, last])
}

const CLSID_FACTORY1: Guid =
    guid(0xcaca_f262, 0x9370, 0x4615, [0xa1, 0x3b, 0x9f, 0x55, 0x39, 0xda, 0x4c, 0x0a]);
const CLSID_FACTORY2: Guid =
    guid(0x317d_06e8, 0x5f24, 0x433d, [0xbd, 0xf7, 0x79, 0xce, 0x68, 0xd8, 0xab, 0xc2]);
const CLSID_DEFAULT_CONVERTER: Guid =
    guid(0x1a3f_11dc, 0xb514, 0x4b17, [0x8c, 0x5f, 0x21, 0x54, 0x51, 0x38, 0x52, 0xf1]);
const IID_FACTORY: Guid =
    guid(0xec5e_c8a9, 0xc395, 0x4314, [0x9c, 0x77, 0x54, 0xd7, 0xa9, 0x35, 0xff, 0x70]);
const IID_FACTORY2: Guid =
    guid(0x7b81_6b45, 0x1996, 0x4476, [0xb1, 0x32, 0xde, 0x9e, 0x24, 0x7c, 0x8a, 0xf0]);
const IID_FORMAT_CONVERTER: Guid =
    guid(0x0000_0301, 0xa8f2, 0x4877, [0xba, 0x0a, 0xfd, 0x2b, 0x66, 0x45, 0xfb, 0x94]);
const CONTAINER_BMP: Guid =
    guid(0x0af1_d87e, 0xfcfe, 0x4188, [0xbd, 0xeb, 0xa7, 0x90, 0x64, 0x71, 0xcb, 0xe3]);
const CONTAINER_PNG: Guid =
    guid(0x1b7c_faf4, 0x713f, 0x473c, [0xbb, 0xcd, 0x61, 0x37, 0x42, 0x5f, 0xae, 0xaf]);
const VENDOR_MICROSOFT: Guid =
    guid(0xf0e7_49ca, 0xedef, 0x4589, [0xa7, 0x3a, 0xee, 0x0e, 0x62, 0x6a, 0x2a, 0x2b]);
const DONT_CARE: Guid = pf(0x00);
const INDEXED_1: Guid = pf(0x01);
const INDEXED_4: Guid = pf(0x03);
const GRAY_8: Guid = pf(0x08);
const BGR24: Guid = pf(0x0c);
const RGB24: Guid = pf(0x0d);
const BGR32: Guid = pf(0x0e);
const BGRA32: Guid = pf(0x0f);
const PBGRA32: Guid = pf(0x10);

#[repr(C)]
#[derive(Clone, Copy)]
struct Rect {
    x: i32,
    y: i32,
    w: i32,
    h: i32,
}

const fn rect(x: i32, y: i32, w: i32, h: i32) -> Rect {
    Rect { x, y, w, h }
}

/// STATSTG.
#[repr(C)]
struct StatStg {
    name: *mut u16,
    kind: u32,
    size: u64,
    times: [u64; 3],
    mode: u32,
    locks: u32,
    clsid: Guid,
    state: u32,
    reserved: u32,
}

/// bmpformat.c's testbmp_24bpp: 2x3, 24 bpp, X 120 dpi, Y 0.
const BMP_24: [u8; 78] = [
    66, 77, 78, 0, 0, 0, 0, 0, 0, 0, 54, 0, 0, 0, // file header
    40, 0, 0, 0, 2, 0, 0, 0, 3, 0, 0, 0, 1, 0, 24, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x74, 0x12, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, // BITMAPINFOHEADER
    0, 0, 0, 0, 255, 0, 0, 0, // bottom row
    255, 0, 0, 255, 255, 0, 0, 0, //
    255, 0, 255, 255, 255, 255, 0, 0, // top row
];
/// Its pixels top-down (24bppBGR).
const BMP_24_PIXELS: [u8; 18] =
    [255, 0, 255, 255, 255, 255, 255, 0, 0, 255, 255, 0, 0, 0, 0, 0, 255, 0];

/// bmpformat.c's testbmp_1bpp: 2x2, a core header, two colours.
const BMP_1: [u8; 40] = [
    66, 77, 40, 0, 0, 0, 0, 0, 0, 0, 32, 0, 0, 0, 12, 0, 0, 0, 2, 0, 2, 0, 1, 0, 1, 0, 255, 0, 0,
    0, 255, 0, 0xc0, 0, 0, 0, 0x80, 0, 0, 0,
];

/// bmpformat.c's testbmp_bitfields_abgr (a BITMAPV5HEADER): 2x1, 32 bpp,
/// red 0xff000000, green 0xff0000, blue 0xff00, 2835 pixels per metre.
const BMP_FIELDS: [u8; 14 + 124 + 8] = bmp_fields();

const fn bmp_fields() -> [u8; 146] {
    let mut b = [0u8; 146];
    let head: [u8; 14 + 56] = [
        b'B', b'M', 146, 0, 0, 0, 0, 0, 0, 0, 138, 0, 0, 0, // file header
        124, 0, 0, 0, 2, 0, 0, 0, 1, 0, 0, 0, 1, 0, 32, 0, 3, 0, 0, 0, 8, 0, 0, 0, 19, 11, 0, 0,
        19, 11, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, // BITMAPINFOHEADER part
        0, 0, 0, 255, 0, 0, 255, 0, 0, 255, 0, 0, 255, 0, 0, 0, // R, G, B, A masks
    ];
    let mut i = 0;
    while i < head.len() {
        b[i] = head[i];
        i += 1;
    }
    // bV5CSType as bmpformat.c writes it, bV5Intent LCS_GM_GRAPHICS.
    b[70] = b'B';
    b[71] = b'G';
    b[72] = b'R';
    b[73] = b's';
    b[122] = 2;
    let bits = [0, 255, 0, 0, 255, 0, 255, 0];
    let mut j = 0;
    while j < 8 {
        b[138 + j] = bits[j];
        j += 1;
    }
    b
}

/// gdiplus_test's 2x2 RGBA PNG: red, green / blue, white with alpha 0.
const PNG_RGBA: [u8; 78] = [
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x02, 0x08, 0x06, 0x00, 0x00, 0x00, 0x72, 0xb6, 0x0d,
    0x24, 0x00, 0x00, 0x00, 0x15, 0x49, 0x44, 0x41, 0x54, 0x78, 0xda, 0x63, 0xf8, 0xcf, 0xc0, 0xf0,
    0x1f, 0x04, 0x19, 0x81, 0x34, 0x10, 0x30, 0x30, 0x02, 0x00, 0x46, 0xd5, 0x07, 0xfb, 0x58, 0x88,
    0xaa, 0x15, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
];

/// A 3x1 8-bit grey PNG: 0, 128, 255.
const PNG_GRAY8: [u8; 69] = [
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x03, 0x00, 0x00, 0x00, 0x01, 0x08, 0x00, 0x00, 0x00, 0x00, 0x3e, 0x8b, 0x4b,
    0x68, 0x00, 0x00, 0x00, 0x0c, 0x49, 0x44, 0x41, 0x54, 0x78, 0xda, 0x63, 0x60, 0x68, 0xf8, 0x0f,
    0x00, 0x02, 0x03, 0x01, 0x80, 0x1a, 0x9c, 0x26, 0x3b, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e,
    0x44, 0xae, 0x42, 0x60, 0x82,
];

/// A 3x1 4-bit palette PNG: indices 0, 1, 2; PLTE red, green, blue;
/// tRNS 255, 128.
const PNG_PAL4: [u8; 103] = [
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x03, 0x00, 0x00, 0x00, 0x01, 0x04, 0x03, 0x00, 0x00, 0x00, 0xe9, 0xce, 0x09,
    0x87, 0x00, 0x00, 0x00, 0x09, 0x50, 0x4c, 0x54, 0x45, 0xff, 0x00, 0x00, 0x00, 0xff, 0x00, 0x00,
    0x00, 0xff, 0x2d, 0x4a, 0xcd, 0x8a, 0x00, 0x00, 0x00, 0x02, 0x74, 0x52, 0x4e, 0x53, 0xff, 0x80,
    0x08, 0x0f, 0xb3, 0x6a, 0x00, 0x00, 0x00, 0x0b, 0x49, 0x44, 0x41, 0x54, 0x78, 0xda, 0x63, 0x60,
    0x54, 0x00, 0x00, 0x00, 0x25, 0x00, 0x22, 0xe9, 0x82, 0x87, 0xe5, 0x00, 0x00, 0x00, 0x00, 0x49,
    0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
];

/// A 2x1 8-bit RGB PNG: red, black; tRNS black; pHYs 2835 x 2835 per
/// metre (72 dpi).
const PNG_RGB8: [u8; 108] = [
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00, 0x00, 0x7b, 0x40, 0xe8,
    0xdd, 0x00, 0x00, 0x00, 0x09, 0x70, 0x48, 0x59, 0x73, 0x00, 0x00, 0x0b, 0x13, 0x00, 0x00, 0x0b,
    0x13, 0x01, 0x00, 0x9a, 0x9c, 0x18, 0x00, 0x00, 0x00, 0x06, 0x74, 0x52, 0x4e, 0x53, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x6e, 0xa6, 0x07, 0x91, 0x00, 0x00, 0x00, 0x0c, 0x49, 0x44, 0x41, 0x54,
    0x78, 0xda, 0x63, 0xf8, 0xcf, 0x00, 0x02, 0x00, 0x06, 0x01, 0x01, 0x00, 0x41, 0x08, 0x8f, 0xf1,
    0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
];

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

/// An `info` line: what Windows answers, for the record (not checked).
fn info(what: &str, v: u64) {
    Line::new()
        .s(NAME.as_bytes())
        .s(b": info ")
        .s(what.as_bytes())
        .s(b" ")
        .hex(v)
        .s(b"\r\n")
        .print();
}

struct Checks(u32);

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
    fn eq(&mut self, got: u64, want: u64) -> Result<(), u32> {
        self.ok(got == want, got)
    }

    /// An HRESULT that should be `want`.
    #[track_caller]
    fn hr(&mut self, got: Hr, want: Hr) -> Result<(), u32> {
        self.ok(got == want, got as u32 as u64)
    }

    /// A GUID that should be `want` (its first eight bytes reported).
    #[track_caller]
    fn guid(&mut self, got: &Guid, want: &Guid) -> Result<(), u32> {
        self.ok(got == want, u64::from_le_bytes(got[..8].try_into().unwrap_or([0; 8])))
    }

    /// Bytes that should be `want` (the first differing index reported).
    #[track_caller]
    fn bytes(&mut self, got: &[u8], want: &[u8]) -> Result<(), u32> {
        let at = got.iter().zip(want).position(|(a, b)| a != b);
        let same = got.len() == want.len() && at.is_none();
        self.ok(same, at.map_or(got.len() as u64, |i| (i as u64) << 32 | u64::from(got[i])))
    }
}

/// Method `i` of COM object `o`, as function type `F`.
///
/// # Safety
/// `o` is a live COM object whose vtable has slot `i` of type `F`.
unsafe fn m<F: Copy>(o: Obj, i: usize) -> F {
    // SAFETY: as the caller promises.
    unsafe {
        let vt = *(o as *const *const usize);
        core::mem::transmute_copy::<usize, F>(&*vt.add(i))
    }
}

type FnObj = extern "system" fn(Obj, *mut Obj) -> Hr;
type FnRef = extern "system" fn(Obj) -> u32;

// SAFETY (all the helpers below): `o` is a live COM object of the
// interface whose method each one calls.
fn release(o: Obj) -> u32 {
    unsafe { m::<FnRef>(o, 2)(o) }
}

fn qi(o: Obj, iid: &Guid) -> (Hr, Obj) {
    let mut out = null_mut();
    let f: extern "system" fn(Obj, *const Guid, *mut Obj) -> Hr = unsafe { m(o, 0) };
    (f(o, iid, &mut out), out)
}

fn size(o: Obj) -> (Hr, u32, u32) {
    let (mut w, mut h) = (0, 0);
    let f: extern "system" fn(Obj, *mut u32, *mut u32) -> Hr = unsafe { m(o, 3) };
    (f(o, &mut w, &mut h), w, h)
}

fn size_raw(o: Obj, w: *mut u32, h: *mut u32) -> Hr {
    let f: extern "system" fn(Obj, *mut u32, *mut u32) -> Hr = unsafe { m(o, 3) };
    f(o, w, h)
}

fn pixel_format(o: Obj) -> (Hr, Guid) {
    let mut g = [0u8; 16];
    let f: extern "system" fn(Obj, *mut Guid) -> Hr = unsafe { m(o, 4) };
    (f(o, &mut g), g)
}

fn pixel_format_raw(o: Obj, g: *mut Guid) -> Hr {
    let f: extern "system" fn(Obj, *mut Guid) -> Hr = unsafe { m(o, 4) };
    f(o, g)
}

fn resolution_raw(o: Obj, x: *mut f64, y: *mut f64) -> Hr {
    let f: extern "system" fn(Obj, *mut f64, *mut f64) -> Hr = unsafe { m(o, 5) };
    f(o, x, y)
}

fn resolution(o: Obj) -> (Hr, f64, f64) {
    let (mut x, mut y) = (-1.0, -1.0);
    let r = resolution_raw(o, &mut x, &mut y);
    (r, x, y)
}

fn copy_palette(o: Obj, p: Obj) -> Hr {
    let f: extern "system" fn(Obj, Obj) -> Hr = unsafe { m(o, 6) };
    f(o, p)
}

fn copy_pixels(o: Obj, r: Option<&Rect>, stride: u32, buf: &mut [u8]) -> Hr {
    let f: extern "system" fn(Obj, *const Rect, u32, u32, *mut u8) -> Hr = unsafe { m(o, 7) };
    f(o, r.map_or(null(), |r| r as *const Rect), stride, buf.len() as u32, buf.as_mut_ptr())
}

fn copy_pixels_sized(o: Obj, r: Option<&Rect>, stride: u32, size: u32, buf: &mut [u8]) -> Hr {
    let f: extern "system" fn(Obj, *const Rect, u32, u32, *mut u8) -> Hr = unsafe { m(o, 7) };
    f(o, r.map_or(null(), |r| r as *const Rect), stride, size, buf.as_mut_ptr())
}

/// The factory method in slot `i` that takes only an output.
fn make(f: Obj, i: usize) -> (Hr, Obj) {
    let mut out = null_mut();
    let g: FnObj = unsafe { m(f, i) };
    (g(f, &mut out), out)
}

fn create_bitmap(f: Obj, w: u32, h: u32, fmt: &Guid) -> (Hr, Obj) {
    let mut out = null_mut();
    let g: extern "system" fn(Obj, u32, u32, *const Guid, u32, *mut Obj) -> Hr =
        unsafe { m(f, 17) };
    (g(f, w, h, fmt, CACHE_ON_LOAD, &mut out), out)
}

fn create_from_source(f: Obj, src: Obj, option: u32) -> (Hr, Obj) {
    let mut out = 0xdead_beef_usize as Obj;
    let g: extern "system" fn(Obj, Obj, u32, *mut Obj) -> Hr = unsafe { m(f, 18) };
    (g(f, src, option, &mut out), out)
}

fn create_from_source_rect(f: Obj, src: Obj, r: [u32; 4]) -> (Hr, Obj) {
    let mut out = null_mut();
    let g: extern "system" fn(Obj, Obj, u32, u32, u32, u32, *mut Obj) -> Hr = unsafe { m(f, 19) };
    (g(f, src, r[0], r[1], r[2], r[3], &mut out), out)
}

fn create_from_memory(f: Obj, w: u32, h: u32, fmt: &Guid, stride: u32, data: &[u8]) -> (Hr, Obj) {
    let mut out = null_mut();
    let g: extern "system" fn(Obj, u32, u32, *const Guid, u32, u32, *const u8, *mut Obj) -> Hr =
        unsafe { m(f, 20) };
    let p = if data.is_empty() { null() } else { data.as_ptr() };
    (g(f, w, h, fmt, stride, data.len() as u32, p, &mut out), out)
}

fn lock(b: Obj, r: Option<&Rect>, flags: u32) -> (Hr, Obj) {
    let mut out = null_mut();
    let g: extern "system" fn(Obj, *const Rect, u32, *mut Obj) -> Hr = unsafe { m(b, 8) };
    (g(b, r.map_or(null(), |r| r as *const Rect), flags, &mut out), out)
}

fn lock_stride(l: Obj) -> (Hr, u32) {
    let mut s = 0;
    let g: extern "system" fn(Obj, *mut u32) -> Hr = unsafe { m(l, 4) };
    (g(l, &mut s), s)
}

fn lock_data(l: Obj) -> (Hr, u32, *mut u8) {
    let (mut n, mut p) = (0, null_mut());
    let g: extern "system" fn(Obj, *mut u32, *mut *mut u8) -> Hr = unsafe { m(l, 5) };
    (g(l, &mut n, &mut p), n, p)
}

fn lock_format(l: Obj) -> (Hr, Guid) {
    let mut g = [0u8; 16];
    let f: extern "system" fn(Obj, *mut Guid) -> Hr = unsafe { m(l, 6) };
    (f(l, &mut g), g)
}

fn set_palette(b: Obj, p: Obj) -> Hr {
    let g: extern "system" fn(Obj, Obj) -> Hr = unsafe { m(b, 9) };
    g(b, p)
}

fn set_resolution(b: Obj, x: f64, y: f64) -> Hr {
    let g: extern "system" fn(Obj, f64, f64) -> Hr = unsafe { m(b, 10) };
    g(b, x, y)
}

fn palette_predefined(p: Obj, kind: u32) -> Hr {
    let g: extern "system" fn(Obj, u32, i32) -> Hr = unsafe { m(p, 3) };
    g(p, kind, 0)
}

fn palette_custom(p: Obj, colors: &[u32]) -> Hr {
    let g: extern "system" fn(Obj, *const u32, u32) -> Hr = unsafe { m(p, 4) };
    g(p, colors.as_ptr(), colors.len() as u32)
}

fn palette_u32(p: Obj, slot: usize) -> (Hr, u32) {
    let mut v = 0xdead_beef;
    let g: extern "system" fn(Obj, *mut u32) -> Hr = unsafe { m(p, slot) };
    (g(p, &mut v), v)
}

fn palette_colors(p: Obj, out: &mut [u32]) -> (Hr, u32) {
    let mut n = 0;
    let g: extern "system" fn(Obj, u32, *mut u32, *mut u32) -> Hr = unsafe { m(p, 9) };
    (g(p, out.len() as u32, out.as_mut_ptr(), &mut n), n)
}

fn palette_bool(p: Obj, slot: usize) -> (Hr, i32) {
    let mut v = -1;
    let g: extern "system" fn(Obj, *mut i32) -> Hr = unsafe { m(p, slot) };
    (g(p, &mut v), v)
}

fn stream_init_memory(s: Obj, data: *mut u8, n: u32) -> Hr {
    let g: extern "system" fn(Obj, *mut u8, u32) -> Hr = unsafe { m(s, 16) };
    g(s, data, n)
}

fn stream_read(s: Obj, buf: *mut u8, n: u32) -> (Hr, u32) {
    let mut got = 0xdead_beef;
    let g: extern "system" fn(Obj, *mut u8, u32, *mut u32) -> Hr = unsafe { m(s, 3) };
    (g(s, buf, n, &mut got), got)
}

fn stream_write(s: Obj, buf: *const u8, n: u32) -> (Hr, u32) {
    let mut done = 0xdead_beef;
    let g: extern "system" fn(Obj, *const u8, u32, *mut u32) -> Hr = unsafe { m(s, 4) };
    (g(s, buf, n, &mut done), done)
}

fn stream_seek(s: Obj, offset: i64, origin: u32) -> (Hr, u64) {
    let mut at = 0xdead_beef_dead_beef;
    let g: extern "system" fn(Obj, i64, u32, *mut u64) -> Hr = unsafe { m(s, 5) };
    (g(s, offset, origin, &mut at), at)
}

fn decoder_from_stream(f: Obj, stream: Obj) -> (Hr, Obj) {
    let mut out = null_mut();
    let g: extern "system" fn(Obj, Obj, *const Guid, u32, *mut Obj) -> Hr = unsafe { m(f, 4) };
    (g(f, stream, null(), 0, &mut out), out)
}

/// A decoder of `data` through an IWICStream over it.
fn decode(f: Obj, data: &mut [u8]) -> (Hr, Obj) {
    let (r, s) = make(f, 14);
    if r != S_OK {
        return (r, null_mut());
    }
    let r = stream_init_memory(s, data.as_mut_ptr(), data.len() as u32);
    if r != S_OK {
        release(s);
        return (r, null_mut());
    }
    let out = decoder_from_stream(f, s);
    release(s);
    out
}

fn frame_count(d: Obj) -> (Hr, u32) {
    let mut n = 0;
    let g: extern "system" fn(Obj, *mut u32) -> Hr = unsafe { m(d, 12) };
    (g(d, &mut n), n)
}

fn get_frame(d: Obj, i: u32) -> (Hr, Obj) {
    let mut out = null_mut();
    let g: extern "system" fn(Obj, u32, *mut Obj) -> Hr = unsafe { m(d, 13) };
    (g(d, i, &mut out), out)
}

fn container(d: Obj) -> (Hr, Guid) {
    let mut g = [0u8; 16];
    let f: extern "system" fn(Obj, *mut Guid) -> Hr = unsafe { m(d, 5) };
    (f(d, &mut g), g)
}

fn converter_init(c: Obj, src: Obj, to: &Guid) -> Hr {
    let g: extern "system" fn(Obj, Obj, *const Guid, u32, Obj, f64, u32) -> Hr = unsafe { m(c, 8) };
    g(c, src, to, 0, null_mut(), 0.0, 0)
}

fn scaler_init(s: Obj, src: Obj, w: u32, h: u32, mode: u32) -> Hr {
    let g: extern "system" fn(Obj, Obj, u32, u32, u32) -> Hr = unsafe { m(s, 8) };
    g(s, src, w, h, mode)
}

fn clipper_init(c: Obj, src: Obj, r: &Rect) -> Hr {
    let g: extern "system" fn(Obj, Obj, *const Rect) -> Hr = unsafe { m(c, 8) };
    g(c, src, r)
}

/// A frame's pixels (its whole image, rows `stride` apart).
fn pixels<const N: usize>(o: Obj, stride: u32) -> (Hr, [u8; N]) {
    let mut b = [0u8; N];
    (copy_pixels(o, None, stride, &mut b), b)
}

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    // SAFETY: COM initialization for this thread.
    let r = unsafe { CoInitializeEx(null_mut(), COINIT_APARTMENTTHREADED) };
    c.ok(r >= 0, r as u32 as u64)?;

    // The factory.
    let mut f: Obj = null_mut();
    // SAFETY: COM's documented call with our outputs.
    let r = unsafe {
        CoCreateInstance(&CLSID_FACTORY2, null_mut(), CLSCTX_INPROC_SERVER, &IID_FACTORY, &mut f)
    };
    c.hr(r, S_OK)?;
    let (r, f2) = qi(f, &IID_FACTORY2);
    c.hr(r, S_OK)?;
    release(f2);
    let mut f1: Obj = null_mut();
    // SAFETY: as above.
    let r = unsafe {
        CoCreateInstance(&CLSID_FACTORY1, null_mut(), CLSCTX_INPROC_SERVER, &IID_FACTORY, &mut f1)
    };
    c.hr(r, S_OK)?;
    release(f1);
    let mut fp: Obj = null_mut();
    // SAFETY: the exported helper with our output.
    c.hr(unsafe { WICCreateImagingFactory_Proxy(0x0237, &mut fp) }, S_OK)?;
    c.ok(!fp.is_null(), 0)?;
    release(fp);
    let mut f0: Obj = null_mut();
    // SAFETY: as above.
    let r = unsafe { WICCreateImagingFactory_Proxy(0, &mut f0) };
    c.hr(r, WINCODEC_ERR_UNSUPPORTEDVERSION)?;

    bitmaps(&mut c, f)?;
    palettes(&mut c, f)?;
    streams(&mut c, f)?;
    bmp(&mut c, f)?;
    png(&mut c, f)?;
    conversion(&mut c, f)?;
    scaler(&mut c, f)?;
    clipper(&mut c, f)?;
    release(f);
    Ok(())
}

/// bitmap.c's test_createbitmap, test_CreateBitmapFromMemory and
/// test_createbitmapfromsource.
fn bitmaps(c: &mut Checks, f: Obj) -> Result<(), u32> {
    let data: [u8; 27] = [
        128, 128, 255, 128, 128, 128, 128, 255, 128, 128, 128, 128, 128, 128, 128, 255, 255, 255,
        255, 128, 128, 255, 255, 255, 255, 255, 255,
    ];
    let (r, b) = create_bitmap(f, 3, 3, &BGR24);
    c.hr(r, S_OK)?;
    let (r, p) = make(f, 9);
    c.hr(r, S_OK)?;
    c.hr(copy_palette(b, p), WINCODEC_ERR_PALETTEUNAVAILABLE)?;
    c.hr(palette_predefined(p, PALETTE_FIXED_GRAY256), S_OK)?;
    c.hr(set_palette(b, p), S_OK)?;
    c.hr(palette_predefined(p, PALETTE_FIXED_GRAY4), S_OK)?;
    c.hr(copy_palette(b, p), S_OK)?;
    let (r, kind) = palette_u32(p, 7);
    c.hr(r, S_OK)?;
    c.eq(kind.into(), PALETTE_FIXED_GRAY256.into())?;
    release(p);
    let (r, got) = pixels::<27>(b, 9);
    c.hr(r, S_OK)?;
    c.bytes(&got, &[0; 27])?;
    for bad in [rect(0, 0, 4, 3), rect(0, 0, 3, 4), rect(4, 0, 3, 3), rect(0, 4, 3, 3)] {
        c.hr(lock(b, Some(&bad), LOCK_READ).0, E_INVALIDARG)?;
    }
    let (r, l) = lock(b, None, LOCK_READ);
    c.hr(r, S_OK)?;
    let (r, w, h) = size(l);
    c.hr(r, S_OK)?;
    c.eq(w.into(), 3)?;
    c.eq(h.into(), 3)?;
    release(l);
    let all = rect(0, 0, 3, 3);
    let (r, l) = lock(b, Some(&all), LOCK_READ);
    c.hr(r, S_OK)?;
    let (r, stride) = lock_stride(l);
    c.hr(r, S_OK)?;
    c.eq(stride.into(), 12)?;
    let (r, n, base) = lock_data(l);
    c.hr(r, S_OK)?;
    c.eq(n.into(), 33)?;
    c.ok(!base.is_null(), 0)?;
    let (r, g) = lock_format(l);
    c.hr(r, S_OK)?;
    c.guid(&g, &BGR24)?;
    let (r, l2) = lock(b, Some(&all), LOCK_READ);
    c.hr(r, S_OK)?;
    let (r, n2, base2) = lock_data(l2);
    c.hr(r, S_OK)?;
    c.eq(n2.into(), 33)?;
    c.ok(base2 == base, base2 as u64)?;
    release(l2);
    c.hr(lock(b, Some(&all), LOCK_WRITE).0, WINCODEC_ERR_ALREADYLOCKED)?;
    for y in 0..3 {
        for x in 0..9 {
            // SAFETY: the lock's 33 bytes, rows 12 apart.
            unsafe { base.add(12 * y + x).write(data[9 * y + x]) };
        }
    }
    release(l);
    let (r, got) = pixels::<27>(b, 9);
    c.hr(r, S_OK)?;
    c.bytes(&got, &data)?;
    let part = rect(2, 0, 1, 2);
    let (r, l) = lock(b, Some(&part), LOCK_WRITE);
    c.hr(r, S_OK)?;
    c.hr(lock(b, Some(&part), LOCK_WRITE).0, WINCODEC_ERR_ALREADYLOCKED)?;
    c.hr(lock(b, Some(&part), LOCK_READ).0, WINCODEC_ERR_ALREADYLOCKED)?;
    let (r, n, p6) = lock_data(l);
    c.hr(r, S_OK)?;
    c.eq(n.into(), 15)?;
    c.ok(p6 as usize == base as usize + 6, p6 as u64)?;
    let (r, w, h) = size(l);
    c.hr(r, S_OK)?;
    c.eq(w.into(), 1)?;
    c.eq(h.into(), 2)?;
    release(l);
    let (r, x, y) = resolution(b);
    c.hr(r, S_OK)?;
    c.ok(x == 0.0 && y == 0.0, x as u64)?;
    c.hr(set_resolution(b, 12.0, 34.0), S_OK)?;
    let (r, x, y) = resolution(b);
    c.hr(r, S_OK)?;
    c.ok(x == 12.0 && y == 34.0, x as u64)?;

    // CreateBitmapFromSource.
    let (r, same) = create_from_source(f, b, NO_CACHE);
    c.hr(r, S_OK)?;
    c.ok(same == b, same as u64)?;
    release(same);
    let (r, copy) = create_from_source(f, b, CACHE_ON_LOAD);
    c.hr(r, S_OK)?;
    let (r, p) = make(f, 9);
    c.hr(r, S_OK)?;
    c.hr(copy_palette(copy, p), WINCODEC_ERR_PALETTEUNAVAILABLE)?;
    release(p);
    let (r, got) = pixels::<27>(copy, 9);
    c.hr(r, S_OK)?;
    c.bytes(&got, &data)?;
    let (r, g) = pixel_format(copy);
    c.hr(r, S_OK)?;
    c.guid(&g, &BGR24)?;
    let (r, x, y) = resolution(copy);
    c.hr(r, S_OK)?;
    c.ok(x == 12.0 && y == 34.0, x as u64)?;
    for (rc, want) in [([0, 0, 16, 32], (3, 3)), ([0, 0, 1, 1], (1, 1)), ([2, 1, 16, 32], (1, 2))] {
        let (r, part) = create_from_source_rect(f, copy, rc);
        c.hr(r, S_OK)?;
        let (r, w, h) = size(part);
        c.hr(r, S_OK)?;
        c.eq(u64::from(w) << 32 | u64::from(h), want.0 << 32 | want.1)?;
        release(part);
    }
    for rc in [[0, 0, 0, 2], [0, 0, 2, 0], [1, 3, 16, 32], [3, 1, 16, 32]] {
        c.hr(create_from_source_rect(f, copy, rc).0, E_INVALIDARG)?;
    }
    release(copy);
    release(b);

    // CreateBitmapFromMemory.
    let mut mem = data;
    c.hr(create_from_memory(f, 3, 3, &BGR24, 0, &[]).0, E_INVALIDARG)?;
    c.hr(create_from_memory(f, 3, 3, &BGR24, 0, &mem).0, E_INVALIDARG)?;
    c.hr(create_from_memory(f, 3, 3, &BGR24, 6, &mem).0, E_INVALIDARG)?;
    c.hr(create_from_memory(f, 3, 3, &BGR24, 12, &mem).0, WINCODEC_ERR_INSUFFICIENTBUFFER)?;
    c.hr(create_from_memory(f, 3, 3, &BGR24, 9, &mem[..26]).0, WINCODEC_ERR_INSUFFICIENTBUFFER)?;
    let (r, b) = create_from_memory(f, 3, 3, &BGR24, 9, &mem);
    c.hr(r, S_OK)?;
    let (r, w, h) = size(b);
    c.hr(r, S_OK)?;
    c.eq(u64::from(w) << 32 | u64::from(h), 3 << 32 | 3)?;
    mem[2] = 192;
    core::hint::black_box(&mem);
    let (r, got) = pixels::<27>(b, 9);
    c.hr(r, S_OK)?;
    c.bytes(&got, &data)?;
    release(b);
    let (r, b) = create_from_memory(f, 3, 2, &BGR24, 13, &data);
    c.hr(r, S_OK)?;
    let mut got = [0u8; 27];
    c.hr(copy_pixels(b, None, 13, &mut got), S_OK)?;
    let want: [u8; 27] = [
        128, 128, 255, 128, 128, 128, 128, 255, 128, 0, 0, 0, 0, 128, 128, 255, 255, 255, 255, 128,
        128, 255, 0, 0, 0, 0, 0,
    ];
    c.bytes(&got, &want)?;
    release(b);
    Ok(())
}

/// palette.c's custom and predefined palettes.
fn palettes(c: &mut Checks, f: Obj) -> Result<(), u32> {
    let (r, p) = make(f, 9);
    c.hr(r, S_OK)?;
    let (r, kind) = palette_u32(p, 7);
    c.hr(r, S_OK)?;
    c.eq(kind.into(), PALETTE_CUSTOM.into())?;
    let (r, n) = palette_u32(p, 8);
    c.hr(r, S_OK)?;
    c.eq(n.into(), 0)?;
    let colors = [0xff00_0000, 0xff00_00ff, 0xff00_ff00, 0xffff_0000];
    c.hr(palette_custom(p, &colors), S_OK)?;
    let mut got = [0u32; 6];
    let (r, n) = palette_colors(p, &mut got);
    c.hr(r, S_OK)?;
    c.eq(n.into(), 4)?;
    c.ok(got[..4] == colors, got[1].into())?;
    let (r, n) = palette_colors(p, &mut got[..2]);
    c.hr(r, S_OK)?;
    c.eq(n.into(), 2)?;
    let (r, v) = palette_bool(p, 12);
    c.hr(r, S_OK)?;
    c.eq(v as u64, 0)?;
    let (r, v) = palette_bool(p, 10);
    c.hr(r, S_OK)?;
    c.eq(v as u64, 0)?;
    let (r, v) = palette_bool(p, 11);
    c.hr(r, S_OK)?;
    c.eq(v as u64, 0)?;
    c.hr(palette_custom(p, &[0x8000_0000]), S_OK)?;
    let (r, v) = palette_bool(p, 12);
    c.hr(r, S_OK)?;
    c.eq(v as u64, 1)?;
    c.hr(palette_predefined(p, PALETTE_CUSTOM), E_INVALIDARG)?;
    c.hr(palette_predefined(p, PALETTE_MEDIAN_CUT), E_INVALIDARG)?;
    c.hr(palette_predefined(p, 0x0f), E_INVALIDARG)?;
    c.hr(palette_predefined(p, PALETTE_FIXED_GRAY4), S_OK)?;
    let (r, n) = palette_colors(p, &mut got);
    c.hr(r, S_OK)?;
    c.eq(n.into(), 4)?;
    c.ok(got[..4] == [0xff00_0000, 0xff55_5555, 0xffaa_aaaa, 0xffff_ffff], got[1].into())?;
    let (r, v) = palette_bool(p, 11);
    c.hr(r, S_OK)?;
    c.eq(v as u64, 1)?;
    c.hr(palette_predefined(p, PALETTE_FIXED_BW), S_OK)?;
    let (r, v) = palette_bool(p, 10);
    c.hr(r, S_OK)?;
    c.eq(v as u64, 1)?;
    let (r, n) = palette_colors(p, &mut got);
    c.hr(r, S_OK)?;
    c.eq(n.into(), 2)?;
    c.ok(got[..2] == [0xff00_0000, 0xffff_ffff], got[0].into())?;
    release(p);
    Ok(())
}

/// stream.c's test_StreamOnMemory.
fn streams(c: &mut Checks, f: Obj) -> Result<(), u32> {
    let mut memory = [0u8; 64];
    for (i, b) in memory.iter_mut().enumerate() {
        *b = i as u8;
    }
    let len = memory.len() as u32;
    let (r, s) = make(f, 14);
    c.hr(r, S_OK)?;
    c.hr(stream_read(s, memory.as_mut_ptr(), 1).0, WINCODEC_ERR_NOTINITIALIZED)?;
    c.hr(stream_init_memory(s, null_mut(), len), E_INVALIDARG)?;
    c.hr(stream_init_memory(s, memory.as_mut_ptr(), len), S_OK)?;
    c.hr(stream_init_memory(s, memory.as_mut_ptr(), len), WINCODEC_ERR_WRONGSTATE)?;
    let mut buf = [0u8; 80];
    let (r, n) = stream_read(s, buf.as_mut_ptr(), 10);
    c.hr(r, S_OK)?;
    c.eq(n.into(), 10)?;
    c.bytes(&buf[..10], &memory[..10])?;
    c.hr(stream_read(s, null_mut(), 1).0, E_INVALIDARG)?;
    let (r, at) = stream_seek(s, 0, STREAM_SEEK_SET);
    c.hr(r, S_OK)?;
    c.eq(at, 0)?;
    let (r, at) = stream_seek(s, 1 << 32, STREAM_SEEK_SET);
    c.hr(r, ARITHMETIC_OVERFLOW)?;
    c.eq(at, 0xdead_beef_dead_beef)?;
    c.hr(stream_seek(s, i64::from(len) + 10, STREAM_SEEK_SET).0, E_INVALIDARG)?;
    c.hr(stream_seek(s, 1, STREAM_SEEK_END).0, E_INVALIDARG)?;
    let (r, at) = stream_seek(s, -1, STREAM_SEEK_END);
    c.hr(r, S_OK)?;
    c.eq(at, u64::from(len) - 1)?;
    c.hr(stream_seek(s, -i64::from(len) - 5, STREAM_SEEK_END).0, ARITHMETIC_OVERFLOW)?;
    let (r, n) = stream_read(s, buf.as_mut_ptr(), 10);
    c.hr(r, S_OK)?;
    c.eq(n.into(), 1)?;
    c.eq(buf[0].into(), 63)?;
    stream_seek(s, 0, STREAM_SEEK_SET);
    let (r, n) = stream_read(s, buf.as_mut_ptr(), len + 10);
    c.hr(r, S_OK)?;
    c.eq(n.into(), len.into())?;
    stream_seek(s, 0, STREAM_SEEK_SET);
    let (r, n) = stream_write(s, [7u8, 8, 9].as_ptr(), 3);
    c.hr(r, S_OK)?;
    c.eq(n.into(), 3)?;
    c.bytes(&memory[..4], &[7, 8, 9, 3])?;
    let big = [0u8; 80];
    let (r, n) = stream_write(s, big.as_ptr(), len + 10);
    c.hr(r, STG_E_MEDIUMFULL)?;
    c.eq(n.into(), 0xdead_beef)?;
    let mut st = StatStg {
        name: core::ptr::dangling_mut(),
        kind: 0,
        size: 0,
        times: [1; 3],
        mode: 1,
        locks: 1,
        clsid: [0; 16],
        state: 1,
        reserved: 0,
    };
    let stat: extern "system" fn(Obj, *mut StatStg, u32) -> Hr = unsafe { m(s, 12) };
    c.hr(stat(s, null_mut(), 0), E_INVALIDARG)?;
    c.hr(stat(s, &mut st, 0), S_OK)?;
    c.ok(st.name.is_null(), st.name as u64)?;
    c.eq(st.kind.into(), 2)?;
    c.eq(st.size, len.into())?;
    c.eq(st.times[0] | st.times[1] | st.times[2] | u64::from(st.mode | st.locks | st.state), 0)?;
    let set_size: extern "system" fn(Obj, u64) -> Hr = unsafe { m(s, 6) };
    c.hr(set_size(s, 10), E_NOTIMPL)?;
    let revert: extern "system" fn(Obj) -> Hr = unsafe { m(s, 9) };
    c.hr(revert(s), E_NOTIMPL)?;
    let commit: extern "system" fn(Obj, u32) -> Hr = unsafe { m(s, 8) };
    c.hr(commit(s, 0), S_OK)?;
    release(s);
    Ok(())
}

/// bmpformat.c's 24 bpp, 1 bpp and bit-field images, and CreateDecoder.
fn bmp(c: &mut Checks, f: Obj) -> Result<(), u32> {
    let mut data = BMP_24;
    let (r, d) = decode(f, &mut data);
    c.hr(r, S_OK)?;
    let (r, g) = container(d);
    c.hr(r, S_OK)?;
    c.guid(&g, &CONTAINER_BMP)?;
    let reader: FnObj = unsafe { m(d, 8) };
    let mut o = null_mut();
    c.hr(reader(d, &mut o), WINCODEC_ERR_UNSUPPORTEDOPERATION)?;
    let thumb: FnObj = unsafe { m(d, 11) };
    c.hr(thumb(d, &mut o), WINCODEC_ERR_CODECNOTHUMBNAIL)?;
    let preview: FnObj = unsafe { m(d, 9) };
    c.hr(preview(d, &mut o), WINCODEC_ERR_UNSUPPORTEDOPERATION)?;
    let (r, n) = frame_count(d);
    c.hr(r, S_OK)?;
    c.eq(n.into(), 1)?;
    c.hr(get_frame(d, 1).0, WINCODEC_ERR_FRAMEMISSING)?;
    let (r, p) = make(f, 9);
    c.hr(r, S_OK)?;
    let dpal: extern "system" fn(Obj, Obj) -> Hr = unsafe { m(d, 7) };
    c.hr(dpal(d, p), WINCODEC_ERR_PALETTEUNAVAILABLE)?;
    let (r, fr) = get_frame(d, 0);
    c.hr(r, S_OK)?;
    let (r, w, h) = size(fr);
    c.hr(r, S_OK)?;
    c.eq(u64::from(w) << 32 | u64::from(h), 2 << 32 | 3)?;
    let (r, x, y) = resolution(fr);
    c.hr(r, S_OK)?;
    c.ok(x == 96.0 && y == 96.0, x as u64)?;
    let (r, g) = pixel_format(fr);
    c.hr(r, S_OK)?;
    c.guid(&g, &BGR24)?;
    c.hr(copy_palette(fr, p), WINCODEC_ERR_PALETTEUNAVAILABLE)?;
    let freader: FnObj = unsafe { m(fr, 8) };
    c.hr(freader(fr, &mut o), WINCODEC_ERR_UNSUPPORTEDOPERATION)?;
    let fcontexts: extern "system" fn(Obj, u32, *mut Obj, *mut u32) -> Hr = unsafe { m(fr, 9) };
    let mut count = 0;
    c.hr(fcontexts(fr, 1, &mut o, &mut count), WINCODEC_ERR_UNSUPPORTEDOPERATION)?;
    let fthumb: FnObj = unsafe { m(fr, 10) };
    c.hr(fthumb(fr, &mut o), WINCODEC_ERR_CODECNOTHUMBNAIL)?;
    let mut img = [0u8; 36];
    c.hr(copy_pixels(fr, Some(&rect(0, 0, 3, 3)), 6, &mut img), E_INVALIDARG)?;
    c.hr(copy_pixels(fr, Some(&rect(-1, 0, 2, 3)), 6, &mut img), E_INVALIDARG)?;
    c.hr(copy_pixels(fr, Some(&rect(0, 0, 2, 3)), 4, &mut img), E_INVALIDARG)?;
    c.hr(copy_pixels_sized(fr, Some(&rect(0, 0, 2, 3)), 6, 5, &mut img), E_INVALIDARG)?;
    c.hr(copy_pixels(fr, Some(&rect(0, 0, 2, 3)), 6, &mut img), S_OK)?;
    c.bytes(&img[..18], &BMP_24_PIXELS)?;
    let (r, got) = pixels::<18>(fr, 6);
    c.hr(r, S_OK)?;
    c.bytes(&got, &BMP_24_PIXELS)?;
    release(fr);
    release(d);

    // CreateDecoder, then Initialize twice.
    let create: extern "system" fn(Obj, *const Guid, *const Guid, *mut Obj) -> Hr =
        unsafe { m(f, 7) };
    let mut d = null_mut();
    c.hr(create(f, null(), null(), null_mut()), E_INVALIDARG)?;
    c.hr(create(f, null(), null(), &mut d), E_INVALIDARG)?;
    c.hr(create(f, &CONTAINER_BMP, &VENDOR_MICROSOFT, &mut d), S_OK)?;
    c.hr(frame_count(d).0, WINCODEC_ERR_NOTINITIALIZED)?;
    let (r, s) = make(f, 14);
    c.hr(r, S_OK)?;
    let mut data = BMP_24;
    c.hr(stream_init_memory(s, data.as_mut_ptr(), data.len() as u32), S_OK)?;
    let init: extern "system" fn(Obj, Obj, u32) -> Hr = unsafe { m(d, 4) };
    c.hr(init(d, s, 1), S_OK)?;
    c.hr(init(d, s, 1), WINCODEC_ERR_WRONGSTATE)?;
    let capability: extern "system" fn(Obj, Obj, *mut u32) -> Hr = unsafe { m(d, 3) };
    let mut cap = 0;
    c.hr(capability(d, s, &mut cap), WINCODEC_ERR_WRONGSTATE)?;
    release(d);
    c.hr(create(f, &CONTAINER_BMP, null(), &mut d), S_OK)?;
    c.hr(capability(d, s, &mut cap), S_OK)?;
    c.eq(cap.into(), 2)?;
    c.hr(init(d, s, 1), WINCODEC_ERR_WRONGSTATE)?;
    release(d);
    release(s);

    // 1 bpp with a core header.
    let mut data = BMP_1;
    let (r, d) = decode(f, &mut data);
    c.hr(r, S_OK)?;
    let (r, fr) = get_frame(d, 0);
    c.hr(r, S_OK)?;
    let (r, g) = pixel_format(fr);
    c.hr(r, S_OK)?;
    c.guid(&g, &INDEXED_1)?;
    c.hr(copy_palette(fr, p), S_OK)?;
    let mut colors = [0u32; 4];
    let (r, n) = palette_colors(p, &mut colors);
    c.hr(r, S_OK)?;
    c.eq(n.into(), 2)?;
    c.ok(colors[..2] == [0xff00_00ff, 0xff00_ff00], colors[0].into())?;
    let (r, got) = pixels::<2>(fr, 1);
    c.hr(r, S_OK)?;
    c.bytes(&got, &[0x80, 0xc0])?;
    release(fr);
    release(d);

    // 32 bpp bit fields.
    let mut data = BMP_FIELDS;
    let (r, d) = decode(f, &mut data);
    c.hr(r, S_OK)?;
    let (r, fr) = get_frame(d, 0);
    c.hr(r, S_OK)?;
    let (r, g) = pixel_format(fr);
    c.hr(r, S_OK)?;
    c.guid(&g, &BGR32)?;
    let (r, x, y) = resolution(fr);
    c.hr(r, S_OK)?;
    c.ok(near72(x) && near72(y), x as u64)?;
    let (r, got) = pixels::<8>(fr, 8);
    c.hr(r, S_OK)?;
    c.bytes(&got, &[0xff, 0, 0, 0, 0, 0xff, 0, 0])?;
    release(fr);
    release(d);
    release(p);
    Ok(())
}

/// PNGs through SHCreateMemStream: native formats, palette, pixels.
fn png(c: &mut Checks, f: Obj) -> Result<(), u32> {
    let open = |c: &mut Checks, data: &[u8]| -> Result<(Obj, Obj), u32> {
        // SAFETY: shlwapi copies our bytes.
        let s = unsafe { SHCreateMemStream(data.as_ptr(), data.len() as u32) };
        c.ok(!s.is_null(), 0)?;
        let (r, d) = decoder_from_stream(f, s);
        release(s);
        c.hr(r, S_OK)?;
        let (r, fr) = get_frame(d, 0);
        c.hr(r, S_OK)?;
        Ok((d, fr))
    };
    let (d, fr) = open(c, &PNG_RGBA)?;
    let (r, g) = container(d);
    c.hr(r, S_OK)?;
    c.guid(&g, &CONTAINER_PNG)?;
    let (r, other) = get_frame(d, 1);
    info("PNG GetFrame(1)", r as u32 as u64);
    if r == S_OK {
        release(other);
    }
    let dcontexts: extern "system" fn(Obj, u32, *mut Obj, *mut u32) -> Hr = unsafe { m(d, 10) };
    let mut count = 0xdead_beef;
    c.hr(dcontexts(d, 0, null_mut(), &mut count), WINCODEC_ERR_UNSUPPORTEDOPERATION)?;
    c.eq(count.into(), 0xdead_beef)?;
    let fcontexts: extern "system" fn(Obj, u32, *mut Obj, *mut u32) -> Hr = unsafe { m(fr, 9) };
    c.hr(fcontexts(fr, 0, null_mut(), null_mut()), E_INVALIDARG)?;
    c.hr(fcontexts(fr, 0, null_mut(), &mut count), S_OK)?;
    c.eq(count.into(), 0)?;
    let (r, w, h) = size(fr);
    c.hr(r, S_OK)?;
    c.eq(u64::from(w) << 32 | u64::from(h), 2 << 32 | 2)?;
    let (r, g) = pixel_format(fr);
    c.hr(r, S_OK)?;
    c.guid(&g, &BGRA32)?;
    let (r, x, _) = resolution(fr);
    c.hr(r, S_OK)?;
    info("PNG without pHYs: dpi x 1000", (x * 1000.0) as u64);
    let (r, got) = pixels::<16>(fr, 8);
    c.hr(r, S_OK)?;
    c.bytes(&got, &[0, 0, 255, 255, 0, 255, 0, 255, 255, 0, 0, 255, 255, 255, 255, 0])?;
    release(fr);
    release(d);

    let (d, fr) = open(c, &PNG_GRAY8)?;
    let (r, g) = pixel_format(fr);
    c.hr(r, S_OK)?;
    c.guid(&g, &GRAY_8)?;
    let (r, got) = pixels::<3>(fr, 3);
    c.hr(r, S_OK)?;
    c.bytes(&got, &[0, 128, 255])?;
    release(fr);
    release(d);

    let (d, fr) = open(c, &PNG_PAL4)?;
    let (r, g) = pixel_format(fr);
    c.hr(r, S_OK)?;
    c.guid(&g, &INDEXED_4)?;
    let (r, p) = make(f, 9);
    c.hr(r, S_OK)?;
    c.hr(copy_palette(fr, p), S_OK)?;
    let mut colors = [0u32; 16];
    let (r, n) = palette_colors(p, &mut colors);
    c.hr(r, S_OK)?;
    c.eq(n.into(), 3)?;
    c.ok(colors[..3] == [0xffff_0000, 0x8000_ff00, 0xff00_00ff], colors[1].into())?;
    release(p);
    let (r, got) = pixels::<2>(fr, 2);
    c.hr(r, S_OK)?;
    c.bytes(&got, &[0x01, 0x20])?;
    release(fr);
    release(d);

    let (d, fr) = open(c, &PNG_RGB8)?;
    let (r, g) = pixel_format(fr);
    c.hr(r, S_OK)?;
    c.guid(&g, &BGR24)?;
    let (r, x, y) = resolution(fr);
    c.hr(r, S_OK)?;
    c.ok(near72(x) && near72(y), x as u64)?;
    let (r, got) = pixels::<6>(fr, 6);
    c.hr(r, S_OK)?;
    c.bytes(&got, &[0, 0, 255, 0, 0, 0])?;
    release(fr);
    release(d);
    Ok(())
}

/// converter.c: BGRA -> PBGRA (Wine's alpha-80 data), BGR -> BGRA,
/// RGB -> BGR, a GUID that is no format, the default converter.
fn conversion(c: &mut Checks, f: Obj) -> Result<(), u32> {
    // Wine's 32bppBGRA80 pixels and their PBGRA values.
    let src: [u8; 32] = [
        255, 0, 0, 80, 0, 255, 0, 80, 3, 3, 3, 80, 63, 63, 63, 80, 98, 98, 98, 80, 156, 156, 156,
        80, 251, 251, 251, 80, 35, 35, 35, 80,
    ];
    let want: [u8; 32] = [
        80, 0, 0, 80, 0, 80, 0, 80, 1, 1, 1, 80, 20, 20, 20, 80, 31, 31, 31, 80, 49, 49, 49, 80,
        79, 79, 79, 80, 11, 11, 11, 80,
    ];
    let (r, b) = create_from_memory(f, 8, 1, &BGRA32, 32, &src);
    c.hr(r, S_OK)?;
    c.hr(set_resolution(b, 96.0, 48.0), S_OK)?;
    let mut out = null_mut();
    // SAFETY: the exported helper with our objects.
    c.hr(unsafe { WICConvertBitmapSource(&PBGRA32, b, &mut out) }, S_OK)?;
    let (r, g) = pixel_format(out);
    c.hr(r, S_OK)?;
    c.guid(&g, &PBGRA32)?;
    let (r, w, h) = size(out);
    c.hr(r, S_OK)?;
    c.eq(u64::from(w) << 32 | u64::from(h), 8 << 32 | 1)?;
    let (r, x, y) = resolution(out);
    c.hr(r, S_OK)?;
    c.ok(x == 96.0 && y == 48.0, x as u64)?;
    let (r, got) = pixels::<32>(out, 32);
    c.hr(r, S_OK)?;
    c.bytes(&got, &want)?;
    let mut two = [0u8; 8];
    c.hr(copy_pixels(out, Some(&rect(2, 0, 2, 1)), 8, &mut two), S_OK)?;
    c.bytes(&two, &want[8..16])?;
    release(out);
    release(b);

    // PBGRA back to straight alpha: the rounding is Windows' own.
    let (r, b) = create_from_memory(f, 8, 1, &PBGRA32, 32, &want);
    c.hr(r, S_OK)?;
    // SAFETY: as above.
    c.hr(unsafe { WICConvertBitmapSource(&BGRA32, b, &mut out) }, S_OK)?;
    let (r, got) = pixels::<32>(out, 32);
    c.hr(r, S_OK)?;
    info("PBGRA (1,1,1,80) -> BGRA", u32::from_le_bytes([got[8], got[9], got[10], got[11]]).into());
    info(
        "PBGRA (20,20,20,80) -> BGRA",
        u32::from_le_bytes([got[12], got[13], got[14], got[15]]).into(),
    );
    release(out);
    release(b);

    // BGR -> BGRA: opaque.
    let bgr: [u8; 8] = [255, 0, 0, 80, 1, 2, 3, 80];
    let (r, b) = create_from_memory(f, 2, 1, &BGR32, 8, &bgr);
    c.hr(r, S_OK)?;
    // SAFETY: as above.
    c.hr(unsafe { WICConvertBitmapSource(&BGRA32, b, &mut out) }, S_OK)?;
    let (r, got) = pixels::<8>(out, 8);
    c.hr(r, S_OK)?;
    c.bytes(&got, &[255, 0, 0, 255, 1, 2, 3, 255])?;
    release(out);
    // The same format: no conversion needed.
    // SAFETY: as above.
    c.hr(unsafe { WICConvertBitmapSource(&BGR32, b, &mut out) }, S_OK)?;
    let (r, g) = pixel_format(out);
    c.hr(r, S_OK)?;
    c.guid(&g, &BGR32)?;
    release(out);
    // A GUID that is no pixel format.
    // SAFETY: as above.
    c.hr(
        unsafe { WICConvertBitmapSource(&VENDOR_MICROSOFT, b, &mut out) },
        WINCODEC_ERR_COMPONENTNOTFOUND,
    )?;
    release(b);

    // 24bppRGB -> 24bppBGR.
    let rgb: [u8; 8] = [1, 2, 3, 4, 5, 6, 0, 0];
    let (r, b) = create_from_memory(f, 2, 1, &RGB24, 8, &rgb);
    c.hr(r, S_OK)?;
    // SAFETY: as above.
    c.hr(unsafe { WICConvertBitmapSource(&BGR24, b, &mut out) }, S_OK)?;
    let (r, got) = pixels::<6>(out, 6);
    c.hr(r, S_OK)?;
    c.bytes(&got, &[3, 2, 1, 6, 5, 4])?;
    release(out);

    // The default converter through COM.
    let mut conv: Obj = null_mut();
    // SAFETY: COM's documented call with our output.
    let r = unsafe {
        CoCreateInstance(
            &CLSID_DEFAULT_CONVERTER,
            null_mut(),
            CLSCTX_INPROC_SERVER,
            &IID_FORMAT_CONVERTER,
            &mut conv,
        )
    };
    c.hr(r, S_OK)?;
    let can: extern "system" fn(Obj, *const Guid, *const Guid, *mut i32) -> Hr =
        unsafe { m(conv, 9) };
    let mut yes = 0;
    c.hr(can(conv, &BGRA32, &BGR32, &mut yes), S_OK)?;
    c.eq(yes as u64, 1)?;
    c.hr(converter_init(conv, b, &BGRA32), S_OK)?;
    c.hr(converter_init(conv, b, &BGRA32), WINCODEC_ERR_WRONGSTATE)?;
    let (r, got) = pixels::<8>(conv, 8);
    c.hr(r, S_OK)?;
    c.bytes(&got, &[3, 2, 1, 255, 6, 5, 4, 255])?;
    release(conv);
    release(b);
    Ok(())
}

/// bitmap.c's test_bitmap_scaler, and pixels.
fn scaler(c: &mut Checks, f: Obj) -> Result<(), u32> {
    let (r, b) = create_bitmap(f, 4, 2, &BGR24);
    c.hr(r, S_OK)?;
    let (r, s) = make(f, 11);
    c.hr(r, S_OK)?;
    c.hr(scaler_init(s, null_mut(), 0, 0, NEAREST), E_INVALIDARG)?;
    c.hr(scaler_init(s, b, 0, 0, NEAREST), E_INVALIDARG)?;
    let (mut w, mut h) = (123u32, 321u32);
    c.hr(size_raw(s, null_mut(), &mut h), WINCODEC_ERR_NOTINITIALIZED)?;
    c.hr(size_raw(s, &mut w, null_mut()), WINCODEC_ERR_NOTINITIALIZED)?;
    c.hr(resolution_raw(s, null_mut(), null_mut()), WINCODEC_ERR_NOTINITIALIZED)?;
    let mut x = 0.1;
    c.hr(resolution_raw(s, &mut x, null_mut()), WINCODEC_ERR_NOTINITIALIZED)?;
    c.ok(x == 0.1, x as u64)?;
    c.hr(pixel_format_raw(s, null_mut()), E_INVALIDARG)?;
    let (r, g) = pixel_format(s);
    c.hr(r, S_OK)?;
    c.guid(&g, &DONT_CARE)?;
    c.hr(size_raw(s, &mut w, &mut h), WINCODEC_ERR_NOTINITIALIZED)?;
    c.eq(u64::from(w) << 32 | u64::from(h), 123 << 32 | 321)?;
    c.hr(copy_palette(s, null_mut()), E_INVALIDARG)?;
    let (r, p) = make(f, 9);
    c.hr(r, S_OK)?;
    c.hr(copy_palette(s, p), WINCODEC_ERR_PALETTEUNAVAILABLE)?;
    c.hr(scaler_init(s, b, 4, 0, NEAREST), E_INVALIDARG)?;
    let mut buf = [0u8; 93];
    c.hr(copy_pixels(s, None, 1, &mut buf), WINCODEC_ERR_NOTINITIALIZED)?;
    c.hr(scaler_init(s, b, 0, 2, NEAREST), E_INVALIDARG)?;
    c.hr(scaler_init(s, null_mut(), 8, 4, NEAREST), E_INVALIDARG)?;
    c.hr(scaler_init(s, b, 7, 4, NEAREST), S_OK)?;
    c.hr(scaler_init(s, b, 0, 4, NEAREST), E_INVALIDARG)?;
    c.hr(scaler_init(s, b, 7, 0, NEAREST), E_INVALIDARG)?;
    c.hr(scaler_init(s, null_mut(), 8, 4, NEAREST), E_INVALIDARG)?;
    c.hr(scaler_init(s, b, 7, 4, NEAREST), WINCODEC_ERR_WRONGSTATE)?;
    let (r, w, h) = size(s);
    c.hr(r, S_OK)?;
    c.eq(u64::from(w) << 32 | u64::from(h), 7 << 32 | 4)?;
    c.hr(size_raw(s, null_mut(), &mut h.clone()), E_INVALIDARG)?;
    c.hr(pixel_format_raw(s, null_mut()), E_INVALIDARG)?;
    let (r, g) = pixel_format(s);
    c.hr(r, S_OK)?;
    c.guid(&g, &BGR24)?;
    c.hr(resolution_raw(s, null_mut(), null_mut()), E_INVALIDARG)?;
    let (r, x, y) = resolution(s);
    c.hr(r, S_OK)?;
    c.ok(x == 0.0 && y == 0.0, x as u64)?;
    c.hr(copy_palette(s, p), WINCODEC_ERR_PALETTEUNAVAILABLE)?;
    c.hr(copy_pixels(s, None, 24, &mut buf), S_OK)?;
    release(p);
    release(s);
    release(b);

    // Nearest neighbour doubling a 2x2 BGRA image: 2x2 blocks.
    let px: [u8; 16] = [1, 2, 3, 255, 4, 5, 6, 255, 7, 8, 9, 255, 10, 11, 12, 255];
    let (r, b) = create_from_memory(f, 2, 2, &BGRA32, 8, &px);
    c.hr(r, S_OK)?;
    let (r, s) = make(f, 11);
    c.hr(r, S_OK)?;
    c.hr(scaler_init(s, b, 4, 4, NEAREST), S_OK)?;
    let (r, got) = pixels::<64>(s, 16);
    c.hr(r, S_OK)?;
    for y in 0..4 {
        for x in 0..4 {
            let want = &px[4 * (2 * (y / 2) + x / 2)..][..4];
            c.bytes(&got[16 * y + 4 * x..][..4], want)?;
        }
    }
    release(s);
    // Halving: which pixel nearest neighbour takes.
    let (r, s) = make(f, 11);
    c.hr(r, S_OK)?;
    c.hr(scaler_init(s, b, 1, 1, NEAREST), S_OK)?;
    let (r, got) = pixels::<4>(s, 4);
    c.hr(r, S_OK)?;
    info("NearestNeighbor 2x2 -> 1x1", u32::from_le_bytes(got).into());
    release(s);
    // Linear between two pixels: Windows' weights.
    let (r, s) = make(f, 11);
    c.hr(r, S_OK)?;
    c.hr(scaler_init(s, b, 4, 1, LINEAR), S_OK)?;
    let (r, got) = pixels::<16>(s, 16);
    c.hr(r, S_OK)?;
    info("Linear 2x2 -> 4x1, pixel 1", u32::from_le_bytes([got[4], got[5], got[6], got[7]]).into());
    release(s);
    release(b);

    // A uniform image keeps its colour in every mode.
    let uniform = [0x40u8, 0x80, 0xc0, 0xff].repeat_into::<64>();
    let (r, b) = create_from_memory(f, 4, 4, &BGRA32, 16, &uniform);
    c.hr(r, S_OK)?;
    for (mode, (w, h)) in [(NEAREST, (7, 3)), (LINEAR, (9, 2)), (LINEAR, (2, 2)), (FANT, (3, 5))] {
        let (r, s) = make(f, 11);
        c.hr(r, S_OK)?;
        c.hr(scaler_init(s, b, w, h, mode), S_OK)?;
        let mut out = [0u8; 4 * 9 * 5];
        c.hr(copy_pixels(s, None, 4 * w, &mut out[..(4 * w * h) as usize]), S_OK)?;
        for i in 0..(w * h) as usize {
            c.bytes(&out[4 * i..4 * i + 4], &[0x40, 0x80, 0xc0, 0xff])?;
        }
        release(s);
    }
    release(b);
    Ok(())
}

/// bitmap.c's test_clipper, over a bitmap with content.
fn clipper(c: &mut Checks, f: Obj) -> Result<(), u32> {
    let (r, b) = create_bitmap(f, 10, 10, &BGR24);
    c.hr(r, S_OK)?;
    let (r, k) = make(f, 12);
    c.hr(r, S_OK)?;
    c.hr(clipper_init(k, b, &rect(0, 0, 11, 11)), E_INVALIDARG)?;
    c.hr(clipper_init(k, b, &rect(5, 5, 6, 6)), E_INVALIDARG)?;
    c.hr(clipper_init(k, b, &rect(5, 5, 5, 5)), S_OK)?;
    let (r, w, h) = size(k);
    c.hr(r, S_OK)?;
    c.eq(u64::from(w) << 32 | u64::from(h), 5 << 32 | 5)?;
    release(k);
    release(b);

    // A 4x4 BGRA bitmap whose pixel (x, y) is (x, y, 0, 255).
    let mut px = [0u8; 64];
    for y in 0..4 {
        for x in 0..4 {
            px[16 * y + 4 * x..][..4].copy_from_slice(&[x as u8, y as u8, 0, 255]);
        }
    }
    let (r, b) = create_from_memory(f, 4, 4, &BGRA32, 16, &px);
    c.hr(r, S_OK)?;
    let (r, k) = make(f, 12);
    c.hr(r, S_OK)?;
    c.hr(clipper_init(k, b, &rect(1, 2, 3, 2)), S_OK)?;
    let mut out = [0u8; 8];
    c.hr(copy_pixels(k, Some(&rect(1, 1, 2, 1)), 8, &mut out), S_OK)?;
    c.bytes(&out, &[2, 3, 0, 255, 3, 3, 0, 255])?;
    c.hr(copy_pixels(k, Some(&rect(0, 0, 20, 20)), 80, &mut [0u8; 1600]), E_INVALIDARG)?;
    c.hr(copy_pixels(k, Some(&rect(2, 1, 2, 1)), 8, &mut out), E_INVALIDARG)?;
    let (r, all) = pixels::<24>(k, 12);
    c.hr(r, S_OK)?;
    c.bytes(&all[..4], &[1, 2, 0, 255])?;
    c.bytes(&all[20..], &[3, 3, 0, 255])?;
    release(k);
    release(b);
    Ok(())
}

/// Within 0.01 of 72 dpi (2835 pixels per metre).
fn near72(v: f64) -> bool {
    v > 71.99 && v < 72.01
}

/// `[u8; 4]` repeated into an array.
trait RepeatInto {
    fn repeat_into<const N: usize>(self) -> [u8; N];
}

impl RepeatInto for [u8; 4] {
    fn repeat_into<const N: usize>(self) -> [u8; N] {
        let mut out = [0u8; N];
        for (i, b) in out.iter_mut().enumerate() {
            *b = self[i % 4];
        }
        out
    }
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
