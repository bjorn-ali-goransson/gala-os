//! PSCI (Arm's Power State Coordination Interface, DEN0022): the
//! firmware's calls that start and stop CPUs, through the conduit the
//! FADT names (HVC or SMC: `boot::acpi::parse_fadt_psci`). SMC Calling
//! Convention: the function id in x0, arguments in x1-x3, the result in
//! x0.

use core::sync::atomic::{AtomicU8, Ordering};

/// PSCI_VERSION, CPU_OFF, CPU_ON and AFFINITY_INFO (SMC64 for the calls
/// that take addresses).
pub const VERSION: u32 = 0x8400_0000;
pub const CPU_OFF: u32 = 0x8400_0002;
pub const CPU_ON: u32 = 0xc400_0003;
pub const AFFINITY_INFO: u32 = 0xc400_0004;

/// AFFINITY_INFO's answer for a CPU that is off.
pub const AFFINITY_OFF: i64 = 1;

/// The conduit: 0 none (no PSCI), 1 HVC, 2 SMC. Read by the CPU_OFF stub
/// (`smp`), which runs on the bootloader's tables.
#[unsafe(no_mangle)]
pub static GALA_PSCI_CONDUIT: AtomicU8 = AtomicU8::new(0);

pub const CONDUIT_HVC: u8 = 1;
pub const CONDUIT_SMC: u8 = 2;

/// PSCI's error codes, by name (DEN0022 5.2.2).
pub fn error_name(code: i64) -> &'static str {
    match code {
        0 => "SUCCESS",
        -1 => "NOT_SUPPORTED",
        -2 => "INVALID_PARAMETERS",
        -3 => "DENIED",
        -4 => "ALREADY_ON",
        -5 => "ON_PENDING",
        -6 => "INTERNAL_FAILURE",
        -7 => "NOT_PRESENT",
        -8 => "DISABLED",
        -9 => "INVALID_ADDRESS",
        _ => "?",
    }
}

/// Sets the conduit (from the FADT) and logs PSCI's version.
pub fn init(psci: Option<crate::boot::acpi::Psci>) {
    let Some(p) = psci else {
        crate::warn!("psci: none offered by the firmware (FADT): CPUs it did not start stay off");
        return;
    };
    GALA_PSCI_CONDUIT.store(if p.hvc { CONDUIT_HVC } else { CONDUIT_SMC }, Ordering::Release);
    let v = call(VERSION, 0, 0, 0);
    crate::info!(
        "psci: version {}.{} through {}",
        (v >> 16) & 0x7fff,
        v & 0xffff,
        if p.hvc { "HVC" } else { "SMC" }
    );
}

pub fn available() -> bool {
    GALA_PSCI_CONDUIT.load(Ordering::Acquire) != 0
}

/// One call; NOT_SUPPORTED without a conduit.
pub fn call(function: u32, a1: u64, a2: u64, a3: u64) -> i64 {
    let mut x0 = u64::from(function);
    match GALA_PSCI_CONDUIT.load(Ordering::Acquire) {
        // SAFETY: a PSCI call to the hypervisor or the secure monitor:
        // it changes no memory of the kernel's; x0-x3 are its results'
        // and arguments, x4-x17 may be clobbered (SMCCC 1.0).
        CONDUIT_HVC => unsafe {
            core::arch::asm!(
                "hvc #0",
                inout("x0") x0, inout("x1") a1 => _, inout("x2") a2 => _, inout("x3") a3 => _,
                out("x4") _, out("x5") _, out("x6") _, out("x7") _, out("x8") _, out("x9") _,
                out("x10") _, out("x11") _, out("x12") _, out("x13") _, out("x14") _,
                out("x15") _, out("x16") _, out("x17") _,
                options(nostack)
            )
        },
        // SAFETY: as above, through the secure monitor.
        CONDUIT_SMC => unsafe {
            core::arch::asm!(
                "smc #0",
                inout("x0") x0, inout("x1") a1 => _, inout("x2") a2 => _, inout("x3") a3 => _,
                out("x4") _, out("x5") _, out("x6") _, out("x7") _, out("x8") _, out("x9") _,
                out("x10") _, out("x11") _, out("x12") _, out("x13") _, out("x14") _,
                out("x15") _, out("x16") _, out("x17") _,
                options(nostack)
            )
        },
        _ => return -1,
    }
    x0 as i64
}
