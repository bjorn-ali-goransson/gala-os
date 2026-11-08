//! Boot phase markers (docs/OBSERVABILITY.md, "Early boot"): one line per
//! step of the boot, written to a debug console port of their own (QEMU
//! `isa-debugcon,iobase=0x403`, which xtask captures to
//! `<image>.phases.log`), so the last step reached is on the host even when
//! the machine dies without a word (a triple fault resets it before the
//! serial log says anything). Lines: `phase <name> cpu <apic id> tsc <n>`,
//! and `fault <vector> rip <addr>` from the exception path.
//!
//! Lock-free (a spin flag, interrupts masked) and allocation-free, so it
//! works from the first instruction of `kernel_main` and from fault
//! handlers. Written only under QEMU (CPUID's hypervisor leaf says TCG or
//! KVM): on a real PC an I/O port in this range may belong to the
//! chipset (ACPI's power-management block often sits at 0x400).

use core::fmt::Write;
use core::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use super::cpu;
use super::port::Port;

/// The port (one past SeaBIOS's 0x402, which xtask captures as well).
pub const PHASE_PORT: u16 = 0x403;

static BUSY: AtomicBool = AtomicBool::new(false);

/// 0: not known yet, 1: under QEMU, 2: not.
static UNDER_QEMU: AtomicU8 = AtomicU8::new(0);

fn under_qemu() -> bool {
    match UNDER_QEMU.load(Ordering::Relaxed) {
        1 => true,
        2 => false,
        _ => {
            let hypervisor = cpu::cpuid(1, 0).ecx & (1 << 31) != 0;
            let v = cpu::cpuid(0x4000_0000, 0);
            let mut name = [0u8; 12];
            name[..4].copy_from_slice(&v.ebx.to_le_bytes());
            name[4..8].copy_from_slice(&v.ecx.to_le_bytes());
            name[8..].copy_from_slice(&v.edx.to_le_bytes());
            let yes = hypervisor && (&name == b"TCGTCGTCGTCG" || &name == b"KVMKVMKVM\0\0\0");
            UNDER_QEMU.store(if yes { 1 } else { 2 }, Ordering::Relaxed);
            yes
        }
    }
}

struct PhasePort(Port<u8>);

impl Write for PhasePort {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        for b in s.bytes() {
            self.0.write(b);
        }
        Ok(())
    }
}

/// The local APIC id from CPUID (works before the APIC is set up).
fn apic_id() -> u32 {
    cpu::cpuid(1, 0).ebx >> 24
}

fn write_line(args: core::fmt::Arguments) {
    if !under_qemu() {
        return;
    }
    let restore = cpu::disable_interrupts();
    // Bounded: a CPU that died holding the flag must not stop the others
    // from reporting (lines may then interleave).
    let mut spins = 0u32;
    while BUSY.swap(true, Ordering::Acquire) && spins < 1_000_000 {
        core::hint::spin_loop();
        spins += 1;
    }
    // SAFETY: the port is QEMU's debug console (or nothing); writing it
    // has no memory effects.
    let mut port = PhasePort(unsafe { Port::new(PHASE_PORT) });
    let _ = port.write_fmt(args);
    BUSY.store(false, Ordering::Release);
    if restore {
        cpu::enable_interrupts();
    }
}

/// Records that the boot reached `phase`.
pub fn mark(phase: &str) {
    write_line(format_args!("phase {phase} cpu {} tsc {}\n", apic_id(), cpu::rdtsc()));
}

/// Records a phase with a number (e.g. `ap_online` and the CPU index).
pub fn mark_n(phase: &str, n: u64) {
    write_line(format_args!("phase {phase} {n} cpu {} tsc {}\n", apic_id(), cpu::rdtsc()));
}

/// Records a CPU exception before it is handled (the handler's own report
/// may not get out).
pub fn fault(vector: u8, rip: u64, error: u64) {
    write_line(format_args!(
        "fault {vector} rip {rip:#x} error {error:#x} cpu {} tsc {}\n",
        apic_id(),
        cpu::rdtsc()
    ));
}

/// The phase after the tick source is calibrated and started (its name
/// on this architecture: the local APIC's).
pub const TIMER: &str = "apic";

/// Records a panic (its message goes to the serial log).
pub fn panic() {
    write_line(format_args!("panic cpu {} tsc {}\n", apic_id(), cpu::rdtsc()));
}
