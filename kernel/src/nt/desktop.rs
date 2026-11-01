//! The desktop: top-level windows of Win32 programs, composited on the
//! screen (the window manager's first milestone). The kernel keeps each
//! window's frame (a 1-pixel border and a caption bar with its title, drawn
//! here) and a copy of its client area, which the program's user32/gdi32
//! draw in its own memory and present (`present`). The screen is redrawn
//! where something changed: each pixel is the topmost visible window's
//! (frame or client), else the desktop's colour. Windows are in z-order
//! (the last created is on top) and go with their process.
//! Design: docs/ARCHITECTURE.md, "Windows".

use super::platform;
use super::status::NtStatus;
use crate::mm::heap::TryVec;
use crate::sync::IrqMutex;
use alloc::vec::Vec;
use noto_sans_mono_bitmap::{FontWeight, RasterHeight, get_raster, get_raster_width};

/// Frame metrics (what user32's GetSystemMetrics reports).
pub const BORDER: u32 = 1;
pub const CAPTION: u32 = 24;
pub const DESKTOP_COLOR: u32 = 0x00_3a_6e_a5;
pub const BORDER_COLOR: u32 = 0x00_20_20_20;
pub const CAPTION_COLOR: u32 = 0x00_00_6c_35;
pub const CAPTION_TEXT: u32 = 0x00_ff_ff_ff;
/// The largest window gala accepts (each pixel is kept twice).
const MAX_SIDE: u32 = 4096;

pub struct Window {
    pub id: u32,
    pub owner: u64,
    x: i32,
    y: i32,
    /// The outer size (frame included).
    w: u32,
    h: u32,
    visible: bool,
    /// The caption bar's pixels (w * CAPTION).
    caption: Vec<u32>,
    /// The client area's pixels (client_w * client_h), white at first.
    client: Vec<u32>,
}

impl Window {
    fn client_size(&self) -> (u32, u32) {
        (self.w - 2 * BORDER, self.h - 2 * BORDER - CAPTION)
    }

    /// The colour of screen point (sx, sy) if this window covers it.
    fn pixel(&self, sx: i64, sy: i64) -> Option<u32> {
        let (x, y) = (sx - i64::from(self.x), sy - i64::from(self.y));
        if !self.visible || x < 0 || y < 0 || x >= i64::from(self.w) || y >= i64::from(self.h) {
            return None;
        }
        let (x, y) = (x as u32, y as u32);
        if x < BORDER || y < BORDER || x >= self.w - BORDER || y >= self.h - BORDER {
            return Some(BORDER_COLOR);
        }
        if y < BORDER + CAPTION {
            return Some(self.caption[((y - BORDER) * self.w + x) as usize]);
        }
        let (cw, _) = self.client_size();
        let (cx, cy) = (x - BORDER, y - BORDER - CAPTION);
        Some(self.client[(cy * cw + cx) as usize])
    }

    /// The screen rectangle it covers.
    fn rect(&self) -> Rect {
        Rect { x: i64::from(self.x), y: i64::from(self.y), w: self.w.into(), h: self.h.into() }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: i64,
    pub y: i64,
    pub w: i64,
    pub h: i64,
}

impl Rect {
    /// Clipped to the screen `(0, 0, sw, sh)`.
    fn clip(self, sw: i64, sh: i64) -> Option<Rect> {
        let (x0, y0) = (self.x.max(0), self.y.max(0));
        let (x1, y1) = ((self.x + self.w).min(sw), (self.y + self.h).min(sh));
        (x1 > x0 && y1 > y0).then_some(Rect { x: x0, y: y0, w: x1 - x0, h: y1 - y0 })
    }
}

pub struct Desktop {
    /// Bottom to top.
    windows: Vec<Window>,
    next_id: u32,
    /// The whole screen has been painted once (the console drew it before).
    painted: bool,
}

impl Default for Desktop {
    fn default() -> Self {
        Self::new()
    }
}

impl Desktop {
    pub const fn new() -> Self {
        Desktop { windows: Vec::new(), next_id: 1, painted: false }
    }

    /// The colour of screen point (x, y).
    pub fn pixel(&self, x: i64, y: i64) -> u32 {
        self.windows.iter().rev().find_map(|w| w.pixel(x, y)).unwrap_or(DESKTOP_COLOR)
    }

    fn find(&mut self, id: u32, owner: u64) -> Result<&mut Window, NtStatus> {
        self.windows
            .iter_mut()
            .find(|w| w.id == id && w.owner == owner)
            .ok_or(NtStatus::INVALID_HANDLE)
    }

