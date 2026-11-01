//! security.dll's exports: the SSPI functions, forwarded to secur32 (as
//! Windows' security.dll forwards them to the SSPI implementation).
fn main() {
    for name in [
        "AcceptSecurityContext",
        "AcquireCredentialsHandleW",
        "ApplyControlToken",
        "CompleteAuthToken",
        "DecryptMessage",
        "DeleteSecurityContext",
        "EncryptMessage",
        "EnumerateSecurityPackagesW",
        "FreeContextBuffer",
        "FreeCredentialsHandle",
        "ImpersonateSecurityContext",
        "InitSecurityInterfaceW",
        "InitializeSecurityContextW",
        "MakeSignature",
        "QueryContextAttributesW",
        "QuerySecurityContextToken",
        "QuerySecurityPackageInfoW",
        "RevertSecurityContext",
        "VerifySignature",
    ] {
        println!("cargo:rustc-cdylib-link-arg=/EXPORT:{name}=secur32.{name}");
    }
}
