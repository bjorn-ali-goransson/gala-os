//! Exports the linker does not take from `#[no_mangle]`, by architecture.
//!
//! x64: the memory and string primitives msvcrt exports are ntdll's
//! (forwarders; the loader follows them), and the variadic functions and
//! `_setjmp`/`longjmp`, written with `global_asm!` (`x64.rs`), are
//! exported by name.
//!
//! x86: those primitives come from ntrt's x86 runtime, which msvcrt links
//! (the compiler calls them; `memcmp` and `strlen` are Rust functions
//! there and exported as they are, the others are its assembly); the
//! variadic functions are naked functions (`x86.rs`), exported as they
//! are.
fn main() {
    match std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() {
        Ok("x86") => {
            for name in ["memcpy", "memmove", "memset"] {
                println!("cargo:rustc-cdylib-link-arg=/EXPORT:{name}");
            }
        }
        _ => {
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
    }
    println!("cargo:rerun-if-changed=build.rs");
}
