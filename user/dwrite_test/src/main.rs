//! DirectWrite's system font collection, as Chromium's browser and Skia
//! use it (ROADMAP BR), with results any Windows install gives (Arial is
//! always there):
//!
//! 1. DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED, IDWriteFactory);
//! 2. GetSystemFontCollection: at least one family; FindFamilyName("Arial")
//!    exists, a made-up name does not;
//! 3. the family's names (en-us) and its regular (400) font: weight 400,
//!    normal stretch and style, HasCharacter('A');
//! 4. the font's face: TrueType, one file, metrics (units per em, ascent,
//!    descent), a glyph for 'A' with a positive advance, a 'cmap' table;
//! 5. the face's file: its loader is a local file loader whose path for
//!    the file's key ends in ".ttf" (the file the font came from), and a
//!    stream over it as long as the file;
//! 6. GetFontFromFontFace gives a font of the same weight;
//! 7. a custom font collection, as Chromium's renderer builds one (its
//!    font proxy): the test's own collection loader enumerates one file,
//!    a custom file reference (CreateCustomFontFileReference) over the
//!    test's own file loader, whose stream serves step 5's file from
//!    memory. CreateCustomFontCollection gives one family; its first
//!    font's face has as many glyphs as the system's face of that file;
//! 8. glyph run analysis (CreateGlyphRunAnalysis) of 'A' at 32 px: in
//!    NATURAL rendering mode the ClearType 3x1 texture has bounds (the
//!    aliased texture's are empty), at most 32 px tall, above the
//!    baseline; CreateAlphaTexture fills some coverage; ALIASED mode gives
//!    an aliased texture (0 or 255) and an empty ClearType one; OUTLINE
//!    mode is E_INVALIDARG;
//! 9. the face's GetGlyphRunOutline for 'A' (em 32) drives a geometry
//!    sink: closed figures with lines or Béziers, above the baseline
//!    (y < 0); the sink is not closed (its owner closes it, as Direct2D's
//!    samples do after GetGlyphRunOutline).

#![no_std]
#![no_main]

use core::ffi::c_void;

type HResult = i32;
type Iid = [u8; 16];

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetStdHandle(which: u32) -> *mut c_void;
    fn WriteFile(
        h: *mut c_void,
        buf: *const u8,
        len: u32,
        written: *mut u32,
        ov: *mut c_void,
    ) -> i32;
    fn ExitProcess(code: u32) -> !;
}

#[link(name = "dwrite", kind = "raw-dylib")]
unsafe extern "system" {
    fn DWriteCreateFactory(kind: u32, iid: *const Iid, factory: *mut *mut c_void) -> HResult;
}

#[unsafe(no_mangle)]
pub static _fltused: i32 = 0;

#[unsafe(no_mangle)]
pub extern "C" fn __CxxFrameHandler3() -> ! {
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xdeac) }
}

#[allow(dead_code)]
#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "C" {
    fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memset(dst: *mut u8, value: i32, n: usize) -> *mut u8;
    fn memcmp(a: *const u8, b: *const u8, n: usize) -> i32;
}

const fn guid(d1: u32, d2: u16, d3: u16, d4: [u8; 8]) -> Iid {
    let a = d1.to_le_bytes();
    let b = d2.to_le_bytes();
    let c = d3.to_le_bytes();
    [
        a[0], a[1], a[2], a[3], b[0], b[1], c[0], c[1], d4[0], d4[1], d4[2], d4[3], d4[4], d4[5],
        d4[6], d4[7],
    ]
}

const IID_FACTORY: Iid =
    guid(0xb859ee5a, 0xd838, 0x4b5b, [0xa2, 0xe8, 0x1a, 0xdc, 0x7d, 0x93, 0xdb, 0x48]);
const IID_LOCAL_LOADER: Iid =
    guid(0xb2d9f3ec, 0xc9fe, 0x4a11, [0xa2, 0xec, 0xd8, 0x62, 0x08, 0xf7, 0xc0, 0xa2]);

