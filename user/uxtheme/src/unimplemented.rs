//! Exports FileZilla imports that gala does not implement yet
//! (`gala_stubs`): calling one reports `dll!function` to the ledger and
//! ends the process.

gala_stubs::unimplemented_exports!("uxtheme.dll":
    CloseThemeData
    DrawThemeBackground
    DrawThemeParentBackground
    GetCurrentThemeName
    GetThemeBackgroundContentRect
    GetThemeBackgroundExtent
    GetThemeColor
    GetThemeFont
    GetThemeInt
    GetThemeMargins
    GetThemePartSize
    GetThemeSysColor
    GetThemeSysFont
    IsAppThemed
    IsThemeActive
    IsThemeBackgroundPartiallyTransparent
    IsThemePartDefined
    OpenThemeData
    SetWindowTheme
);
