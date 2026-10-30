//! The scheduler's single use of `unsafe`: performing the context switch.

use crate::arch::context::{self, Context};

/// Switches from the context at `from` to the one at `to`.
///
/// Only `sched::schedule` calls this, and it guarantees the contract of
/// `arch::context::switch`: interrupts are disabled; both pointers point
/// into boxed `Thread`s, which are never freed (only their stacks are, and
/// only once dead and switched away from); `to` is a saved or fresh context
/// of a thread that is not running, with a live stack; and `from` is the
/// current thread's context.
pub(super) fn switch(from: *const Context, to: *const Context) {
    // SAFETY: see above.
    unsafe { context::switch(&*from, &*to) }
}
