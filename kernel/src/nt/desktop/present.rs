//! Presenters (ROADMAP RS step B3a; ARCHITECTURE "Presenters (B3a)"): what
//! shows a session's desktop. The desktop keeps its windows and their last
//! pixels whatever shows it (window state apart from composition); a
//! presenter is handed the composed rows of what changed and shows them:
//!
//! - **Console**: the local screen through `platform::Display`;
//! - **None**: nothing (disconnected): nothing is composed, the windows
//!   stay alive and are drawn whole when a presenter attaches;
//! - **Test**: a headless frame in memory (`Frame`), which a program reads
//!   with GalaSession and xtask's screen checks read with QEMU's `memsave`
//!   (`presenter=test`).

use super::Rect;
use super::platform;
use alloc::vec::Vec;

/// The size of a desktop no screen sizes (the window state's, which bare
/// desktops have too).
pub use super::super::winstate::DEFAULT_SIZE;

/// The largest side of a Test frame (as of a window).
pub const MAX_SIDE: u32 = 4096;

/// The kinds of presenter (GalaSession's numbers).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    None = 0,
    Console = 1,
    Test = 2,
}

impl Kind {
    pub fn from_number(n: u64) -> Option<Kind> {
        match n {
            0 => Some(Kind::None),
            1 => Some(Kind::Console),
            2 => Some(Kind::Test),
            _ => None,
        }
    }

    /// Its name in the log, the events and the boot option.
    pub fn name(self) -> &'static str {
        match self {
            Kind::None => "none",
            Kind::Console => "console",
            Kind::Test => "test",
        }
    }
}

/// A headless presenter's picture: the composed desktop in memory,
/// 0x00RRGGBB, `w` pixels a row. Made once at its size (its address is
/// logged for xtask and never changes).
pub struct Frame {
    w: u32,
    h: u32,
    pixels: Vec<u32>,
    /// Areas shown so far (one per redrawn damage rectangle).
    flushes: u64,
}

impl Frame {
    /// A black w x h frame; None when the size is out of range or there is
    /// no memory.
    pub fn new(w: u32, h: u32) -> Option<Frame> {
        if w == 0 || h == 0 || w > MAX_SIDE || h > MAX_SIDE {
            return None;
        }
        let n = w as usize * h as usize;
        let mut pixels = Vec::new();
        pixels.try_reserve_exact(n).ok()?;
        pixels.resize(n, 0);
        Some(Frame { w, h, pixels, flushes: 0 })
    }

    pub fn size(&self) -> (u32, u32) {
        (self.w, self.h)
    }

    pub fn flushes(&self) -> u64 {
        self.flushes
    }

    /// Where its pixels are (the `presenter: test frame` log line).
    pub fn address(&self) -> usize {
        self.pixels.as_ptr() as usize
    }

    fn write_row(&mut self, x: u32, y: u32, rgb: &[u32]) {
        if y >= self.h || x >= self.w {
            return;
        }
        let n = rgb.len().min((self.w - x) as usize);
        let at = (y * self.w + x) as usize;
        self.pixels[at..at + n].copy_from_slice(&rgb[..n]);
    }

    /// The pixel at (x, y), if inside.
    pub fn pixel(&self, x: u32, y: u32) -> Option<u32> {
        (x < self.w && y < self.h).then(|| self.pixels[(y * self.w + x) as usize])
    }

    /// Copies area `r` (inside the frame) to `out`, row after row; false
    /// when it is not inside or `out` is too small.
    pub fn read(&self, r: Rect, out: &mut [u32]) -> bool {
        let inside = r.x >= 0
            && r.y >= 0
            && r.w > 0
            && r.h > 0
            && r.x + r.w <= i64::from(self.w)
            && r.y + r.h <= i64::from(self.h);
        if !inside || out.len() < (r.w * r.h) as usize {
            return false;
        }
        let w = r.w as usize;
        for (row, y) in (r.y..r.y + r.h).enumerate() {
            let at = y as usize * self.w as usize + r.x as usize;
            out[row * w..row * w + w].copy_from_slice(&self.pixels[at..at + w]);
        }
        true
    }
}

/// What shows a desktop.
#[derive(Default)]
pub enum Presenter {
    #[default]
    None,
    Console,
    Test(Frame),
}

impl Presenter {
    pub fn kind(&self) -> Kind {
        match self {
            Presenter::None => Kind::None,
            Presenter::Console => Kind::Console,
            Presenter::Test(_) => Kind::Test,
        }
    }

    /// The size it shows: the screen's, the frame's; None for None (the
    /// desktop keeps its own).
    pub fn size(&self) -> Option<(u32, u32)> {
        match self {
            Presenter::None => None,
            Presenter::Console => platform::screen_size(),
            Presenter::Test(f) => Some(f.size()),
        }
    }

    /// Writes composed pixels from (x, y) to the right (clipped).
    pub fn write_row(&mut self, x: u32, y: u32, rgb: &[u32]) {
        match self {
            Presenter::None => {}
            Presenter::Console => platform::screen_write_row(x, y, rgb),
            Presenter::Test(f) => f.write_row(x, y, rgb),
        }
    }

    /// The rows of area (x, y, w, h) are written: show them (a GPU copies
    /// them to the host and flushes; a frame counts a flush).
    pub fn flush(&mut self, x: u32, y: u32, w: u32, h: u32) {
        match self {
            Presenter::None => {}
            Presenter::Console => platform::screen_flush(x, y, w, h),
            Presenter::Test(f) => f.flushes += 1,
        }
    }
}

/// `presenter=` on the command line: what shows the console session's
/// desktop (none given: the screen, if the machine has one). 0: none
/// given; else 1 + the kind.
static LOCAL: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

/// The boot option `presenter=<test|none>`: the console session's desktop
/// is shown by a Test frame or by nothing, with or without a screen, and
/// every session's desktop is live (a headless desktop). False for
/// another value.
pub fn set_local(name: &str) -> bool {
    let kind = match name {
        "test" => Kind::Test,
        "none" => Kind::None,
        _ => return false,
    };
    LOCAL.store(1 + kind as u32, core::sync::atomic::Ordering::Relaxed);
    true
}

/// The kind `presenter=` asked for, if any.
pub fn local() -> Option<Kind> {
    match LOCAL.load(core::sync::atomic::Ordering::Relaxed) {
        0 => None,
        n => Kind::from_number(u64::from(n - 1)),
    }
}
