//! A SATA disk on an AHCI host bus adapter (the Serial ATA AHCI 1.3.1
//! specification, Intel, and the ATA command set ACS; see THIRD_PARTY.md).
//! Polled, one command at a time in command slot 0 of one port: the
//! command list, the received-FIS area and the one command table share a
//! page, and a 64 KiB buffer is the data of every transfer (one PRDT
//! entry). Commands: IDENTIFY DEVICE, READ/WRITE DMA EXT (48-bit LBA),
//! FLUSH CACHE EXT. Design: docs/ARCHITECTURE.md, "Disk".
//!
//! The driver is generic over `hw::Platform` so that its logic runs on
//! the host against a model of an HBA (`tests`).

use super::hw::{self, MS, Mem, Platform, SECOND, Text};
use super::{DiskError, SECTOR};

// Generic host control (section 3.1).
const CAP: usize = 0x00;
const GHC: usize = 0x04;
const PI: usize = 0x0c;
const VS: usize = 0x10;
const CAP2: usize = 0x24;
const BOHC: usize = 0x28;

const CAP_S64A: u32 = 1 << 31;
const GHC_AE: u32 = 1 << 31;
const GHC_HR: u32 = 1 << 0;
const CAP2_BOH: u32 = 1 << 0;
const BOHC_BOS: u32 = 1 << 0;
const BOHC_OOS: u32 = 1 << 1;
const BOHC_BB: u32 = 1 << 4;

// Port registers (section 3.3), at 0x100 + 0x80 * port.
const P_CLB: usize = 0x00;
const P_CLBU: usize = 0x04;
const P_FB: usize = 0x08;
const P_FBU: usize = 0x0c;
const P_IS: usize = 0x10;
const P_IE: usize = 0x14;
const P_CMD: usize = 0x18;
const P_TFD: usize = 0x20;
const P_SIG: usize = 0x24;
const P_SSTS: usize = 0x28;
const P_SCTL: usize = 0x2c;
const P_SERR: usize = 0x30;
const P_CI: usize = 0x38;

const CMD_ST: u32 = 1 << 0;
const CMD_SUD: u32 = 1 << 1;
const CMD_POD: u32 = 1 << 2;
const CMD_FRE: u32 = 1 << 4;
const CMD_FR: u32 = 1 << 14;
const CMD_CR: u32 = 1 << 15;

const IS_TFES: u32 = 1 << 30;
/// Host bus fatal, host bus data and interface fatal errors: the port
/// stopped and must be set up again.
const IS_FATAL: u32 = 1 << 29 | 1 << 28 | 1 << 27;

const TFD_ERR: u32 = 1 << 0;
const TFD_DRQ: u32 = 1 << 3;
const TFD_BSY: u32 = 1 << 7;

const DET_MASK: u32 = 0xf;
/// PxSSTS.DET: a device is present and communication is established.
const DET_PRESENT: u32 = 3;

/// PxSIG of an ATA disk (an ATAPI device, a CD, is 0xeb14_0101).
const SIG_ATA: u32 = 0x0000_0101;
/// PxSIG before the device's first register FIS.
const SIG_NONE: u32 = 0xffff_ffff;

/// The register window: the generic registers and 32 ports.
pub const REGS_LEN: usize = 0x100 + 32 * 0x80;

fn port_reg(port: usize, reg: usize) -> usize {
    0x100 + 0x80 * port + reg
}

// The control page: the command list (32 headers of 32 bytes, 1 KiB
// aligned), the received FIS (256 bytes, 256-byte aligned), and the one
// command table (128-byte aligned: the command FIS, then the PRDT at 0x80).
const CONTROL_LEN: usize = 4096;
const FIS_AT: usize = 0x400;
const TABLE_AT: usize = 0x800;
const PRDT_AT: usize = TABLE_AT + 0x80;

/// The most bytes one command moves (the data buffer).
pub const MAX_TRANSFER: usize = 64 * 1024;

const ATA_IDENTIFY: u8 = 0xec;
const ATA_READ_DMA_EXT: u8 = 0x25;
const ATA_WRITE_DMA_EXT: u8 = 0x35;
const ATA_FLUSH_CACHE_EXT: u8 = 0xea;

/// A command not finished by then is given up on and the port set up
/// again (ATA's longest command timeouts are of this order).
pub const COMMAND_TIMEOUT: u64 = 30 * SECOND;
/// PxCMD.CR and FR clear within 500 ms of ST and FRE (section 10.1.2).
const ENGINE_STOP: u64 = 500 * MS;
/// GHC.HR clears within 1 s (section 10.4.3).
const HBA_RESET: u64 = SECOND;
/// A device may take this long to spin up after a COMRESET.
const SPIN_UP: u64 = 31 * SECOND;

