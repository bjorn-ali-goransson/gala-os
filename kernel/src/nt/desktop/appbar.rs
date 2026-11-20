//! App bars: windows that reserve an edge of the screen (SHAppBarMessage:
//! shell32 hands ABM_NEW, ABM_REMOVE, ABM_QUERYPOS and ABM_SETPOS to
//! GalaAppBar). As Microsoft documents them ("Using Application Desktop
//! Toolbars"): a bar is registered once per window with its callback
//! message; QUERYPOS moves a proposed rectangle so it does not overlap
//! the bars already on that edge; SETPOS takes the strip from the work
//! area; the other bars on the same edge are told ABN_POSCHANGED when a
//! bar there is set or removed. ARCHITECTURE "App bars".
//!
//! The taskbar is a bar itself, on the bottom edge (Windows' is
//! Shell_TrayWnd's): the desktop registers its own (window 0, process 0:
//! no program's window is 0, so no program can move or remove it) when
//! the screen is laid out, and ABM_GETTASKBARPOS answers with its strip.
//! A program's bottom bar goes above it.
//!
//! gala's own rule (the docked agent panel's): a bar on the left or the
//! right edge reaches from the top of the screen down to the bars on the
//! bottom edge (the taskbar), never over them; those keep the screen's
//! whole width.
//!
//! The bars keep what they must tell programs in `posts`, posted once the
//! desktop's lock is let go, as the notification area's are.

use super::NtStatus;
use super::Rect;
use super::tray::Post;
use crate::mm::heap::TryVec;
use alloc::vec::Vec;

pub const ABM_NEW: u32 = 0;
pub const ABM_REMOVE: u32 = 1;
pub const ABM_QUERYPOS: u32 = 2;
pub const ABM_SETPOS: u32 = 3;
pub const ABM_GETTASKBARPOS: u32 = 5;
/// gala's own (no ABM_ has it): the session's shell takes the taskbar's
/// bar for its window (`AppBars::hand_taskbar`). The syscall lets only
/// the shell window's process ask.
pub const ABM_GALA_TASKBAR: u32 = 0x100;

/// The desktop's own taskbar's bar: its window (no program's is 0) and
/// its process.
pub const TASKBAR_WINDOW: u64 = 0;

pub const ABE_LEFT: u32 = 0;
pub const ABE_TOP: u32 = 1;
pub const ABE_RIGHT: u32 = 2;
pub const ABE_BOTTOM: u32 = 3;

/// The wParam of a bar's callback message: something on its edge moved.
pub const ABN_POSCHANGED: u64 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bar {
    pub owner: u64,
    pub hwnd: u64,
    pub callback: u32,
    /// Its edge and strip once ABM_SETPOS placed it.
    pub placed: Option<(u32, Rect)>,
}

#[derive(Default)]
pub struct AppBars {
    bars: Vec<Bar>,
    posts: Vec<Post>,
    /// The bar that is the taskbar (ABM_GETTASKBARPOS).
    taskbar: Option<u64>,
}

/// What a change did to the work area.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Changed {
    No,
    Yes,
}

impl AppBars {
    pub const fn new() -> Self {
        Self { bars: Vec::new(), posts: Vec::new(), taskbar: None }
    }

    pub fn len(&self) -> usize {
        self.bars.len()
    }

    pub fn is_empty(&self) -> bool {
        self.bars.is_empty()
    }

    /// The bars placed on `edge`.
    pub fn on(&self, edge: u32) -> usize {
        self.bars.iter().filter(|b| b.placed.is_some_and(|(e, _)| e == edge)).count()
    }

    /// The taskbar's edge and strip, once placed (ABM_GETTASKBARPOS).
    pub fn taskbar(&self) -> Option<(u32, Rect)> {
        let hwnd = self.taskbar?;
        self.bars.iter().find(|b| b.hwnd == hwnd)?.placed
    }

    /// Places the taskbar, bar `hwnd` of process `owner`, on `edge` at
    /// `rect` (registered first if it is not a bar yet): its strip, or
    /// None when that cannot be (no memory, not an edge).
    pub fn place_taskbar(
        &mut self,
        owner: u64,
        hwnd: u64,
        edge: u32,
        rect: Rect,
        screen: (i64, i64),
    ) -> Option<Rect> {
        self.add(owner, hwnd, 0).ok()?;
        self.taskbar = Some(hwnd);
        self.set(hwnd, edge, rect, screen)
    }

    /// The taskbar's window: TASKBAR_WINDOW while the desktop draws it,
    /// the shell's once it was handed over (`hand_taskbar`).
    pub fn taskbar_window(&self) -> Option<u64> {
        self.taskbar
    }

