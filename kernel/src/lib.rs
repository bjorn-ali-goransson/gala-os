//! gala-os kernel core.
//!
//! `unsafe` is denied crate-wide and only allowed in the thin layers that talk
//! to hardware (`arch`) and to the bootloader (`boot`). Everything else must be
//! safe Rust.

#![cfg_attr(not(test), no_std)]
#![deny(unsafe_code)]
#![deny(unsafe_op_in_unsafe_fn)]

#[allow(unsafe_code)]
pub mod arch;
#[allow(unsafe_code)]
pub mod boot;
#[macro_use]
pub mod console;
pub mod mm;

use core::sync::atomic::{AtomicBool, Ordering};

use arch::qemu::{self, QemuExitCode};
use boot::{BootInfo, FirmwareType};
use mm::memmap::{MemorySummary, RegionKind};

/// Set when the kernel command line asks us to report the result to QEMU's
/// `isa-debug-exit` device and terminate instead of halting.
static QEMU_EXIT: AtomicBool = AtomicBool::new(false);

/// Kernel command-line flag that enables [`QEMU_EXIT`].
const QEMU_EXIT_FLAG: &str = "qemu_exit";

/// Marker line the test harness waits for on the serial console.
pub const BOOT_OK_MARKER: &str = "gala: boot ok";

pub fn kernel_main() -> ! {
    console::init();
    kprintln!();
    kprintln!("gala-os kernel {}", env!("CARGO_PKG_VERSION"));

    let info = match boot::collect() {
        Ok(info) => info,
        Err(e) => panic!("bootloader handoff rejected: {e}"),
    };
    if info
        .cmdline
        .split_ascii_whitespace()
        .any(|w| w == QEMU_EXIT_FLAG)
    {
        QEMU_EXIT.store(true, Ordering::Relaxed);
    }

    log_boot_info(&info);
    let summary = log_memory_map(&info);
    if let Some(fb) = &info.framebuffer {
        info!(
            "framebuffer: {}x{} pitch={} bpp={}",
            fb.width(),
            fb.height(),
            fb.pitch(),
            fb.bpp()
        );
        fb.fill(0x00_10_20_40);
    } else {
        warn!("framebuffer: none provided");
    }

    if summary.usable_bytes == 0 {
        panic!("no usable memory reported");
    }

    kprintln!("{BOOT_OK_MARKER}");
    finish(QemuExitCode::Success)
}

fn log_boot_info(info: &BootInfo) {
    info!("limine base revision: {}", info.base_revision);
    if let Some((name, version)) = info.bootloader {
        info!("bootloader: {name} {version}");
    }
    match info.firmware {
        Some(FirmwareType::Efi64) => info!("firmware: UEFI x86_64"),
        Some(other) => warn!("firmware: {other:?} (only UEFI x86_64 is supported)"),
        None => warn!("firmware: unknown"),
    }
    info!("cmdline: {:?}", info.cmdline);
    info!("hhdm offset: {:#018x}", info.hhdm_offset);
    if let Some(addr) = info.kernel_address {
        info!(
            "kernel image: phys {:#x} -> virt {:#x}",
            addr.physical_base, addr.virtual_base
        );
    }
}

fn log_memory_map(info: &BootInfo) -> MemorySummary {
    info!("memory map ({} entries):", info.memory_map.len());
    for region in info.memory_map.iter() {
        info!(
            "  {:#018x}-{:#018x} {:>10} KiB  {}",
            region.base,
            region.end(),
            region.len / 1024,
            region.kind
        );
    }
    let summary = match MemorySummary::from_regions(info.memory_map.iter()) {
        Ok(summary) => summary,
        Err(e) => panic!("invalid memory map: {e}"),
    };
    for kind in RegionKind::KNOWN {
        let bytes = summary.bytes_of(kind);
        if bytes != 0 {
            info!("  total {:<22} {:>8} KiB", kind.name(), bytes / 1024);
        }
    }
    if summary.unknown_bytes != 0 {
        warn!(
            "  total {:<22} {:>8} KiB",
            "unknown types",
            summary.unknown_bytes / 1024
        );
    }
    info!(
        "usable memory: {} MiB, highest usable address {:#x}",
        summary.usable_bytes / (1024 * 1024),
        summary.highest_usable_end
    );
    summary
}

/// Ends the boot path: reports to QEMU in test mode, otherwise idles forever.
fn finish(code: QemuExitCode) -> ! {
    if QEMU_EXIT.load(Ordering::Relaxed) {
        qemu::exit(code);
    }
    arch::cpu::halt_forever()
}

/// Called from the executable's `#[panic_handler]`.
pub fn panic(info: &core::panic::PanicInfo) -> ! {
    console::emergency_println(format_args!("KERNEL PANIC: {info}"));
    finish(QemuExitCode::Failure)
}
