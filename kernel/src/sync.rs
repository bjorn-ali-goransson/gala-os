//! Locks for kernel data shared with interrupt handlers.

use core::ops::{Deref, DerefMut};

#[cfg(not(any(test, feature = "hosted")))]
use crate::arch::cpu;
#[cfg(any(test, feature = "hosted"))]
use host_cpu as cpu;

/// A spinlock that keeps interrupts disabled on this CPU while it is held,
/// so an interrupt handler on the same CPU can never deadlock on it.
pub struct IrqMutex<T> {
    inner: spin::Mutex<T>,
    /// The lock-order class (`lockdep`): a name given, else the type the
    /// lock protects.
    class: Option<&'static str>,
}

impl<T> IrqMutex<T> {
    pub const fn new(value: T) -> Self {
        Self { inner: spin::Mutex::new(value), class: None }
    }

    /// A lock with a lock-order class of its own: for locks that protect
    /// a plain type (a number, an Option of an id), whose type says
    /// nothing about which lock it is.
    pub const fn named(class: &'static str, value: T) -> Self {
        Self { inner: spin::Mutex::new(value), class: Some(class) }
    }

    fn class_name(&self) -> &'static str {
        self.class.unwrap_or(core::any::type_name::<T>())
    }

    pub fn lock(&self) -> IrqGuard<'_, T> {
        let restore = cpu::disable_interrupts();
        // Debug builds: an order that contradicts one seen before panics
        // here, before the deadlock it could cause.
        let class = lockdep::acquire(self.class_name(), true);
        let guard = loop {
            if let Some(guard) = self.inner.try_lock() {
                break guard;
            }
            // The holder may be waiting for this CPU (interrupts off) to
            // flush its TLB.
            cpu::lock_contended();
            core::hint::spin_loop();
        };
        IrqGuard { guard: Some(guard), restore, class }
    }
}

impl<T> IrqMutex<T> {
    /// Takes the lock if it is free (for paths that must not wait, such as
    /// panic output).
    pub fn try_lock(&self) -> Option<IrqGuard<'_, T>> {
        let restore = cpu::disable_interrupts();
        match self.inner.try_lock() {
            // Trying cannot deadlock: no order check, but it is held.
            Some(guard) => Some(IrqGuard {
                guard: Some(guard),
                restore,
                class: lockdep::acquire(self.class_name(), false),
            }),
            None => {
                if restore {
                    cpu::enable_interrupts();
                }
                None
            }
        }
    }
}

impl<T> IrqMutex<T> {
    /// `try_lock` that lockdep does not track: for diagnostics that run
    /// on top of whatever the interrupted code holds (an NMI's state dump,
    /// the watchdog, emergency output). Tracked, they would record an
    /// order between the interrupted code's locks and this one that no
    /// real code path has, and a later correct path would be reported as
    /// an inversion. Hold it briefly and take nothing under it.
    pub fn try_lock_untracked(&self) -> Option<IrqGuard<'_, T>> {
        let restore = cpu::disable_interrupts();
        match self.inner.try_lock() {
            Some(guard) => Some(IrqGuard { guard: Some(guard), restore, class: 0 }),
            None => {
                if restore {
                    cpu::enable_interrupts();
                }
                None
            }
        }
    }
}

pub struct IrqGuard<'a, T> {
    guard: Option<spin::MutexGuard<'a, T>>,
    restore: bool,
    /// The lock's class while lockdep tracks it (0: untracked).
    class: u16,
}

impl<T> Deref for IrqGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &T {
        self.guard.as_ref().expect("guard is present until drop")
    }
}

impl<T> DerefMut for IrqGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        self.guard.as_mut().expect("guard is present until drop")
    }
}

impl<T> Drop for IrqGuard<'_, T> {
    fn drop(&mut self) {
        // Release the lock before re-enabling interrupts.
        self.guard = None;
        lockdep::release(self.class);
        if self.restore {
            // Only the outermost lock turns interrupts back on; released
            // before an inner one, it would leave that one held with
            // interrupts on, and an interrupt on this CPU taking it would
            // spin forever (debug builds panic: `lockdep::none_held`).
            lockdep::none_held(self.class);
            cpu::enable_interrupts();
        }
    }
}

impl<T: Default> Default for IrqMutex<T> {
    fn default() -> Self {
        Self::new(T::default())
    }
}

