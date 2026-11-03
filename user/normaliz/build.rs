//! normaliz.dll held the IDN functions on old Windows versions; on
//! current ones they are kernel32's, and programs built against
//! normaliz.lib (libcurl) still import them from here. IdnToAscii
//! forwards to kernel32.dll; IdnToUnicode is normaliz's own (src/lib.rs)
//! until gala's kernel32 has it. (NormalizeString and IsNormalizedString
//! would join them once kernel32 has them.)
fn main() {
    println!("cargo:rustc-cdylib-link-arg=/EXPORT:IdnToAscii=kernel32.IdnToAscii");
}
