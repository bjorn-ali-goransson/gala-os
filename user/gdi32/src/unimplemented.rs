//! Exports that programs gala runs import (SQL Server LocalDB, FileZilla)
//! and gala does not implement yet (`gala_stubs`): calling one reports
//! `dll!function` to the ledger and ends the process.

gala_stubs::unimplemented_exports!("gdi32.dll":
    AbortDoc
    AddFontResourceExW
    AddFontResourceW
    AngleArc
    ArcTo
    Chord
    CopyEnhMetaFileW
    CreateHalftonePalette
    CreatePenIndirect
    CreateRoundRectRgn
    EnumFontsW
    FrameRgn
    GetBitmapBits
    GetCharABCWidthsW
    GetEnhMetaFileDescriptionW
    GetEnhMetaFilePaletteEntries
    GetEnhMetaFileW
    GetMetaFileBitsEx
    GetSystemPaletteEntries
    GetTextExtentPointW
    GetWinMetaFileBits
    LineDDA
    PolyBezier
    RectVisible
    RemoveFontResourceW
    ResizePalette
    SetMetaFileBitsEx
    SetWinMetaFileBits
    UnrealizeObject
);