/// Host unit tests and gala-um run in user mode, where `cli`/`sti` would
/// fault.
#[cfg(any(test, feature = "hosted"))]
mod host_cpu {
    /// gala-um's simulation (`--sim`) counts the locks a thread holds, as
    /// the kernel's interrupt flag does: when the last is released, a
    /// pending "timer interrupt" may preempt the thread there
    /// (`hosted::sim::lock_point`). Otherwise nothing.
    pub fn disable_interrupts() -> bool {
        #[cfg(feature = "hosted")]
        if crate::hosted::sim::on() {
            crate::hosted::sim::lock_taken();
            return true;
        }
        false
    }
    pub fn enable_interrupts() {
        #[cfg(feature = "hosted")]
        crate::hosted::sim::lock_released();
    }
    /// In gala-um the holder is a host thread the host may deschedule
    /// (the kernel's holders run with interrupts off): spinning would keep
    /// it from finishing, so give the processor away.
    pub fn lock_contended() {
        #[cfg(feature = "hosted")]
        {
            if crate::hosted::sim::on() {
                crate::hosted::sim::lock_contended();
            }
            std::thread::yield_now();
        }
    }
}

/// Lock-order checking (debug builds; ROADMAP R3), a small lockdep: a
/// lock's class is its name (`IrqMutex::named`) or the type it protects; each CPU (each host thread in
/// tests and gala-um) keeps the classes it holds; taking class B while
/// holding A records "A before B", and taking B while holding A when "B
/// before A" was seen panics with both names, before the deadlock such an
/// inversion can cause. Locks of one class nested in each other are not
/// checked (no order between them is known). Release builds compile it
/// out.
pub mod lockdep {
    #[cfg(debug_assertions)]
    use core::sync::atomic::{
        AtomicBool, AtomicU8, AtomicU16, AtomicU32, AtomicU64, AtomicUsize, Ordering,
    };

    /// Lockdep runs (the kernel turns it on once each CPU's per-CPU data,
    /// which says which CPU it is, exists).
    #[cfg(debug_assertions)]
    static ON: AtomicBool = AtomicBool::new(cfg!(any(test, feature = "hosted")));

    pub fn enable() {
        #[cfg(debug_assertions)]
        ON.store(true, Ordering::Release);
    }

    #[cfg(debug_assertions)]
    const MAX_CLASSES: usize = 512;
    #[cfg(debug_assertions)]
    const MAX_EDGES: usize = 8192;
    #[cfg(debug_assertions)]
    const MAX_HELD: usize = 16;
    /// How much of its name every class keeps for messages. Type names are
    /// long (`core::option::Option<kernel::sched::Scheduler>` is 44
    /// bytes); a longer one is cut at a character boundary and shown with
    /// `..`, never dropped.
    #[cfg(debug_assertions)]
    const NAME_MAX: usize = 96;

    /// What makes a class a class: the address of its name, which is one
    /// `&'static str` per type (`type_name`) or per `IrqMutex::named`
    /// literal. Id = index + 1.
    #[cfg(debug_assertions)]
    static NAMES: [AtomicUsize; MAX_CLASSES] = [const { AtomicUsize::new(0) }; MAX_CLASSES];
    /// Each class's whole name length, and the marker that its name has
    /// been written (0: the slot's claimer has not finished copying).
    #[cfg(debug_assertions)]
    static LENS: [AtomicUsize; MAX_CLASSES] = [const { AtomicUsize::new(0) }; MAX_CLASSES];
    /// Each class's name, copied byte by byte because a `&'static str`
    /// fits in no atomic and rebuilding one from a pointer and a length
    /// needs `unsafe`, which this module may not use. The CPU that claims
    /// the slot writes it before handing the id out, so every class a
    /// message can mention has its name: there is no side table to fill
    /// up and no name is lost to contention (both of which printed "?").
    #[cfg(debug_assertions)]
    static NAME_BYTES: [[AtomicU8; NAME_MAX]; MAX_CLASSES] =
        [const { [const { AtomicU8::new(0) }; NAME_MAX] }; MAX_CLASSES];
    /// The orders seen: `a << 16 | b` for "a held when b was taken".
    #[cfg(debug_assertions)]
    static EDGES: [AtomicU32; MAX_EDGES] = [const { AtomicU32::new(0) }; MAX_EDGES];

