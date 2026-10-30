//! In-kernel tests that need real hardware state, run at boot when the
//! command line contains `selftest`. A failing test panics, which the test
//! harness sees as QEMU exit status 35.

use crate::arch::interrupts;

struct Test {
    name: &'static str,
    run: fn(),
}

const TESTS: &[Test] = &[Test {
    name: "breakpoint_returns",
    run: breakpoint_returns,
}];

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
