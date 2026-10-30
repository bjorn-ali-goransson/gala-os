//! In-kernel tests that need real hardware state, run at boot when the
//! command line contains `selftest`. A failing test panics, which the test
//! harness sees as QEMU exit status 35.

use crate::arch::{interrupts, paging};
use alloc::vec::Vec;

use crate::arch::cpu;
use crate::mm::heap::{TryVec, try_box};
use crate::sync::IrqMutex;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::{mm, sched, time};

struct Test {
    name: &'static str,
    run: fn(),
}

const TESTS: &[Test] = &[
    Test { name: "breakpoint_returns", run: breakpoint_returns },
    Test { name: "frame_alloc_roundtrip", run: frame_alloc_roundtrip },
    Test { name: "kernel_sections_mapped_with_permissions", run: kernel_sections_mapped },
    Test { name: "heap_small_and_large", run: heap_small_and_large },
    Test { name: "heap_exhaustion_is_an_error", run: heap_exhaustion_is_an_error },
    Test { name: "timer_ticks_advance", run: timer_ticks_advance },
    Test { name: "irq_mutex_masks_interrupts", run: irq_mutex_masks_interrupts },
    Test { name: "threads_run_and_join", run: threads_run_and_join },
    Test { name: "preemption_without_yield", run: preemption_without_yield },
    Test { name: "sleep_waits_long_enough", run: sleep_waits_long_enough },
    Test { name: "thread_stacks_are_reaped", run: thread_stacks_are_reaped },
    Test { name: "event_wakes_blocked_waiter", run: event_wakes_blocked_waiter },
];

pub fn run_all() {
    for test in TESTS {
        kprint!("selftest {} ... ", test.name);
        (test.run)();
        kprintln!("ok");
    }
    info!("selftest: {} passed", TESTS.len());
}

fn breakpoint_returns() {
    let before = interrupts::breakpoint_count();
    interrupts::breakpoint();
    interrupts::breakpoint();
    assert_eq!(interrupts::breakpoint_count(), before + 2);
}

fn frame_alloc_roundtrip() {
    let (free_before, _) = mm::frame_stats();
    let a = mm::allocate_frame().expect("frame");
    let b = mm::allocate_frame().expect("frame");
    assert_ne!(a, b);
    let run = mm::allocate_frames(16, 16).expect("16 contiguous frames");
    assert_eq!(run.number() % 16, 0);
    assert_eq!(mm::frame_stats().0, free_before - 18);
    mm::free_frame(a).unwrap();
    mm::free_frame(b).unwrap();
    for i in 0..16 {
        mm::free_frame(mm::frame::Frame::from_number(run.number() + i)).unwrap();
    }
    assert_eq!(mm::frame_stats().0, free_before);
    assert!(mm::free_frame(a).is_err(), "double free must be rejected");
}

fn kernel_sections_mapped() {
    let text = kernel_sections_mapped as fn() as usize as u64;
    let t = paging::translate(text).expect("own code is mapped");
    assert!(t.flags.executable && !t.flags.writable, "text must be RX: {:?}", t.flags);

    static RODATA: [u8; 4] = *b"gala";
    let t = paging::translate(RODATA.as_ptr() as u64).expect("rodata is mapped");
    assert!(!t.flags.executable && !t.flags.writable, "rodata must be RO+NX: {:?}", t.flags);

    static DATA: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0);
    let t = paging::translate(DATA.as_ptr() as u64).expect("data is mapped");
    assert!(!t.flags.executable && t.flags.writable, "data must be RW+NX: {:?}", t.flags);

    let stack_probe = 0u64;
    let t = paging::translate(&raw const stack_probe as u64).expect("stack is mapped");
    assert!(!t.flags.executable, "stack must be NX");

    assert_eq!(paging::translate(0x1000), None, "lower half must be unmapped");
    let image = paging::kernel_image();
    assert!(image.contains(&text));
}

fn heap_small_and_large() {
    let before = mm::heap::stats();
    {
        let boxed = try_box([7u64; 4]).expect("small box");
        assert_eq!(boxed.iter().sum::<u64>(), 28);
        let big = try_box([0x5au8; 16 * 1024]).expect("16 KiB box");
        assert!(big.iter().all(|&b| b == 0x5a));
        assert_eq!(big.as_ptr() as usize % 4096, 0);

        let mut v: Vec<u32> = Vec::new();
        for i in 0..10_000 {
            v.try_push(i).expect("push");
        }
        assert_eq!(v.iter().map(|&x| u64::from(x)).sum::<u64>(), 49_995_000);
        let aligned = try_box(Aligned([0; 256])).expect("aligned box");
        assert_eq!(&raw const *aligned as usize % 256, 0);
        assert_eq!(aligned.0[255], 0);
    }
    let after = mm::heap::stats();
    assert_eq!(after.in_use, before.in_use, "everything freed");
    assert_eq!(after.large_pages, before.large_pages);
}

#[repr(align(256))]
struct Aligned([u8; 256]);

fn heap_exhaustion_is_an_error() {
    let (free_before, _) = mm::frame_stats();
    let mut v: Vec<u8> = Vec::new();
    // More than all RAM: must fail cleanly instead of panicking.
    assert!(v.try_reserve(1 << 40).is_err());
    assert_eq!(mm::frame_stats().0, free_before, "failed reservation leaks nothing");
}

