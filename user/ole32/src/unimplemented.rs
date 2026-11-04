//! Exports SQL Server LocalDB and wxWidgets import that gala does not
//! implement yet (`gala_stubs`): calling one reports `dll!function` to
//! the ledger and ends the process.

gala_stubs::unimplemented_exports!("ole32.dll":
    CoRegisterMallocSpy
    GetHGlobalFromStream
    DoDragDrop
    OleFlushClipboard
    OleGetClipboard
    OleIsCurrentClipboard
    OleSetClipboard
);
