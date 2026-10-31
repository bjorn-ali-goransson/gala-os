//! Export forwarders: these kernel32 exports name ntdll's functions, as on
//! Windows (the loader follows them; defining them here would shadow the
//! ntdll imports of the same name).
fn main() {
    for name in [
        "RtlLookupFunctionEntry",
        "RtlVirtualUnwind",
        "RtlPcToFileHeader",
        "RtlCaptureContext",
        "RtlUnwind",
        "RtlUnwindEx",
        "RtlRestoreContext",
        "RtlAddFunctionTable",
        "RtlDeleteFunctionTable",
    ] {
        println!("cargo:rustc-cdylib-link-arg=/EXPORT:{name}=ntdll.{name}");
    }
}
