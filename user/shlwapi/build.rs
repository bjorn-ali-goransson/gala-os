//! shlwapi's functions that programs import by ordinal (Chromium imports
//! SHCreateMemStream as #12, QISearch as #219 and IsOS as #437, the
//! numbers every shlwapi keeps):
//! the exports carry them.
fn main() {
    let ordinals: &[(&str, u32)] = &[("SHCreateMemStream", 12), ("QISearch", 219), ("IsOS", 437)];
    // (rustc's .def names them too: lld warns about a duplicate export and
    // keeps this one, with its ordinal.)
    for (name, ordinal) in ordinals {
        println!("cargo:rustc-cdylib-link-arg=/EXPORT:{name},@{ordinal}");
    }
}
