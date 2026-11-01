//! Exports SQL Server LocalDB imports that gala does not implement yet
//! (`gala_stubs`): calling one reports `dll!function` to the ledger and
//! ends the process.

gala_stubs::unimplemented_exports!("advapi32.dll":
    AllocateLocallyUniqueId
    ChangeServiceConfigA
    ChangeServiceConfigW
    CloseServiceHandle
    ControlService
    ControlTraceW
    ConvertSidToStringSidA
    CryptContextAddRef
    CryptImportKey
    CryptSetProvParam
    DeleteAce
    EnableTrace
    EnumDependentServicesW
    EnumerateTraceGuids
    GetSidLengthRequired
    LockServiceDatabase
    LogonUserExW
    LsaAddAccountRights
    LsaClose
    LsaOpenPolicy
    OpenSCManagerA
    OpenSCManagerW
    OpenServiceA
    OpenServiceW
    QueryAllTracesW
    QueryServiceConfigA
    QueryServiceConfigW
    QueryServiceStatus
    QueryServiceStatusEx
    RegConnectRegistryW
    RegCopyTreeW
    RegisterServiceCtrlHandlerA
    RegisterServiceCtrlHandlerExW
    RegisterServiceCtrlHandlerW
    SetSecurityDescriptorGroup
    SetServiceBits
    SetServiceStatus
    StartServiceA
    StartServiceW
    StartTraceW
    UnlockServiceDatabase
);
