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
    BCryptDuplicateKey
    BCryptEnumAlgorithms
    BCryptEnumContextFunctionProviders
    BCryptEnumContextFunctions
    BCryptEnumContexts
    BCryptEnumProviders
    BCryptEnumRegisteredProviders
    BCryptFinalizeKeyPair
    BCryptFreeBuffer
    BCryptGenerateKeyPair
    BCryptGetFipsAlgorithmMode
    BCryptQueryContextConfiguration
    BCryptQueryContextFunctionConfiguration
    BCryptQueryContextFunctionProperty
    BCryptQueryProviderRegistration
    BCryptRegisterConfigChangeNotify
    BCryptRemoveContextFunction
    BCryptResolveProviders
    BCryptSecretAgreement
    BCryptSetContextFunctionProperty
    BCryptSignHash
    BCryptUnregisterConfigChangeNotify
    BCryptVerifySignature
);
