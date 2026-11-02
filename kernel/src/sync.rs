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
    use core::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};

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

    /// The classes' names (`type_name` pointer and length), id = index + 1.
    #[cfg(debug_assertions)]
    static NAMES: [AtomicUsize; MAX_CLASSES] = [const { AtomicUsize::new(0) }; MAX_CLASSES];
    #[cfg(debug_assertions)]
    static LENS: [AtomicUsize; MAX_CLASSES] = [const { AtomicUsize::new(0) }; MAX_CLASSES];
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
                        LENS[i].store(len, Ordering::Release);
                        NAME_STRS.note(name);
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

    #[cfg(debug_assertions)]
    fn name(class: u16) -> &'static str {
        let i = usize::from(class).saturating_sub(1).min(MAX_CLASSES - 1);
        let (ptr, len) = (NAMES[i].load(Ordering::Acquire), LENS[i].load(Ordering::Acquire));
        // The pointer and length of a `&'static str` from `type_name`,
        // stored as numbers: rebuild it without unsafe code from a table
        // of the names seen.
        NAME_STRS.iter().find(|(p, l, _)| *p == ptr && *l == len).map_or("?", |(_, _, s)| s)
    }

    /// Names kept as `&'static str` (filled as classes appear; up to
    /// MAX_CLASSES, behind a tiny spinlock of its own).
    #[cfg(debug_assertions)]
    static NAME_STRS: NameTable = NameTable::new();

    /// (pointer, length, name) of each class seen, and how many.
    #[cfg(debug_assertions)]
    type Names = ([(usize, usize, &'static str); 64], usize);

    #[cfg(debug_assertions)]
    struct NameTable {
        busy: AtomicBool,
        cell: spin::Mutex<Names>,
    }

    #[cfg(debug_assertions)]
    impl NameTable {
        const fn new() -> Self {
            NameTable {
                busy: AtomicBool::new(false),
                cell: spin::Mutex::new(([(0, 0, ""); 64], 0)),
            }
        }

        fn note(&self, s: &'static str) {
            if self.busy.swap(true, Ordering::Acquire) {
                return; // names are for messages only: skip when contended
            }
            if let Some(mut t) = self.cell.try_lock() {
                let key = (s.as_ptr() as usize, s.len());
                let n = t.1;
                if !t.0[..n].iter().any(|(p, l, _)| (*p, *l) == key) && n < t.0.len() {
                    t.0[n] = (key.0, key.1, s);
                    t.1 = n + 1;
                }
            }
            self.busy.store(false, Ordering::Release);
        }

        fn iter(&self) -> impl Iterator<Item = (usize, usize, &'static str)> {
            let copy = self.cell.try_lock().map(|t| (t.0, t.1));
            let (items, n) = copy.unwrap_or(([(0, 0, ""); 64], 0));
            (0..n).map(move |i| items[i])
        }
    }

    /// The edge table is open-addressed: a key's slots start at its hash
    /// and go on until it or an empty slot (keys are never removed).
    #[cfg(debug_assertions)]
    fn slots(key: u32) -> impl Iterator<Item = &'static AtomicU32> {
        let start = (key.wrapping_mul(0x9e37_79b1) as usize) % MAX_EDGES;
        (0..MAX_EDGES).map(move |i| &EDGES[(start + i) % MAX_EDGES])
    }

    #[cfg(debug_assertions)]
    fn edge_seen(a: u16, b: u16) -> bool {
        let key = (u32::from(a) << 16) | u32::from(b);
        for e in slots(key) {
            match e.load(Ordering::Acquire) {
                0 => return false,
                k if k == key => return true,
                _ => {}
            }
        }
        false
    }

    #[cfg(debug_assertions)]
    fn add_edge(a: u16, b: u16) {
        let key = (u32::from(a) << 16) | u32::from(b);
        for e in slots(key) {
            match e.compare_exchange(0, key, Ordering::AcqRel, Ordering::Acquire) {
                Ok(_) => return,
                Err(k) if k == key => return,
                Err(_) => {}
            }
        }
    }

    /// The classes this CPU (thread) holds.
    #[cfg(all(debug_assertions, not(any(test, feature = "hosted"))))]
    mod held {
        use super::MAX_HELD;
        use core::sync::atomic::{AtomicU16, Ordering};
        const CPUS: usize = crate::arch::smp::MAX_CPUS;
        static HELD: [[AtomicU16; MAX_HELD]; CPUS] =
            [const { [const { AtomicU16::new(0) }; MAX_HELD] }; CPUS];

        /// Runs `f` on this CPU's slots (interrupts are off while any
        /// IrqMutex is held, so the CPU does not change under it).
        pub fn with<R>(f: impl FnOnce(&[AtomicU16; MAX_HELD]) -> R) -> R {
            let cpu = (crate::arch::percpu::cpu_index() as usize).min(CPUS - 1);
            f(&HELD[cpu])
        }

        pub fn get(slot: &AtomicU16) -> u16 {
            slot.load(Ordering::Relaxed)
        }

        pub fn set(slot: &AtomicU16, v: u16) {
            slot.store(v, Ordering::Relaxed);
        }
    }

    #[cfg(all(debug_assertions, any(test, feature = "hosted")))]
    mod held {
        use super::MAX_HELD;
        use core::sync::atomic::{AtomicU16, Ordering};
        std::thread_local! {
            static HELD: [AtomicU16; MAX_HELD] = const { [const { AtomicU16::new(0) }; MAX_HELD] };
        }

        pub fn with<R>(f: impl FnOnce(&[AtomicU16; MAX_HELD]) -> R) -> R {
            HELD.with(|h| f(h))
        }

        pub fn get(slot: &AtomicU16) -> u16 {
            slot.load(Ordering::Relaxed)
        }

        pub fn set(slot: &AtomicU16, v: u16) {
            slot.store(v, Ordering::Relaxed);
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
        held::with(|slots| {
            for s in slots.iter() {
                let a = held::get(s);
                if a == 0 || a == class {
                    continue;
                }
                if check && edge_seen(class, a) {
                    panic!(
                        "lockdep: {} taken while holding {}, but {} was taken while holding {} before",
                        name(class),
                        name(a),
                        name(a),
                        name(class)
                    );
                }
                add_edge(a, class);
            }
            match slots.iter().find(|s| held::get(s) == 0) {
                Some(free) => {
                    held::set(free, class);
                    class
                }
                None => 0, // nested too deep to track
            }
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
            held::with(|slots| {
                if let Some(s) = slots.iter().rev().find(|s| held::get(s) == class) {
                    held::set(s, 0);
                }
            });
        }
        #[cfg(not(debug_assertions))]
        let _ = class;
    }

    #[cfg(test)]
    mod tests {
        use super::super::IrqMutex;

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
