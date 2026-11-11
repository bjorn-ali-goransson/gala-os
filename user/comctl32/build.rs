//! comctl32's exports that programs import by ordinal: LoadIconMetric is
//! #380 and LoadIconWithScaleDown #381 (Wine's comctl32 misc.c test
//! checks GetProcAddress by these ordinals on Windows; the numbers only,
//! THIRD_PARTY.md). Without them the linker numbers the exports itself,
//! and an import by ordinal would bind to whichever function got that
//! number.
fn main() {
    let ordinals: &[(&str, u32)] = &[("LoadIconMetric", 380), ("LoadIconWithScaleDown", 381)];
    // (rustc's .def names them too: lld warns about a duplicate export and
    // keeps this one, with its ordinal.)
    for (name, ordinal) in ordinals {
        println!("cargo:rustc-cdylib-link-arg=/EXPORT:{name},@{ordinal}");
    }
}