    #[cfg(debug_assertions)]
    fn class_of(name: &'static str) -> u16 {
        let (ptr, len) = (name.as_ptr() as usize, name.len());
        // Open addressing from the name's address (classes never go).
        let start = ptr.wrapping_mul(0x9e37_79b9_7f4a_7c15) >> 7;
        for k in 0..MAX_CLASSES {
            let i = (start + k) % MAX_CLASSES;
            match NAMES[i].load(Ordering::Acquire) {
                p if p == ptr => return (i + 1) as u16,
                0 => match NAMES[i].compare_exchange(0, ptr, Ordering::AcqRel, Ordering::Acquire) {
                    Ok(_) => {
                        // This CPU owns slot i now: copy the name, then
                        // publish the length, which is what `name` reads
                        // first (its Acquire pairs with this Release, so
                        // it sees whole bytes).
                        let room = NAME_MAX.min(len);
                        let keep =
                            (0..=room).rev().find(|&k| name.is_char_boundary(k)).unwrap_or(0);
                        for (slot, b) in NAME_BYTES[i].iter().zip(&name.as_bytes()[..keep]) {
                            slot.store(*b, Ordering::Relaxed);
                        }
                        LENS[i].store(len, Ordering::Release);
                        return (i + 1) as u16;
                    }
                    Err(p) if p == ptr => return (i + 1) as u16,
                    Err(_) => {}
                },
                _ => {}
            }
        }
        0 // the table is full: untracked
    }

    /// A class's name, read out of `NAME_BYTES` for a message. Every
    /// class an id was handed out for has one, so this is "?" only for
    /// class 0, which is no class.
    #[cfg(debug_assertions)]
    struct ClassName {
        bytes: [u8; NAME_MAX],
        len: usize,
        /// The whole name was longer than `NAME_MAX`: shown with `..`.
        cut: bool,
    }

    #[cfg(debug_assertions)]
    impl ClassName {
        /// As much of the name as is kept (`..` is left to `Display`).
        fn as_str(&self) -> &str {
            core::str::from_utf8(&self.bytes[..self.len]).unwrap_or("?")
        }
    }

