//! Exports SQL Server LocalDB imports that gala does not implement yet
//! (`gala_stubs`): calling one reports `dll!function` to the ledger and
//! ends the process.

gala_stubs::unimplemented_exports!("user32.dll":
    AnimateWindow
    ChangeDisplaySettingsExW
    ChildWindowFromPoint
    ChildWindowFromPointEx
    CreateCaret
    DdeCreateDataHandle
    DdeFreeDataHandle
    DdeGetData
    DdeNameService
    DdePostAdvise
    DdeQueryStringW
    DestroyCaret
    DrawStateW
    ExitWindowsEx
    GetProcessDefaultLayout
    HideCaret
    LoadCursorFromFileW
    SetCaretBlinkTime
    SetCaretPos
    SetLayeredWindowAttributes
    ShowCaret
    ToUnicode
    ValidateRgn
    WaitForInputIdle
    keybd_event
    mouse_event
    DialogBoxParamW
    MessageBoxIndirectW
    // Inno Setup's Setup (Delphi's VCL) imports these.
    ActivateKeyboardLayout
    CopyImage
    CreateIcon
    DefFrameProcW
    DefMDIChildProcW
    GetDCEx
    GetKeyNameTextW
    GetLastActivePopup
    GetMenuItemRect
    GetScrollBarInfo
    LoadKeyboardLayoutW
    ShowOwnedPopups
    TranslateMDISysAccel
);
