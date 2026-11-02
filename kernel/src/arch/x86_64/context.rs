//! Kernel stacks and the low-level thread context switch.

use core::arch::global_asm;
use core::sync::atomic::AtomicU64;

use super::paging;
use super::xstate::{NoMemory, XArea, XState};
use crate::mm::paging::PageFlags;
use crate::mm::{self, PAGE_SIZE};
use crate::sync::IrqMutex;

/// Virtual window for kernel stacks: fixed-size slots, each a guard area
/// (never mapped) followed by the stack pages.
const STACK_WINDOW_START: u64 = 0xffff_d000_0000_0000;
const SLOT_SIZE: u64 = 128 * 1024;
const MAX_SLOTS: usize = 1024;
/// Default kernel stack size; the rest of the slot below it is guard space.
pub const STACK_PAGES: usize = 16;

static SLOTS: IrqMutex<[u64; MAX_SLOTS / 64]> = IrqMutex::new([0; MAX_SLOTS / 64]);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StackError {
    TooManyStacks,
    OutOfMemory,
}

/// A mapped kernel stack with an unmapped guard region below it. Unmapped
/// and freed on drop.
pub struct KernelStack {
    slot: usize,
    pages: usize,
}

impl KernelStack {
    pub fn new() -> Result<Self, StackError> {
        let slot = {
            let mut slots = SLOTS.lock();
            let (word, bit) = slots
                .iter()
                .enumerate()
                .find(|(_, w)| **w != u64::MAX)
                .map(|(i, w)| (i, w.trailing_ones() as usize))
                .ok_or(StackError::TooManyStacks)?;
            slots[word] |= 1 << bit;
            word * 64 + bit
        };
        let mut stack = KernelStack { slot, pages: 0 };
        for i in 0..STACK_PAGES {
            let frame = mm::allocate_frame().ok_or(StackError::OutOfMemory)?;
            let virt = stack.bottom() + i as u64 * PAGE_SIZE;
            if paging::map_kernel_page(virt, frame, PageFlags::KERNEL_RW).is_err() {
                let _ = mm::free_frame(frame);
                return Err(StackError::OutOfMemory); // `stack` drop frees the rest
            }
            stack.pages += 1;
        }
        Ok(stack)
    }

    /// Lowest mapped address.
    fn bottom(&self) -> u64 {
        STACK_WINDOW_START + (self.slot as u64 + 1) * SLOT_SIZE - STACK_PAGES as u64 * PAGE_SIZE
    }

    /// One past the highest stack address (16-byte aligned).
    pub fn top(&self) -> u64 {
        self.bottom() + STACK_PAGES as u64 * PAGE_SIZE
    }

    /// The guard region `[start, end)` below the stack.
    pub fn guard(&self) -> core::ops::Range<u64> {
        STACK_WINDOW_START + self.slot as u64 * SLOT_SIZE..self.bottom()
    }
}

/// Dropping a stack flushes every CPU's TLB, so it must not happen while an
/// `IrqMutex` is held (see `smp::flush_tlb_everywhere`).
impl Drop for KernelStack {
    fn drop(&mut self) {
        let mut frames = [None; STACK_PAGES];
        for (i, slot) in frames.iter_mut().enumerate().take(self.pages) {
            *slot = paging::unmap_kernel_page(self.bottom() + i as u64 * PAGE_SIZE);
        }
        // Other CPUs may still cache the old translations (kernel pages are
        // global): flush them before the frames and the slot are reused.
        if self.pages > 0 {
            super::smp::flush_tlb_everywhere();
        }
        for frame in frames.into_iter().flatten() {
            let _ = mm::free_frame(frame);
        }
        SLOTS.lock()[self.slot / 64] &= !(1 << (self.slot % 64));
    }
}

/// Saved state of a switched-out thread: stack pointer and its x87/SSE/
/// AVX state (an `XArea`: `xstate::size()` bytes on the heap, 64-byte
/// aligned). Layout used by `gala_context_switch`: rsp at 0, the area's
/// address at 8.
#[repr(C)]
pub struct Context {
    rsp: AtomicU64,
    area: XArea,
}

impl core::fmt::Debug for Context {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Context").field("rsp", &self.rsp).finish_non_exhaustive()
    }
}

/// Callee-saved registers pushed by `gala_context_switch`.
const SAVED_REGS: usize = 6;

