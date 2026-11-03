//! The virtio-gpu driver's transport in the kernel: the device's modern
//! virtio-pci interface (`arch::virtio::ModernDevice`), its control queue
//! (one request in flight, answered in the same slot) and cursor queue,
//! and guest memory for resources (`arch::virtio::DmaBuffer`).
//!
//! A request is waited for by spinning (bounded by REPLY_TIMEOUT_NS): the
//! desktop flushes under its lock with interrupts off, and QEMU answers in
//! microseconds. A request with no answer in time marks the device broken,
//! so the driver resets it (invariant 4).

use super::virtio_gpu::{Memory, Transport, TransportError, proto};
use crate::arch::pci::{self, Function};
use crate::arch::virtio::{DmaBuffer, InitError, ModernDevice};

const VENDOR_VIRTIO: u16 = 0x1af4;
/// Modern device ids are 0x1040 + the device type; virtio-gpu is type 16.
const DEVICE_GPU: u16 = 0x1050;

const CONTROL: u16 = 0;
const CURSOR: u16 = 1;
/// One control request and its answer: room for ATTACH_BACKING with 500
/// chunks, or GET_EDID's 1 KiB answer.
const CONTROL_SLOT: usize = 8192;
const CURSOR_SLOT: usize = 64;
/// The largest memory chunk asked for (1 MiB); fewer pages when memory is
/// fragmented.
const MAX_CHUNK_PAGES: usize = 256;
const MAX_CHUNKS: usize = (CONTROL_SLOT - 2 * proto::HEADER - 8) / proto::MEM_ENTRY;
const REPLY_TIMEOUT_NS: u64 = 2_000_000_000;

/// The features gala uses: EDID (the 3D ones come with stage G3).
const WANTED: u64 = proto::F_EDID;

#[derive(Debug)]
pub enum Error {
    NotFound,
    Init(InitError),
}

impl From<InitError> for Error {
    fn from(e: InitError) -> Self {
        Self::Init(e)
    }
}

pub struct VirtioGpuPci {
    dev: ModernDevice,
    pub at: pci::Address,
    /// A request went unanswered: the device must be reset.
    broken: bool,
}

impl VirtioGpuPci {
    /// Finds the first virtio-gpu function and starts it.
    pub fn probe() -> Result<Self, Error> {
        let mut found: Option<Function> = None;
        pci::for_each(|f| {
            if f.vendor == VENDOR_VIRTIO && f.device == DEVICE_GPU && found.is_none() {
                found = Some(*f);
            }
        });
        let function = found.ok_or(Error::NotFound)?;
        let mut gpu = Self {
            dev: ModernDevice::new(&function, WANTED)?,
            at: function.address,
            broken: false,
        };
        gpu.queues()?;
        Ok(gpu)
    }

    fn queues(&mut self) -> Result<(), InitError> {
        self.dev.setup_chained_queue(CONTROL, 1, CONTROL_SLOT)?;
        self.dev.setup_queue(CURSOR, 1, CURSOR_SLOT, false)?;
        self.dev.start();
        Ok(())
    }

    /// For `gpureset=`: resets the device under the driver.
    pub fn reset_now(&mut self) {
        self.dev.reset_now();
    }

    /// Waits for the queue's one slot to come back; how many bytes the
    /// device wrote.
    fn wait(&mut self, queue: u16) -> Result<usize, TransportError> {
        let start = crate::time::raw_monotonic_ns();
        loop {
            let q = self.dev.queue(queue).ok_or(TransportError::NoQueue)?;
            if let Some((_, written)) = q.pop_used() {
                return Ok(written);
            }
            if crate::time::raw_monotonic_ns().saturating_sub(start) > REPLY_TIMEOUT_NS {
                self.broken = true;
                return Err(if self.dev.needs_reset() {
                    TransportError::DeviceReset
                } else {
                    TransportError::Timeout
                });
            }
            if self.dev.needs_reset() {
                self.broken = true;
                return Err(TransportError::DeviceReset);
            }
            core::hint::spin_loop();
        }
    }
}

impl Transport for VirtioGpuPci {
    type Memory = DmaBuffer;

    fn features(&self) -> u64 {
        self.dev.features()
    }

    fn control(&mut self, req: &[u8], resp: &mut [u8]) -> Result<usize, TransportError> {
        if self.broken || self.dev.needs_reset() {
            return Err(TransportError::DeviceReset);
        }
        let q = self.dev.queue(CONTROL).ok_or(TransportError::NoQueue)?;
        if req.len() + resp.len() > q.slot_size() {
            return Err(TransportError::TooLarge);
        }
        if !(q.write_slot(0, 0, req) && q.post_chain(0, req.len(), resp.len())) {
            return Err(TransportError::NoQueue);
        }
        self.dev.notify(CONTROL);
        let written = self.wait(CONTROL)?.min(resp.len());
        let q = self.dev.queue(CONTROL).ok_or(TransportError::NoQueue)?;
        if !q.read_slot(0, req.len(), &mut resp[..written]) {
            return Err(TransportError::NoQueue);
        }
        Ok(written)
    }

    fn cursor(&mut self, req: &[u8]) -> Result<(), TransportError> {
        if self.broken || self.dev.needs_reset() {
            return Err(TransportError::DeviceReset);
        }
        let q = self.dev.queue(CURSOR).ok_or(TransportError::NoQueue)?;
        if !(q.write_slot(0, 0, req) && q.post(0, req.len())) {
            return Err(TransportError::TooLarge);
        }
        self.dev.notify(CURSOR);
        self.wait(CURSOR).map(|_| ())
    }

    fn max_control(&self) -> usize {
        CONTROL_SLOT
    }

    fn needs_reset(&self) -> bool {
        self.broken || self.dev.needs_reset()
    }

    fn reinit(&mut self) -> Result<(), TransportError> {
        self.dev.reinit().map_err(|_| TransportError::DeviceReset)?;
        self.queues().map_err(|_| TransportError::NoQueue)?;
        self.broken = false;
        Ok(())
    }

    fn config_u32(&self, offset: u16) -> u32 {
        self.dev.config_u32(offset)
    }

    fn set_config_u32(&mut self, offset: u16, value: u32) {
        self.dev.set_config_u32(offset, value);
    }

    fn alloc(&mut self, bytes: usize) -> Option<DmaBuffer> {
        DmaBuffer::new(bytes, MAX_CHUNK_PAGES, MAX_CHUNKS)
    }
}

impl Memory for DmaBuffer {
    fn len(&self) -> usize {
        DmaBuffer::len(self)
    }
    fn chunks(&self) -> usize {
        DmaBuffer::chunks(self)
    }
    fn chunk(&self, index: usize) -> (u64, u32) {
        DmaBuffer::chunk(self, index)
    }
    fn write(&mut self, offset: usize, pixels: &[u32]) {
        self.write_u32s(offset, pixels);
    }
}
