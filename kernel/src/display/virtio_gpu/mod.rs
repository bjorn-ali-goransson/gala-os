//! The virtio-gpu 2D driver's logic (ARCHITECTURE "Plan: graphics and
//! GPU", stage G1): one scanout shown from a guest-memory resource, damage
//! sent with TRANSFER_TO_HOST_2D + RESOURCE_FLUSH, a hardware cursor,
//! display changes (the device's DISPLAY event) and recovery after a device
//! reset (invariant 4).
//!
//! The device is reached through [`Transport`], so all of this runs on the
//! host against a fake device (`tests.rs`); the kernel's transport is
//! modern virtio-pci (`arch::virtio::ModernDevice`, `display::pci`).
//! Requests are encoded by `proto` (plain data). Every command waits for
//! its answer (the control queue has one request in flight), so no fences
//! are needed: when a command returns, the device is done with it.

pub mod proto;

/// The kernel's log (host unit tests have no serial port to write to).
macro_rules! log {
    ($level:ident, $($arg:tt)*) => {
        #[cfg(not(test))]
        crate::$level!($($arg)*);
    };
}

use alloc::vec::Vec;
use proto::{DeviceError, Mode, Rect};

/// Why the transport could not carry a request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransportError {
    /// No answer in time.
    Timeout,
    /// The device gave up or was reset (DRIVER_OK gone).
    DeviceReset,
    /// The request (or its answer) does not fit the queue's buffer.
    TooLarge,
    /// The queue is not set up.
    NoQueue,
}

/// The device as the driver sees it: two queues (control, cursor), the
/// configuration, and guest memory the device may read.
pub trait Transport {
    type Memory: Memory;
    /// The negotiated feature bits.
    fn features(&self) -> u64;
    /// Sends `req` on the control queue and waits for the answer, written
    /// into `resp`; how many bytes the device wrote.
    fn control(&mut self, req: &[u8], resp: &mut [u8]) -> Result<usize, TransportError>;
    /// Sends `req` on the cursor queue (no answer) and waits until the
    /// device has taken it.
    fn cursor(&mut self, req: &[u8]) -> Result<(), TransportError>;
    /// The largest request plus answer the control queue carries.
    fn max_control(&self) -> usize;
    /// Whether the device must be set up again.
    fn needs_reset(&self) -> bool;
    /// Resets the device and sets it up again (same features and queues).
    fn reinit(&mut self) -> Result<(), TransportError>;
    fn config_u32(&self, offset: u16) -> u32;
    fn set_config_u32(&mut self, offset: u16, value: u32);
    /// Zeroed memory of `bytes` the device can read, in a few physically
    /// contiguous chunks.
    fn alloc(&mut self, bytes: usize) -> Option<Self::Memory>;
}

/// Guest memory a resource is backed by.
pub trait Memory {
    fn len(&self) -> usize;
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
    /// Physically contiguous chunks: (guest-physical address, bytes).
    fn chunks(&self) -> usize;
    fn chunk(&self, index: usize) -> (u64, u32);
    /// Stores `pixels` from byte `offset` (as little-endian u32s).
    fn write(&mut self, offset: usize, pixels: &[u32]);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Transport(TransportError),
    Device(DeviceError),
    NoMemory,
    /// No scanout has a usable mode.
    NoScanout,
    /// The backing has more chunks than one request can describe.
    TooManyChunks,
}

impl From<TransportError> for Error {
    fn from(e: TransportError) -> Self {
        Self::Transport(e)
    }
}

impl From<DeviceError> for Error {
    fn from(e: DeviceError) -> Self {
        Self::Device(e)
    }
}

/// The resolution used when the host reports none (a scanout with no size).
pub const FALLBACK_MODE: (u32, u32) = (1024, 768);
/// The largest mode taken (16 MiB of pixels at 4 bytes: 2048 x 2048 and
/// below, or 3840 x 1080 and so on).
pub const MAX_PIXELS: u64 = 4096 * 1024;

