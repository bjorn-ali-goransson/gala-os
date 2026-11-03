//! gala-um's platform (docs/GALA_UM.md): the NT layer running inside a
//! Linux process, built with `--features hosted` and driven by `um/`.
//!
//! `Um` implements `nt::platform`'s traits with host means: memory.rs
//! (a memfd as physical memory), threads.rs (host threads, condition
//! variables, the host clocks) and entry.rs (signals: syscall user
//! dispatch, faults, user-mode entry). The NT layer itself is the same
//! code as in the kernel. sim.rs is the deterministic simulation (`um
//! --sim=<seed>`: one thread at a time, virtual time).
//!
//! Not yet (each reported where it matters): child processes (one NT
//! process per Linux process), suspending a thread while it runs program
//! code (it takes effect at its next system call), file-backed sections
//! (file pages live in the host heap, not in the memfd), networking.

#![allow(clippy::disallowed_methods, clippy::disallowed_macros)]

pub mod entry;
pub mod memory;
pub mod sim;
pub mod threads;

use alloc::string::String;
use alloc::vec::Vec;

use crate::arch::interrupts::TrapFrame;
use crate::arch::syscall::{SyscallFrame, UserRegs};
use crate::arch::xstate::XState;
use crate::mm::frame::Frame;
use crate::nt::platform::{
    Clock, DiskError, Display, SharedUserSpace, Storage, Threads, UserEntry, UserMemory,
};
use crate::sched::{SpawnError, ThreadId, WakeReason};

/// Host code lives at and above this address (PIE binaries at 0x55...,
/// libraries and the host's mmap area up to 0x7fff...); program code must
/// stay below it (see entry.rs).
pub const HOST_START: u64 = 0x5000_0000_0000;

/// Makes the host's address-space layout fixed: if address randomization
/// is on, turns it off for this process (`personality(ADDR_NO_RANDOMIZE)`,
/// as `setarch -R` does) and executes the binary again with the same
/// arguments. Without it the host's libraries may land on gala's fixed
/// addresses (the PEB area at 0x7ff0_0000_0000 is inside the 0x7f... band
/// where ASLR places them); without randomization they sit just below
/// 0x7fff_f7.., far from it. Call first thing in `main`.
pub fn fix_address_space() {
    const ADDR_NO_RANDOMIZE: libc::c_ulong = 0x0040000;
    // SAFETY: personality(0xffffffff) only reads the current persona.
    let current = unsafe { libc::personality(0xffff_ffff) };
    if current < 0 || current as libc::c_ulong & ADDR_NO_RANDOMIZE != 0 {
        return;
    }
    // SAFETY: changes this process's persona; then replaces the process
    // image with itself (argv from /proc/self/cmdline via std).
    unsafe {
        if libc::personality(current as libc::c_ulong | ADDR_NO_RANDOMIZE) < 0 {
            return;
        }
    }
    use std::os::unix::process::CommandExt;
    let mut args = std::env::args_os();
    let Some(_) = args.next() else { return };
    let error = std::process::Command::new("/proc/self/exe").args(args).exec();
    // Still here: run with randomization (collisions are rare).
    crate::warn!("gala-um: cannot re-execute without ASLR: {error}");
}

std::thread_local! {
    /// Whether this thread runs NT-layer code now (see `nt_code`).
    static IN_NT: core::cell::Cell<bool> = const { core::cell::Cell::new(false) };
}

/// Runs `f` as NT-layer code: the `heap` failure point applies to its
/// allocations. gala-um shares the host's allocator with std and with
/// its own platform code, which allocate infallibly (a failure there
/// aborts the process); only the NT layer, which the kernel runs, is
/// under test. Entered by the signal handlers that run NT code, by NT
/// helper threads and by `run`; `host_code` leaves it.
pub fn nt_code<R>(f: impl FnOnce() -> R) -> R {
    let before = IN_NT.with(|c| c.replace(true));
    let result = f();
    IN_NT.with(|c| c.set(before));
    result
}

/// gala-um's own (host) code until dropped: no injected heap failures
/// (std and the platform's bookkeeping allocate infallibly).
pub(crate) struct HostCode(bool);

