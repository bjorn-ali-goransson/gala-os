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
#[cfg(feature = "hosted")]
extern crate std;

#[allow(unsafe_code)]
pub mod arch;
#[allow(unsafe_code)]
pub mod boot;
pub mod clockctl;
pub mod cmdline;
#[macro_use]
pub mod console;
pub mod crash;
pub mod disk;
pub mod events;
pub mod failpoint;
pub mod flight;
pub mod fs;
/// gala-um's platform: the NT layer inside a Linux process.
#[cfg(feature = "hosted")]
#[allow(unsafe_code)]
pub mod hosted;
#[cfg(not(feature = "hosted"))]
pub mod input;
pub mod mm;
pub mod net;
pub mod nt;
#[cfg(not(feature = "hosted"))]
pub mod platform;
pub mod replay;
pub mod sched;
mod selftest;
/// Deterministic simulation's scheduler and clock (gala-um `--sim`).
#[cfg(any(test, feature = "hosted"))]
pub mod sim;
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

    let mut info = match boot::collect() {
        Ok(info) => info,
        Err(e) => panic!("bootloader handoff rejected: {e}"),
    };
    let cmdline = Cmdline::new(info.cmdline);
    if cmdline.flag(QEMU_EXIT_FLAG) {
        QEMU_EXIT.store(true, Ordering::Relaxed);
    }

    arch::gdt::init_bsp();
    arch::percpu::init_bsp();
    // Lock-order checking (debug builds) needs to know the CPU: from here.
    sync::lockdep::enable();
    arch::cpu::enable_sse();
    interrupts::init();
    arch::syscall::init_cpu();
    info!("cpu: GDT, TSS and IDT loaded");

    log_boot_info(&info);
    let summary = log_memory_map(&info);
    // `noscreen`: serial only (boot tests read the serial log; drawing text
    // is slow under emulation).
    if let Some(fb) = info.framebuffer.take().filter(|_| !cmdline.flag("noscreen")) {
        info!("framebuffer: {}x{} pitch={} bpp={}", fb.width(), fb.height(), fb.pitch(), fb.bpp());
        console::attach_framebuffer(
            fb,
            concat!("gala-os ", env!("CARGO_PKG_VERSION"), "  |  qal'a  |  Built in Saudi"),
            // `bootlog`: the log's text instead of the splash.
            cmdline.flag("bootlog"),
        );
        kprintln!("gala-os kernel {} (screen console)", env!("CARGO_PKG_VERSION"));
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
    init_timer(info.boot_unix_time);
    if let Err(e) = sched::init() {
        panic!("scheduler: {e:?}");
    }
    info!("sched: boot thread {} running, idle thread started", sched::current());
    arch::smp::init_bsp();
    match arch::smp::start_aps(&info.processors, sched::run_ap) {
        Ok(n) => {
            info!("smp: {n} CPUs online ({} reported by the bootloader)", info.processors.len())
        }
        Err(e) => warn!("smp: cannot start the other CPUs: {e:?}"),
    }
    disk::init();
    // The network driver's recovery test: reset after the n-th frame sent.
    if let Some(n) = cmdline.value("netreset").and_then(|v| v.parse().ok()) {
        net::virtio_net::RESET_AT.store(n, Ordering::Relaxed);
    }
    // The disk driver's recovery test: the n-th request's device is reset.
    if let Some(n) = cmdline.value("diskreset").and_then(|v| v.parse().ok()) {
        disk::RESET_AT.store(n, Ordering::Relaxed);
    }
    nt::init();
    net::init();
    for module in info.modules.iter() {
        // `tar:<dir>`: a directory tree packed by xtask, shipped as C:\<dir>
        // (read-only files); `tarw:<dir>`: the same, writable (data files).
        if let Some(dir) = module.cmdline.strip_prefix("tar:") {
            add_tree(dir, module.path, module.data, false);
        } else if let Some(dir) = module.cmdline.strip_prefix("tarw:") {
            add_tree(dir, module.path, module.data, true);
        } else if module.cmdline == "registry" {
            // A scenario's registry keys (nt::registry::load_more).
            let loaded = core::str::from_utf8(module.data)
                .map_err(|_| nt::status::NtStatus::INVALID_PARAMETER)
                .and_then(|text| nt::registry::load_more(text, nt::platform::system_time()));
            if let Err(e) = loaded {
                warn!("registry: cannot load {}: {e:?}", module.path);
            }
        } else if let Err(e) =
            nt::file::add_boot_file(module.cmdline, module.file_name(), module.data, false)
        {
            warn!("fs: cannot add {}: {e:?}", module.path);
        }
    }

    // Profiles among the boot's files belong to their users.
    if let Err(e) = nt::accounts::assign_boot_profiles() {
        warn!("profiles: {e:?}");
    }
    // `persist`: C: is kept on the disk (changes since earlier boots are
    // replayed now, over the boot's files).
    if cmdline.flag("persist") {
        nt::persist::CRASH_TEST.store(cmdline.flag("crashtest"), Ordering::Relaxed);
        nt::persist::mount();
    }

    // The desktop's wallpaper (`wallpaper=<name>`, else one at random).
    nt::desktop::choose_wallpaper(cmdline.value("wallpaper"));
    #[cfg(not(feature = "desktop"))]
    info!("desktop: not in this kernel (server build)");
    // `profile=server` (ROADMAP SD): no graphical interface, ever.
    if cmdline.value("profile") == Some("server") {
        nt::desktop::set_server_profile();
        info!("profile: server (no desktop, no console windows)");
    }
    nt::syscalls::set_trace(cmdline.flag("trace"));
    // The watchdog's test: CPU 1 stops taking timer interrupts for a while.
    time::set_hang_test(cmdline.flag("hangtest"));
    // The kernel watchdog's test: this thread stays in the kernel 25 s
    // (interrupts on, never blocking).
    if cmdline.flag("spintest") {
        info!("spintest: the boot thread spins in the kernel for 25 s");
        let until = time::raw_monotonic_ns() + 25_000_000_000;
        while time::raw_monotonic_ns() < until {
            core::hint::spin_loop();
        }
        info!("spintest: done");
    }
    events::set_mask(cmdline.value("events").map_or(0, events::parse_mask));
    // `run=a.exe,b.exe` runs the programs one after another; `args=` holds
    // their (percent-encoded) command-line arguments in the same order;
    // `cwd=C:/dir` (percent-encoded) is their current directory (default
    // C:\gala).
    // `as=system`: they run as SYSTEM (the services' account), else as
    // the boot user (nt::token).
    // `wait=all`: a program has ended when the processes it left running
    // have too (sshd -d hands its connection to sshd-session and exits).
    nt::token::set_boot_system(cmdline.value("as") == Some("system"));
    let wait_all = cmdline.value("wait") == Some("all");
    // `failpoint=heap:every:97,...` (and `failseed=<n>`): fault injection
    // while the programs run (kernel/src/failpoint.rs).
    configure_failpoints(&cmdline);
    failpoint::arm();
    // `clock=control,freeze,...`: clock control (kernel/src/clockctl.rs),
    // from the programs' start on.
    configure_clock(&cmdline);
    time::arm_clock_control();
    // `record`: what the programs read from outside goes to the debug
    // console; `replay=<boot module>`: they read it from that recording
    // (kernel/src/replay.rs).
    start_record_replay(&info, &cmdline);
    // `start=a.exe,b.exe` (with `start_args=`) starts programs first and
    // does not wait for them: a server the `run=` programs talk to. They
    // end with the boot.
    if let Some(names) = cmdline.value("start") {
        let mut args = cmdline.value("start_args").unwrap_or("").split(',');
        for name in names.split(',').filter(|n| !n.is_empty()) {
            let mut decoded = alloc::vec::Vec::new();
            if cmdline::percent_decode(args.next().unwrap_or(""), &mut decoded).is_err() {
                error!("start: out of memory for {name}'s arguments");
                continue;
            }
            let console = console_window(&info, name, cmdline.value("console"));
            start_program(&info, name, core::str::from_utf8(&decoded).unwrap_or(""), console);
        }
    }
    if let Some(names) = cmdline.value("run") {
        let mut args = cmdline.value("args").unwrap_or("").split(',');
        for name in names.split(',').filter(|n| !n.is_empty()) {
            let mut decoded = alloc::vec::Vec::new();
            info!("run: begin {name}");
            if cmdline::percent_decode(args.next().unwrap_or(""), &mut decoded).is_err() {
                error!("run: out of memory for {name}'s arguments");
                info!("run: end {name}");
                continue;
            }
            // `cwd=` is percent-encoded too (C:/Program%20Files/...).
            let mut cwd_decoded = alloc::vec::Vec::new();
            let cwd = match cmdline.value("cwd") {
                Some(c) if cmdline::percent_decode(c, &mut cwd_decoded).is_ok() => {
                    core::str::from_utf8(&cwd_decoded).ok()
                }
                other => other,
            };
            let console = console_window(&info, name, cmdline.value("console"));
            run_program(
                &info,
                name,
                core::str::from_utf8(&decoded).unwrap_or(""),
                cwd,
                wait_all,
                console,
            );
            // What the program's end changed (delete-on-close at exit).
            nt::persist::sync();
            nt::ledger::dump();
            info!("run: end {name}");
        }
    }
    failpoint::disarm();
    if failpoint::configured() {
        info!("failpoint: {}", failpoint::Summary);
    }
    replay::finish();

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
    if cmdline.flag("net_echo") {
        net::echo_test();
    }
    if cmdline.flag("selftest") {
        selftest::run_all();
    }

    nt::ledger::dump();
    if events::written() > 0 {
        info!("events: {} written to the debug console", events::written());
    }
    net::log_stats();
    info!("nt: {}", nt::stats::Summary);
    let (shootdowns, ipis) = arch::smp::flush_stats();
    let heap = mm::heap::stats();
    let window_flushes = mm::vmap::WINDOW.lock().flushes();
    info!(
        "stats: {shootdowns} TLB shootdowns ({ipis} IPIs), {} frames leaked by deferred frees; \
         heap {} KiB in use, {} large pages, {window_flushes} mapped-window flushes; \
         frames free {} of {}",
        arch::paging::leaked_frames(),
        heap.in_use / 1024,
        heap.large_pages,
        mm::frame_stats().0,
        mm::frame_stats().1
    );
    kprintln!("{BOOT_OK_MARKER}");
    if QEMU_EXIT.load(Ordering::Relaxed) {
        qemu::exit(QemuExitCode::Success);
    }
    loop {
        arch::cpu::enable_interrupts_and_halt();
    }
}

