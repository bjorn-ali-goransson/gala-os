//! GDI+'s flat API (gdiplusflat.h) as the Office Deployment Tool's
//! setup.exe draws its window with it (ROADMAP OF): the seventeen
//! functions it delay-loads, with the results Microsoft's documentation
//! gives (the GdiplusStartup, Bitmap, SolidBrush, Graphics and Image
//! pages; the Status values of gdiplustypes.h) and Wine's gdiplus tests,
//! which run on Windows (graphics.c: GdipCreateFromHDC(NULL) is
//! OutOfMemory; image.c: GdipCreateBitmapFromScan0's arguments).
//!
//! - GdiplusStartup: version 1 Ok with a token; version 42
//!   UnsupportedGdiplusVersion.
//! - GdipCreateBitmapFromScan0: a width below 1, a buffer with stride 0
//!   or a stride that is not a multiple of four: InvalidParameter. Over
//!   the caller's 4x3 PixelFormat32bppARGB buffer: the bitmap draws into
//!   that memory; GdipGetImageWidth/Height give 4 and 3.
//! - Filling (SourceOver, the default): an opaque brush's colour exactly,
//!   only in the rectangle; half-transparent blue over white about
//!   0xFF7F7FFF (each channel within 1: rounding is not documented); a
//!   brush of alpha 0 leaves the pixel; a cloned brush fills the same.
//! - GdipLoadImageFromStream of a 2x2 RGBA PNG (red, green / blue,
//!   transparent white; its second row Sub-filtered): 2x2, drawn at its
//!   size onto a transparent bitmap pixel for pixel, the transparent one
//!   leaving the destination as it was; GdipCloneImage is 2x2 too. A
//!   uniform 2x2 bitmap drawn at 4x4 is that colour inside.
//! - GdipCreateFromHDC on a memory DC holding a 4x4 32 bpp DIB section of
//!   white: the fill and the PNG land in the DIB's bits (the colour
//!   bytes; the alpha byte of a DIB pixel is not checked).
//! - NULL objects: InvalidParameter (GdipGetImageWidth, GdipDisposeImage,
//!   GdipDeleteBrush, GdipDeleteGraphics, GdipLoadImageFromStream).
//! - GdipAlloc gives memory GdipFree takes.
//!
//! Pixels are read after GdipDeleteGraphics, so that drawing deferred
//! until then is seen too.
//!
//! Prints `gdiplus_test: ok` (`wow64_gdiplus_test` for the x86 build), or
//! `<name>: check N got X (line L)` and exits with N.

#![no_std]
#![no_main]

use core::ffi::c_void;
use core::ptr::null_mut;

use ntrt as _;

#[cfg(target_arch = "x86_64")]
const NAME: &str = "gdiplus_test";
#[cfg(target_arch = "x86")]
const NAME: &str = "wow64_gdiplus_test";

type Handle = usize;
type Gp = *mut c_void;

#[cfg_attr(
    target_arch = "x86",
    link(name = "kernel32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "kernel32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, overlapped: usize) -> i32;
    fn ExitProcess(code: u32) -> !;
}

#[cfg_attr(
    target_arch = "x86",
    link(name = "gdi32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "gdi32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn CreateCompatibleDC(hdc: Handle) -> Handle;
    fn DeleteDC(hdc: Handle) -> i32;
    fn CreateDIBSection(
        hdc: Handle,
        info: *const BitmapInfoHeader,
        usage: u32,
        bits: *mut *mut u32,
        section: Handle,
        offset: u32,
    ) -> Handle;
    fn SelectObject(hdc: Handle, object: Handle) -> Handle;
    fn DeleteObject(object: Handle) -> i32;
}

#[cfg_attr(
    target_arch = "x86",
    link(name = "shlwapi", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "shlwapi", kind = "raw-dylib"))]
unsafe extern "system" {
    fn SHCreateMemStream(data: *const u8, size: u32) -> *mut c_void;
}

#[cfg_attr(
    target_arch = "x86",
    link(name = "gdiplus", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "gdiplus", kind = "raw-dylib"))]
