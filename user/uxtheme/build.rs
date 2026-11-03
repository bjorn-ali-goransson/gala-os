//! uxtheme's functions programs import by ordinal: DrawThemeBackgroundEx
//! is #47 (Chromium imports it so; the numbering Wine's uxtheme.spec
//! records).
fn main() {
    println!("cargo:rustc-cdylib-link-arg=/EXPORT:DrawThemeBackgroundEx,@47");
}
