//! Exports the linker does not take from `#[no_mangle]`: NdrClientCall2,
//! a `global_asm!` entry (`ndr.rs`: it spills the variadic arguments
//! before the interpreter reads them).
fn main() {
    println!("cargo:rustc-cdylib-link-arg=/EXPORT:NdrClientCall2");
}