    /// The shell takes the taskbar (ARCHITECTURE "The user-mode shell"):
    /// window `hwnd` of process `owner`, told `callback`, becomes the bar
    /// that is the taskbar, in its place (the same edge and strip, so the
    /// work area and the other bars are as they were: none is told). Its
    /// edge and strip, or None when there is no taskbar placed or `hwnd`
    /// is another bar already.
    pub fn hand_taskbar(&mut self, owner: u64, hwnd: u64, callback: u32) -> Option<(u32, Rect)> {
        let old = self.taskbar?;
        if old != hwnd && self.bars.iter().any(|b| b.hwnd == hwnd) {
            return None;
        }
        let bar = self.bars.iter_mut().find(|b| b.hwnd == old)?;
        let placed = bar.placed?;
        *bar = Bar { owner, hwnd, callback, placed: Some(placed) };
        self.taskbar = Some(hwnd);
        Some(placed)
    }

    /// ABM_NEW: false when `hwnd` is a bar already.
    pub fn add(&mut self, owner: u64, hwnd: u64, callback: u32) -> Result<bool, NtStatus> {
        if self.bars.iter().any(|b| b.hwnd == hwnd) {
            return Ok(false);
        }
        self.bars
            .try_push(Bar { owner, hwnd, callback, placed: None })
            .map_err(|_| NtStatus::NO_MEMORY)?;
        Ok(true)
    }

    /// ABM_REMOVE: whether it was a bar, and whether the work area changed
    /// (the other bars on its edge are told).
    pub fn remove(&mut self, hwnd: u64) -> (bool, Changed) {
        let Some(i) = self.bars.iter().position(|b| b.hwnd == hwnd) else {
            return (false, Changed::No);
        };
        let bar = self.bars.remove(i);
        if self.taskbar == Some(hwnd) {
            self.taskbar = None;
        }
        match bar.placed {
            Some((edge, _)) => {
                self.tell(edge, hwnd);
                (true, Changed::Yes)
            }
            None => (true, Changed::No),
        }
    }

    /// The bars of process `owner` go (its end): whether the work area
    /// changed.
    pub fn drop_owned(&mut self, owner: u64) -> Changed {
        let mut changed = Changed::No;
        while let Some(b) = self.bars.iter().find(|b| b.owner == owner).copied() {
            if self.remove(b.hwnd).1 == Changed::Yes {
                changed = Changed::Yes;
            }
        }
        changed
    }

    /// ABM_QUERYPOS: `rect` on `edge` of `screen`, moved off the other
    /// bars on that edge (a side bar also ends above the bars on the
    /// bottom edge) and kept on the screen; None when `hwnd` is not a bar
    /// or the edge is not one.
    pub fn query(&self, hwnd: u64, edge: u32, rect: Rect, screen: (i64, i64)) -> Option<Rect> {
        if edge > ABE_BOTTOM || !self.bars.iter().any(|b| b.hwnd == hwnd) {
            return None;
        }
        let (sw, sh) = screen;
        let (mut l, mut t, mut r, mut b) = (rect.x, rect.y, rect.x + rect.w, rect.y + rect.h);
        let others = self.bars.iter().filter(|o| o.hwnd != hwnd).filter_map(|o| o.placed);
        // The top of the bars on the bottom edge (the taskbar's).
        let floor =
            others.clone().filter(|(e, _)| *e == ABE_BOTTOM).map(|(_, o)| o.y).min().unwrap_or(sh);
        match edge {
            // Beside: from the top down to the bottom bars (the taskbar),
            // inside the bars already there.
            ABE_LEFT | ABE_RIGHT => {
                let (w, bound) = (r - l, if edge == ABE_LEFT { 0 } else { sw });
                let mut at = bound;
                for (e, o) in others.filter(|(e, _)| *e == edge) {
                    at = if e == ABE_LEFT { at.max(o.x + o.w) } else { at.min(o.x) };
                }
                // Against the edge (or the bars there), its width kept.
                if edge == ABE_LEFT {
                    l = at;
                    r = l + w;
                } else {
                    r = at;
                    l = r - w;
                }
                t = 0;
                b = floor;
            }
            // Above or below: the whole width, under the bars already at
            // the top, above the bars at the bottom (the taskbar).
            _ => {
                let h = b - t;
                let mut top = 0;
                let bottom = floor;
                for (e, o) in others {
                    if e == ABE_TOP {
                        top = top.max(o.y + o.h);
                    }
                }
                if edge == ABE_TOP {
                    t = top;
                    b = t + h;
                } else {
                    b = bottom;
                    t = b - h;
                }
                l = 0;
                r = sw;
            }
        }
        Some(Rect { x: l, y: t, w: (r - l).max(0), h: (b - t).max(0) })
    }

