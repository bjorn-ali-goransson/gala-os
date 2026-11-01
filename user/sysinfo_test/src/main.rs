//! System and process information that ntdll/kernel32 initialization reads,
//! with results expected to match real Windows (x64): NtQuerySystemInformation
//! SystemBasicInformation, SystemTimeOfDayInformation and
//! SystemQueryPerformanceCounterInformation (version 1; the KernelQpc
//! flag valid), and
//! NtQueryInformationProcess ProcessImageFileName, ProcessDebugPort,
//! ProcessWow64Information and ProcessSessionInformation. Prints
//! `sysinfo_test: ok` or exits with the number of the failing check.

#![no_std]
#![no_main]

use ntrt::*;

const STATUS_INFO_LENGTH_MISMATCH: NtStatus = 0xc000_0004;
const STATUS_INVALID_INFO_CLASS: NtStatus = 0xc000_0003;
const SYSTEM_BASIC_INFORMATION: u32 = 0;
const SYSTEM_TIME_OF_DAY_INFORMATION: u32 = 3;
const PROCESS_DEBUG_PORT: u32 = 7;
const PROCESS_SESSION_INFORMATION: u32 = 24;
const PROCESS_WOW64_INFORMATION: u32 = 26;
const PROCESS_IMAGE_FILE_NAME: u32 = 27;
const KUSER_ACTIVE_PROCESSOR_COUNT: usize = 0x7ffe_03c0;
const PEB_NUMBER_OF_PROCESSORS: usize = 0xb8;
const PEB_SESSION_ID: usize = 0x2c0;

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
        print("sysinfo_test: check ");
        write_file(stdout(), &hex32(self.0));
        print(" got ");
        write_file(stdout(), &hex32(got as u32));
        print("\r\n");
        Err(self.0)
    }
}

fn u32_at(b: &[u8], off: usize) -> u64 {
    u32::from_le_bytes(b[off..off + 4].try_into().unwrap()).into()
}

fn u64_at(b: &[u8], off: usize) -> u64 {
    u64::from_le_bytes(b[off..off + 8].try_into().unwrap())
}

