fn main() {
    // Only the bare-metal kernel binary uses the linker script; host unit-test
    // builds of the library link normally.
    if std::env::var("TARGET").as_deref() == Ok("x86_64-unknown-none") {
        let dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
        println!("cargo:rustc-link-arg-bins=-T{dir}/linker.ld");
    }
    println!("cargo:rerun-if-changed=linker.ld");
}
