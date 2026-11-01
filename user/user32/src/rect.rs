//! The RECT helpers, as documented: an empty rectangle has no area (right
//! <= left or bottom <= top); IntersectRect and UnionRect leave an empty
//! result zeroed and return FALSE; PtInRect includes the left and top
//! edges, not the right and bottom; SubtractRect only shrinks the source
//! when the other covers a whole side of it.

pub type Rect = [i32; 4];

fn empty(r: &Rect) -> bool {
    r[2] <= r[0] || r[3] <= r[1]
}

/// Runs `f` on the caller's RECT if it is not NULL.
fn rect<R>(p: *mut Rect, f: impl FnOnce(&mut Rect) -> R) -> Option<R> {
    // SAFETY: the caller's RECT.
    (!p.is_null()).then(|| f(unsafe { &mut *p }))
}

fn read(p: *const Rect) -> Option<Rect> {
    // SAFETY: the caller's RECT.
    (!p.is_null()).then(|| unsafe { *p })
}

#[unsafe(no_mangle)]
pub extern "system" fn SetRect(r: *mut Rect, l: i32, t: i32, rr: i32, b: i32) -> i32 {
    i32::from(rect(r, |r| *r = [l, t, rr, b]).is_some())
}

#[unsafe(no_mangle)]
pub extern "system" fn SetRectEmpty(r: *mut Rect) -> i32 {
    i32::from(rect(r, |r| *r = [0; 4]).is_some())
}

#[unsafe(no_mangle)]
pub extern "system" fn CopyRect(dst: *mut Rect, src: *const Rect) -> i32 {
    let Some(s) = read(src) else { return 0 };
    i32::from(rect(dst, |d| *d = s).is_some())
}

#[unsafe(no_mangle)]
pub extern "system" fn IsRectEmpty(r: *const Rect) -> i32 {
    read(r).map_or(1, |r| i32::from(empty(&r)))
}

#[unsafe(no_mangle)]
pub extern "system" fn EqualRect(a: *const Rect, b: *const Rect) -> i32 {
    match (read(a), read(b)) {
        (Some(a), Some(b)) => i32::from(a == b),
        _ => 0,
    }
}

/// PtInRect(rect, POINT by value: x low, y high).
#[unsafe(no_mangle)]
pub extern "system" fn PtInRect(r: *const Rect, point: u64) -> i32 {
    let (x, y) = (point as u32 as i32, (point >> 32) as u32 as i32);
    read(r).map_or(0, |r| i32::from(x >= r[0] && x < r[2] && y >= r[1] && y < r[3]))
}

#[unsafe(no_mangle)]
pub extern "system" fn InflateRect(r: *mut Rect, dx: i32, dy: i32) -> i32 {
    i32::from(rect(r, |r| *r = [r[0] - dx, r[1] - dy, r[2] + dx, r[3] + dy]).is_some())
}

#[unsafe(no_mangle)]
pub extern "system" fn OffsetRect(r: *mut Rect, dx: i32, dy: i32) -> i32 {
    i32::from(rect(r, |r| *r = [r[0] + dx, r[1] + dy, r[2] + dx, r[3] + dy]).is_some())
}

#[unsafe(no_mangle)]
pub extern "system" fn IntersectRect(dst: *mut Rect, a: *const Rect, b: *const Rect) -> i32 {
    let (Some(a), Some(b)) = (read(a), read(b)) else { return 0 };
    let r = [a[0].max(b[0]), a[1].max(b[1]), a[2].min(b[2]), a[3].min(b[3])];
    let ok = !empty(&r) && !empty(&a) && !empty(&b);
    rect(dst, |d| *d = if ok { r } else { [0; 4] });
    i32::from(ok)
}

#[unsafe(no_mangle)]
pub extern "system" fn UnionRect(dst: *mut Rect, a: *const Rect, b: *const Rect) -> i32 {
    let (Some(a), Some(b)) = (read(a), read(b)) else { return 0 };
    let r = match (empty(&a), empty(&b)) {
        (true, true) => [0; 4],
        (true, false) => b,
        (false, true) => a,
        (false, false) => [a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])],
    };
    rect(dst, |d| *d = r);
    i32::from(!empty(&r))
}

/// SubtractRect(dst, a, b): `a` without `b` when that is a rectangle
/// (`b` spans `a` in one direction); else `a`. FALSE for an empty result.
#[unsafe(no_mangle)]
pub extern "system" fn SubtractRect(dst: *mut Rect, a: *const Rect, b: *const Rect) -> i32 {
    let (Some(a), Some(b)) = (read(a), read(b)) else { return 0 };
    let mut r = a;
    let spans_x = b[0] <= a[0] && b[2] >= a[2];
    let spans_y = b[1] <= a[1] && b[3] >= a[3];
    if spans_x && b[1] <= a[1] && b[3] > a[1] {
        r[1] = b[3].min(a[3]);
    } else if spans_x && b[3] >= a[3] && b[1] < a[3] {
        r[3] = b[1].max(a[1]);
    } else if spans_y && b[0] <= a[0] && b[2] > a[0] {
        r[0] = b[2].min(a[2]);
    } else if spans_y && b[2] >= a[2] && b[0] < a[2] {
        r[2] = b[0].max(a[0]);
    }
    let ok = !empty(&r);
    rect(dst, |d| *d = if ok { r } else { [0; 4] });
    i32::from(ok)
}
