//! An embedded manifest that asks for `asInvoker`: Windows' installer
//! detection takes a 32-bit image without one whose name contains
//! "setup" for an installer and refuses to start it without elevation
//! (ERROR_ELEVATION_REQUIRED; measured on 26200, issue #59).
fn main() {
    println!("cargo:rustc-link-arg-bins=/MANIFEST:EMBED");
    println!("cargo:rustc-link-arg-bins=/MANIFESTUAC:level='asInvoker' uiAccess='false'");
}
