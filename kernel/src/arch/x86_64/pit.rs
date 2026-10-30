//! 8254 programmable interval timer, used only to calibrate other clocks.
//! Channel 2 is used because its output can be polled through port 0x61.

use super::port::Port;

pub const FREQUENCY_HZ: u64 = 1_193_182;

const CHANNEL2_DATA: u16 = 0x42;
const COMMAND: u16 = 0x43;
const CONTROL_61: u16 = 0x61;
const GATE2: u8 = 1 << 0;
const SPEAKER: u8 = 1 << 1;
const OUT2: u8 = 1 << 5;

fn ports() -> (Port<u8>, Port<u8>, Port<u8>) {
    // SAFETY: legacy PIT and system control ports; only the PIT channel 2
    // gate and speaker bits of port 0x61 are changed. Calibration runs
    // single-threaded during boot.
    unsafe { (Port::new(CHANNEL2_DATA), Port::new(COMMAND), Port::new(CONTROL_61)) }
}

/// Starts channel 2 counting down `ticks` in mode 0 (interrupt on terminal count).
pub fn start_oneshot(ticks: u16) {
    let (mut data, mut command, mut control) = ports();
    let c = control.read() & !(GATE2 | SPEAKER);
    control.write(c);
    command.write(0b1011_0000); // channel 2, lobyte/hibyte, mode 0, binary
    data.write(ticks as u8);
    data.write((ticks >> 8) as u8);
    control.write(c | GATE2); // rising gate edge starts the count
}

/// Waits for the one-shot to expire. Returns false if it never does.
pub fn wait_oneshot() -> bool {
    let (_, _, mut control) = ports();
    for _ in 0..100_000_000u64 {
        if control.read() & OUT2 != 0 {
            return true;
        }
        core::hint::spin_loop();
    }
    false
}
