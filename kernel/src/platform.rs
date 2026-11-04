//! The kernel as the NT layer's platform (`nt::platform`): each trait
//! method forwards to the kernel subsystem that provides it.

use crate::arch::{cpu, interrupts, paging, smp, syscall, xstate};
use crate::nt::platform::{
    Clock, DiskError, Display, Frame, Personality, SharedUserSpace, SpawnError, Storage,
    SyscallFrame, ThreadId, Threads, TrapFrame, UserEntry, UserMemory, UserRegs, WakeReason,
    XState,
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
    fn clock_control(op: crate::clockctl::Op) -> crate::clockctl::State {
        crate::clockctl::apply(op, time::raw_monotonic_ns(), time::boot_system_time())
    }
}

impl Threads for Kernel {
    fn last_user_rip(thread: ThreadId) -> Option<u64> {
        crate::time::last_user_rip(thread)
    }

    fn spawn(name: &'static str, entry: fn(usize), arg: usize) -> Result<ThreadId, SpawnError> {
        sched::spawn(name, entry, arg)
    }
    fn spawn_user(
        name: &'static str,
        entry: fn(usize),
        arg: usize,
        space: SharedUserSpace,
        gs_base: u64,
        personality: Personality,
    ) -> Result<ThreadId, SpawnError> {
        sched::spawn_user(name, entry, arg, space, gs_base, personality)
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
    fn context_switches(id: ThreadId) -> u32 {
        sched::context_switches(id)
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
    fn set_syscall_handler(
        personality: Personality,
        name: &'static str,
        handler: fn(&mut SyscallFrame),
    ) {
        syscall::set_handler(personality, name, handler)
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
    fn reissue_syscall(frame: &mut SyscallFrame) -> ! {
        syscall::reissue(frame)
    }
    fn continue_with(regs: UserRegs) {
        syscall::continue_with(regs)
    }
    fn xsave() -> XState {
        xstate::save()
    }
    fn xrstor(state: &XState) {
        xstate::restore(state)
    }
    fn set_mxcsr(value: u32) {
        xstate::set_mxcsr(value)
    }
    fn xstate_enabled() -> u64 {
        xstate::enabled()
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

/// Fills `buf` from the CPU's generator (RDRAND/RDSEED).
pub fn fill_from_cpu(buf: &mut [u8]) -> bool {
    for chunk in buf.chunks_mut(8) {
        let Some(word) = cpu::rdrand() else { return false };
        chunk.copy_from_slice(&word.to_le_bytes()[..chunk.len()]);
    }
    true
}

impl crate::nt::platform::Entropy for Kernel {
    fn entropy_seeded() -> bool {
        false
    }
    fn fill_entropy(buf: &mut [u8]) -> bool {
        fill_from_cpu(buf)
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

/// The framebuffer once the desktop has taken it from the console.
static SCREEN: crate::sync::IrqMutex<Option<crate::boot::Framebuffer>> =
    crate::sync::IrqMutex::new(None);
/// The desktop has taken the screen (once: the console stops drawing).
static TAKEN: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// The desktop takes the screen: the console stops drawing on the
/// framebuffer (with virtio-gpu it is not what is shown, and it is
/// dropped), the input thread starts.
fn take_screen() {
    if TAKEN.swap(true, core::sync::atomic::Ordering::AcqRel) {
        return;
    }
    let fb = crate::console::take_framebuffer();
    if crate::display::present() {
        crate::input::start();
    } else if let Some(fb) = fb {
        *SCREEN.lock() = Some(fb);
        crate::input::start();
    }
}

impl Display for Kernel {
    fn screen_size() -> Option<(u32, u32)> {
        take_screen();
        if crate::display::present() {
            return crate::display::size();
        }
        SCREEN.lock().as_ref().map(|fb| (fb.width() as u32, fb.height() as u32))
    }
    fn screen_write_row(x: u32, y: u32, rgb: &[u32]) {
        if crate::display::present() {
            crate::display::write_row(x, y, rgb);
        } else if let Some(fb) = SCREEN.lock().as_ref() {
            fb.write_row(x.into(), y.into(), rgb);
        }
    }
    fn screen_flush(x: u32, y: u32, w: u32, h: u32) {
        crate::display::flush(x, y, w, h);
    }
    fn screen_changed() -> Option<(u32, u32)> {
        crate::display::poll()
    }
}
