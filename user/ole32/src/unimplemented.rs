//! Exports SQL Server LocalDB imports that gala does not implement yet
//! (`gala_stubs`): calling one reports `dll!function` to the ledger and
//! ends the process.

gala_stubs::unimplemented_exports!("ole32.dll":
    CoCreateInstanceEx
    CoImpersonateClient
    CoQueryProxyBlanket
    CoRegisterMallocSpy
    CoRevertToSelf
    CoSetProxyBlanket
    CreateBindCtx
    GetHGlobalFromStream
    CoFreeUnusedLibraries
    DoDragDrop
    OleFlushClipboard
    OleGetClipboard
    OleIsCurrentClipboard
    OleSetClipboard
    PropVariantClear
    ReleaseStgMedium
);
