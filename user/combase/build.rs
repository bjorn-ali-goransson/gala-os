//! combase.dll's exports: Windows' list (`exports.txt`, from Wine's
//! combase.spec), each a forwarder to gala's ole32.dll, which implements
//! the COM and Windows Runtime base (on Windows ole32 forwards to combase;
//! on gala the code is ole32's, ARCHITECTURE "COM: ole32 and oleaut32").
//! A forwarder whose target ole32 does not implement resolves to nothing
//! (GetProcAddress NULL, an import of it missing and in the ledger), as an
//! absent export would; it works once ole32 has the function. Those of
//! the architecture built.
fn main() {
    println!("cargo:rerun-if-changed=exports.txt");
    println!("cargo:rerun-if-changed=build.rs");
    let arch = match std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() {
        Ok("x86") => "x86",
        _ => "x64",
    };
    let list = std::fs::read_to_string("exports.txt").expect("user/combase/exports.txt");
    for line in list.lines().filter(|l| !l.starts_with('#') && !l.is_empty()) {
        let mut words = line.split_whitespace();
        let (Some(name), Some(only)) = (words.next(), words.next()) else {
            panic!("exports.txt: bad line {line:?}");
        };
        if only != "all" && only != arch {
            continue;
        }
        println!("cargo:rustc-cdylib-link-arg=/EXPORT:{name}=ole32.{name}");
    }
}
