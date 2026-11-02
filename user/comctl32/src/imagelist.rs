//! Image lists: each image is a pair of gdi32 bitmaps of the list's
//! size, a 32-bit colour bitmap (alpha bytes kept) and a monochrome mask
//! (white: transparent), cut from the bitmaps programs add by gdi32's
//! GalaCutImage (which also blacks out the colour under the mask) and
//! drawn by its GalaDrawIcon (alpha when the colour has any, else the
//! mask). A list without ILC_MASK draws opaque. HIMAGELIST is
//! 0x1000 + 16 * slot.

use alloc::vec::Vec;
use core::cell::UnsafeCell;
use core::ffi::c_void;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn LoadLibraryW(name: *const u16) -> usize;
    fn GetProcAddress(module: usize, name: *const u8) -> *const c_void;
}

const ILC_MASK: u32 = 1;
const CLR_NONE: u32 = 0xffff_ffff;
const ILD_TRANSPARENT: u32 = 1;
const ILD_MASK: u32 = 0x10;
const ILCF_SWAP: u32 = 1;
const MAX_LISTS: usize = 256;

struct List {
    cx: i32,
    cy: i32,
    flags: u32,
    background: u32,
    /// (colour, mask) bitmaps.
    images: Vec<(usize, usize)>,
}

struct Table {
    lock: AtomicBool,
    lists: UnsafeCell<[Option<List>; MAX_LISTS]>,
}

// SAFETY: `lists` is only touched under `lock`.
unsafe impl Sync for Table {}

static TABLE: Table =
    Table { lock: AtomicBool::new(false), lists: UnsafeCell::new([const { None }; MAX_LISTS]) };

/// Runs `f` on the list `h` names, under the table's lock (gdi32 calls
/// made inside take only gdi32's own lock).
fn with<R>(h: usize, f: impl FnOnce(&mut List) -> R) -> Option<R> {
    let slot = h.checked_sub(0x1000).filter(|v| v % 16 == 0).map(|v| v / 16)?;
    if slot >= MAX_LISTS {
        return None;
    }
    while TABLE.lock.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() {
        core::hint::spin_loop();
    }
    // SAFETY: under the lock.
    let lists = unsafe { &mut *TABLE.lists.get() };
    let r = lists[slot].as_mut().map(f);
    TABLE.lock.store(false, Ordering::Release);
    r
}

/// `dll`'s function `name` (both NUL-terminated), resolved once per
/// call site.
pub(crate) fn resolve(cache: &AtomicUsize, dll: &[u8], name: &[u8]) -> usize {
    let f = cache.load(Ordering::Acquire);
    if f != 0 {
        return f;
    }
    let mut wide = [0u16; 16];
    for (w, &b) in wide.iter_mut().zip(dll) {
        *w = u16::from(b);
    }
    // SAFETY: NUL-terminated names.
    let f = unsafe {
        let m = LoadLibraryW(wide.as_ptr());
        if m == 0 { 0 } else { GetProcAddress(m, name.as_ptr()) as usize }
    };
    cache.store(f, Ordering::Release);
    f
}

macro_rules! import {
    ($dll:literal, $name:literal, $ty:ty) => {{
        static CACHE: AtomicUsize = AtomicUsize::new(0);
        let f = resolve(&CACHE, concat!($dll, "\0").as_bytes(), concat!($name, "\0").as_bytes());
        // SAFETY: that DLL's function of that name has this signature.
        (f != 0).then(|| unsafe { core::mem::transmute::<usize, $ty>(f) })
    }};
}

type CutImage = extern "system" fn(
    usize,
    i32,
    i32,
    i32,
    i32,
    usize,
    i32,
    i32,
    u32,
    i32,
    i32,
    *mut [usize; 2],
) -> i32;

/// gdi32's GalaCutImage: the list-sized image at (sx, sy, sw, sh) of
/// `src` with its mask from `mask` at (mx, my) or from the colour `key`.
#[allow(clippy::too_many_arguments)]
fn cut(
    l: &List,
    src: usize,
    sx: i32,
    sy: i32,
    sw: i32,
    sh: i32,
    mask: usize,
    mx: i32,
    my: i32,
    key: u32,
) -> Option<(usize, usize)> {
    let f = import!("gdi32.dll", "GalaCutImage", CutImage)?;
    let (mask, key) = if l.flags & ILC_MASK != 0 { (mask, key) } else { (0, CLR_NONE) };
    let mut out = [0usize; 2];
    (f(src, sx, sy, sw, sh, mask, mx, my, key, l.cx, l.cy, &mut out) != 0)
        .then_some((out[0], out[1]))
}

