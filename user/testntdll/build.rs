//! Exports of functions written in assembly (`global_asm!` in seh.rs,
//! x64's, seh_arm64.rs, ARM64's, and x86.rs's arithmetic helpers), which
//! the cdylib's own export list does not cover.
fn main() {
    let names: &[&str] = match std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() {
        // The MSVC 64-bit arithmetic helpers (x86.rs).
        Ok("x86") => &[
            "_allmul",
            "_alldiv",
            "_allrem",
            "_aulldiv",
            "_aullrem",
            "_alldvrm",
            "_aulldvrm",
            "_allshl",
            "_allshr",
            "_aullshr",
        ],
        Ok("x86_64") => &[
            "RtlCaptureContext",
            "RtlRaiseException",
            "RtlUnwindEx",
            "RtlUnwind",
            "KiUserExceptionDispatcher",
        ],
        Ok("aarch64") => &[
            "RtlCaptureContext",
            "RtlRaiseException",
            "RtlUnwindEx",
            "RtlUnwind",
            "KiUserExceptionDispatcher",
        ],
        _ => &[],
    };
    for name in names {
        println!("cargo:rustc-cdylib-link-arg=/EXPORT:{name}");
    }
}