    /// ABM_SETPOS: `rect` as QUERYPOS makes it, the bar's strip from now
    /// on; the other bars on its edge are told.
    pub fn set(&mut self, hwnd: u64, edge: u32, rect: Rect, screen: (i64, i64)) -> Option<Rect> {
        let placed = self.query(hwnd, edge, rect, screen)?;
        let bar = self.bars.iter_mut().find(|b| b.hwnd == hwnd)?;
        if bar.placed != Some((edge, placed)) {
            bar.placed = Some((edge, placed));
            self.tell(edge, hwnd);
        }
        Some(placed)
    }

    /// What the bars take from the screen (the taskbar's strip too):
    /// left, top, right and bottom insets.
    pub fn insets(&self, screen: (i64, i64)) -> [i64; 4] {
        let mut ins = [0i64; 4];
        for (edge, r) in self.bars.iter().filter_map(|b| b.placed) {
            match edge {
                ABE_LEFT => ins[0] = ins[0].max(r.x + r.w),
                ABE_TOP => ins[1] = ins[1].max(r.y + r.h),
                ABE_RIGHT => ins[2] = ins[2].max(screen.0 - r.x),
                _ => ins[3] = ins[3].max(screen.1 - r.y),
            }
        }
        for v in &mut ins {
            *v = (*v).max(0);
        }
        ins
    }

    /// Queues ABN_POSCHANGED for the bars on `edge` other than `except`.
    fn tell(&mut self, edge: u32, except: u64) {
        for b in &self.bars {
            if b.hwnd != except && b.callback != 0 && b.placed.is_some_and(|(e, _)| e == edge) {
                let _ = self.posts.try_push(Post {
                    hwnd: b.hwnd,
                    message: b.callback,
                    wparam: ABN_POSCHANGED,
                    lparam: 0,
                });
            }
        }
    }

