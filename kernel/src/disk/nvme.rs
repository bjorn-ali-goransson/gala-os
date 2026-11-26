//! An NVMe disk (the NVM Express Base and NVM Command Set specifications;
//! see THIRD_PARTY.md): namespace 1 of the first NVMe controller.
//! Polled, one command at a time: the admin queue pair, and one I/O queue
//! pair without interrupts, each of a few entries; a 64 KiB buffer is the
//! data of every transfer, described by PRP1/PRP2 or a PRP list.
//! Commands: Identify, Set Features (number of queues), Create I/O
//! Completion/Submission Queue; Read, Write, Flush. Design:
//! docs/ARCHITECTURE.md, "Disk".
//!
//! The driver is generic over `hw::Platform` so that its logic runs on
//! the host against a model of a controller (`tests`).

use super::hw::{self, Mem, Platform, SECOND, Text};
use super::{DiskError, SECTOR};

// Controller registers (Base specification, "Controller Registers").
const CAP: usize = 0x00;
const VS: usize = 0x08;
const CC: usize = 0x14;
const CSTS: usize = 0x1c;
const AQA: usize = 0x24;
const ASQ: usize = 0x28;
const ACQ: usize = 0x30;
const DOORBELLS: usize = 0x1000;

const CC_EN: u32 = 1 << 0;
/// I/O submission and completion queue entries of 2^6 and 2^4 bytes.
const CC_QUEUE_ENTRIES: u32 = 6 << 16 | 4 << 20;
const CSTS_RDY: u32 = 1 << 0;
const CSTS_CFS: u32 = 1 << 1;
/// CAP.CSS bit 37: the NVM command set.
const CAP_CSS_NVM: u64 = 1 << 37;

/// The register window: the registers and the doorbells of two queue
/// pairs at the largest stride this driver accepts.
pub const REGS_LEN: usize = 0x2000;

const PAGE: usize = 4096;
const SQE: usize = 64;
const CQE: usize = 16;
/// Entries per queue (each queue's memory is one page).
const QUEUE_ENTRIES: u16 = 8;

// The control memory: a page each.
const ASQ_AT: usize = 0;
const ACQ_AT: usize = PAGE;
const IOSQ_AT: usize = 2 * PAGE;
const IOCQ_AT: usize = 3 * PAGE;
const IDENTIFY_AT: usize = 4 * PAGE;
const PRP_LIST_AT: usize = 5 * PAGE;
const CONTROL_LEN: usize = 6 * PAGE;

/// The most bytes one command moves (the data buffer), unless the
/// controller's MDTS says less.
pub const MAX_TRANSFER: usize = 64 * 1024;

const ADMIN_CREATE_SQ: u8 = 0x01;
const ADMIN_CREATE_CQ: u8 = 0x05;
const ADMIN_IDENTIFY: u8 = 0x06;
const ADMIN_SET_FEATURES: u8 = 0x09;
const FEATURE_QUEUES: u32 = 0x07;
const CNS_NAMESPACE: u32 = 0x00;
const CNS_CONTROLLER: u32 = 0x01;

const IO_FLUSH: u8 = 0x00;
const IO_WRITE: u8 = 0x01;
const IO_READ: u8 = 0x02;

const NSID: u32 = 1;

/// A command not finished by then is given up on and the controller set
/// up again.
pub const COMMAND_TIMEOUT: u64 = 30 * SECOND;

/// Why the controller or its namespace is not usable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// No NVM command set (CAP.CSS).
    NoNvmCommandSet,
    /// The smallest memory page it takes is larger than 4 KiB.
    PageSize {
        mpsmin: u32,
    },
    /// The doorbell stride puts the doorbells outside the mapped window.
    Stride {
        dstrd: u32,
    },
    /// CSTS.RDY did not follow CC.EN within CAP.TO.
    NotReady {
        enable: bool,
        csts: u32,
    },
    /// The controller reported a fatal status (CSTS.CFS).
    Fatal {
        csts: u32,
    },
    NoMemory,
    /// An admin command failed with this status.
    Command {
        opcode: u8,
        status: Status,
    },
    /// An admin command did not complete.
    Timeout {
        opcode: u8,
    },
    /// No namespace 1.
    NoNamespace,
    /// Namespace 1's format: logical blocks of this many bytes, with
    /// metadata of `metadata` bytes (only 512 and none are supported).
    Format {
        block: u64,
        metadata: u16,
    },
}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match *self {
            Error::NoNvmCommandSet => write!(f, "no NVM command set"),
            Error::PageSize { mpsmin } => {
                write!(f, "its smallest page is {} bytes (4096 is used)", 4096u64 << mpsmin)
            }
            Error::Stride { dstrd } => write!(f, "doorbell stride {dstrd} is not supported"),
            Error::NotReady { enable, csts } => write!(
                f,
                "it did not become {} within CAP.TO (CSTS {csts:#x})",
                if enable { "ready" } else { "disabled" }
            ),
            Error::Fatal { csts } => write!(f, "controller fatal status (CSTS {csts:#x})"),
            Error::NoMemory => write!(f, "no memory for its queues or buffer"),
            Error::Command { opcode, status } => {
                write!(f, "admin command {opcode:#04x} failed: {status}")
            }
            Error::Timeout { opcode } => write!(f, "admin command {opcode:#04x} did not complete"),
            Error::NoNamespace => write!(f, "no namespace 1"),
            Error::Format { block, metadata } => write!(
                f,
                "namespace 1 has {block}-byte blocks with {metadata} bytes of metadata (only 512 without is supported)"
            ),
        }
    }
}

