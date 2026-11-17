//! oleaut32's exports have standard ordinals, and programs (CoreCLR)
//! import them by ordinal only. The numbers are those of Windows'
//! oleaut32.dll, taken from Wine's dlls/oleaut32/oleaut32.spec (LGPL; the
//! numbers only, recorded in THIRD_PARTY.md) and, for the active objects
//! and the conversions Office imports, read from the export table of an
//! installed oleaut32.dll (which agrees with the spec throughout). Get/Set/CreateErrorInfo
//! forward to ole32, as Windows' oleaut32 forwards them to COM's base.
fn main() {
    let ordinals: &[(&str, u32)] = &[
        ("SysAllocString", 2),
        ("SysReAllocString", 3),
        ("SysAllocStringLen", 4),
        ("SysReAllocStringLen", 5),
        ("SysFreeString", 6),
        ("SysStringLen", 7),
        ("VariantInit", 8),
        ("VariantClear", 9),
        ("VariantCopy", 10),
        ("VariantCopyInd", 11),
        ("VariantChangeType", 12),
        ("SafeArrayCreate", 15),
        ("SafeArrayDestroy", 16),
        ("SafeArrayGetDim", 17),
        ("SafeArrayGetElemsize", 18),
        ("SafeArrayGetUBound", 19),
        ("SafeArrayGetLBound", 20),
        ("SafeArrayLock", 21),
        ("SafeArrayUnlock", 22),
        ("SafeArrayAccessData", 23),
        ("SafeArrayUnaccessData", 24),
        ("SafeArrayGetElement", 25),
        ("SafeArrayPutElement", 26),
        ("SafeArrayCopy", 27),
        ("SafeArrayAllocDescriptor", 36),
        ("SafeArrayAllocData", 37),
        ("SafeArrayDestroyDescriptor", 38),
        ("SafeArrayDestroyData", 39),
        ("SafeArrayAllocDescriptorEx", 41),
        ("SafeArrayCreateEx", 42),
        ("SafeArrayCreateVectorEx", 43),
        ("SafeArraySetRecordInfo", 44),
        ("SafeArrayGetRecordInfo", 45),
        ("SafeArrayGetVartype", 77),
        ("VariantChangeTypeEx", 147),
        ("SafeArrayPtrOfIndex", 148),
        ("SysStringByteLen", 149),
        ("SysAllocStringByteLen", 150),
        ("VarUI4FromStr", 277),
        ("VarI4FromStr", 64),
        ("VarR8FromStr", 84),
        ("VarDateFromStr", 94),
        ("VarCyFromStr", 104),
        ("VarBstrFromBool", 116),
        ("VarBoolFromStr", 125),
        ("VarCmp", 176),
        ("RegisterActiveObject", 33),
        ("RevokeActiveObject", 34),
        ("GetActiveObject", 35),
        ("VarR4FromR8", 71),
        ("VarR8FromR4", 81),
        ("VarBstrFromI4", 110),
        ("VarBstrFromR4", 111),
        ("VarCyFromR8", 102),
        ("VarBstrFromCy", 113),
        ("SystemTimeToVariantTime", 184),
        ("VariantTimeToSystemTime", 185),
        ("LoadTypeLib", 161),
        ("LoadRegTypeLib", 162),
        ("RegisterTypeLib", 163),
        ("UnRegisterTypeLib", 186),
        ("QueryPathOfRegTypeLib", 164),
        ("LoadTypeLibEx", 183),
        ("VarCyFromDec", 228),
        ("VarBstrCmp", 314),
        ("GetRecordInfoFromGuids", 322),
        ("GetRecordInfoFromTypeInfo", 323),
        ("SafeArrayCreateVector", 411),
        ("VarR8FromCy", 82),
        ("VarR8FromDate", 83),
        ("VarDateFromR8", 92),
        ("VarDateFromCy", 93),
        ("VarDateFromBool", 96),
        ("VarCyFromDate", 103),
        ("VarBstrFromR8", 112),
        ("VarBoolFromDate", 123),
        ("VarBoolFromCy", 124),
        ("VarR4FromStr", 74),
        ("VarBstrFromDate", 114),
        ("VarAdd", 141),
        ("VarAnd", 142),
        ("VarDiv", 143),
        ("VarIdiv", 153),
        ("VarMod", 155),
        ("VarMul", 156),
        ("VarOr", 157),
        ("VarSub", 159),
        ("VarXor", 167),
        ("VarNeg", 173),
        ("VarNot", 174),
        // What Office's Click-to-Run UI (OfficeC2RClient.exe) imports.
        ("SafeArrayRedim", 40),
        ("VarDecAdd", 177),
        ("VarDecDiv", 178),
        ("VarDecMul", 179),
        ("VarDecSub", 181),
        ("VarDecAbs", 182),
        ("VarDecFix", 187),
        ("VarDecInt", 188),
        ("VarDecNeg", 189),
        ("VarDecFromI4", 192),
        ("VarDecFromR8", 194),
        ("VarDecRound", 203),
        ("VarDecCmp", 204),
        ("VarR8FromDec", 220),
        ("OleCreateFontIndirect", 420),
    ];
    for (name, ordinal) in ordinals {
        println!("cargo:rustc-cdylib-link-arg=/EXPORT:{name},@{ordinal}");
    }
    for (name, ordinal) in [("GetErrorInfo", 200), ("SetErrorInfo", 201), ("CreateErrorInfo", 202)]
    {
        println!("cargo:rustc-cdylib-link-arg=/EXPORT:{name}=ole32.{name},@{ordinal}");
    }

    // Keep rpcrt4.dll in the import table. The import is declared in
    // src/lib.rs (raw-dylib) but nothing calls it, and the linker drops an
    // import whose only reference is a static it also drops, so the name is
    // pinned on the link command line instead. Why the dependency is there
    // at all: ARCHITECTURE "The module graph". gala has no 32-bit rpcrt4,
    // so the 32-bit build goes without it.
    if std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() != Ok("x86") {
        println!("cargo:rustc-cdylib-link-arg=/INCLUDE:__imp_RpcRaiseException");
    }
}