impl HostCode {
    pub(crate) fn enter() -> Self {
        HostCode(IN_NT.with(|c| c.replace(false)))
    }
}

impl Drop for HostCode {
    fn drop(&mut self) {
        IN_NT.with(|c| c.set(self.0));
    }
}

/// Whether this thread runs NT-layer code (no allocation: a `const`
/// thread-local without a destructor).
pub fn in_nt_code() -> bool {
    IN_NT.with(core::cell::Cell::get)
}

/// `--failtrace`: where an injected failure happened (failpoint::report).
pub fn report_failure(point: &str, n: u64) {
    let _host = HostCode::enter();
    let trace = std::backtrace::Backtrace::force_capture();
    std::eprintln!("failpoint: {point} #{n} failed here:\n{trace}");
}

/// `failpoint=` and `failseed=` values, as on the kernel command line
/// (um's `--failpoint=` and `--failseed=`); armed while the program runs.
pub fn configure_failpoints(spec: &str, seed: Option<&str>) -> Result<(), String> {
    crate::failpoint::configure_from(spec, seed)
        .map_err(|e| alloc::format!("--failpoint={spec}: {e:?}"))
}

/// `clock=` values, as on the kernel command line (um's `--clock=`);
/// armed when the program starts.
pub fn configure_clock(spec: &str) -> Result<(), String> {
    crate::clockctl::configure(spec)
        .map(|_| ())
        .map_err(|item| alloc::format!("--clock={spec}: cannot use {item:?}"))
}

/// The recording's file descriptor (`--record=`).
static RECORD_FD: core::sync::atomic::AtomicI32 = core::sync::atomic::AtomicI32::new(-1);

/// Writes to the recording: write(2) on the file, which is
/// async-signal-safe and takes no host lock (it runs inside system
/// calls, on signal stacks).
fn write_recording(data: &[u8]) {
    let fd = RECORD_FD.load(core::sync::atomic::Ordering::Relaxed);
    let mut rest = data;
    while !rest.is_empty() && fd >= 0 {
        // SAFETY: a valid buffer of that length; fd is the recording.
        let n = unsafe { libc::write(fd, rest.as_ptr().cast(), rest.len()) };
        if n <= 0 {
            return;
        }
        rest = &rest[n as usize..];
    }
}

/// `--record=<file>`: what the run reads from outside goes to `path`
/// (`crate::replay`; the settings with `replay::info`).
pub fn record_to(path: &str) -> Result<(), String> {
    use std::os::fd::IntoRawFd;
    let file = std::fs::File::create(path).map_err(|e| alloc::format!("{path}: {e}"))?;
    RECORD_FD.store(file.into_raw_fd(), core::sync::atomic::Ordering::Relaxed);
    crate::replay::record_to(write_recording);
    Ok(())
}

/// `--replay=<file>`: the run reads what the recording at `path` holds.
pub fn replay_from(path: &str) -> Result<(), String> {
    let data = std::fs::read(path).map_err(|e| alloc::format!("{path}: {e}"))?;
    crate::replay::replay_from(data.leak())
        .map_err(|e| alloc::format!("{path}: not a recording this build reads ({e:?})"))
}

/// gala-um, implementing `nt::platform`'s traits.
pub struct Um;

/// gala-um has no screen.
impl Display for Um {
    fn screen_size() -> Option<(u32, u32)> {
        None
    }
    fn screen_write_row(_: u32, _: u32, _: &[u32]) {}
}

/// Random numbers: the seed's under `--sim`, else the CPU's generator.
impl crate::nt::platform::Entropy for Um {
    fn entropy_seeded() -> bool {
        sim::on()
    }
    fn fill_entropy(buf: &mut [u8]) -> bool {
        if sim::on() {
            sim::fill_random(buf);
            return true;
        }
        for chunk in buf.chunks_mut(8) {
            let Some(word) = crate::arch::cpu::rdrand() else { return false };
            chunk.copy_from_slice(&word.to_le_bytes()[..chunk.len()]);
        }
        true
    }
}

