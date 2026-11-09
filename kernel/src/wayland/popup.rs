//! Where an xdg_popup goes (ROADMAP LD step 2; ARCHITECTURE "Linux
//! applications on the desktop (Wayland)", "Popups"): an xdg_positioner's
//! rules, and the place they give against the parent's window geometry
//! and the desktop's work area. Pure arithmetic, host-tested.
//!
//! Source (CLEAN_ROOM.md "Linux"): xdg-shell.xml's xdg_positioner, read as
//! the specification:
//! - the **anchor point** is a corner of the anchor rectangle for a corner
//!   anchor, the middle of an edge for an edge, its centre for none (a
//!   half rounds down, as weston computes it: `x + width / 2`);
//! - the **gravity** puts the popup towards it from the anchor point, and
//!   centres it on an axis without gravity (`- size / 2`); then the
//!   **offset** is added;
//! - **constraint adjustment**, each axis on its own, only where the popup
//!   is constrained (partly outside the work area), in the protocol's
//!   order: flip (anchor and gravity inverted on that axis, the same
//!   anchor rectangle and offset; kept only if no longer constrained),
//!   then slide (first towards the gravity, until the edge opposite it is
//!   inside or the edge towards it would leave; then against it, until
//!   the edge towards it is inside or the opposite edge would leave),
//!   then resize (cut to the area, if any of it is inside).
//!
//! Positions are i64 inside (a client's offsets are any i32) and clamped
//! to i32 in the result.

/// xdg_positioner's anchor and gravity values (the same enum numbers).
pub const NONE: u32 = 0;
pub const TOP: u32 = 1;
pub const BOTTOM: u32 = 2;
pub const LEFT: u32 = 3;
pub const RIGHT: u32 = 4;
pub const TOP_LEFT: u32 = 5;
pub const BOTTOM_LEFT: u32 = 6;
pub const TOP_RIGHT: u32 = 7;
pub const BOTTOM_RIGHT: u32 = 8;

/// constraint_adjustment's bits.
pub const SLIDE_X: u32 = 1;
pub const SLIDE_Y: u32 = 2;
pub const FLIP_X: u32 = 4;
pub const FLIP_Y: u32 = 8;
pub const RESIZE_X: u32 = 16;
pub const RESIZE_Y: u32 = 32;
/// Every bit the protocol defines.
pub const ADJUST_ALL: u32 = 63;

/// An xdg_positioner's rules (a popup keeps a copy of them: the
/// positioner may change or go after).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rules {
    /// The popup's window geometry size (0x0: not set).
    pub size: (i32, i32),
    /// x, y, width, height in the parent's window geometry (None: not
    /// set).
    pub rect: Option<(i32, i32, i32, i32)>,
    pub anchor: u32,
    pub gravity: u32,
    /// constraint_adjustment bits.
    pub adjust: u32,
    pub offset: (i32, i32),
    /// set_reactive (version 3): kept; the popup is not reconstrained
    /// when its parent moves yet.
    pub reactive: bool,
}

impl Rules {
    /// Whether it can place a popup: a size and an anchor rectangle set.
    pub fn complete(&self) -> bool {
        self.size.0 > 0 && self.size.1 > 0 && self.rect.is_some()
    }
}

/// Whether `v` is one of the anchor (or gravity) values.
pub fn valid_direction(v: u32) -> bool {
    v <= BOTTOM_RIGHT
}

/// One axis of a direction: -1 towards left/top, 1 towards right/bottom,
/// 0 neither.
fn axis(dir: u32, horizontal: bool) -> i64 {
    let (lo, hi) = if horizontal {
        (
            matches!(dir, LEFT | TOP_LEFT | BOTTOM_LEFT),
            matches!(dir, RIGHT | TOP_RIGHT | BOTTOM_RIGHT),
        )
    } else {
        (
            matches!(dir, TOP | TOP_LEFT | TOP_RIGHT),
            matches!(dir, BOTTOM | BOTTOM_LEFT | BOTTOM_RIGHT),
        )
    };
    match (lo, hi) {
        (true, _) => -1,
        (_, true) => 1,
        _ => 0,
    }
}

/// `dir` inverted on one axis (left and right swapped, or top and
/// bottom).
fn flip(dir: u32, horizontal: bool) -> u32 {
    if horizontal {
        match dir {
            LEFT => RIGHT,
            RIGHT => LEFT,
            TOP_LEFT => TOP_RIGHT,
            TOP_RIGHT => TOP_LEFT,
            BOTTOM_LEFT => BOTTOM_RIGHT,
            BOTTOM_RIGHT => BOTTOM_LEFT,
            other => other,
        }
    } else {
        match dir {
            TOP => BOTTOM,
            BOTTOM => TOP,
            TOP_LEFT => BOTTOM_LEFT,
            BOTTOM_LEFT => TOP_LEFT,
            TOP_RIGHT => BOTTOM_RIGHT,
            BOTTOM_RIGHT => TOP_RIGHT,
            other => other,
        }
    }
}

