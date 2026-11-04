//! Exports SQL Server LocalDB imports that gala does not implement yet
//! (`gala_stubs`): calling one reports `dll!function` to the ledger and
//! ends the process. MiniDumpWriteDump is SqlDumper's (a crash dump).

gala_stubs::unimplemented_exports!("dbghelp.dll":
    MiniDumpWriteDump
);