fn out(text: &[u8]) {
    let mut written = 0;
    // SAFETY: valid buffer and output pointer.
    unsafe {
        WriteFile(
            GetStdHandle(-11i32 as u32),
            text.as_ptr(),
            text.len() as u32,
            &mut written,
            core::ptr::null_mut(),
        )
    };
}

fn fail(n: u32) -> ! {
    let mut line = *b"dwrite_test: check 00 failed\r\n";
    line[19] = b'0' + (n / 10) as u8;
    line[20] = b'0' + (n % 10) as u8;
    out(&line);
    // SAFETY: ends the process.
    unsafe { ExitProcess(n) }
}

fn check(ok: bool, n: u32) {
    if !ok {
        fail(n);
    }
}

/// Method `slot` of COM object `this`, as a function of type `F`.
///
/// # Safety
/// `this` is a COM object whose vtable has that slot with that type.
unsafe fn method<F: Copy>(this: *mut c_void, slot: usize) -> F {
    // SAFETY: as the caller promises.
    unsafe {
        let vtbl = *this.cast::<*const usize>();
        core::mem::transmute_copy(&*vtbl.add(slot))
    }
}

type GetPtr = extern "system" fn(*mut c_void, *mut *mut c_void) -> HResult;
type GetIndexed = extern "system" fn(*mut c_void, u32, *mut *mut c_void) -> HResult;
type GetU32 = extern "system" fn(*mut c_void) -> u32;
type GetPtr1 = extern "system" fn(*mut c_void, *mut c_void) -> HResult;

/// Step 8's texture buffer (static: a frame over a page would need a
/// stack probe).
static mut ALPHA: [u8; 3 * 48 * 48] = [0; 3 * 48 * 48];

/// Step 9's ID2D1SimplifiedGeometrySink: counts what it is told.
mod sink {
    use super::{HResult, Iid};
    use core::ffi::c_void;
    use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

    pub static FIGURES: AtomicU32 = AtomicU32::new(0);
    pub static ENDED: AtomicU32 = AtomicU32::new(0);
    pub static SEGMENTS: AtomicU32 = AtomicU32::new(0);
    pub static CLOSED: AtomicU32 = AtomicU32::new(0);
    /// Some point is above the baseline (y < 0: y grows downward).
    pub static ABOVE: AtomicBool = AtomicBool::new(false);

    #[repr(C)]
    #[derive(Clone, Copy)]
    pub struct Point {
        x: f32,
        y: f32,
    }

    #[repr(C)]
    struct Obj(*const ());
    struct VTable([*const (); 10]);
    // SAFETY (both): pointers to a static vtable and functions.
    unsafe impl Sync for Obj {}
    unsafe impl Sync for VTable {}

    static VTBL: VTable = VTable([
        qi as *const (),
        add_ref as *const (),
        release as *const (),
        set_fill_mode as *const (),
        set_segment_flags as *const (),
        begin_figure as *const (),
        add_lines as *const (),
        add_beziers as *const (),
        end_figure as *const (),
        close as *const (),
    ]);
    static SINK: Obj = Obj(VTBL.0.as_ptr().cast());

    pub fn sink() -> *mut c_void {
        (&raw const SINK).cast_mut().cast()
    }

    fn see(p: Point) {
        if p.y < -1.0 {
            ABOVE.store(true, Ordering::Relaxed);
        }
    }

    extern "system" fn qi(this: *mut c_void, _iid: *const Iid, out: *mut *mut c_void) -> HResult {
        // SAFETY: the caller's output.
        unsafe { *out = this };
        0
    }
    extern "system" fn add_ref(_: *mut c_void) -> u32 {
        1
    }
    extern "system" fn release(_: *mut c_void) -> u32 {
        1
    }
    extern "system" fn set_fill_mode(_: *mut c_void, _: u32) {}
    extern "system" fn set_segment_flags(_: *mut c_void, _: u32) {}
    extern "system" fn begin_figure(_: *mut c_void, start: Point, _: u32) {
        FIGURES.fetch_add(1, Ordering::Relaxed);
        see(start);
    }
    extern "system" fn add_lines(_: *mut c_void, points: *const Point, count: u32) {
        SEGMENTS.fetch_add(count, Ordering::Relaxed);
        for i in 0..count as usize {
            // SAFETY: the caller's `count` points.
            see(unsafe { *points.add(i) });
        }
    }
    extern "system" fn add_beziers(_: *mut c_void, beziers: *const [Point; 3], count: u32) {
        SEGMENTS.fetch_add(count, Ordering::Relaxed);
        for i in 0..count as usize {
            // SAFETY: the caller's `count` segments.
            see(unsafe { (*beziers.add(i))[2] });
        }
    }
    extern "system" fn end_figure(_: *mut c_void, _: u32) {
        ENDED.fetch_add(1, Ordering::Relaxed);
    }
    extern "system" fn close(_: *mut c_void) -> HResult {
        CLOSED.fetch_add(1, Ordering::Relaxed);
        0
    }
}

