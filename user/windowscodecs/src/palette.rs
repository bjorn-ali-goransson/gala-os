//! IWICPalette (CreatePalette): a list of 0xAARRGGBB colours and its
//! type. Custom palettes take the caller's colours; of the predefined
//! ones FixedBW (black, white), FixedGray4, FixedGray16 and FixedGray256
//! (even steps from black to white) are made, their add-transparent flag
//! changing nothing, as Wine's palette test shows on Windows. The
//! halftone palettes are not made (recorded, E_NOTIMPL); Custom,
//! MedianCut and unknown types are E_INVALIDARG. InitializeFromBitmap
//! (an optimal palette for an image) is not implemented (recorded).
//! IsBlackWhite and IsGrayscale follow the predefined type; a custom
//! palette is grey when every colour is, and black-and-white when it is
//! exactly opaque black and white.

use alloc::vec::Vec;

use crate::{
    E_INVALIDARG, E_NOTIMPL, E_OUTOFMEMORY, HResult, Header, IID_IWICPALETTE, Obj, S_OK, Spin,
    VTable, add_ref, gap, not_implemented, put, query, release_object, slot, try_box,
};

pub const CUSTOM: u32 = 0;
const MEDIAN_CUT: u32 = 1;
const FIXED_BW: u32 = 2;
const FIXED_HALFTONE_256: u32 = 9;
const FIXED_GRAY4: u32 = 10;
const FIXED_GRAY16: u32 = 11;
const FIXED_GRAY256: u32 = 12;

#[repr(C)]
pub struct Palette {
    h: Header,
    state: Spin<(u32, Vec<u32>)>,
}

static PALETTE_VTBL: VTable<13> = VTable([
    palette_query as *const (),
    add_ref as *const (),
    palette_release as *const (),
    initialize_predefined as *const (),
    initialize_custom as *const (),
    initialize_from_bitmap as *const (),
    initialize_from_palette as *const (),
    get_type as *const (),
    get_color_count as *const (),
    get_colors as *const (),
    is_black_white as *const (),
    is_grayscale as *const (),
    has_alpha as *const (),
]);

/// A new, empty custom palette (one reference), or None out of memory.
pub fn new_palette() -> Option<Obj> {
    try_box(Palette { h: Header::new(PALETTE_VTBL.addr()), state: Spin::new((CUSTOM, Vec::new())) })
        .map(|p| p as Obj)
}

fn this<'a>(o: Obj) -> &'a Palette {
    // SAFETY: one of our palettes.
    unsafe { &*(o as *const Palette) }
}

extern "system" fn palette_query(this: Obj, riid: *const crate::Guid, out: *mut Obj) -> HResult {
    query(this, &[IID_IWICPALETTE], riid, out)
}

extern "system" fn palette_release(this: Obj) -> u32 {
    release_object::<Palette>(this)
}

/// The colours of a predefined palette type (None: not one gala makes).
fn predefined(kind: u32) -> Option<Vec<u32>> {
    let steps = match kind {
        FIXED_BW => 2,
        FIXED_GRAY4 => 4,
        FIXED_GRAY16 => 16,
        FIXED_GRAY256 => 256,
        _ => return None,
    };
    let mut v = Vec::new();
    v.try_reserve_exact(steps).ok()?;
    for i in 0..steps as u32 {
        let g = i * 255 / (steps as u32 - 1);
        v.push(0xff00_0000 | g << 16 | g << 8 | g);
    }
    Some(v)
}

extern "system" fn initialize_predefined(this_: Obj, kind: u32, _add_transparent: i32) -> HResult {
    match kind {
        CUSTOM | MEDIAN_CUT => E_INVALIDARG,
        k if k > FIXED_GRAY256 => E_INVALIDARG,
        k if (3..=FIXED_HALFTONE_256).contains(&k) => {
            gap(b"IWICPalette::InitializePredefined(halftone)", k.into());
            E_NOTIMPL
        }
        k => match predefined(k) {
            Some(colors) => {
                this(this_).state.with(|s| *s = (k, colors));
                S_OK
            }
            None => E_OUTOFMEMORY,
        },
    }
}

extern "system" fn initialize_custom(this_: Obj, colors: *const u32, count: u32) -> HResult {
    if colors.is_null() && count != 0 {
        return E_INVALIDARG;
    }
    let mut v = Vec::new();
    if v.try_reserve_exact(count as usize).is_err() {
        return E_OUTOFMEMORY;
    }
    for i in 0..count as usize {
        // SAFETY: the caller's `count` colours.
        v.push(unsafe { colors.add(i).read_unaligned() });
    }
    this(this_).state.with(|s| *s = (CUSTOM, v));
    S_OK
}

