//! Exports SQL Server LocalDB imports that gala does not implement yet
//! (`gala_stubs`): calling one reports `dll!function` to the ledger and
//! ends the process.

gala_stubs::unimplemented_exports!("kernel32.dll":
    AllocateUserPhysicalPagesNuma
    // Inno Setup's compiler (ISCmplr.dll, which its IDE loads) writes the
    // icons and version of the Setup it makes with these: rewriting a
    // PE's resource section is not there yet.
    BeginUpdateResourceW
    EndUpdateResourceW
    UpdateResourceW
    CancelSynchronousIo
    CheckTokenMembershipEx
    ConvertThreadToFiber
    CreateFiber
    CreateFileMappingNumaW
    CreateHardLinkA
    EnumResourceLanguagesA
    FindFirstChangeNotificationW
    FindNextChangeNotification
    GetNamedPipeHandleStateA
    GetNamedPipeInfo
    GetNumaNodeProcessorMaskEx
    GetSystemTimeAdjustment
    GetTapeParameters
    GetTapePosition
    GetTapeStatus
    GlobalMemoryStatus
    MapUserPhysicalPagesScatter
    MapViewOfFileExNuma
    OpenJobObjectW
    PrepareTape
    ReadConsoleOutputA
    RemoveDirectoryA
    ScrollConsoleScreenBufferA
    SetFileIoOverlappedRange
    SetFileValidData
    SetProcessAffinityMask
    SetProcessAffinityUpdateMode
    SetTapeParameters
    SetTapePosition
    TerminateJobObject
    VerLanguageNameW
    WaitNamedPipeA
    WaitNamedPipeW
    WriteConsoleOutputA
    WriteTapemark
    ClearCommBreak
    ClearCommError
    CopyFileA
    CreateDirectoryExA
    CreateTapePartition
    EraseTape
    EscapeCommFunction
    GetBinaryTypeW
    GetCommModemStatus
    GetCommState
    GetConsoleProcessList
    GetCurrentConsoleFontEx
    GetNamedPipeClientProcessId
    GetNumberOfConsoleMouseButtons
    IdnToUnicode
    PurgeComm
    ReadConsoleOutputW
    ScrollConsoleScreenBufferW
    SetCommBreak
    SetCommMask
    SetCommState
    SetCommTimeouts
    SetComputerNameExW
    TransmitCommChar
    WaitCommEvent
    WriteConsoleInputW
    WriteConsoleOutputW
    EnumResourceNamesW
    ReadConsoleOutputCharacterA
    // Inno Setup's Setup (Delphi's VCL) imports these.
);
