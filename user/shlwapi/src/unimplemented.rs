//! Exports SQL Server LocalDB imports that gala does not implement yet
//! (`gala_stubs`): calling one reports `dll!function` to the ledger and
//! ends the process.

gala_stubs::unimplemented_exports!("shlwapi.dll":
    PathCanonicalizeW
    PathCombineW
    PathCreateFromUrlW
    PathFindExtensionW
    PathIsRelativeW
    PathIsURLW
    PathRemoveBackslashW
    PathRemoveExtensionW
    PathSearchAndQualifyW
    SHCreateStreamOnFileEx
    SHDeleteKeyW
    StrCmpIW
    StrCmpNIW
    StrCmpW
    StrStrIW
    StrStrW
    UrlCanonicalizeW
    UrlCombineW
    UrlCreateFromPathW
    UrlEscapeW
    UrlIsW
    UrlUnescapeW
    AssocQueryStringW
    PathMatchSpecW
    SHAutoComplete
    StrCmpLogicalW
);
