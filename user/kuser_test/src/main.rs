//! KUSER_SHARED_DATA at 0x7FFE0000 (public layout from Microsoft's
//! documentation / ntddk.h), with values expected on the pinned build
//! (Windows 11 24H2, 26100). Prints `kuser_test: ok` or exits with the
//! number of the failing check.

#![no_std]
#![no_main]

use ntrt::*;

const KUSER: usize = 0x7ffe_0000;

fn u8_at(off: usize) -> u8 {
    // SAFETY: KUSER_SHARED_DATA is always mapped readable in user mode.
    unsafe { ((KUSER + off) as *const u8).read_volatile() }
}

fn u16_at(off: usize) -> u16 {
    // SAFETY: as above.
    unsafe { ((KUSER + off) as *const u16).read_volatile() }
}

fn u32_at(off: usize) -> u32 {
    // SAFETY: as above.
    unsafe { ((KUSER + off) as *const u32).read_volatile() }
}

fn u64_at(off: usize) -> u64 {
    // SAFETY: as above.
    unsafe { ((KUSER + off) as *const u64).read_volatile() }
}

/// Reads a KSYSTEM_TIME { LowPart, High1Time, High2Time } consistently.
fn ksystem_time(off: usize) -> u64 {
    loop {
        let high1 = u32_at(off + 4);
        let low = u32_at(off);
        let high2 = u32_at(off + 8);
        if high1 == high2 {
            return u64::from(high1) << 32 | u64::from(low);
        }
    }
}

struct Checks(u32);

impl Checks {
    fn eq(&mut self, got: u64, want: u64) -> Result<(), u32> {
        self.ok(got == want, got)
    }

    fn ok(&mut self, cond: bool, got: u64) -> Result<(), u32> {
        self.0 += 1;
        if cond {
            return Ok(());
        }
        print("kuser_test: check ");
        write_file(stdout(), &hex32(self.0));
        print(" got ");
        write_file(stdout(), &hex32(got as u32));
        print("\r\n");
        Err(self.0)
    }
}

fn run() -> Result<(), u32> {
    let mut c = Checks(0);

    // A read-only page.
    let (st, mbi) = query(KUSER);
    c.eq(st.into(), STATUS_SUCCESS.into())?;
    c.eq(mbi.state.into(), MEM_COMMIT.into())?;
    c.eq(mbi.protect.into(), PAGE_READONLY.into())?;
    c.eq(mbi.region_size as u64, 0x1000)?;

    // Version and platform.
    c.eq(u16_at(0x2c).into(), 0x8664)?; // ImageNumberLow
    c.eq(u16_at(0x2e).into(), 0x8664)?; // ImageNumberHigh
    c.eq(u32_at(0x26c).into(), 10)?; // NtMajorVersion
    c.eq(u32_at(0x270).into(), 0)?; // NtMinorVersion
    c.eq(u32_at(0x260).into(), 26100)?; // NtBuildNumber
    c.eq(u32_at(0x264).into(), 1)?; // NtProductType = NtProductWinNt
    c.eq(u8_at(0x268).into(), 1)?; // ProductTypeIsValid
    c.eq(u16_at(0x26a).into(), 9)?; // NativeProcessorArchitecture = AMD64
    let root = wide!("C:\\Windows");
    for (i, &ch) in root.iter().enumerate() {
        c.eq(u16_at(0x30 + 2 * i).into(), ch.into())?; // NtSystemRoot
    }
    c.eq(u16_at(0x30 + 2 * root.len()).into(), 0)?;
    c.ok(u32_at(0x2e8) > 0, u32_at(0x2e8).into())?; // NumberOfPhysicalPages
    c.ok(u32_at(0x3c0) >= 1, u32_at(0x3c0).into())?; // ActiveProcessorCount

    // Clocks.
    let (_, frequency) = performance_counter();
    c.eq(u64_at(0x300), frequency as u64)?; // QpcFrequency
    let (_, now) = system_time();
    let shared = ksystem_time(0x14); // SystemTime
    let diff = (now as u64).abs_diff(shared);
    c.ok(diff < 10_000_000, diff)?; // within a second
    let interrupt0 = ksystem_time(0x08); // InterruptTime
    let ticks0 = u64_at(0x320); // TickCountQuad
    let multiplier = u64::from(u32_at(0x04)); // TickCountMultiplier
    c.ok(multiplier != 0, multiplier)?;
    delay(relative_ms(20));
    let interrupt1 = ksystem_time(0x08);
    let ticks1 = u64_at(0x320);
    let waited_100ns = interrupt1 - interrupt0;
    c.ok(waited_100ns >= 150_000, waited_100ns)?; // >= 15 ms
    let waited_ms = ((ticks1 - ticks0) * multiplier) >> 24;
    c.ok(waited_ms >= 15, waited_ms)?;
    Ok(())
}

fn main() -> u32 {
    match run() {
        Ok(()) => {
            print("kuser_test: ok\r\n");
            0
        }
        Err(id) => id,
    }
}

entry!(main);
