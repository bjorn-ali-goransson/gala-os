//! Exports of functions written in assembly (`global_asm!` in seh.rs),
//! which the cdylib's own export list does not cover.
fn main() {
    for name in [
        "RtlCaptureContext",
        "RtlRaiseException",
        "RtlUnwindEx",
        "RtlUnwind",
        "KiUserExceptionDispatcher",
    ] {
        println!("cargo:rustc-cdylib-link-arg=/EXPORT:{name}");
    }
}