/// Why the HBA or its disk is not usable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// No implemented port has an ATA disk.
    NoDisk,
    /// The firmware kept the HBA (BIOS/OS handoff, section 10.6).
    Handoff,
    /// GHC.HR did not clear.
    HbaReset,
    /// A port's command engine did not stop.
    EngineStuck { cmd: u32 },
    /// No link after a COMRESET.
    NoLink { ssts: u32 },
    /// The device stayed busy after a COMRESET.
    Busy { tfd: u32 },
    /// No memory for the command list or the data buffer.
    NoMemory,
    /// The memory is above 4 GiB and the HBA addresses 32 bits (CAP.S64A).
    Above4G { phys: u64 },
    /// A command at setup (IDENTIFY) failed.
    Command { tfd: u32, is: u32 },
    /// A command at setup did not finish.
    Timeout { tfd: u32, is: u32, ci: u32 },
    /// The disk has no 48-bit addressing.
    NoLba48,
    /// Logical sectors of this many bytes (only 512 is supported).
    SectorSize(u32),
}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match *self {
            Error::NoDisk => write!(f, "no ATA disk on its ports"),
            Error::Handoff => write!(f, "the firmware did not hand the HBA over"),
            Error::HbaReset => write!(f, "the HBA reset did not complete in 1 s"),
            Error::EngineStuck { cmd } => {
                write!(f, "the command engine did not stop (PxCMD {cmd:#010x})")
            }
            Error::NoLink { ssts } => write!(f, "no link after a COMRESET (PxSSTS {ssts:#x})"),
            Error::Busy { tfd } => write!(f, "the device stayed busy (PxTFD {tfd:#x})"),
            Error::NoMemory => write!(f, "no memory for the command list or buffer"),
            Error::Above4G { phys } => write!(
                f,
                "its memory at {phys:#x} is above 4 GiB and the HBA has no 64-bit addressing"
            ),
            Error::Command { tfd, is } => {
                write!(f, "IDENTIFY DEVICE failed (PxTFD {tfd:#x}, PxIS {is:#010x})")
            }
            Error::Timeout { tfd, is, ci } => write!(
                f,
                "IDENTIFY DEVICE did not finish (PxTFD {tfd:#x}, PxIS {is:#010x}, PxCI {ci:#x})"
            ),
            Error::NoLba48 => write!(f, "the disk has no 48-bit addressing"),
            Error::SectorSize(n) => {
                write!(f, "logical sectors of {n} bytes (only 512 is supported)")
            }
        }
    }
}

/// What IDENTIFY DEVICE says about the disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Identity {
    pub sectors: u64,
    pub model: Text<40>,
    pub serial: Text<20>,
    pub firmware: Text<8>,
    /// FLUSH CACHE EXT is supported.
    pub flush: bool,
}

fn word(id: &[u8; 512], w: usize) -> u16 {
    u16::from_le_bytes([id[2 * w], id[2 * w + 1]])
}

/// An ATA string: two characters a word, the first in the high byte.
fn ata_string<const N: usize>(id: &[u8; 512], first: usize) -> Text<N> {
    let mut raw = [0u8; N];
    for (i, pair) in raw.chunks_mut(2).enumerate() {
        let w = word(id, first + i).to_be_bytes();
        pair.copy_from_slice(&w[..pair.len()]);
    }
    Text::new(&raw)
}

/// Reads the IDENTIFY DEVICE data (ACS: words 10-19 serial number, 23-26
/// firmware, 27-46 model, 83 the command sets supported, 100-103 the
/// 48-bit sector count, 106 and 117-118 the logical sector size).
pub fn parse_identify(id: &[u8; 512]) -> Result<Identity, Error> {
    let valid = |w: u16| w & 0xc000 == 0x4000;
    let w83 = word(id, 83);
    if !valid(w83) || w83 & 1 << 10 == 0 {
        return Err(Error::NoLba48);
    }
    let sectors = (0..4).fold(0u64, |acc, i| acc | u64::from(word(id, 100 + i)) << (16 * i));
    if sectors == 0 {
        return Err(Error::NoLba48);
    }
    let w106 = word(id, 106);
    let logical = if valid(w106) && w106 & 1 << 12 != 0 {
        // In 16-bit words.
        (u32::from(word(id, 117)) | u32::from(word(id, 118)) << 16).saturating_mul(2)
    } else {
        SECTOR as u32
    };
    if logical != SECTOR as u32 {
        return Err(Error::SectorSize(logical));
    }
    Ok(Identity {
        sectors,
        model: ata_string(id, 27),
        serial: ata_string(id, 10),
        firmware: ata_string(id, 23),
        flush: w83 & 1 << 13 != 0,
    })
}

/// A Register host-to-device FIS (SATA; AHCI section 4.2.3's command
/// FIS) carrying `command` with a 48-bit LBA and sector count.
pub fn h2d_fis(command: u8, lba: u64, count: u16) -> [u8; 20] {
    let l = lba.to_le_bytes();
    let c = count.to_le_bytes();
    // Device: bit 6 selects LBA addressing (the transfer commands).
    let device = if command == ATA_IDENTIFY { 0 } else { 0x40 };
    [
        0x27, 0x80, command, 0, l[0], l[1], l[2], device, l[3], l[4], l[5], 0, c[0], c[1], 0, 0, 0,
        0, 0, 0,
    ]
}

/// What the command in slot 0 is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum InFlight {
    None,
    Read { sector: u64, len: usize },
    Write { sector: u64 },
    Flush,
}

pub struct Ahci<P: Platform> {
    p: P,
    pub port: usize,
    /// The AHCI version (VS).
    pub version: u32,
    pub identity: Identity,
    control: P::Mem,
    data: P::Mem,
    in_flight: InFlight,
    started: u64,
    /// Error recovery failed: the port is to be set up again.
    broken: bool,
}

/// Turns on AHCI mode (GHC.AE; section 10.1.2 step 1).
fn enable_ahci<P: Platform>(p: &mut P) {
    let ghc = p.read32(GHC);
    if ghc & GHC_AE == 0 {
        p.write32(GHC, ghc | GHC_AE);
    }
}

/// Takes the HBA from the firmware (section 10.6.3): asks for it (OOS) and
/// waits for the firmware to let go (BOS), longer while it says it is busy
/// (BB).
fn handoff<P: Platform>(p: &mut P) -> Result<(), Error> {
    let bohc = p.read32(BOHC);
    if bohc & BOHC_BOS == 0 && bohc & BOHC_OOS != 0 {
        return Ok(());
    }
    p.write32(BOHC, bohc | BOHC_OOS);
    if hw::wait_for(p, 25 * MS, |p| p.read32(BOHC) & BOHC_BOS == 0) {
        return Ok(());
    }
    if p.read32(BOHC) & BOHC_BB == 0 {
        // Not busy within 25 ms: the firmware is not going to answer, and
        // the OS may take the HBA.
        log!(warn, "disk: AHCI: the firmware did not answer the handoff; taking the HBA");
        return Ok(());
    }
    if hw::wait_for(p, 2 * SECOND, |p| p.read32(BOHC) & BOHC_BOS == 0) {
        Ok(())
    } else {
        Err(Error::Handoff)
    }
}

