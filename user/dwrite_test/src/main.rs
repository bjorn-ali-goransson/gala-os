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
//! 6. GetFontFromFontFace gives a font of the same weight.

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
            core::array::from_fn(|i| path[len.saturating_sub(4) + i] | 0x20 * u16::from(i > 0));
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
