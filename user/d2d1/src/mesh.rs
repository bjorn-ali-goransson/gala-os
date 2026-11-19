//! Meshes: a list of triangles (ID2D1Mesh) that
//! ID2D1RenderTarget::FillMesh draws, given to it through an
//! ID2D1TessellationSink. Direct2D hands the triangles to the hardware
//! as they are, which is what a mesh is for: a shape already tessellated
//! by the program (Word tessellates its glyph runs and its drawing
//! canvas this way).
//!
//! A mesh takes its triangles once. Open gives the one sink it will ever
//! give: a second Open, before or after the sink's Close, is
//! D2DERR_WRONG_STATE, and triangles added after Close are dropped.
//! Until that Close the mesh has no tessellation at all, and filling it
//! fails the frame with D2DERR_WRONG_STATE (measured, `d2d1_test`).
//!
//! The triangles stay in the coordinates they were given in; the render
//! target puts them through its transform, one triangle at a time, the
//! way the hardware draws a triangle list: overlapping triangles of one
//! mesh blend twice under a half transparent brush, and a triangle wound
//! the other way fills all the same (measured).

use alloc::vec::Vec;

use crate::{
    D2DERR_WRONG_STATE, E_OUTOFMEMORY, Guid, HResult, Header, IID_MESH, IID_RESOURCE,
    IID_TESSELLATION_SINK, Obj, Point, Res, S_OK, Spin, VTable, add_ref, give, push, put, query,
    release_object, vtable_of,
};

/// D2D1_TRIANGLE.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Triangle {
    pub a: Point,
    pub b: Point,
    pub c: Point,
}

/// Where the mesh is in its one pass: before its sink, taking
/// triangles, done, or out of memory (its Close answers
/// E_OUTOFMEMORY and it never gets a tessellation).
#[derive(PartialEq)]
enum Phase {
    Fresh,
    Open,
    Closed,
    NoMemory,
}

struct MeshState {
    phase: Phase,
    tris: Vec<Triangle>,
}

#[repr(C)]
pub struct Mesh {
    res: Res,
    state: Spin<MeshState>,
}

/// The sink of one mesh (a reference to it held): it is the mesh that
/// keeps the triangles and the phase.
#[repr(C)]
struct Sink {
    hdr: Header,
    mesh: Obj,
}

impl Drop for Sink {
    fn drop(&mut self) {
        release_object::<Mesh>(self.mesh);
    }
}

// The interfaces this module's vtable slots belong to: `cargo xtask
// gen-trace` names each wrapper's method from them (xtask/src/gentrace.rs).
// gen-trace prefix mesh_ ID2D1Mesh
// gen-trace prefix sink_ ID2D1TessellationSink
static MESH_VTBL: VTable<5> = VTable([
    mesh_query as *const (),
    add_ref as *const (),
    mesh_release as *const (),
    crate::traced::get_factory as *const (),
    traced::mesh_Open as *const (),
]);

static SINK_VTBL: VTable<5> = VTable([
    sink_query as *const (),
    add_ref as *const (),
    sink_release as *const (),
    traced::sink_AddTriangles as *const (),
    traced::sink_Close as *const (),
]);

/// ID2D1RenderTarget::CreateMesh: an empty mesh of `factory`.
pub fn create(factory: Obj, out: *mut Obj) -> HResult {
    if out.is_null() {
        return crate::E_POINTER;
    }
    give(
        out,
        Mesh {
            res: Res::new(MESH_VTBL.addr(), factory),
            state: Spin::new(MeshState { phase: Phase::Fresh, tris: Vec::new() }),
        },
    )
}

/// Whether `o` is one of our meshes (FillMesh draws no other).
pub fn is_mesh(o: Obj) -> bool {
    !o.is_null() && vtable_of(o) == MESH_VTBL.addr()
}

/// The factory mesh `o` was made by (FillMesh refuses another's,
/// D2DERR_WRONG_FACTORY).
pub fn factory_of(o: Obj) -> Obj {
    // SAFETY: one of our meshes.
    unsafe { (*(o as *const Mesh)).res.factory.0 }
}

fn state<R>(m: Obj, f: impl FnOnce(&mut MeshState) -> R) -> R {
    // SAFETY: one of our meshes.
    unsafe { (*(m as *const Mesh)).state.with(f) }
}

/// Runs `f` on the mesh's triangles, None while it has no tessellation
/// (its sink never closed, or no sink at all): the frame then fails.
pub fn with_triangles<R>(m: Obj, f: impl FnOnce(&[Triangle]) -> R) -> Option<R> {
    state(m, |s| if s.phase == Phase::Closed { Some(f(&s.tris)) } else { None })
}

