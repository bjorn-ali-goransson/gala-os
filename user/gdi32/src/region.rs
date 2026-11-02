//! Regions: sets of pixels kept as non-overlapping rectangles in one
//! canonical form (horizontal bands, each a run of spans, bands with the
//! same spans merged), so equal regions have equal lists. Combining
//! (AND, OR, XOR, DIFF, COPY) cuts the plane into the cells of both
//! regions' edges and keeps the cells the operation keeps. HRGN is an
//! object of the table (`Object::Region`, an index into this store). A
//! DC's clip is up to `CLIP_RECTS` device rectangles kept in the DC (a
//! clip with more becomes its bounding box, recorded).

use crate::{Handle, Object, slot_of, with, with_dc};
use alloc::vec::Vec;
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, Ordering};

pub type Rect = [i32; 4];
pub type Rects = Vec<Rect>;

pub const NULLREGION: i32 = 1;
pub const SIMPLEREGION: i32 = 2;
pub const COMPLEXREGION: i32 = 3;
const ERROR: i32 = 0;
const RGN_AND: i32 = 1;
const RGN_OR: i32 = 2;
const RGN_XOR: i32 = 3;
const RGN_DIFF: i32 = 4;
const RGN_COPY: i32 = 5;
/// The rectangles a DC's clip holds.
pub const CLIP_RECTS: usize = 8;

/// A DC's clip: device rectangles and their bounding box.
#[derive(Clone, Copy)]
pub struct Clip {
    pub rects: [Rect; CLIP_RECTS],
    pub count: usize,
    pub bound: Rect,
}

impl Clip {
    pub fn contains(&self, x: i32, y: i32) -> bool {
        let b = self.bound;
        x >= b[0]
            && y >= b[1]
            && x < b[2]
            && y < b[3]
            && self.rects[..self.count]
                .iter()
                .any(|r| x >= r[0] && y >= r[1] && x < r[2] && y < r[3])
    }

    fn of(rects: &[Rect]) -> Clip {
        let mut c = Clip { rects: [[0; 4]; CLIP_RECTS], count: 0, bound: bound(rects) };
        if rects.len() > CLIP_RECTS {
            crate::unsupported(b"gdi32.dll!clip region (rectangles)", rects.len() as u64);
            c.rects[0] = c.bound;
            c.count = usize::from(!rects.is_empty());
        } else {
            c.rects[..rects.len()].copy_from_slice(rects);
            c.count = rects.len();
        }
        c
    }

    fn list(&self) -> Rects {
        let mut v = Vec::new();
        if v.try_reserve(self.count).is_ok() {
            v.extend_from_slice(&self.rects[..self.count]);
        }
        v
    }
}

// ---- The store ----

struct Store {
    lock: AtomicBool,
    regions: UnsafeCell<Vec<Option<Rects>>>,
}

// SAFETY: `regions` is only touched under `lock`.
unsafe impl Sync for Store {}

static STORE: Store = Store { lock: AtomicBool::new(false), regions: UnsafeCell::new(Vec::new()) };

fn store<R>(f: impl FnOnce(&mut Vec<Option<Rects>>) -> R) -> R {
    while STORE.lock.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() {
        core::hint::spin_loop();
    }
    // SAFETY: under the lock.
    let r = f(unsafe { &mut *STORE.regions.get() });
    STORE.lock.store(false, Ordering::Release);
    r
}

/// Frees a region's rectangles (DeleteObject).
pub fn free(id: usize) {
    store(|s| {
        if let Some(slot) = s.get_mut(id) {
            *slot = None;
        }
    });
}

fn new_region(rects: Rects) -> Handle {
    let Some(id) = store(|s| {
        if let Some(i) = s.iter().position(Option::is_none) {
            s[i] = Some(rects);
            return Some(i);
        }
        if s.try_reserve(1).is_err() {
            return None;
        }
        s.push(Some(rects));
        Some(s.len() - 1)
    }) else {
        return core::ptr::null_mut();
    };
    let h = crate::new_object(Object::Region(id));
    if h.is_null() {
        free(id);
    }
    h
}

fn id_of(h: Handle) -> Option<usize> {
    let i = slot_of(h)?;
    with(|slots, _| match slots[i] {
        Object::Region(id) => Some(id),
        _ => None,
    })
}