unsafe extern "system" {
    fn GdiplusStartup(token: *mut usize, input: *const StartupInput, output: *mut c_void) -> u32;
    fn GdiplusShutdown(token: usize);
    fn GdipAlloc(size: usize) -> *mut c_void;
    fn GdipFree(p: *mut c_void);
    fn GdipCreateBitmapFromScan0(
        width: i32,
        height: i32,
        stride: i32,
        format: u32,
        scan0: *mut u32,
        bitmap: *mut Gp,
    ) -> u32;
    fn GdipGetImageWidth(image: Gp, width: *mut u32) -> u32;
    fn GdipGetImageHeight(image: Gp, height: *mut u32) -> u32;
    fn GdipCloneImage(image: Gp, clone: *mut Gp) -> u32;
    fn GdipDisposeImage(image: Gp) -> u32;
    fn GdipLoadImageFromStream(stream: *mut c_void, image: *mut Gp) -> u32;
    fn GdipCreateSolidFill(argb: u32, brush: *mut Gp) -> u32;
    fn GdipCloneBrush(brush: Gp, clone: *mut Gp) -> u32;
    fn GdipDeleteBrush(brush: Gp) -> u32;
    fn GdipCreateFromHDC(hdc: Handle, graphics: *mut Gp) -> u32;
    fn GdipGetImageGraphicsContext(image: Gp, graphics: *mut Gp) -> u32;
    fn GdipDeleteGraphics(graphics: Gp) -> u32;
    fn GdipFillRectangleI(graphics: Gp, brush: Gp, x: i32, y: i32, w: i32, h: i32) -> u32;
    fn GdipDrawImageRectI(graphics: Gp, image: Gp, x: i32, y: i32, w: i32, h: i32) -> u32;
}

#[repr(C)]
struct StartupInput {
    version: u32,
    debug_callback: usize,
    suppress_background_thread: i32,
    suppress_external_codecs: i32,
}

#[repr(C)]
struct BitmapInfoHeader {
    size: u32,
    width: i32,
    height: i32,
    planes: u16,
    bit_count: u16,
    compression: u32,
    size_image: u32,
    x_ppm: i32,
    y_ppm: i32,
    clr_used: u32,
    clr_important: u32,
}

/// Status values (gdiplustypes.h).
const OK: u32 = 0;
const INVALID_PARAMETER: u32 = 2;
const OUT_OF_MEMORY: u32 = 3;
const UNSUPPORTED_GDIPLUS_VERSION: u32 = 17;
const PIXEL_FORMAT_32BPP_ARGB: u32 = 0x0026_200a;