/// A completion's status field (status code type, status code, do not
/// retry).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Status {
    pub sct: u8,
    pub sc: u8,
    pub dnr: bool,
}

impl Status {
    /// From completion dword 3.
    pub fn from_dw3(dw3: u32) -> Self {
        Self { sc: (dw3 >> 17) as u8, sct: (dw3 >> 25 & 7) as u8, dnr: dw3 & 1 << 31 != 0 }
    }

    pub fn ok(&self) -> bool {
        self.sct == 0 && self.sc == 0
    }

    /// What the specification calls it (the common ones).
    pub fn text(&self) -> &'static str {
        match (self.sct, self.sc) {
            (0, 0x00) => "successful completion",
            (0, 0x01) => "invalid command opcode",
            (0, 0x02) => "invalid field in command",
            (0, 0x03) => "command ID conflict",
            (0, 0x04) => "data transfer error",
            (0, 0x05) => "commands aborted due to power loss notification",
            (0, 0x06) => "internal error",
            (0, 0x07) => "command abort requested",
            (0, 0x08) => "command aborted due to SQ deletion",
            (0, 0x0b) => "invalid namespace or format",
            (0, 0x13) => "invalid PRP offset",
            (0, 0x80) => "LBA out of range",
            (0, 0x81) => "capacity exceeded",
            (0, 0x82) => "namespace not ready",
            (1, 0x00) => "completion queue invalid",
            (1, 0x01) => "invalid queue identifier",
            (1, 0x02) => "invalid queue size",
            (1, 0x80) => "conflicting attributes",
            (1, 0x82) => "attempted write to read only range",
            (2, 0x80) => "write fault",
            (2, 0x81) => "unrecovered read error",
            (2, 0x82) => "end-to-end guard check error",
            (2, 0x85) => "compare failure",
            (2, 0x86) => "access denied",
            (2, 0x87) => "deallocated or unwritten logical block",
            (0, _) => "generic command status",
            (1, _) => "command specific status",
            (2, _) => "media and data integrity error",
            (3, _) => "path related status",
            _ => "vendor specific status",
        }
    }
}

impl core::fmt::Display for Status {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "{} (type {}, code {:#04x}{})",
            self.text(),
            self.sct,
            self.sc,
            if self.dnr { ", do not retry" } else { "" }
        )
    }
}

/// PRP1, PRP2 and the PRP list for `len` bytes of a page-aligned,
/// contiguous buffer at `data`, with the list at `list` (Base
/// specification, "Physical Region Page Entry and List"): one page needs
/// PRP1 only, two pages PRP1 and PRP2, more pages PRP2 pointing at a list
/// of the second and later pages.
pub fn prps(data: u64, len: usize, list: u64) -> (u64, u64, [u64; MAX_TRANSFER / PAGE]) {
    let mut entries = [0u64; MAX_TRANSFER / PAGE];
    let pages = len.div_ceil(PAGE);
    let prp2 = match pages {
        0 | 1 => 0,
        2 => data + PAGE as u64,
        _ => {
            for (i, e) in entries.iter_mut().take(pages - 1).enumerate() {
                *e = data + ((i + 1) * PAGE) as u64;
            }
            list
        }
    };
    (data, prp2, entries)
}

/// What Identify says about the controller and namespace 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Identity {
    pub model: Text<40>,
    pub serial: Text<20>,
    pub firmware: Text<8>,
    /// Namespaces (NN).
    pub namespaces: u32,
    /// MDTS: the largest transfer is 2^mdts minimum pages (0: no limit).
    pub mdts: u8,
    /// A volatile write cache (VWC), which Flush empties.
    pub write_cache: bool,
    pub sectors: u64,
}

fn le32(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

fn le64(b: &[u8], at: usize) -> u64 {
    u64::from(le32(b, at)) | u64::from(le32(b, at + 4)) << 32
}

/// The Identify Controller data structure (bytes 4-23 serial number,
/// 24-63 model, 64-71 firmware, 77 MDTS, 516-519 NN, 525 VWC).
pub fn parse_controller(id: &[u8; PAGE]) -> Identity {
    Identity {
        serial: Text::new(&id[4..24]),
        model: Text::new(&id[24..64]),
        firmware: Text::new(&id[64..72]),
        mdts: id[77],
        namespaces: le32(id, 516),
        write_cache: id[525] & 1 != 0,
        sectors: 0,
    }
}

/// The Identify Namespace data structure: its size in logical blocks
/// (NSZE, bytes 0-7), which must be of 512 bytes without metadata (the
/// format FLBAS, byte 26, selects among the LBA formats at byte 128).
pub fn parse_namespace(id: &[u8; PAGE]) -> Result<u64, Error> {
    let sectors = le64(id, 0);
    if sectors == 0 {
        return Err(Error::NoNamespace);
    }
    let format = usize::from(id[26] & 0xf);
    let lbaf = le32(id, 128 + 4 * format);
    let metadata = lbaf as u16;
    let lbads = lbaf >> 16 & 0xff;
    let block = if lbads < 64 { 1u64 << lbads } else { 0 };
    if block != SECTOR as u64 || metadata != 0 {
        return Err(Error::Format { block, metadata });
    }
    Ok(sectors)
}

/// One submission and completion queue pair.
#[derive(Clone, Copy, Debug)]
struct Queue {
    id: u16,
    sq_at: usize,
    cq_at: usize,
    entries: u16,
    sq_tail: u16,
    cq_head: u16,
    /// The phase tag of new completions (starts 1, flips at each wrap).
    phase: bool,
}

impl Queue {
    fn new(id: u16, sq_at: usize, cq_at: usize, entries: u16) -> Self {
        Self { id, sq_at, cq_at, entries, sq_tail: 0, cq_head: 0, phase: true }
    }
}

/// What the command on the I/O queue is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum InFlight {
    None,
    Read { sector: u64, len: usize },
    Write { sector: u64 },
    Flush,
}

