//! A 32-bit (x86) program, run through WOW64 (ARCHITECTURE "Plan:
//! WOW64", ROADMAP WOW): what it sees of itself and a few NT services
//! called through the 32-bit system call path, with the results 64-bit
//! Windows of the pinned build gives a 32-bit program.
//!
//! - It runs in compatibility mode: CS is 0x23.
//! - TEB32 (`fs:`): Self, the stack (esp between StackLimit and
//!   StackBase), ClientId, the PEB, WOW32Reserved (the transition to
//!   64-bit code, a 32-bit address) set.
//! - PEB32: ImageBaseAddress is this image, the OS version 10.0.26100,
//!   VER_PLATFORM_WIN32_NT, NumberOfProcessors > 0, the console
//!   subsystem; the 32-bit process parameters are normalized and hold
//!   the image path and command line (`wow64_test.exe`), the current
//!   directory and an environment.
//! - NtQueryInformationProcess: ProcessBasicInformation (the 24-byte x86
//!   layout) gives this process's id (TEB32.ClientId); ProcessWow64Information
//!   (a 4-byte ULONG_PTR) the 32-bit PEB, which is non-zero for a WOW64
//!   process.
//! - Virtual memory: NtAllocateVirtualMemory gives 64 KiB below 4 GiB,
//!   usable; NtQueryVirtualMemory describes it (and this image, MEM_IMAGE)
//!   in the 28-byte x86 MEMORY_BASIC_INFORMATION; NtProtectVirtualMemory
//!   gives the old protection; NtFreeVirtualMemory releases it.
//! - NtQueryPerformanceCounter (64-bit values through 32-bit pointers).
//! - NtClose of an invalid handle: STATUS_INVALID_HANDLE.
//!
//! Prints `wow64_test: ok` and exits with 0, or exits with the number of
//! the failing check (`wow64_test: check N failed` first). With the
//! argument `crash` it writes to address 0 instead (an unhandled access
//! violation in 32-bit code).

#![no_std]
#![no_main]

use ntrt::*;

unsafe extern "C" {
    /// The linker-defined start of this image.
    static __ImageBase: u8;
}

const PROCESS_BASIC_INFORMATION: u32 = 0;
const PROCESS_WOW64_INFORMATION: u32 = 26;
const VER_PLATFORM_WIN32_NT: u32 = 2;
const IMAGE_SUBSYSTEM_WINDOWS_CUI: u32 = 3;
const RTL_USER_PROC_PARAMS_NORMALIZED: u32 = 1;
/// 32-bit user code's selector on 64-bit Windows.
const COMPAT_CS: u16 = 0x23;

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
        let mut line = *b"wow64_test: check 00000000 failed, got 00000000 00000000\n";
        line[18..26].copy_from_slice(&hex32(self.0));
        line[39..47].copy_from_slice(&hex32((got >> 32) as u32));
        line[48..56].copy_from_slice(&hex32(got as u32));
        if let Ok(text) = core::str::from_utf8(&line) {
            print(text);
        }
        Err(self.0)
    }
}

fn read_u32(addr: usize) -> u32 {
    // SAFETY: the callers read fields of the TEB, PEB and parameters,
    // which are mapped for the life of the process.
    unsafe { core::ptr::read_unaligned(addr as *const u32) }
}

fn read_u16(addr: usize) -> u16 {
    // SAFETY: as in `read_u32`.
    unsafe { core::ptr::read_unaligned(addr as *const u16) }
}

/// A UNICODE_STRING32 at `addr` as (length in bytes, buffer).
fn unicode(addr: usize) -> (usize, usize) {
    (usize::from(read_u16(addr)), read_u32(addr + 4) as usize)
}