/// Installs the `failpoint=`/`failseed=` rules (not armed yet).
fn configure_failpoints(cmdline: &Cmdline) {
    let Some(spec) = cmdline.value("failpoint") else { return };
    if !failpoint::COMPILED_IN {
        warn!("failpoint: compiled out of release builds; {spec:?} ignored");
        return;
    }
    // `failtrace`: each injected failure logged with a backtrace.
    failpoint::set_trace(cmdline.flag("failtrace"));
    match failpoint::configure_from(spec, cmdline.value("failseed")) {
        Ok(()) => info!("failpoint: {spec} (seed {})", cmdline.value("failseed").unwrap_or("0")),
        Err(e) => error!("failpoint: {spec:?}: {e:?}"),
    }
}

/// Waits up to `secs` seconds of real time for `objects` (one of them);
/// whether it was signaled. Under clock control the programs' clock may
/// be frozen or jump, so the wait looks at the real clock every second of
/// the programs' time (with no clock control: one wait).
fn wait_real_secs(objects: &[nt::object::ObjectRef], secs: u64) -> bool {
    use nt::wait::Timeout;
    let until = time::raw_ticks() + secs * time::TICK_HZ;
    loop {
        let step = if clockctl::armed() { time::TICK_HZ } else { secs * time::TICK_HZ };
        let timeout = Timeout::Tick(time::ticks() + step);
        if nt::wait::wait_for(objects, false, timeout) != nt::status::NtStatus::TIMEOUT {
            return true;
        }
        if time::raw_ticks() >= until {
            return false;
        }
    }
}