/// The popup's start on one axis from the anchor rectangle's start and
/// length, the popup's length, the anchor's and gravity's sides on that
/// axis and the offset.
fn start(rect: (i64, i64), len: i64, anchor: i64, gravity: i64, offset: i64) -> i64 {
    let point = match anchor {
        -1 => rect.0,
        1 => rect.0 + rect.1,
        _ => rect.0 + rect.1 / 2,
    };
    let at = match gravity {
        -1 => point - len,
        1 => point,
        _ => point - len / 2,
    };
    at + offset
}

/// One axis's adjustment against [lo, hi) of the area (both in the
/// parent's coordinates): the start and length.
#[allow(clippy::too_many_arguments)]
fn adjust_axis(
    rect: (i64, i64),
    len: i64,
    anchor: u32,
    gravity: u32,
    offset: i64,
    (lo, hi): (i64, i64),
    (flips, slides, resizes): (bool, bool, bool),
    horizontal: bool,
) -> (i64, i64) {
    let constrained = |at: i64, len: i64| at < lo || at + len > hi;
    let mut at = start(rect, len, axis(anchor, horizontal), axis(gravity, horizontal), offset);
    let mut len = len;
    let mut gravity_side = axis(gravity, horizontal);
    if !constrained(at, len) {
        return (at, len);
    }
    if flips {
        let (a, g) = (flip(anchor, horizontal), flip(gravity, horizontal));
        let flipped = start(rect, len, axis(a, horizontal), axis(g, horizontal), offset);
        if !constrained(flipped, len) {
            return (flipped, len);
        }
    }
    if slides {
        // Towards the gravity first (none: as towards its end), then
        // against it.
        if gravity_side == 0 {
            gravity_side = 1;
        }
        let toward_end = |at: i64| if at < lo { at.max(lo.min(hi - len)) } else { at };
        let toward_start = |at: i64| if at + len > hi { at.min((hi - len).max(lo)) } else { at };
        at = if gravity_side > 0 {
            toward_start(toward_end(at))
        } else {
            toward_end(toward_start(at))
        };
    }
    if resizes && constrained(at, len) {
        let (from, to) = (at.max(lo), (at + len).min(hi));
        if to > from {
            at = from;
            len = to - from;
        }
    }
    (at, len)
}

/// The popup's place relative to the parent's window geometry and its
/// size (the configure's x, y, width, height) for `rules`, which must be
/// complete. `origin`: the parent's window geometry's screen position, and
/// `area` the work area (x, y, width, height) on the screen; without an
/// origin (a parent not shown) nothing is adjusted.
pub fn place(
    rules: &Rules,
    origin: Option<(i32, i32)>,
    area: (i32, i32, u32, u32),
) -> (i32, i32, i32, i32) {
    let (rx, ry, rw, rh) = rules.rect.unwrap_or((0, 0, 1, 1));
    let (w, h) = (i64::from(rules.size.0.max(1)), i64::from(rules.size.1.max(1)));
    let x_rect = (i64::from(rx), i64::from(rw));
    let y_rect = (i64::from(ry), i64::from(rh));
    let (ox, oy) = (i64::from(rules.offset.0), i64::from(rules.offset.1));
    let (x, w, y, h) = match origin {
        None => (
            start(x_rect, w, axis(rules.anchor, true), axis(rules.gravity, true), ox),
            w,
            start(y_rect, h, axis(rules.anchor, false), axis(rules.gravity, false), oy),
            h,
        ),
        Some((px, py)) => {
            // The area in the parent's coordinates.
            let (px, py) = (i64::from(px), i64::from(py));
            let xs = (i64::from(area.0) - px, i64::from(area.0) + i64::from(area.2) - px);
            let ys = (i64::from(area.1) - py, i64::from(area.1) + i64::from(area.3) - py);
            let a = rules.adjust;
            let (x, w) = adjust_axis(
                x_rect,
                w,
                rules.anchor,
                rules.gravity,
                ox,
                xs,
                (a & FLIP_X != 0, a & SLIDE_X != 0, a & RESIZE_X != 0),
                true,
            );
            let (y, h) = adjust_axis(
                y_rect,
                h,
                rules.anchor,
                rules.gravity,
                oy,
                ys,
                (a & FLIP_Y != 0, a & SLIDE_Y != 0, a & RESIZE_Y != 0),
                false,
            );
            (x, w, y, h)
        }
    };
    let clamp = |v: i64| v.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32;
    (clamp(x), clamp(y), clamp(w), clamp(h))
}
