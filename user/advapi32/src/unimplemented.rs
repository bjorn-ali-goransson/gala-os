//! Exports SQL Server LocalDB imports that gala does not implement yet
//! (`gala_stubs`): calling one reports `dll!function` to the ledger and
//! ends the process.

gala_stubs::unimplemented_exports!("advapi32.dll":
    ChangeServiceConfigA
    ChangeServiceConfigW
    ControlTraceW
    CryptContextAddRef
    CryptEncrypt
    CryptGetHashParam
    CryptHashData
    CryptImportKey
    CryptSetProvParam
    DeleteAce
    EnableTrace
    EnumDependentServicesW
    EnumerateTraceGuids
    GetSidLengthRequired
    LockServiceDatabase
    LogonUserExW
    LookupAccountSidA
    QueryAllTracesW
    RegConnectRegistryW
    RegCopyTreeW
    SetEntriesInAclA
    SetServiceBits
    StartTraceW
    UnlockServiceDatabase
    CredDeleteW
    CredFree
    CredReadW
    CredWriteW
    CryptSignHashA
    // Inno Setup's Setup (Delphi's VCL) imports these.
    RegLoadKeyW
    RegReplaceKeyW
    RegRestoreKeyW
    RegSaveKeyW
    RegUnLoadKeyW
);
