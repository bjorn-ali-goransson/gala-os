//! virtio-blk over the legacy PCI interface (`arch::virtio`; the virtio
//! 1.x specification's block device chapter, OASIS, see THIRD_PARTY.md).
//! One request queue with one slot: a request is the 16-byte header
//! (type, reserved, sector), the data, and the status byte; the device
//! reads the header (and a write's data) and writes the status (and a
//! read's data), so each request is a two-descriptor chain.

use crate::arch::pci::{self, Function};
use crate::arch::virtio::{InitError, LegacyDevice};

const VENDOR_VIRTIO: u16 = 0x1af4;
/// Transitional virtio-blk (the legacy interface is available).
const DEVICE_BLK_TRANSITIONAL: u16 = 0x1001;
const SUBSYSTEM_BLK: u16 = 2;

const F_RO: u32 = 1 << 5;
const F_FLUSH: u32 = 1 << 9;
const F_ANY_LAYOUT: u32 = 1 << 27;

const T_IN: u32 = 0;
const T_OUT: u32 = 1;
const T_FLUSH: u32 = 4;
const S_OK: u8 = 0;

const QUEUE: u16 = 0;
const HEADER: usize = 16;
/// The most bytes one request moves.
pub const MAX_TRANSFER: usize = 64 * 1024;
const SLOT: usize = HEADER + MAX_TRANSFER + 1;

#[derive(Debug)]
pub enum Error {
    NotFound,
    Init(InitError),
    NoQueue,
}

impl From<InitError> for Error {
    fn from(e: InitError) -> Self {
        Self::Init(e)
    }
}

use super::DiskError;

/// What the request on the device is.
#[derive(Clone, Copy)]
enum InFlight {
    None,
    /// A read of `len` bytes (status after them), or a write or flush
    /// (status at `len`, after the header and data).
    Read {
        len: usize,
    },
    Status {
        at: usize,
    },
}

pub struct VirtioBlk {
    dev: LegacyDevice,
    pub at: pci::Address,
    pub sectors: u64,
    pub read_only: bool,
    flush: bool,
    in_flight: InFlight,
}

impl VirtioBlk {
    /// Finds the first transitional virtio-blk function and starts it.
    pub fn probe() -> Result<Self, Error> {
        let mut found: Option<Function> = None;
        pci::for_each(|f| {
            let blk = f.vendor == VENDOR_VIRTIO
                && f.device == DEVICE_BLK_TRANSITIONAL
                && f.subsystem == SUBSYSTEM_BLK;
            if blk && found.is_none() {
                found = Some(*f);
            }
        });
        let function = found.ok_or(Error::NotFound)?;
        let (mut dev, features) = LegacyDevice::new(&function, F_RO | F_FLUSH | F_ANY_LAYOUT)?;
        // Configuration: capacity (u64, in 512-byte sectors) first.
        let sectors = u64::from(dev.config_u32(0)) | u64::from(dev.config_u32(4)) << 32;
        dev.setup_chained_queue(QUEUE, 1, SLOT)?;
        dev.start();
        Ok(Self {
            dev,
            at: function.address,
            sectors,
            read_only: features & F_RO != 0,
            flush: features & F_FLUSH != 0,
            in_flight: InFlight::None,
        })
    }

    pub fn failed(&self) -> bool {
        self.dev.failed()
    }

    fn post(
        &mut self,
        kind: u32,
        sector: u64,
        data: &[u8],
        read_len: usize,
    ) -> Result<(), DiskError> {
        let mut header = [0u8; HEADER];
        header[..4].copy_from_slice(&kind.to_le_bytes());
        header[8..].copy_from_slice(&sector.to_le_bytes());
        let q = self.dev.queue(QUEUE).ok_or(DiskError::Device)?;
        let readable = HEADER + data.len();
        let ok = q.write_slot(0, 0, &header)
            && q.write_slot(0, HEADER, data)
            // The status byte starts as an error the device overwrites.
            && q.write_slot(0, readable + read_len, &[0xff])
            && q.post_chain(0, readable, read_len + 1);
        if !ok {
            return Err(DiskError::Device);
        }
        self.dev.notify(QUEUE);
        Ok(())
    }

    pub fn start_read(&mut self, sector: u64, len: usize) -> Result<(), DiskError> {
        self.post(T_IN, sector, &[], len)?;
        self.in_flight = InFlight::Read { len };
        Ok(())
    }

    pub fn start_write(&mut self, sector: u64, data: &[u8]) -> Result<(), DiskError> {
        if self.read_only {
            return Err(DiskError::ReadOnly);
        }
        self.post(T_OUT, sector, data, 0)?;
        self.in_flight = InFlight::Status { at: HEADER + data.len() };
        Ok(())
    }

    /// A flush, or nothing if the device has no write cache to flush.
    pub fn start_flush(&mut self) -> Result<(), DiskError> {
        if !self.flush {
            self.in_flight = InFlight::None;
            return Ok(());
        }
        self.post(T_FLUSH, 0, &[], 0)?;
        self.in_flight = InFlight::Status { at: HEADER };
        Ok(())
    }

    /// Whether the request on the device has finished (or there is none).
    pub fn done(&mut self) -> bool {
        if matches!(self.in_flight, InFlight::None) {
            return true;
        }
        let Some(q) = self.dev.queue(QUEUE) else { return true };
        q.pop_used().is_some()
    }

    fn status(&mut self, at: usize) -> Result<(), DiskError> {
        let q = self.dev.queue(QUEUE).ok_or(DiskError::Device)?;
        let mut status = [0xffu8];
        if !q.read_slot(0, at, &mut status) || status[0] != S_OK {
            return Err(DiskError::Device);
        }
        Ok(())
    }

    /// The result of a finished write or flush.
    pub fn finish(&mut self) -> Result<(), DiskError> {
        let flight = core::mem::replace(&mut self.in_flight, InFlight::None);
        match flight {
            InFlight::None => Ok(()),
            InFlight::Status { at } => self.status(at),
            InFlight::Read { .. } => Err(DiskError::Device),
        }
    }

    /// Copies a finished read's data into `out`.
    pub fn finish_read(&mut self, out: &mut [u8]) -> Result<(), DiskError> {
        let flight = core::mem::replace(&mut self.in_flight, InFlight::None);
        let InFlight::Read { len } = flight else { return Err(DiskError::Device) };
        self.status(HEADER + len)?;
        let q = self.dev.queue(QUEUE).ok_or(DiskError::Device)?;
        if out.len() != len || !q.read_slot(0, HEADER, out) {
            return Err(DiskError::Device);
        }
        Ok(())
    }
}
