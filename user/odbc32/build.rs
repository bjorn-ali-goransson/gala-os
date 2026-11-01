//! Programs import ODBC's functions from odbc32.dll by ordinal only. The
//! numbers are those of Windows' odbc32.dll, taken from Wine's
//! dlls/odbc32/odbc32.spec (LGPL; the numbers only, recorded in
//! THIRD_PARTY.md).
fn main() {
    let ordinals: &[(&str, u32)] = &[
        ("SQLBindCol", 4),
        ("SQLCancel", 5),
        ("SQLDisconnect", 9),
        ("SQLExecute", 12),
        ("SQLFetch", 13),
        ("SQLNumResultCols", 18),
        ("SQLRowCount", 20),
        ("SQLAllocHandle", 24),
        ("SQLEndTran", 29),
        ("SQLFreeHandle", 31),
        ("SQLGetData", 43),
        ("SQLMoreResults", 61),
        ("SQLBindParameter", 72),
        ("SQLSetEnvAttr", 75),
        ("SQLDescribeColW", 108),
        ("SQLExecDirectW", 111),
        ("SQLPrepareW", 119),
        ("SQLColAttributeW", 127),
        ("SQLGetConnectAttrW", 132),
        ("SQLGetDiagRecW", 136),
        ("SQLSetConnectAttrW", 139),
        ("SQLDriverConnectW", 141),
        ("SQLSetStmtAttrW", 176),
    ];
    for (name, ordinal) in ordinals {
        println!("cargo:rustc-cdylib-link-arg=/EXPORT:{name},@{ordinal}");
    }
}
