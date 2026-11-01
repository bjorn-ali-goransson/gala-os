//! Exports SQL Server LocalDB imports that gala does not implement yet
//! (`gala_stubs`): calling one reports `dll!function` to the ledger and
//! ends the process.

gala_stubs::unimplemented_exports!("dbghelp.dll":
    SymFromAddr
    SymLoadModuleExW
    SymUnloadModule64
);