fn delete_object(h: usize) {
    if let Some(f) = import!("gdi32.dll", "DeleteObject", extern "system" fn(usize) -> i32)
        && h != 0
    {
        f(h);
    }
}

fn delete_image(image: (usize, usize)) {
    delete_object(image.0);
    delete_object(image.1);
}

/// A bitmap's width and height (GetObjectW's BITMAP).
fn bitmap_size(h: usize) -> Option<(i32, i32)> {
    let f = import!("gdi32.dll", "GetObjectW", extern "system" fn(usize, i32, *mut c_void) -> i32)?;
    let mut bm = [0i32; 8];
    (f(h, 32, bm.as_mut_ptr().cast()) != 0).then_some((bm[1], bm[2]))
}

#[unsafe(no_mangle)]
pub extern "system" fn ImageList_Create(
    cx: i32,
    cy: i32,
    flags: u32,
    _initial: i32,
    _grow: i32,
) -> usize {
    if cx <= 0 || cy <= 0 {
        return 0;
    }
    while TABLE.lock.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() {
        core::hint::spin_loop();
    }
    // SAFETY: under the lock.
    let lists = unsafe { &mut *TABLE.lists.get() };
    let h = match lists.iter().position(Option::is_none) {
        Some(slot) => {
            lists[slot] = Some(List { cx, cy, flags, background: CLR_NONE, images: Vec::new() });
            0x1000 + 16 * slot
        }
        None => 0,
    };
    TABLE.lock.store(false, Ordering::Release);
    h
}

#[unsafe(no_mangle)]
pub extern "system" fn ImageList_Destroy(h: usize) -> i32 {
    let Some(images) = with(h, |l| core::mem::take(&mut l.images)) else { return 0 };
    for i in images {
        delete_image(i);
    }
    let slot = (h - 0x1000) / 16;
    while TABLE.lock.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() {
        core::hint::spin_loop();
    }
    // SAFETY: under the lock; `with` checked the slot.
    unsafe { (*TABLE.lists.get())[slot] = None };
    TABLE.lock.store(false, Ordering::Release);
    1
}

#[unsafe(no_mangle)]
pub extern "system" fn ImageList_GetImageCount(h: usize) -> i32 {
    with(h, |l| l.images.len() as i32).unwrap_or(0)
}

#[unsafe(no_mangle)]
pub extern "system" fn ImageList_GetIconSize(h: usize, cx: *mut i32, cy: *mut i32) -> i32 {
    if cx.is_null() || cy.is_null() {
        return 0;
    }
    with(h, |l| {
        // SAFETY: the caller's ints.
        unsafe { (*cx, *cy) = (l.cx, l.cy) };
        1
    })
    .unwrap_or(0)
}

/// Appends the images `bitmap` holds side by side (its width / the
/// list's), masked by `mask` or the colour `key`; the first one's index.
fn add(h: usize, bitmap: usize, mask: usize, key: u32) -> i32 {
    let Some((w, ht)) = bitmap_size(bitmap) else { return -1 };
    with(h, |l| {
        let first = l.images.len() as i32;
        for n in 0..(w / l.cx).max(1) {
            let Some(image) =
                cut(l, bitmap, n * l.cx, 0, l.cx, ht.min(l.cy), mask, n * l.cx, 0, key)
            else {
                return -1;
            };
            if l.images.try_reserve(1).is_err() {
                delete_image(image);
                return -1;
            }
            l.images.push(image);
        }
        first
    })
    .unwrap_or(-1)
}

#[unsafe(no_mangle)]
pub extern "system" fn ImageList_Add(h: usize, bitmap: usize, mask: usize) -> i32 {
    add(h, bitmap, mask, CLR_NONE)
}

/// ImageList_AddMasked: the pixels of colour `key` (a COLORREF) are
/// transparent (and black in the colour bitmap).
#[unsafe(no_mangle)]
pub extern "system" fn ImageList_AddMasked(h: usize, bitmap: usize, key: u32) -> i32 {
    add(h, bitmap, 0, key)
}