pub struct Nvme<P: Platform> {
    p: P,
    pub version: u32,
    pub identity: Identity,
    control: P::Mem,
    data: P::Mem,
    /// The doorbell stride: 4 << CAP.DSTRD bytes.
    stride: usize,
    /// CAP.TO: how long CSTS.RDY may take to follow CC.EN.
    ready_timeout: u64,
    entries: u16,
    admin: Queue,
    io: Queue,
    cid: u16,
    max_transfer: usize,
    in_flight: InFlight,
    /// The completion of the command in flight, once seen.
    completion: Option<Status>,
    started: u64,
}

impl<P: Platform> Nvme<P> {
    /// Takes the controller, resets and enables it, identifies it and
    /// namespace 1, and creates the I/O queue pair.
    pub fn start(mut p: P) -> Result<Self, Error> {
        let cap = p.read64(CAP);
        if cap & CAP_CSS_NVM == 0 {
            return Err(Error::NoNvmCommandSet);
        }
        let mpsmin = (cap >> 48 & 0xf) as u32;
        if mpsmin != 0 {
            return Err(Error::PageSize { mpsmin });
        }
        let dstrd = (cap >> 32 & 0xf) as u32;
        let stride = 4usize << dstrd;
        if DOORBELLS + 4 * stride > REGS_LEN {
            return Err(Error::Stride { dstrd });
        }
        // MQES is zero-based; at least 2 entries.
        let entries = QUEUE_ENTRIES.min((cap & 0xffff) as u16 + 1);
        let ready_timeout = (cap >> 24 & 0xff).max(1) * 500 * hw::MS;
        let control = p.alloc(CONTROL_LEN).ok_or(Error::NoMemory)?;
        let data = p.alloc(MAX_TRANSFER).ok_or(Error::NoMemory)?;
        let version = p.read32(VS);
        let mut dev = Self {
            p,
            version,
            identity: Identity {
                model: Text::new(b""),
                serial: Text::new(b""),
                firmware: Text::new(b""),
                namespaces: 0,
                mdts: 0,
                write_cache: false,
                sectors: 0,
            },
            control,
            data,
            stride,
            ready_timeout,
            entries,
            admin: Queue::new(0, ASQ_AT, ACQ_AT, entries),
            io: Queue::new(1, IOSQ_AT, IOCQ_AT, entries),
            cid: 0,
            max_transfer: MAX_TRANSFER,
            in_flight: InFlight::None,
            completion: None,
            started: 0,
        };
        let result = dev.bring_up(true);
        if result.is_err() {
            // It must not write the memory once it is freed.
            let _ = dev.disable();
        }
        result.map(|()| dev)
    }

    /// CC.EN = 0 and CSTS.RDY following (a controller reset, which also
    /// deletes the I/O queues).
    fn disable(&mut self) -> Result<(), Error> {
        let cc = self.p.read32(CC);
        if cc & CC_EN != 0 {
            self.p.write32(CC, cc & !CC_EN);
        }
        if !hw::wait_for(&self.p, self.ready_timeout, |p| p.read32(CSTS) & CSTS_RDY == 0) {
            return Err(Error::NotReady { enable: false, csts: self.p.read32(CSTS) });
        }
        Ok(())
    }

    /// From any state: disable, give it the admin queues, enable, and set
    /// up the I/O queue pair (and, the first time, identify).
    fn bring_up(&mut self, identify: bool) -> Result<(), Error> {
        self.disable()?;
        self.control.zero(0, IDENTIFY_AT);
        self.admin = Queue::new(0, ASQ_AT, ACQ_AT, self.entries);
        self.io = Queue::new(1, IOSQ_AT, IOCQ_AT, self.entries);
        self.in_flight = InFlight::None;
        self.completion = None;
        let size = u32::from(self.entries - 1);
        self.p.write32(AQA, size << 16 | size);
        self.p.write64(ASQ, self.control.phys() + ASQ_AT as u64);
        self.p.write64(ACQ, self.control.phys() + ACQ_AT as u64);
        // NVM command set, 4 KiB pages, round robin.
        self.p.write32(CC, CC_QUEUE_ENTRIES);
        self.p.write32(CC, CC_QUEUE_ENTRIES | CC_EN);
        let ready = hw::wait_for(&self.p, self.ready_timeout, |p| {
            p.read32(CSTS) & (CSTS_RDY | CSTS_CFS) != 0
        });
        let csts = self.p.read32(CSTS);
        if csts & CSTS_CFS != 0 {
            return Err(Error::Fatal { csts });
        }
        if !ready {
            return Err(Error::NotReady { enable: true, csts });
        }
        if identify {
            self.identify()?;
        }
        // One I/O queue pair (zero-based counts), then the pair itself.
        self.admin_command(ADMIN_SET_FEATURES, 0, 0, [FEATURE_QUEUES, 0, 0, 0, 0, 0])?;
        let size = u32::from(self.entries - 1) << 16;
        let cq = self.control.phys() + IOCQ_AT as u64;
        // Physically contiguous, no interrupts.
        self.admin_command(ADMIN_CREATE_CQ, 0, cq, [size | 1, 1, 0, 0, 0, 0])?;
        let sq = self.control.phys() + IOSQ_AT as u64;
        self.admin_command(ADMIN_CREATE_SQ, 0, sq, [size | 1, 1 << 16 | 1, 0, 0, 0, 0])?;
        Ok(())
    }

