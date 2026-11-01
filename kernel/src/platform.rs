//! The kernel as the NT layer's platform (`nt::platform`): each trait
//! method forwards to the kernel subsystem that provides it.

use crate::arch::{cpu, interrupts, paging, smp, syscall};
use crate::nt::platform::{
    Clock, DiskError, Frame, FxState, SharedUserSpace, SpawnError, Storage, SyscallFrame, ThreadId,
    Threads, TrapFrame, UserEntry, UserMemory, UserRegs, WakeReason,
};
use crate::{mm, sched, time};

/// The kernel, implementing `nt::platform`'s traits.
pub struct Kernel;

impl Clock for Kernel {
    const TICK_HZ: u64 = time::TICK_HZ;
    const PERFORMANCE_FREQUENCY: u64 = time::PERFORMANCE_FREQUENCY;

    fn ticks() -> u64 {
        time::ticks()
    }
    fn monotonic_ns() -> u64 {
        time::monotonic_ns()
    }
    fn system_time() -> u64 {
        time::system_time()
    }
    fn boot_system_time() -> u64 {
        time::boot_system_time()
    }
    fn set_tick_callback(callback: fn(u64)) {
        time::set_tick_callback(callback)
    }
}

impl Threads for Kernel {
    fn spawn(name: &'static str, entry: fn(usize), arg: usize) -> Result<ThreadId, SpawnError> {
        sched::spawn(name, entry, arg)
    }
    fn spawn_user(
        name: &'static str,
        entry: fn(usize),
        arg: usize,
        space: SharedUserSpace,
        gs_base: u64,
    ) -> Result<ThreadId, SpawnError> {
        sched::spawn_user(name, entry, arg, space, gs_base)
    }
    fn current() -> ThreadId {
        sched::current()
    }
    fn exit() -> ! {
        sched::exit()
    }
    fn yield_now() {
        sched::yield_now()
    }
    fn sleep_ms(ms: u64) {
        sched::sleep_ms(ms)
    }
    fn prepare_wait(deadline: Option<u64>) {
        sched::prepare_wait(deadline)
    }
    fn wait() -> WakeReason {
        sched::wait()
    }
    fn wake(id: ThreadId) {
        sched::wake(id)
    }
    fn interrupt(_id: ThreadId) {
        // Nothing to do: a thread in user mode reaches the hook at its next
        // timer tick (within 1 ms).
    }
    fn set_affinity(id: ThreadId, mask: u64) {
        sched::set_affinity(id, mask)
    }
    fn current_cpu() -> u32 {
        sched::current_cpu()
    }
    fn online_count() -> u32 {
        smp::online_count()
    }
    fn online_mask() -> u64 {
        smp::online_mask()
    }
}

impl UserEntry for Kernel {
    fn set_syscall_handler(handler: fn(&mut SyscallFrame)) {
        syscall::set_handler(handler)
    }
    fn set_user_return_hook(hook: fn(&mut UserRegs) -> bool) {
        syscall::set_user_return_hook(hook)
    }
    fn set_user_exception_handler(handler: fn(&mut TrapFrame, Option<u64>)) {
        interrupts::set_user_exception_handler(handler)
    }
    fn exception_name(vector: u8) -> &'static str {
        interrupts::exception_name(vector)
    }
    fn enter_user(regs: &UserRegs) -> ! {
        syscall::enter_user_regs(regs)
    }
    fn continue_with(regs: UserRegs) {
        syscall::continue_with(regs)
    }
    fn fxsave() -> FxState {
        cpu::fxsave()
    }
    fn fxrstor(state: &FxState) {
        cpu::fxrstor(state)
    }
    fn set_mxcsr(value: u32) {
        cpu::set_mxcsr(value)
    }
}

impl UserMemory for Kernel {
    fn zeroed_frame() -> Option<Frame> {
        paging::UserSpace::zeroed_frame()
    }
    fn frame_with(data: &[u8]) -> Option<Frame> {
        paging::UserSpace::frame_with(data)
    }
    fn free_frame(frame: Frame) {
        let _ = mm::free_frame(frame);
    }
    fn frame_of_heap_page(addr: usize) -> Frame {
        // Heap pages live in the HHDM: the physical address is the offset.
        Frame::containing(addr as u64 - mm::hhdm_offset())
    }
    fn frame_stats() -> (u64, u64) {
        mm::frame_stats()
    }
}

impl Storage for Kernel {
    fn disk_sectors() -> u64 {
        crate::disk::sectors()
    }
    fn disk_read(first: u64, buf: &mut [u8]) -> Result<(), DiskError> {
        crate::disk::read(first, buf)
    }
    fn disk_write(first: u64, data: &[u8]) -> Result<(), DiskError> {
        crate::disk::write(first, data)
    }
    fn disk_flush() -> Result<(), DiskError> {
        crate::disk::flush()
    }
}
