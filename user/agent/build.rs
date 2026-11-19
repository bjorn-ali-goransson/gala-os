//! agent.exe is a GUI program (IMAGE_SUBSYSTEM_WINDOWS_GUI): started with
//! the session it gets no console window. The workspace's link arguments
//! make console programs; lld-link takes the last /SUBSYSTEM.
fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").is_ok_and(|os| os == "windows") {
        println!("cargo:rustc-link-arg-bins=/SUBSYSTEM:WINDOWS");
    }
}