/// What the driver counts (the `stats:` line, `gala`'s view later).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    pub commands: u64,
    pub errors: u64,
    pub resets: u64,
    pub flushes: u64,
    pub mode_changes: u64,
    /// The last failed command and why (for the state dump).
    pub last_error: Option<(u32, Error)>,
}

/// The screen: a resource on a scanout.
struct Screen<M> {
    scanout: u32,
    resource: u32,
    width: u32,
    height: u32,
    memory: M,
}

/// The hardware cursor's image.
struct Cursor<M> {
    resource: u32,
    hot: (u32, u32),
    memory: M,
}

/// The driver. Field order matters: the transport is dropped first (the
/// kernel's resets the device), then the memory the device read.
pub struct Gpu<T: Transport> {
    t: T,
    screen: Option<Screen<T::Memory>>,
    cursor: Option<Cursor<T::Memory>>,
    cursor_at: (u32, u32),
    next_resource: u32,
    pub edid: Option<proto::EdidInfo>,
    pub stats: Stats,
    /// Log every command (`gputrace`).
    pub trace: bool,
}

impl<T: Transport> Gpu<T> {
    /// Takes over the device: reads the display configuration (and the
    /// EDID if offered) and shows a black screen of the host's preferred
    /// size on the first enabled scanout.
    pub fn start(t: T, trace: bool) -> Result<Self, Error> {
        let mut gpu = Self {
            t,
            screen: None,
            cursor: None,
            cursor_at: (0, 0),
            next_resource: 1,
            edid: None,
            stats: Stats::default(),
            trace,
        };
        let modes = gpu.display_info()?;
        let (scanout, (w, h)) = choose_mode(&modes).ok_or(Error::NoScanout)?;
        if gpu.t.features() & proto::F_EDID != 0 {
            gpu.edid = gpu.read_edid(scanout).ok().flatten();
        }
        gpu.set_mode(scanout, w, h)?;
        Ok(gpu)
    }

    /// The screen's size.
    pub fn size(&self) -> Option<(u32, u32)> {
        self.screen.as_ref().map(|s| (s.width, s.height))
    }

    pub fn features(&self) -> u64 {
        self.t.features()
    }

    /// The transport (the kernel's recovery test resets the device).
    pub fn transport_mut(&mut self) -> &mut T {
        &mut self.t
    }

    /// Writes `0x00RRGGBB` pixels from (x, y) to the right (clipped) into
    /// the screen's memory; [`flush`](Self::flush) shows them.
    pub fn write_row(&mut self, x: u32, y: u32, rgb: &[u32]) {
        let Some(s) = self.screen.as_mut() else { return };
        if y >= s.height || x >= s.width {
            return;
        }
        let n = rgb.len().min((s.width - x) as usize);
        let offset = (y as usize * s.width as usize + x as usize) * 4;
        s.memory.write(offset, &rgb[..n]);
    }

    /// Shows area `r` of the screen's memory: TRANSFER_TO_HOST_2D, then
    /// RESOURCE_FLUSH. A device that was reset is set up again (which
    /// shows the whole screen).
    pub fn flush(&mut self, r: Rect) -> Result<(), Error> {
        let Some(s) = self.screen.as_ref() else { return Ok(()) };
        let Some(r) = r.clip(s.width, s.height) else { return Ok(()) };
        let (resource, width) = (s.resource, s.width);
        self.stats.flushes += 1;
        let offset = (u64::from(r.y) * u64::from(width) + u64::from(r.x)) * 4;
        let result = self
            .nodata(&proto::transfer_to_host_2d(resource, r, offset))
            .and_then(|()| self.nodata(&proto::resource_flush(resource, r)));
        match result {
            Err(e) if self.t.needs_reset() => self.recover(e),
            other => other,
        }
    }

