fn main() {
    // Only the bare-metal kernel binary uses the linker script; host unit-test
    // builds of the library link normally.
    if std::env::var("TARGET").as_deref() == Ok("x86_64-unknown-none") {
        let dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
        println!("cargo:rustc-link-arg-bins=-T{dir}/linker.ld");
        // What a hot swap of the kernel into a resumed snapshot diffs
        // builds with (SN step 1e, xtask/src/hotswap.rs): the relocations
        // kept in the output (`.rela.*`, not loaded: every absolute and
        // relative reference of the code and data), and lld's map of every
        // input section beside the binary (`kernel.map`). OUT_DIR is
        // `<profile dir>/build/kernel-<hash>/out`.
        println!("cargo:rustc-link-arg-bins=--emit-relocs");
        let out = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap());
        if let Some(profile_dir) = out.ancestors().nth(3) {
            println!("cargo:rustc-link-arg-bins=-Map={}", profile_dir.join("kernel.map").display());
        }
    }
    // SN step 1e: the kernel builds the hot swap scenarios swap into a
    // snapshot of the default one (nt::hotswap_probe), chosen by
    // `GALA_KERNEL_VARIANT`: `hotswap_v2` changes the probe's functions and
    // adds a static; `hotswap_layout` also changes a static's size
    // (refused). A cfg from here, not a cargo feature: a feature changes
    // the crate's metadata, and with it every TypeId and the code that
    // holds one, which a swap would see as changed. xtask builds them in a
    // profile of their own (`khotswap`). Never set for a release.
    println!("cargo:rustc-check-cfg=cfg(hotswap_v2)");
    println!("cargo:rustc-check-cfg=cfg(hotswap_layout)");
    println!("cargo:rerun-if-env-changed=GALA_KERNEL_VARIANT");
    match std::env::var("GALA_KERNEL_VARIANT").as_deref() {
        Ok("hotswap_v2") => println!("cargo:rustc-cfg=hotswap_v2"),
        Ok("hotswap_layout") => {
            println!("cargo:rustc-cfg=hotswap_v2");
            println!("cargo:rustc-cfg=hotswap_layout");
        }
        Ok("") | Err(_) => {}
        Ok(other) => panic!("GALA_KERNEL_VARIANT={other}: hotswap_v2 or hotswap_layout"),
    }
    println!("cargo:rerun-if-changed=linker.ld");
}
