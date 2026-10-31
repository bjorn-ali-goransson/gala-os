//! A 3 MiB stack reservation in the header: the main thread and threads
//! created with a stack size of 0 get it (checked by the test).
fn main() {
    println!("cargo:rustc-link-arg=/STACK:0x300000");
}
