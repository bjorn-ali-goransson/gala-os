//! The serial console: a PL011 UART (polled, transmit only), the one the
//! firmware's SPCR names (a PL011 or an SBSA generic UART, whose
//! registers are a PL011's); without an SPCR, QEMU `virt`'s first one at
//! physical 0x0900_0000 (the board's layout), which `report` says.
//!
//! The UART is memory-mapped, and the bootloader maps no device, so
//! `init` maps its page first (`early_map`); until then, and if that
//! fails, writes go nowhere. The firmware has already programmed it (the
//! boot messages came out of it), so `init` changes no setting.
//!
//! Its one input: a break on the line asks for a state dump, as x86-64's
//! NMI from QEMU's monitor does (QEMU `virt` has no NMI to inject; xtask
//! sends the break at a timeout, `chardev-send-break`). `listen` turns the
//! receive interrupts on; the GIC takes the UART's interrupt at the NMI's
//! priority (`gic::enable_console`), so a CPU spinning with interrupts
//! off still takes it, and `take_input` drains what came.

use core::fmt;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use crate::sync::IrqMutex;

/// QEMU `virt`'s first PL011, without an SPCR.
const UART0_PHYS: u64 = 0x0900_0000;
/// SPCR interface types with a PL011's registers: the PL011, the SBSA
/// generic UART (32-bit access and its older form).
const PL011_INTERFACES: [u8; 3] = [0x03, 0x0d, 0x0e];

/// The UART's physical address, and whether the SPCR named it.
static PHYS: AtomicU64 = AtomicU64::new(0);
static FROM_SPCR: AtomicBool = AtomicBool::new(false);

/// PL011 registers: data (BE: the entry is a break), flags (TXFF: the
/// transmit FIFO is full; RXFE: the receive FIFO is empty), the interrupt
/// mask (RXIM receive, RTIM receive timeout, BEIM break) and clear.
const DR: u64 = 0x00;
const DR_BE: u32 = 1 << 10;
const FR: u64 = 0x18;
const FR_TXFF: u32 = 1 << 5;
const FR_RXFE: u32 = 1 << 4;
const IMSC: u64 = 0x38;
const ICR: u64 = 0x44;
const INT_RX: u32 = (1 << 4) | (1 << 6) | (1 << 9);

/// QEMU `virt`'s first PL011's interrupt (SPI 1), without an SPCR.
const UART0_INTID: u32 = 33;
/// The UART's GIC interrupt (the SPCR's GSIV).
static INTID: AtomicU32 = AtomicU32::new(UART0_INTID);
/// Breaks taken, and other bytes received (ignored).
static BREAKS: AtomicU64 = AtomicU64::new(0);
static IGNORED: AtomicU64 = AtomicU64::new(0);

/// The UART's virtual address once mapped (0: not yet).
static BASE: AtomicU64 = AtomicU64::new(0);

/// The console's UART.
pub static COM1: IrqMutex<SerialPort> = IrqMutex::new(SerialPort);

/// The UART without its lock, for a panic that may hold it (text may
/// interleave with the holder's; nothing else).
pub fn com1_emergency() -> SerialPort {
    SerialPort
}

pub struct SerialPort;

impl SerialPort {
    /// Maps the UART (once). Its failure is printed nowhere but kept: the
    /// first line out of a working console is the kernel's banner.
    pub fn init(&mut self) {
        if BASE.load(Ordering::Relaxed) != 0 {
            return;
        }
        let Some((hhdm, kernel)) = crate::boot::early_addresses() else { return };
        let spcr = crate::boot::acpi::read_spcr(hhdm, crate::boot::early_rsdp())
            .filter(|u| PL011_INTERFACES.contains(&u.interface));
        let phys = spcr.map_or(UART0_PHYS, |u| u.base);
        if let Some(u) = spcr {
            // An SPCR without a GIC interrupt: none to listen on.
            INTID.store(u.gsiv.unwrap_or(0), Ordering::Relaxed);
        }
        FROM_SPCR.store(spcr.is_some(), Ordering::Relaxed);
        PHYS.store(phys, Ordering::Relaxed);
        if let Ok(va) = super::early_map::map_device(phys & !0xfff, hhdm, kernel) {
            BASE.store(va + (phys & 0xfff), Ordering::Relaxed);
        }
    }

    pub fn write_byte(&mut self, byte: u8) {
        let base = BASE.load(Ordering::Relaxed);
        if base == 0 {
            return;
        }
        // Bounded spin so a wedged UART cannot hang the kernel.
        for _ in 0..100_000 {
            // SAFETY: the UART's mapped flag register (Device memory).
            let flags = unsafe { ((base + FR) as *const u32).read_volatile() };
            if flags & FR_TXFF == 0 {
                break;
            }
            core::hint::spin_loop();
        }
        // SAFETY: the UART's mapped data register.
        unsafe { ((base + DR) as *mut u32).write_volatile(u32::from(byte)) };
    }
}

fn reg_read(base: u64, reg: u64) -> u32 {
    // SAFETY: a register of the UART's mapping (Device memory).
    unsafe { ((base + reg) as *const u32).read_volatile() }
}

fn reg_write(base: u64, reg: u64, value: u32) {
    // SAFETY: as reg_read.
    unsafe { ((base + reg) as *mut u32).write_volatile(value) }
}

/// The UART's GIC interrupt (0: none known).
pub fn interrupt() -> u32 {
    INTID.load(Ordering::Relaxed)
}

/// Turns the receive interrupts on (after the GIC routes the UART's,
/// `gic::enable_console`); what was received before is dropped.
pub fn listen() {
    let base = BASE.load(Ordering::Relaxed);
    if base == 0 {
        return;
    }
    let _ = take_input();
    reg_write(base, ICR, INT_RX);
    reg_write(base, IMSC, reg_read(base, IMSC) | INT_RX);
}

/// Drains the receive FIFO and clears its interrupts (the GIC's handler
/// of the UART's interrupt, at the NMI's priority: no lock is taken).
/// True if a break came, a state dump's request.
pub fn take_input() -> bool {
    let base = BASE.load(Ordering::Relaxed);
    if base == 0 {
        return false;
    }
    let mut asked = false;
    for _ in 0..64 {
        if reg_read(base, FR) & FR_RXFE != 0 {
            break;
        }
        if reg_read(base, DR) & DR_BE != 0 {
            asked = true;
        } else {
            IGNORED.fetch_add(1, Ordering::Relaxed);
        }
    }
    reg_write(base, ICR, INT_RX);
    if asked {
        BREAKS.fetch_add(1, Ordering::Relaxed);
    }
    asked
}

/// (breaks taken, other bytes ignored) since boot.
pub fn input_counts() -> (u64, u64) {
    (BREAKS.load(Ordering::Relaxed), IGNORED.load(Ordering::Relaxed))
}

/// Says which UART the console is and where its address came from.
pub fn report() {
    let phys = PHYS.load(Ordering::Relaxed);
    if FROM_SPCR.load(Ordering::Relaxed) {
        crate::info!("serial: PL011 at {phys:#x} (the firmware's SPCR)");
    } else {
        crate::warn!("serial: PL011 at {phys:#x} (QEMU virt's: no SPCR from the firmware)");
    }
}

impl fmt::Write for SerialPort {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for byte in s.bytes() {
            if byte == b'\n' {
                self.write_byte(b'\r');
            }
            self.write_byte(byte);
        }
        Ok(())
    }
}
