//! The screen's device when it is not the bootloader's framebuffer:
//! virtio-gpu (ARCHITECTURE "Plan: graphics and GPU", stage G1). Chosen
//! with the boot option `display=virtio` (default `display=fb`: the
//! Limine framebuffer, as before); without a virtio-gpu the framebuffer is
//! used. `platform::Display` reaches it through `size`, `write_row`,
//! `flush` and `poll`.
//!
//! The driver's logic is `virtio_gpu` (plain data and a `Transport`
//! trait, tested on the host against a fake device); `pci` is the kernel's
//! transport.

pub mod pci;
pub mod virtio_gpu;

pub use kernel_side::*;

mod kernel_side {
    use super::pci::VirtioGpuPci;
    use super::virtio_gpu::{Gpu, proto::Rect};
    use crate::sync::IrqMutex;
    use core::sync::atomic::{AtomicU64, Ordering};

    static GPU: IrqMutex<Option<Gpu<VirtioGpuPci>>> = IrqMutex::named("display.gpu", None);

    /// `gpureset=<n>`: the device is reset under the driver after the n-th
    /// flush (the recovery test; 0: never).
    pub static RESET_AT: AtomicU64 = AtomicU64::new(0);

    /// What `display=` asked for.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Choice {
        /// The bootloader's framebuffer (the default).
        Framebuffer,
        /// virtio-gpu if there is one, else the framebuffer.
        Virtio,
    }

    impl Choice {
        pub fn parse(value: Option<&str>) -> Self {
            match value {
                Some("virtio") => Self::Virtio,
                _ => Self::Framebuffer,
            }
        }
    }

    /// Finds and starts the display device `choice` names (`trace`:
    /// `gputrace`, every command logged).
    pub fn init(choice: Choice, trace: bool) {
        if choice != Choice::Virtio {
            return;
        }
        let transport = match VirtioGpuPci::probe() {
            Ok(t) => t,
            Err(super::pci::Error::NotFound) => {
                warn!("display: no virtio-gpu; using the framebuffer");
                return;
            }
            Err(e) => {
                warn!("display: virtio-gpu not usable ({e:?}); using the framebuffer");
                return;
            }
        };
        let at = transport.at;
        match Gpu::start(transport, trace) {
            Ok(gpu) => {
                let (w, h) = gpu.size().unwrap_or((0, 0));
                info!("display: virtio-gpu at {at}, {w}x{h}, features {:#x}", gpu.features());
                if let Some(e) = gpu.edid {
                    info!(
                        "display: EDID {} product {:#06x}, preferred mode {:?}",
                        core::str::from_utf8(&e.manufacturer).unwrap_or("?"),
                        e.product,
                        e.preferred
                    );
                }
                *GPU.lock() = Some(gpu);
            }
            Err(e) => warn!("display: virtio-gpu did not start ({e:?}); using the framebuffer"),
        }
    }

    /// Whether virtio-gpu is the screen.
    pub fn present() -> bool {
        GPU.lock().is_some()
    }

    pub fn size() -> Option<(u32, u32)> {
        GPU.lock().as_ref().and_then(Gpu::size)
    }

    pub fn write_row(x: u32, y: u32, rgb: &[u32]) {
        if let Some(gpu) = GPU.lock().as_mut() {
            gpu.write_row(x, y, rgb);
        }
    }

    /// Shows what was written in the area (x, y, w, h).
    pub fn flush(x: u32, y: u32, w: u32, h: u32) {
        let mut gpu = GPU.lock();
        let Some(gpu) = gpu.as_mut() else { return };
        if let Err(e) = gpu.flush(Rect::new(x, y, w, h)) {
            warn!("display: flush failed: {e:?}");
        }
        let at = RESET_AT.load(Ordering::Relaxed);
        if at != 0 && gpu.stats.flushes == at {
            warn!("display: resetting virtio-gpu under the driver (gpureset={at})");
            gpu.transport_mut().reset_now();
        }
    }

    /// Checks the device's events (the input thread, every second): the
    /// new screen size after a display change.
    pub fn poll() -> Option<(u32, u32)> {
        let mut gpu = GPU.lock();
        let gpu = gpu.as_mut()?;
        match gpu.poll() {
            Ok(Some((w, h))) => {
                info!("display: the screen is now {w}x{h}");
                Some((w, h))
            }
            Ok(None) => None,
            Err(e) => {
                warn!("display: poll failed: {e:?}");
                None
            }
        }
    }

    /// The `display:` line at the end of a boot.
    pub fn log_stats() {
        let gpu = GPU.lock();
        let Some(gpu) = gpu.as_ref() else { return };
        let s = gpu.stats;
        let (w, h) = gpu.size().unwrap_or((0, 0));
        info!(
            "display: virtio-gpu {w}x{h}: {} commands, {} flushes, {} mode changes, {} errors, {} resets",
            s.commands, s.flushes, s.mode_changes, s.errors, s.resets
        );
        if let Some((kind, e)) = s.last_error {
            info!("display: last error: {} {e:?}", super::virtio_gpu::proto::name(kind));
        }
    }
}
