//! Exports of functions written in assembly (`global_asm!` in seh.rs and
//! seh_x86.rs), which the cdylib's own export list does not cover.
fn main() {
    let names: &[&str] = match std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() {
        Ok("x86") => &[],
        _ => &[
            "RtlCaptureContext",
            "RtlRaiseException",
            "RtlUnwindEx",
            "RtlUnwind",
            "KiUserExceptionDispatcher",
        ],
    };
    for name in names {
        println!("cargo:rustc-cdylib-link-arg=/EXPORT:{name}");
    }
}
