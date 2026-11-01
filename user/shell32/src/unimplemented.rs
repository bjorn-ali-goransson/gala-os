//! Exports that programs gala runs import (SQL Server LocalDB, FileZilla)
//! and gala does not implement yet (`gala_stubs`): calling one reports
//! `dll!function` to the ledger and ends the process.

gala_stubs::unimplemented_exports!("shell32.dll":
    DragAcceptFiles
    DragFinish
    DragQueryFileW
    DragQueryPoint
    ExtractIconExW
    ExtractIconW
    SHBrowseForFolderW
    SHChangeNotify
    SHDefExtractIconW
    SHFileOperationW
    SHGetFileInfoW
    SHGetIconOverlayIndexW
    SHGetMalloc
    SHGetPathFromIDListW
    SHGetSpecialFolderLocation
    SetCurrentProcessExplicitAppUserModelID
    ShellExecuteExW
    Shell_NotifyIconW
);
