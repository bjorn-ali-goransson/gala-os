//! normaliz.dll held the IDN functions on old Windows versions; on
//! current ones they are kernel32's, and programs built against
//! normaliz.lib (libcurl) still import them from here. normaliz also
//! held the normalization functions, which Office's wwlib imports from
//! kernel32 (ROADMAP OF). IdnToAscii, NormalizeString and
//! IsNormalizedString forward to kernel32.dll; IdnToUnicode is
//! normaliz's own (src/lib.rs) until gala's kernel32 has it.
fn main() {
    for name in ["IdnToAscii", "NormalizeString", "IsNormalizedString"] {
        println!("cargo:rustc-cdylib-link-arg=/EXPORT:{name}=kernel32.{name}");
    }
}