    fn identify(&mut self) -> Result<(), Error> {
        let id_page = self.control.phys() + IDENTIFY_AT as u64;
        let mut id = [0u8; PAGE];
        self.admin_command(ADMIN_IDENTIFY, 0, id_page, [CNS_CONTROLLER, 0, 0, 0, 0, 0])?;
        self.control.copy_out(IDENTIFY_AT, &mut id);
        let mut identity = parse_controller(&id);
        if identity.namespaces < NSID {
            return Err(Error::NoNamespace);
        }
        self.admin_command(ADMIN_IDENTIFY, NSID, id_page, [CNS_NAMESPACE, 0, 0, 0, 0, 0])?;
        self.control.copy_out(IDENTIFY_AT, &mut id);
        identity.sectors = parse_namespace(&id)?;
        self.identity = identity;
        // MDTS in minimum pages (4 KiB, see `start`).
        if identity.mdts != 0 && identity.mdts < 16 {
            self.max_transfer = MAX_TRANSFER.min(PAGE << identity.mdts);
        }
        Ok(())
    }

    fn sq_doorbell(&self, q: &Queue) -> usize {
        DOORBELLS + 2 * usize::from(q.id) * self.stride
    }

    fn cq_doorbell(&self, q: &Queue) -> usize {
        DOORBELLS + (2 * usize::from(q.id) + 1) * self.stride
    }

    /// Writes a command at the queue's tail and rings its doorbell; the
    /// command id.
    fn submit(&mut self, io: bool, opcode: u8, nsid: u32, prp: (u64, u64), cdw: [u32; 6]) -> u16 {
        self.cid = self.cid.wrapping_add(1);
        let cid = self.cid;
        let mut q = if io { self.io } else { self.admin };
        let at = q.sq_at + usize::from(q.sq_tail) * SQE;
        self.control.zero(at, SQE);
        self.control.write32(at, u32::from(opcode) | u32::from(cid) << 16);
        self.control.write32(at + 4, nsid);
        self.control.write32(at + 24, prp.0 as u32);
        self.control.write32(at + 28, (prp.0 >> 32) as u32);
        self.control.write32(at + 32, prp.1 as u32);
        self.control.write32(at + 36, (prp.1 >> 32) as u32);
        for (i, v) in cdw.iter().enumerate() {
            self.control.write32(at + 40 + 4 * i, *v);
        }
        q.sq_tail = (q.sq_tail + 1) % q.entries;
        let doorbell = self.sq_doorbell(&q);
        if io {
            self.io = q;
        } else {
            self.admin = q;
        }
        self.started = self.p.now_ns();
        self.p.write32(doorbell, u32::from(q.sq_tail));
        cid
    }

    /// The next completion on the queue, if the controller posted one
    /// (its phase tag is the current phase): consumed, its head doorbell
    /// rung. (command id, status)
    fn reap(&mut self, io: bool) -> Option<(u16, Status)> {
        let mut q = if io { self.io } else { self.admin };
        let at = q.cq_at + usize::from(q.cq_head) * CQE;
        let dw3 = self.control.read32(at + 12);
        if (dw3 >> 16 & 1 != 0) != q.phase {
            return None;
        }
        q.cq_head += 1;
        if q.cq_head == q.entries {
            q.cq_head = 0;
            q.phase = !q.phase;
        }
        let doorbell = self.cq_doorbell(&q);
        if io {
            self.io = q;
        } else {
            self.admin = q;
        }
        self.p.write32(doorbell, u32::from(q.cq_head));
        Some((dw3 as u16, Status::from_dw3(dw3)))
    }

    /// An admin command, waited for.
    fn admin_command(
        &mut self,
        opcode: u8,
        nsid: u32,
        prp1: u64,
        cdw: [u32; 6],
    ) -> Result<(), Error> {
        let cid = self.submit(false, opcode, nsid, (prp1, 0), cdw);
        let start = self.p.now_ns();
        loop {
            if let Some((id, status)) = self.reap(false) {
                if id != cid {
                    continue;
                }
                return if status.ok() { Ok(()) } else { Err(Error::Command { opcode, status }) };
            }
            let csts = self.p.read32(CSTS);
            if csts & CSTS_CFS != 0 {
                return Err(Error::Fatal { csts });
            }
            if self.p.now_ns().saturating_sub(start) > COMMAND_TIMEOUT {
                return Err(Error::Timeout { opcode });
            }
            core::hint::spin_loop();
        }
    }

    pub fn max_transfer(&self) -> usize {
        self.max_transfer
    }

    fn check_transfer(&self, sector: u64, len: usize) -> Result<u32, DiskError> {
        if len == 0 || len > self.max_transfer || !len.is_multiple_of(SECTOR) {
            return Err(DiskError::Unaligned);
        }
        if sector + (len / SECTOR) as u64 > self.identity.sectors {
            return Err(DiskError::OutOfRange);
        }
        Ok((len / SECTOR) as u32)
    }

    fn transfer(&mut self, opcode: u8, sector: u64, len: usize, blocks: u32) {
        let list = self.control.phys() + PRP_LIST_AT as u64;
        let (prp1, prp2, entries) = prps(self.data.phys(), len, list);
        if prp2 == list {
            for (i, e) in entries.iter().enumerate() {
                self.control.write32(PRP_LIST_AT + 8 * i, *e as u32);
                self.control.write32(PRP_LIST_AT + 8 * i + 4, (*e >> 32) as u32);
            }
        }
        let cdw = [sector as u32, (sector >> 32) as u32, blocks - 1, 0, 0, 0];
        self.completion = None;
        self.submit(true, opcode, NSID, (prp1, prp2), cdw);
    }