    /// A new hidden window of outer size w x h at (x, y) titled `title`.
    pub fn create(
        &mut self,
        owner: u64,
        x: i32,
        y: i32,
        w: u32,
        h: u32,
        title: &[u16],
    ) -> Result<u32, NtStatus> {
        if w < 2 * BORDER + 1 || h < 2 * BORDER + CAPTION + 1 || w > MAX_SIDE || h > MAX_SIDE {
            return Err(NtStatus::INVALID_PARAMETER);
        }
        let mut caption = Vec::new();
        caption.try_reserve_exact((w * CAPTION) as usize).map_err(|_| NtStatus::NO_MEMORY)?;
        caption.resize((w * CAPTION) as usize, CAPTION_COLOR);
        draw_text(&mut caption, w, 6, 4, title, CAPTION_TEXT);
        let mut client = Vec::new();
        let n = ((w - 2 * BORDER) * (h - 2 * BORDER - CAPTION)) as usize;
        client.try_reserve_exact(n).map_err(|_| NtStatus::NO_MEMORY)?;
        client.resize(n, 0x00_ff_ff_ff);
        let id = self.next_id;
        let window = Window { id, owner, x, y, w, h, visible: false, caption, client };
        self.windows.try_push(window).map_err(|_| NtStatus::NO_MEMORY)?;
        self.next_id += 1;
        Ok(id)
    }

    /// Shows or hides a window; the screen area to redraw.
    pub fn show(&mut self, id: u32, owner: u64, visible: bool) -> Result<Rect, NtStatus> {
        let win = self.find(id, owner)?;
        win.visible = visible;
        Ok(win.rect())
    }

    /// Copies a w x h rectangle at (x, y) of the client area from `pixels`
    /// (rows of `stride` pixels); the screen area to redraw.
    pub fn present(
        &mut self,
        id: u32,
        owner: u64,
        r: Rect,
        pixels: &[u32],
        stride: usize,
    ) -> Result<Rect, NtStatus> {
        let win = self.find(id, owner)?;
        let (cw, ch) = win.client_size();
        let clipped = r.clip(cw.into(), ch.into()).ok_or(NtStatus::INVALID_PARAMETER)?;
        let (dx, dy) = (clipped.x - r.x, clipped.y - r.y);
        for row in 0..clipped.h {
            let src = ((dy + row) as usize) * stride + dx as usize;
            let dst = ((clipped.y + row) as u32 * cw) as usize + clipped.x as usize;
            let n = clipped.w as usize;
            let Some(from) = pixels.get(src..src + n) else {
                return Err(NtStatus::INVALID_PARAMETER);
            };
            win.client[dst..dst + n].copy_from_slice(from);
        }
        let origin =
            (i64::from(win.x) + i64::from(BORDER), i64::from(win.y) + i64::from(BORDER + CAPTION));
        Ok(Rect { x: origin.0 + clipped.x, y: origin.1 + clipped.y, ..clipped })
    }

    pub fn destroy(&mut self, id: u32, owner: u64) -> Result<Rect, NtStatus> {
        let i = self.windows.iter().position(|w| w.id == id && w.owner == owner);
        let i = i.ok_or(NtStatus::INVALID_HANDLE)?;
        Ok(self.windows.remove(i).rect())
    }

