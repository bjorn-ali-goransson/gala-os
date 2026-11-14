//! testdll's ordinals, with a gap: gala_add is #1 and gala_get_value #3,
//! so the export address table's slot for #2 holds RVA 0, an ordinal
//! with no export (as shell32's numbering leaves many). loader_test asks
//! for #2 and loads testdll_gap.dll, which imports it. The linker numbers
//! the other exports after the highest given.
fn main() {
    for (name, ordinal) in [("gala_add", 1), ("gala_get_value", 3)] {
        println!("cargo:rustc-cdylib-link-arg=/EXPORT:{name},@{ordinal}");
    }
}