/// `record` / `replay=<boot module>`: starts recording the programs'
/// inputs to the debug console, or replaying them from the boot module
/// with that file name; the boot time is the first input.
fn start_record_replay(info: &BootInfo, cmdline: &Cmdline) {
    if cmdline.flag("record") {
        replay::record_to(events::emit_record);
        replay::info("cmdline", cmdline.as_str());
        info!("replay: recording to the debug console");
    } else if let Some(name) = cmdline.value("replay") {
        let module = info.modules.iter().find(|m| m.file_name() == name);
        match module.map(|m| replay::replay_from(m.data)) {
            Some(Ok(())) => info!("replay: replaying {name}"),
            Some(Err(e)) => error!("replay: {name}: not a recording this build reads ({e:?})"),
            None => error!("replay: no boot module {name}"),
        }
    }
    if replay::on() {
        time::set_boot_system_time(replay::u64_input(
            replay::Kind::BootTime,
            0,
            time::boot_system_time,
        ));
    }
}

/// Installs the `clock=` settings (armed when the programs start).
fn configure_clock(cmdline: &Cmdline) {
    let Some(spec) = cmdline.value("clock") else { return };
    if !clockctl::COMPILED_IN {
        warn!("clock: clock control is compiled out of release builds; {spec:?} ignored");
        return;
    }
    if let Err(item) = clockctl::configure(spec) {
        error!("clock: {spec:?}: cannot use {item:?}");
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
    // Return addresses, innermost first; `cargo xtask` turns them into
    // function names and lines from the kernel binary.
    struct Backtrace;
    impl core::fmt::Display for Backtrace {
        fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
            f.write_str("backtrace:")?;
            let mut result = Ok(());
            arch::backtrace::walk(|ret| {
                if result.is_ok() {
                    result = write!(f, " {ret:#x}");
                }
            });
            result
        }
    }
    console::emergency_println(format_args!("{Backtrace}"));
    // What the CPUs did last (the flight recorder).
    flight::dump(u64::MAX, 48, console::emergency_println);
    // Kept on the disk for the next boot (when a volume is mounted).
    crash::save(info, &Backtrace);
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

fn init_timer(boot_time: Option<i64>) {
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
    if let Some(t) = boot_time {
        time::set_boot_time(t);
    }
    arch::cpu::enable_interrupts();
    info!("interrupts enabled, tick {} Hz", time::TICK_HZ);
}

/// Adds the regular files (in place) and directories of the USTAR archive
/// `data` (boot module `path`) under C:\\<dir>.
fn add_tree(dir: &str, path: &str, data: &'static [u8], writable: bool) {
    let mut entries = fs::tar::entries(data);
    let mut count = 0;
    loop {
        match entries.next() {
            Ok(Some(entry)) if entry.directory => {
                let mut buf = [0u8; 512];
                match join_dir(&mut buf, dir, entry.path).map(nt::file::add_boot_directory) {
                    Some(Ok(())) => {}
                    Some(Err(e)) => warn!("fs: cannot add {dir}/{}: {e:?}", entry.path),
                    None => warn!("fs: path too long: {}", entry.path),
                }
            }
            Ok(Some(entry)) => {
                let (sub, name) = entry.path.rsplit_once('/').unwrap_or(("", entry.path));
                let mut buf = [0u8; 512];
                let added = join_dir(&mut buf, dir, sub);
                match added.map(|full| nt::file::add_boot_file(full, name, entry.data, writable)) {
                    Some(Ok(())) => count += 1,
                    Some(Err(e)) => warn!("fs: cannot add {dir}/{}: {e:?}", entry.path),
                    None => warn!("fs: path too long: {}", entry.path),
                }
            }
            Ok(None) => break,
            Err(e) => {
                warn!("fs: archive {path}: {e:?}");
                break;
            }
        }
    }
    info!("fs: {count} files from {path} in C:\\{dir}");
}

/// `dir/sub` (or `dir` when `sub` is empty) in `buf`.
fn join_dir<'b>(buf: &'b mut [u8; 512], dir: &str, sub: &str) -> Option<&'b str> {
    let mut len = 0;
    for part in [dir, if sub.is_empty() { "" } else { "/" }, sub] {
        buf.get_mut(len..len + part.len())?.copy_from_slice(part.as_bytes());
        len += part.len();
    }
    core::str::from_utf8(&buf[..len]).ok()
}

