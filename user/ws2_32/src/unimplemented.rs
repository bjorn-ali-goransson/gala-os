//! Exports SQL Server LocalDB imports that gala does not implement yet
//! (`gala_stubs`): calling one reports `dll!function` to the ledger and
//! ends the process.

gala_stubs::unimplemented_exports!("ws2_32.dll":
    GetNameInfoW
    InetNtopW
    InetPtonW
    WSAAddressToStringW
    WSAStringToAddressA
    WSAStringToAddressW
    inet_ntop
    inet_pton
);
