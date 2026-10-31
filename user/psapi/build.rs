//! psapi.dll's exports: since Windows 7 the process status functions live
//! in kernel32 as K32*, and psapi.dll forwards its names to them
//! (documented on the psapi functions' pages: "Kernel32.dll on Windows 7
//! ... Psapi.dll if PSAPI_VERSION=1"). gala's kernel32 has the K32*
//! functions; these are forwarders to them (the loader resolves
//! `kernel32.K32...`).
fn main() {
    for name in [
        "EnumProcessModules",
        "GetModuleFileNameExA",
        "GetModuleFileNameExW",
        "GetModuleInformation",
        "GetProcessMemoryInfo",
    ] {
        println!("cargo:rustc-cdylib-link-arg=/EXPORT:{name}=kernel32.K32{name}");
    }
}