/// Step 7's objects: a collection loader whose enumerator gives one custom
/// file reference (key: the number 1) over a file loader whose stream
/// serves `DATA` (the font file's bytes). Static objects: AddRef and
/// Release count nothing.
mod custom {
    use super::{HResult, Iid};
    use core::ffi::c_void;
    use core::sync::atomic::{AtomicPtr, AtomicU32, AtomicUsize, Ordering};

    pub static DATA: AtomicPtr<u8> = AtomicPtr::new(core::ptr::null_mut());
    pub static LEN: AtomicUsize = AtomicUsize::new(0);
    pub static FACTORY: AtomicPtr<c_void> = AtomicPtr::new(core::ptr::null_mut());
    /// The enumerator's position: 0 before the file, 1 on it, 2 past it.
    static POSITION: AtomicU32 = AtomicU32::new(0);
    static FILE_KEY: u32 = 1;

    #[repr(C)]
    struct Obj(*const ());
    struct VTable<const N: usize>([*const (); N]);
    // SAFETY (both): pointers to static vtables and functions.
    unsafe impl Sync for Obj {}
    unsafe impl<const N: usize> Sync for VTable<N> {}

    static FILE_LOADER_VTBL: VTable<4> = VTable([
        qi as *const (),
        add_ref as *const (),
        release as *const (),
        create_stream as *const (),
    ]);
    static STREAM_VTBL: VTable<7> = VTable([
        qi as *const (),
        add_ref as *const (),
        release as *const (),
        read_fragment as *const (),
        release_fragment as *const (),
        file_size as *const (),
        write_time as *const (),
    ]);
    static COLLECTION_LOADER_VTBL: VTable<4> = VTable([
        qi as *const (),
        add_ref as *const (),
        release as *const (),
        create_enumerator as *const (),
    ]);
    static ENUMERATOR_VTBL: VTable<5> = VTable([
        qi as *const (),
        add_ref as *const (),
        release as *const (),
        move_next as *const (),
        current as *const (),
    ]);

    static FILE_LOADER: Obj = Obj(FILE_LOADER_VTBL.0.as_ptr().cast());
    static STREAM: Obj = Obj(STREAM_VTBL.0.as_ptr().cast());
    static COLLECTION_LOADER: Obj = Obj(COLLECTION_LOADER_VTBL.0.as_ptr().cast());
    static ENUMERATOR: Obj = Obj(ENUMERATOR_VTBL.0.as_ptr().cast());

    pub fn file_loader() -> *mut c_void {
        (&raw const FILE_LOADER).cast_mut().cast()
    }
    pub fn collection_loader() -> *mut c_void {
        (&raw const COLLECTION_LOADER).cast_mut().cast()
    }

