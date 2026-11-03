//! 8254 programmable interval timer, used only to calibrate other clocks
//! when the machine has neither an HPET nor an ACPI PM timer
//! (`refclock`): channel 2 (gated through port 0x61, speaker off) as a
//! free-running counter.

use super::port::Port;

pub const FREQUENCY_HZ: u64 = 1_193_182;

const CHANNEL2_DATA: u16 = 0x42;
const COMMAND: u16 = 0x43;
const CONTROL_61: u16 = 0x61;
const GATE2: u8 = 1 << 0;
const SPEAKER: u8 = 1 << 1;

fn ports() -> (Port<u8>, Port<u8>, Port<u8>) {
    // SAFETY: legacy PIT and system control ports; only the PIT channel 2
    // gate and speaker bits of port 0x61 are changed. Calibration runs
    // single-threaded during boot.
    unsafe { (Port::new(CHANNEL2_DATA), Port::new(COMMAND), Port::new(CONTROL_61)) }
}

/// Starts channel 2 counting down from 65536 for ever (mode 2, rate
/// generator), for `read_count_2`; the speaker stays off.
pub fn start_free_running() {
    let (mut data, mut command, mut control) = ports();
    let c = control.read() & !(GATE2 | SPEAKER);
    control.write(c);
    command.write(0b1011_0100); // channel 2, lobyte/hibyte, mode 2, binary
    data.write(0);
    data.write(0);
    control.write(c | GATE2);
}

/// Channel 2's current count, latched by `latch` (the latch command).
pub fn read_count_2(latch: u8) -> u16 {
    let (mut data, mut command, _) = ports();
    command.write(latch);
    let low = data.read();
    let high = data.read();
    u16::from_le_bytes([low, high])
}
