//! Exports that programs gala runs import (SQL Server LocalDB, FileZilla)
//! and gala does not implement yet (`gala_stubs`): calling one reports
//! `dll!function` to the ledger and ends the process.

gala_stubs::unimplemented_exports!("comctl32.dll":
    CreateUpDownControl
    ImageList_BeginDrag
    ImageList_DragEnter
    ImageList_DragLeave
    ImageList_DragMove
    ImageList_EndDrag
    ImageList_SetDragCursorImage
);