fn timer_ticks_advance() {
    assert!(cpu::interrupts_enabled());
    let (t0, ns0) = (time::ticks(), time::monotonic_ns());
    while time::ticks() < t0 + 20 {
        cpu::enable_interrupts_and_halt();
    }
    let elapsed_ms = (time::monotonic_ns() - ns0) / 1_000_000;
    // 20 ticks at 1 kHz = 20 ms. Emulation is imprecise, and an emulator
    // starved of host CPU (CI boots many VMs at once) delivers ticks late,
    // so only catch gross calibration errors: wrong units are 1000x off.
    assert!((5..=10_000).contains(&elapsed_ms), "20 ticks took {elapsed_ms} ms by the TSC");
}

fn irq_mutex_masks_interrupts() {
    static M: IrqMutex<u32> = IrqMutex::new(0);
    {
        let mut g = M.lock();
        assert!(!cpu::interrupts_enabled());
        *g += 1;
        let t = time::ticks();
        for _ in 0..1_000_000 {
            core::hint::spin_loop();
        }
        assert_eq!(time::ticks(), t, "no tick while the lock is held");
    }
    assert!(cpu::interrupts_enabled(), "interrupts restored on unlock");
}

fn threads_run_and_join() {
    static SUM: AtomicU64 = AtomicU64::new(0);
    fn worker(n: usize) {
        for _ in 0..100 {
            SUM.fetch_add(n as u64, Ordering::Relaxed);
            sched::yield_now();
        }
    }
    SUM.store(0, Ordering::Relaxed);
    let mut ids = [None; 8];
    for (n, slot) in ids.iter_mut().enumerate() {
        *slot = Some(sched::spawn("worker", worker, n + 1).expect("spawn"));
    }
    for id in ids.into_iter().flatten() {
        sched::join(id).expect("join");
    }
    assert_eq!(SUM.load(Ordering::Relaxed), 100 * (1..=8).sum::<u64>());
    assert_eq!(sched::join(sched::current()), Err(sched::JoinError::JoinSelf));
}

fn preemption_without_yield() {
    static STOP: AtomicBool = AtomicBool::new(false);
    static SPINS: AtomicU64 = AtomicU64::new(0);
    fn spinner(_: usize) {
        // Never yields: only the timer can take the CPU away.
        while !STOP.load(Ordering::Relaxed) {
            SPINS.fetch_add(1, Ordering::Relaxed);
        }
    }
    STOP.store(false, Ordering::Relaxed);
    let t = sched::spawn("spinner", spinner, 0).expect("spawn");
    // Sleeping hands the CPU to the spinner; we only get it back through
    // preemption.
    sched::sleep_ms(30);
    STOP.store(true, Ordering::Relaxed);
    sched::join(t).expect("join");
    assert!(SPINS.load(Ordering::Relaxed) > 0, "spinner ran");
}

fn sleep_waits_long_enough() {
    let start = time::monotonic_ns();
    sched::sleep_ms(25);
    let ms = (time::monotonic_ns() - start) / 1_000_000;
    assert!(ms >= 20, "slept only {ms} ms");
}

fn thread_stacks_are_reaped() {
    fn nothing(_: usize) {}
    sched::reap();
    let (free_before, _) = mm::frame_stats();
    for _ in 0..50 {
        let t = sched::spawn("short", nothing, 0).expect("spawn");
        sched::join(t).expect("join");
        while sched::reap() > 0 {}
    }
    // Thread records stay (ids are unique), but stacks must be returned.
    let leaked = free_before.saturating_sub(mm::frame_stats().0);
    assert!(leaked < 16, "{leaked} frames leaked by 50 threads");
}

fn event_wakes_blocked_waiter() {
    use crate::nt::object::{Event, EventType, KObject, ObjectBody, ObjectRef};
    use crate::nt::status::NtStatus;
    use crate::nt::wait::{self, Dispatcher, Timeout};

    static EVENT: crate::sync::IrqMutex<Option<ObjectRef>> = crate::sync::IrqMutex::new(None);
    static RESULT: AtomicU64 = AtomicU64::new(u64::MAX);
    fn waiter(_: usize) {
        let ev = EVENT.lock().clone().expect("event");
        let status = wait::wait_for(core::slice::from_ref(&ev), false, Timeout::Infinite);
        RESULT.store(u64::from(status.0), Ordering::SeqCst);
    }
    let ev = KObject::new(ObjectBody::Event(Event {
        kind: EventType::Synchronization,
        dispatcher: Dispatcher::new(0),
    }))
    .expect("event");
    *EVENT.lock() = Some(ev.clone());
    let t = sched::spawn("waiter", waiter, 0).expect("spawn");
    sched::sleep_ms(10);
    assert_eq!(RESULT.load(Ordering::SeqCst), u64::MAX, "waiter must still be blocked");
    wait::set_signal(&ev, 1);
    sched::join(t).expect("join");
    assert_eq!(RESULT.load(Ordering::SeqCst), u64::from(NtStatus::SUCCESS.0));
    assert_eq!(ev.dispatcher().unwrap().signal_state(), 0, "auto-reset consumed by the waiter");
    *EVENT.lock() = None;
}
