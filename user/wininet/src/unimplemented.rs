//! Exports SQL Server LocalDB imports that gala does not implement yet
//! (`gala_stubs`): calling one reports `dll!function` to the ledger and
//! ends the process.

gala_stubs::unimplemented_exports!("wininet.dll":
    HttpAddRequestHeadersW
    HttpOpenRequestW
    HttpQueryInfoW
    HttpSendRequestW
    InternetAttemptConnect
    InternetCloseHandle
    InternetConnectW
    InternetOpenW
    InternetReadFile
    InternetSetOptionW
    InternetTimeFromSystemTimeW
    InternetTimeToSystemTimeW
);
