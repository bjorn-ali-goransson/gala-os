//! Exports SQL Server LocalDB imports that gala does not implement yet
//! (`gala_stubs`): calling one reports `dll!function` to the ledger and
//! ends the process.

gala_stubs::unimplemented_exports!("odbc32.dll":
    SQLAllocHandle
    SQLBindCol
    SQLBindParameter
    SQLBrowseConnect
    SQLBrowseConnectW
    SQLCancel
    SQLColAttributeW
    SQLDescribeColW
    SQLDisconnect
    SQLDriverConnectA
    SQLDriverConnectW
    SQLEndTran
    SQLExecDirectW
    SQLExecute
    SQLFetch
    SQLFreeHandle
    SQLFreeStmt
    SQLGetConnectAttrW
    SQLGetData
    SQLGetDescFieldW
    SQLGetDiagFieldW
    SQLGetDiagRec
    SQLGetDiagRecW
    SQLGetInfoA
    SQLGetInfoW
    SQLGetStmtAttrW
    SQLMoreResults
    SQLNumResultCols
    SQLPrepareW
    SQLRowCount
    SQLSetConnectAttr
    SQLSetConnectAttrW
    SQLSetEnvAttr
    SQLSetStmtAttrW
);
