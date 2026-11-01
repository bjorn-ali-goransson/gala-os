//! Exports SQL Server LocalDB imports that gala does not implement yet
//! (`gala_stubs`): calling one reports `dll!function` to the ledger and
//! ends the process.

gala_stubs::unimplemented_exports!("odbc32.dll":
    SQLAllocHandle
    SQLBindCol
    SQLBindParameter
    SQLCancel
    SQLColAttributeW
    SQLDescribeColW
    SQLDisconnect
    SQLDriverConnectW
    SQLEndTran
    SQLExecDirectW
    SQLExecute
    SQLFetch
    SQLFreeHandle
    SQLGetConnectAttrW
    SQLGetData
    SQLGetDiagRecW
    SQLMoreResults
    SQLNumResultCols
    SQLPrepareW
    SQLRowCount
    SQLSetConnectAttrW
    SQLSetEnvAttr
    SQLSetStmtAttrW
);
