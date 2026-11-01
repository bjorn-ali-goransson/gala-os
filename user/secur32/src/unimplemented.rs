//! Exports SQL Server LocalDB imports that gala does not implement yet
//! (`gala_stubs`): calling one reports `dll!function` to the ledger and
//! ends the process.

gala_stubs::unimplemented_exports!("secur32.dll":
    LsaConnectUntrusted
    LsaDeregisterLogonProcess
    LsaFreeReturnBuffer
    LsaLogonUser
    LsaLookupAuthenticationPackage
    LsaRegisterLogonProcess
);
