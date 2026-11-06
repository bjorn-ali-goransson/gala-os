//! wow64.dll's entry points are assembly (`src/lib.rs`): exported by name
//! here, as the kernel finds them (nt::wow64).
fn main() {
    for name in ["GalaWow64Start", "GalaWow64Transition", "GalaWow64Exception", "GalaWow64Apc"] {
        println!("cargo:rustc-cdylib-link-arg=/EXPORT:{name}");
    }
}
