//! The scheduler's uses of `unsafe`: switching address space and context.

use crate::arch::context::{self, Context};
use crate::arch::{paging, percpu};

/// What to install for the thread being switched to.
pub(super) struct Target {
    pub context: *const Context,
    pub kernel_stack_top: Option<u64>,
    /// PML4 to load: the thread's user space, or the kernel's.
    pub root: u64,
    pub user_gs_base: u64,
    /// Its personality (where its system calls are dispatched).
    pub personality: crate::personality::Personality,
}

/// Switches from the context at `from` to `to`.
///
/// Only `sched::schedule` calls this, and it guarantees:
/// - interrupts are disabled;
/// - both contexts live in boxed `Thread`s, which are never freed (only
///   their stacks and address spaces are, once dead and switched away from);
/// - `to.context` is a saved or fresh context of a thread that is not
///   running, with a live stack, and `from` is the current thread's;
/// - `to.root` is the kernel root or the root of a `UserSpace` kept alive by
///   the thread's `KArc`.
pub(super) fn switch(from: *const Context, to: Target) {
    if let Some(top) = to.kernel_stack_top {
        percpu::set_kernel_stack(top);
    }
    percpu::set_user_gs_base(to.user_gs_base);
    percpu::set_personality(to.personality);
    // SAFETY: see above.
    unsafe {
        paging::activate(to.root);
        context::switch(&*from, &*to.context);
    }
}

/// The x87/SSE/AVX state `context` was saved with. The scheduler calls
/// this with its lock held for a thread that is on no CPU (`on_cpu`
/// false), so no switch writes the area meanwhile.
pub(super) fn saved_xstate(context: &Context) -> crate::arch::xstate::XState {
    // SAFETY: see above: the thread is not running and cannot start while
    // the scheduler's lock is held.
    unsafe { context.saved_xstate() }
}