    pub fn start_read(&mut self, sector: u64, len: usize) -> Result<(), DiskError> {
        let blocks = self.check_transfer(sector, len)?;
        self.transfer(IO_READ, sector, len, blocks);
        self.in_flight = InFlight::Read { sector, len };
        Ok(())
    }

    pub fn start_write(&mut self, sector: u64, data: &[u8]) -> Result<(), DiskError> {
        let blocks = self.check_transfer(sector, data.len())?;
        self.data.copy_in(0, data);
        self.transfer(IO_WRITE, sector, data.len(), blocks);
        self.in_flight = InFlight::Write { sector };
        Ok(())
    }

    /// A Flush, or nothing if there is no volatile write cache.
    pub fn start_flush(&mut self) -> Result<(), DiskError> {
        if !self.identity.write_cache {
            self.in_flight = InFlight::None;
            return Ok(());
        }
        self.completion = None;
        self.submit(true, IO_FLUSH, NSID, (0, 0), [0; 6]);
        self.in_flight = InFlight::Flush;
        Ok(())
    }

    /// Whether the command completed (or there is none).
    pub fn done(&mut self) -> bool {
        if self.in_flight == InFlight::None || self.completion.is_some() {
            return true;
        }
        while let Some((cid, status)) = self.reap(true) {
            if cid == self.cid {
                self.completion = Some(status);
                return true;
            }
            log!(warn, "disk: NVMe: a completion for command {cid}, not {}", self.cid);
        }
        false
    }

    /// Whether the controller stopped for good (for a caller that cannot
    /// wait for `reset`: the panic path).
    pub fn failed(&self) -> bool {
        let csts = self.p.read32(CSTS);
        csts == !0 || csts & CSTS_CFS != 0 || csts & CSTS_RDY == 0
    }

    fn reset_reason(&self) -> Option<&'static str> {
        let csts = self.p.read32(CSTS);
        if csts == !0 {
            return Some("the controller does not answer");
        }
        if csts & CSTS_CFS != 0 {
            return Some("controller fatal status");
        }
        if csts & CSTS_RDY == 0 || self.p.read32(CC) & CC_EN == 0 {
            return Some("the controller was disabled (reset)");
        }
        if self.in_flight != InFlight::None
            && self.completion.is_none()
            && self.p.now_ns().saturating_sub(self.started) > COMMAND_TIMEOUT
        {
            return Some("a command did not complete in 30 s");
        }
        None
    }

    /// Whether the controller must be set up again (`reset`): it reported
    /// a fatal status, was disabled under the driver, or a command did not
    /// complete in COMMAND_TIMEOUT.
    pub fn needs_reset(&self) -> bool {
        let Some(why) = self.reset_reason() else { return false };
        log!(
            warn,
            "disk: NVMe: {why} (CSTS {:#x}, CC {:#x}, {:?})",
            self.p.read32(CSTS),
            self.p.read32(CC),
            self.in_flight
        );
        true
    }

    /// Sets the controller up again (ARCHITECTURE invariant 4): a
    /// controller reset, the admin queues, the I/O queue pair. The command
    /// in flight is forgotten (the caller submits it again).
    pub fn reset(&mut self) -> Result<(), Error> {
        self.bring_up(false)
    }

    /// For `diskreset=`: disables the controller under the driver.
    pub fn reset_now(&mut self) {
        let cc = self.p.read32(CC);
        self.p.write32(CC, cc & !CC_EN);
    }

    /// The result of a completed command; an error status is logged.
    pub fn finish(&mut self) -> Result<(), DiskError> {
        let flight = core::mem::replace(&mut self.in_flight, InFlight::None);
        if flight == InFlight::None {
            return Ok(());
        }
        match self.completion.take() {
            Some(status) if status.ok() => Ok(()),
            Some(status) => {
                log!(warn, "disk: NVMe: {flight:?} failed: {status}");
                Err(DiskError::Device)
            }
            None => Err(DiskError::Device),
        }
    }

    /// Copies a completed read's data into `out`.
    pub fn finish_read(&mut self, out: &mut [u8]) -> Result<(), DiskError> {
        let InFlight::Read { len, .. } = self.in_flight else { return Err(DiskError::Device) };
        self.finish()?;
        if out.len() != len {
            return Err(DiskError::Device);
        }
        self.data.copy_out(0, out);
        Ok(())
    }
}

#[cfg(not(test))]
mod kernel {
    use super::{Nvme, REGS_LEN};
    use crate::arch::mmio::Mmio;
    use crate::arch::pci::{self, Function};
    use crate::disk::hw::KernelPlatform;

    const CLASS_STORAGE: u8 = 0x01;
    const SUBCLASS_NVM: u8 = 0x08;
    const PROG_IF_NVME: u32 = 0x02;

    /// Finds the first NVMe controller and sets it up; one that is not
    /// usable says why in the log.
    pub fn probe() -> Option<(Nvme<KernelPlatform>, pci::Address)> {
        let mut found: Option<Function> = None;
        pci::for_each(|f| {
            let prog_if = f.config_u32(0x08) >> 8 & 0xff;
            if found.is_none()
                && f.class == CLASS_STORAGE
                && f.subclass == SUBCLASS_NVM
                && prog_if == PROG_IF_NVME
            {
                found = Some(*f);
            }
        });
        let f = found?;
        f.enable_memory_and_dma();
        let regs = match Mmio::bar(&f, 0, REGS_LEN) {
            Ok(regs) => regs,
            Err(e) => {
                warn!("disk: NVMe at {}: its registers could not be mapped ({e:?})", f.address);
                return None;
            }
        };
        match Nvme::start(KernelPlatform::new(regs)) {
            Ok(dev) => Some((dev, f.address)),
            Err(e) => {
                warn!("disk: NVMe at {}: not usable: {e}", f.address);
                None
            }
        }
    }
}

