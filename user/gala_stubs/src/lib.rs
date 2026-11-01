//! Exports that gala does not implement yet, for gala's user-mode DLLs.
//!
//! A program whose imports cannot all be bound does not load (Windows
//! refuses it, and so does gala's loader), so a DLL must export every
//! function that programs import even before gala implements it. Such a
//! stub cannot fail honestly in the function's own terms (it does not know
//! whether the function returns a BOOL, an HRESULT, a handle or a status),
//! so it never pretends: it reports `dll!function` to the kernel's
//! missing-features ledger ("not implemented") and ends the process with
//! STATUS_NOT_IMPLEMENTED. The ledger then names the function to implement
//! next (`unimplemented_test`).
//!
//! Usage, in a DLL's crate:
//!
//! ```ignore
//! gala_stubs::unimplemented_exports!("xolehlp.dll":
//!     DtcGetTransactionManagerExW
//! );
//! ```

#![no_std]

#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "system" {
    fn GalaRecordMissing(name: *const u8, length: usize, what: u32, detail: u64) -> u32;
    fn NtTerminateProcess(process: isize, status: u32) -> u32;
}

/// GalaRecordMissing's "not implemented".
const NOT_IMPLEMENTED: u32 = 3;
const STATUS_NOT_IMPLEMENTED: u32 = 0xc000_0002;

/// Reports `name` (`dll!function`) to the ledger and ends the process.
#[cold]
pub fn unimplemented(name: &str) -> ! {
    // SAFETY: gala's ledger service reads `name` (ASCII, its length);
    // NtTerminateProcess(NtCurrentProcess) does not return.
    unsafe {
        GalaRecordMissing(name.as_ptr(), name.len(), NOT_IMPLEMENTED, 0);
        NtTerminateProcess(-1, STATUS_NOT_IMPLEMENTED);
    }
    loop {
        core::hint::spin_loop();
    }
}

/// Defines an exported stub per name (see the crate documentation). The
/// stubs take no parameters: under the x64 calling convention the caller
/// owns its arguments, so any call is safe to receive. They live in the
/// `.gstub` section, which gala's ntdll knows: GetProcAddress does not
/// return them (a program probing for an optional function takes its
/// fallback), only static imports bind to them.
#[macro_export]
macro_rules! unimplemented_exports {
    ($dll:literal: $($name:ident)*) => {
        $(
            #[allow(non_snake_case)]
            #[unsafe(no_mangle)]
            #[unsafe(link_section = ".gstub")]
            pub extern "system" fn $name() -> ! {
                $crate::unimplemented(concat!($dll, "!", stringify!($name)))
            }
        )*
    };
}