#[unsafe(no_mangle)]
pub extern "system" fn ImageList_Replace(h: usize, i: i32, bitmap: usize, mask: usize) -> i32 {
    let Some((w, ht)) = bitmap_size(bitmap) else { return 0 };
    with(h, |l| {
        let Some(slot) = usize::try_from(i).ok().filter(|&i| i < l.images.len()) else {
            return 0;
        };
        let Some(image) = cut(l, bitmap, 0, 0, w.min(l.cx), ht.min(l.cy), mask, 0, 0, CLR_NONE)
        else {
            return 0;
        };
        delete_image(core::mem::replace(&mut l.images[slot], image));
        1
    })
    .unwrap_or(0)
}

/// ImageList_ReplaceIcon(h, i, icon): image `i` (-1: a new one at the
/// end) from the icon, scaled to the list's size; its index.
#[unsafe(no_mangle)]
pub extern "system" fn ImageList_ReplaceIcon(h: usize, i: i32, icon: usize) -> i32 {
    let Some(info) =
        import!("user32.dll", "GetIconInfo", extern "system" fn(usize, *mut [usize; 4]) -> i32)
    else {
        return -1;
    };
    // ICONINFO: fIcon and the hotspot (16 bytes), hbmMask, hbmColor.
    let mut ii = [0usize; 4];
    if info(icon, &mut ii) == 0 {
        return -1;
    }
    let (mask, color) = (ii[2], ii[3]);
    let (w, mh) = bitmap_size(mask).unwrap_or((0, 0));
    // A monochrome icon: the mask's top half ANDs, the bottom half XORs.
    let (src, sy, ht) = if color != 0 { (color, 0, mh) } else { (mask, mh / 2, mh / 2) };
    let r = with(h, |l| {
        let Some(image) = cut(l, src, 0, sy, w, ht, mask, 0, 0, CLR_NONE) else { return -1 };
        if i < 0 {
            if l.images.try_reserve(1).is_err() {
                delete_image(image);
                return -1;
            }
            l.images.push(image);
            l.images.len() as i32 - 1
        } else if (i as usize) < l.images.len() {
            delete_image(core::mem::replace(&mut l.images[i as usize], image));
            i
        } else {
            delete_image(image);
            -1
        }
    })
    .unwrap_or(-1);
    delete_object(mask);
    delete_object(color);
    r
}

/// ImageList_Remove(h, i): image `i`, or every one for -1.
#[unsafe(no_mangle)]
pub extern "system" fn ImageList_Remove(h: usize, i: i32) -> i32 {
    let removed = with(h, |l| {
        if i == -1 {
            Some(core::mem::take(&mut l.images))
        } else if i >= 0 && (i as usize) < l.images.len() {
            let mut v = Vec::new();
            if v.try_reserve(1).is_err() {
                return None;
            }
            v.push(l.images.remove(i as usize));
            Some(v)
        } else {
            None
        }
    });
    match removed {
        Some(Some(images)) => {
            for image in images {
                delete_image(image);
            }
            1
        }
        _ => 0,
    }
}

/// ImageList_SetBkColor: the colour ILD_NORMAL draws under transparent
/// pixels (CLR_NONE: none); the previous one.
#[unsafe(no_mangle)]
pub extern "system" fn ImageList_SetBkColor(h: usize, color: u32) -> u32 {
    with(h, |l| core::mem::replace(&mut l.background, color)).unwrap_or(CLR_NONE)
}

#[unsafe(no_mangle)]
pub extern "system" fn ImageList_GetBkColor(h: usize) -> u32 {
    with(h, |l| l.background).unwrap_or(CLR_NONE)
}

/// Fills a rectangle of `hdc` with a COLORREF.
fn fill(hdc: usize, x: i32, y: i32, w: i32, h: i32, color: u32) {
    let (Some(brush), Some(select), Some(patblt)) = (
        import!("gdi32.dll", "CreateSolidBrush", extern "system" fn(u32) -> usize),
        import!("gdi32.dll", "SelectObject", extern "system" fn(usize, usize) -> usize),
        import!("gdi32.dll", "PatBlt", extern "system" fn(usize, i32, i32, i32, i32, u32) -> i32),
    ) else {
        return;
    };
    let b = brush(color);
    let old = select(hdc, b);
    patblt(hdc, x, y, w, h, 0x00f0_0021); // PATCOPY
    select(hdc, old);
    delete_object(b);
}