fn rects_of(h: Handle) -> Option<Rects> {
    let id = id_of(h)?;
    store(|s| s.get(id).and_then(Clone::clone))
}

fn set_rects(h: Handle, rects: Rects) -> i32 {
    let Some(id) = id_of(h) else { return ERROR };
    let t = kind(&rects);
    store(|s| {
        if let Some(slot) = s.get_mut(id) {
            *slot = Some(rects);
        }
    });
    t
}

// ---- Geometry ----

fn kind(r: &[Rect]) -> i32 {
    match r.len() {
        0 => NULLREGION,
        1 => SIMPLEREGION,
        _ => COMPLEXREGION,
    }
}

fn bound(r: &[Rect]) -> Rect {
    if r.is_empty() {
        return [0; 4];
    }
    let mut b = r[0];
    for x in &r[1..] {
        b = [b[0].min(x[0]), b[1].min(x[1]), b[2].max(x[2]), b[3].max(x[3])];
    }
    b
}

fn inside(r: &[Rect], x: i32, y: i32) -> bool {
    r.iter().any(|q| x >= q[0] && y >= q[1] && x < q[2] && y < q[3])
}

fn sorted_edges(a: &[Rect], b: &[Rect], first: usize, second: usize) -> Vec<i32> {
    let mut v: Vec<i32> = Vec::new();
    let _ = v.try_reserve(2 * (a.len() + b.len()));
    for r in a.iter().chain(b) {
        v.push(r[first]);
        v.push(r[second]);
    }
    v.sort_unstable();
    v.dedup();
    v
}

/// The canonical rectangles of the cells where `keep(in a, in b)`.
fn combine(a: &[Rect], b: &[Rect], keep: impl Fn(bool, bool) -> bool) -> Rects {
    let xs = sorted_edges(a, b, 0, 2);
    let ys = sorted_edges(a, b, 1, 3);
    let mut out: Rects = Vec::new();
    // The previous band's spans and where its rectangles start in `out`.
    let mut prev: (Vec<(i32, i32)>, usize, i32) = (Vec::new(), 0, i32::MIN);
    for j in 0..ys.len().saturating_sub(1) {
        let (y0, y1) = (ys[j], ys[j + 1]);
        let mut spans: Vec<(i32, i32)> = Vec::new();
        for i in 0..xs.len().saturating_sub(1) {
            let (x0, x1) = (xs[i], xs[i + 1]);
            if keep(inside(a, x0, y0), inside(b, x0, y0)) {
                match spans.last_mut() {
                    Some(s) if s.1 == x0 => s.1 = x1,
                    _ => {
                        if spans.try_reserve(1).is_ok() {
                            spans.push((x0, x1));
                        }
                    }
                }
            }
        }
        if spans.is_empty() {
            prev = (Vec::new(), out.len(), i32::MIN);
            continue;
        }
        if spans == prev.0 && prev.2 == y0 {
            // The same spans as the band above, touching it: one taller
            // band.
            for r in &mut out[prev.1..] {
                r[3] = y1;
            }
            prev.2 = y1;
            continue;
        }
        let start = out.len();
        if out.try_reserve(spans.len()).is_err() {
            break;
        }
        for &(x0, x1) in &spans {
            out.push([x0, y0, x1, y1]);
        }
        prev = (spans, start, y1);
    }
    out
}

fn normalize(r: &[Rect]) -> Rects {
    let valid: Vec<Rect> = r.iter().copied().filter(|q| q[2] > q[0] && q[3] > q[1]).collect();
    combine(&valid, &[], |a, _| a)
}

fn op(a: &[Rect], b: &[Rect], mode: i32) -> Option<Rects> {
    Some(match mode {
        RGN_AND => combine(a, b, |x, y| x && y),
        RGN_OR => combine(a, b, |x, y| x || y),
        RGN_XOR => combine(a, b, |x, y| x != y),
        RGN_DIFF => combine(a, b, |x, y| x && !y),
        RGN_COPY => normalize(a),
        _ => return None,
    })
}

fn rect_list(r: Rect) -> Rects {
    let mut v = Vec::new();
    if r[2] > r[0] && r[3] > r[1] && v.try_reserve(1).is_ok() {
        v.push(r);
    }
    v
}

// ---- Regions ----

