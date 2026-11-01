//! Exports that programs gala runs import (SQL Server LocalDB, FileZilla)
//! and gala does not implement yet (`gala_stubs`): calling one reports
//! `dll!function` to the ledger and ends the process.

gala_stubs::unimplemented_exports!("comctl32.dll":
    CreateUpDownControl
    ImageList_Add
    ImageList_AddMasked
    ImageList_BeginDrag
    ImageList_Copy
    ImageList_Create
    ImageList_Destroy
    ImageList_DragEnter
    ImageList_DragLeave
    ImageList_DragMove
    ImageList_Draw
    ImageList_EndDrag
    ImageList_GetIcon
    ImageList_GetIconSize
    ImageList_GetImageCount
    ImageList_GetImageInfo
    ImageList_Remove
    ImageList_Replace
    ImageList_ReplaceIcon
    ImageList_SetBkColor
    ImageList_SetDragCursorImage
);
