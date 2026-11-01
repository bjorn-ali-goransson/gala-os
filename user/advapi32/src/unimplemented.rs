//! Exports SQL Server LocalDB imports that gala does not implement yet
//! (`gala_stubs`): calling one reports `dll!function` to the ledger and
//! ends the process.

gala_stubs::unimplemented_exports!("advapi32.dll":
    ChangeServiceConfigA
    ChangeServiceConfigW
    CloseServiceHandle
    ControlService
    ControlTraceW
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
    CreateServiceA
    CryptEncrypt
    CryptGetHashParam
    CryptHashData
    DeleteService
    LookupAccountSidA
);
