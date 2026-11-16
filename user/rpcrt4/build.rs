//! Exports the linker does not take from `#[no_mangle]`: NdrClientCall2,
//! a `global_asm!` entry (`ndr.rs`: it spills the variadic arguments
//! before the interpreter reads them), x64's and ARM64's.
fn main() {
    let arch = std::env::var("CARGO_CFG_TARGET_ARCH");
    if matches!(arch.as_deref(), Ok("x86_64" | "aarch64")) {
        println!("cargo:rustc-cdylib-link-arg=/EXPORT:NdrClientCall2");
    }
}
