//! msi.dll's functions that programs import by ordinal (the Office
//! Deployment Tool's setup.exe and its integrator.exe import every msi
//! function so, and Word's shared code -- MSO.DLL and the
//! Mso*win32client.dll chain -- delay-loads eleven of them): the exports
//! carry Windows' ordinals, read from the export directory of build
//! 26200's msi.dll -- which lists all but #92 the same as Wine's
//! msi.spec, where the others came from.
fn main() {
    let ordinals: &[(&str, u32)] = &[
        ("MsiCloseHandle", 8),
        ("MsiDatabaseOpenViewW", 32),
        ("MsiEnumClientsW", 37),
        ("MsiEnumComponentQualifiersW", 39),
        ("MsiEnumProductsW", 45),
        ("MsiGetProductCodeA", 65),
        ("MsiGetProductCodeW", 66),
        ("MsiGetUserInfoW", 82),
        ("MsiInstallProductW", 88),
        ("MsiLocateComponentW", 90),
        ("MsiOpenDatabaseW", 92),
        ("MsiProvideQualifiedComponentW", 109),
        ("MsiQueryFeatureStateW", 111),
        ("MsiQueryProductStateW", 113),
        ("MsiRecordGetFieldCount", 115),
        ("MsiRecordGetInteger", 116),
        ("MsiRecordGetStringW", 118),
        ("MsiRecordIsNull", 119),
        ("MsiReinstallFeatureW", 129),
        ("MsiSetExternalUIW", 137),
        ("MsiSetInternalUI", 141),
        ("MsiUseFeatureW", 155),
        ("MsiViewClose", 158),
        ("MsiViewExecute", 159),
        ("MsiViewFetch", 160),
        ("MsiDatabaseIsTablePersistentW", 165),
        ("MsiGetComponentPathA", 172),
        ("MsiGetComponentPathW", 173),
        ("MsiConfigureProductExW", 190),
        ("MsiUseFeatureExW", 193),
        ("MsiProvideQualifiedComponentExW", 203),
    ];
    // (rustc's .def names them too: lld warns about a duplicate export and
    // keeps this one, with its ordinal.)
    for (name, ordinal) in ordinals {
        println!("cargo:rustc-cdylib-link-arg=/EXPORT:{name},@{ordinal}");
    }

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
