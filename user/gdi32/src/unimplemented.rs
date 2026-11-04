//! Exports that programs gala runs import (SQL Server LocalDB, FileZilla)
//! and gala does not implement yet (`gala_stubs`): calling one reports
//! `dll!function` to the ledger and ends the process.

gala_stubs::unimplemented_exports!("gdi32.dll":
    AddFontResourceExW
    CopyEnhMetaFileW
    GetCharABCWidthsW
    GetEnhMetaFileW
    GetMetaFileBitsEx
    GetSystemPaletteEntries
    GetWinMetaFileBits
    SetMetaFileBitsEx
    SetWinMetaFileBits
);