#[unsafe(no_mangle)]
pub extern "system" fn CreateRectRgn(l: i32, t: i32, r: i32, b: i32) -> Handle {
    new_region(rect_list([l.min(r), t.min(b), l.max(r), t.max(b)]))
}

#[unsafe(no_mangle)]
pub extern "system" fn CreateRectRgnIndirect(r: *const Rect) -> Handle {
    if r.is_null() {
        return core::ptr::null_mut();
    }
    // SAFETY: the caller's RECT.
    let [l, t, rr, b] = unsafe { *r };
    CreateRectRgn(l, t, rr, b)
}

/// SetRectRgn: the region becomes the rectangle.
#[unsafe(no_mangle)]
pub extern "system" fn SetRectRgn(h: Handle, l: i32, t: i32, r: i32, b: i32) -> i32 {
    i32::from(set_rects(h, rect_list([l.min(r), t.min(b), l.max(r), t.max(b)])) != ERROR)
}

/// CreatePolygonRgn(points, n, mode): the polygon's interior, row by row
/// (even-odd crossings; WINDING is filled as ALTERNATE).
#[unsafe(no_mangle)]
pub extern "system" fn CreatePolygonRgn(points: *const [i32; 2], n: i32, _mode: i32) -> Handle {
    if points.is_null() || n < 3 {
        return CreateRectRgn(0, 0, 0, 0);
    }
    // SAFETY: the caller's `n` POINTs.
    let p = unsafe { core::slice::from_raw_parts(points, n as usize) };
    let (top, bottom) =
        (p.iter().map(|q| q[1]).min().unwrap_or(0), p.iter().map(|q| q[1]).max().unwrap_or(0));
    let mut rows: Rects = Vec::new();
    for y in top..bottom {
        let fy = 2 * y + 1; // the row's middle, doubled
        let mut xs: Vec<i32> = Vec::new();
        for k in 0..p.len() {
            let (a, b) = (p[k], p[(k + 1) % p.len()]);
            let (ay, by) = (2 * a[1], 2 * b[1]);
            if (ay <= fy) != (by <= fy) {
                // x where the edge crosses the row's middle.
                let x = a[0] + ((fy - ay) * (b[0] - a[0])) / (by - ay);
                if xs.try_reserve(1).is_ok() {
                    xs.push(x);
                }
            }
        }
        xs.sort_unstable();
        for pair in xs.chunks(2) {
            if let [x0, x1] = *pair
                && rows.try_reserve(1).is_ok()
            {
                rows.push([x0, y, x1, y + 1]);
            }
        }
    }
    new_region(normalize(&rows))
}

