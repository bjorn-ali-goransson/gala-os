//! The DLL's preferred base is the executable's (0x400000, the i686
//! default), so the loader must relocate it: its HIGHLOW fixups are
//! exercised on Windows and on gala alike.
fn main() {
    println!("cargo:rustc-cdylib-link-arg=/BASE:0x400000");
}