fn read_u32(addr: usize) -> u64 {
    // SAFETY: callers read the PEB and KUSER_SHARED_DATA.
    unsafe { u64::from((addr as *const u32).read_volatile()) }
}

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    let ok = u64::from(STATUS_SUCCESS);

    // SystemBasicInformation (64 bytes on x64).
    let mut b = [0u8; 64];
    let (s, ret) = query_system(SYSTEM_BASIC_INFORMATION, &mut b);
    c.eq(s.into(), ok)?;
    c.eq(ret.into(), 64)?;
    c.eq(u32_at(&b, 8), 4096)?; // PageSize
    c.ok(u32_at(&b, 12) > 0, 0)?; // NumberOfPhysicalPages
    c.eq(u32_at(&b, 24), 0x1_0000)?; // AllocationGranularity
    c.eq(u64_at(&b, 32), 0x1_0000)?; // MinimumUserModeAddress
    c.eq(u64_at(&b, 40), 0x7fff_fffe_ffff)?; // MaximumUserModeAddress
    let cpus = u64::from(b[56]); // NumberOfProcessors
    c.ok(cpus >= 1, cpus)?;
    c.eq(u64_at(&b, 48), (1u64 << cpus) - 1)?; // ActiveProcessorsAffinityMask
    c.eq(read_u32(peb() as usize + PEB_NUMBER_OF_PROCESSORS), cpus)?;
    c.eq(read_u32(KUSER_ACTIVE_PROCESSOR_COUNT), cpus)?;
    c.eq(
        query_system(SYSTEM_BASIC_INFORMATION, &mut b[..63]).0.into(),
        STATUS_INFO_LENGTH_MISMATCH.into(),
    )?;
    let mut big = [0u8; 128];
    c.eq(
        query_system(SYSTEM_BASIC_INFORMATION, &mut big).0.into(),
        STATUS_INFO_LENGTH_MISMATCH.into(),
    )?;
    c.eq(query_system(0x7fff, &mut big).0.into(), STATUS_INVALID_INFO_CLASS.into())?;

    // SystemTimeOfDayInformation (48 bytes).
    let mut t = [0u8; 48];
    let (s, ret) = query_system(SYSTEM_TIME_OF_DAY_INFORMATION, &mut t);
    let (_, now) = system_time();
    c.eq(s.into(), ok)?;
    c.eq(ret.into(), 48)?;
    let (boot, current) = (u64_at(&t, 0), u64_at(&t, 8));
    c.ok(boot > 0 && boot < current, boot)?;
    c.ok(current <= now as u64 && now as u64 - current < 10_000_000, current)?; // within 1 s

    // SystemQueryPerformanceCounterInformation (124, 12 bytes): Version,
    // Flags, ValidFlags (bit 0: KernelQpc).
    let mut q = [0u8; 12];
    let (s, ret) = query_system(124, &mut q);
    c.eq(s.into(), ok)?;
    c.eq(ret.into(), 12)?;
    c.eq(u32_at(&q, 0), 1)?;
    c.eq(u32_at(&q, 8) & 1, 1)?;

    // ProcessImageFileName: UNICODE_STRING + NT path of our image.
    let mut name = [0u8; 600];
    let (s, ret) = query_process(CURRENT_PROCESS, PROCESS_IMAGE_FILE_NAME, &mut name);
    c.eq(s.into(), ok)?;
    let len = u32_at(&name, 0) as usize & 0xffff;
    c.eq(ret.into(), (16 + len + 2) as u64)?;
    let ptr = u64_at(&name, 8) as usize;
    c.eq(ptr as u64, name.as_ptr() as u64 + 16)?;
    // SAFETY: the string's buffer follows the header inside `name`.
    let path = unsafe { core::slice::from_raw_parts(ptr as *const u16, len / 2) };
    c.ok(ends_with_ci(path, "\\sysinfo_test.exe"), len as u64)?;
    let device = wide!("\\Device\\HarddiskVolume");
    c.ok(path.len() > device.len() && path[..device.len()] == device, 0)?;
    let (s, ret) = query_process(CURRENT_PROCESS, PROCESS_IMAGE_FILE_NAME, &mut name[..16]);
    c.ok(s != STATUS_SUCCESS, s.into())?;
    c.eq(ret.into(), (16 + len + 2) as u64)?;

    // ProcessDebugPort: no debugger.
    let mut port = [0xffu8; 8];
    c.eq(query_process(CURRENT_PROCESS, PROCESS_DEBUG_PORT, &mut port).0.into(), ok)?;
    c.eq(u64_at(&port, 0), 0)?;
    c.eq(
        query_process(CURRENT_PROCESS, PROCESS_DEBUG_PORT, &mut port[..4]).0.into(),
        STATUS_INFO_LENGTH_MISMATCH.into(),
    )?;

    // ProcessWow64Information: a native 64-bit process.
    let mut wow = [0xffu8; 8];
    c.eq(query_process(CURRENT_PROCESS, PROCESS_WOW64_INFORMATION, &mut wow).0.into(), ok)?;
    c.eq(u64_at(&wow, 0), 0)?;

    // ProcessSessionInformation: the PEB's session.
    let mut session = [0xffu8; 4];
    c.eq(query_process(CURRENT_PROCESS, PROCESS_SESSION_INFORMATION, &mut session).0.into(), ok)?;
    c.eq(u32_at(&session, 0), read_u32(peb() as usize + PEB_SESSION_ID))?;
    Ok(())
}

fn main() -> u32 {
    match run() {
        Ok(()) => {
            print("sysinfo_test: ok\r\n");
            0
        }
        Err(id) => id,
    }
}

entry!(main);
