//! gala-um's platform (docs/GALA_UM.md): the NT layer running inside a
//! Linux process, built with `--features hosted` and driven by `um/`.
//!
//! `Um` implements `nt::platform`'s traits with host means: memory.rs
//! (a memfd as physical memory), threads.rs (host threads, condition
//! variables, the host clocks) and entry.rs (signals: syscall user
//! dispatch, faults, user-mode entry). The NT layer itself is the same
//! code as in the kernel.
//!
//! Not yet (each reported where it matters): child processes (one NT
//! process per Linux process), suspending a thread while it runs program
//! code (it takes effect at its next system call), file-backed sections
//! (file pages live in the host heap, not in the memfd), networking.

#![allow(clippy::disallowed_methods, clippy::disallowed_macros)]

pub mod entry;
pub mod memory;
pub mod threads;

use alloc::string::String;
use alloc::vec::Vec;

use crate::arch::cpu::FxState;
use crate::arch::interrupts::TrapFrame;
use crate::arch::syscall::{SyscallFrame, UserRegs};
use crate::mm::frame::Frame;
use crate::nt::platform::{Clock, SharedUserSpace, Threads, UserEntry, UserMemory};
use crate::sched::{SpawnError, ThreadId, WakeReason};

/// Host code lives at and above this address (PIE binaries at 0x55...,
/// libraries and the host's mmap area up to 0x7fff...); program code must
/// stay below it (see entry.rs).
pub const HOST_START: u64 = 0x5000_0000_0000;

/// gala-um, implementing `nt::platform`'s traits.
pub struct Um;

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
}

impl Threads for Um {
    fn spawn(name: &'static str, entry: fn(usize), arg: usize) -> Result<ThreadId, SpawnError> {
        threads::spawn(name, entry, arg, || {})
    }
    fn spawn_user(
        name: &'static str,
        entry: fn(usize),
        arg: usize,
        space: SharedUserSpace,
        gs_base: u64,
    ) -> Result<ThreadId, SpawnError> {
        threads::spawn(name, entry, arg, move || {
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
        std::thread::yield_now()
    }
    fn sleep_ms(ms: u64) {
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
    // One processor, as far as the program can tell: the host schedules.
    fn set_affinity(_id: ThreadId, _mask: u64) {}
    fn current_cpu() -> u32 {
        0
    }
    fn online_count() -> u32 {
        1
    }
    fn online_mask() -> u64 {
        1
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
    fn fxsave() -> FxState {
        entry::fxsave()
    }
    fn fxrstor(state: &FxState) {
        entry::fxrstor(state)
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
    data: Vec<u8>,
    args: &str,
    modules: Vec<(String, String, Vec<u8>)>,
) -> Result<u32, String> {
    memory::init()?;
    entry::init();
    crate::nt::init();
    for (directory, file, contents) in modules {
        crate::nt::file::add_boot_file(&directory, &file, contents.leak())
            .map_err(|e| alloc::format!("adding {directory}/{file}: {e:?}"))?;
    }
    let name: &'static str = String::from(name).leak();
    let data: &'static [u8] = data.leak();
    crate::nt::file::add_boot_file("", name, data)
        .map_err(|e| alloc::format!("adding {name}: {e:?}"))?;
    let (process, main_thread) = crate::nt::process::spawn_from_pe(name, "", data, args)
        .map_err(|e| alloc::format!("starting {name}: {e:?}"))?;
    drop(main_thread);
    let object =
        crate::nt::object::KObject::new(crate::nt::object::ObjectBody::Process(process.clone()))
            .map_err(|_| String::from("out of memory"))?;
    crate::nt::wait::wait_for(
        core::slice::from_ref(&object),
        false,
        crate::nt::wait::Timeout::Infinite,
    );
    // As the kernel after each `run=` program.
    match process.exit_status() {
        Some(status) => {
            crate::info!("run: {} finished with status {:#010x}", process.name, status.0)
        }
        None => crate::warn!("run: {} ended without an exit status", process.name),
    }
    crate::nt::ledger::dump();
    Ok(process.exit_status().map_or(u32::MAX, |s| s.0))
}
