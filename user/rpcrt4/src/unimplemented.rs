//! Exports SQL Server LocalDB imports that gala does not implement yet
//! (`gala_stubs`): calling one reports `dll!function` to the ledger and
//! ends the process.

gala_stubs::unimplemented_exports!("rpcrt4.dll":
    NdrClientCall3
    RpcBindingFree
    RpcBindingFromStringBindingW
    RpcExceptionFilter
    RpcStringBindingComposeW
    UuidCreateSequential
);
