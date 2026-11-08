//! Which of kernel32 and kernelbase export a name, as measured on Windows
//! 11 25H2 (build 26200, x64; GetProcAddress of each name from both DLLs,
//! issue #15). Shared by kernel32_test and wow64_k32_test (`#[path]`); the
//! 32-bit DLLs are taken to split the same way until a 32-bit run says
//! otherwise.

use core::ffi::CStr;

/// kernelbase's alone: kernel32's GetProcAddress is NULL.
pub const KERNELBASE_ONLY: &[&CStr] = &[
    c"PathCchAppend",
    c"PathCchCombine",
    c"PathCchCombineEx",
    c"PathCchRemoveBackslash",
    c"PathCchRemoveExtension",
    c"PathCchRemoveFileSpec",
    c"PathCchSkipRoot",
    c"WaitOnAddress",
    c"WakeByAddressAll",
    c"WakeByAddressSingle",
    c"QueryInterruptTime",
    c"QueryInterruptTimePrecise",
    c"QueryUnbiasedInterruptTimePrecise",
];

/// kernel32's alone: kernelbase's GetProcAddress is NULL.
pub const KERNEL32_ONLY: &[&CStr] = &[c"MapUserPhysicalPagesScatter"];

/// In both, kernel32's a forwarder to kernelbase's: the same address.
pub const FORWARDED: &[&CStr] = &[c"FlsGetValue2", c"TlsGetValue2"];

/// In both, each DLL with its own entry (the addresses differ on Windows;
/// only the presence is asserted).
pub const BOTH: &[&CStr] = &[
    c"CheckTokenMembershipEx",
    c"GetEnabledXStateFeatures",
    c"GetXStateFeaturesMask",
    c"LocateXStateFeature",
    c"SetXStateFeaturesMask",
    c"SetFileIoOverlappedRange",
];

/// What one DLL gives for a name: GetProcAddress's address and whether
/// the name is in its export directory (gala's stubs of functions it does
/// not implement yet are there, but GetProcAddress does not return them).
#[derive(Clone, Copy)]
pub struct Found {
    pub address: usize,
    pub exported: bool,
}

/// Whether the image mapped at `base` names `name` in its export
/// directory (PE32 or PE32+).
///
/// # Safety
/// `base` is a mapped image.
pub unsafe fn exported(base: usize, name: &CStr) -> bool {
    let at = |offset: usize| -> u32 {
        // SAFETY: within the mapped image (the caller's).
        unsafe { ((base + offset) as *const u32).read_unaligned() }
    };
    let nt = at(0x3c) as usize;
    let optional = nt + 24;
    // SAFETY: as above.
    let magic = unsafe { ((base + optional) as *const u16).read_unaligned() };
    let directory = optional + if magic == 0x20b { 0x70 } else { 0x60 };
    let exports = at(directory) as usize;
    if exports == 0 {
        return false;
    }
    let count = at(exports + 0x18) as usize;
    let names = at(exports + 0x20) as usize;
    let want = name.to_bytes_with_nul();
    (0..count).any(|i| {
        let text = base + at(names + 4 * i) as usize;
        // SAFETY: export names are NUL-terminated strings in the image;
        // the comparison stops at the first difference or the NUL.
        want.iter().enumerate().all(|(j, &c)| unsafe { *((text + j) as *const u8) } == c)
    })
}

/// The first name whose exports differ from the measurement, with what
/// each DLL gave (`k32` and `kb` look a name up in kernel32 and
/// kernelbase); None when all agree.
pub fn first_mismatch(
    k32: impl Fn(&CStr) -> Found,
    kb: impl Fn(&CStr) -> Found,
) -> Option<(&'static CStr, Found, Found)> {
    /// Whether kernel32's and kernelbase's answers agree with a table's.
    type Rule = fn(Found, Found) -> bool;
    fn absent(f: Found) -> bool {
        f.address == 0 && !f.exported
    }
    let tables: [(&[&'static CStr], Rule); 4] = [
        (KERNELBASE_ONLY, |a, b| absent(a) && b.exported),
        (KERNEL32_ONLY, |a, b| a.exported && absent(b)),
        (FORWARDED, |a, b| a.address != 0 && a.address == b.address),
        (BOTH, |a, b| a.exported && b.exported),
    ];
    for (names, agrees) in tables {
        for &name in names {
            let (a, b) = (k32(name), kb(name));
            if !agrees(a, b) {
                return Some((name, a, b));
            }
        }
    }
    None
}
