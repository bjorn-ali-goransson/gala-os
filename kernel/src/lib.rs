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
pub mod clockcal;
pub mod clockctl;
pub mod cmdline;
#[macro_use]
pub mod console;
pub mod crash;
pub mod disk;
pub mod display;
pub mod events;
pub mod failpoint;
pub mod flight;
pub mod fs;
pub mod handover;
/// gala-um's platform: the NT layer inside a Linux process (a Linux x86_64
/// host's: its signal contexts are x86-64's).
#[cfg(feature = "hosted")]
#[allow(unsafe_code)]
pub mod hosted;
#[cfg(all(feature = "hosted", not(target_arch = "x86_64")))]
compile_error!("gala-um (the `hosted` feature) runs on Linux x86_64 hosts only");
#[cfg(not(feature = "hosted"))]
pub mod input;
/// The Linux personality's groundwork (ROADMAP LX): not wired in yet.
pub mod lx;
pub mod mm;
pub mod net;
pub mod nt;
pub mod personality;
#[cfg(not(feature = "hosted"))]
pub mod platform;
pub mod quota;
pub mod replay;
pub mod runtime;
pub mod sched;
mod selftest;
/// Deterministic simulation's scheduler and clock (gala-um `--sim`).
#[cfg(any(test, feature = "hosted"))]
pub mod sim;
pub mod statedump;
pub mod sync;
pub mod time;
pub mod trust;
pub mod unix;
pub mod wayland;

use core::sync::atomic::{AtomicBool, Ordering};

use arch::interrupts::Fault;
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

