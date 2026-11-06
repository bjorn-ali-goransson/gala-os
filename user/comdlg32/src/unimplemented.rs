//! Exports programs import (FileZilla) that gala does not implement yet
//! (`gala_stubs`): calling one reports `dll!function` to the ledger and
//! ends the process.

gala_stubs::unimplemented_exports!("comdlg32.dll":
    ChooseFontW
    GetOpenFileNameW
    GetSaveFileNameW
);
