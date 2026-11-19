//! Drawing state blocks (ID2D1DrawingStateBlock, d2d1.h;
//! ID2D1DrawingStateBlock1, d2d1_1.h): a render target's drawing state
//! kept away from the target -- its transform, its antialias modes, its
//! tags and its text-rendering parameters, and from 1.1 the primitive
//! blend and the unit mode -- which SaveDrawingState writes into and
//! RestoreDrawingState reads back. Word makes two of them before it
//! draws anything.
//!
//! One object answers both interfaces, as Windows' does: a block from
//! ID2D1Factory::CreateDrawingStateBlock answers ID2D1DrawingStateBlock1
//! too, holds the 1.1 description whichever interface made it, and the
//! 1.0 SetDescription leaves the primitive blend and the unit mode as
//! they stand (all three measured on Windows; `d2d1_test`'s
//! `state_blocks`).

use crate::{
    ComRef, E_POINTER, Guid, HResult, IID_DRAWING_STATE_BLOCK, IID_DRAWING_STATE_BLOCK1,
    IID_RESOURCE, Matrix, Obj, Res, Spin, VTable, add_ref, answered, com_add_ref, give, put, query,
    read, release_object, vtable_of,
};

/// D2D1_DRAWING_STATE_DESCRIPTION.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct StateDesc {
    pub antialias: u32,
    pub text_antialias: u32,
    pub tag1: u64,
    pub tag2: u64,
    pub transform: Matrix,
}

/// D2D1_DRAWING_STATE_DESCRIPTION1: the 1.0 description, then the
/// primitive blend and the unit mode.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct StateDesc1 {
    pub base: StateDesc,
    pub primitive_blend: u32,
    pub unit_mode: u32,
}

impl StateDesc1 {
    /// What a block holds when it is made without a description
    /// (measured on Windows): per-primitive antialiasing, the default
    /// text antialiasing, no tags, the identity transform, source-over
    /// and DIPs.
    pub const DEFAULT: StateDesc1 = StateDesc1 {
        base: StateDesc {
            antialias: 0,
            text_antialias: 0,
            tag1: 0,
            tag2: 0,
            transform: Matrix::IDENTITY,
        },
        primitive_blend: 0,
        unit_mode: 0,
    };
}

/// The state a block holds: the 1.1 description and the text-rendering
/// parameters it keeps a reference to (NULL for none).
struct Saved {
    desc: StateDesc1,
    params: ComRef,
}

#[repr(C)]
pub struct Block {
    res: Res,
    state: Spin<Saved>,
}

// The interfaces this module's vtable slots belong to: `cargo xtask
// gen-trace` names each wrapper's method from them (xtask/src/gentrace.rs).
// gen-trace prefix b_ ID2D1DrawingStateBlock
// gen-trace prefix b1_ ID2D1DrawingStateBlock1
static BLOCK_VTBL: VTable<10> = VTable([
    block_query as *const (),
    add_ref as *const (),
    block_release as *const (),
    crate::traced::get_factory as *const (),
    traced::b_GetDescription as *const (),
    traced::b_SetDescription as *const (),
    traced::b_SetTextRenderingParams as *const (),
    traced::b_GetTextRenderingParams as *const (),
    traced::b1_GetDescription as *const (),
    traced::b1_SetDescription as *const (),
]);

/// ID2D1Factory::CreateDrawingStateBlock(description, params, &block):
/// the 1.0 description, the rest of the state as a fresh block's.
pub fn create(factory: Obj, desc: *const StateDesc, params: Obj, out: *mut Obj) -> HResult {
    let mut d = StateDesc1::DEFAULT;
    if let Some(base) = read(desc) {
        d.base = base;
    }
    let hr = new_block(factory, d, params, out);
    answered(b"ID2D1Factory::CreateDrawingStateBlock", hr);
    hr
}

/// ID2D1Factory1::CreateDrawingStateBlock(description1, params, &block).
pub fn create1(factory: Obj, desc: *const StateDesc1, params: Obj, out: *mut Obj) -> HResult {
    let hr = new_block(factory, read(desc).unwrap_or(StateDesc1::DEFAULT), params, out);
    answered(b"ID2D1Factory1::CreateDrawingStateBlock", hr);
    hr
}

fn new_block(factory: Obj, desc: StateDesc1, params: Obj, out: *mut Obj) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, core::ptr::null_mut());
    give(
        out,
        Block {
            res: Res::new(BLOCK_VTBL.addr(), factory),
            state: Spin::new(Saved { desc, params: ComRef::new(params) }),
        },
    )
}

/// What a render target writes into a block (SaveDrawingState): false
/// when the object is not one of gala's blocks, which the target
/// records.
pub fn save(block: Obj, desc: StateDesc1, params: Obj) -> bool {
    if !ours(block) {
        return false;
    }
    let new = ComRef::new(params);
    let old = with(block, |s| {
        s.desc = desc;
        core::mem::replace(&mut s.params, new)
    });
    drop(old);
    true
}

