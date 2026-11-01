//! oleaut32's exports have standard ordinals, and programs (CoreCLR)
//! import them by ordinal only. The numbers are those of Windows'
//! oleaut32.dll, taken from Wine's dlls/oleaut32/oleaut32.spec (LGPL; the
//! numbers only, recorded in THIRD_PARTY.md). Get/Set/CreateErrorInfo
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
        ("LoadTypeLib", 161),
        ("LoadRegTypeLib", 162),
        ("QueryPathOfRegTypeLib", 164),
        ("LoadTypeLibEx", 183),
        ("VarCyFromDec", 228),
        ("GetRecordInfoFromGuids", 322),
        ("GetRecordInfoFromTypeInfo", 323),
        ("SafeArrayCreateVector", 411),
    ];
    for (name, ordinal) in ordinals {
        println!("cargo:rustc-cdylib-link-arg=/EXPORT:{name},@{ordinal}");
    }
    for (name, ordinal) in [("GetErrorInfo", 200), ("SetErrorInfo", 201), ("CreateErrorInfo", 202)]
    {
        println!("cargo:rustc-cdylib-link-arg=/EXPORT:{name}=ole32.{name},@{ordinal}");
    }
}