/// gala-um has no disk.
impl Storage for Um {
    fn disk_sectors() -> u64 {
        0
    }
    fn disk_read(_: u64, _: &mut [u8]) -> Result<(), DiskError> {
        Err(DiskError::OutOfRange)
    }
    fn disk_write(_: u64, _: &[u8]) -> Result<(), DiskError> {
        Err(DiskError::OutOfRange)
    }
    fn disk_flush() -> Result<(), DiskError> {
        Err(DiskError::OutOfRange)
    }
}

impl Clock for Um {
    const TICK_HZ: u64 = 1000;
    const PERFORMANCE_FREQUENCY: u64 = crate::time::PERFORMANCE_FREQUENCY;

    fn ticks() -> u64 {
        threads::ticks()
    }
    fn monotonic_ns() -> u64 {
        threads::monotonic_ns()
    }
    fn system_time() -> u64 {
        threads::system_time()
    }
    fn boot_system_time() -> u64 {
        threads::boot_system_time()
    }
    fn set_tick_callback(callback: fn(u64)) {
        threads::start_ticks(callback)
    }
    fn clock_control(op: crate::clockctl::Op) -> crate::clockctl::State {
        threads::clock_control(op)
    }
}

impl Threads for Um {
    fn spawn(name: &'static str, entry: fn(usize), arg: usize) -> Result<ThreadId, SpawnError> {
        threads::spawn(name, entry, arg, false, || {})
    }
    fn spawn_user(
        name: &'static str,
        entry: fn(usize),
        arg: usize,
        space: SharedUserSpace,
        gs_base: u64,
    ) -> Result<ThreadId, SpawnError> {
        threads::spawn(name, entry, arg, true, move || {
            // The thread keeps its address space alive, as in the kernel.
            core::mem::forget(space);
            entry::init_thread();
            entry::set_gs_base(gs_base);
        })
    }
    fn current() -> ThreadId {
        threads::current()
    }
    fn exit() -> ! {
        threads::exit()
    }
    fn yield_now() {
        if sim::on() {
            return sim::yield_now();
        }
        std::thread::yield_now()
    }
    fn sleep_ms(ms: u64) {
        if sim::on() {
            return sim::sleep_ms(ms);
        }
        std::thread::sleep(core::time::Duration::from_millis(ms))
    }
    fn prepare_wait(deadline: Option<u64>) {
        threads::prepare_wait(deadline)
    }
    fn wait() -> WakeReason {
        threads::wait()
    }
    fn wake(id: ThreadId) {
        threads::wake(id)
    }
    fn interrupt(id: ThreadId) {
        threads::signal(id, entry::SIG_INTERRUPT)
    }
    // The host's processors (see threads::cpu_count).
    fn set_affinity(id: ThreadId, mask: u64) {
        threads::set_affinity(id, mask)
    }
    fn current_cpu() -> u32 {
        threads::current_cpu()
    }
    fn online_count() -> u32 {
        threads::cpu_count()
    }
    fn online_mask() -> u64 {
        threads::online_mask()
    }
}

impl UserEntry for Um {
    fn set_syscall_handler(handler: fn(&mut SyscallFrame)) {
        entry::set_syscall_handler(handler)
    }
    fn set_user_return_hook(hook: fn(&mut UserRegs) -> bool) {
        entry::set_user_return_hook(hook)
    }
    fn set_user_exception_handler(handler: fn(&mut TrapFrame, Option<u64>)) {
        entry::set_user_exception_handler(handler)
    }
    fn exception_name(vector: u8) -> &'static str {
        crate::arch::interrupts::exception_name(vector)
    }
    fn enter_user(regs: &UserRegs) -> ! {
        entry::enter_user(regs)
    }
    fn continue_with(regs: UserRegs) {
        entry::continue_with(regs)
    }
    fn xsave() -> XState {
        entry::xsave()
    }
    fn xrstor(state: &XState) {
        entry::xrstor(state)
    }
    fn xstate_enabled() -> u64 {
        entry::xstate_enabled()
    }
    fn set_mxcsr(value: u32) {
        entry::set_mxcsr(value)
    }
}

