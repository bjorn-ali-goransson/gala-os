//! The seam between the NT layer and what runs it (docs/GALA_UM.md).
//!
//! The NT layer reaches the machine only through this module: five traits
//! say what it needs (time, threads and waits, entering and leaving user
//! mode, user memory, the disk), `P` names the implementation (the kernel today,
//! `crate::platform::Kernel`; gala-um's host process in U2), and the
//! functions below are the NT layer's way to call it. No other file in
//! `nt/` names `crate::arch`, `crate::sched`, `crate::time`, `crate::disk`
//! or the frame allocator (xtask's unit tests check this). With the `hosted` feature,
//! `P` and the address-space types come from `crate::hosted` (gala-um).
//!
//! The data types the platform provides (thread ids, register sets, the
//! address space, page flags, frames) are re-exported here for now; they
//! become trait-associated or NT-owned types when `nt/` leaves the kernel
//! crate (see "Remaining" in docs/GALA_UM.md).

pub use crate::arch::cpu::FxState;
pub use crate::arch::interrupts::TrapFrame;
pub use crate::arch::paging::{PagingError, UserFault};
#[cfg(not(feature = "hosted"))]
pub use crate::arch::paging::{SharedPage, SharedUserSpace, SpaceLock, UserSpace};
pub use crate::arch::syscall::{SyscallFrame, UserRegs};
pub use crate::disk::{DiskError, SECTOR as DISK_SECTOR};
#[cfg(feature = "hosted")]
pub use crate::hosted::memory::{SharedPage, SharedUserSpace, SpaceLock, UserSpace};
pub use crate::mm::PAGE_SIZE;
pub use crate::mm::frame::Frame;
pub use crate::mm::paging::{CacheMode, PageFlags};
pub use crate::sched::{SpawnError, ThreadId, WakeReason};

/// The implementation the NT layer runs on: the kernel, or gala-um's host
/// process (`--features hosted`).
#[cfg(not(feature = "hosted"))]
pub type P = crate::platform::Kernel;
#[cfg(feature = "hosted")]
pub type P = crate::hosted::Um;

/// Time: a periodic tick (timers, the shared page) and two clocks.
pub trait Clock {
    /// Ticks per second.
    const TICK_HZ: u64;
    /// The performance counter's frequency (NtQueryPerformanceCounter).
    const PERFORMANCE_FREQUENCY: u64;
    /// Ticks since start.
    fn ticks() -> u64;
    /// Nanoseconds since start, finer than a tick.
    fn monotonic_ns() -> u64;
    /// Now, in 100 ns units since 1601 (UTC).
    fn system_time() -> u64;
    /// The system time at start.
    fn boot_system_time() -> u64;
    /// Calls `callback(ticks)` on every tick (it must not block).
    fn set_tick_callback(callback: fn(u64));
}

