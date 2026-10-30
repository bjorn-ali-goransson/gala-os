//! In-kernel tests that need real hardware state, run at boot when the
//! command line contains `selftest`. A failing test panics, which the test
//! harness sees as QEMU exit status 35.

use crate::arch::{interrupts, paging};
use crate::mm;

struct Test {
    name: &'static str,
    run: fn(),
}

const TESTS: &[Test] = &[
    Test { name: "breakpoint_returns", run: breakpoint_returns },
    Test { name: "frame_alloc_roundtrip", run: frame_alloc_roundtrip },
    Test { name: "kernel_sections_mapped_with_permissions", run: kernel_sections_mapped },
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
