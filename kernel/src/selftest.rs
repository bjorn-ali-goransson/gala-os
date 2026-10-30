//! In-kernel tests that need real hardware state, run at boot when the
//! command line contains `selftest`. A failing test panics, which the test
//! harness sees as QEMU exit status 35.

use crate::arch::{interrupts, paging};
use alloc::vec::Vec;

use crate::mm;
use crate::mm::heap::{TryVec, try_box};

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
