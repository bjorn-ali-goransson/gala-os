//! Exports programs import (SQL Server LocalDB, the JDK) that gala does not
//! implement yet (`gala_stubs`): calling one reports `dll!function` to the
//! ledger and ends the process.

gala_stubs::unimplemented_exports!("winhttp.dll":
    WinHttpQueryAuthSchemes
    WinHttpSetCredentials
    WinHttpTimeFromSystemTime
);