/// A 2x2 RGBA PNG: red, green / blue, white with alpha 0 (the second
/// row Sub-filtered), zlib-compressed.
const PNG: [u8; 78] = [
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x02, 0x08, 0x06, 0x00, 0x00, 0x00, 0x72, 0xb6, 0x0d,
    0x24, 0x00, 0x00, 0x00, 0x15, 0x49, 0x44, 0x41, 0x54, 0x78, 0xda, 0x63, 0xf8, 0xcf, 0xc0, 0xf0,
    0x1f, 0x04, 0x19, 0x81, 0x34, 0x10, 0x30, 0x30, 0x02, 0x00, 0x46, 0xd5, 0x07, 0xfb, 0x58, 0x88,
    0xaa, 0x15, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
];
/// The PNG's pixels as 0xAARRGGBB.
const PNG_PIXELS: [u32; 4] = [0xffff_0000, 0xff00_ff00, 0xff00_00ff, 0x00ff_ffff];

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

    /// A GpStatus that should be `want`.
    #[track_caller]
    fn st(&mut self, got: u32, want: u32) -> Result<(), u32> {
        self.ok(got == want, got.into())
    }

    /// Each 8-bit channel of `got` within 1 of `want`'s.
    #[track_caller]
    fn near(&mut self, got: u32, want: u32) -> Result<(), u32> {
        let close = (0..4).all(|k| {
            let (g, w) = ((got >> (8 * k)) & 0xff, (want >> (8 * k)) & 0xff);
            g.abs_diff(w) <= 1
        });
        self.ok(close, got.into())
    }
}

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    // SAFETY: GDI+ calls with our own buffers and the objects it hands
    // out; the DIB's bits are 16 u32s while it lives.
    unsafe {
        // Startup.
        let mut token = 0usize;
        let mut input = StartupInput {
            version: 42,
            debug_callback: 0,
            suppress_background_thread: 0,
            suppress_external_codecs: 0,
        };
        c.st(GdiplusStartup(&mut token, &input, null_mut()), UNSUPPORTED_GDIPLUS_VERSION)?;
        input.version = 1;
        c.st(GdiplusStartup(&mut token, &input, null_mut()), OK)?;
        c.ok(token != 0, 0)?;

        // Bitmaps over the caller's memory.
        let mut bitmap: Gp = null_mut();
        let mut buf = [0u32; 12];
        let f = PIXEL_FORMAT_32BPP_ARGB;
        c.st(
            GdipCreateBitmapFromScan0(-1, 3, 16, f, buf.as_mut_ptr(), &mut bitmap),
            INVALID_PARAMETER,
        )?;
        c.st(
            GdipCreateBitmapFromScan0(4, 3, 0, f, buf.as_mut_ptr(), &mut bitmap),
            INVALID_PARAMETER,
        )?;
        c.st(
            GdipCreateBitmapFromScan0(4, 3, 18, f, buf.as_mut_ptr(), &mut bitmap),
            INVALID_PARAMETER,
        )?;
        c.st(GdipCreateBitmapFromScan0(4, 3, 16, f, buf.as_mut_ptr(), &mut bitmap), OK)?;
        let (mut w, mut h) = (0u32, 0u32);
        c.st(GdipGetImageWidth(bitmap, &mut w), OK)?;
        c.st(GdipGetImageHeight(bitmap, &mut h), OK)?;
        c.eq(w.into(), 4)?;
        c.eq(h.into(), 3)?;
        c.st(GdipGetImageWidth(null_mut(), &mut w), INVALID_PARAMETER)?;

        // Filling.
        let mut graphics: Gp = null_mut();
        c.st(GdipGetImageGraphicsContext(bitmap, &mut graphics), OK)?;
        let (mut red, mut red2, mut white, mut blue, mut clear): (Gp, Gp, Gp, Gp, Gp) =
            (null_mut(), null_mut(), null_mut(), null_mut(), null_mut());
        c.st(GdipCreateSolidFill(0xffff_0000, &mut red), OK)?;
        c.st(GdipCloneBrush(red, &mut red2), OK)?;
        c.st(GdipCreateSolidFill(0xffff_ffff, &mut white), OK)?;
        c.st(GdipCreateSolidFill(0x8000_00ff, &mut blue), OK)?;
        c.st(GdipCreateSolidFill(0x0012_3456, &mut clear), OK)?;
        c.st(GdipFillRectangleI(graphics, red, 1, 1, 2, 1), OK)?;
        c.st(GdipFillRectangleI(graphics, white, 0, 2, 4, 1), OK)?;
        c.st(GdipFillRectangleI(graphics, blue, 0, 2, 1, 1), OK)?;
        c.st(GdipFillRectangleI(graphics, clear, 1, 2, 1, 1), OK)?;
        c.st(GdipFillRectangleI(graphics, red2, 3, 0, 1, 1), OK)?;
        c.st(GdipDeleteGraphics(graphics), OK)?;
        c.st(GdipDeleteGraphics(null_mut()), INVALID_PARAMETER)?;
        let b = core::ptr::read_volatile(&buf);
        c.eq(b[5].into(), 0xffff_0000)?;
        c.eq(b[6].into(), 0xffff_0000)?;
        c.eq(b[4].into(), 0)?;
        c.eq(b[7].into(), 0)?;
        c.eq(b[0].into(), 0)?;
        c.eq(b[3].into(), 0xffff_0000)?;
        c.near(b[8], 0xff7f_7fff)?;
        c.eq(b[9].into(), 0xffff_ffff)?;
        c.eq(b[11].into(), 0xffff_ffff)?;
        c.st(GdipDisposeImage(bitmap), OK)?;
        c.st(GdipDisposeImage(null_mut()), INVALID_PARAMETER)?;

        // A PNG from a stream.
        let stream = SHCreateMemStream(PNG.as_ptr(), PNG.len() as u32);
        c.ok(!stream.is_null(), 0)?;
        let mut png: Gp = null_mut();
        c.st(GdipLoadImageFromStream(null_mut(), &mut png), INVALID_PARAMETER)?;
        c.st(GdipLoadImageFromStream(stream, &mut png), OK)?;
        c.st(GdipGetImageWidth(png, &mut w), OK)?;
        c.st(GdipGetImageHeight(png, &mut h), OK)?;
        c.eq(w.into(), 2)?;
        c.eq(h.into(), 2)?;
        let mut clone: Gp = null_mut();
        c.st(GdipCloneImage(png, &mut clone), OK)?;
        c.st(GdipGetImageWidth(clone, &mut w), OK)?;
        c.eq(w.into(), 2)?;
        c.st(GdipDisposeImage(clone), OK)?;
        let mut small = [0u32; 4];
        c.st(GdipCreateBitmapFromScan0(2, 2, 8, f, small.as_mut_ptr(), &mut bitmap), OK)?;
        c.st(GdipGetImageGraphicsContext(bitmap, &mut graphics), OK)?;
        c.st(GdipDrawImageRectI(graphics, png, 0, 0, 2, 2), OK)?;
        c.st(GdipDeleteGraphics(graphics), OK)?;
        let s = core::ptr::read_volatile(&small);
        for (got, want) in s.iter().zip([PNG_PIXELS[0], PNG_PIXELS[1], PNG_PIXELS[2], 0]) {
            c.eq((*got).into(), want.into())?;
        }
        c.st(GdipDisposeImage(bitmap), OK)?;

        // A uniform bitmap scaled up: its colour inside.
        let mut green: Gp = null_mut();
        c.st(GdipCreateSolidFill(0xff00_ff00, &mut green), OK)?;
        let mut two: Gp = null_mut();
        c.st(GdipCreateBitmapFromScan0(2, 2, 0, f, null_mut(), &mut two), OK)?;
        c.st(GdipGetImageGraphicsContext(two, &mut graphics), OK)?;
        c.st(GdipFillRectangleI(graphics, green, 0, 0, 2, 2), OK)?;
        c.st(GdipDeleteGraphics(graphics), OK)?;
        let mut big = [0u32; 16];
        c.st(GdipCreateBitmapFromScan0(4, 4, 16, f, big.as_mut_ptr(), &mut bitmap), OK)?;
        c.st(GdipGetImageGraphicsContext(bitmap, &mut graphics), OK)?;
        c.st(GdipDrawImageRectI(graphics, two, 0, 0, 4, 4), OK)?;
        c.st(GdipDeleteGraphics(graphics), OK)?;
        let s = core::ptr::read_volatile(&big);
        c.eq(s[5].into(), 0xff00_ff00)?;
        c.eq(s[10].into(), 0xff00_ff00)?;
        c.st(GdipDisposeImage(bitmap), OK)?;
        c.st(GdipDisposeImage(two), OK)?;

        // A device context: a memory DC holding a 32 bpp DIB section.
        c.st(GdipCreateFromHDC(0, &mut graphics), OUT_OF_MEMORY)?;
        let dc = CreateCompatibleDC(0);
        c.ok(dc != 0, 0)?;
        let header = BitmapInfoHeader {
            size: 40,
            width: 4,
            height: -4,
            planes: 1,
            bit_count: 32,
            compression: 0,
            size_image: 0,
            x_ppm: 0,
            y_ppm: 0,
            clr_used: 0,
            clr_important: 0,
        };
        let mut bits: *mut u32 = null_mut();
        let dib = CreateDIBSection(dc, &header, 0, &mut bits, 0, 0);
        c.ok(dib != 0 && !bits.is_null(), 0)?;
        for i in 0..16 {
            bits.add(i).write_volatile(0x00ff_ffff);
        }
        let old = SelectObject(dc, dib);
        c.st(GdipCreateFromHDC(dc, &mut graphics), OK)?;
        c.st(GdipFillRectangleI(graphics, red, 0, 0, 2, 2), OK)?;
        c.st(GdipDrawImageRectI(graphics, png, 2, 2, 2, 2), OK)?;
        c.st(GdipDeleteGraphics(graphics), OK)?;
        let px = |i: usize| u64::from(bits.add(i).read_volatile() & 0x00ff_ffff);
        c.eq(px(0), 0xff_0000)?;
        c.eq(px(5), 0xff_0000)?;
        c.eq(px(2), 0xff_ffff)?;
        c.eq(px(8), 0xff_ffff)?;
        c.eq(px(10), 0xff_0000)?;
        c.eq(px(11), 0x00_ff00)?;
        c.eq(px(14), 0x00_00ff)?;
        c.eq(px(15), 0xff_ffff)?;
        SelectObject(dc, old);
        DeleteObject(dib);
        DeleteDC(dc);

        for brush in [red, red2, white, blue, clear, green] {
            c.st(GdipDeleteBrush(brush), OK)?;
        }
        c.st(GdipDeleteBrush(null_mut()), INVALID_PARAMETER)?;
        c.st(GdipDisposeImage(png), OK)?;

        let p = GdipAlloc(64);
        c.ok(!p.is_null(), 0)?;
        GdipFree(p);
        GdiplusShutdown(token);
    }
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
