//! slc.dll's exports (`exports.txt`, Windows' own list): the Software
//! Licensing Client. Windows' slc.dll forwards 31 of its 40 names to
//! SPPC and keeps 9 of its own; gala's does the same, forwarding to
//! gala's sppc.dll (ARCHITECTURE "sppc", "slc"). A forwarder whose
//! target sppc does not implement resolves to nothing (GetProcAddress
//! NULL, an import of it missing and in the ledger), as an absent
//! export would, and works as soon as sppc has the function.
fn main() {
    println!("cargo:rerun-if-changed=exports.txt");
    println!("cargo:rerun-if-changed=build.rs");
    let list = std::fs::read_to_string("exports.txt").expect("user/slc/exports.txt");
    for line in list.lines().filter(|l| !l.starts_with('#') && !l.is_empty()) {
        let mut words = line.split_whitespace();
        let (Some(name), Some(kind)) = (words.next(), words.next()) else {
            panic!("exports.txt: bad line {line:?}");
        };
        match kind {
            "sppc" => println!("cargo:rustc-cdylib-link-arg=/EXPORT:{name}=sppc.{name}"),
            // `code` names are exported by the library itself, `absent`
            // ones are not exported at all.
            "code" | "absent" => {}
            _ => panic!("exports.txt: bad kind {kind:?} for {name}"),
        }
    }
}
