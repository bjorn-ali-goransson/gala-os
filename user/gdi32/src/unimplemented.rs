//! Exports that programs gala runs import (SQL Server LocalDB, FileZilla)
//! and gala does not implement yet (`gala_stubs`): calling one reports
//! `dll!function` to the ledger and ends the process.

gala_stubs::unimplemented_exports!("gdi32.dll":
    AddFontResourceExW
    CloseEnhMetaFile
    CopyEnhMetaFileW
    CreateEnhMetaFileW
    DeleteEnhMetaFile
    EnumFontFamiliesExW
    GetCharABCWidthsW
    GetEnhMetaFileHeader
    GetEnhMetaFileW
    GetMetaFileBitsEx
    GetOutlineTextMetricsW
    GetSystemPaletteEntries
    GetWinMetaFileBits
    PlayEnhMetaFile
    SetMetaFileBitsEx
    SetWinMetaFileBits
);
