//! shell32's functions that programs import by ordinal (Chromium imports
//! SHChangeNotifyRegister as #2, SHChangeNotifyDeregister as #4, ILFree
//! as #155, ILCreateFromPathW as #190 and IsUserAnAdmin as #680; ILGetSize
//! is #152): the exports carry them. The numbers are those of Wine's
//! dlls/shell32/shell32.spec (the numbers only; THIRD_PARTY.md). Without
//! them the linker numbers the exports itself, and an import by ordinal
//! would bind to whichever function got that number.
fn main() {
    let ordinals: &[(&str, u32)] = &[
        ("SHChangeNotifyRegister", 2),
        ("SHChangeNotifyDeregister", 4),
        ("ILFindLastID", 16),
        ("ILRemoveLastID", 17),
        ("ILClone", 18),
        ("ILCloneFirst", 19),
        ("ILIsEqual", 21),
        ("ILIsParent", 23),
        ("ILFindChild", 24),
        ("ILCombine", 25),
        ("ILGetSize", 152),
        ("ILGetNext", 153),
        ("ILAppendID", 154),
        ("ILFree", 155),
        ("SHSimpleIDListFromPath", 162),
        ("ILCreateFromPathW", 190),
        ("IsUserAnAdmin", 680),
    ];
    // (rustc's .def names them too: lld warns about a duplicate export and
    // keeps this one, with its ordinal.)
    for (name, ordinal) in ordinals {
        println!("cargo:rustc-cdylib-link-arg=/EXPORT:{name},@{ordinal}");
    }
}