/// The program `name`: a boot module, or a file in a `tar:` module (a
/// directory tree); its file name, contents and directory on C: (in
/// `buf`).
fn find_program<'b>(
    info: &BootInfo,
    name: &str,
    buf: &'b mut [u8; 512],
) -> Option<(&'static str, &'static [u8], &'b str)> {
    if let Some(module) = info.modules.find(name) {
        return Some((module.file_name(), module.data, join_dir(buf, module.cmdline, "")?));
    }
    for module in info.modules.iter() {
        let Some(dir) = module.cmdline.strip_prefix("tar:") else { continue };
        let mut entries = fs::tar::entries(module.data);
        while let Ok(Some(entry)) = entries.next() {
            // A name with `/` is a path in the tree (`cmd/git.exe`) or
            // below C: (`git/bin/bash.exe`: the tree's directory too).
            let matches = if name.contains('/') {
                let full = name
                    .len()
                    .checked_sub(entry.path.len() + 1)
                    .filter(|&n| name.is_char_boundary(n))
                    .is_some_and(|n| {
                        name[..n].eq_ignore_ascii_case(dir)
                            && name.as_bytes()[n] == b'/'
                            && name[n + 1..].eq_ignore_ascii_case(entry.path)
                    });
                full || entry.path.eq_ignore_ascii_case(name)
            } else {
                entry.name.eq_ignore_ascii_case(name)
            };
            if !entry.directory && matches {
                let sub = entry.path.rsplit_once('/').map_or("", |(sub, _)| sub);
                let (file, data) = (entry.name, entry.data);
                return Some((file, data, join_dir(buf, dir, sub)?));
            }
        }
    }
    None
}