impl UserMemory for Um {
    fn zeroed_frame() -> Option<Frame> {
        memory::UserSpace::zeroed_frame()
    }
    fn frame_with(data: &[u8]) -> Option<Frame> {
        memory::UserSpace::frame_with(data)
    }
    fn free_frame(frame: Frame) {
        let _ = crate::mm::free_frame(frame);
    }
    fn frame_of_heap_page(addr: usize) -> Frame {
        match memory::frame_of_heap_page(addr) {
            Some(frame) => frame,
            None => {
                // Only if `memory::Allocator` is not the global allocator.
                std::eprintln!("[error] gala-um: heap page {addr:#x} is not a memfd frame");
                std::process::exit(3)
            }
        }
    }
    fn frame_stats() -> (u64, u64) {
        crate::mm::frame_stats()
    }
}

/// Runs the program `name` (its image `data`, placed in C:\gala) with
/// command-line arguments `args` and returns its exit status, as the
/// kernel's `run=` does. `modules` are (directory on C:, name, data), what
/// the kernel gets as boot modules (e.g. ntdll.dll in Windows/System32).
pub fn run(
    name: &str,
    directory: &str,
    data: Vec<u8>,
    args: &str,
    modules: Vec<(String, String, Vec<u8>)>,
) -> Result<u32, String> {
    // Simulation: this thread holds the baton from here on.
    if sim::on() {
        sim::start();
    }
    memory::init()?;
    // As the kernel's boot log (the host's processors, see threads.rs).
    crate::info!("smp: {} CPUs online (host)", threads::cpu_count());
    entry::init();
    crate::nt::init();
    // The TCP stack on its loopback interface (no network card here).
    if let Err(e) = crate::net::tcp::start(None) {
        crate::warn!("net: not started ({e:?})");
    }
    for (directory, file, contents) in modules {
        // `--module=registry/<file>`: a scenario's registry keys, as the
        // kernel's `registry` boot modules.
        if directory == "registry" {
            let text =
                core::str::from_utf8(&contents).map_err(|_| alloc::format!("{file}: not UTF-8"))?;
            crate::nt::registry::load_more(text, crate::nt::platform::system_time())
                .map_err(|e| alloc::format!("loading {file}: {e:?}"))?;
            continue;
        }
        crate::nt::file::add_boot_file(&directory, &file, contents.leak(), false)
            .map_err(|e| alloc::format!("adding {directory}/{file}: {e:?}"))?;
    }
    let name: &'static str = String::from(name).leak();
    let data: &'static [u8] = data.leak();
    // A program that is also one of the modules (`--dir=jdk/bin` with
    // the JDK's tree) is there already.
    match crate::nt::file::add_boot_file(directory, name, data, false) {
        Ok(()) | Err(crate::nt::status::NtStatus::OBJECT_NAME_COLLISION) => {}
        Err(e) => return Err(alloc::format!("adding {name}: {e:?}")),
    }
    // Fault injection while the program runs, as the kernel's `run=`.
    if crate::failpoint::configured() {
        crate::info!("failpoint: armed while {name} runs");
    }
    crate::failpoint::arm();
    // Clock control (`--clock=`) from the program's start on.
    threads::arm_clock_control();
    let started = nt_code(|| {
        let (process, main_thread) =
            crate::nt::process::spawn_from_pe(name, directory, data, args, None, None)
                .map_err(Ok)?;
        drop(main_thread);
        let body = crate::nt::object::ObjectBody::Process(process.clone());
        let object = crate::nt::object::KObject::new(body).map_err(Err)?;
        crate::nt::wait::wait_for(
            core::slice::from_ref(&object),
            false,
            crate::nt::wait::Timeout::Infinite,
        );
        Ok(process)
    });
    crate::failpoint::disarm();
    if crate::failpoint::configured() {
        crate::info!("failpoint: {}", crate::failpoint::Summary);
    }
    let process = started.map_err(|e| match e {
        Ok(load) => alloc::format!("starting {name}: {load:?}"),
        Err(_) => String::from("out of memory"),
    })?;
    // As the kernel after each `run=` program.
    match process.exit_status() {
        Some(status) => {
            crate::info!("run: {} finished with status {:#010x}", process.name, status.0)
        }
        None => crate::warn!("run: {} ended without an exit status", process.name),
    }
    crate::nt::ledger::dump();
    sim::report();
    crate::replay::finish();
    Ok(process.exit_status().map_or(u32::MAX, |s| s.0))
}
