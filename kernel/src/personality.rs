//! Personalities: the system-call ABI a process speaks, and the table of
//! system-call dispatchers keyed by it (ARCHITECTURE "Plan: a second
//! personality (Linux)", seam 2).
//!
//! Every process has a personality, fixed when it is created (from its
//! image: a PE image is NT; an ELF loader would make Linux processes). Its
//! threads carry it into the scheduler (the kernel keeps the running
//! thread's in its per-CPU block; gala-um in a thread-local), and the
//! system-call entry of each platform (`arch::syscall::dispatch`,
//! `hosted::entry`'s SIGSYS handler) hands the call to the dispatcher
//! registered for it here. NT is the default and today the only real one;
//! host unit tests add a second, `Test`, to check that the entry is keyed
//! by the personality and not hard-wired to NT.
//!
//! The dispatchers are registered once, at start (`nt::syscalls::init`
//! registers NT's), and read without a lock on every system call.

use crate::arch::syscall::SyscallFrame;

/// The system-call ABI of a process (and of each of its threads).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
pub enum Personality {
    /// Windows NT: ntdll's system calls (`nt::syscalls`).
    #[default]
    Nt = 0,
    /// Host unit tests only: a second personality with its own dispatcher.
    #[cfg(test)]
    Test = 1,
}

impl Personality {
    /// The number of personalities (the dispatcher table's size).
    pub const COUNT: usize = if cfg!(test) { 2 } else { 1 };

    pub const fn index(self) -> usize {
        self as usize
    }

    /// The personality with index `i` (the per-CPU block keeps it as a
    /// number); None for a number no personality has.
    pub const fn from_index(i: usize) -> Option<Personality> {
        match i {
            0 => Some(Personality::Nt),
            #[cfg(test)]
            1 => Some(Personality::Test),
            _ => None,
        }
    }

    /// Its name in logs and events (`"personality":"nt"`).
    pub const fn name(self) -> &'static str {
        match self {
            Personality::Nt => "nt",
            #[cfg(test)]
            Personality::Test => "test",
        }
    }
}

impl core::fmt::Display for Personality {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.name())
    }
}

/// A system-call handler: reads the call from `frame` and leaves its
/// result in `frame.rax`.
pub type Handler = fn(&mut SyscallFrame);

/// A registered dispatcher: its name (in "unknown system call" logs) and
/// its handler.
#[derive(Clone, Copy)]
pub struct Dispatcher {
    pub name: &'static str,
    pub handler: Handler,
}

/// What `Dispatchers::dispatch` reports when the personality has no
/// dispatcher (the platform logs it and fails the call).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NoDispatcher(pub Personality);

/// What a system call returns in rax when its thread's personality has no
/// dispatcher: -1 (no ABI's success value).
pub const NO_DISPATCHER_RESULT: u64 = u64::MAX;

/// One dispatcher per personality. Each platform's system-call entry owns
/// one table (a static); registration is once per personality.
pub struct Dispatchers {
    slots: [spin::Once<Dispatcher>; Personality::COUNT],
}

impl Default for Dispatchers {
    fn default() -> Self {
        Self::new()
    }
}

impl Dispatchers {
    pub const fn new() -> Self {
        Dispatchers { slots: [const { spin::Once::new() }; Personality::COUNT] }
    }

    /// Registers `handler` (called `name`) for `personality`. False if the
    /// personality already has one (the first stays).
    pub fn register(&self, personality: Personality, name: &'static str, handler: Handler) -> bool {
        let mut new = false;
        self.slots[personality.index()].call_once(|| {
            new = true;
            Dispatcher { name, handler }
        });
        new
    }

    /// The dispatcher of `personality`, if registered.
    pub fn get(&self, personality: Personality) -> Option<Dispatcher> {
        self.slots[personality.index()].get().copied()
    }

    /// Hands the system call in `frame`, made by a thread of
    /// `personality`, to that personality's dispatcher; returns its name.
    pub fn dispatch(
        &self,
        personality: Personality,
        frame: &mut SyscallFrame,
    ) -> Result<&'static str, NoDispatcher> {
        let d = self.get(personality).ok_or(NoDispatcher(personality))?;
        (d.handler)(frame);
        Ok(d.name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(number: u64) -> SyscallFrame {
        SyscallFrame {
            r15: 0,
            r14: 0,
            r13: 0,
            r12: 0,
            rbp: 0,
            rbx: 0,
            rsi: 0,
            rdi: 0,
            r9: 0,
            r8: 0,
            rdx: 0,
            r10: 0,
            rax: number,
            rip: 0x1000,
            rflags: 0x202,
            rsp: 0x2000,
        }
    }

    fn nt_handler(f: &mut SyscallFrame) {
        f.rax |= 0x4e54_0000;
    }

    fn test_handler(f: &mut SyscallFrame) {
        f.rax |= 0x7e57_0000;
    }

    /// A process of the second personality: its threads carry the
    /// personality it was created with.
    struct Process {
        personality: Personality,
    }

    #[test]
    fn a_process_of_the_test_personality_reaches_its_dispatcher() {
        let table = Dispatchers::new();
        assert!(table.register(Personality::Nt, "nt::syscalls", nt_handler));
        assert!(table.register(Personality::Test, "test::syscalls", test_handler));
        let windows = Process { personality: Personality::default() };
        let other = Process { personality: Personality::Test };

        let mut f = frame(0x12);
        assert_eq!(table.dispatch(other.personality, &mut f), Ok("test::syscalls"));
        assert_eq!(f.rax, 0x7e57_0012, "the test dispatcher handled it, not NT's");

        let mut f = frame(0x12);
        assert_eq!(table.dispatch(windows.personality, &mut f), Ok("nt::syscalls"));
        assert_eq!(f.rax, 0x4e54_0012, "NT stays the default");
    }

    #[test]
    fn a_personality_without_a_dispatcher_is_reported_not_sent_to_nt() {
        let table = Dispatchers::new();
        assert!(table.register(Personality::Nt, "nt::syscalls", nt_handler));
        let mut f = frame(0x12);
        assert_eq!(table.dispatch(Personality::Test, &mut f), Err(NoDispatcher(Personality::Test)));
        assert_eq!(f.rax, 0x12, "nothing handled it");
    }

    #[test]
    fn registration_is_once_per_personality() {
        let table = Dispatchers::new();
        assert!(table.register(Personality::Nt, "nt::syscalls", nt_handler));
        assert!(!table.register(Personality::Nt, "other", test_handler));
        assert_eq!(table.get(Personality::Nt).map(|d| d.name), Some("nt::syscalls"));
        assert!(table.get(Personality::Test).is_none());
    }

    #[test]
    fn indices_and_names_round_trip() {
        for i in 0..Personality::COUNT {
            let p = Personality::from_index(i).unwrap();
            assert_eq!(p.index(), i);
        }
        assert_eq!(Personality::from_index(Personality::COUNT), None);
        assert_eq!(Personality::Nt.name(), "nt");
        assert_eq!(alloc::format!("{}", Personality::Test), "test");
    }
}