    /// Polls the device's events: on a display change, reads the display
    /// configuration again and, if the screen's scanout has a new size,
    /// switches to it. The new size, if it changed.
    pub fn poll(&mut self) -> Result<Option<(u32, u32)>, Error> {
        if self.t.needs_reset() {
            self.recover(Error::Transport(TransportError::DeviceReset))?;
        }
        let events = self.t.config_u32(proto::CONFIG_EVENTS_READ);
        if events == 0 {
            return Ok(None);
        }
        self.t.set_config_u32(proto::CONFIG_EVENTS_CLEAR, events);
        if events & proto::EVENT_DISPLAY == 0 {
            return Ok(None);
        }
        let modes = self.display_info()?;
        let current = self.screen.as_ref().map(|s| (s.scanout, s.width, s.height));
        // The current scanout's new size, else whichever scanout is on.
        let wanted = current
            .and_then(|(id, ..)| {
                let m = modes.get(id as usize)?;
                usable(m).then_some((id, (m.rect.width, m.rect.height)))
            })
            .or_else(|| choose_mode(&modes));
        let Some((scanout, (w, h))) = wanted else { return Ok(None) };
        if current == Some((scanout, w, h)) {
            return Ok(None);
        }
        self.set_mode(scanout, w, h)?;
        Ok(Some((w, h)))
    }