impl Context {
    /// Context for the thread that is already running (its state is saved
    /// by the first switch away from it).
    pub fn current() -> Result<Self, NoMemory> {
        Ok(Context { rsp: AtomicU64::new(0), area: XArea::new()? })
    }

    /// A context that starts executing `entry` on `stack` when switched to.
    /// `entry` runs with interrupts disabled and must not return.
    pub fn new_thread(stack: &KernelStack, entry: extern "C" fn() -> !) -> Result<Self, NoMemory> {
        let area = XArea::new()?;
        // Frame popped by the switch, lowest address first: r15, r14, r13,
        // r12, rbx, rbp, return address, then two padding words so that
        // after the `ret` (rsp = top - 16) the trampoline's `call` leaves
        // `entry` with the ABI-required rsp % 16 == 8. r13 carries `entry`.
        let mut frame = [0u64; SAVED_REGS + 3];
        frame[2] = entry as *const () as u64; // r13
        frame[SAVED_REGS] = gala_thread_trampoline as *const () as u64;
        let rsp = stack.top() - (frame.len() * 8) as u64;
        // SAFETY: `[rsp, top)` lies inside the mapped stack, which nothing
        // else uses before the thread first runs.
        unsafe {
            let p = core::ptr::with_exposed_provenance_mut::<u64>(rsp as usize);
            p.copy_from_nonoverlapping(frame.as_ptr(), frame.len());
        }
        Ok(Context { rsp: AtomicU64::new(rsp), area })
    }

    /// The x87/SSE/AVX state saved by the last switch away from this
    /// context (a kernel handover carries a blocked thread's user state).
    /// The one accessor for the save area (its size: `xstate::size()`).
    ///
    /// # Safety
    /// The thread must not be running on any CPU (the switch writes the
    /// area while it saves it), which the scheduler's `on_cpu` tells.
    pub unsafe fn saved_xstate(&self) -> XState {
        // SAFETY: forwarded to the caller: nothing writes the area while
        // the thread is switched out.
        unsafe { self.area.load() }
    }
}

// The save instruction follows `xstate::GALA_XSAVE_MODE` (0 FXSAVE,
// 1 XSAVE, 2 XSAVEOPT); EDX:EAX = all ones asks for every component XCR0
// enables. rax, rcx, rdx and r8 are caller-saved, so free here; ecx keeps
// the mode across the switch.
global_asm!(
    r#"
    .section .text.context, "ax"
    .global gala_context_switch
gala_context_switch:
    mov r8, [rdi + 8]
    movzx ecx, byte ptr [rip + {mode}]
    mov eax, -1
    mov edx, -1
    cmp ecx, 2
    jne 1f
    xsaveopt64 [r8]
    jmp 3f
1:
    cmp ecx, 1
    jne 2f
    xsave64 [r8]
    jmp 3f
2:
    fxsave64 [r8]
3:
    push rbp
    push rbx
    push r12
    push r13
    push r14
    push r15
    mov [rdi], rsp
    mov rsp, [rsi]
    pop r15
    pop r14
    pop r13
    pop r12
    pop rbx
    pop rbp
    mov r8, [rsi + 8]
    mov eax, -1
    mov edx, -1
    test ecx, ecx
    jz 4f
    xrstor64 [r8]
    ret
4:
    fxrstor64 [r8]
    ret

    .global gala_thread_trampoline
gala_thread_trampoline:
    call r13
    ud2
    "#,
    mode = sym super::xstate::GALA_XSAVE_MODE,
);

unsafe extern "C" {
    fn gala_context_switch(from: *const Context, to: *const Context);
    fn gala_thread_trampoline();
}

/// Saves the current thread's state into `from` and resumes `to`. Returns
/// when something switches back to `from`.
///
/// # Safety
/// Interrupts must be disabled. `to` must hold a context saved by `switch`
/// or created by `Context::new_thread`, whose stack is still mapped and
/// which is not running anywhere else. `from` must stay alive until it is
/// switched back to (or never be resumed).
pub unsafe fn switch(from: &Context, to: &Context) {
    // SAFETY: forwarded to the caller. The asm writes `from.rsp`
    // (atomic) and the bytes of `from`'s save area (owned by the context,
    // written only here), and reads `to`.
    unsafe { gala_context_switch(from, to) }
}
