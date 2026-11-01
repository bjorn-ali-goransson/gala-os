//! Exports SQL Server LocalDB imports that gala does not implement yet:
//! calling one reports `ntdll.dll!function` to the ledger and ends the
//! process. ntdll cannot use `gala_stubs` (it would import itself), so the
//! same stubs are made here, in the same `.gstub` section (which
//! LdrGetProcedureAddress does not return).

use super::{GalaRecordMissing, NtTerminateProcess};

#[cold]
fn unimplemented(name: &str) -> ! {
    GalaRecordMissing(name.as_ptr(), name.len(), 3, 0);
    NtTerminateProcess(-1, 0xc000_0002); // STATUS_NOT_IMPLEMENTED
    loop {
        core::hint::spin_loop();
    }
}

macro_rules! unimplemented_exports {
    ($($name:ident)*) => {
        $(
            #[allow(non_snake_case)]
            #[unsafe(no_mangle)]
            #[unsafe(link_section = ".gstub")]
            pub extern "system" fn $name() -> ! {
                unimplemented(concat!("ntdll.dll!", stringify!($name)))
            }
        )*
    };
}

unimplemented_exports!(
    NtSetEaFile
    RtlIpv4AddressToStringExW
    RtlIpv4StringToAddressExW
    RtlIpv6StringToAddressExW
);
