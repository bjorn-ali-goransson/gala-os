//! Export forwarders: the memory and string primitives msvcrt exports are
//! ntdll's (the loader follows the forwarders), and the variadic
//! functions written in assembly (`stdio.rs`, `scan.rs`, `wide.rs`) and
//! `_setjmp`/`longjmp` (`setjmp.rs`) are exported by name.
fn main() {
    for name in ["memcmp", "memcpy", "memmove", "memset", "strlen"] {
        println!("cargo:rustc-cdylib-link-arg=/EXPORT:{name}=ntdll.{name}");
    }
    for name in [
        "fprintf",
        "fwprintf",
        "printf",
        "sprintf",
        "_snprintf",
        "sscanf",
        "_setjmp",
        "longjmp",
        "swprintf",
        "fscanf",
        "_execl",
        "_execlp",
    ] {
        println!("cargo:rustc-cdylib-link-arg=/EXPORT:{name}");
    }
}
