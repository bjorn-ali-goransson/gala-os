//! msi.dll's functions that programs import by ordinal (the Office
//! Deployment Tool's setup.exe imports every msi function so): the
//! exports carry Windows' ordinals, as Wine's msi.spec lists them.
fn main() {
    let ordinals: &[(&str, u32)] = &[
        ("MsiCloseHandle", 8),
        ("MsiEnumComponentQualifiersW", 39),
        ("MsiEnumProductsW", 45),
        ("MsiLocateComponentW", 90),
        ("MsiQueryProductStateW", 113),
        ("MsiSetInternalUI", 141),
        ("MsiGetComponentPathW", 173),
        ("MsiConfigureProductExW", 190),
        ("MsiProvideQualifiedComponentExW", 203),
    ];
    // (rustc's .def names them too: lld warns about a duplicate export and
    // keeps this one, with its ordinal.)
    for (name, ordinal) in ordinals {
        println!("cargo:rustc-cdylib-link-arg=/EXPORT:{name},@{ordinal}");
    }
}