    /// Shows a new w x h resource on `scanout`, then lets the old one go.
    pub fn set_mode(&mut self, scanout: u32, w: u32, h: u32) -> Result<(), Error> {
        if w == 0 || h == 0 || u64::from(w) * u64::from(h) > MAX_PIXELS {
            return Err(Error::Device(DeviceError::InvalidParameter));
        }
        let memory = self.t.alloc(w as usize * h as usize * 4).ok_or(Error::NoMemory)?;
        let resource = self.new_resource();
        let rect = Rect::new(0, 0, w, h);
        self.nodata(&proto::resource_create_2d(resource, proto::FORMAT_B8G8R8X8_UNORM, w, h))?;
        let shown = self
            .attach(resource, &memory)
            .and_then(|()| self.nodata(&proto::set_scanout(scanout, resource, rect)))
            .and_then(|()| self.nodata(&proto::transfer_to_host_2d(resource, rect, 0)))
            .and_then(|()| self.nodata(&proto::resource_flush(resource, rect)));
        if let Err(e) = shown {
            // Unused: let the device forget it (it may have the memory).
            self.release(resource);
            return Err(e);
        }
        let old = self.screen.replace(Screen { scanout, resource, width: w, height: h, memory });
        if let Some(old) = old {
            self.release(old.resource);
            // The device has detached it: its memory may go now.
            drop(old);
        }
        self.stats.mode_changes += 1;
        crate::events::emit(
            crate::events::Category::Display,
            "display mode",
            format_args!(r#""scanout":{scanout},"width":{w},"height":{h}"#),
        );
        Ok(())
    }

    /// The hardware cursor: a 64 x 64 `0xAARRGGBB` image with its hot spot
    /// (None hides it).
    pub fn set_cursor(&mut self, image: Option<(&[u32], (u32, u32))>) -> Result<(), Error> {
        let scanout = self.screen.as_ref().map_or(0, |s| s.scanout);
        let Some((pixels, hot)) = image else {
            let (x, y) = self.cursor_at;
            self.cursor_cmd(&proto::cursor(true, scanout, x, y, 0, (0, 0)))?;
            if let Some(old) = self.cursor.take() {
                self.release(old.resource);
            }
            return Ok(());
        };
        let side = proto::CURSOR_SIZE;
        let mut memory = self.t.alloc((side * side * 4) as usize).ok_or(Error::NoMemory)?;
        for (row, chunk) in pixels.chunks(side as usize).take(side as usize).enumerate() {
            memory.write(row * side as usize * 4, chunk);
        }
        let resource = self.new_resource();
        let rect = Rect::new(0, 0, side, side);
        self.nodata(&proto::resource_create_2d(
            resource,
            proto::FORMAT_B8G8R8A8_UNORM,
            side,
            side,
        ))?;
        let uploaded = self
            .attach(resource, &memory)
            .and_then(|()| self.nodata(&proto::transfer_to_host_2d(resource, rect, 0)));
        if let Err(e) = uploaded {
            self.release(resource);
            return Err(e);
        }
        let (x, y) = self.cursor_at;
        self.cursor_cmd(&proto::cursor(true, scanout, x, y, resource, hot))?;
        if let Some(old) = self.cursor.replace(Cursor { resource, hot, memory }) {
            self.release(old.resource);
        }
        Ok(())
    }

    /// Moves the hardware cursor's hot spot to (x, y).
    pub fn move_cursor(&mut self, x: u32, y: u32) -> Result<(), Error> {
        self.cursor_at = (x, y);
        if self.cursor.is_none() {
            return Ok(());
        }
        let scanout = self.screen.as_ref().map_or(0, |s| s.scanout);
        self.cursor_cmd(&proto::cursor(false, scanout, x, y, 0, (0, 0)))
    }

    /// Sets the device up again after it was reset or gave up (invariant
    /// 4): the transport again, then the screen's resource from the memory
    /// it still has (so the screen comes back as it was, no redraw needed)
    /// and the cursor. `cause` is what failed.
    fn recover(&mut self, cause: Error) -> Result<(), Error> {
        self.stats.resets += 1;
        log!(warn, "display: virtio-gpu was reset or gave up ({cause:?}); setting it up again");
        crate::events::emit(
            crate::events::Category::Display,
            "display reset",
            format_args!(r#""resets":{}"#, self.stats.resets),
        );
        let mut last = cause;
        for _ in 0..3 {
            match self.restore() {
                Ok(()) => return Ok(()),
                Err(e) => last = e,
            }
        }
        Err(last)
    }

    fn restore(&mut self) -> Result<(), Error> {
        self.t.reinit()?;
        // The device forgot every resource; ids start again.
        self.next_resource = 1;
        if let Some(s) = self.screen.take() {
            let resource = self.new_resource();
            let rect = Rect::new(0, 0, s.width, s.height);
            let r = self
                .nodata(&proto::resource_create_2d(
                    resource,
                    proto::FORMAT_B8G8R8X8_UNORM,
                    s.width,
                    s.height,
                ))
                .and_then(|()| self.attach(resource, &s.memory))
                .and_then(|()| self.nodata(&proto::set_scanout(s.scanout, resource, rect)))
                .and_then(|()| self.nodata(&proto::transfer_to_host_2d(resource, rect, 0)))
                .and_then(|()| self.nodata(&proto::resource_flush(resource, rect)));
            self.screen = Some(Screen { resource, ..s });
            r?;
        }
        if let Some(c) = self.cursor.take() {
            let resource = self.new_resource();
            let side = proto::CURSOR_SIZE;
            let rect = Rect::new(0, 0, side, side);
            let r = self
                .nodata(&proto::resource_create_2d(
                    resource,
                    proto::FORMAT_B8G8R8A8_UNORM,
                    side,
                    side,
                ))
                .and_then(|()| self.attach(resource, &c.memory))
                .and_then(|()| self.nodata(&proto::transfer_to_host_2d(resource, rect, 0)));
            let hot = c.hot;
            self.cursor = Some(Cursor { resource, ..c });
            r?;
            let scanout = self.screen.as_ref().map_or(0, |s| s.scanout);
            let (x, y) = self.cursor_at;
            self.cursor_cmd(&proto::cursor(true, scanout, x, y, resource, hot))?;
        }
        Ok(())
    }

    fn new_resource(&mut self) -> u32 {
        let id = self.next_resource;
        self.next_resource = self.next_resource.wrapping_add(1).max(1);
        id
    }

    /// Detaches `resource`'s memory and destroys it (errors only logged:
    /// the device forgets everything on a reset anyway).
    fn release(&mut self, resource: u32) {
        let _ = self.nodata(&proto::resource_detach_backing(resource));
        let _ = self.nodata(&proto::resource_unref(resource));
    }

    fn attach(&mut self, resource: u32, memory: &T::Memory) -> Result<(), Error> {
        let len = proto::attach_backing_size(memory.chunks());
        if len + proto::HEADER > self.t.max_control() {
            return Err(Error::TooManyChunks);
        }
        let mut req = Vec::new();
        req.try_reserve_exact(len).map_err(|_| Error::NoMemory)?;
        req.resize(len, 0);
        let chunks = (0..memory.chunks()).map(|i| memory.chunk(i));
        proto::attach_backing(&mut req, resource, chunks).ok_or(Error::TooManyChunks)?;
        self.exchange(&req, proto::RESP_OK_NODATA, &mut [0; proto::HEADER])
    }

    fn display_info(&mut self) -> Result<[Mode; proto::MAX_SCANOUTS], Error> {
        let mut resp = [0u8; proto::DISPLAY_INFO_SIZE];
        let req = proto::get_display_info();
        self.exchange(req.as_bytes(), proto::RESP_OK_DISPLAY_INFO, &mut resp)?;
        Ok(proto::display_info(&resp)?)
    }

    fn read_edid(&mut self, scanout: u32) -> Result<Option<proto::EdidInfo>, Error> {
        let mut resp = Vec::new();
        resp.try_reserve_exact(proto::EDID_RESPONSE_SIZE).map_err(|_| Error::NoMemory)?;
        resp.resize(proto::EDID_RESPONSE_SIZE, 0);
        let req = proto::get_edid(scanout);
        self.exchange(req.as_bytes(), proto::RESP_OK_EDID, &mut resp)?;
        Ok(proto::edid(&resp).ok().and_then(proto::parse_edid))
    }

    fn nodata(&mut self, req: &proto::Request) -> Result<(), Error> {
        self.exchange(req.as_bytes(), proto::RESP_OK_NODATA, &mut [0; proto::HEADER])
    }

    /// One control command: sent, answered, the answer checked.
    fn exchange(&mut self, req: &[u8], want: u32, resp: &mut [u8]) -> Result<(), Error> {
        self.stats.commands += 1;
        let kind = proto::le32(req, 0);
        let result = self
            .t
            .control(req, resp)
            .map_err(Error::from)
            .and_then(|n| Ok(proto::check(&resp[..n.min(resp.len())], want)?));
        self.note(kind, result)
    }

    fn cursor_cmd(&mut self, req: &proto::Request) -> Result<(), Error> {
        self.stats.commands += 1;
        let result = self.t.cursor(req.as_bytes()).map_err(Error::from);
        self.note(req.kind(), result)
    }

    /// Counts, traces and reports a command's result.
    fn note(&mut self, kind: u32, result: Result<(), Error>) -> Result<(), Error> {
        if self.trace {
            log!(info, "gputrace: {} -> {:?}", proto::name(kind), result);
        }
        if let Err(e) = result {
            self.stats.errors += 1;
            self.stats.last_error = Some((kind, e));
            crate::events::emit(
                crate::events::Category::Display,
                "display error",
                format_args!(r#""command":"{}","error":"{e:?}""#, proto::name(kind)),
            );
        }
        result
    }
}

fn usable(m: &Mode) -> bool {
    m.enabled
        && m.rect.width > 0
        && m.rect.height > 0
        && u64::from(m.rect.width) * u64::from(m.rect.height) <= MAX_PIXELS
}

/// The first enabled scanout with a usable size, else scanout 0 at
/// FALLBACK_MODE (a device that reports nothing still shows scanout 0).
pub fn choose_mode(modes: &[Mode; proto::MAX_SCANOUTS]) -> Option<(u32, (u32, u32))> {
    modes
        .iter()
        .position(usable)
        .map(|i| (i as u32, (modes[i].rect.width, modes[i].rect.height)))
        .or(Some((0, FALLBACK_MODE)))
}

#[cfg(test)]
mod tests;