/// Memory for the HBA, below 4 GiB unless it has 64-bit addressing.
fn alloc<P: Platform>(p: &mut P, bytes: usize, s64a: bool) -> Result<P::Mem, Error> {
    let mem = p.alloc(bytes).ok_or(Error::NoMemory)?;
    let end = mem.phys() + mem.len() as u64;
    if !s64a && end > 1 << 32 {
        return Err(Error::Above4G { phys: mem.phys() });
    }
    Ok(mem)
}

impl<P: Platform> Ahci<P> {
    /// Takes the HBA and sets up its first ATA disk: AHCI mode, the
    /// handoff, then each implemented port with a device present, in
    /// order, until one answers IDENTIFY DEVICE as a disk this driver can
    /// use. Ports with another device (a CD) are left alone.
    pub fn start(mut p: P) -> Result<Self, Error> {
        enable_ahci(&mut p);
        if p.read32(CAP2) & CAP2_BOH != 0 {
            handoff(&mut p)?;
        }
        let s64a = p.read32(CAP) & CAP_S64A != 0;
        let control = alloc(&mut p, CONTROL_LEN, s64a)?;
        let data = alloc(&mut p, MAX_TRANSFER, s64a)?;
        let pi = p.read32(PI);
        let version = p.read32(VS);
        let mut dev = Self {
            p,
            port: 0,
            version,
            identity: Identity {
                sectors: 0,
                model: Text::new(b""),
                serial: Text::new(b""),
                firmware: Text::new(b""),
                flush: false,
            },
            control,
            data,
            in_flight: InFlight::None,
            started: 0,
            broken: false,
        };
        let mut result = Err(Error::NoDisk);
        for port in (0..32).filter(|&port| pi & 1 << port != 0) {
            dev.port = port;
            if dev.preg(P_SSTS) & DET_MASK != DET_PRESENT {
                continue;
            }
            let sig = dev.preg(P_SIG);
            if sig != SIG_ATA && sig != SIG_NONE {
                continue;
            }
            let tried = dev.setup_port(false).and_then(|()| {
                if dev.preg(P_SIG) != SIG_ATA {
                    return Err(Error::NoDisk);
                }
                dev.identify()
            });
            match tried {
                Ok(identity) => {
                    dev.identity = identity;
                    return Ok(dev);
                }
                Err(e) => {
                    if e != Error::NoDisk {
                        log!(warn, "disk: AHCI port {port}: {e}");
                    }
                    result = Err(e);
                    // It must not write the memory once it is freed.
                    if dev.stop_engine().is_err() {
                        let _ = dev.hba_reset();
                    }
                }
            }
        }
        result
    }

    fn preg(&self, reg: usize) -> u32 {
        self.p.read32(port_reg(self.port, reg))
    }

    fn pwrite(&mut self, reg: usize, value: u32) {
        self.p.write32(port_reg(self.port, reg), value);
    }

    fn wait_port(&self, timeout: u64, reg: usize, cond: impl Fn(u32) -> bool) -> bool {
        let at = port_reg(self.port, reg);
        hw::wait_for(&self.p, timeout, |p| cond(p.read32(at)))
    }

    /// Resets the whole HBA (GHC.HR, section 10.4.3) and turns AHCI mode
    /// back on: every port's engine stops.
    fn hba_reset(&mut self) -> Result<(), Error> {
        let ghc = self.p.read32(GHC);
        self.p.write32(GHC, ghc | GHC_HR);
        if !hw::wait_for(&self.p, HBA_RESET, |p| p.read32(GHC) & GHC_HR == 0) {
            return Err(Error::HbaReset);
        }
        enable_ahci(&mut self.p);
        Ok(())
    }

    /// Stops the port's command engine, then its FIS receive (section
    /// 10.1.2: ST then CR, FRE then FR, 500 ms each).
    fn stop_engine(&mut self) -> Result<(), Error> {
        let cmd = self.preg(P_CMD);
        if cmd & (CMD_ST | CMD_CR) != 0 {
            self.pwrite(P_CMD, cmd & !CMD_ST);
            if !self.wait_port(ENGINE_STOP, P_CMD, |c| c & CMD_CR == 0) {
                return Err(Error::EngineStuck { cmd: self.preg(P_CMD) });
            }
        }
        let cmd = self.preg(P_CMD);
        if cmd & (CMD_FRE | CMD_FR) != 0 {
            self.pwrite(P_CMD, cmd & !CMD_FRE);
            if !self.wait_port(ENGINE_STOP, P_CMD, |c| c & CMD_FR == 0) {
                return Err(Error::EngineStuck { cmd: self.preg(P_CMD) });
            }
        }
        Ok(())
    }

    /// A COMRESET (section 10.4.2): PxSCTL.DET = 1 for at least 1 ms, then
    /// 0; then the link comes back and the device's first register FIS
    /// clears BSY. The engine is stopped (ST = 0) and FIS receive on
    /// (FRE = 1).
    fn comreset(&mut self) -> Result<(), Error> {
        let sctl = self.preg(P_SCTL) & !DET_MASK;
        self.pwrite(P_SCTL, sctl | 1);
        hw::delay(&self.p, 2 * MS);
        self.pwrite(P_SCTL, sctl);
        if !self.wait_port(SECOND, P_SSTS, |s| s & DET_MASK == DET_PRESENT) {
            return Err(Error::NoLink { ssts: self.preg(P_SSTS) });
        }
        self.pwrite(P_SERR, !0);
        self.pwrite(P_IS, !0);
        if !self.wait_port(SPIN_UP, P_TFD, |t| t & (TFD_BSY | TFD_DRQ) == 0) {
            return Err(Error::Busy { tfd: self.preg(P_TFD) });
        }
        Ok(())
    }