    pub fn take_posts(&mut self) -> Vec<Post> {
        core::mem::take(&mut self.posts)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCREEN: (i64, i64) = (1280, 800);
    const TB: i64 = 48;

    fn r(l: i64, t: i64, rr: i64, b: i64) -> Rect {
        Rect { x: l, y: t, w: rr - l, h: b - t }
    }

    /// Bars with the desktop's taskbar placed at the bottom.
    fn with_taskbar() -> AppBars {
        let mut a = AppBars::new();
        let tb = r(0, 800 - TB, 1280, 800);
        assert_eq!(a.place_taskbar(0, TASKBAR_WINDOW, ABE_BOTTOM, tb, SCREEN), Some(tb));
        a
    }

    #[test]
    fn registered_once() {
        let mut a = AppBars::new();
        assert_eq!(a.add(4, 0x10, 0x8041), Ok(true));
        assert_eq!(a.add(4, 0x10, 0x8041), Ok(false));
        assert_eq!(a.remove(0x10), (true, Changed::No));
        assert_eq!(a.remove(0x10), (false, Changed::No));
    }

    #[test]
    fn the_taskbar_is_a_bar_on_the_bottom_edge() {
        let mut a = with_taskbar();
        assert_eq!(a.taskbar(), Some((ABE_BOTTOM, r(0, 752, 1280, 800))));
        assert_eq!(a.insets(SCREEN), [0, 0, 0, TB]);
        // Placed again (a new screen size): the same bar, moved.
        let tb = r(0, 720, 1024, 768);
        assert_eq!(a.place_taskbar(0, TASKBAR_WINDOW, ABE_BOTTOM, tb, (1024, 768)), Some(tb));
        assert_eq!(a.len(), 1);
        assert_eq!(a.taskbar(), Some((ABE_BOTTOM, tb)));
        // Removed, it is no longer the taskbar.
        assert_eq!(a.remove(TASKBAR_WINDOW), (true, Changed::Yes));
        assert_eq!(a.taskbar(), None);
    }

    #[test]
    fn right_bar_down_to_the_taskbar_and_inset() {
        let mut a = with_taskbar();
        a.add(4, 0x10, 0x8041).unwrap();
        // Asked for the whole height: it ends above the taskbar.
        let q = a.query(0x10, ABE_RIGHT, r(980, 0, 1280, 800), SCREEN).unwrap();
        assert_eq!(q, r(980, 0, 1280, 752));
        // Off the screen: brought back to its edge.
        let q = a.query(0x10, ABE_RIGHT, r(1000, 0, 1400, 800), SCREEN).unwrap();
        assert_eq!(q, r(880, 0, 1280, 752));
        assert_eq!(
            a.set(0x10, ABE_RIGHT, r(980, 0, 1280, 800), SCREEN),
            Some(r(980, 0, 1280, 752))
        );
        assert_eq!(a.insets(SCREEN), [0, 0, 300, TB]);
        assert!(a.take_posts().is_empty());
    }

    #[test]
    fn without_a_taskbar_a_side_bar_takes_the_whole_height() {
        let mut a = AppBars::new();
        a.add(4, 0x10, 0x8041).unwrap();
        let q = a.query(0x10, ABE_RIGHT, r(980, 0, 1280, 800), SCREEN).unwrap();
        assert_eq!(q, r(980, 0, 1280, 800));
    }

    #[test]
    fn second_bar_goes_beside_and_first_is_told() {
        let mut a = with_taskbar();
        a.add(4, 0x10, 0x8041).unwrap();
        a.add(4, 0x20, 0x8042).unwrap();
        a.set(0x10, ABE_RIGHT, r(980, 0, 1280, 800), SCREEN).unwrap();
        let q = a.query(0x20, ABE_RIGHT, r(1180, 0, 1280, 800), SCREEN).unwrap();
        assert_eq!(q, r(880, 0, 980, 752));
        a.set(0x20, ABE_RIGHT, q, SCREEN).unwrap();
        assert_eq!(a.insets(SCREEN), [0, 0, 400, TB]);
        let posts = a.take_posts();
        assert_eq!(posts.len(), 1);
        assert_eq!((posts[0].hwnd, posts[0].message, posts[0].wparam), (0x10, 0x8041, 1));
        assert_eq!(a.remove(0x20), (true, Changed::Yes));
        assert_eq!(a.take_posts().len(), 1);
        assert_eq!(a.insets(SCREEN), [0, 0, 300, TB]);
        assert_eq!(a.drop_owned(4), Changed::Yes);
        assert_eq!(a.len(), 1);
        assert_eq!(a.insets(SCREEN), [0, 0, 0, TB]);
    }

    #[test]
    fn top_and_bottom_bars() {
        let mut a = with_taskbar();
        a.add(4, 0x10, 1).unwrap();
        a.add(4, 0x20, 2).unwrap();
        a.set(0x10, ABE_TOP, r(0, 0, 1280, 40), SCREEN).unwrap();
        let b = a.set(0x20, ABE_BOTTOM, r(0, 760, 1280, 800), SCREEN).unwrap();
        // Above the taskbar, which stays where it is.
        assert_eq!(b, r(0, 712, 1280, 752));
        assert_eq!(a.insets(SCREEN), [0, 40, 0, 40 + TB]);
        assert_eq!(a.taskbar(), Some((ABE_BOTTOM, r(0, 752, 1280, 800))));
    }

    #[test]
    fn the_shell_takes_the_taskbar_and_gives_it_back_by_going() {
        let mut a = with_taskbar();
        a.add(4, 0x10, 0x8041).unwrap();
        a.set(0x10, ABE_RIGHT, r(980, 0, 1280, 800), SCREEN).unwrap();
        let _ = a.take_posts();
        // Another program's bar cannot be made the taskbar.
        assert_eq!(a.hand_taskbar(5, 0x10, 0x8043), None);
        let strip = r(0, 752, 1280, 800);
        assert_eq!(a.hand_taskbar(5, 0x30, 0x8043), Some((ABE_BOTTOM, strip)));
        assert_eq!(a.taskbar_window(), Some(0x30));
        assert_eq!(a.taskbar(), Some((ABE_BOTTOM, strip)));
        // In place: one bar fewer than a new one, nothing moved, no one told.
        assert_eq!(a.len(), 2);
        assert_eq!(a.insets(SCREEN), [0, 0, 300, TB]);
        assert!(a.take_posts().is_empty());
        // Again by the same window: the same strip.
        assert_eq!(a.hand_taskbar(5, 0x30, 0x8043), Some((ABE_BOTTOM, strip)));
        // The shell ends: no taskbar until the desktop places its own.
        assert_eq!(a.drop_owned(5), Changed::Yes);
        assert_eq!(a.taskbar_window(), None);
        assert_eq!(a.insets(SCREEN), [0, 0, 300, 0]);
        assert_eq!(a.place_taskbar(0, TASKBAR_WINDOW, ABE_BOTTOM, strip, SCREEN), Some(strip));
        assert_eq!(a.taskbar_window(), Some(TASKBAR_WINDOW));
    }

    #[test]
    fn no_taskbar_to_hand_over() {
        let mut a = AppBars::new();
        assert_eq!(a.hand_taskbar(5, 0x30, 1), None);
    }

    #[test]
    fn not_a_bar() {
        let a = AppBars::new();
        assert_eq!(a.query(0x10, ABE_RIGHT, r(0, 0, 1, 1), SCREEN), None);
    }
}
