//! shfolder.dll: SHGetFolderPathA/W for programs written before shell32
//! had them (NSIS installers look SHGetFolderPathW up there). On Windows
//! both are shell32's; here each export forwards to shell32.dll.
fn main() {
    for name in ["SHGetFolderPathA", "SHGetFolderPathW"] {
        println!("cargo:rustc-cdylib-link-arg=/EXPORT:{name}=shell32.{name}");
    }
}
