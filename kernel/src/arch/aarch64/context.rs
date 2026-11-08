//! Kernel stacks and thread contexts (stub).

use super::unported;
use super::xstate::{NoMemory, XState};

pub const STACK_PAGES: usize = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StackError {
    TooManyStacks,
    OutOfMemory,
}

/// A kernel stack with a guard page below it.
#[derive(Debug)]
pub struct KernelStack {
    top: u64,
}

/// Frees the stack's pages (none are allocated yet).
impl Drop for KernelStack {
    fn drop(&mut self) {}
}

impl KernelStack {
    pub fn new() -> Result<Self, StackError> {
        unported("KernelStack::new")
    }

    pub fn top(&self) -> u64 {
        self.top
    }

    pub fn guard(&self) -> core::ops::Range<u64> {
        self.top..self.top
    }
}

/// A thread's saved kernel context (callee-saved registers, sp, FP/SIMD).
#[derive(Debug)]
pub struct Context {
    sp: u64,
}

impl Context {
    pub fn current() -> Result<Self, NoMemory> {
        Ok(Context { sp: 0 })
    }

    pub fn new_thread(
        _stack: &KernelStack,
        _entry: extern "C" fn() -> !,
    ) -> Result<Self, NoMemory> {
        unported("Context::new_thread")
    }

    /// # Safety
    /// The context's thread is not running.
    pub unsafe fn saved_xstate(&self) -> XState {
        let _ = self.sp;
        XState::initial()
    }
}

/// # Safety
/// As x86-64's: `from` is the running thread's, `to` a stopped one's.
pub unsafe fn switch(_from: &Context, _to: &Context) {
    unported("context::switch")
}