    #[cfg(debug_assertions)]
    impl core::fmt::Display for ClassName {
        fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
            f.write_str(self.as_str())?;
            if self.cut {
                f.write_str("..")?;
            }
            Ok(())
        }
    }

    #[cfg(debug_assertions)]
    fn name(class: u16) -> ClassName {
        let unknown = ClassName { bytes: [b'?'; NAME_MAX], len: 1, cut: false };
        let Some(i) = usize::from(class).checked_sub(1).filter(|i| *i < MAX_CLASSES) else {
            return unknown;
        };
        // A slot's claimer copies the name with interrupts off and
        // takes no lock, so it publishes the length within a few
        // hundred cycles of the id it handed out: wait that out rather
        // than print "?" for a class that has a name. Bounded, because
        // the message may be a panic's and the claimer may be the CPU
        // that is panicking.
        let mut len = LENS[i].load(Ordering::Acquire);
        for _ in 0..10_000 {
            if len != 0 {
                break;
            }
            core::hint::spin_loop();
            len = LENS[i].load(Ordering::Acquire);
        }
        if len == 0 {
            return unknown; // claimed, and its claimer never finished
        }
        let mut out = ClassName { bytes: [0; NAME_MAX], len: 0, cut: len > NAME_MAX };
        for (b, slot) in out.bytes.iter_mut().zip(NAME_BYTES[i].iter()) {
            *b = slot.load(Ordering::Relaxed);
        }
        out.len = out.bytes.iter().position(|b| *b == 0).unwrap_or(NAME_MAX);
        out
    }

    /// A lock's holder in a message. The token is a thread's own number
    /// plus one (0: no thread recorded), printed the way the rest of the
    /// log prints a thread (`sched::ThreadIdNumber`): the index, and
    /// `.generation` when the thread record has been reused.
    #[cfg(debug_assertions)]
    struct Holder(u64);

    #[cfg(debug_assertions)]
    impl core::fmt::Display for Holder {
        fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
            let Some(raw) = self.0.checked_sub(1) else {
                return f.write_str("an unknown thread");
            };
            write!(f, "thread {}", raw & 0xffff_ffff)?;
            match raw >> 32 {
                0 => Ok(()),
                generation => write!(f, ".{generation}"),
            }
        }
    }

    /// The edge table is open-addressed: a key's slots start at its hash
    /// and go on until it or an empty slot (keys are never removed). The
    /// probes are plain loops: lockdep runs at every lock taken, and the
    /// kernel's debug build (opt-level 1) does not flatten iterator
    /// chains, which made lockdep half of the kernel's time in lx_gimp.
    #[cfg(debug_assertions)]
    const _: () = assert!(MAX_EDGES.is_power_of_two());

    #[cfg(debug_assertions)]
    fn first_slot(key: u32) -> usize {
        key.wrapping_mul(0x9e37_79b1) as usize & (MAX_EDGES - 1)
    }

    #[cfg(debug_assertions)]
    fn edge_seen(a: u16, b: u16) -> bool {
        let key = (u32::from(a) << 16) | u32::from(b);
        let mut i = first_slot(key);
        for _ in 0..MAX_EDGES {
            match EDGES[i].load(Ordering::Acquire) {
                0 => return false,
                k if k == key => return true,
                _ => i = (i + 1) & (MAX_EDGES - 1),
            }
        }
        false
    }

    /// Records "a held when b was taken". Nearly every order has been
    /// seen before: it is read first, and only an empty slot is
    /// claimed (a compare-exchange on every take would have each CPU
    /// writing the table's lines).
    #[cfg(debug_assertions)]
    fn add_edge(a: u16, b: u16) {
        let key = (u32::from(a) << 16) | u32::from(b);
        let mut i = first_slot(key);
        for _ in 0..MAX_EDGES {
            let e = &EDGES[i];
            match e.load(Ordering::Acquire) {
                k if k == key => return,
                0 => match e.compare_exchange(0, key, Ordering::AcqRel, Ordering::Acquire) {
                    Ok(_) => return,
                    Err(k) if k == key => return,
                    Err(_) => {}
                },
                _ => {}
            }
            i = (i + 1) & (MAX_EDGES - 1);
        }
    }

    /// The locks one CPU (one host thread, in tests and gala-um) holds:
    /// the class in each slot and the thread that took it, packed in the
    /// first `depth` slots (a release moves the last into the gap), so
    /// that a take, a release and the checks read only the held ones. The holder is
    /// kept because the slots belong to the CPU, not to the thread: a
    /// lock left held across a wait or a switch is found by whatever runs
    /// next, and only the holder says whose it was.
    #[cfg(debug_assertions)]
    struct Slots<'a> {
        class: &'a [AtomicU16; MAX_HELD],
        holder: &'a [AtomicU64; MAX_HELD],
        depth: &'a AtomicU8,
    }

    #[cfg(debug_assertions)]
    impl Slots<'_> {
        /// Slot `i`'s class (0: free) and the thread that took it.
        fn get(&self, i: usize) -> (u16, u64) {
            (self.class[i].load(Ordering::Relaxed), self.holder[i].load(Ordering::Relaxed))
        }

        /// The holder goes in before the class, so a slot that reads as
        /// taken always reads with its holder.
        fn take(&self, i: usize, class: u16, holder: u64) {
            self.holder[i].store(holder, Ordering::Relaxed);
            self.class[i].store(class, Ordering::Relaxed);
        }

        fn free(&self, i: usize) {
            self.class[i].store(0, Ordering::Relaxed);
            self.holder[i].store(0, Ordering::Relaxed);
        }

        /// How many slots are taken (slots 0..depth).
        fn depth(&self) -> usize {
            usize::from(self.depth.load(Ordering::Relaxed)).min(MAX_HELD)
        }

        /// The first held lock that `keep` accepts (class, holder).
        fn find(&self, keep: impl Fn(u16, u64) -> bool) -> Option<(u16, u64)> {
            let mut i = 0;
            while i < self.depth() {
                let (class, by) = self.get(i);
                if keep(class, by) {
                    return Some((class, by));
                }
                i += 1;
            }
            None
        }

        /// Takes the next slot; false when all are taken.
        fn push(&self, class: u16, holder: u64) -> bool {
            let depth = self.depth();
            if depth == MAX_HELD {
                return false;
            }
            self.take(depth, class, holder);
            self.depth.store(depth as u8 + 1, Ordering::Relaxed);
            true
        }

        /// Frees the latest slot holding `class`, the last slot moving
        /// into it.
        fn remove(&self, class: u16) {
            let depth = self.depth();
            let mut i = depth;
            while i > 0 {
                i -= 1;
                if self.get(i).0 == class {
                    let last = depth - 1;
                    if i != last {
                        let (moved, by) = self.get(last);
                        self.take(i, moved, by);
                    }
                    self.free(last);
                    self.depth.store(last as u8, Ordering::Relaxed);
                    return;
                }
            }
        }
    }

    #[cfg(all(debug_assertions, not(any(test, feature = "hosted"))))]
    mod held {
        use super::{MAX_HELD, Slots};
        use core::sync::atomic::{AtomicU8, AtomicU16, AtomicU32, AtomicU64, Ordering};
        const CPUS: usize = crate::arch::smp::MAX_CPUS;
        static DEPTH: [AtomicU8; CPUS] = [const { AtomicU8::new(0) }; CPUS];
        static HELD: [[AtomicU16; MAX_HELD]; CPUS] =
            [const { [const { AtomicU16::new(0) }; MAX_HELD] }; CPUS];
        static HOLDER: [[AtomicU64; MAX_HELD]; CPUS] =
            [const { [const { AtomicU64::new(0) }; MAX_HELD] }; CPUS];
        /// The thread each CPU runs, as the scheduler last said
        /// (`lockdep::running`). Lockdep runs inside `IrqMutex::lock` and
        /// the scheduler's own state is behind such a lock, so it cannot
        /// ask the scheduler who is running: it is told instead.
        static CURRENT: [AtomicU64; CPUS] = [const { AtomicU64::new(0) }; CPUS];

        fn here() -> usize {
            (crate::arch::percpu::cpu_index() as usize).min(CPUS - 1)
        }

        /// Runs `f` on this CPU's slots (interrupts are off while any
        /// IrqMutex is held, so the CPU does not change under it).
        pub fn with<R>(f: impl FnOnce(&Slots<'_>) -> R) -> R {
            let cpu = here();
            f(&Slots { class: &HELD[cpu], holder: &HOLDER[cpu], depth: &DEPTH[cpu] })
        }

        /// The thread taking a lock here: whoever the scheduler last put
        /// on this CPU.
        pub fn current() -> u64 {
            CURRENT[here()].load(Ordering::Relaxed)
        }

        pub fn set_current(cpu: usize, thread: u64) {
            CURRENT[cpu.min(CPUS - 1)].store(thread, Ordering::Relaxed);
        }

        /// How many `UserMemoryUnchecked` guards this CPU has (made with
        /// a lock held, so interrupts are off and the CPU does not change
        /// under them).
        static UNCHECKED: [AtomicU32; CPUS] = [const { AtomicU32::new(0) }; CPUS];

        pub fn unchecked() -> u32 {
            UNCHECKED[here()].load(Ordering::Relaxed)
        }

        pub fn set_unchecked(n: u32) {
            UNCHECKED[here()].store(n, Ordering::Relaxed);
        }
    }

    #[cfg(all(debug_assertions, any(test, feature = "hosted")))]
    mod held {
        use super::{MAX_HELD, Slots};
        use core::sync::atomic::{AtomicU8, AtomicU16, AtomicU64, Ordering};
        std::thread_local! {
            static HELD: ([AtomicU16; MAX_HELD], [AtomicU64; MAX_HELD], AtomicU8) = const {
                (
                    [const { AtomicU16::new(0) }; MAX_HELD],
                    [const { AtomicU64::new(0) }; MAX_HELD],
                    AtomicU8::new(0),
                )
            };
            /// This host thread's number: here a kernel thread is a host
            /// thread, so the holder needs no word from the scheduler.
            static ID: u64 = NEXT.fetch_add(1, Ordering::Relaxed) + 1;
            static UNCHECKED: core::cell::Cell<u32> = const { core::cell::Cell::new(0) };
        }
        static NEXT: AtomicU64 = AtomicU64::new(0);

        pub fn with<R>(f: impl FnOnce(&Slots<'_>) -> R) -> R {
            HELD.with(|h| f(&Slots { class: &h.0, holder: &h.1, depth: &h.2 }))
        }

        pub fn current() -> u64 {
            // 0 ("an unknown thread") once this thread's locals are gone.
            ID.try_with(|id| *id).unwrap_or(0)
        }

        pub fn set_current(_cpu: usize, _thread: u64) {}

        /// How many `UserMemoryUnchecked` guards this thread has (here a
        /// kernel thread is a host thread).
        pub fn unchecked() -> u32 {
            UNCHECKED.try_with(|n| n.get()).unwrap_or(0)
        }

        pub fn set_unchecked(n: u32) {
            let _ = UNCHECKED.try_with(|slot| slot.set(n));
        }
    }

    /// Before taking a lock of class `name` (`check`: a blocking take);
    /// its class id.
    #[cfg(debug_assertions)]
    pub fn acquire(class_name: &'static str, check: bool) -> u16 {
        if !ON.load(Ordering::Acquire) {
            return 0;
        }
        let class = class_of(class_name);
        if class == 0 {
            return 0;
        }
        let me = held::current();
        held::with(|slots| {
            for i in 0..slots.depth() {
                let (a, by) = slots.get(i);
                if a == 0 || a == class {
                    continue;
                }
                if check && edge_seen(class, a) {
                    panic!(
                        "lockdep: {} taken by {} while holding {} (taken by {}), \
                         but {} was taken while holding {} before",
                        name(class),
                        Holder(me),
                        name(a),
                        Holder(by),
                        name(a),
                        name(class)
                    );
                }
                add_edge(a, class);
            }
            // Nested too deep: untracked.
            if slots.push(class, me) { class } else { 0 }
        })
    }

    #[cfg(not(debug_assertions))]
    pub fn acquire(_name: &'static str, _check: bool) -> u16 {
        0
    }

    /// After releasing a lock of class `class`.
    pub fn release(class: u16) {
        #[cfg(debug_assertions)]
        if class != 0 {
            held::with(|slots| slots.remove(class));
        }
        #[cfg(not(debug_assertions))]
        let _ = class;
    }

    /// Before a released lock of class `class` turns interrupts back on
    /// (it was taken with them on): panics if this CPU still holds a lock
    /// (debug kernel builds), which would then be held with interrupts on:
    /// locks released out of order (hygiene sweep, 2026-11-03).
    pub fn none_held(class: u16) {
        #[cfg(all(debug_assertions, not(any(test, feature = "hosted"))))]
        if ON.load(Ordering::Acquire) {
            let still = held::with(|slots| slots.find(|c, _| c != 0));
            if let Some((other, by)) = still {
                // `by` is the thread that took the lock and `me` the one
                // releasing: when they differ, the lock was held across a
                // wait, a yield or a migration, and this CPU's slots
                // outlived the thread that filled them.
                let me = held::current();
                panic!(
                    "lockdep: releasing {} turns interrupts on while {} is still held \
                     (locks released out of order); {} was taken by {}, and {} is \
                     released by {}",
                    name(class),
                    name(other),
                    name(other),
                    Holder(by),
                    name(class),
                    Holder(me)
                );
            }
        }
        let _ = class;
    }

    /// Before a thread gives up its CPU (`sched::schedule`): panics if
    /// it still holds a lock. The held locks belong to the CPU, so one
    /// carried across a switch is found by whichever thread runs here
    /// next, and the panic lands in a thread that is not the culprit.
    /// This catches it while the culprit is still on the CPU.
    pub fn none_held_at_switch() {
        #[cfg(debug_assertions)]
        if ON.load(Ordering::Acquire) {
            let still = held::with(|slots| slots.find(|c, _| c != 0));
            if let Some((class, by)) = still {
                panic!(
                    "lockdep: {} gives up its CPU while holding {} (taken by {}): \
                     a lock may not be held across a wait, a yield or a switch",
                    Holder(held::current()),
                    name(class),
                    Holder(by)
                );
            }
        }
    }

    /// Before the kernel copies user memory (`nt::process::with_faults`):
    /// panics if the copying thread holds a lock (debug kernel builds). A
    /// page that is not there is made on the way, which reads the file or
    /// the disk and waits, so a lock held over a copy is a lock held
    /// across a wait. Checked at every copy, not only at the copies that
    /// do wait: whether a page is there depends on what the machine has
    /// been doing, so without this the mistake shows up as a panic in a
    /// long run under memory pressure and in no test.
    ///
    /// The copying thread's own locks only: the slots belong to the CPU,
    /// and one whose holder is another thread is a lock carried across a
    /// switch, which `none_held_at_switch` and `none_held` report where
    /// it happens. Such a slot has been seen here all the same (ROADMAP
    /// "A lock can be marked held by a thread that is not running"), and
    /// a copy is not where that is diagnosed.
    pub fn none_held_at_user_memory() {
        #[cfg(debug_assertions)]
        if ON.load(Ordering::Acquire) && held::unchecked() == 0 {
            let me = held::current();
            let mine = held::with(|slots| slots.find(|c, by| c != 0 && by == me));
            if let Some((class, _)) = mine {
                panic!(
                    "lockdep: {} copies user memory while holding {}: the copy waits \
                     for a page that is not there, and a lock may not be held across \
                     a wait",
                    Holder(me),
                    name(class)
                );
            }
        }
    }

    /// While one of these lives, this CPU may copy a program's memory
    /// with a lock held: `none_held_at_user_memory` says nothing about
    /// it. It is for the paths that still serve a request under their own
    /// lock -- each names itself where it makes its guard, and ROADMAP
    /// carries the work of turning it around. The invariant itself holds
    /// for them too: a copy that does wait panics in `none_held_at_switch`
    /// as before; it is only the deterministic check at every copy that
    /// such a path is not ready for.
    ///
    /// Made with the lock already held: interrupts are off then, so the
    /// CPU whose count it is does not change under it.
    pub struct UserMemoryUnchecked(());

    impl UserMemoryUnchecked {
        /// `why`: the path, for whoever reads it here.
        pub fn new(why: &'static str) -> Self {
            let _ = why;
            #[cfg(debug_assertions)]
            held::set_unchecked(held::unchecked().saturating_add(1));
            UserMemoryUnchecked(())
        }
    }

    impl Drop for UserMemoryUnchecked {
        fn drop(&mut self) {
            #[cfg(debug_assertions)]
            held::set_unchecked(held::unchecked().saturating_sub(1));
        }
    }

    /// The scheduler has made `thread` what CPU `cpu` runs: the thread
    /// lockdep names as the holder of the locks taken there from now on.
    /// `thread` is the thread's own number plus one (`sched`'s
    /// `lockdep_holder`), and this is called with interrupts off, where
    /// the scheduler records the same thread as the CPU's current one.
    pub fn running(cpu: usize, thread: u64) {
        #[cfg(debug_assertions)]
        held::set_current(cpu, thread);
        let _ = (cpu, thread);
    }

    /// The thread recorded as holding `class` on this CPU (0: not held).
    #[cfg(test)]
    fn holder_of(class: u16) -> u64 {
        held::with(|slots| slots.find(|c, _| c == class).map_or(0, |(_, by)| by))
    }

    #[cfg(test)]
    mod tests {
        use super::super::IrqMutex;
        use super::{MAX_CLASSES, MAX_HELD, NAME_MAX, class_of, edge_seen, held, holder_of, name};
        use std::boxed::Box;
        use std::{format, vec::Vec};

        /// Every class keeps its name, however many classes there are.
        /// The names used to live in a side table of 64, so the 65th
        /// class and up printed "?" in panics -- which is what hid the
        /// lock in the Office install's panic.
        #[test]
        fn every_class_keeps_its_name() {
            let mut classes = Vec::new();
            for i in 0..MAX_CLASSES / 2 {
                let want: &'static str = Box::leak(format!("class_{i}").into_boxed_str());
                classes.push((class_of(want), want));
            }
            for (class, want) in classes {
                assert_ne!(class, 0, "the class table filled up");
                assert_eq!(name(class).as_str(), want);
            }
        }

        /// A name too long for the room kept per class is cut and says so.
        #[test]
        fn a_name_longer_than_its_room_is_cut_not_dropped() {
            let want: &'static str = Box::leak("l".repeat(NAME_MAX + 20).into_boxed_str());
            let shown = format!("{}", name(class_of(want)));
            assert_eq!(shown, format!("{}..", &want[..NAME_MAX]));
        }

        /// Each held lock records the thread that took it, so a panic can
        /// say whose lock it is -- the point being the case where that is
        /// not the thread the panic happens in.
        #[test]
        fn each_held_lock_records_the_thread_that_took_it() {
            let mine: &'static str = "lockdep_holder_test";
            let lock = IrqMutex::named(mine, ());
            let class = class_of(mine);
            let guard = lock.lock();
            assert_eq!(holder_of(class), held::current());
            assert_ne!(holder_of(class), 0, "a held lock with no holder");
            drop(guard);
            assert_eq!(holder_of(class), 0, "the slot kept its holder after release");

            // Another thread's locks are its own: the holder tells them
            // apart, which is what the kernel's per-CPU slots cannot.
            let theirs: &'static str = "lockdep_holder_test_elsewhere";
            let (id, recorded) = std::thread::spawn(move || {
                let other = IrqMutex::named(theirs, ());
                let _held = other.lock();
                (held::current(), holder_of(class_of(theirs)))
            })
            .join()
            .unwrap();
            assert_eq!(id, recorded);
            assert_ne!(id, held::current());
        }

        /// Giving up the CPU while holding a lock is caught there, in
        /// the thread that holds it, not later in an innocent one.
        #[test]
        fn giving_up_the_cpu_while_holding_a_lock_panics() {
            let held = std::thread::spawn(|| {
                let lock = IrqMutex::named("lockdep_switch_test", ());
                let _guard = lock.lock();
                super::none_held_at_switch();
            })
            .join();
            let message = held.unwrap_err();
            let text = message.downcast_ref::<std::string::String>().unwrap();
            assert!(text.contains("gives up its CPU while holding lockdep_switch_test"), "{text}");
        }

        /// Locks released out of order leave exactly the others held,
        /// and the next lock records an order under each of those and
        /// none under the released one; a CPU's slots fill to MAX_HELD
        /// and are all free again after.
        #[test]
        fn locks_released_out_of_order_leave_the_rest_held() {
            std::thread::spawn(|| {
                let names: [&'static str; 4] =
                    ["lockdep_ooo_a", "lockdep_ooo_b", "lockdep_ooo_c", "lockdep_ooo_d"];
                let [a, b, c, d] = names.map(|n| IrqMutex::named(n, ()));
                let [ca, cb, cc, cd] = names.map(class_of);
                let (ga, gb, gc) = (a.lock(), b.lock(), c.lock());
                drop(gb);
                assert_eq!(holder_of(cb), 0, "the released lock is still held");
                assert_ne!(holder_of(ca), 0);
                assert_ne!(holder_of(cc), 0);
                let gd = d.lock();
                assert!(edge_seen(ca, cd) && edge_seen(cc, cd));
                assert!(!edge_seen(cb, cd), "an order under a lock no longer held");
                drop(ga);
                drop(gd);
                assert_eq!(holder_of(ca), 0);
                assert_eq!(holder_of(cd), 0);
                assert_ne!(holder_of(cc), 0);
                drop(gc);
                super::none_held_at_switch();

                // Every slot, released from the middle out.
                let many: Vec<&'static str> = (0..MAX_HELD)
                    .map(|i| -> &'static str {
                        Box::leak(format!("lockdep_ooo_{i}").into_boxed_str())
                    })
                    .collect();
                let locks: Vec<IrqMutex<()>> =
                    many.iter().map(|n| IrqMutex::named(n, ())).collect();
                let mut guards: Vec<Option<_>> = locks.iter().map(|l| Some(l.lock())).collect();
                for n in &many {
                    assert_ne!(holder_of(class_of(n)), 0, "{n} untracked");
                }
                for i in (0..MAX_HELD).map(|k| (k * 7 + 3) % MAX_HELD) {
                    guards[i] = None;
                    assert_eq!(holder_of(class_of(many[i])), 0, "{} still held", many[i]);
                }
                super::none_held_at_switch();
            })
            .join()
            .unwrap();
        }

        #[test]
        fn copying_a_program_s_memory_while_holding_a_lock_panics() {
            let held = std::thread::spawn(|| {
                let lock = IrqMutex::named("lockdep_usermem_test", ());
                let _guard = lock.lock();
                super::none_held_at_user_memory();
            })
            .join();
            let message = held.unwrap_err();
            let text = message.downcast_ref::<std::string::String>().unwrap();
            assert!(
                text.contains("copies user memory while holding lockdep_usermem_test"),
                "{text}"
            );
        }

        #[test]
        fn a_path_that_says_it_is_not_ready_may_copy_with_a_lock_held() {
            // Through, while its guard lives.
            std::thread::spawn(|| {
                let lock = IrqMutex::named("lockdep_unchecked_test", ());
                let _guard = lock.lock();
                let _unchecked = super::UserMemoryUnchecked::new("a path of this test's own");
                super::none_held_at_user_memory();
            })
            .join()
            .unwrap();
            // Checked again once the guard is gone.
            let held = std::thread::spawn(|| {
                let lock = IrqMutex::named("lockdep_unchecked_test", ());
                let _guard = lock.lock();
                drop(super::UserMemoryUnchecked::new("a path of this test's own"));
                super::none_held_at_user_memory();
            })
            .join();
            let message = held.unwrap_err();
            let text = message.downcast_ref::<std::string::String>().unwrap();
            assert!(
                text.contains("copies user memory while holding lockdep_unchecked_test"),
                "{text}"
            );
        }

        struct First;
        struct Second;
        struct Third;

        #[test]
        fn a_consistent_order_passes_and_an_inversion_panics() {
            let (a, b, c) = (IrqMutex::new(First), IrqMutex::new(Second), IrqMutex::new(Third));
            // First before Second, twice; Second alone; First, Third.
            for _ in 0..2 {
                let _x = a.lock();
                let _y = b.lock();
            }
            drop(b.lock());
            {
                let _x = a.lock();
                let _z = c.lock();
            }
            // Second, then First: the inverse of the order seen.
            let inverted = std::thread::spawn(move || {
                let _y = b.lock();
                let _x = a.lock();
            })
            .join();
            let message = inverted.unwrap_err();
            let text = message.downcast_ref::<std::string::String>().unwrap();
            assert!(text.contains("lockdep:") && text.contains("First") && text.contains("Second"));
        }
    }
}