    /// Gives the port the command list and FIS area and turns on FIS
    /// receive; polled, so no port interrupts.
    fn program_port(&mut self) {
        let clb = self.control.phys();
        let fb = clb + FIS_AT as u64;
        self.pwrite(P_CLB, clb as u32);
        self.pwrite(P_CLBU, (clb >> 32) as u32);
        self.pwrite(P_FB, fb as u32);
        self.pwrite(P_FBU, (fb >> 32) as u32);
        self.pwrite(P_IE, 0);
        let cmd = self.preg(P_CMD);
        self.pwrite(P_CMD, cmd | CMD_SUD | CMD_POD | CMD_FRE);
        self.pwrite(P_SERR, !0);
        self.pwrite(P_IS, !0);
    }

    fn ready(tfd: u32) -> bool {
        tfd & (TFD_BSY | TFD_DRQ) == 0
    }

    /// Sets the port up from whatever state it is in (a firmware's, a
    /// previous kernel's, an error): stops it (resetting the HBA if it will
    /// not stop), gives it this driver's memory, resets the link if asked
    /// or if the device is not ready, and starts the engine (ST, which
    /// section 10.3.1 allows only with BSY and DRQ clear and the link up).
    fn setup_port(&mut self, comreset: bool) -> Result<(), Error> {
        if let Err(e) = self.stop_engine() {
            log!(warn, "disk: AHCI port {}: {e}; resetting the HBA", self.port);
            self.hba_reset()?;
        }
        self.control.zero(0, CONTROL_LEN);
        // FIS receive before a COMRESET: the device's first register FIS,
        // which clears BSY in PxTFD, is received only with FRE on
        // (section 10.1.2).
        self.program_port();
        if comreset {
            self.comreset()?;
        }
        let up = |p: &P, port: usize| {
            Self::ready(p.read32(port_reg(port, P_TFD)))
                && p.read32(port_reg(port, P_SSTS)) & DET_MASK == DET_PRESENT
        };
        let port = self.port;
        if !hw::wait_for(&self.p, SECOND, |p| up(p, port)) {
            self.comreset()?;
        }
        let cmd = self.preg(P_CMD);
        self.pwrite(P_CMD, cmd | CMD_ST);
        self.in_flight = InFlight::None;
        self.broken = false;
        Ok(())
    }

    /// Puts `command` in slot 0 and issues it: the command header (FIS
    /// length 5 dwords, the write bit, one PRDT entry for `bytes` of the
    /// buffer) and the command table.
    fn issue(&mut self, command: u8, lba: u64, count: u16, bytes: usize, write: bool) {
        let prdtl = u32::from(bytes > 0);
        self.control.write32(0, 5 | u32::from(write) << 6 | prdtl << 16);
        self.control.write32(4, 0);
        let table = self.control.phys() + TABLE_AT as u64;
        self.control.write32(8, table as u32);
        self.control.write32(12, (table >> 32) as u32);
        self.control.zero(TABLE_AT, 0x80);
        self.control.copy_in(TABLE_AT, &h2d_fis(command, lba, count));
        if bytes > 0 {
            let dba = self.data.phys();
            self.control.write32(PRDT_AT, dba as u32);
            self.control.write32(PRDT_AT + 4, (dba >> 32) as u32);
            self.control.write32(PRDT_AT + 8, 0);
            self.control.write32(PRDT_AT + 12, (bytes - 1) as u32);
        }
        self.pwrite(P_IS, !0);
        self.started = self.p.now_ns();
        self.pwrite(P_CI, 1);
    }

    /// IDENTIFY DEVICE, waited for (at setup).
    fn identify(&mut self) -> Result<Identity, Error> {
        self.issue(ATA_IDENTIFY, 0, 0, 512, false);
        let port = self.port;
        let finished = hw::wait_for(&self.p, COMMAND_TIMEOUT, |p| {
            p.read32(port_reg(port, P_CI)) & 1 == 0
                || p.read32(port_reg(port, P_IS)) & (IS_TFES | IS_FATAL) != 0
        });
        let (tfd, is) = (self.preg(P_TFD), self.preg(P_IS));
        if !finished {
            return Err(Error::Timeout { tfd, is, ci: self.preg(P_CI) });
        }
        if is & (IS_TFES | IS_FATAL) != 0 || tfd & TFD_ERR != 0 {
            return Err(Error::Command { tfd, is });
        }
        let mut id = [0u8; 512];
        self.data.copy_out(0, &mut id);
        parse_identify(&id)
    }

    pub fn max_transfer(&self) -> usize {
        MAX_TRANSFER
    }

    fn check_transfer(&self, sector: u64, len: usize) -> Result<u16, DiskError> {
        if len == 0 || len > MAX_TRANSFER || !len.is_multiple_of(SECTOR) {
            return Err(DiskError::Unaligned);
        }
        if sector + (len / SECTOR) as u64 > self.identity.sectors {
            return Err(DiskError::OutOfRange);
        }
        Ok((len / SECTOR) as u16)
    }

    pub fn start_read(&mut self, sector: u64, len: usize) -> Result<(), DiskError> {
        let count = self.check_transfer(sector, len)?;
        self.issue(ATA_READ_DMA_EXT, sector, count, len, false);
        self.in_flight = InFlight::Read { sector, len };
        Ok(())
    }

    pub fn start_write(&mut self, sector: u64, data: &[u8]) -> Result<(), DiskError> {
        let count = self.check_transfer(sector, data.len())?;
        self.data.copy_in(0, data);
        self.issue(ATA_WRITE_DMA_EXT, sector, count, data.len(), true);
        self.in_flight = InFlight::Write { sector };
        Ok(())
    }

