//! Exports that programs gala runs import (SQL Server LocalDB, FileZilla)
//! and gala does not implement yet (`gala_stubs`): calling one reports
//! `dll!function` to the ledger and ends the process.

gala_stubs::unimplemented_exports!("comctl32.dll":
    CreateUpDownControl
    FlatSB_GetScrollInfo
    FlatSB_GetScrollPos
    FlatSB_SetScrollInfo
    FlatSB_SetScrollPos
    FlatSB_SetScrollProp
    ImageList_DragShowNolock
    ImageList_DrawEx
    ImageList_DrawIndirect
    ImageList_GetDragImage
    ImageList_LoadImageW
    ImageList_Read
    ImageList_SetIconSize
    ImageList_SetImageCount
    ImageList_SetOverlayImage
    ImageList_Write
    InitializeFlatSB
);
