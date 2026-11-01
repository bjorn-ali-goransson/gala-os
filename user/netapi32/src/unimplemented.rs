//! Exports SQL Server LocalDB imports that gala does not implement yet
//! (`gala_stubs`): calling one reports `dll!function` to the ledger and
//! ends the process.

gala_stubs::unimplemented_exports!("netapi32.dll":
    DsGetDcNameW
    NetApiBufferFree
    NetGroupGetUsers
    NetLocalGroupGetMembers
    NetUserModalsGet
    NetUserModalsSet
    NetWkstaGetInfo
);