/// Runs the boot module `name` as a user process with command-line
/// arguments `args` and waits for it.
/// Waits until no user process is running (`wait=all`).
fn wait_for_the_rest() {
    loop {
        let Ok(running) = nt::process::running() else {
            error!("run: out of memory");
            return;
        };
        let Some(next) = running.first().cloned() else { return };
        drop(running);
        info!("run: waiting for {} (pid {})", next.name, next.id);
        let Ok(object) = nt::object::KObject::new(nt::object::ObjectBody::Process(next.clone()))
        else {
            error!("run: out of memory");
            return;
        };
        nt::wait::wait_for(core::slice::from_ref(&object), false, nt::wait::Timeout::Infinite);
        match next.exit_status() {
            Some(status) => info!("run: {} finished with status {:#010x}", next.name, status.0),
            None => warn!("run: {} ended without an exit status", next.name),
        }
    }
}

/// `console=<program>`: that program (a `start=` or `run=` module name)
/// gets a console window: a console session, hosted by gala's conhost.exe
/// (started here with the session's id and the program's name).
fn console_window(
    info: &BootInfo,
    name: &str,
    wanted: Option<&str>,
) -> Option<mm::heap::KArc<nt::conhost::Session>> {
    // The module's name or its file name (`dir/x.exe` or `x.exe`).
    let file = name.rsplit('/').next().unwrap_or(name);
    if !wanted.is_some_and(|w| w.eq_ignore_ascii_case(name) || w.eq_ignore_ascii_case(file)) {
        return None;
    }
    if nt::desktop::server_profile() {
        info!("console: no console window for {file} (server profile)");
        return None;
    }
    let session = match nt::conhost::create() {
        Ok(s) => s,
        Err(e) => {
            error!("console: no session: {e:?}");
            return None;
        }
    };
    let mut args = [0u8; 160];
    let mut w = Cursor { buf: &mut args, at: 0 };
    let _ = core::fmt::write(&mut w, format_args!("{} {}", session.id, file));
    let at = w.at;
    let text = core::str::from_utf8(&args[..at]).unwrap_or("");
    start_program(info, "conhost.exe", text, None);
    Some(session)
}

