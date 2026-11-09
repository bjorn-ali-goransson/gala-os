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
//!
//! The same source builds gala's ucrtbase.dll (user/ucrtbase points its
//! lib and build script here): for that package this sets `cfg(ucrt)`,
//! which adds the UCRT's own functions (`src/ucrt/`).
fn main() {
    println!("cargo::rustc-check-cfg=cfg(ucrt)");
    let ucrt = std::env::var("CARGO_PKG_NAME").as_deref() == Ok("ucrtbase");
    if ucrt {
        println!("cargo:rustc-cfg=ucrt");
    }
    // The UCRT's aliases: api-ms-win-crt-private's `_o_` names (the same
    // functions), and the `_l` forms, which take a locale as their last
    // argument: gala's C runtime has one locale ("C"), so they are the
    // plain functions (an extra last argument is the caller's to pop in
    // the C convention of both architectures, and the callee ignores it).
    if ucrt {
        for (alias, target) in UCRT_ALIASES {
            println!("cargo:rustc-cdylib-link-arg=/EXPORT:{alias}={target}");
        }
    }
    // The UCRT's setjmp intrinsics (what MSVC and mingw-w64's UCRT
    // headers call): `_setjmp` with x64's buffer (its longjmp does not
    // unwind, as msvcrt's here).
    if ucrt && std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() == Ok("x86_64") {
        for name in ["__intrinsic_setjmp", "__intrinsic_setjmpex"] {
            println!("cargo:rustc-cdylib-link-arg=/EXPORT:{name}=_setjmp");
        }
    }
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

/// The UCRT's alias exports (`main`).
const UCRT_ALIASES: &[(&str, &str)] = &[
    ("_o___stdio_common_vsnprintf_s", "__stdio_common_vsnprintf_s"),
    ("_o___stdio_common_vswprintf", "__stdio_common_vswprintf"),
    ("_o__callnewh", "_callnewh"),
    ("_o__cexit", "_cexit"),
    ("_o__configure_narrow_argv", "_configure_narrow_argv"),
    ("_o__configure_wide_argv", "_configure_wide_argv"),
    ("_o__crt_atexit", "_crt_atexit"),
    ("_o__errno", "_errno"),
    ("_o__execute_onexit_table", "_execute_onexit_table"),
    ("_o__initialize_narrow_environment", "_initialize_narrow_environment"),
    ("_o__initialize_wide_environment", "_initialize_wide_environment"),
    ("_o__initialize_onexit_table", "_initialize_onexit_table"),
    ("_o__invalid_parameter_noinfo", "_invalid_parameter_noinfo"),
    ("_o__invalid_parameter_noinfo_noreturn", "_invalid_parameter_noinfo_noreturn"),
    ("_o__register_onexit_function", "_register_onexit_function"),
    ("_o__seh_filter_dll", "_seh_filter_dll"),
    ("_o__seh_filter_exe", "_seh_filter_exe"),
    ("_o_free", "free"),
    ("_o_malloc", "malloc"),
    ("_o_calloc", "calloc"),
    ("_o_realloc", "realloc"),
    ("_o_terminate", "terminate"),
    ("_calloc_base", "calloc"),
    ("_realloc_base", "realloc"),
    ("_atoi64", "atoll"),
    ("_atof_l", "atof"),
    ("_atoi_l", "atoi"),
    ("_wtoi_l", "_wtoi"),
    ("_wtol_l", "_wtol"),
    ("_strtol_l", "strtol"),
    ("_strtoul_l", "strtoul"),
    ("_strtoi64_l", "_strtoi64"),
    ("_strtoui64_l", "_strtoui64"),
    ("_strtod_l", "strtod"),
    ("_wcstol_l", "wcstol"),
    ("_wcstoul_l", "wcstoul"),
    ("_wcstoi64_l", "_wcstoi64"),
    ("_wcstoui64_l", "_wcstoui64"),
    ("_wcstod_l", "wcstod"),
    ("_wcstombs_l", "wcstombs"),
    ("_mbstowcs_l", "mbstowcs"),
    ("_tolower_l", "tolower"),
    ("_toupper_l", "toupper"),
    ("_towlower_l", "towlower"),
    ("_towupper_l", "towupper"),
    ("_stricmp_l", "_stricmp"),
    ("_strnicmp_l", "_strnicmp"),
    ("_wcsicmp_l", "_wcsicmp"),
    ("_wcsnicmp_l", "_wcsnicmp"),
    ("_strcoll_l", "strcoll"),
    ("_wcscoll_l", "wcscoll"),
    ("_iswalnum_l", "iswalnum"),
    ("_iswalpha_l", "iswalpha"),
    ("_iswcntrl_l", "iswcntrl"),
    ("_iswdigit_l", "iswdigit"),
    ("_iswprint_l", "iswprint"),
    ("_iswspace_l", "iswspace"),
    ("_iswxdigit_l", "iswxdigit"),
    ("_iswlower_l", "iswlower"),
    ("_iswupper_l", "iswupper"),
    ("_iswpunct_l", "iswpunct"),
    ("_iswctype_l", "iswctype"),
];