/// ImageList_Draw(h, i, hdc, x, y, style): ILD_NORMAL fills the
/// background colour (unless CLR_NONE) first, ILD_TRANSPARENT does not;
/// ILD_MASK (the mask alone) is not supported yet (ledger); ILD_BLEND (selection tints) is not applied.
#[unsafe(no_mangle)]
pub extern "system" fn ImageList_Draw(
    h: usize,
    i: i32,
    hdc: usize,
    x: i32,
    y: i32,
    style: u32,
) -> i32 {
    let Some(Some((cx, cy, background, image))) = with(h, |l| {
        usize::try_from(i)
            .ok()
            .and_then(|i| l.images.get(i))
            .map(|&im| (l.cx, l.cy, l.background, im))
    }) else {
        return 0;
    };
    type DrawIcon = extern "system" fn(usize, i32, i32, i32, i32, usize, usize, u32) -> i32;
    let Some(draw) = import!("gdi32.dll", "GalaDrawIcon", DrawIcon) else { return 0 };
    if style & ILD_MASK != 0 {
        crate::unsupported(b"comctl32.dll!ImageList_Draw(ILD_MASK)", u64::from(style));
    }
    if style & ILD_TRANSPARENT == 0 && background != CLR_NONE {
        fill(hdc, x, y, cx, cy, background);
    }
    draw(hdc, x, y, cx, cy, image.0, image.1, 0)
}

/// IMAGEINFO.
#[repr(C)]
pub struct ImageInfo {
    image: usize,
    mask: usize,
    unused: [i32; 2],
    rect: [i32; 4],
}

/// ImageList_GetImageInfo: the image's own colour and mask bitmaps (the
/// list's; not to be deleted) and its rectangle in them.
#[unsafe(no_mangle)]
pub extern "system" fn ImageList_GetImageInfo(h: usize, i: i32, out: *mut ImageInfo) -> i32 {
    if out.is_null() {
        return 0;
    }
    with(h, |l| {
        let Some(&(image, mask)) = usize::try_from(i).ok().and_then(|i| l.images.get(i)) else {
            return 0;
        };
        // SAFETY: the caller's IMAGEINFO.
        unsafe { out.write(ImageInfo { image, mask, unused: [0; 2], rect: [0, 0, l.cx, l.cy] }) };
        1
    })
    .unwrap_or(0)
}

/// ImageList_GetIcon: an icon made of copies of the image's bitmaps.
#[unsafe(no_mangle)]
pub extern "system" fn ImageList_GetIcon(h: usize, i: i32, _flags: u32) -> usize {
    let Some(Some(image)) =
        with(h, |l| usize::try_from(i).ok().and_then(|i| l.images.get(i)).copied())
    else {
        return 0;
    };
    let Some(create) =
        import!("user32.dll", "CreateIconIndirect", extern "system" fn(*const [usize; 4]) -> usize)
    else {
        return 0;
    };
    // CreateIconIndirect copies the bitmaps itself. ICONINFO: fIcon TRUE, no hotspot, hbmMask, hbmColor.
    let info = [1usize, 0, image.1, image.0];
    create(&info)
}

/// ImageList_Copy(dst, i, src, j, flags): within one list, image `j`
/// copied over image `i` (ILCF_MOVE), or the two swapped (ILCF_SWAP).
#[unsafe(no_mangle)]
pub extern "system" fn ImageList_Copy(dst: usize, i: i32, src: usize, j: i32, flags: u32) -> i32 {
    if dst != src {
        crate::unsupported(b"comctl32.dll!ImageList_Copy(two lists)", 0);
        return 0;
    }
    let Some(copy) = import!("gdi32.dll", "GalaCopyBitmap", extern "system" fn(usize) -> usize)
    else {
        return 0;
    };
    with(dst, |l| {
        let n = l.images.len();
        let (Ok(i), Ok(j)) = (usize::try_from(i), usize::try_from(j)) else { return 0 };
        if i >= n || j >= n {
            return 0;
        }
        if flags & ILCF_SWAP != 0 {
            l.images.swap(i, j);
        } else {
            let image = (copy(l.images[j].0), copy(l.images[j].1));
            delete_image(core::mem::replace(&mut l.images[i], image));
        }
        1
    })
    .unwrap_or(0)
}