/// The kernel's start: from Limine (`handover` is `None`), or from the
/// previous kernel after a kernel handover (ROADMAP R9).
pub fn kernel_main(handover: Option<boot::HandoverEntry>) -> ! {
    arch::bootphase::mark("entry");
    console::init();
    kprintln!();
    kprintln!("gala-os kernel {}", env!("CARGO_PKG_VERSION"));

    let mut info = match boot::collect(handover) {
        Ok(info) => info,
        Err(e) => panic!("bootloader handoff rejected: {e}"),
    };
    arch::bootphase::mark("boot_info");
    let cmdline = Cmdline::new(info.cmdline);
    if cmdline.flag(QEMU_EXIT_FLAG) {
        QEMU_EXIT.store(true, Ordering::Relaxed);
    }

    arch::init_boot_cpu();
    // Lock-order checking (debug builds) needs to know the CPU: from here.
    sync::lockdep::enable();
    arch::init_boot_traps();

    log_boot_info(&info);
    handover::report(&info);
    let summary = log_memory_map(&info);
    arch::bootphase::mark("memory_map");
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
    arch::bootphase::mark("frames");
    let (free, total) = mm::frame_stats();
    info!(
        "frames: {} free of {} tracked ({} MiB free)",
        free,
        total,
        free * mm::PAGE_SIZE / (1024 * 1024)
    );
    init_paging(&info);
    arch::bootphase::mark("paging");
    init_timer(&info, cmdline.flag("calstall"), cmdline.flag("calwrong"));
    arch::bootphase::mark(arch::bootphase::TIMER);
    // A state dump on an NMI (xtask asks for one when a boot test times out).
    statedump::init();
    handover::check_marker(&info);
    if let Err(e) = sched::init() {
        panic!("scheduler: {e:?}");
    }
    arch::bootphase::mark("sched");
    // The frames' owner counts (a fork's copy-on-write pages), from the heap.
    mm::init_sharing();
    info!("sched: boot thread {} running, idle thread started", sched::current());
    arch::smp::init_bsp();
    arch::bootphase::mark("smp_start");
    start_cpus(&info, &cmdline);
    arch::bootphase::mark("smp_done");
    disk::init();
    // `display=virtio`: the screen is virtio-gpu (default `fb`: the
    // bootloader's framebuffer); `gputrace` logs every command and
    // `gpureset=<n>` resets the device after the n-th flush (recovery).
    if let Some(n) = cmdline.value("gpureset").and_then(|v| v.parse().ok()) {
        display::RESET_AT.store(n, Ordering::Relaxed);
    }
    display::init(display::Choice::parse(cmdline.value("display")), cmdline.flag("gputrace"));
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
    arch::bootphase::mark("drivers");
    // R8: the signed manifest, checked before anything else is loaded.
    trust::boot(&info);
    arch::bootphase::mark("manifest");
    if handover::wanted(&cmdline)
        && let Some(file) = handover::next_image(&info, &cmdline)
    {
        trust::prepare(file);
    }
    for module in info.modules.iter() {
        if module.cmdline == trust::MODULE_STRING {
            continue;
        }
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
        handover::check_volume(&info);
        // R8 step 6: a release below the disk's rollback floor does not run.
        if let Err(e) = trust::check_rollback(nt::persist::floor().map(|f| f.floor)) {
            error!("rollback: refused: {e}; this release must not run on this disk");
            stop(QemuExitCode::Failure);
        }
    }
    // C: is made: what is older is made again by the next kernel's boot (a
    // kernel handover reopens the programs' files).
    nt::file::mark_boot_done();
    // Programs the previous kernel handed over go on (ROADMAP R9 (b)).
    handover::resume_programs(&info);
    // `handover=<n>`: programs may ask for a handover (GalaRequestHandover).
    let handover_requests = if handover::wanted(&cmdline) {
        match nt::syscalls::allow_handover_requests() {
            Ok(event) => Some(event),
            Err(e) => {
                error!("handover: cannot take requests: {e:?}");
                None
            }
        }
    } else {
        None
    };

    // The desktop's wallpaper (`wallpaper=<name>`, else one at random).
    nt::desktop::choose_wallpaper(cmdline.value("wallpaper"));
    // `presenter=test|none` (ROADMAP RS B3a): a headless desktop, the
    // console session's shown by a frame in memory or by nothing.
    if let Some(name) = cmdline.value("presenter") {
        if nt::desktop::set_local_presenter(name) {
            info!("desktop: the console session's desktop is shown by {name} (presenter=)");
        } else {
            warn!("desktop: presenter={name} is not a presenter (test, none); ignored");
        }
    }
    // `profile=server` (ROADMAP SD): no graphical interface, ever.
    if cmdline.value("profile") == Some("server") {
        nt::desktop::set_server_profile();
        info!("profile: server (no desktop, no console windows)");
    }
    nt::syscalls::set_trace(cmdline.flag("trace"));
    // `conpty=trace`: pseudo consoles log their VT traffic (nt::conhost).
    nt::conhost::set_tracing(cmdline.value("conpty") == Some("trace"));
    // `logtee=C:/dir` (percent-encoded): writes to files below it are
    // logged too (nt::logtee).
    if let Some(dir) = cmdline.value("logtee") {
        let mut decoded = alloc::vec::Vec::new();
        if cmdline::percent_decode(dir, &mut decoded).is_ok() {
            nt::logtee::configure(core::str::from_utf8(&decoded).unwrap_or(""));
        }
    }
    sched::CHECK_USER_RETURN.store(cmdline.flag("checkuser"), Ordering::Relaxed);
    arch::smp::set_tlb_paranoid(cmdline.flag("tlbparanoid"));
    sched::NO_MIGRATE.store(cmdline.flag("nomigrate"), Ordering::Relaxed);
    mm::FRAME_CHECK.store(cmdline.flag("framecheck"), Ordering::Relaxed);
    nt::vm::EAGER_COMMIT.store(cmdline.flag("eagercommit"), Ordering::Relaxed);
    // The watchdog's test: CPU 1 stops taking timer interrupts for a while.
    time::set_hang_test(cmdline.flag("hangtest"));
    time::set_sample_probe(cmdline.flag("sampleprobe"));
    if cmdline.flag("parkdelay") {
        arch::handover::delay_next_park();
        info!("handover: parkdelay: the next park IPI is taken 3 s late (test)");
    }
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
    if let Some(pids) = cmdline.value("events_pid") {
        events::set_syscall_pids(pids);
    }
    if let Some(text) = cmdline.value("events_match") {
        events::set_syscall_match(text);
    }
    if let Some(hz) = cmdline.value("sample_hz").and_then(|v| v.parse::<u64>().ok()) {
        time::set_sample_hz(hz);
    }
    time::emit_calibration_event();
    // The TSC checked against the RTC in the background (recalibrated if
    // they disagree); under `calwrong` and `calstall` (the clock's tests)
    // the programs wait for it.
    time::start_clock_check(cmdline.flag("calwrong") || cmdline.flag("calstall"));
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
    // `quota=paged:<bytes>,nonpaged:<bytes>,handles:<n>`: every process's
    // pool and handle limits (nt::quota; tests give small ones).
    if let Some(spec) = cmdline.value("quota") {
        match nt::quota::configure(spec) {
            Ok(()) => info!("quota: {spec}"),
            Err(item) => error!("quota: {spec:?}: cannot use {item:?}"),
        }
    }
    // `record`: what the programs read from outside goes to the debug
    // console; `replay=<boot module>`: they read it from that recording
    // (kernel/src/replay.rs).
    start_record_replay(&info, &cmdline);
    // `wayland=on` (or `trace`: every request and event logged): the
    // Wayland server for Linux desktop programs, listening on
    // /run/user/<uid>/wayland-0 before any program starts (ROADMAP LD).
    if let Some(mode) = cmdline.value("wayland")
        && let Err(e) = wayland::start(mode)
    {
        error!("wayland: cannot start the server: {e:?}");
    }
    // `start=a.exe,b.exe` (with `start_args=`) starts programs first and
    // does not wait for them: a server the `run=` programs talk to. They
    // end with the boot. `start_as=system`: they run as SYSTEM (services,
    // in session 0) while the `run=` programs stay the boot user's.
    // `start_wait=a.exe,...`: those are waited for before the next one
    // starts (`ssh-keygen -A` making sshd's host keys).
    if let Some(names) = cmdline.value("start") {
        let start_system = cmdline.value("start_as") == Some("system");
        if start_system {
            nt::token::set_boot_system(true);
        }
        start_programs(&info, &cmdline, names);
        nt::token::set_boot_system(cmdline.value("as") == Some("system"));
    }
    // `sampleprobe`: before the programs, while nothing waits on this CPU.
    time::sample_probe();
    if let Some(names) = cmdline.value("run") {
        let mut args = cmdline.value("args").unwrap_or("").split(',');
        for name in names.split(',').filter(|n| !n.is_empty()) {
            let mut decoded = alloc::vec::Vec::new();
            info!("run: begin {name}");
            arch::bootphase::mark("program");
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
            // `restart=name[:max]`: a service that is started again when
            // it ends (at most `max` times, default 10; a run of over a
            // minute starts the count again), as the service control
            // manager's recovery actions restart a failed service.
            let restarts = restart_limit(cmdline.value("restart"), name);
            let mut attempt = 0u32;
            loop {
                let started = time::raw_monotonic_ns();
                run_program(
                    &info,
                    &cmdline,
                    name,
                    core::str::from_utf8(&decoded).unwrap_or(""),
                    cwd,
                    wait_all,
                    console.clone(),
                    handover_requests.as_ref(),
                );
                let Some(max) = restarts else { break };
                if time::raw_monotonic_ns().saturating_sub(started) > 60_000_000_000 {
                    attempt = 0;
                }
                if attempt >= max {
                    warn!("service: {name} ended {attempt} times; not restarted again");
                    break;
                }
                attempt += 1;
                info!("service: {name} ended; restarting it ({attempt} of {max})");
                events::emit(
                    events::Category::Process,
                    "service restart",
                    format_args!(r#""name":{},"attempt":{attempt}"#, events::Str(name)),
                );
                sched::sleep_ms(1000);
            }
            // What the program's end changed (delete-on-close at exit).
            nt::persist::sync();
            nt::ledger::dump();
            info!("run: end {name}");
        }
    }
    // `lxrun=/lx/hello_test`: Linux programs, after the `run=` ones
    // (`run_linux_programs`).
    if let Some(paths) = cmdline.value("lxrun") {
        run_linux_programs(&info, &cmdline, paths, wait_all, handover_requests.as_ref());
    }
    // `dump=C:/hs_err_pid4.log,...` (percent-encoded): files the programs
    // left in the ramfs, printed to the log once they have ended (a
    // crashing JVM's error report would otherwise be lost with the VM).
    if let Some(paths) = cmdline.value("dump") {
        for path in paths.split(',').filter(|p| !p.is_empty()) {
            dump_file(path);
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
                let provoke = |i: usize| arch::interrupts::provoke(Fault::ALL[i]);
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
        use core::sync::atomic::Ordering::Relaxed;
        info!(
            "events: {} written to the debug console; profiler samples: {} asked of another CPU, {} in user mode, {} in the kernel, {} lost (interrupts off too long)",
            events::written(),
            time::SAMPLES_ASKED.load(Relaxed),
            time::SAMPLES_USER.load(Relaxed),
            time::SAMPLES_KERNEL.load(Relaxed),
            time::SAMPLES_LOST.load(Relaxed)
        );
    }
    net::log_stats();
    display::log_stats();
    #[cfg(not(feature = "hosted"))]
    input::log_stats();
    info!("nt: {}", nt::stats::Summary);
    info!("quota: {}", nt::quota::peaks());
    let (shootdowns, ipis) = arch::smp::flush_stats();
    let heap = mm::heap::stats();
    let window_flushes = mm::vmap::WINDOW.lock().flushes();
    info!(
        "stats: {shootdowns} TLB shootdowns ({ipis} IPIs, {} after a move to another CPU), \
         {} frames leaked by deferred frees, {} freed twice; \
         heap {} KiB in use, {} large pages, {window_flushes} mapped-window flushes; \
         frames free {} of {}; heap peak {} KiB, fewest frames free {}; \
         {} frames shared copy-on-write, {} copied at a write",
        arch::smp::moved_flushes(),
        arch::paging::leaked_frames(),
        mm::double_frees(),
        heap.in_use / 1024,
        heap.large_pages,
        mm::frame_stats().0,
        mm::frame_stats().1,
        heap.peak_in_use / 1024,
        mm::frame_low_water(),
        mm::sharing_stats().0,
        mm::sharing_stats().1
    );
    // R8 step 6: this release has booted and done its work; the disk's
    // rollback floor goes up to what its manifest says.
    raise_rollback_floor();
    // `handover=<n>`: hand the machine over to a fresh copy of the kernel
    // until n handovers are done (ROADMAP R9). A refusal (versions, an
    // image not in the signed manifest) leaves this kernel running.
    if handover::wanted(&cmdline) {
        match handover::go(&info, &cmdline) {
            handover::HandoverError::Incompatible(why) => warn!("handover: refused: {why}"),
            handover::HandoverError::Refused(why) => warn!("handover: refused: {why}"),
            e => panic!("handover: failed: {e:?}"),
        }
    }
    arch::bootphase::mark("boot_ok");
    kprintln!("{BOOT_OK_MARKER}");
    stop(QemuExitCode::Success)
}

/// The end of the boot thread's work: QEMU exits with `code` (`qemu_exit`),
/// else the boot thread ends and its CPU runs its idle thread. (A boot
/// thread that halted in a loop instead stayed "in the kernel without
/// blocking" forever: the flight recorder's watchdog reported it every
/// 20 s and its reports pushed every real event out of the ring.)
fn stop(code: QemuExitCode) -> ! {
    if QEMU_EXIT.load(Ordering::Relaxed) {
        qemu::exit(code);
    }
    info!("boot: the boot thread's work is done; it ends");
    sched::exit()
}

/// Raises the disk's rollback floor to the verified manifest's (R8).
fn raise_rollback_floor() {
    let (Some(want), Some(before)) = (trust::floor_for_disk(), nt::persist::floor()) else {
        return;
    };
    match nt::persist::raise_floor(want) {
        Some(Ok(now)) if now != before => info!(
            "rollback: the disk's floor is release {} (was {}), highest release seen {}",
            now.floor, before.floor, now.highest
        ),
        Some(Err(e)) => warn!("rollback: cannot raise the disk's floor: {e:?}"),
        _ => {}
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
/// the index of the one signaled. Under clock control the programs' clock
/// may be frozen or jump, so the wait looks at the real clock every second
/// of the programs' time (with no clock control: one wait).
fn wait_real_secs_any(objects: &[nt::object::ObjectRef], secs: u64) -> Option<usize> {
    use nt::wait::Timeout;
    let until = time::raw_ticks() + secs * time::TICK_HZ;
    loop {
        // A second at a time: a state dump asked for by an NMI (a boot
        // test timing out) is done from here, where locks may be taken.
        let step = time::TICK_HZ.min(secs * time::TICK_HZ);
        let timeout = Timeout::Tick(time::ticks() + step);
        let status = nt::wait::wait_for(objects, false, timeout);
        if status != nt::status::NtStatus::TIMEOUT {
            return (status.0 < objects.len() as u32).then_some(status.0 as usize).or(Some(0));
        }
        if statedump::take_request() {
            statedump::full_dump();
        }
        if time::raw_ticks() >= until {
            return None;
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
    if info.handover.is_none() {
        info!("limine base revision: {}", info.base_revision);
    }
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
    arch::bootphase::panic();
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
    // Memory at the time (if those locks are free: the panic may be
    // inside the allocator).
    statedump::stats_line(mm::heap::try_stats(), mm::try_frame_stats());
    // Which thread each CPU ran (if the scheduler's lock is free).
    // (Only called once the scheduler exists, so the per-CPU data does.)
    let listed = sched::try_cpu_threads(|cpu, id, name, state| {
        let me = arch::percpu::cpu_index() as usize;
        let this = if cpu == me { " (the panicking CPU)" } else { "" };
        console::emergency_line(format_args!(
            "panic: cpu {cpu} runs thread {id} ({name}) {state}{this}"
        ));
    });
    if !listed {
        console::emergency_line(format_args!(
            "panic: the scheduler's lock is held (threads unknown)"
        ));
    }
    // Kept on the disk for the next boot (when a volume is mounted).
    crash::save(info, &Backtrace);
    finish(QemuExitCode::Failure)
}

/// Starts the other CPUs and says what the bootloader, the firmware and
/// the kernel each made of them (ARCHITECTURE "Processors").
fn start_cpus(info: &BootInfo, cmdline: &Cmdline) {
    use arch::smp::SmpTest;
    log_processors(info);
    let test = match cmdline.value("smptest") {
        Some("hide") => SmpTest::Hide,
        Some("late") => SmpTest::Late,
        _ => SmpTest::None,
    };
    let t0 = time::raw_monotonic_ns();
    let report = match arch::smp::start_aps(info, test, sched::run_ap) {
        Ok(report) => report,
        Err(e) => {
            warn!("smp: cannot start the other CPUs: {e:?}");
            return;
        }
    };
    let ms = (time::raw_monotonic_ns() - t0) / 1_000_000;
    if let Some(what) = report.late_replay {
        info!("smptest: an AP made to answer the bootloader late: {what}");
    }
    let n = report.online;
    match report.firmware {
        Some(fw) => info!(
            "smp: {n} CPUs online ({} reported by the bootloader, {fw} in the MADT; {ms} ms)",
            report.reported
        ),
        None => {
            info!("smp: {n} CPUs online ({} reported by the bootloader; {ms} ms)", report.reported)
        }
    }
    if report.started_here > 0 {
        warn!(
            "smp: the bootloader did not start {} of the MADT's CPUs; the kernel started {} itself",
            report.started_here + report.failed,
            report.started_here
        );
    }
    if let Some(id) = report.first_failed {
        error!(
            "smp: {} CPU(s) of the MADT's did not start (first: APIC id {id}); running on {n}",
            report.failed
        );
    }
    handover::report_cpus(info, n);
}

/// The bootloader's MP response and the firmware's MADT, side by side.
fn log_processors(info: &BootInfo) {
    use core::fmt::Write;
    let mut line = heapless_line();
    let _ = write!(
        line,
        "smp: {}: {} CPU(s), BSP APIC id {}:",
        if info.handover.is_some() { "parked by the previous kernel" } else { "bootloader" },
        info.processors.len(),
        info.processors.bsp_lapic_id
    );
    for p in info.processors.iter() {
        let _ = write!(line, " {}", p.lapic_id);
    }
    info!("{}", line.as_str());
    match arch::smp::shelter_state() {
        Some(arch::smp::Shelter::Added) => {
            info!("smp: low 4 GiB identity-mapped in the bootloader's tables (for late APs)")
        }
        Some(arch::smp::Shelter::AlreadyMapped) => {}
        Some(arch::smp::Shelter::NoTrampoline) => {
            info!("smp: no bootloader AP trampoline in memory (no AP was sent there)")
        }
        Some(s) => warn!(
            "smp: bootloader's AP tables not changed ({s:?}): a late AP would reset the machine"
        ),
        None => {}
    }
    match &info.firmware_cpus {
        None => {}
        Some(Err(e)) => warn!("smp: MADT not read: {e:?}"),
        Some(Ok(fw)) => {
            let mut line = heapless_line();
            let _ = write!(line, "smp: MADT: {} enabled CPU(s):", fw.len());
            for id in fw.ids() {
                let _ = write!(line, " {id}");
            }
            let missing =
                fw.ids().iter().filter(|id| !info.processors.iter().any(|p| p.lapic_id == **id));
            let mut first = true;
            for id in missing {
                let _ = write!(
                    line,
                    "{} {id}",
                    if first { "; not started by the bootloader: APIC id" } else { "," }
                );
                first = false;
            }
            info!("{}", line.as_str());
        }
    }
}

/// A fixed-size line buffer (no allocation).
struct Line {
    buf: [u8; 256],
    len: usize,
}

fn heapless_line() -> Line {
    Line { buf: [0; 256], len: 0 }
}

impl Line {
    fn as_str(&self) -> &str {
        core::str::from_utf8(&self.buf[..self.len]).unwrap_or("")
    }
}

impl core::fmt::Write for Line {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let n = s.len().min(self.buf.len() - self.len);
        self.buf[self.len..self.len + n].copy_from_slice(&s.as_bytes()[..n]);
        self.len += n;
        Ok(())
    }
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

/// `stall` (`calstall`): the calibration's test stalls; `wrong`
/// (`calwrong`): its result made 20x low, for the RTC check to catch.
fn init_timer(info: &BootInfo, stall: bool, wrong: bool) {
    let carried = {
        let (mut counter, mut timer) = (0, 0);
        handover::carried_rates(info, &mut counter, &mut timer);
        (counter != 0 && timer != 0).then_some((counter, timer))
    };
    let mut calibration = arch::timer::calibrate(info, stall, carried);
    time::log_calibration(&calibration);
    if stall {
        info!("clockcal: stall injected ({} stalls inside the calibration)", calibration.stalls());
    }
    if wrong {
        calibration.set_rates(calibration.counter_hz() / 20, calibration.timer_hz() / 20);
        warn!("clockcal: calibration made 20x low (calwrong): guest time runs 20x fast");
    }
    // After a handover: the first boot's rates (one clock across kernels).
    let (mut counter, mut timer) = (calibration.counter_hz(), calibration.timer_hz());
    handover::carried_rates(info, &mut counter, &mut timer);
    calibration.set_rates(counter, timer);
    calibration.log_started();
    time::init(&calibration);
    if info.handover.is_some() {
        // The clock goes on from the previous kernel's.
        handover::resume_clock(info);
    } else if let Some(t) = info.boot_unix_time {
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
        // In steps (a state dump may be asked for meanwhile).
        while wait_real_secs_any(core::slice::from_ref(&object), 3600).is_none() {}
        match next.exit_status() {
            Some(status) => info!("run: {} finished with status {:#010x}", next.name, status.0),
            None => warn!("run: {} ended without an exit status", next.name),
        }
    }
}

/// How many times `restart=a.exe[:max],b.exe...` restarts program `name`
/// (None: it is not listed; the default limit is 10).
fn restart_limit(value: Option<&str>, name: &str) -> Option<u32> {
    value?.split(',').find_map(|item| {
        let (n, max) = match item.split_once(':') {
            Some((n, max)) => (n, max.parse().ok()?),
            None => (item, 10),
        };
        n.eq_ignore_ascii_case(name).then_some(max)
    })
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

/// The `start=` programs (`names`, with `start_args=`), one after another
/// without waiting for them, except those `start_wait=` names: each of
/// those ends (its status logged) before the next one starts.
fn start_programs(info: &BootInfo, cmdline: &Cmdline, names: &str) {
    let mut args = cmdline.value("start_args").unwrap_or("").split(',');
    let waited = cmdline.value("start_wait").unwrap_or("");
    for name in names.split(',').filter(|n| !n.is_empty()) {
        let mut decoded = alloc::vec::Vec::new();
        if cmdline::percent_decode(args.next().unwrap_or(""), &mut decoded).is_err() {
            error!("start: out of memory for {name}'s arguments");
            continue;
        }
        if let Some(p) = handover::carry::resumed(name) {
            info!("start: {name} goes on from the previous kernel (pid {})", p.id);
            continue;
        }
        let console = console_window(info, name, cmdline.value("console"));
        let started =
            start_program(info, name, core::str::from_utf8(&decoded).unwrap_or(""), console);
        if let Some(process) = started
            && waited.split(',').any(|w| w.eq_ignore_ascii_case(name))
        {
            await_started(&process);
        }
    }
}

/// Waits for the `start_wait=` program `process` to end (logging every
/// minute it has not) and logs its status.
fn await_started(process: &mm::heap::KArc<nt::process::Process>) {
    let object = match nt::object::KObject::new(nt::object::ObjectBody::Process(process.clone())) {
        Ok(object) => object,
        Err(_) => {
            error!("start: out of memory waiting for {}; not waited for", process.name);
            return;
        }
    };
    let mut objects = alloc::vec::Vec::new();
    if mm::heap::TryVec::try_push(&mut objects, object).is_err() {
        error!("start: out of memory waiting for {}; not waited for", process.name);
        return;
    }
    let mut minutes = 0u32;
    while wait_real_secs_any(&objects, 60).is_none() {
        minutes += 1;
        warn!("start: still waiting for {} (pid {}) after {minutes} min", process.name, process.id);
    }
    let status = process.exit_status().map_or(0, |s| s.0);
    info!("start: {} (pid {}) finished with status {status:#010x}", process.name, process.id);
}

/// Starts the boot module `name` as a user process (C:\gala as its
/// current directory) without waiting for it.
fn start_program(
    info: &BootInfo,
    name: &str,
    args: &str,
    console: Option<mm::heap::KArc<nt::conhost::Session>>,
) -> Option<mm::heap::KArc<nt::process::Process>> {
    let mut buf = [0u8; 512];
    let Some((file_name, data, directory)) = find_program(info, name, &mut buf) else {
        panic!("start: no boot module named {name:?}");
    };
    match nt::process::spawn_from_pe(file_name, directory, data, args, None, console) {
        Ok((process, _thread)) => {
            info!(
                "start: {} (pid {}, as {})",
                process.name,
                process.id,
                if nt::token::boot_system() { "SYSTEM" } else { "the boot user" }
            );
            Some(process)
        }
        Err(e) => {
            error!("start: cannot start {name}: {e:?}");
            None
        }
    }
}

/// Prints file `path` (`C:/dir/name`, percent-encoded) to the log, one
/// `dump:` line per line of text; a missing file is one line saying so.
fn dump_file(path: &str) {
    let mut decoded = alloc::vec::Vec::new();
    if cmdline::percent_decode(path, &mut decoded).is_err() {
        error!("dump: out of memory");
        return;
    }
    let text = core::str::from_utf8(&decoded).unwrap_or("");
    let rest = text.strip_prefix("C:").or_else(|| text.strip_prefix("c:")).unwrap_or(text);
    // The volume's separators (an ASCII byte for an ASCII byte: the text
    // stays UTF-8), copied into room reserved first.
    use crate::mm::heap::TryVec;
    let mut bytes = alloc::vec::Vec::new();
    if bytes.try_extend_from_slice(rest.as_bytes()).is_err() {
        error!("dump: out of memory");
        return;
    }
    for b in bytes.iter_mut().filter(|b| **b == b'/') {
        *b = b'\\';
    }
    let Ok(volume_path) = alloc::string::String::from_utf8(bytes) else { return };
    let data = match nt::file::lookup(&volume_path).and_then(nt::file::read_all) {
        Ok(data) => data,
        Err(e) => {
            info!("dump: {text}: none ({e:?})");
            return;
        }
    };
    info!("dump: begin {text} ({} bytes)", data.len());
    for line in data.split(|&b| b == b'\n') {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        info!("dump: {}", Lossy(line));
    }
    info!("dump: end {text}");
}

/// Text that may not be UTF-8, shown with U+FFFD for bad bytes.
struct Lossy<'a>(&'a [u8]);

impl core::fmt::Display for Lossy<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        for chunk in self.0.utf8_chunks() {
            f.write_str(chunk.valid())?;
            if !chunk.invalid().is_empty() {
                f.write_str("\u{fffd}")?;
            }
        }
        Ok(())
    }
}

#[allow(clippy::too_many_arguments)]
fn run_program(
    info: &BootInfo,
    cmdline: &Cmdline,
    name: &str,
    args: &str,
    cwd: Option<&str>,
    wait_all: bool,
    console: Option<mm::heap::KArc<nt::conhost::Session>>,
    handover_requests: Option<&nt::object::ObjectRef>,
) {
    let mut buf = [0u8; 512];
    let Some((file_name, data, directory)) = find_program(info, name, &mut buf) else {
        panic!("run: no boot module named {name:?}");
    };
    let (free_before, _) = mm::frame_stats();
    // A program the previous kernel handed over goes on (not again).
    let started = match handover::carry::resumed(file_name) {
        Some(process) => {
            info!("run: {file_name} goes on from the previous kernel (pid {})", process.id);
            Ok(process)
        }
        None => {
            info!("run: {directory}/{file_name} ({} bytes)", data.len());
            nt::process::spawn_from_pe(file_name, directory, data, args, cwd, console)
                .map(|(process, _main)| process)
        }
    };
    let process = match started {
        Ok(process) => process,
        Err(e) => {
            error!("run: cannot start {name}: {e:?}");
            return;
        }
    };
    await_program(info, cmdline, process, wait_all, handover_requests, free_before);
}

/// `lxrun=/lx/a,/lx/b`: Linux programs (static ELF; ROADMAP LX), run one
/// after another like `run=`'s. `lxargs=` holds each one's whole argv,
/// argv[0] first (percent-encoded, shell-style words with '..' and ".."
/// quoting, comma-separated per program; default: the path alone);
/// `lxenv=` the environment of all of them (the same encoding; default
/// none).
fn run_linux_programs(
    info: &BootInfo,
    cmdline: &Cmdline,
    paths: &str,
    wait_all: bool,
    handover_requests: Option<&nt::object::ObjectRef>,
) {
    let mut env_text = alloc::vec::Vec::new();
    if cmdline::percent_decode(cmdline.value("lxenv").unwrap_or(""), &mut env_text).is_err() {
        error!("lxrun: out of memory for lxenv");
        return;
    }
    let mut env_words = alloc::vec::Vec::new();
    if cmdline::shell_words(&env_text, &mut env_words).is_err() {
        error!("lxrun: lxenv is not a list of words (an unclosed quote, or out of memory)");
        return;
    }
    let mut args = cmdline.value("lxargs").unwrap_or("").split(',');
    for path in paths.split(',').filter(|p| !p.is_empty()) {
        info!("run: begin {path}");
        arch::bootphase::mark("program");
        let mut text = alloc::vec::Vec::new();
        let mut words = alloc::vec::Vec::new();
        let decoded = cmdline::percent_decode(args.next().unwrap_or(""), &mut text)
            .map_err(cmdline::WordsError::from)
            .and_then(|()| cmdline::shell_words(&text, &mut words));
        if let Err(e) = decoded {
            error!("lxrun: {path}'s lxargs are not a list of words: {e:?}");
            info!("run: end {path}");
            continue;
        }
        let mut argv: alloc::vec::Vec<&[u8]> = alloc::vec::Vec::new();
        let mut envp: alloc::vec::Vec<&[u8]> = alloc::vec::Vec::new();
        let listed = (if words.is_empty() {
            mm::heap::TryVec::try_push(&mut argv, path.as_bytes())
        } else {
            words.iter().try_for_each(|w| mm::heap::TryVec::try_push(&mut argv, w.as_slice()))
        })
        .and_then(|()| {
            env_words.iter().try_for_each(|w| mm::heap::TryVec::try_push(&mut envp, w.as_slice()))
        });
        if listed.is_err() {
            error!("lxrun: out of memory for {path}'s arguments");
            info!("run: end {path}");
            continue;
        }
        let (free_before, _) = mm::frame_stats();
        match lx::exec::spawn(path, &argv, &envp) {
            Ok(process) => {
                await_program(info, cmdline, process, wait_all, handover_requests, free_before)
            }
            Err(e) => error!("run: cannot start {path}: {e}"),
        }
        nt::ledger::dump();
        info!("run: end {path}");
    }
}

/// Waits for `process` (a `run=` or `lxrun=` program) to end, then checks
/// that its address space is freed.
fn await_program(
    info: &BootInfo,
    cmdline: &Cmdline,
    process: mm::heap::KArc<nt::process::Process>,
    wait_all: bool,
    handover_requests: Option<&nt::object::ObjectRef>,
    free_before: u64,
) {
    let space = {
        // Wait for the whole process (all threads) to end.
        match nt::object::KObject::new(nt::object::ObjectBody::Process(process.clone())) {
            Ok(object) => {
                let mut objects = alloc::vec::Vec::new();
                // Without it the wait below would never end (an
                // empty list): try again until memory comes back.
                while mm::heap::TryVec::try_push(&mut objects, object.clone()).is_err() {
                    sched::sleep_ms(1);
                }
                if let Some(r) = handover_requests
                    && mm::heap::TryVec::try_push(&mut objects, r.clone()).is_err()
                {
                    error!("run: out of memory for the handover requests");
                }
                let mut reported = false;
                loop {
                    match wait_real_secs_any(&objects, if reported { 30 } else { 60 }) {
                        Some(0) => break,
                        // The program asked for a kernel handover: it
                        // happens once its threads are all blocked.
                        Some(_) => {
                            let e = handover::go(info, cmdline);
                            match e {
                                handover::HandoverError::Refused(why) => {
                                    warn!("handover: refused: {why}")
                                }
                                handover::HandoverError::Incompatible(why) => {
                                    warn!("handover: refused: {why}")
                                }
                                e => error!("handover: failed: {e:?}"),
                            }
                        }
                        // A minute without finishing: log every
                        // thread's state (what a hang is waiting for),
                        // then wait on.
                        None if !reported => {
                            reported = true;
                            warn!("run: {} still running after a minute; threads:", process.name);
                            nt::thread::dump();
                            sched::dump();
                            // What it has missed so far (the ledger
                            // otherwise prints when the run ends, which
                            // a hang never does).
                            nt::ledger::dump();
                        }
                        // Then what it newly missed every 30 seconds
                        // (e.g. after a screen check's input).
                        None => {
                            info!("run: {} still running; the ledger so far:", process.name);
                            nt::ledger::dump();
                        }
                    }
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
    };
    // Only `space` may hold the address space now.
    drop(process);
    // Teardown check: once the exiting threads are reaped, ours must be the
    // last reference to the address space; then its memory is returned.
    // A leak is a second without progress: the teardown itself takes time
    // (dropping the process frees its sections, ~0.5 s for pwsh under TCG,
    // several times that on a loaded host), so the deadline moves on each
    // reference that goes.
    let count = || mm::heap::KArc::strong_count(&space);
    let mut left = count();
    let mut deadline = time::raw_ticks() + time::TICK_HZ;
    while count() > 1 && time::raw_ticks() < deadline {
        sched::reap();
        // Exited processes nothing refers to any more (kept for OpenProcess).
        nt::process::prune();
        if count() < left {
            left = count();
            deadline = time::raw_ticks() + time::TICK_HZ;
        }
        sched::sleep_ms(1);
    }
    if count() > 1 {
        // Who holds it: threads of the process not reaped yet (each holds
        // the space), the process itself while referenced, and reaps in
        // progress (an idle thread's batch).
        let root = space.lock().root();
        let threads = sched::dump_space_holders(root);
        info!(
            "run: {threads} threads hold the address space, {} held by reaps in progress; \
             process {}",
            sched::held_by_reaps(),
            nt::process::describe_holders(&space)
        );
        warn!("run: address space still referenced {} times after exit", count() - 1);
    }
    drop(space);
    let (free_after, _) = mm::frame_stats();
    // Heap slab pages are kept by design, so a few frames remain in use.
    info!("run: {} frames not returned", free_before.saturating_sub(free_after));
}
