//! gala's bcryptprimitives.dll: ProcessPrng, the process-wide random
//! number generator (what the Go runtime and the C runtime use). Bytes come
//! from the CPU's hardware generator (RDRAND, falling back to RDSEED),
//! retried as Intel's guidance recommends. Like the real one, it never
//! fails: a CPU without either instruction ends the process (reported to
//! the missing-features ledger first), rather than return non-random data.

#![no_std]
#![allow(non_snake_case)]
// The export is a C-ABI function called by foreign code.
#![allow(clippy::not_unsafe_ptr_arg_deref)]

#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "system" {
    fn NtTerminateProcess(process: isize, status: u32) -> u32;
    fn GalaRecordMissing(name: *const u8, length: usize, what: u32, detail: u64) -> u32;
}

#[allow(dead_code)]
#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "C" {
    fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memset(dst: *mut u8, value: i32, n: usize) -> *mut u8;
}

/// One random word from RDRAND (10 tries, as recommended), else RDSEED.
fn random_word() -> Option<u64> {
    let ecx = core::arch::x86_64::__cpuid(1).ecx;
    if ecx & (1 << 30) != 0 {
        for _ in 0..10 {
            let (value, ok): (u64, u8);
            // SAFETY: RDRAND exists (CPUID.1:ECX[30]); it only writes the
            // register and the carry flag.
            unsafe {
                core::arch::asm!("rdrand {}", "setc {}", out(reg) value, out(reg_byte) ok, options(nomem, nostack))
            };
            if ok != 0 {
                return Some(value);
            }
        }
    }
    let ebx = core::arch::x86_64::__cpuid_count(7, 0).ebx;
    if ebx & (1 << 18) != 0 {
        for _ in 0..1000 {
            let (value, ok): (u64, u8);
            // SAFETY: RDSEED exists (CPUID.7.0:EBX[18]).
            unsafe {
                core::arch::asm!("rdseed {}", "setc {}", out(reg) value, out(reg_byte) ok, options(nomem, nostack))
            };
            if ok != 0 {
                return Some(value);
            }
            core::hint::spin_loop();
        }
    }
    None
}

/// ProcessPrng(Data, Length): fills `data` with random bytes; always TRUE.
#[unsafe(no_mangle)]
pub extern "system" fn ProcessPrng(data: *mut u8, length: usize) -> i32 {
    let mut i = 0;
    while i < length {
        let Some(word) = random_word() else {
            let name = b"bcryptprimitives.dll!ProcessPrng(no RDRAND/RDSEED)";
            // SAFETY: a valid buffer; then the process ends.
            unsafe {
                GalaRecordMissing(name.as_ptr(), name.len(), 2, 0);
                NtTerminateProcess(-1, 0xc000_0409);
            }
            loop {}
        };
        for b in word.to_le_bytes() {
            if i == length {
                break;
            }
            // SAFETY: the caller's buffer holds `length` bytes.
            unsafe { *data.add(i) = b };
            i += 1;
        }
    }
    1
}

/// DllMain (the entry point our link flags name).
#[unsafe(no_mangle)]
pub extern "system" fn mainCRTStartup(_instance: usize, _reason: u32, _reserved: usize) -> i32 {
    1
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {}
}