    /// Removes `owner`'s windows; the areas to redraw.
    pub fn destroy_owned(&mut self, owner: u64) -> Vec<Rect> {
        let mut gone = Vec::new();
        self.windows.retain(|w| {
            if w.owner == owner {
                let _ = gone.try_push(w.rect());
                false
            } else {
                true
            }
        });
        gone
    }
}

/// Draws `text` (UTF-16; what the font lacks shows as '?') into a buffer
/// of `width` pixels per row at (x, y), opaque glyph cells blended over
/// what is there.
fn draw_text(buf: &mut [u32], width: u32, x: u32, y: u32, text: &[u16], fg: u32) {
    let cell = get_raster_width(FontWeight::Regular, RasterHeight::Size16) as u32;
    for (i, c) in char::decode_utf16(text.iter().copied()).enumerate() {
        let c = c.unwrap_or('?');
        let raster = get_raster(c, FontWeight::Regular, RasterHeight::Size16)
            .or_else(|| get_raster('?', FontWeight::Regular, RasterHeight::Size16));
        let Some(raster) = raster else { continue };
        let gx = x + i as u32 * cell;
        for (dy, row) in raster.raster().iter().enumerate() {
            for (dx, &coverage) in row.iter().enumerate() {
                let (px, py) = (gx + dx as u32, y + dy as u32);
                if px >= width {
                    continue;
                }
                let Some(p) = buf.get_mut((py * width + px) as usize) else { continue };
                *p = blend(*p, fg, coverage);
            }
        }
    }
}

fn blend(bg: u32, fg: u32, t: u8) -> u32 {
    let t = u32::from(t);
    let mix = |shift: u32| {
        let (a, b) = ((bg >> shift) & 0xff, (fg >> shift) & 0xff);
        ((a * (255 - t) + b * t) / 255) << shift
    };
    mix(16) | mix(8) | mix(0)
}

static DESKTOP: IrqMutex<Desktop> = IrqMutex::new(Desktop::new());

/// Redraws screen area `r` from the desktop.
fn redraw(desktop: &Desktop, r: Rect) {
    let Some((sw, sh)) = platform::screen_size() else { return };
    let Some(r) = r.clip(sw.into(), sh.into()) else { return };
    let mut row = Vec::new();
    if row.try_reserve_exact(r.w as usize).is_err() {
        return;
    }
    for y in r.y..r.y + r.h {
        row.clear();
        row.extend((r.x..r.x + r.w).map(|x| desktop.pixel(x, y)));
        platform::screen_write_row(r.x as u32, y as u32, &row);
    }
}

/// Runs `f` on the desktop and redraws what it changed; STATUS_NOT_SUPPORTED
/// without a screen.
pub fn with<R>(
    f: impl FnOnce(&mut Desktop) -> Result<(R, Option<Rect>), NtStatus>,
) -> Result<R, NtStatus> {
    if platform::screen_size().is_none() {
        return Err(NtStatus::NOT_SUPPORTED);
    }
    let mut d = DESKTOP.lock();
    if !d.painted
        && let Some((w, h)) = platform::screen_size()
    {
        d.painted = true;
        redraw(&d, Rect { x: 0, y: 0, w: w.into(), h: h.into() });
    }
    let (r, area) = f(&mut d)?;
    if let Some(area) = area {
        redraw(&d, area);
    }
    Ok(r)
}

/// A process ended: its windows go.
pub fn process_ended(owner: u64) {
    if platform::screen_size().is_none() {
        return;
    }
    let mut d = DESKTOP.lock();
    for area in d.destroy_owned(owner) {
        redraw(&d, area);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_composite_in_z_order_with_their_frames() {
        let mut d = Desktop::new();
        let t: Vec<u16> = "Hi".encode_utf16().collect();
        let a = d.create(1, 10, 10, 100, 80, &t).unwrap();
        // Hidden until shown.
        assert_eq!(d.pixel(50, 50), DESKTOP_COLOR);
        d.show(a, 1, true).unwrap();
        assert_eq!(d.pixel(10, 10), BORDER_COLOR);
        assert_eq!(d.pixel(100, 15), CAPTION_COLOR); // past the title text
        let client = (10 + BORDER as i64, 10 + (BORDER + CAPTION) as i64);
        assert_eq!(d.pixel(client.0, client.1), 0x00ff_ffff);
        // Present a 2 x 2 red square at client (3, 4).
        let red = [0x00ff_0000u32; 4];
        let area = d.present(a, 1, Rect { x: 3, y: 4, w: 2, h: 2 }, &red, 2).unwrap();
        assert_eq!(area, Rect { x: client.0 + 3, y: client.1 + 4, w: 2, h: 2 });
        assert_eq!(d.pixel(client.0 + 4, client.1 + 5), 0x00ff_0000);
        // Another owner's window on top covers it; only its owner may
        // present to the first.
        let b = d.create(2, 0, 0, 60, 60, &t).unwrap();
        d.show(b, 2, true).unwrap();
        assert_eq!(d.pixel(30, 40), 0x00ff_ffff);
        assert_eq!(
            d.present(a, 2, Rect { x: 0, y: 0, w: 1, h: 1 }, &red, 1).err(),
            Some(NtStatus::INVALID_HANDLE)
        );
        d.destroy_owned(2);
        assert_eq!(d.pixel(client.0 + 4, client.1 + 5), 0x00ff_0000);
        d.destroy(a, 1).unwrap();
        assert_eq!(d.pixel(client.0 + 4, client.1 + 5), DESKTOP_COLOR);
        // The caption shows its title: glyph pixels (anti-aliased, so
        // between the bar's and the text's colours) where the text is.
        let c = d.create(3, 0, 0, 100, 40, &t).unwrap();
        d.show(c, 3, true).unwrap();
        let text = (1..30).flat_map(|x| (1..24).map(move |y| (x, y)));
        assert!(text.clone().any(|(x, y)| d.pixel(x, y) != CAPTION_COLOR));
    }
}
