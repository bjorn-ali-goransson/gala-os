//! kernelbase.dll's exports: Windows' list (`exports.txt`, from Wine's
//! kernelbase.spec by tools/kernelbase_exports.py), each a forwarder to
//! the gala DLL that implements it (ARCHITECTURE "API sets"), except the
//! functions kernelbase implements itself (OWN, in src/lib.rs). A
//! forwarder whose target gala does not implement yet resolves to nothing
//! (GetProcAddress NULL, an import of it missing and in the ledger), as an
//! absent export would; it starts working when the target gains the
//! function. Those of the architecture built.

/// What src/lib.rs defines (and gala_stubs' VirtualAlloc2).
const OWN: &[&str] = &[
    "CompareObjectHandles",
    "QueryInterruptTime",
    "QueryInterruptTimePrecise",
    "QueryUnbiasedInterruptTimePrecise",
    "VirtualAlloc2",
];

fn main() {
    println!("cargo:rerun-if-changed=exports.txt");
    println!("cargo:rerun-if-changed=build.rs");
    let arch = match std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() {
        Ok("x86") => "x86",
        _ => "x64",
    };
    let list = std::fs::read_to_string("exports.txt").expect("user/kernelbase/exports.txt");
    for line in list.lines().filter(|l| !l.starts_with('#') && !l.is_empty()) {
        let mut words = line.split_whitespace();
        let (Some(name), Some(target), Some(only)) = (words.next(), words.next(), words.next())
        else {
            panic!("exports.txt: bad line {line:?}");
        };
        if OWN.contains(&name) || (only != "all" && only != arch) {
            continue;
        }
        println!("cargo:rustc-cdylib-link-arg=/EXPORT:{name}={target}.{name}");
    }
}
