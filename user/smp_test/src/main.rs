//! Processor count and numbering, as on real Windows: NumberOfProcessors in
//! SystemBasicInformation matches its ActiveProcessorsAffinityMask, the
//! PEB and KUSER_SHARED_DATA; the process and thread affinity masks are
//! that mask; and the processor number in IA32_TSC_AUX (read with
//! `rdtscp`, as RtlGetCurrentProcessorNumber does) is below the count.
//! With `cpus=N` on the command line (gala's scenario passes the QEMU
//! `-smp` count) the count must be N. Prints the count and
//! `smp_test: ok`, or exits with the number of the failing check.

#![no_std]
#![no_main]

use ntrt::*;

const SYSTEM_BASIC_INFORMATION: u32 = 0;
const PROCESS_BASIC_INFORMATION: u32 = 0;
const THREAD_BASIC_INFORMATION: u32 = 0;
const CURRENT_THREAD: Handle = Handle(-2isize as usize);
const KUSER_ACTIVE_PROCESSOR_COUNT: usize = 0x7ffe_03c0;
const PEB_NUMBER_OF_PROCESSORS: usize = 0xb8;

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
        print("smp_test: check ");
        write_file(stdout(), &hex32(self.0));
        print(" got ");
        write_file(stdout(), &hex32(got as u32));
        print("\r\n");
        Err(self.0)
    }
}

fn u64_at(b: &[u8], off: usize) -> u64 {
    u64::from_le_bytes(b[off..off + 8].try_into().unwrap())
}

fn read_u32(addr: usize) -> u64 {
    // SAFETY: callers read the PEB and KUSER_SHARED_DATA, always mapped.
    u64::from(unsafe { (addr as *const u32).read_volatile() })
}

/// The command line (PEB->ProcessParameters->CommandLine).
fn command_line() -> &'static [u16] {
    // SAFETY: the loader always provides ProcessParameters and CommandLine.
    unsafe {
        let params = *(peb().add(0x20) as *const *const u8);
        let len = usize::from(*(params.add(0x70) as *const u16)) / 2;
        core::slice::from_raw_parts(*(params.add(0x78) as *const *const u16), len)
    }
}

/// N from a `cpus=N` argument, if there is one.
fn expected_cpus() -> Option<u64> {
    let line = command_line();
    let key: &[u16] = &wide!("cpus=");
    let at = line.windows(key.len()).position(|w| w == key)? + key.len();
    let mut n = 0u64;
    let mut digits = 0;
    for &c in &line[at..] {
        if !(u16::from(b'0')..=u16::from(b'9')).contains(&c) {
            break;
        }
        n = n * 10 + u64::from(c - u16::from(b'0'));
        digits += 1;
    }
    (digits > 0).then_some(n)
}

/// IA32_TSC_AUX through `rdtscp`.
fn tsc_aux() -> u32 {
    let aux: u32;
    // SAFETY: rdtscp only reads the TSC and TSC_AUX (x64 Windows requires
    // it on 24H2-capable CPUs).
    unsafe {
        core::arch::asm!("rdtscp", out("eax") _, out("edx") _, out("ecx") aux, options(nomem, nostack))
    };
    aux
}

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    let ok = u64::from(STATUS_SUCCESS);

    let mut b = [0u8; 64];
    c.eq(query_system(SYSTEM_BASIC_INFORMATION, &mut b).0.into(), ok)?;
    let cpus = u64::from(b[56]);
    let mask = u64_at(&b, 48);
    print("smp_test: ");
    write_file(stdout(), &hex32(cpus as u32));
    print(" processors\r\n");
    c.ok(cpus >= 1, cpus)?;
    c.eq(mask.count_ones().into(), cpus)?;
    if let Some(want) = expected_cpus() {
        c.eq(cpus, want)?;
    }
    c.eq(read_u32(peb() as usize + PEB_NUMBER_OF_PROCESSORS), cpus)?;
    c.eq(read_u32(KUSER_ACTIVE_PROCESSOR_COUNT), cpus)?;

    // PROCESS_BASIC_INFORMATION.AffinityMask, THREAD_BASIC_INFORMATION's.
    let mut p = [0u8; 48];
    c.eq(query_process(CURRENT_PROCESS, PROCESS_BASIC_INFORMATION, &mut p).0.into(), ok)?;
    c.eq(u64_at(&p, 16), mask)?;
    let mut t = [0u8; 48];
    c.eq(query_thread(CURRENT_THREAD, THREAD_BASIC_INFORMATION, &mut t).0.into(), ok)?;
    c.eq(u64_at(&t, 32), mask)?;

    // The processor number, sampled a few times (the thread may move).
    let mut highest = 0;
    for _ in 0..64 {
        highest = highest.max(u64::from(tsc_aux() & 0xfff));
        yield_execution();
    }
    c.ok(highest < cpus, highest)?;
    Ok(())
}

fn main() -> u32 {
    match run() {
        Ok(()) => {
            print("smp_test: ok\r\n");
            0
        }
        Err(id) => id,
    }
}

entry!(main);
