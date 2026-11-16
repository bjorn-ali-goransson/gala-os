//! advapi32 keeps one import it never calls: rpcrt4.dll, which Windows'
//! advapi32 imports statically, so a program that reads the module graph
//! sees it (ARCHITECTURE "The module graph").

fn main() {
    // Keep rpcrt4.dll in the import table. The import is declared in
    // src/lib.rs (raw-dylib) but nothing calls it, and the linker drops an
    // import whose only reference is a static it also drops, so the name is
    // pinned on the link command line instead. Why the dependency is there
    // at all: ARCHITECTURE "The module graph". gala has no 32-bit rpcrt4,
    // so the 32-bit build goes without it.
    if std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() != Ok("x86") {
        println!("cargo:rustc-cdylib-link-arg=/INCLUDE:__imp_RpcStringFreeW");
    }
}