/// What a render target reads back (RestoreDrawingState): the
/// description, and a reference to the block's text-rendering parameters
/// (NULL when it holds none). None when the object is not one of gala's
/// blocks.
pub fn load(block: Obj) -> Option<(StateDesc1, Obj)> {
    if !ours(block) {
        return None;
    }
    let (desc, p) = with(block, |s| (s.desc, s.params.0));
    if !p.is_null() {
        com_add_ref(p);
    }
    Some((desc, p))
}

/// Whether `block` is one of this DLL's blocks (its vtable says so).
fn ours(block: Obj) -> bool {
    vtable_of(block) == BLOCK_VTBL.addr()
}

fn with<R>(this: Obj, f: impl FnOnce(&mut Saved) -> R) -> R {
    // SAFETY: one of our blocks: it starts with a Block.
    unsafe { (*(this as *const Block)).state.with(f) }
}

extern "system" fn block_query(this: Obj, riid: *const Guid, out: *mut Obj) -> HResult {
    query(
        this,
        b"ID2D1DrawingStateBlock",
        &[IID_RESOURCE, IID_DRAWING_STATE_BLOCK, IID_DRAWING_STATE_BLOCK1],
        riid,
        out,
    )
}

extern "system" fn block_release(this: Obj) -> u32 {
    release_object::<Block>(this)
}

extern "system" fn b_GetDescription(this: Obj, out: *mut StateDesc) {
    put(out, with(this, |s| s.desc.base));
}

/// The 1.0 SetDescription: the primitive blend and the unit mode are
/// left as they stand (Windows does the same).
extern "system" fn b_SetDescription(this: Obj, desc: *const StateDesc) {
    if let Some(d) = read(desc) {
        with(this, |s| s.desc.base = d);
    }
}

extern "system" fn b_SetTextRenderingParams(this: Obj, params: Obj) {
    let new = ComRef::new(params);
    let old = with(this, |s| core::mem::replace(&mut s.params, new));
    drop(old);
}

extern "system" fn b_GetTextRenderingParams(this: Obj, out: *mut Obj) {
    let p = with(this, |s| s.params.0);
    if !p.is_null() {
        com_add_ref(p);
    }
    put(out, p);
}

extern "system" fn b1_GetDescription(this: Obj, out: *mut StateDesc1) {
    put(out, with(this, |s| s.desc));
}

extern "system" fn b1_SetDescription(this: Obj, desc: *const StateDesc1) {
    if let Some(d) = read(desc) {
        with(this, |s| s.desc = d);
    }
}

// ==== generated by `cargo xtask gen-trace`: do not edit ====
/// The wrapper of every vtable slot of this module, each recording
/// the first call that reaches its method (`cargo xtask gen-trace`).
pub(crate) mod traced {
    use super::*;
    /// ID2D1DrawingStateBlock1::GetDescription, traced.
    pub(super) extern "system" fn b1_GetDescription(this: Obj, out: *mut StateDesc1) {
        crate::reached!(b"ID2D1DrawingStateBlock1::GetDescription");
        super::b1_GetDescription(this, out)
    }
    /// ID2D1DrawingStateBlock1::SetDescription, traced.
    pub(super) extern "system" fn b1_SetDescription(this: Obj, desc: *const StateDesc1) {
        crate::reached!(b"ID2D1DrawingStateBlock1::SetDescription");
        super::b1_SetDescription(this, desc)
    }
    /// ID2D1DrawingStateBlock::GetDescription, traced.
    pub(super) extern "system" fn b_GetDescription(this: Obj, out: *mut StateDesc) {
        crate::reached!(b"ID2D1DrawingStateBlock::GetDescription");
        super::b_GetDescription(this, out)
    }
    /// ID2D1DrawingStateBlock::GetTextRenderingParams, traced.
    pub(super) extern "system" fn b_GetTextRenderingParams(this: Obj, out: *mut Obj) {
        crate::reached!(b"ID2D1DrawingStateBlock::GetTextRenderingParams");
        super::b_GetTextRenderingParams(this, out)
    }
    /// ID2D1DrawingStateBlock::SetDescription, traced.
    pub(super) extern "system" fn b_SetDescription(this: Obj, desc: *const StateDesc) {
        crate::reached!(b"ID2D1DrawingStateBlock::SetDescription");
        super::b_SetDescription(this, desc)
    }
    /// ID2D1DrawingStateBlock::SetTextRenderingParams, traced.
    pub(super) extern "system" fn b_SetTextRenderingParams(this: Obj, params: Obj) {
        crate::reached!(b"ID2D1DrawingStateBlock::SetTextRenderingParams");
        super::b_SetTextRenderingParams(this, params)
    }
}
