//! The app model's queries (the `api-ms-win-appmodel-runtime-l1-1-*` API
//! sets, which resolve to kernelbase and so here): gala's processes are
//! desktop programs, never packaged (MSIX/Store) apps. As Microsoft
//! documents them: the AppPolicy queries answer a desktop program's
//! policies (ExitProcess at the end, no Windows Runtime initialized for
//! new threads, developer diagnostics shown, the classic desktop window
//! model, every Media Foundation codec), ERROR_SUCCESS; the package
//! queries APPMODEL_ERROR_NO_PACKAGE, returned (not set as the last
//! error). The C++ runtime loads the API set for the policies (MSVC's
//! `__acrt_get_process_end_policy`); Office's Click-to-Run UI imports it.

use super::*;

const ERROR_SUCCESS: i32 = 0;
const APPMODEL_ERROR_NO_PACKAGE: i32 = 15700;

/// Writes `value` to the caller's policy (when there is one).
fn policy(out: *mut u32, value: u32) -> i32 {
    if out.is_null() {
        return ERROR_INVALID_PARAMETER as i32;
    }
    // SAFETY: the caller's policy variable.
    unsafe { *out = value };
    ERROR_SUCCESS
}

/// AppPolicyProcessTerminationMethod_ExitProcess.
#[unsafe(no_mangle)]
pub extern "system" fn AppPolicyGetProcessTerminationMethod(_token: Handle, out: *mut u32) -> i32 {
    policy(out, 0)
}

/// AppPolicyThreadInitializationType_None.
#[unsafe(no_mangle)]
pub extern "system" fn AppPolicyGetThreadInitializationType(_token: Handle, out: *mut u32) -> i32 {
    policy(out, 0)
}

/// AppPolicyShowDeveloperDiagnostic_ShowUI.
#[unsafe(no_mangle)]
pub extern "system" fn AppPolicyGetShowDeveloperDiagnostic(_token: Handle, out: *mut u32) -> i32 {
    policy(out, 1)
}

/// AppPolicyWindowingModel_ClassicDesktop.
#[unsafe(no_mangle)]
pub extern "system" fn AppPolicyGetWindowingModel(_token: Handle, out: *mut u32) -> i32 {
    policy(out, 2)
}

/// AppPolicyMediaFoundationCodecLoading_All.
#[unsafe(no_mangle)]
pub extern "system" fn AppPolicyGetMediaFoundationCodecLoading(
    _token: Handle,
    out: *mut u32,
) -> i32 {
    policy(out, 0)
}

#[unsafe(no_mangle)]
pub extern "system" fn GetCurrentPackageFullName(_length: *mut u32, _name: *mut u16) -> i32 {
    APPMODEL_ERROR_NO_PACKAGE
}

#[unsafe(no_mangle)]
pub extern "system" fn GetCurrentPackageFamilyName(_length: *mut u32, _name: *mut u16) -> i32 {
    APPMODEL_ERROR_NO_PACKAGE
}

#[unsafe(no_mangle)]
pub extern "system" fn GetCurrentPackagePath(_length: *mut u32, _path: *mut u16) -> i32 {
    APPMODEL_ERROR_NO_PACKAGE
}

#[unsafe(no_mangle)]
pub extern "system" fn GetCurrentPackageInfo(
    _flags: u32,
    _length: *mut u32,
    _buffer: *mut u8,
    _count: *mut u32,
) -> i32 {
    APPMODEL_ERROR_NO_PACKAGE
}

/// GetPackageFullName(hProcess, ...): no process of gala's is packaged.
#[unsafe(no_mangle)]
pub extern "system" fn GetPackageFullName(
    _process: Handle,
    _length: *mut u32,
    _name: *mut u16,
) -> i32 {
    APPMODEL_ERROR_NO_PACKAGE
}

#[unsafe(no_mangle)]
pub extern "system" fn GetPackageFamilyName(
    _process: Handle,
    _length: *mut u32,
    _name: *mut u16,
) -> i32 {
    APPMODEL_ERROR_NO_PACKAGE
}