extern "system" fn mesh_query(this: Obj, riid: *const Guid, out: *mut Obj) -> HResult {
    query(this, b"ID2D1Mesh", &[IID_MESH, IID_RESOURCE], riid, out)
}

extern "system" fn mesh_release(this: Obj) -> u32 {
    release_object::<Mesh>(this)
}

/// ID2D1Mesh::Open: the one sink the mesh gives.
#[allow(non_snake_case)]
extern "system" fn mesh_Open(this: Obj, out: *mut Obj) -> HResult {
    put(out, core::ptr::null_mut());
    if out.is_null() {
        return crate::E_POINTER;
    }
    if !state(this, |s| {
        let fresh = s.phase == Phase::Fresh;
        if fresh {
            s.phase = Phase::Open;
        }
        fresh
    }) {
        return D2DERR_WRONG_STATE;
    }
    // The sink holds the mesh while it fills it.
    add_ref(this);
    let hr = give(out, Sink { hdr: Header::new(SINK_VTBL.addr()), mesh: this });
    if hr != S_OK {
        state(this, |s| s.phase = Phase::Fresh);
        release_object::<Mesh>(this);
    }
    hr
}

extern "system" fn sink_query(this: Obj, riid: *const Guid, out: *mut Obj) -> HResult {
    query(this, b"ID2D1TessellationSink", &[IID_TESSELLATION_SINK], riid, out)
}

extern "system" fn sink_release(this: Obj) -> u32 {
    release_object::<Sink>(this)
}

/// The mesh a sink fills.
fn mesh_of(sink: Obj) -> Obj {
    // SAFETY: one of our sinks.
    unsafe { (*(sink as *const Sink)).mesh }
}

/// ID2D1TessellationSink::AddTriangles: more triangles, until Close
/// (after it they are dropped, as Direct2D drops them).
#[allow(non_snake_case)]
extern "system" fn sink_AddTriangles(this: Obj, tris: *const Triangle, count: u32) {
    let mesh = mesh_of(this);
    state(mesh, |s| {
        if s.phase != Phase::Open || count == 0 || tris.is_null() {
            return;
        }
        for i in 0..count as usize {
            // SAFETY: the caller's `count` triangles at `tris`.
            let t = unsafe { tris.add(i).read_unaligned() };
            if !push(&mut s.tris, t) {
                // No memory for the tessellation: the mesh keeps none,
                // and its Close says so.
                s.tris = Vec::new();
                s.phase = Phase::NoMemory;
                return;
            }
        }
    });
}

/// ID2D1TessellationSink::Close: the tessellation is complete. A second
/// Close is D2DERR_WRONG_STATE.
#[allow(non_snake_case)]
extern "system" fn sink_Close(this: Obj) -> HResult {
    state(mesh_of(this), |s| match s.phase {
        Phase::Open => {
            s.phase = Phase::Closed;
            S_OK
        }
        Phase::NoMemory => E_OUTOFMEMORY,
        _ => D2DERR_WRONG_STATE,
    })
}

// ==== generated by `cargo xtask gen-trace`: do not edit ====
/// The wrapper of every vtable slot of this module: each records
/// the first call that reaches its method, and every call at all
/// once `trace_from=` has armed one (`cargo xtask gen-trace`).
pub(crate) mod traced {
    use super::*;
    /// ID2D1Mesh::Open, traced.
    pub(super) extern "system" fn mesh_Open(this: Obj, out: *mut Obj) -> HResult {
        crate::traced!(b"ID2D1Mesh::Open", [this, out]);
        let answer = super::mesh_Open(this, out);
        crate::answered_call!(b"ID2D1Mesh::Open", answer);
        answer
    }
    /// ID2D1TessellationSink::AddTriangles, traced.
    pub(super) extern "system" fn sink_AddTriangles(this: Obj, tris: *const Triangle, count: u32) {
        crate::traced!(b"ID2D1TessellationSink::AddTriangles", [this, tris, count]);
        super::sink_AddTriangles(this, tris, count)
    }
    /// ID2D1TessellationSink::Close, traced.
    pub(super) extern "system" fn sink_Close(this: Obj) -> HResult {
        crate::traced!(b"ID2D1TessellationSink::Close", [this]);
        let answer = super::sink_Close(this);
        crate::answered_call!(b"ID2D1TessellationSink::Close", answer);
        answer
    }
}