    extern "system" fn qi(this: *mut c_void, _iid: *const Iid, out: *mut *mut c_void) -> HResult {
        // SAFETY: the caller's output.
        unsafe { *out = this };
        0
    }
    extern "system" fn add_ref(_: *mut c_void) -> u32 {
        1
    }
    extern "system" fn release(_: *mut c_void) -> u32 {
        1
    }
    extern "system" fn create_stream(
        _: *mut c_void,
        key: *const u32,
        size: u32,
        out: *mut *mut c_void,
    ) -> HResult {
        // SAFETY: the reference key handed out below.
        if size != 4 || unsafe { *key } != FILE_KEY {
            return 0x8000_4005_u32 as i32; // E_FAIL
        }
        // SAFETY: the caller's output.
        unsafe { *out = (&raw const STREAM).cast_mut().cast() };
        0
    }
    extern "system" fn read_fragment(
        _: *mut c_void,
        start: *mut *const u8,
        offset: u64,
        size: u64,
        context: *mut *mut c_void,
    ) -> HResult {
        let len = LEN.load(Ordering::Relaxed) as u64;
        if offset.checked_add(size).is_none_or(|end| end > len) {
            return 0x8000_4005_u32 as i32;
        }
        // SAFETY: inside the file's bytes; the caller's outputs.
        unsafe {
            *start = DATA.load(Ordering::Relaxed).add(offset as usize);
            *context = core::ptr::null_mut();
        }
        0
    }
    extern "system" fn release_fragment(_: *mut c_void, _: *mut c_void) {}
    extern "system" fn file_size(_: *mut c_void, size: *mut u64) -> HResult {
        // SAFETY: the caller's output.
        unsafe { *size = LEN.load(Ordering::Relaxed) as u64 };
        0
    }
    extern "system" fn write_time(_: *mut c_void, time: *mut u64) -> HResult {
        // SAFETY: the caller's output.
        unsafe { *time = 0 };
        0
    }
    extern "system" fn create_enumerator(
        _: *mut c_void,
        _factory: *mut c_void,
        key: *const u32,
        size: u32,
        out: *mut *mut c_void,
    ) -> HResult {
        // SAFETY: the test's collection key.
        if size != 4 || unsafe { *key } != 7 {
            return 0x8007_0057_u32 as i32; // E_INVALIDARG
        }
        POSITION.store(0, Ordering::Relaxed);
        // SAFETY: the caller's output.
        unsafe { *out = (&raw const ENUMERATOR).cast_mut().cast() };
        0
    }
    extern "system" fn move_next(_: *mut c_void, has: *mut i32) -> HResult {
        let p = POSITION.fetch_add(1, Ordering::Relaxed) + 1;
        // SAFETY: the caller's output.
        unsafe { *has = i32::from(p == 1) };
        0
    }
    extern "system" fn current(_: *mut c_void, file: *mut *mut c_void) -> HResult {
        if POSITION.load(Ordering::Relaxed) != 1 {
            return 0x8000_4005_u32 as i32;
        }
        let factory = FACTORY.load(Ordering::Relaxed);
        // SAFETY: IDWriteFactory::CreateCustomFontFileReference (slot 8).
        unsafe {
            let create: extern "system" fn(
                *mut c_void,
                *const c_void,
                u32,
                *mut c_void,
                *mut *mut c_void,
            ) -> HResult = super::method(factory, 8);
            create(factory, (&raw const FILE_KEY).cast(), 4, file_loader(), file)
        }
    }
}

fn w(s: &str, buf: &mut [u16; 64]) -> *const u16 {
    for (i, c) in s.bytes().enumerate() {
        buf[i] = u16::from(c);
    }
    buf[s.len()] = 0;
    buf.as_ptr()
}