/// CombineRgn(dest, a, b, mode): the type of the result, ERROR (0) for
/// bad handles or modes.
#[unsafe(no_mangle)]
pub extern "system" fn CombineRgn(dest: Handle, a: Handle, b: Handle, mode: i32) -> i32 {
    let Some(ra) = rects_of(a) else { return ERROR };
    let rb = if mode == RGN_COPY { Vec::new() } else { rects_of(b).unwrap_or_default() };
    if mode != RGN_COPY && rects_of(b).is_none() {
        return ERROR;
    }
    match op(&ra, &rb, mode) {
        Some(r) => set_rects(dest, r),
        None => ERROR,
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn GetRgnBox(h: Handle, out: *mut Rect) -> i32 {
    let Some(r) = rects_of(h) else { return ERROR };
    if !out.is_null() {
        // SAFETY: the caller's RECT.
        unsafe { *out = bound(&r) };
    }
    kind(&r)
}

#[unsafe(no_mangle)]
pub extern "system" fn PtInRegion(h: Handle, x: i32, y: i32) -> i32 {
    rects_of(h).map_or(0, |r| i32::from(inside(&r, x, y)))
}

#[unsafe(no_mangle)]
pub extern "system" fn RectInRegion(h: Handle, rect: *const Rect) -> i32 {
    if rect.is_null() {
        return 0;
    }
    // SAFETY: the caller's RECT.
    let q = unsafe { *rect };
    rects_of(h).map_or(0, |r| i32::from(!combine(&r, &rect_list(q), |a, b| a && b).is_empty()))
}

#[unsafe(no_mangle)]
pub extern "system" fn EqualRgn(a: Handle, b: Handle) -> i32 {
    match (rects_of(a), rects_of(b)) {
        (Some(x), Some(y)) => i32::from(x == y),
        _ => ERROR,
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn OffsetRgn(h: Handle, dx: i32, dy: i32) -> i32 {
    let Some(mut r) = rects_of(h) else { return ERROR };
    for q in &mut r {
        *q = [q[0] + dx, q[1] + dy, q[2] + dx, q[3] + dy];
    }
    set_rects(h, r)
}

/// GetRegionData(h, size, data): an RGNDATAHEADER (32 bytes: dwSize,
/// iType RDH_RECTANGLES, nCount, nRgnSize, rcBound) and the rectangles;
/// the size needed (and copied, when `size` holds it), 0 if too small.
#[unsafe(no_mangle)]
pub extern "system" fn GetRegionData(h: Handle, size: u32, data: *mut u8) -> u32 {
    let Some(r) = rects_of(h) else { return 0 };
    let need = 32 + 16 * r.len();
    if data.is_null() {
        return need as u32;
    }
    if (size as usize) < need {
        return 0;
    }
    // SAFETY: the caller's buffer of `size` bytes.
    unsafe {
        let p = data.cast::<u32>();
        *p = 32;
        *p.add(1) = 1;
        *p.add(2) = r.len() as u32;
        *p.add(3) = 16 * r.len() as u32;
        data.add(16).cast::<Rect>().write_unaligned(bound(&r));
        for (i, q) in r.iter().enumerate() {
            data.add(32 + 16 * i).cast::<Rect>().write_unaligned(*q);
        }
    }
    need as u32
}

/// ExtCreateRegion(xform, size, data): the region of RGNDATA's
/// rectangles (a transform is not applied: recorded).
#[unsafe(no_mangle)]
pub extern "system" fn ExtCreateRegion(xform: *const u8, size: u32, data: *const u8) -> Handle {
    if data.is_null() || size < 32 {
        return core::ptr::null_mut();
    }
    if !xform.is_null() {
        crate::unsupported(b"gdi32.dll!ExtCreateRegion(xform)", 0);
    }
    // SAFETY: the caller's RGNDATA of `size` bytes.
    let (header, count) = unsafe { (*data.cast::<u32>(), *data.cast::<u32>().add(2) as usize) };
    let header = header as usize;
    if header + 16 * count > size as usize {
        return core::ptr::null_mut();
    }
    let mut r: Rects = Vec::new();
    if r.try_reserve(count).is_err() {
        return core::ptr::null_mut();
    }
    for i in 0..count {
        // SAFETY: inside the buffer.
        r.push(unsafe { data.add(header + 16 * i).cast::<Rect>().read_unaligned() });
    }
    new_region(normalize(&r))
}

/// FillRgn(hdc, h, brush): the brush's colour over the region (logical
/// coordinates).
#[unsafe(no_mangle)]
pub extern "system" fn FillRgn(hdc: Handle, h: Handle, brush: Handle) -> i32 {
    let Some(r) = rects_of(h) else { return 0 };
    let Some(bi) = slot_of(brush) else { return 0 };
    with_dc(hdc, |dc, slots| {
        let Object::Brush { color, null } = slots[bi] else { return 0 };
        if null {
            return 1;
        }
        for q in &r {
            let (x0, y0) = dc.device_point(q[0], q[1]);
            let (x1, y1) = dc.device_point(q[2], q[3]);
            dc.fill(x0, y0, x1 - x0, y1 - y0, color);
        }
        1
    })
    .unwrap_or(0)
}

/// PaintRgn: FillRgn with the DC's brush.
#[unsafe(no_mangle)]
pub extern "system" fn PaintRgn(hdc: Handle, h: Handle) -> i32 {
    let Some(brush) = with_dc(hdc, |dc, _| dc.brush) else { return 0 };
    FillRgn(hdc, h, crate::handle(brush))
}

// ---- Clipping ----

/// The DC's drawable area in device coordinates (its surface within its
/// bounds).
fn area(dc: &crate::Dc) -> Rect {
    let b = dc.bounds;
    [b[0].max(0), b[1].max(0), b[2].min(dc.surface.width), b[3].min(dc.surface.height)]
}

/// The DC's clip as device rectangles (its area when it has none).
fn clip_of(dc: &crate::Dc) -> Rects {
    match dc.clip {
        Some(c) => c.list(),
        None => rect_list(area(dc)),
    }
}

fn to_device(dc: &crate::Dc, r: &[Rect]) -> Rects {
    r.iter()
        .map(|q| {
            let (x0, y0) = dc.device_point(q[0], q[1]);
            let (x1, y1) = dc.device_point(q[2], q[3]);
            [x0.min(x1), y0.min(y1), x0.max(x1), y0.max(y1)]
        })
        .collect()
}

/// Sets the DC's clip to device rectangles; their type.
fn set_clip(dc: &mut crate::Dc, r: Rects) -> i32 {
    let r = combine(&r, &rect_list(area(dc)), |a, b| a && b);
    let t = kind(&r);
    dc.clip = Some(Clip::of(&r));
    t
}

/// ExtSelectClipRgn(hdc, h, mode): the clip combined with the region
/// (RGN_COPY: replaced; a NULL region with RGN_COPY removes the clip).
#[unsafe(no_mangle)]
pub extern "system" fn ExtSelectClipRgn(hdc: Handle, h: Handle, mode: i32) -> i32 {
    if h.is_null() {
        if mode != RGN_COPY {
            return ERROR;
        }
        return with_dc(hdc, |dc, _| {
            dc.clip = None;
            SIMPLEREGION
        })
        .unwrap_or(ERROR);
    }
    let Some(r) = rects_of(h) else { return ERROR };
    with_dc(hdc, |dc, _| {
        let given = to_device(dc, &r);
        let current = clip_of(dc);
        let Some(new) = op(&current, &given, mode)
            .map(|x| if mode == RGN_COPY { normalize(&given) } else { x })
        else {
            return ERROR;
        };
        set_clip(dc, new)
    })
    .unwrap_or(ERROR)
}

#[unsafe(no_mangle)]
pub extern "system" fn SelectClipRgn(hdc: Handle, h: Handle) -> i32 {
    ExtSelectClipRgn(hdc, h, RGN_COPY)
}

/// GetClipBox: the clip's bounding box in logical coordinates (the
/// drawable area's without a clip); the clip's type.
#[unsafe(no_mangle)]
pub extern "system" fn GetClipBox(hdc: Handle, out: *mut Rect) -> i32 {
    with_dc(hdc, |dc, _| {
        let r = clip_of(dc);
        let b = bound(&r);
        if !out.is_null() {
            let (x0, y0) = dc.device_to_logical(b[0] - dc.base.0, b[1] - dc.base.1);
            let (x1, y1) = dc.device_to_logical(b[2] - dc.base.0, b[3] - dc.base.1);
            // SAFETY: the caller's RECT.
            unsafe { *out = [x0, y0, x1, y1] };
        }
        kind(&r)
    })
    .unwrap_or(ERROR)
}

fn clip_rect(hdc: Handle, rect: Rect, mode: i32) -> i32 {
    with_dc(hdc, |dc, _| {
        let given = to_device(dc, &[rect]);
        let current = clip_of(dc);
        match op(&current, &given, mode) {
            Some(r) => set_clip(dc, r),
            None => ERROR,
        }
    })
    .unwrap_or(ERROR)
}

#[unsafe(no_mangle)]
pub extern "system" fn ExcludeClipRect(hdc: Handle, l: i32, t: i32, r: i32, b: i32) -> i32 {
    clip_rect(hdc, [l, t, r, b], RGN_DIFF)
}

#[unsafe(no_mangle)]
pub extern "system" fn IntersectClipRect(hdc: Handle, l: i32, t: i32, r: i32, b: i32) -> i32 {
    clip_rect(hdc, [l, t, r, b], RGN_AND)
}

/// GetClipRgn(hdc, h): copies the clip into `h` (logical coordinates); 1,
/// or 0 when the DC has none.
#[unsafe(no_mangle)]
pub extern "system" fn GetClipRgn(hdc: Handle, h: Handle) -> i32 {
    let Some(Some(r)) = with_dc(hdc, |dc, _| {
        dc.clip.map(|c| {
            c.list()
                .iter()
                .map(|q| {
                    let (x0, y0) = dc.device_to_logical(q[0] - dc.base.0, q[1] - dc.base.1);
                    let (x1, y1) = dc.device_to_logical(q[2] - dc.base.0, q[3] - dc.base.1);
                    [x0, y0, x1, y1]
                })
                .collect::<Rects>()
        })
    }) else {
        return 0;
    };
    set_rects(h, r);
    1
}
