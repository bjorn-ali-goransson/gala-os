//! Exports SQL Server LocalDB imports that gala does not implement yet
//! (`gala_stubs`): calling one reports `dll!function` to the ledger and
//! ends the process.

gala_stubs::unimplemented_exports!("shlwapi.dll":
    PathCanonicalizeW
    PathCombineW
    PathCreateFromUrlW
    PathFileExistsW
    PathFindExtensionW
    PathFindFileNameW
    PathIsRelativeW
    PathIsURLW
    PathRemoveBackslashW
    PathRemoveExtensionW
    PathRemoveFileSpecW
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
);
