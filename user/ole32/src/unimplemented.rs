//! Exports SQL Server LocalDB imports that gala does not implement yet
//! (`gala_stubs`): calling one reports `dll!function` to the ledger and
//! ends the process.

gala_stubs::unimplemented_exports!("ole32.dll":
    CLSIDFromString
    CoCreateInstanceEx
    CoGetMalloc
    CoImpersonateClient
    CoQueryProxyBlanket
    CoRegisterMallocSpy
    CoRevertToSelf
    CoSetProxyBlanket
    CreateBindCtx
    GetHGlobalFromStream
    IIDFromString
    OleInitialize
    OleUninitialize
    StringFromCLSID
    StringFromIID
);