/// Threads and waits. A wait is `prepare_wait` (under the caller's lock,
/// so a `wake` in between is not lost), then `wait`.
pub trait Threads {
    /// A helper thread of the NT layer (no user mode), e.g. timers.
    fn spawn(name: &'static str, entry: fn(usize), arg: usize) -> Result<ThreadId, SpawnError>;
    /// A thread of a process: runs `entry(arg)` in the address space
    /// `space` with the user GS base `gs_base`; `entry` ends by entering
    /// user mode.
    fn spawn_user(
        name: &'static str,
        entry: fn(usize),
        arg: usize,
        space: SharedUserSpace,
        gs_base: u64,
    ) -> Result<ThreadId, SpawnError>;
    fn current() -> ThreadId;
    fn exit() -> !;
    fn yield_now();
    fn sleep_ms(ms: u64);
    /// The next `wait` ends at `deadline` (in ticks) if not woken first.
    fn prepare_wait(deadline: Option<u64>);
    fn wait() -> WakeReason;
    fn wake(id: ThreadId);
    /// Makes `id`, if it is running user code, pass through the
    /// user-return hook soon (a suspension or termination request then
    /// takes effect).
    fn interrupt(id: ThreadId);
    /// The processors `id` may run on (bit n: processor n).
    fn set_affinity(id: ThreadId, mask: u64);
    fn current_cpu() -> u32;
    fn online_count() -> u32;
    fn online_mask() -> u64;
}

/// Entering and leaving user mode: system calls, returns to user mode,
/// exceptions and the user's floating-point state.
pub trait UserEntry {
    /// Every NT system call goes to `handler`.
    fn set_syscall_handler(handler: fn(&mut SyscallFrame));
    /// `hook` runs before each return to user mode; it may change the
    /// registers (APCs, suspension) and returns whether it did.
    fn set_user_return_hook(hook: fn(&mut UserRegs) -> bool);
    /// CPU exceptions raised in user mode go to `handler` (with the
    /// faulting address of a page fault).
    fn set_user_exception_handler(handler: fn(&mut TrapFrame, Option<u64>));
    fn exception_name(vector: u8) -> &'static str;
    /// Enters user mode with `regs` (a new thread's first entry).
    fn enter_user(regs: &UserRegs) -> !;
    /// The current system call returns to user mode with `regs` instead
    /// of its own result (NtContinue, exception dispatch).
    fn continue_with(regs: UserRegs);
    fn fxsave() -> FxState;
    fn fxrstor(state: &FxState);
    fn set_mxcsr(value: u32);
}

/// User memory: the address space of a process (`UserSpace`, whose
/// methods are the per-space operations: map_zeroed, map_borrowed,
/// map_private_copy, set_flags, unmap_page, mapping, read, write, ...)
/// and the page frames that sections own.
pub trait UserMemory {
    /// A zeroed frame owned by the caller.
    fn zeroed_frame() -> Option<Frame>;
    /// A frame holding `data` (at most a page; the rest zeroed).
    fn frame_with(data: &[u8]) -> Option<Frame>;
    fn free_frame(frame: Frame);
    /// The frame of a page-aligned, page-sized kernel heap allocation
    /// (file pages, which sections map directly).
    fn frame_of_heap_page(addr: usize) -> Frame;
    /// (free, total) page frames.
    fn frame_stats() -> (u64, u64);
}

/// The machine's disk, in 512-byte sectors (none: 0 sectors).
pub trait Storage {
    fn disk_sectors() -> u64;
    fn disk_read(first: u64, buf: &mut [u8]) -> Result<(), DiskError>;
    fn disk_write(first: u64, data: &[u8]) -> Result<(), DiskError>;
    fn disk_flush() -> Result<(), DiskError>;
}

// The NT layer's calls. Thin and inlined: the kernel implementation costs
// nothing over calling it directly.

#[inline]
pub fn ticks() -> u64 {
    <P as Clock>::ticks()
}
pub const TICK_HZ: u64 = <P as Clock>::TICK_HZ;
pub const PERFORMANCE_FREQUENCY: u64 = <P as Clock>::PERFORMANCE_FREQUENCY;
#[inline]
pub fn monotonic_ns() -> u64 {
    <P as Clock>::monotonic_ns()
}
#[inline]
pub fn system_time() -> u64 {
    <P as Clock>::system_time()
}
#[inline]
pub fn boot_system_time() -> u64 {
    <P as Clock>::boot_system_time()
}
#[inline]
pub fn set_tick_callback(callback: fn(u64)) {
    <P as Clock>::set_tick_callback(callback)
}

#[inline]
pub fn spawn(name: &'static str, entry: fn(usize), arg: usize) -> Result<ThreadId, SpawnError> {
    <P as Threads>::spawn(name, entry, arg)
}
#[inline]
pub fn spawn_user(
    name: &'static str,
    entry: fn(usize),
    arg: usize,
    space: SharedUserSpace,
    gs_base: u64,
) -> Result<ThreadId, SpawnError> {
    <P as Threads>::spawn_user(name, entry, arg, space, gs_base)
}
#[inline]
pub fn current() -> ThreadId {
    <P as Threads>::current()
}
#[inline]
pub fn exit() -> ! {
    <P as Threads>::exit()
}
#[inline]
pub fn yield_now() {
    <P as Threads>::yield_now()
}
#[inline]
pub fn sleep_ms(ms: u64) {
    <P as Threads>::sleep_ms(ms)
}
#[inline]
pub fn prepare_wait(deadline: Option<u64>) {
    <P as Threads>::prepare_wait(deadline)
}
#[inline]
pub fn wait() -> WakeReason {
    <P as Threads>::wait()
}
#[inline]
pub fn wake(id: ThreadId) {
    <P as Threads>::wake(id)
}
#[inline]
pub fn interrupt(id: ThreadId) {
    <P as Threads>::interrupt(id)
}
#[inline]
pub fn set_affinity(id: ThreadId, mask: u64) {
    <P as Threads>::set_affinity(id, mask)
}
#[inline]
pub fn current_cpu() -> u32 {
    <P as Threads>::current_cpu()
}
/// CPUID leaf `leaf` (subleaf `sub`) of this processor: (eax, ebx, ecx, edx).
#[inline]
pub fn cpuid(leaf: u32, sub: u32) -> (u32, u32, u32, u32) {
    let r = crate::arch::cpu::cpuid(leaf, sub);
    (r.eax, r.ebx, r.ecx, r.edx)
}
#[inline]
pub fn online_count() -> u32 {
    <P as Threads>::online_count()
}
#[inline]
pub fn online_mask() -> u64 {
    <P as Threads>::online_mask()
}

#[inline]
pub fn set_syscall_handler(handler: fn(&mut SyscallFrame)) {
    <P as UserEntry>::set_syscall_handler(handler)
}
#[inline]
pub fn set_user_return_hook(hook: fn(&mut UserRegs) -> bool) {
    <P as UserEntry>::set_user_return_hook(hook)
}
#[inline]
pub fn set_user_exception_handler(handler: fn(&mut TrapFrame, Option<u64>)) {
    <P as UserEntry>::set_user_exception_handler(handler)
}
#[inline]
pub fn exception_name(vector: u8) -> &'static str {
    <P as UserEntry>::exception_name(vector)
}
#[inline]
pub fn enter_user(regs: &UserRegs) -> ! {
    <P as UserEntry>::enter_user(regs)
}
#[inline]
pub fn continue_with(regs: UserRegs) {
    <P as UserEntry>::continue_with(regs)
}
#[inline]
pub fn fxsave() -> FxState {
    <P as UserEntry>::fxsave()
}
#[inline]
pub fn fxrstor(state: &FxState) {
    <P as UserEntry>::fxrstor(state)
}
#[inline]
pub fn set_mxcsr(value: u32) {
    <P as UserEntry>::set_mxcsr(value)
}

#[inline]
pub fn zeroed_frame() -> Option<Frame> {
    <P as UserMemory>::zeroed_frame()
}
#[inline]
pub fn frame_with(data: &[u8]) -> Option<Frame> {
    <P as UserMemory>::frame_with(data)
}
#[inline]
pub fn free_frame(frame: Frame) {
    <P as UserMemory>::free_frame(frame)
}
#[inline]
pub fn frame_of_heap_page(addr: usize) -> Frame {
    <P as UserMemory>::frame_of_heap_page(addr)
}
#[inline]
pub fn frame_stats() -> (u64, u64) {
    <P as UserMemory>::frame_stats()
}

#[inline]
pub fn disk_sectors() -> u64 {
    <P as Storage>::disk_sectors()
}
#[inline]
pub fn disk_read(first: u64, buf: &mut [u8]) -> Result<(), DiskError> {
    <P as Storage>::disk_read(first, buf)
}
#[inline]
pub fn disk_write(first: u64, data: &[u8]) -> Result<(), DiskError> {
    <P as Storage>::disk_write(first, data)
}
#[inline]
pub fn disk_flush() -> Result<(), DiskError> {
    <P as Storage>::disk_flush()
}
