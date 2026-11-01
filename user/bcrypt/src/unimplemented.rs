//! Exports SQL Server LocalDB imports that gala does not implement yet
//! (`gala_stubs`): calling one reports `dll!function` to the ledger and
//! ends the process.

gala_stubs::unimplemented_exports!("bcrypt.dll":
    BCryptAddContextFunction
    BCryptConfigureContext
    BCryptConfigureContextFunction
    BCryptCreateContext
    BCryptDeleteContext
    BCryptDeriveKey
    BCryptDeriveKeyCapi
    BCryptDestroySecret
    BCryptEnumAlgorithms
    BCryptEnumContextFunctionProviders
    BCryptEnumContextFunctions
    BCryptEnumContexts
    BCryptEnumProviders
    BCryptEnumRegisteredProviders
    BCryptFreeBuffer
    BCryptQueryContextConfiguration
    BCryptQueryContextFunctionConfiguration
    BCryptQueryContextFunctionProperty
    BCryptQueryProviderRegistration
    BCryptRegisterConfigChangeNotify
    BCryptRemoveContextFunction
    BCryptResolveProviders
    BCryptSecretAgreement
    BCryptSetContextFunctionProperty
    BCryptUnregisterConfigChangeNotify
);