extern "system" fn initialize_from_bitmap(
    _this: Obj,
    _source: Obj,
    _count: u32,
    _add_transparent: i32,
) -> HResult {
    not_implemented(b"IWICPalette::InitializeFromBitmap")
}

extern "system" fn initialize_from_palette(this_: Obj, other: Obj) -> HResult {
    if other.is_null() {
        return E_INVALIDARG;
    }
    match colors(other) {
        Some((kind, v)) => {
            this(this_).state.with(|s| *s = (kind, v));
            S_OK
        }
        None => E_OUTOFMEMORY,
    }
}

extern "system" fn get_type(this_: Obj, out: *mut u32) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    put(out, this(this_).state.with(|s| s.0));
    S_OK
}

extern "system" fn get_color_count(this_: Obj, out: *mut u32) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    put(out, this(this_).state.with(|s| s.1.len() as u32));
    S_OK
}

extern "system" fn get_colors(
    this_: Obj,
    count: u32,
    colors: *mut u32,
    actual: *mut u32,
) -> HResult {
    if colors.is_null() || actual.is_null() {
        return E_INVALIDARG;
    }
    let n = this(this_).state.with(|s| {
        let n = s.1.len().min(count as usize);
        for (i, c) in s.1[..n].iter().enumerate() {
            // SAFETY: the caller's array of `count` colours.
            unsafe { colors.add(i).write_unaligned(*c) };
        }
        n
    });
    put(actual, n as u32);
    S_OK
}

fn answer(out: *mut i32, v: bool) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    put(out, i32::from(v));
    S_OK
}

extern "system" fn is_black_white(this_: Obj, out: *mut i32) -> HResult {
    let v = this(this_)
        .state
        .with(|s| s.0 == FIXED_BW || (s.0 == CUSTOM && s.1[..] == [0xff00_0000, 0xffff_ffff]));
    answer(out, v)
}

extern "system" fn is_grayscale(this_: Obj, out: *mut i32) -> HResult {
    let v = this(this_).state.with(|s| match s.0 {
        FIXED_BW | FIXED_GRAY4 | FIXED_GRAY16 | FIXED_GRAY256 => true,
        CUSTOM => {
            !s.1.is_empty()
                && s.1.iter().all(|c| {
                    let (r, g, b) = ((c >> 16) & 0xff, (c >> 8) & 0xff, c & 0xff);
                    r == g && g == b
                })
        }
        _ => false,
    });
    answer(out, v)
}

extern "system" fn has_alpha(this_: Obj, out: *mut i32) -> HResult {
    let v = this(this_).state.with(|s| s.1.iter().any(|c| c >> 24 != 0xff));
    answer(out, v)
}

type FnU32 = extern "system" fn(Obj, *mut u32) -> HResult;
type FnColors = extern "system" fn(Obj, u32, *mut u32, *mut u32) -> HResult;
type FnCustom = extern "system" fn(Obj, *const u32, u32) -> HResult;
type FnPredefined = extern "system" fn(Obj, u32, i32) -> HResult;

/// Any IWICPalette's type and colours, through its vtable (None when it
/// fails or memory runs out).
pub fn colors(p: Obj) -> Option<(u32, Vec<u32>)> {
    // SAFETY: IWICPalette's GetType, GetColorCount and GetColors.
    let (get_type, get_count, get): (FnU32, FnU32, FnColors) = unsafe {
        (
            core::mem::transmute::<*const (), FnU32>(slot(p, 7)),
            core::mem::transmute::<*const (), FnU32>(slot(p, 8)),
            core::mem::transmute::<*const (), FnColors>(slot(p, 9)),
        )
    };
    let (mut kind, mut n) = (0, 0);
    if get_type(p, &mut kind) != S_OK || get_count(p, &mut n) != S_OK {
        return None;
    }
    let mut v = Vec::new();
    v.try_reserve_exact(n as usize).ok()?;
    v.resize(n as usize, 0);
    let mut got = 0;
    if n > 0 && get(p, n, v.as_mut_ptr(), &mut got) != S_OK {
        return None;
    }
    v.truncate(got as usize);
    Some((kind, v))
}

/// Gives any IWICPalette `p` a type and colours (a predefined type by
/// InitializePredefined, a custom one by InitializeCustom).
pub fn assign(p: Obj, kind: u32, v: &[u32]) -> HResult {
    if p.is_null() {
        return E_INVALIDARG;
    }
    if kind != CUSTOM {
        // SAFETY: IWICPalette::InitializePredefined.
        let f: FnPredefined = unsafe { core::mem::transmute(slot(p, 3)) };
        if f(p, kind, 0) == S_OK {
            return S_OK;
        }
    }
    // SAFETY: IWICPalette::InitializeCustom.
    let f: FnCustom = unsafe { core::mem::transmute(slot(p, 4)) };
    f(p, v.as_ptr(), v.len() as u32)
}