/// Whether the UTF-16 string at `buffer` (`len` bytes) ends with `tail`,
/// ignoring ASCII case.
fn ends_with(len: usize, buffer: usize, tail: &str) -> bool {
    let chars = len / 2;
    if buffer == 0 || chars < tail.len() {
        return false;
    }
    let start = buffer + 2 * (chars - tail.len());
    tail.bytes().enumerate().all(|(i, b)| {
        let c = read_u16(start + 2 * i);
        c < 0x80 && (c as u8).eq_ignore_ascii_case(&b)
    })
}

/// Whether the UTF-16 string contains `needle` (ASCII, case ignored).
fn contains(len: usize, buffer: usize, needle: &str) -> bool {
    let chars = len / 2;
    (0..chars.saturating_sub(needle.len()) + 1).any(|at| {
        needle.bytes().enumerate().all(|(i, b)| {
            let c = read_u16(buffer + 2 * (at + i));
            c < 0x80 && (c as u8).eq_ignore_ascii_case(&b)
        })
    })
}

fn run(c: &mut Checks) -> Result<(), u32> {
    // Compatibility mode.
    c.eq(code_segment().into(), COMPAT_CS.into())?;

    // TEB32.
    let teb = teb_field(teb::SELF);
    c.ok(teb != 0, teb as u64)?;
    let (base, limit) = (teb_field(teb::STACK_BASE), teb_field(teb::STACK_LIMIT));
    let sp = stack_pointer();
    c.ok(limit < sp && sp < base, sp as u64)?;
    let pid = teb_field(teb::CLIENT_ID_PROCESS);
    c.ok(pid != 0 && pid.is_multiple_of(4), pid as u64)?;
    let tid = teb_field(teb::CLIENT_ID_THREAD);
    c.ok(tid != 0 && tid.is_multiple_of(4), tid as u64)?;
    let transition = teb_field(teb::WOW32_RESERVED);
    c.ok(transition != 0, transition as u64)?;

    // PEB32.
    let peb_addr = peb() as usize;
    c.ok(peb_addr != 0, peb_addr as u64)?;
    let image = &raw const __ImageBase as usize;
    c.eq(read_u32(peb_addr + peb::IMAGE_BASE_ADDRESS).into(), image as u64)?;
    c.eq(read_u32(peb_addr + peb::OS_MAJOR_VERSION).into(), 10)?;
    c.eq(read_u32(peb_addr + peb::OS_MINOR_VERSION).into(), 0)?;
    c.eq(read_u16(peb_addr + peb::OS_BUILD_NUMBER).into(), 26100)?;
    c.eq(read_u32(peb_addr + peb::OS_PLATFORM_ID).into(), VER_PLATFORM_WIN32_NT.into())?;
    let cpus = read_u32(peb_addr + peb::NUMBER_OF_PROCESSORS);
    c.ok(cpus > 0, cpus.into())?;
    c.eq(read_u32(peb_addr + peb::IMAGE_SUBSYSTEM).into(), IMAGE_SUBSYSTEM_WINDOWS_CUI.into())?;

    // The 32-bit process parameters.
    let pp = process_parameters() as usize;
    c.ok(pp != 0, pp as u64)?;
    let flags = read_u32(pp + params::FLAGS);
    c.ok(flags & RTL_USER_PROC_PARAMS_NORMALIZED != 0, flags.into())?;
    let (len, buf) = unicode(pp + params::IMAGE_PATH_NAME);
    c.ok(ends_with(len, buf, "\\wow64_test.exe"), len as u64)?;
    let (len, buf) = unicode(pp + params::COMMAND_LINE);
    c.ok(contains(len, buf, "wow64_test.exe"), len as u64)?;
    let (len, buf) = unicode(pp + params::CURRENT_DIRECTORY);
    c.ok(len >= 6 && read_u16(buf + 2) == u16::from(b':'), len as u64)?;
    let env = read_u32(pp + params::ENVIRONMENT) as usize;
    c.ok(env != 0 && read_u16(env) != 0, env as u64)?;

    // NtQueryInformationProcess.
    let mut info = [0u8; 24];
    let (status, returned) = query_process(CURRENT_PROCESS, PROCESS_BASIC_INFORMATION, &mut info);
    c.eq(status.into(), STATUS_SUCCESS.into())?;
    c.eq(returned.into(), 24)?;
    let field =
        |off: usize| u32::from_le_bytes([info[off], info[off + 1], info[off + 2], info[off + 3]]);
    c.eq(field(0x10).into(), pid as u64)?;
    c.ok(field(4) != 0, field(4).into())?;
    let mut wow = [0u8; 4];
    let (status, returned) = query_process(CURRENT_PROCESS, PROCESS_WOW64_INFORMATION, &mut wow);
    c.eq(status.into(), STATUS_SUCCESS.into())?;
    c.eq(returned.into(), 4)?;
    c.eq(u32::from_le_bytes(wow).into(), peb_addr as u64)?;

    // Virtual memory.
    let (status, base, size) = allocate(0, 0x1_0000, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE);
    c.eq(status.into(), STATUS_SUCCESS.into())?;
    c.ok(base != 0 && base.is_multiple_of(0x1_0000), base as u64)?;
    c.eq(size as u64, 0x1_0000)?;
    // SAFETY: the allocation is committed and writable.
    unsafe {
        core::ptr::write_volatile((base + 0x1234) as *mut u32, 0x5157_3634);
        c.eq(core::ptr::read_volatile((base + 0x1234) as *const u32).into(), 0x5157_3634)?;
    }
    let (status, mbi, returned) = query(base + 0x2000);
    c.eq(status.into(), STATUS_SUCCESS.into())?;
    c.eq(returned as u64, 28)?;
    c.eq(mbi.base_address as u64, (base + 0x2000) as u64)?;
    c.eq(mbi.allocation_base as u64, base as u64)?;
    c.eq(mbi.region_size as u64, 0xe000)?;
    c.eq(mbi.state.into(), MEM_COMMIT.into())?;
    c.eq(mbi.protect.into(), PAGE_READWRITE.into())?;
    let (status, old) = protect(base, 0x1000, PAGE_READONLY);
    c.eq(status.into(), STATUS_SUCCESS.into())?;
    c.eq(old.into(), PAGE_READWRITE.into())?;
    let (status, freed_base, freed_size) = free(base, 0, MEM_RELEASE);
    c.eq(status.into(), STATUS_SUCCESS.into())?;
    c.eq(freed_base as u64, base as u64)?;
    c.eq(freed_size as u64, 0x1_0000)?;
    let (status, mbi, _) = query(image);
    c.eq(status.into(), STATUS_SUCCESS.into())?;
    c.eq(mbi.allocation_base as u64, image as u64)?;
    c.eq(mbi.kind.into(), MEM_IMAGE.into())?;

    // 64-bit results through 32-bit pointers.
    let (status, counter, frequency) = performance_counter();
    c.eq(status.into(), STATUS_SUCCESS.into())?;
    c.ok(counter > 0 && frequency > 0, frequency as u64)?;

    c.eq(close(Handle(0x1234_5678)).into(), STATUS_INVALID_HANDLE.into())?;
    Ok(())
}

fn main() -> u32 {
    // `wow64_test crash`: an unhandled access violation in 32-bit code
    // (scenario wow64_crash: the crash report's 32-bit registers).
    let pp = process_parameters() as usize;
    let (len, buf) = unicode(pp + params::COMMAND_LINE);
    if ends_with(len, buf, " crash") {
        print("wow64_test: writing to address 0\n");
        // SAFETY: deliberately faults; the process ends.
        unsafe { core::ptr::write_volatile(core::ptr::null_mut::<u32>(), 1) };
    }
    let mut c = Checks(0);
    match run(&mut c) {
        Ok(()) => {
            print("wow64_test: ok\n");
            0
        }
        Err(n) => n,
    }
}

entry!(main);