/// A fixed buffer to format into.
struct Cursor<'a> {
    buf: &'a mut [u8],
    at: usize,
}

impl core::fmt::Write for Cursor<'_> {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let end = self.at + s.len();
        let dst = self.buf.get_mut(self.at..end).ok_or(core::fmt::Error)?;
        dst.copy_from_slice(s.as_bytes());
        self.at = end;
        Ok(())
    }
}

/// Starts the boot module `name` as a user process (C:\gala as its
/// current directory) without waiting for it.
fn start_program(
    info: &BootInfo,
    name: &str,
    args: &str,
    console: Option<mm::heap::KArc<nt::conhost::Session>>,
) {
    let mut buf = [0u8; 512];
    let Some((file_name, data, directory)) = find_program(info, name, &mut buf) else {
        panic!("start: no boot module named {name:?}");
    };
    match nt::process::spawn_from_pe(file_name, directory, data, args, None, console) {
        Ok((process, _thread)) => info!("start: {} (pid {})", process.name, process.id),
        Err(e) => error!("start: cannot start {name}: {e:?}"),
    }
}

fn run_program(
    info: &BootInfo,
    name: &str,
    args: &str,
    cwd: Option<&str>,
    wait_all: bool,
    console: Option<mm::heap::KArc<nt::conhost::Session>>,
) {
    let mut buf = [0u8; 512];
    let Some((file_name, data, directory)) = find_program(info, name, &mut buf) else {
        panic!("run: no boot module named {name:?}");
    };
    info!("run: {directory}/{file_name} ({} bytes)", data.len());
    let (free_before, _) = mm::frame_stats();
    let space = match nt::process::spawn_from_pe(file_name, directory, data, args, cwd, console) {
        Ok((process, main_thread)) => {
            drop(main_thread);
            // Wait for the whole process (all threads) to end.
            match nt::object::KObject::new(nt::object::ObjectBody::Process(process.clone())) {
                Ok(object) => {
                    let one = core::slice::from_ref(&object);
                    // A minute without finishing: log every thread's
                    // state (what a hang is waiting for), then wait on.
                    if !wait_real_secs(one, 60) {
                        warn!("run: {} still running after a minute; threads:", process.name);
                        nt::thread::dump();
                        sched::dump();
                        // What it has missed so far (the ledger otherwise
                        // prints when the run ends, which a hang never
                        // does).
                        nt::ledger::dump();
                    }
                    // Then what it newly missed every 30 seconds (e.g.
                    // after a screen check's input).
                    while !wait_real_secs(one, 30) {
                        info!("run: {} still running; the ledger so far:", process.name);
                        nt::ledger::dump();
                    }
                }
                Err(_) => error!("run: out of memory"),
            }
            match process.exit_status() {
                Some(status) => {
                    info!("run: {} finished with status {:#010x}", process.name, status.0)
                }
                None => warn!("run: {} ended without an exit status", process.name),
            }
            if wait_all {
                wait_for_the_rest();
            }
            process.space.clone()
        }
        Err(e) => {
            error!("run: cannot start {name}: {e:?}");
            return;
        }
    };
    // Teardown check: once the exiting threads are reaped, ours must be the
    // last reference to the address space; then its memory is returned.
    let deadline = time::raw_ticks() + time::TICK_HZ;
    while mm::heap::KArc::strong_count(&space) > 1 && time::raw_ticks() < deadline {
        sched::reap();
        // Exited processes nothing refers to any more (kept for OpenProcess).
        nt::process::prune();
        sched::sleep_ms(1);
    }
    if mm::heap::KArc::strong_count(&space) > 1 {
        warn!(
            "run: address space still referenced {} times after exit",
            mm::heap::KArc::strong_count(&space) - 1
        );
    }
    drop(space);
    let (free_after, _) = mm::frame_stats();
    // Heap slab pages are kept by design, so a few frames remain in use.
    info!("run: {} frames not returned", free_before.saturating_sub(free_after));
}