fn tests() {
    // SAFETY (whole function): documented COM calls through each
    // interface's vtable (slot numbers from dwrite.h), valid arguments.
    unsafe {
        // 1.
        let mut factory: *mut c_void = core::ptr::null_mut();
        check(DWriteCreateFactory(0, &IID_FACTORY, &mut factory) == 0 && !factory.is_null(), 1);

        // 2. IDWriteFactory::GetSystemFontCollection (3).
        let mut collection: *mut c_void = core::ptr::null_mut();
        let get_collection: extern "system" fn(*mut c_void, *mut *mut c_void, i32) -> HResult =
            method(factory, 3);
        check(get_collection(factory, &mut collection, 0) == 0 && !collection.is_null(), 2);
        let count: GetU32 = method(collection, 3);
        check(count(collection) >= 1, 2);
        let find: extern "system" fn(*mut c_void, *const u16, *mut u32, *mut i32) -> HResult =
            method(collection, 5);
        let (mut name, mut index, mut exists) = ([0u16; 64], 0u32, 0i32);
        check(find(collection, w("Arial", &mut name), &mut index, &mut exists) == 0, 2);
        check(exists != 0, 2);
        let mut other = 0i32;
        let mut ignored = 0u32;
        check(
            find(collection, w("No Such Family 42", &mut name), &mut ignored, &mut other) == 0,
            2,
        );
        check(other == 0, 2);

        // 3. The family (GetFontFamily, 4), its names (6) and regular font
        // (GetFirstMatchingFont, 7).
        let mut family: *mut c_void = core::ptr::null_mut();
        let get_family: GetIndexed = method(collection, 4);
        check(get_family(collection, index, &mut family) == 0 && !family.is_null(), 3);
        let mut names: *mut c_void = core::ptr::null_mut();
        let family_names: GetPtr = method(family, 6);
        check(family_names(family, &mut names) == 0, 3);
        let strings_count: GetU32 = method(names, 3);
        check(strings_count(names) >= 1, 3);
        let mut length = 0u32;
        let string_length: extern "system" fn(*mut c_void, u32, *mut u32) -> HResult =
            method(names, 7);
        check(string_length(names, 0, &mut length) == 0 && length > 0, 3);
        let first: extern "system" fn(*mut c_void, u32, u32, u32, *mut *mut c_void) -> HResult =
            method(family, 7);
        let mut font: *mut c_void = core::ptr::null_mut();
        check(first(family, 400, 5, 0, &mut font) == 0 && !font.is_null(), 3);
        let weight: GetU32 = method(font, 4);
        let stretch: GetU32 = method(font, 5);
        let style: GetU32 = method(font, 6);
        check(weight(font) == 400 && stretch(font) == 5 && style(font) == 0, 3);
        let has: extern "system" fn(*mut c_void, u32, *mut i32) -> HResult = method(font, 12);
        let mut has_a = 0;
        check(has(font, u32::from(b'A'), &mut has_a) == 0 && has_a != 0, 3);

        // 4. The face (IDWriteFont::CreateFontFace, 13).
        let mut face: *mut c_void = core::ptr::null_mut();
        let create_face: GetPtr = method(font, 13);
        check(create_face(font, &mut face) == 0 && !face.is_null(), 4);
        let face_type: GetU32 = method(face, 3);
        check(face_type(face) == 1, 4); // DWRITE_FONT_FACE_TYPE_TRUETYPE
        let mut metrics = [0u16; 10];
        let get_metrics: extern "system" fn(*mut c_void, *mut [u16; 10]) = method(face, 8);
        get_metrics(face, &mut metrics);
        check(metrics[0] >= 16 && metrics[1] > 0 && metrics[2] > 0, 4);
        let indices: extern "system" fn(*mut c_void, *const u32, u32, *mut u16) -> HResult =
            method(face, 11);
        let a = u32::from(b'A');
        let mut glyph = 0u16;
        check(indices(face, &a, 1, &mut glyph) == 0 && glyph != 0, 4);
        let glyph_metrics: extern "system" fn(
            *mut c_void,
            *const u16,
            u32,
            *mut [i32; 7],
            i32,
        ) -> HResult = method(face, 10);
        let mut gm = [0i32; 7];
        check(glyph_metrics(face, &glyph, 1, &mut gm, 0) == 0 && gm[1] > 0, 4);
        let table: extern "system" fn(
            *mut c_void,
            u32,
            *mut *const u8,
            *mut u32,
            *mut *mut c_void,
            *mut i32,
        ) -> HResult = method(face, 12);
        let cmap = u32::from_le_bytes(*b"cmap");
        let (mut data, mut size, mut context, mut found) =
            (core::ptr::null(), 0u32, core::ptr::null_mut(), 0i32);
        check(table(face, cmap, &mut data, &mut size, &mut context, &mut found) == 0, 4);
        check(found != 0 && size > 4 && !data.is_null(), 4);
        let release_table: extern "system" fn(*mut c_void, *mut c_void) = method(face, 13);
        release_table(face, context);

        // 5. The file (GetFiles, 4), its key (3) and loader (4).
        let files: extern "system" fn(*mut c_void, *mut u32, *mut *mut c_void) -> HResult =
            method(face, 4);
        let mut n = 0u32;
        check(files(face, &mut n, core::ptr::null_mut()) == 0 && n == 1, 5);
        let mut file: *mut c_void = core::ptr::null_mut();
        check(files(face, &mut n, &mut file) == 0 && !file.is_null(), 5);
        let key_of: extern "system" fn(*mut c_void, *mut *const c_void, *mut u32) -> HResult =
            method(file, 3);
        let (mut key, mut key_size) = (core::ptr::null(), 0u32);
        check(key_of(file, &mut key, &mut key_size) == 0, 5);
        let mut loader: *mut c_void = core::ptr::null_mut();
        let get_loader: GetPtr = method(file, 4);
        check(get_loader(file, &mut loader) == 0 && !loader.is_null(), 5);
        let qi: extern "system" fn(*mut c_void, *const Iid, *mut *mut c_void) -> HResult =
            method(loader, 0);
        let mut local: *mut c_void = core::ptr::null_mut();
        check(qi(loader, &IID_LOCAL_LOADER, &mut local) == 0 && !local.is_null(), 5);
        let path_of: extern "system" fn(*mut c_void, *const c_void, u32, *mut u16, u32) -> HResult =
            method(local, 5);
        let mut path = [0u16; 300];
        check(path_of(local, key, key_size, path.as_mut_ptr(), 300) == 0, 5);
        let len = path.iter().position(|&c| c == 0).unwrap_or(0);
        let ext: [u16; 4] = core::array::from_fn(|i| u16::from(b".ttf"[i]));
        let tail: [u16; 4] =
            core::array::from_fn(|i| path[len.saturating_sub(4) + i] | (0x20 * u16::from(i > 0)));
        check(len > 4 && tail == ext, 5);
        let stream_of: extern "system" fn(
            *mut c_void,
            *const c_void,
            u32,
            *mut *mut c_void,
        ) -> HResult = method(local, 3);
        let mut stream: *mut c_void = core::ptr::null_mut();
        check(stream_of(local, key, key_size, &mut stream) == 0 && !stream.is_null(), 5);
        let stream_size: extern "system" fn(*mut c_void, *mut u64) -> HResult = method(stream, 5);
        let mut file_size = 0u64;
        check(stream_size(stream, &mut file_size) == 0 && file_size > u64::from(size), 5);

        // 6. GetFontFromFontFace (6).
        let from_face: extern "system" fn(*mut c_void, *mut c_void, *mut *mut c_void) -> HResult =
            method(collection, 6);
        let mut again: *mut c_void = core::ptr::null_mut();
        check(from_face(collection, face, &mut again) == 0 && !again.is_null(), 6);
        check(weight(again) == 400, 6);

        // 7. The file's bytes (the local stream's fragment, slot 3) back
        // the test's own stream.
        let fragment: extern "system" fn(
            *mut c_void,
            *mut *const u8,
            u64,
            u64,
            *mut *mut c_void,
        ) -> HResult = method(stream, 3);
        let (mut bytes, mut fragment_context) = (core::ptr::null(), core::ptr::null_mut());
        check(fragment(stream, &mut bytes, 0, file_size, &mut fragment_context) == 0, 7);
        custom::DATA.store(bytes.cast_mut(), core::sync::atomic::Ordering::Relaxed);
        custom::LEN.store(file_size as usize, core::sync::atomic::Ordering::Relaxed);
        custom::FACTORY.store(factory, core::sync::atomic::Ordering::Relaxed);
        // RegisterFontFileLoader (13), RegisterFontCollectionLoader (5).
        let register: GetPtr1 = method(factory, 13);
        check(register(factory, custom::file_loader()) == 0, 7);
        let register: GetPtr1 = method(factory, 5);
        check(register(factory, custom::collection_loader()) == 0, 7);
        // CreateCustomFontCollection (4).
        let create: extern "system" fn(
            *mut c_void,
            *mut c_void,
            *const c_void,
            u32,
            *mut *mut c_void,
        ) -> HResult = method(factory, 4);
        let key = 7u32;
        let mut custom_collection: *mut c_void = core::ptr::null_mut();
        let hr = create(
            factory,
            custom::collection_loader(),
            (&raw const key).cast(),
            4,
            &mut custom_collection,
        );
        check(hr == 0 && !custom_collection.is_null(), 7);
        check(count(custom_collection) == 1, 7);
        let mut custom_family: *mut c_void = core::ptr::null_mut();
        let get_family: GetIndexed = method(custom_collection, 4);
        check(get_family(custom_collection, 0, &mut custom_family) == 0, 7);
        let font_count: GetU32 = method(custom_family, 4);
        check(font_count(custom_family) >= 1, 7);
        let get_font: GetIndexed = method(custom_family, 5);
        let mut custom_font: *mut c_void = core::ptr::null_mut();
        check(get_font(custom_family, 0, &mut custom_font) == 0 && !custom_font.is_null(), 7);
        let mut custom_face: *mut c_void = core::ptr::null_mut();
        let create_face: GetPtr = method(custom_font, 13);
        check(create_face(custom_font, &mut custom_face) == 0 && !custom_face.is_null(), 7);
        let glyphs: extern "system" fn(*mut c_void) -> u16 = method(face, 9);
        let custom_glyphs: extern "system" fn(*mut c_void) -> u16 = method(custom_face, 9);
        check(glyphs(face) > 0 && custom_glyphs(custom_face) == glyphs(face), 7);
        let release_fragment: extern "system" fn(*mut c_void, *mut c_void) = method(stream, 4);
        for o in [custom_face, custom_font, custom_family, custom_collection] {
            let release: GetU32 = method(o, 2);
            release(o);
        }
        // UnregisterFontCollectionLoader (6), UnregisterFontFileLoader (14).
        let unregister: GetPtr1 = method(factory, 6);
        check(unregister(factory, custom::collection_loader()) == 0, 7);
        let unregister: GetPtr1 = method(factory, 14);
        check(unregister(factory, custom::file_loader()) == 0, 7);
        release_fragment(stream, fragment_context);

        // 8. CreateGlyphRunAnalysis (23): DWRITE_GLYPH_RUN (face, em
        // size, count, indices, advances, offsets, sideways, bidi level).
        #[repr(C)]
        struct GlyphRun {
            face: *mut c_void,
            em: f32,
            count: u32,
            indices: *const u16,
            advances: *const f32,
            offsets: *const c_void,
            sideways: i32,
            bidi: u32,
        }
        let run = GlyphRun {
            face,
            em: 32.0,
            count: 1,
            indices: &glyph,
            advances: core::ptr::null(),
            offsets: core::ptr::null(),
            sideways: 0,
            bidi: 0,
        };
        let analyze: extern "system" fn(
            *mut c_void,
            *const GlyphRun,
            f32,
            *const c_void,
            u32,
            u32,
            f32,
            f32,
            *mut *mut c_void,
        ) -> HResult = method(factory, 23);
        let mut analysis: *mut c_void = core::ptr::null_mut();
        // NATURAL (4), measuring NATURAL (0), baseline at (10, 40).
        let hr = analyze(factory, &run, 1.0, core::ptr::null(), 4, 0, 10.0, 40.0, &mut analysis);
        check(hr == 0 && !analysis.is_null(), 8);
        let bounds: extern "system" fn(*mut c_void, u32, *mut [i32; 4]) -> HResult =
            method(analysis, 3);
        let mut aliased = [1i32; 4];
        check(bounds(analysis, 0, &mut aliased) == 0, 8);
        check(aliased[0] >= aliased[2] || aliased[1] >= aliased[3], 8);
        let mut rect = [0i32; 4];
        check(bounds(analysis, 1, &mut rect) == 0, 8);
        let (width, height) = (rect[2] - rect[0], rect[3] - rect[1]);
        check(width > 0 && height > 0 && height <= 33 && rect[3] <= 41 && rect[0] >= 9, 8);
        let texture: extern "system" fn(
            *mut c_void,
            u32,
            *const [i32; 4],
            *mut u8,
            u32,
        ) -> HResult = method(analysis, 4);
        // One thread; the only reference to ALPHA (inside the function's
        // unsafe block).
        let alpha = core::slice::from_raw_parts_mut((&raw mut ALPHA).cast::<u8>(), 3 * 48 * 48);
        let size = (3 * width * height) as u32;
        check(size as usize <= alpha.len(), 8);
        check(texture(analysis, 1, &rect, alpha.as_mut_ptr(), size) == 0, 8);
        check(alpha[..size as usize].iter().any(|&a| a > 0), 8);
        // A buffer too small is E_NOT_SUFFICIENT_BUFFER.
        check(texture(analysis, 1, &rect, alpha.as_mut_ptr(), size - 1) != 0, 8);
        let release: GetU32 = method(analysis, 2);
        release(analysis);
        // ALIASED (1): an aliased texture of 0 and 255.
        let mut analysis: *mut c_void = core::ptr::null_mut();
        let hr = analyze(factory, &run, 1.0, core::ptr::null(), 1, 0, 10.0, 40.0, &mut analysis);
        check(hr == 0 && !analysis.is_null(), 8);
        let bounds: extern "system" fn(*mut c_void, u32, *mut [i32; 4]) -> HResult =
            method(analysis, 3);
        let mut clear_type = [1i32; 4];
        check(bounds(analysis, 1, &mut clear_type) == 0, 8);
        check(clear_type[0] >= clear_type[2] || clear_type[1] >= clear_type[3], 8);
        check(bounds(analysis, 0, &mut rect) == 0, 8);
        let (width, height) = (rect[2] - rect[0], rect[3] - rect[1]);
        check(width > 0 && height > 0, 8);
        let texture: extern "system" fn(
            *mut c_void,
            u32,
            *const [i32; 4],
            *mut u8,
            u32,
        ) -> HResult = method(analysis, 4);
        let size = (width * height) as u32;
        check(texture(analysis, 0, &rect, alpha.as_mut_ptr(), size) == 0, 8);
        let used = &alpha[..size as usize];
        check(used.contains(&255) && used.iter().all(|&a| a == 0 || a == 255), 8);
        let release: GetU32 = method(analysis, 2);
        release(analysis);
        // OUTLINE (6) cannot be analyzed.
        let mut none: *mut c_void = core::ptr::null_mut();
        let hr = analyze(factory, &run, 1.0, core::ptr::null(), 6, 0, 0.0, 0.0, &mut none);
        check(hr == 0x8007_0057_u32 as i32 && none.is_null(), 8);

        // 9. GetGlyphRunOutline (14) into the test's sink.
        let outline: extern "system" fn(
            *mut c_void,
            f32,
            *const u16,
            *const f32,
            *const c_void,
            u32,
            i32,
            i32,
            *mut c_void,
        ) -> HResult = method(face, 14);
        let hr = outline(
            face,
            32.0,
            &glyph,
            core::ptr::null(),
            core::ptr::null(),
            1,
            0,
            0,
            sink::sink(),
        );
        check(hr == 0, 9);
        let figures = sink::FIGURES.load(core::sync::atomic::Ordering::Relaxed);
        let ended = sink::ENDED.load(core::sync::atomic::Ordering::Relaxed);
        let segments = sink::SEGMENTS.load(core::sync::atomic::Ordering::Relaxed);
        check(figures >= 1 && ended == figures && segments >= 3, 9);
        check(sink::ABOVE.load(core::sync::atomic::Ordering::Relaxed), 9);
        check(sink::CLOSED.load(core::sync::atomic::Ordering::Relaxed) == 0, 9);

        // Release what we were given (each Release is slot 2).
        for o in
            [again, stream, local, loader, file, face, font, names, family, collection, factory]
        {
            let release: GetU32 = method(o, 2);
            release(o);
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    tests();
    out(b"dwrite_test: ok\r\n");
    // SAFETY: ends the process.
    unsafe { ExitProcess(0) }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xdead) }
}