    /// FLUSH CACHE EXT, or nothing if the disk does not have it.
    pub fn start_flush(&mut self) -> Result<(), DiskError> {
        if !self.identity.flush {
            self.in_flight = InFlight::None;
            return Ok(());
        }
        self.issue(ATA_FLUSH_CACHE_EXT, 0, 0, 0, false);
        self.in_flight = InFlight::Flush;
        Ok(())
    }

    /// Whether the command finished, with or without an error (or there is
    /// none).
    pub fn done(&mut self) -> bool {
        if self.in_flight == InFlight::None {
            return true;
        }
        self.preg(P_CI) & 1 == 0 || self.preg(P_IS) & (IS_TFES | IS_FATAL) != 0
    }

    /// Whether the port stopped for good (for a caller that cannot wait
    /// for `reset`: the panic path).
    pub fn failed(&self) -> bool {
        self.p.read32(GHC) == !0
            || self.preg(P_IS) & IS_FATAL != 0
            || self.preg(P_CMD) & CMD_ST == 0
            || self.broken
    }

    /// Why the port must be set up again (`reset`), if it must: the HBA
    /// was reset or left AHCI mode, the engine stopped or lost its command
    /// list, a fatal error, a failed error recovery, or a command that did
    /// not finish in COMMAND_TIMEOUT.
    fn reset_reason(&self) -> Option<&'static str> {
        let ghc = self.p.read32(GHC);
        if ghc == !0 {
            return Some("the HBA does not answer");
        }
        if ghc & GHC_AE == 0 {
            return Some("the HBA left AHCI mode (reset)");
        }
        if self.preg(P_CMD) & CMD_ST == 0 {
            return Some("the port's command engine stopped");
        }
        if self.preg(P_CLB) != self.control.phys() as u32 {
            return Some("the port lost its command list");
        }
        if self.preg(P_IS) & IS_FATAL != 0 {
            return Some("a fatal port error");
        }
        if self.broken {
            return Some("error recovery failed");
        }
        if self.in_flight != InFlight::None
            && self.p.now_ns().saturating_sub(self.started) > COMMAND_TIMEOUT
        {
            return Some("a command did not finish in 30 s");
        }
        None
    }

    pub fn needs_reset(&self) -> bool {
        let Some(why) = self.reset_reason() else { return false };
        log!(
            warn,
            "disk: AHCI port {}: {why} (PxCMD {:#x}, PxIS {:#x}, PxTFD {:#x}, PxSERR {:#x}, PxCI {:#x}, {:?})",
            self.port,
            self.preg(P_CMD),
            self.preg(P_IS),
            self.preg(P_TFD),
            self.preg(P_SERR),
            self.preg(P_CI),
            self.in_flight
        );
        true
    }

    /// Sets the port up again after `needs_reset` (ARCHITECTURE invariant
    /// 4): AHCI mode back on, the engine stopped, a COMRESET, the engine
    /// started. The command in flight is forgotten (the caller issues it
    /// again).
    pub fn reset(&mut self) -> Result<(), Error> {
        if !hw::wait_for(&self.p, HBA_RESET, |p| p.read32(GHC) & GHC_HR == 0) {
            return Err(Error::HbaReset);
        }
        enable_ahci(&mut self.p);
        self.setup_port(true)
    }

    /// For `diskreset=`: resets the HBA under the driver.
    pub fn reset_now(&mut self) {
        let ghc = self.p.read32(GHC);
        self.p.write32(GHC, ghc | GHC_HR);
    }

    /// After a command error (PxIS.TFES), section 6.2.2.1: stop the engine,
    /// clear the errors, COMRESET if the device is still busy, start again.
    fn recover(&mut self) {
        let result = (|| {
            let cmd = self.preg(P_CMD);
            self.pwrite(P_CMD, cmd & !CMD_ST);
            if !self.wait_port(ENGINE_STOP, P_CMD, |c| c & CMD_CR == 0) {
                return Err(Error::EngineStuck { cmd: self.preg(P_CMD) });
            }
            self.pwrite(P_SERR, !0);
            self.pwrite(P_IS, !0);
            if !Self::ready(self.preg(P_TFD)) {
                self.comreset()?;
            }
            let cmd = self.preg(P_CMD);
            self.pwrite(P_CMD, cmd | CMD_ST);
            Ok(())
        })();
        if let Err(e) = result {
            log!(warn, "disk: AHCI port {}: error recovery failed: {e}", self.port);
            self.broken = true;
        }
    }

    /// The result of a finished command; on an error it is logged with the
    /// task file and the port recovered for the next one.
    pub fn finish(&mut self) -> Result<(), DiskError> {
        let flight = core::mem::replace(&mut self.in_flight, InFlight::None);
        if flight == InFlight::None {
            return Ok(());
        }
        let (is, tfd) = (self.preg(P_IS), self.preg(P_TFD));
        if is & (IS_TFES | IS_FATAL) != 0 || tfd & TFD_ERR != 0 || self.preg(P_CI) & 1 != 0 {
            log!(
                warn,
                "disk: AHCI port {}: {flight:?} failed: status {:#04x}, error {:#04x} (PxIS {is:#010x}, PxSERR {:#010x})",
                self.port,
                tfd & 0xff,
                tfd >> 8 & 0xff,
                self.preg(P_SERR)
            );
            if is & IS_FATAL == 0 {
                self.recover();
            }
            return Err(DiskError::Device);
        }
        Ok(())
    }

    /// Copies a finished read's data into `out`.
    pub fn finish_read(&mut self, out: &mut [u8]) -> Result<(), DiskError> {
        let flight = self.in_flight;
        let InFlight::Read { len, .. } = flight else { return Err(DiskError::Device) };
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
    use super::{Ahci, Error, REGS_LEN};
    use crate::arch::mmio::Mmio;
    use crate::arch::pci::{self, Function};
    use crate::disk::hw::KernelPlatform;

    const CLASS_STORAGE: u8 = 0x01;
    const SUBCLASS_SATA: u8 = 0x06;
    const PROG_IF_AHCI: u32 = 0x01;
    /// ABAR, the HBA's registers.
    const ABAR: u8 = 5;

    /// Finds the first AHCI HBA with a usable ATA disk and sets it up;
    /// each HBA that has none says why in the log.
    pub fn probe() -> Option<(Ahci<KernelPlatform>, pci::Address)> {
        let mut found: [Option<Function>; 8] = [None; 8];
        let mut n = 0;
        pci::for_each(|f| {
            let prog_if = f.config_u32(0x08) >> 8 & 0xff;
            if f.class == CLASS_STORAGE
                && f.subclass == SUBCLASS_SATA
                && prog_if == PROG_IF_AHCI
                && n < found.len()
            {
                found[n] = Some(*f);
                n += 1;
            }
        });
        for f in found.iter().flatten() {
            f.enable_memory_and_dma();
            let regs = match Mmio::bar(f, ABAR, REGS_LEN) {
                Ok(regs) => regs,
                Err(e) => {
                    warn!("disk: AHCI at {}: its registers could not be mapped ({e:?})", f.address);
                    continue;
                }
            };
            match Ahci::start(KernelPlatform::new(regs)) {
                Ok(dev) => return Some((dev, f.address)),
                Err(Error::NoDisk) => info!("disk: AHCI at {}: no ATA disk", f.address),
                Err(e) => warn!("disk: AHCI at {}: not usable: {e}", f.address),
            }
        }
        None
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

    /// IDENTIFY DEVICE data of a disk of `sectors` 512-byte sectors.
    fn identity_block(sectors: u64, model: &str) -> [u8; 512] {
        let mut id = [0u8; 512];
        let mut set = |w: usize, v: u16| id[2 * w..2 * w + 2].copy_from_slice(&v.to_le_bytes());
        set(83, 0x4000 | 1 << 10 | 1 << 13);
        for i in 0..4 {
            set(100 + i, (sectors >> (16 * i)) as u16);
        }
        set(106, 0x4000);
        let mut text = [b' '; 40];
        text[..model.len()].copy_from_slice(model.as_bytes());
        for i in 0..20 {
            set(27 + i, u16::from_be_bytes([text[2 * i], text[2 * i + 1]]));
        }
        set(10, u16::from_be_bytes(*b"SN"));
        set(11, u16::from_be_bytes(*b"42"));
        id
    }

    #[test]
    fn identify_is_parsed() {
        let id = identity_block(0x1_0000_0001, "QEMU HARDDISK");
        let identity = parse_identify(&id).unwrap();
        assert_eq!(identity.sectors, 0x1_0000_0001);
        assert_eq!(identity.model.as_str(), "QEMU HARDDISK");
        assert_eq!(identity.serial.as_str(), "SN42");
        assert!(identity.flush);
    }

    #[test]
    fn identify_refuses_what_the_driver_cannot_address() {
        let mut id = identity_block(100, "D");
        id[2 * 83 + 1] &= !(1 << 2); // word 83 bit 10: no LBA48
        assert_eq!(parse_identify(&id), Err(Error::NoLba48));
        let mut id = identity_block(100, "D");
        // Word 106: logical sectors longer than 256 words; 117-118: 2048 words.
        id[2 * 106..2 * 106 + 2].copy_from_slice(&(0x4000u16 | 1 << 12).to_le_bytes());
        id[2 * 117..2 * 117 + 2].copy_from_slice(&2048u16.to_le_bytes());
        assert_eq!(parse_identify(&id), Err(Error::SectorSize(4096)));
        // Word 106 not valid: 512 is assumed.
        id[2 * 106 + 1] = 0;
        assert!(parse_identify(&id).is_ok());
    }

    #[test]
    fn fis_encodes_a_48_bit_command() {
        let fis = h2d_fis(ATA_WRITE_DMA_EXT, 0x0605_0403_0201, 0x0180);
        assert_eq!(&fis[..4], &[0x27, 0x80, 0x35, 0]);
        assert_eq!(&fis[4..8], &[0x01, 0x02, 0x03, 0x40]);
        assert_eq!(&fis[8..11], &[0x04, 0x05, 0x06]);
        assert_eq!(&fis[12..14], &[0x80, 0x01]);
        assert_eq!(h2d_fis(ATA_IDENTIFY, 0, 0)[7], 0);
    }

    /// A model of an HBA with one SATA disk on port `disk_port` and a CD
    /// on port 0, executing a command synchronously when PxCI is written.
    struct Hba {
        ghc: u32,
        cap: u32,
        ports: [Port; 4],
        disk_port: usize,
        disk: Vec<u8>,
        identity: [u8; 512],
        /// The next transfer fails with an ABORT (TFES).
        fail_next: bool,
        /// Commands never complete (a hung device).
        hang: bool,
        commands: Vec<(u8, u64, u16)>,
    }

    #[derive(Default, Clone, Copy)]
    struct Port {
        clb: u64,
        fb: u64,
        is: u32,
        cmd: u32,
        tfd: u32,
        sig: u32,
        ssts: u32,
        sctl: u32,
        serr: u32,
        ci: u32,
        comresets: u32,
        /// The device's register FIS after a reset, not received yet:
        /// PxTFD reads 0x7f until FIS receive is on (FRE).
        fis_pending: bool,
    }

    struct FakeHba {
        hba: Rc<RefCell<Hba>>,
        phys: Shared,
        clock: Clock,
    }

    fn hba(sectors: u64) -> (FakeHba, Rc<RefCell<Hba>>, Shared) {
        let mut ports = [Port::default(); 4];
        ports[0] = Port { sig: 0xeb14_0101, ssts: 0x113, tfd: 0x50, ..Port::default() };
        ports[2] = Port { sig: SIG_ATA, ssts: 0x113, tfd: 0x50, ..Port::default() };
        let state = Rc::new(RefCell::new(Hba {
            ghc: 0,
            cap: 0,
            ports,
            disk_port: 2,
            disk: std::vec![0; sectors as usize * SECTOR],
            identity: identity_block(sectors, "FAKE DISK"),
            fail_next: false,
            hang: false,
            commands: Vec::new(),
        }));
        let phys = Phys::shared();
        let fake = FakeHba { hba: state.clone(), phys: phys.clone(), clock: Clock::default() };
        (fake, state, phys)
    }

    impl Hba {
        fn hba_reset(&mut self) {
            self.ghc = 0;
            for port in &mut self.ports {
                port.cmd = 0;
                port.is = 0;
                port.ci = 0;
                if port.ssts & 0xf == 3 {
                    port.tfd = 0x7f;
                    port.fis_pending = true;
                }
            }
        }

        fn execute(&mut self, n: usize, phys: &Shared) {
            let port = self.ports[n];
            let mut m = phys.borrow_mut();
            let dw0 = m.read32(port.clb);
            let table = m.read64(port.clb + 8);
            let mut fis = [0u8; 20];
            m.read(table, &mut fis);
            assert_eq!((fis[0], fis[1], dw0 & 0x1f), (0x27, 0x80, 5));
            let lba = u64::from_le_bytes([fis[4], fis[5], fis[6], fis[8], fis[9], fis[10], 0, 0]);
            let count = u16::from_le_bytes([fis[12], fis[13]]);
            self.commands.push((fis[2], lba, count));
            if self.hang {
                return;
            }
            let prdt = if dw0 >> 16 > 0 {
                let dba = m.read64(table + 0x80);
                let dbc = (m.read32(table + 0x8c) & 0x3f_ffff) as usize + 1;
                Some((dba, dbc))
            } else {
                None
            };
            let p = &mut self.ports[n];
            if n != self.disk_port || self.fail_next {
                self.fail_next = false;
                p.tfd = 0x04 << 8 | 0x51;
                p.is |= IS_TFES;
                return;
            }
            let at = lba as usize * SECTOR;
            let bytes = usize::from(count) * SECTOR;
            match fis[2] {
                ATA_IDENTIFY => m.write(prdt.unwrap().0, &self.identity),
                ATA_READ_DMA_EXT => {
                    assert_eq!(prdt.unwrap().1, bytes);
                    assert_eq!(dw0 & 1 << 6, 0);
                    m.write(prdt.unwrap().0, &self.disk[at..at + bytes]);
                }
                ATA_WRITE_DMA_EXT => {
                    assert_eq!(prdt.unwrap().1, bytes);
                    assert_ne!(dw0 & 1 << 6, 0);
                    m.read(prdt.unwrap().0, &mut self.disk[at..at + bytes]);
                }
                ATA_FLUSH_CACHE_EXT => {}
                other => panic!("unexpected command {other:#x}"),
            }
            p.tfd = 0x50;
            p.ci = 0;
        }
    }

    impl Platform for FakeHba {
        type Mem = FakeMem;
        fn read32(&self, offset: usize) -> u32 {
            let h = self.hba.borrow();
            match offset {
                CAP => h.cap,
                GHC => h.ghc,
                PI => 0b0111,
                VS => 0x0001_0301,
                CAP2 | BOHC => 0,
                _ => {
                    let n = (offset - 0x100) / 0x80;
                    let Some(p) = h.ports.get(n) else { return 0 };
                    match (offset - 0x100) % 0x80 {
                        P_CLB => p.clb as u32,
                        P_CLBU => (p.clb >> 32) as u32,
                        P_IS => p.is,
                        P_CMD => p.cmd,
                        P_TFD => p.tfd,
                        P_SIG => p.sig,
                        P_SSTS => p.ssts,
                        P_SCTL => p.sctl,
                        P_SERR => p.serr,
                        P_CI => p.ci,
                        _ => 0,
                    }
                }
            }
        }

        fn write32(&mut self, offset: usize, v: u32) {
            let mut h = self.hba.borrow_mut();
            match offset {
                GHC => {
                    if v & GHC_HR != 0 {
                        h.hba_reset();
                    } else {
                        h.ghc = v;
                    }
                }
                CAP | PI | VS | CAP2 | BOHC => {}
                _ => {
                    let n = (offset - 0x100) / 0x80;
                    assert_ne!(h.ghc & GHC_AE, 0, "port register written outside AHCI mode");
                    let p = &mut h.ports[n];
                    match (offset - 0x100) % 0x80 {
                        P_CLB => p.clb = p.clb & !0xffff_ffff | u64::from(v),
                        P_CLBU => p.clb = p.clb & 0xffff_ffff | u64::from(v) << 32,
                        P_FB => p.fb = p.fb & !0xffff_ffff | u64::from(v),
                        P_FBU => p.fb = p.fb & 0xffff_ffff | u64::from(v) << 32,
                        P_IS => p.is &= !v,
                        P_SERR => p.serr &= !v,
                        P_IE => {}
                        P_CMD => {
                            if v & CMD_ST != 0 {
                                assert_ne!(v & CMD_FRE, 0, "ST without FRE");
                                assert_eq!(p.tfd & 0x88, 0, "ST while BSY or DRQ");
                            }
                            p.cmd = v & !(CMD_CR | CMD_FR);
                            if v & CMD_ST != 0 {
                                p.cmd |= CMD_CR;
                            }
                            if v & CMD_FRE != 0 {
                                p.cmd |= CMD_FR;
                                if p.fis_pending {
                                    p.fis_pending = false;
                                    p.tfd = 0x50;
                                }
                            }
                        }
                        P_SCTL => {
                            assert_eq!(p.cmd & CMD_ST, 0, "COMRESET with the engine running");
                            if p.sctl & 0xf == 1 && v & 0xf == 0 {
                                p.comresets += 1;
                                p.ci = 0;
                                // The device's register FIS clears BSY
                                // once it can be received.
                                p.fis_pending = p.cmd & CMD_FRE == 0;
                                p.tfd = if p.fis_pending { 0x7f } else { 0x50 };
                            }
                            p.sctl = v;
                        }
                        P_CI => {
                            assert_ne!(p.cmd & CMD_ST, 0, "command issued with the engine off");
                            p.ci |= v;
                            if v & 1 != 0 {
                                let phys = self.phys.clone();
                                h.execute(n, &phys);
                            }
                        }
                        other => panic!("write to port register {other:#x}"),
                    }
                }
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

    fn run(dev: &mut Ahci<FakeHba>) -> bool {
        for _ in 0..10 {
            if dev.done() {
                return true;
            }
        }
        false
    }

    #[test]
    fn sets_up_the_disk_port_and_leaves_the_cd_alone() {
        let (fake, state, _) = hba(64);
        let dev = Ahci::start(fake).unwrap();
        assert_eq!(dev.port, 2);
        assert_eq!(dev.identity.sectors, 64);
        assert_eq!(dev.identity.model.as_str(), "FAKE DISK");
        let h = state.borrow();
        assert_ne!(h.ghc & GHC_AE, 0);
        assert_eq!(h.ports[0].cmd, 0, "the CD's port was touched");
        assert_eq!(h.ports[2].cmd & (CMD_ST | CMD_FRE), CMD_ST | CMD_FRE);
        assert_eq!(h.ports[2].clb, dev.control.phys());
        assert_eq!(h.ports[2].fb, dev.control.phys() + FIS_AT as u64);
        assert_eq!(h.commands, [(ATA_IDENTIFY, 0, 0)]);
    }

    #[test]
    fn no_disk_without_an_ata_signature() {
        let (fake, state, _) = hba(64);
        state.borrow_mut().ports[2].sig = 0xc33c_0101; // an enclosure
        assert_eq!(Ahci::start(fake).err(), Some(Error::NoDisk));
    }

    #[test]
    fn refuses_memory_above_4g_without_64_bit_addressing() {
        let (fake, _, phys) = hba(64);
        phys.borrow_mut().alloc(1 << 32);
        assert!(matches!(Ahci::start(fake).err(), Some(Error::Above4G { .. })));
        let (fake, state, phys) = hba(64);
        phys.borrow_mut().alloc(1 << 32);
        state.borrow_mut().cap = CAP_S64A;
        let dev = Ahci::start(fake).unwrap();
        assert_eq!(state.borrow().ports[2].clb, dev.control.phys());
    }

    #[test]
    fn reads_and_writes_sectors() {
        let (fake, state, _) = hba(256);
        let mut dev = Ahci::start(fake).unwrap();
        let data: Vec<u8> = (0..MAX_TRANSFER).map(|i| (i * 7) as u8).collect();
        dev.start_write(100, &data).unwrap();
        assert!(run(&mut dev));
        dev.finish().unwrap();
        assert_eq!(&state.borrow().disk[100 * SECTOR..][..MAX_TRANSFER], &data[..]);
        dev.start_read(101, 1024).unwrap();
        assert!(run(&mut dev));
        let mut out = [0u8; 1024];
        dev.finish_read(&mut out).unwrap();
        assert_eq!(&out[..], &data[SECTOR..SECTOR + 1024]);
        dev.start_flush().unwrap();
        assert!(run(&mut dev));
        dev.finish().unwrap();
        let commands = &state.borrow().commands;
        assert_eq!(commands[1..], [(0x35, 100, 128), (0x25, 101, 2), (0xea, 0, 0)]);
        assert_eq!(dev.start_read(255, 1024), Err(DiskError::OutOfRange));
    }

    #[test]
    fn a_failed_command_is_reported_and_the_port_recovers() {
        let (fake, state, _) = hba(64);
        let mut dev = Ahci::start(fake).unwrap();
        state.borrow_mut().fail_next = true;
        dev.start_read(0, 512).unwrap();
        assert!(run(&mut dev));
        let mut out = [0u8; 512];
        assert_eq!(dev.finish_read(&mut out), Err(DiskError::Device));
        assert!(!dev.needs_reset());
        // The next command works.
        dev.start_read(0, 512).unwrap();
        assert!(run(&mut dev));
        dev.finish_read(&mut out).unwrap();
    }

    #[test]
    fn an_hba_reset_under_the_driver_is_noticed_and_recovered() {
        let (fake, state, _) = hba(64);
        let mut dev = Ahci::start(fake).unwrap();
        dev.start_write(3, &[0x5a; 512]).unwrap();
        dev.reset_now();
        assert!(dev.needs_reset());
        dev.reset().unwrap();
        assert!(!dev.needs_reset());
        assert_eq!(state.borrow().ports[2].comresets, 1);
        dev.start_write(3, &[0x5a; 512]).unwrap();
        assert!(run(&mut dev));
        dev.finish().unwrap();
        assert_eq!(state.borrow().disk[3 * SECTOR], 0x5a);
    }

    #[test]
    fn a_command_that_never_finishes_times_out() {
        let (fake, state, _) = hba(64);
        let mut dev = Ahci::start(fake).unwrap();
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
    fn a_port_left_running_is_stopped_before_it_is_reprogrammed() {
        let (fake, state, _) = hba(64);
        {
            let mut h = state.borrow_mut();
            h.ghc = GHC_AE;
            h.ports[2].cmd = CMD_ST | CMD_CR | CMD_FRE | CMD_FR;
            h.ports[2].clb = 0xdead_0000;
        }
        let dev = Ahci::start(fake).unwrap();
        assert_eq!(state.borrow().ports[2].clb, dev.control.phys());
    }
}
