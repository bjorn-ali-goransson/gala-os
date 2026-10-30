//! gala-os kernel core.
//!
//! `unsafe` is denied crate-wide and only allowed in the thin layers that talk
//! to hardware (`arch`) and to the bootloader (`boot`). Everything else must be
//! safe Rust.

#![cfg_attr(not(test), no_std)]
#![deny(unsafe_code)]
#![deny(unsafe_op_in_unsafe_fn)]
// Host unit tests may use the infallible allocation APIs.
#![cfg_attr(test, allow(clippy::disallowed_methods, clippy::disallowed_macros))]

extern crate alloc;

#[allow(unsafe_code)]
pub mod arch;
#[allow(unsafe_code)]
pub mod boot;
pub mod cmdline;
#[macro_use]
pub mod console;
pub mod mm;
pub mod nt;
pub mod sched;
mod selftest;
pub mod sync;
pub mod time;

use core::sync::atomic::{AtomicBool, Ordering};

use arch::interrupts::{self, Fault};
use arch::qemu::{self, QemuExitCode};
use boot::{BootInfo, FirmwareType};
use cmdline::Cmdline;
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
    let cmdline = Cmdline::new(info.cmdline);
    if cmdline.flag(QEMU_EXIT_FLAG) {
        QEMU_EXIT.store(true, Ordering::Relaxed);
    }

    arch::gdt::init_bsp();
    arch::percpu::init_bsp();
    arch::cpu::enable_sse();
    interrupts::init();
    info!("cpu: GDT, TSS and IDT loaded");

    log_boot_info(&info);
    let summary = log_memory_map(&info);
    if let Some(fb) = &info.framebuffer {
        info!("framebuffer: {}x{} pitch={} bpp={}", fb.width(), fb.height(), fb.pitch(), fb.bpp());
        fb.fill(0x00_10_20_40);
    } else {
        warn!("framebuffer: none provided");
    }

    if summary.usable_bytes == 0 {
        panic!("no usable memory reported");
    }
    if let Err(e) = mm::init(&info) {
        panic!("frame allocator: {e:?}");
    }
    let (free, total) = mm::frame_stats();
    info!(
        "frames: {} free of {} tracked ({} MiB free)",
        free,
        total,
        free * mm::PAGE_SIZE / (1024 * 1024)
    );
    init_paging(&info);
    init_timer();
    if let Err(e) = sched::init() {
        panic!("scheduler: {e:?}");
    }
    info!("sched: boot thread {} running, idle thread started", sched::current());
    nt::init();

    if let Some(name) = cmdline.value("run") {
        run_program(&info, name);
    }

    if let Some(name) = cmdline.value("fault") {
        match Fault::from_name(name) {
            Some(fault) => {
                // On a scheduler thread, so stack overflows hit a guard page.
                info!("provoking fault {fault:?}");
                let index = Fault::ALL.iter().position(|&f| f == fault).unwrap_or(0);
                let provoke = |i: usize| interrupts::provoke(Fault::ALL[i]);
                match sched::spawn("fault", provoke, index) {
                    Ok(t) => {
                        let _ = sched::join(t);
                    }
                    Err(e) => panic!("spawn: {e:?}"),
                }
                panic!("provoked fault did not stop the kernel");
            }
            None => panic!("unknown fault {name:?}"),
        }
    }
    if cmdline.flag("selftest") {
        selftest::run_all();
    }

    kprintln!("{BOOT_OK_MARKER}");
    if QEMU_EXIT.load(Ordering::Relaxed) {
        qemu::exit(QemuExitCode::Success);
    }
    loop {
        arch::cpu::enable_interrupts_and_halt();
    }
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
        info!("kernel image: phys {:#x} -> virt {:#x}", addr.physical_base, addr.virtual_base);
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
        warn!("  total {:<22} {:>8} KiB", "unknown types", summary.unknown_bytes / 1024);
    }
    info!(
        "usable memory: {} MiB, highest usable address {:#x}",
        summary.usable_bytes / (1024 * 1024),
        summary.highest_usable_end
    );
    summary
}

/// Stops after a failure: reports to QEMU in test mode, otherwise halts.
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

fn init_paging(info: &BootInfo) {
    let Some(kernel) = info.kernel_address else {
        panic!("bootloader did not report the kernel's load address");
    };
    let mut regions = info.memory_map.iter();
    let layout = arch::paging::KernelLayout {
        hhdm_offset: info.hhdm_offset,
        kernel_phys_base: kernel.physical_base,
        kernel_virt_base: kernel.virtual_base,
        memory_map: &mut regions,
    };
    match arch::paging::init(layout) {
        Ok(stats) => info!(
            "paging: own tables at {:#x}; HHDM {}x4K {}x2M {}x1G, kernel {} pages",
            arch::paging::active_root(),
            stats.hhdm_pages[0],
            stats.hhdm_pages[1],
            stats.hhdm_pages[2],
            stats.kernel_pages
        ),
        Err(e) => panic!("paging: {e:?}"),
    }
}

fn init_timer() {
    let calibration = match arch::apic::init_bsp() {
        Ok(c) => c,
        Err(e) => panic!("local APIC: {e:?}"),
    };
    info!(
        "apic: id {} ({}), timer {} kHz, TSC {} MHz",
        calibration.apic_id,
        if calibration.x2apic { "x2APIC" } else { "xAPIC" },
        calibration.timer_hz / 1000,
        calibration.tsc_hz / 1_000_000
    );
    time::init(&calibration);
    arch::cpu::enable_interrupts();
    info!("interrupts enabled, tick {} Hz", time::TICK_HZ);
}

/// Runs the boot module `name` as a user process and waits for it.
fn run_program(info: &BootInfo, name: &str) {
    let Some(module) = info.modules.find(name) else {
        panic!("run: no boot module named {name:?}");
    };
    info!("run: {} ({} bytes)", module.path, module.data.len());
    match nt::process::spawn_from_pe(module.file_name(), module.data) {
        Ok((process, thread)) => {
            let _ = sched::join(thread);
            match process.exit_status() {
                Some(status) => {
                    info!("run: {} finished with status {:#010x}", process.name, status.0)
                }
                None => warn!("run: {} main thread ended without an exit status", process.name),
            }
        }
        Err(e) => error!("run: cannot start {name}: {e:?}"),
    }
}