#[cfg(not(test))]
pub use kernel::probe;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::disk::hw::fake::{Clock, FakeMem, Phys, Shared};
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::vec::Vec;

    #[test]
    fn prps_by_length() {
        let (p1, p2, _) = prps(0x10000, 512, 0x9000);
        assert_eq!((p1, p2), (0x10000, 0));
        let (_, p2, _) = prps(0x10000, 4096, 0x9000);
        assert_eq!(p2, 0);
        let (_, p2, _) = prps(0x10000, 8192, 0x9000);
        assert_eq!(p2, 0x11000);
        let (_, p2, list) = prps(0x10000, 3 * 4096 + 512, 0x9000);
        assert_eq!(p2, 0x9000);
        assert_eq!(list[..4], [0x11000, 0x12000, 0x13000, 0]);
        let (_, _, list) = prps(0x10000, MAX_TRANSFER, 0x9000);
        assert_eq!(list[14], 0x1f000);
        assert_eq!(list[15], 0);
    }

    #[test]
    fn status_is_decoded() {
        // Phase 1, SC 0x80, SCT 0: LBA out of range.
        let s = Status::from_dw3(1 << 16 | 0x80 << 17);
        assert_eq!((s.sct, s.sc, s.dnr), (0, 0x80, false));
        assert_eq!(s.text(), "LBA out of range");
        let s = Status::from_dw3(2 << 25 | 0x81 << 17 | 1 << 31);
        assert_eq!(s.text(), "unrecovered read error");
        assert!(s.dnr && !s.ok());
        assert!(Status::from_dw3(1 << 16 | 0x1234).ok());
        assert_eq!(Status::from_dw3(7 << 25 | 1 << 17).text(), "vendor specific status");
    }

    fn namespace(sectors: u64, lbads: u32, metadata: u16) -> [u8; PAGE] {
        let mut id = [0u8; PAGE];
        id[..8].copy_from_slice(&sectors.to_le_bytes());
        id[25] = 1;
        id[26] = 1;
        id[128..132].copy_from_slice(&(12u32 << 16).to_le_bytes());
        id[132..136].copy_from_slice(&(lbads << 16 | u32::from(metadata)).to_le_bytes());
        id
    }

    fn controller() -> [u8; PAGE] {
        let mut id = [0u8; PAGE];
        id[4..24].copy_from_slice(b"gala0               ");
        id[24..64].copy_from_slice(b"QEMU NVMe Ctrl                          ");
        id[64..72].copy_from_slice(b"8.2.2   ");
        id[77] = 7;
        id[516..520].copy_from_slice(&1u32.to_le_bytes());
        id[525] = 1;
        id
    }

    #[test]
    fn identify_is_parsed() {
        let c = parse_controller(&controller());
        assert_eq!(c.model.as_str(), "QEMU NVMe Ctrl");
        assert_eq!(c.serial.as_str(), "gala0");
        assert_eq!(c.firmware.as_str(), "8.2.2");
        assert_eq!((c.mdts, c.namespaces, c.write_cache), (7, 1, true));
        assert_eq!(parse_namespace(&namespace(32768, 9, 0)), Ok(32768));
        assert_eq!(
            parse_namespace(&namespace(32768, 12, 0)),
            Err(Error::Format { block: 4096, metadata: 0 })
        );
        assert_eq!(
            parse_namespace(&namespace(32768, 9, 8)),
            Err(Error::Format { block: 512, metadata: 8 })
        );
        assert_eq!(parse_namespace(&namespace(0, 9, 0)), Err(Error::NoNamespace));
    }

    /// A queue pair as the controller sees it: (sq base, cq base, entries,
    /// sq head, cq tail, phase).
    type QueueState = (u64, u64, u16, u16, u16, bool);

    /// A model of an NVMe controller with namespace 1, executing the
    /// commands up to a submission queue's tail when its doorbell rings.
    struct Ctrl {
        cap: u64,
        cc: u32,
        csts: u32,
        aqa: u32,
        asq: u64,
        acq: u64,
        /// Per queue id.
        queues: [Option<QueueState>; 2],
        disk: Vec<u8>,
        controller: [u8; PAGE],
        namespace: [u8; PAGE],
        /// The next I/O command completes with this status (SCT << 8 | SC).
        fail_next: Option<u16>,
        hang: bool,
        commands: Vec<(u8, u64, u32)>,
        doorbells: Vec<(usize, u32)>,
    }

    struct FakeNvme {
        c: Rc<RefCell<Ctrl>>,
        phys: Shared,
        clock: Clock,
    }

    fn nvme(sectors: u64) -> (FakeNvme, Rc<RefCell<Ctrl>>) {
        let state = Rc::new(RefCell::new(Ctrl {
            // MQES 63, CSS NVM, TO 1 (500 ms), DSTRD 0, MPSMIN 0.
            cap: 63 | 1 << 24 | CAP_CSS_NVM,
            cc: 0,
            csts: 0,
            aqa: 0,
            asq: 0,
            acq: 0,
            queues: [None; 2],
            disk: std::vec![0; sectors as usize * SECTOR],
            controller: controller(),
            namespace: namespace(sectors, 9, 0),
            fail_next: None,
            hang: false,
            commands: Vec::new(),
            doorbells: Vec::new(),
        }));
        let fake = FakeNvme { c: state.clone(), phys: Phys::shared(), clock: Clock::default() };
        (fake, state)
    }

    impl Ctrl {
        /// Gathers the data pages of a transfer from its PRPs.
        fn pages(m: &mut Phys, prp1: u64, prp2: u64, len: usize) -> Vec<u64> {
            let n = len.div_ceil(PAGE);
            let mut pages = std::vec![prp1];
            if n == 2 {
                pages.push(prp2);
            } else if n > 2 {
                for i in 0..n - 1 {
                    pages.push(m.read64(prp2 + 8 * i as u64));
                }
            }
            pages
        }

        fn run(&mut self, qid: usize, tail: u16, phys: &Shared) {
            let mut m = phys.borrow_mut();
            let Some((sq, cq, entries, mut head, mut cq_tail, mut phase)) = self.queues[qid] else {
                panic!("doorbell of queue {qid}, which does not exist");
            };
            while head != tail {
                let at = sq + u64::from(head) * SQE as u64;
                head = (head + 1) % entries;
                let dw0 = m.read32(at);
                let (opcode, cid) = (dw0 as u8, (dw0 >> 16) as u16);
                let prp1 = m.read64(at + 24);
                let prp2 = m.read64(at + 32);
                let cdw: Vec<u32> = (0..6).map(|i| m.read32(at + 40 + 4 * i)).collect();
                let mut status = 0u16;
                if qid == 0 {
                    self.commands.push((opcode, prp1, cdw[0]));
                    match opcode {
                        ADMIN_IDENTIFY if cdw[0] == 1 => m.write(prp1, &self.controller),
                        ADMIN_IDENTIFY => m.write(prp1, &self.namespace),
                        ADMIN_SET_FEATURES => assert_eq!(cdw[0], FEATURE_QUEUES),
                        ADMIN_CREATE_CQ => {
                            assert_eq!((cdw[0] & 0xffff, cdw[1] & 3), (1, 1));
                            let n = (cdw[0] >> 16) as u16 + 1;
                            self.queues[1] = Some((0, prp1, n, 0, 0, true));
                        }
                        ADMIN_CREATE_SQ => {
                            assert_eq!(cdw[1], 1 << 16 | 1);
                            let q = self.queues[1].as_mut().expect("SQ before its CQ");
                            q.0 = prp1;
                        }
                        _ => status = 1,
                    }
                } else {
                    let lba = u64::from(cdw[0]) | u64::from(cdw[1]) << 32;
                    let blocks = (cdw[2] & 0xffff) + 1;
                    self.commands.push((opcode, lba, blocks));
                    if self.hang {
                        continue;
                    }
                    let at = lba as usize * SECTOR;
                    let len = blocks as usize * SECTOR;
                    if let Some(s) = self.fail_next.take() {
                        status = s;
                    } else {
                        match opcode {
                            IO_READ | IO_WRITE => {
                                let pages = Self::pages(&mut m, prp1, prp2, len);
                                for (i, page) in pages.iter().enumerate() {
                                    let part = (len - i * PAGE).min(PAGE);
                                    let bytes = &mut self.disk[at + i * PAGE..][..part];
                                    if opcode == IO_READ {
                                        m.write(*page, bytes);
                                    } else {
                                        m.read(*page, bytes);
                                    }
                                }
                            }
                            IO_FLUSH => {}
                            _ => status = 1,
                        }
                    }
                }
                let entry = cq + u64::from(cq_tail) * CQE as u64;
                m.write32(entry + 8, u32::from(head) | (qid as u32) << 16);
                m.write32(
                    entry + 12,
                    u32::from(cid) | u32::from(phase) << 16 | u32::from(status) << 17,
                );
                cq_tail += 1;
                if cq_tail == entries {
                    cq_tail = 0;
                    phase = !phase;
                }
            }
            if qid == 1 {
                self.queues[1] = Some((sq, cq, entries, head, cq_tail, phase));
            } else {
                self.queues[0] = Some((sq, cq, entries, head, cq_tail, phase));
            }
        }
    }

    impl Platform for FakeNvme {
        type Mem = FakeMem;
        fn read32(&self, offset: usize) -> u32 {
            let c = self.c.borrow();
            match offset {
                0x00 => c.cap as u32,
                0x04 => (c.cap >> 32) as u32,
                VS => 0x0001_0400,
                CC => c.cc,
                CSTS => c.csts,
                _ => 0,
            }
        }

        fn write32(&mut self, offset: usize, v: u32) {
            let mut c = self.c.borrow_mut();
            c.doorbells.push((offset, v));
            match offset {
                CC => {
                    if v & CC_EN != 0 && c.cc & CC_EN == 0 {
                        assert_eq!(v & 0x00ff_0000, CC_QUEUE_ENTRIES);
                        let n = (c.aqa & 0xfff) as u16 + 1;
                        let (asq, acq) = (c.asq, c.acq);
                        c.queues = [Some((asq, acq, n, 0, 0, true)), None];
                        c.csts = CSTS_RDY;
                    } else if v & CC_EN == 0 {
                        c.queues = [None, None];
                        c.csts = 0;
                    }
                    c.cc = v;
                }
                AQA => c.aqa = v,
                ASQ => c.asq = c.asq & !0xffff_ffff | u64::from(v),
                0x2c => c.asq = c.asq & 0xffff_ffff | u64::from(v) << 32,
                ACQ => c.acq = c.acq & !0xffff_ffff | u64::from(v),
                0x34 => c.acq = c.acq & 0xffff_ffff | u64::from(v) << 32,
                0x1000 => c.run(0, v as u16, &self.phys),
                0x1008 => c.run(1, v as u16, &self.phys),
                0x1004 | 0x100c => {}
                other => panic!("write to register {other:#x}"),
            }
        }

        fn alloc(&mut self, bytes: usize) -> Option<FakeMem> {
            let at = self.phys.borrow_mut().alloc(bytes);
            Some(FakeMem { phys: self.phys.clone(), at, len: bytes })
        }

        fn now_ns(&self) -> u64 {
            self.clock.now()
        }
    }

    fn run(dev: &mut Nvme<FakeNvme>) -> bool {
        (0..10).any(|_| dev.done())
    }

    #[test]
    fn brings_the_controller_up() {
        let (fake, state) = nvme(64);
        let dev = Nvme::start(fake).unwrap();
        assert_eq!(dev.identity.sectors, 64);
        assert_eq!(dev.identity.model.as_str(), "QEMU NVMe Ctrl");
        // MDTS 7: 512 KiB, more than the buffer.
        assert_eq!(dev.max_transfer(), MAX_TRANSFER);
        let opcodes: Vec<u8> = state.borrow().commands.iter().map(|c| c.0).collect();
        assert_eq!(opcodes, [ADMIN_IDENTIFY, ADMIN_IDENTIFY, ADMIN_SET_FEATURES, 0x05, 0x01]);
        assert_eq!(state.borrow().aqa, 7 << 16 | 7);
    }

    #[test]
    fn refuses_formats_it_cannot_use() {
        let (fake, state) = nvme(64);
        state.borrow_mut().namespace = namespace(64, 12, 0);
        assert_eq!(Nvme::start(fake).err(), Some(Error::Format { block: 4096, metadata: 0 }));
        assert_eq!(state.borrow().cc & CC_EN, 0, "left enabled after a refusal");
        let (fake, state) = nvme(64);
        state.borrow_mut().cap |= 1 << 48;
        assert_eq!(Nvme::start(fake).err(), Some(Error::PageSize { mpsmin: 1 }));
    }

    #[test]
    fn a_small_mdts_limits_the_transfer() {
        let (fake, state) = nvme(64);
        state.borrow_mut().controller[77] = 2;
        assert_eq!(Nvme::start(fake).unwrap().max_transfer(), 16 * 1024);
    }

    #[test]
    fn reads_and_writes_through_every_prp_shape() {
        let (fake, state) = nvme(512);
        let mut dev = Nvme::start(fake).unwrap();
        // More commands than queue entries: the queues wrap and the phase flips.
        for (i, len) in [512, 4096, 8192, 12288, MAX_TRANSFER, 512, 1024, 4608, 65024, 512]
            .into_iter()
            .enumerate()
        {
            let data: Vec<u8> = (0..len).map(|b| (b + i * 13) as u8).collect();
            let sector = 3 + i as u64 * 7;
            dev.start_write(sector, &data).unwrap();
            assert!(run(&mut dev));
            dev.finish().unwrap();
            let mut out = std::vec![0u8; len];
            dev.start_read(sector, len).unwrap();
            assert!(run(&mut dev));
            dev.finish_read(&mut out).unwrap();
            assert_eq!(out, data, "{len} bytes");
        }
        dev.start_flush().unwrap();
        assert!(run(&mut dev));
        dev.finish().unwrap();
        assert_eq!(state.borrow().commands.last(), Some(&(IO_FLUSH, 0, 1)));
        assert_eq!(dev.start_read(511, 1024), Err(DiskError::OutOfRange));
    }

    #[test]
    fn an_error_status_fails_the_command_only() {
        let (fake, state) = nvme(64);
        let mut dev = Nvme::start(fake).unwrap();
        state.borrow_mut().fail_next = Some(2 << 8 | 0x81);
        dev.start_read(0, 512).unwrap();
        assert!(run(&mut dev));
        let mut out = [0u8; 512];
        assert_eq!(dev.finish_read(&mut out), Err(DiskError::Device));
        assert!(!dev.needs_reset());
        dev.start_read(0, 512).unwrap();
        assert!(run(&mut dev));
        dev.finish_read(&mut out).unwrap();
    }

    #[test]
    fn a_disabled_controller_is_noticed_and_brought_up_again() {
        let (fake, state) = nvme(64);
        let mut dev = Nvme::start(fake).unwrap();
        state.borrow_mut().hang = true;
        dev.start_write(5, &[7; 1024]).unwrap();
        dev.reset_now();
        assert!(dev.needs_reset());
        state.borrow_mut().hang = false;
        dev.reset().unwrap();
        assert!(!dev.needs_reset());
        dev.start_write(5, &[7; 1024]).unwrap();
        assert!(run(&mut dev));
        dev.finish().unwrap();
        assert_eq!(state.borrow().disk[5 * SECTOR + 1023], 7);
        // Identify is not repeated.
        let identifies = state.borrow().commands.iter().filter(|c| c.0 == ADMIN_IDENTIFY).count();
        assert_eq!(identifies, 2);
    }

    #[test]
    fn a_command_that_never_completes_times_out() {
        let (fake, state) = nvme(64);
        let mut dev = Nvme::start(fake).unwrap();
        state.borrow_mut().hang = true;
        dev.start_read(0, 512).unwrap();
        let mut polls = 0;
        while !dev.needs_reset() {
            assert!(!dev.done());
            polls += 1;
            assert!(polls < 1_000_000);
        }
        state.borrow_mut().hang = false;
        dev.reset().unwrap();
        dev.start_read(0, 512).unwrap();
        assert!(run(&mut dev));
    }

    #[test]
    fn a_fatal_status_needs_a_reset() {
        let (fake, state) = nvme(64);
        let dev = Nvme::start(fake).unwrap();
        state.borrow_mut().csts |= CSTS_CFS;
        assert!(dev.failed());
        assert!(dev.needs_reset());
    }
}
