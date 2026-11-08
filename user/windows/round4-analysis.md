# Round 4 Windows failures, classified

Source: user/windows/windows.json at HEAD of ccr-3715ad98-k2vzr6 (Windows 11 25H2,
26200, non-admin "a", session 1, Defender on), binaries from f010281, the same test
sources as HEAD. 83 entries are non-zero or did not run. Lines are the report's own
`(line L)`, read against HEAD. Round 3 items whose class and line are unchanged are only
listed at the end.

Notes for the round:

- **triage.md's `asserts:` lines are often wrong.** They quote a neighbouring line, not
  the reported one. Examples: cmd_test, kernel32_clr_test, kernel32_system_test,
  security_test, sspi_test, gdi32_test, shell32_test, user32_test, controls_test,
  wow64_import_test. Every entry below was read from the source at the reported line.
- **shlwapi_test's line 242 is url.c:242, not main.c:242.** Counting gives 99 CHECKs
  in main.c up to its line 242, plus 14 from the StrCmpLogicalW loop, which makes 113.
  url_checks adds 106 (219) and assoc_checks 15 up to url.c:242, which makes 234.
  Check 234 is exactly the reported one. main.c:242 would only be check 113.
- **The Round 3 probes in notes-from-linux.md ("New probes, from the fixes") have not
  been answered**, but two answers come out of this sweep anyway. seh_test answers the
  RtlRaiseException 0x80 question, and wow64_k32_test settles GetSystemDirectory under
  WOW64.

---------------------------------------------------------------------------------------
# 1. test-bug

## browser_api_test (check 23, no line, got -)
class: test-bug (undiagnosable as written)
asserts: one of seven `check(.., 23)` at lines 614, 628, 629, 632, 633, 634 and 635:
the EXPLICIT_ACCESS layout (TrusteeForm at +28 == TRUSTEE_IS_NAME), then
BuildSecurityDescriptorW, IsValidSecurityDescriptor, the DACL query (one ACE) and the SACL query (not present).
reading: the layout read at line 614 looks right for x64 (TRUSTEE_W at +16, TrusteeForm
at +28), so the failure is more likely in BuildSecurityDescriptorW or the queries. That is unsure.
fix or probe: give each call site its own number (23..29, renumbering the later
checks), or better, make `check` `#[track_caller]` and print
`Location::caller().line()` the way the other Rust tests do. `fail()` prints only two
digits, so numbers above 99 would print wrong as well.

## quota_test (check 10, line 620, got 16711680)
class: test-bug (missing diagnostics; the formula is unverified)
asserts: `at_limit == base + 2*2 + 1 + n` (base = GetProcessHandleCount before two
start_child calls and CreateEvent; n = the number of duplicates made until ERROR_NO_SYSTEM_RESOURCES)
reading: at_limit is exactly 0xff0000 (the per-process limit; line 617 passed), but
base + 5 + n differs from it. Something other than the event, the two children's
process and thread handles, and the duplicates holds handles. That could be handles
CreateProcess or the loader opened between base and the loop (unsure).
fix or probe: on failure, print base, n and at_limit. Also print GetProcessHandleCount right after the two
start_child calls and right after CreateEventW. Structural fix: take `before_loop =
handle_count()` just before the DuplicateHandle loop and assert `at_limit == before_loop + n`.
That isolates the loop from whatever else the process opened. Keep a separate,
printed check of `before_loop - base` if the 2*2+1 claim matters.

## kernel32_clr_test (check 132, line 570, got 0)
class: test-bug (gala path)
asserts: `LoadLibraryExW("C:\\gala\\testdll.dll", NULL, LOAD_LIBRARY_AS_DATAFILE)` gives a handle tagged `& 3 == 1`
reading: NULL. There is no C:\gala on Windows. testdll.dll sits in user/bin next to the exe.
fix or probe: build the path from the exe's own directory (GetModuleFileNameW(NULL)
up to the last `\`). Lines 588 (`C:\gala\dotnet_hello.dll`, which is in user/bin) and 597
(`C:\gala\x86_native.dll`, which is not exported to user/bin) need the same, and
x86_native.dll must be exported. Next blocker after this: line 612, see kernel32_thread_test.

## advapi32_test (check 65, line 1430, got 5)
class: test-bug (admin assumed)
asserts: `RegCreateKeyExW(HKLM, "SYSTEM\\CurrentControlSet\\Services\\GalaTestSvc", KEY_ALL_ACCESS) == 0`
(it registers a service so that LookupAccountNameW("NT SERVICE\\GalaTestSvc") resolves)
reading: ERROR_ACCESS_DENIED. A standard user cannot write HKLM\SYSTEM.
fix or probe: when the create returns 5 and the token is not elevated
(TokenElevation), print "service_sids: skipped (needs an administrator)" and return
Ok. As a non-admin case, LookupAccountNameW("NT SERVICE\\TrustedInstaller") could be
checked against its documented SID S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464,
type 5. That needs the service key in gala's registry too (unsure of the SidType).

## win32_misc_test (check 73, line 1206, got 7)
class: test-bug (machine-dependent value)
asserts: `CountClipboardFormats() == 0`
reading: the user's clipboard holds 7 formats.
fix or probe: do not assume an empty clipboard, and never empty the user's clipboard.
Assert `CountClipboardFormats()` equals the number EnumClipboardFormats yields between
OpenClipboard(NULL) and CloseClipboard, or only that GetLastError() is 0 after the call.

## wow64_test (check 1478, line 932, got 00000000 c0000004)
class: test-bug (buffer size)
asserts: `NtQuerySystemInformation(SystemProcessInformation, list, 0x4_0000, &returned) == 0`
reading: STATUS_INFO_LENGTH_MISMATCH. 256 KiB of x86 records is too small for a real
machine, which is the same bug as Round 2's kernel32_sync_test and smpsched_test. The
triage's "OSBuildNumber" text is stale.
fix or probe: loop: allocate `returned` (plus a margin) and retry until it fits, as
kernel32_sync_test now does. Check the rest of wow64_test for other fixed SPI buffers.

## conpty_test (check 4, 20 s)
class: test-bug (assumes the parent's standard handles are a console)
asserts: the child started on the pseudo console prints "child: ready"
reading: windows-output.txt shows the child's own "child: not a console, error 6" in
run-all's captured stdout. So the child's stdout was the parent's redirected pipe, not
the pseudo console, and GetConsoleMode(stdin) failed with ERROR_INVALID_HANDLE. That
matches CreateProcess's documented behaviour: with no STARTF_USESTDHANDLES, a console
child gets the parent's standard handles when those are not console handles. On gala
the parent's handles are the console, so the child falls back to the pseudo console.
fix or probe: set `dwFlags = STARTF_USESTDHANDLES` (startup[60..64] = 0x100) with all
three handles NULL in the STARTUPINFOEX (Microsoft's ConPTY samples do this), so the
child takes the pseudo console's handles. gala: check that CreateProcessW (user/kernel32)
gives a console child the parent's non-console std handles when STARTF_USESTDHANDLES
is absent. A conpty_test variant with redirected parent stdio would cover it.

## token_test (check 133, line 563, got 0x3e6)
class: test-bug (alignment), likely; unsure
asserts: `CreateRestrictedToken(full, 0, 1, &[EVERYONE.as_ptr(), 0], 1, &[SE_SHUTDOWN,0,0], 1, &[EVERYONE..], &mut filtered) != 0`
reading: ERROR_NOACCESS (998), which is what STATUS_DATATYPE_MISALIGNMENT (and an AV)
maps to. EVERYONE is `const [u8; 12]` (token_test.rs:182), with alignment 1. A SID has
ULONG subauthorities, and the kernel probes SIDs with ULONG alignment, so the SID
pointer handed to NtFilterToken may be odd. This is the same family as Round 3's threadinfo_test.
fix or probe: put the SID in a `#[repr(C, align(4))]` static (check security_test,
access_test, logon_test, wow64_adv_test and the other `[u8; 12]` SIDs passed as
pointers too). If an aligned SID still gives 998, record GetLastError with each
array NULL in turn. gala: once confirmed, have NtFilterToken (kernel/src/nt/token_calls.rs)
refuse a misaligned SID with STATUS_DATATYPE_MISALIGNMENT.

## user32_test (check 134, line 890, got -)
class: test-bug (foreground rights assumed)
asserts: `SetForegroundWindow(w) && GetForegroundWindow() == w && GetActiveWindow() == w` for a new top-level window
reading: the documented foreground lock: a process run from a background PowerShell
(not the foreground process, no recent input) may not take the foreground, so the call
returns FALSE or the window stays behind. gala's harness gives the test the foreground.
fix or probe: if SetForegroundWindow returns FALSE, assert `GetForegroundWindow() != w`
and skip the rest of the foreground part with a printed reason. Keep
SetActiveWindow/BringWindowToTop, which work within the thread. A probe that would
settle it: the same call from an interactively started console (it should then succeed).

## oleaut32_test (check 108, line 403, got -)
class: test-bug (COM not initialised), likely
asserts: `GetActiveObject(&nothing, NULL, &unk) == MK_E_UNAVAILABLE && unk == NULL`
reading: the test never calls CoInitialize/OleInitialize. On Windows GetActiveObject
goes through GetRunningObjectTable, which needs COM and otherwise gives
CO_E_NOTINITIALIZED (0x800401F0) (unsure whether unk is cleared). gala's GetActiveObject
(user/oleaut32/src/conversions.rs:419) returns MK_E_UNAVAILABLE unconditionally.
fix or probe: call `CoInitializeEx(NULL, COINIT_APARTMENTTHREADED)` first, and keep
MK_E_UNAVAILABLE. Probe for gala: the HRESULT and unk without CoInitialize. If it is
CO_E_NOTINITIALIZED, gala should return it when the thread has no apartment.

## kernel32_locale_test (check 85, line 551, got 0x61) (unchanged; Round 3 fix not applied)
class: test-bug (machine value)
asserts: `GetSystemPreferredUILanguages(MUI_LANGUAGE_NAME)` starts "en-US"
fix or probe: compare with `LCIDToLocaleName(GetSystemDefaultUILanguage())`. The probe
in notes-from-linux.md (the whole list) is still open.

---------------------------------------------------------------------------------------
# 2. windows-behaviour, enough evidence

## cmd_test (check 4, line 313, got 4)
class: windows-behaviour (cmd keeps the space before a redirection) and a test fix
asserts: `cmd.exe /c ""<me>" print > "%TEMP%cmd_test_out.txt"" exits 0`
reading: 4 is not anything the "print" child returns, since it exits 0. Windows' cmd
passes the program `"<me>" print ` with the space before `>` kept, as `echo x > f`
famously writes "x ". So `child()`'s `ends(" print")` fails, and the child runs the whole
test itself with stdout on the file. Its own check 4 runs a nested cmd whose `> file`
cannot open the file the outer cmd holds ("being used by another process", exit 1). So
the child exits 4, and the outer test reads 4. gala's cmd strips trailing whitespace
(user/cmd/src/main.rs:430, "Trailing whitespace (before a removed redirection) goes").
fix or probe: test: in child(), trim trailing spaces before matching (and treat an
unknown mode as exit 99, not as a full re-run). gala: remove the trim at
user/cmd/src/main.rs:430-433 so the command line is passed as written. Optional confirmation:
have "print" also write GetCommandLineW, or run `cmd /c "<me>" child 7 > nul` (it should give 7 only after the trim fix).

## kernel32_file_test (check 41, line 418, got 0)
class: windows-behaviour (an unknown flag is accepted); a narrow probe for the refusal rule
asserts: `NtFlushBuffersFileEx(h, 0x100, NULL, 0, &iosb) == STATUS_INVALID_PARAMETER`
reading: STATUS_SUCCESS on NTFS. Windows does not refuse 0x100. gala refuses any flag
outside 0..7 (user/testntdll/src/lib.rs:1994).
fix or probe: probe flags 0x8, 0x10, 0x100, 0x8000_0000 and 0xffff_ffff, plus a non-NULL Parameters
with size 0 and with size 4, and record the statuses. Test and gala then follow what is
refused. If nothing is, drop the flag check and keep only what is measured.

## kernel32_thread_test (check 54, line 359, got 0x10000000)
class: windows-behaviour (as SetThreadDescription in Round 3)
asserts: `GetThreadDescription(t, &back) == 0`
reading: GetThreadDescription also returns HRESULT_FROM_NT(STATUS_SUCCESS) =
0x10000000 on success.
fix or probe: tests: expect 0x1000_0000 here and at kernel32_clr_test:612. gala:
user/kernel32/src/runtime.rs, the success `0` at the end of GetThreadDescription ->
`nt_hresult(STATUS_SUCCESS)`.

## seh_test (check 51, line 693, got 0x80)
class: windows-behaviour (answers the open probe in notes-from-linux.md)
asserts: the record the inner handler sees from `RtlRaiseException(record with ExceptionFlags 0)` has flags 0
reading: 0x80, EXCEPTION_SOFTWARE_ORIGINATE. RtlRaiseException itself sets it (seh_raise
calls the ntdll export directly), not only kernel32's RaiseException.
fix or probe: test: expect 0x80 (line 693). gala: set the bit in testntdll's
RtlRaiseException (user/testntdll/src/seh.rs ~1090/1169, the x64 path and the x86 one
if WOW64 has its own). The explicit `| EXCEPTION_SOFTWARE_ORIGINATE` in
user/kernel32/src/mapping.rs:684 then becomes redundant, and its comment ("whether
RtlRaiseException itself sets it is not known") should change. Unsure whether NtRaiseException
from user code also sets it.

## security_test (check 33, line 344, got 0)
class: windows-behaviour (strong inference; a cheap confirmation is below)
asserts: (count 1-32 from run(); check 33 is the second field of the first ACE of
a.txt, the file created with a NULL SA in the directory) `ACE flags == 0x10
(INHERITED_ACE)`, the Everyone ACE inherited from `D:P(A;OICI;FA;;;WD)(A;;FR;;;SY)`
reading: the inherited ACE's flags are 0. The parent's descriptor came from SDDL
without "AI", so SE_DACL_AUTO_INHERITED is clear. Windows marks inherited ACEs with
INHERITED_ACE (and the child's SE_DACL_AUTO_INHERITED) only when the parent's DACL is
auto-inherited (the SEF_DACL_AUTO_INHERIT behaviour of CreatePrivateObjectSecurityEx).
gala always sets INHERITED_ACE (kernel/src/nt/security.rs:204-256, inherited_aces).
fix or probe: confirm: the same run with the parent made from `D:PAI(A;OICI;FA;;;WD)(A;;FR;;;SY)`,
where the file's ACE should be 0x10 and the subdirectory's 0x13. Test: expect 0x00 / 0x03
for the "D:P" parent, and add the "D:PAI" case expecting 0x10 / 0x13. gala: pass the
parent's control into inherited_aces and OR INHERITED_ACE only when it has
SE_DACL_AUTO_INHERITED (the control-bit logic at security.rs:341-355 must agree). Then
recheck persist_test and accounts.rs, which build AI descriptors.

## threadinfo_test (check 23, line 167, got 1)
class: windows-behaviour
asserts: after `NtSetInformationThread(t, ThreadBasePriority, 1)`,
THREAD_BASIC_INFORMATION.BasePriority (+44) == 9 (process base 8 + 1)
reading: 1. ThreadBasicInformation's BasePriority is the increment relative to the
process's base (what KeQueryBasePriorityThread gives), not the absolute priority.
SYSTEM_THREAD_INFORMATION's BasePriority (sysinfo.rs) is a different field and stays absolute (unsure).
fix or probe: test: expect 1 (and -2..2 / ±15 saturation as the increment). gala:
kernel/src/nt/syscalls.rs:3388 writes the stored absolute `base`. Write `base -
PROCESS_BASE_PRIORITY` (or store the increment; kernel/src/nt/thread.rs:139). Probe for
saturation: increment 15 reads 15? -15 reads -15?

## sspi_test (check 71, line 648, got 1)
class: windows-behaviour
asserts: `InitSecurityInterfaceA()->dwVersion == 4`
reading: Windows' ANSI table reports version 1 (SECURITY_SUPPORT_PROVIDER_INTERFACE_VERSION).
The triage's "GALA\\gala user name" text is stale.
fix or probe: test: expect 1 (the module doc's "version 4" too). gala:
user/secur32/src/sspi.rs:1165 `TABLE_A.version.store(4)` -> 1. Worth a one-line probe:
InitSecurityInterfaceW's version (gala says 4), to settle the W table at the same time.

## udp_test (check 86, line 517, got 0xffffff01)
class: windows-behaviour
asserts: `getsockopt(fresh, SOL_SOCKET, SO_RANDOMIZE_PORT)` into an int initialised to -1 reads 1
reading: only the low byte was written (0xffffff01). Windows stores this option as a
1-byte BOOLEAN. optlen likely comes back as 1 (unsure).
fix or probe: test: start v at 0 and check `v & 0xff == 1` and the returned len (record
it once: 1 or 4). gala: user/ws2_32/src/lib.rs (~1424/1538), getsockopt SO_RANDOMIZE_PORT
writes one byte and sets *optlen to what the probe shows. Check the IP_DONTFRAGMENT read
at the next lines the same way.

## wow64_k32_test (check 28, line 207, got 0x73)
class: windows-behaviour
asserts: GetSystemDirectoryW in a 32-bit process == "C:\\Windows\\SysWOW64" (case-insensitive; len 19 passed)
reading: dir[11] = 's' (0x73), but "SysWOW64"[0] is 'S' (0x53), which the
case-insensitive check would have accepted. So Windows returned "C:\WINDOWS\system32",
also 19 characters. Under WOW64, GetSystemDirectory gives System32 (the file system
redirector maps it), and only GetSystemWow64Directory gives SysWOW64.
fix or probe: test: expect "C:\\Windows\\System32" case-insensitively, and check
GetSystemWow64DirectoryW separately (32-bit: ERROR_CALL_NOT_IMPLEMENTED? unsure). gala:
user/kernel32/src/x86.rs:80 SYSTEM_DIRECTORY = `\\system32`. That needs gala's
WOW64 file system redirection (System32 -> SysWOW64 for 32-bit processes) to exist, or
32-bit programs that open GetSystemDirectory paths break. Check the kernel's open path
for it before changing.

## cert_test (check 48, line 654, got 1)
class: windows-behaviour (the return value is known; the buffer content is not)
asserts: `CertGetNameStringA(cert, SIMPLE_DISPLAY, 0, NULL, narrow, 5)` returns 5 with "gala\0" (truncated)
reading: it returns 1. The A form does not truncate. With a too-small buffer it
evidently gives the empty string (count 1). The W form's truncation was not tested
at this size.
fix or probe: test: `got == 1 && narrow[0] == 0` (record narrow[0..5] once to confirm it
is "\0" and not untouched). Also add the W form with cchString 5. gala:
user/crypt32/src/misc.rs:137-150, when `needed > len`, write a single NUL and return 1.

## kernel32_misc_test (check 34, line 547, got 0x3c860000)
class: windows-behaviour (the layout), with a layout-proof test fix
asserts: `RtlPcToFileHeader(ExceptionRecord.ExceptionAddress) == GetModuleHandleW("kernel32.dll")` after RaiseException
reading: the result is a module base whose low half is 0x3c860000, a different module
from kernel32. kernel32!RaiseException forwards to KernelBase, where the raise happens
(very likely KernelBase; unsure until named).
fix or probe: test: compare with `RtlPcToFileHeader(GetProcAddress(kernel32, "RaiseException"))`,
the module that really holds RaiseException on either system. Optional probe:
GetModuleFileNameW of the base. gala: no change needed; moving RaiseException into kernelbase is optional.

## loader_test (check 31, line 189, got 0x12)
class: windows-behaviour (likely ntdll's entry); a light probe would name it
asserts: for each InLoadOrder entry, `BaseDllName.Buffer == FullDllName.Buffer + Full.Length - Base.Length`
reading: the failing entry's BaseDllName is 18 bytes, "ntdll.dll" (the exe's name is
30 bytes, and kernel32/KernelBase are longer). The first entry passed. On Windows,
ntdll's BaseDllName evidently points at a separate string, not into FullDllName.
fix or probe: test: compare the contents (BaseDllName equals the tail of FullDllName
case-insensitively) instead of the pointer. Probe: print the entry index and both
buffers for that entry. gala: none needed (pointing into FullDllName is allowed).

---------------------------------------------------------------------------------------
# 3. probe needed

## kernel32_system_test (check 90, line 631, got 0x18)
class: probe needed (windows-behaviour: the high half is set)
asserts: `VerSetConditionMask(0, VER_MAJORVERSION, VER_GREATER_EQUAL) == 0x18`
reading: the low 32 bits are 0x18 (eq prints `got as u32`), so the high half is non-zero.
Possibly bit 63 set as a marker (0x8000_0000_0000_0018); unsure.
fix or probe: print the full u64 for (0, MAJOR, GE), (0, MINOR, EQUAL), (0, BUILDNUMBER, LESS)
and chained (MAJOR then MINOR). gala: user/testntdll/src/lib.rs:2179. The kernel32_py_test
VerifyVersionInfoA use at line 433 should still pass with the real value.

## shlwapi_test (check 234, url.c:242, got -)
class: probe needed (windows-behaviour)
asserts: `AssocQueryStringW(0, ASSOCSTR_COMMAND, ".galatest", NULL, out, &len /* 4 */) == E_POINTER`, then len == needed
reading: the docs say E_POINTER for a short buffer. Windows returns something else,
perhaps HRESULT_FROM_WIN32(ERROR_INSUFFICIENT_BUFFER) 0x8007007A (unsure). Lines 239-240
(the NULL-buffer size query: S_FALSE and the size) passed.
fix or probe: record the HRESULT, len after the call, and `out` (truncated or untouched).
gala: user/shlwapi AssocQueryStringW, the short-buffer branch.

## ole32_test (check 32, line 451, got -)
class: probe needed
asserts: after `CoCreateInstanceEx(.., 2, {{IUnknown, NULL, 7}, {uia, (IUnknown*)1, 7}}) == E_INVALIDARG`
(line 450 passed), the array is untouched
reading: Round 3's probe had pItf=1 in every entry. Here entry 0 is valid, and Windows
changed something: perhaps entry 0's hr, or it NULLs or sets E_INVALIDARG per entry (unsure).
fix or probe: record bad[0].hr, bad[0].pItf, bad[1].hr and bad[1].pItf, and run the same
with the bad entry first. gala's validation then follows.

## controls_test (check 49, line 225, got -)
class: probe needed (likely windows-behaviour)
asserts: `CB_RESETCONTENT == CB_OKAY (0) && CB_GETCOUNT == 0`
reading: the docs say CB_RESETCONTENT "always returns CB_OKAY". Wine's combo returns TRUE,
so Windows likely returns 1 (unsure which half failed).
fix or probe: record the two return values separately (on the CBS_DROPDOWNLIST box of
the test). gala: user/comctl32 or user32's combo box, CB_RESETCONTENT.

## comctl32_test (check 56, line 319) and gui_test (non-fatal check 7, line 685)
class: probe needed (tooltips in comctl32 5.82, no manifest)
asserts: comctl32_test: after TTM_ADDTOOLW (TTF_IDISHWND|TTF_SUBCLASS, uId = parent)
succeeded, `TTM_GETTOOLCOUNT == 1`. gui_test: TTM_ADDTOOLW (TTF_IDISHWND|TTF_SUBCLASS,
uId = an edit, owner the top window) != 0, which gave 0.
reading: both use cbSize = 72 (TTTOOLINFOW_V3 size with lpReserved) against v5. One
ADDTOOL reports success with no tool counted, the other fails. A cbSize the v5 control
does not accept is the usual suspect (unsure).
fix or probe: in each, record the ADDTOOL return, the GETTOOLCOUNT, and then the same with
cbSize = TTTOOLINFOW_V2_SIZE (offsetof lpReserved = 64) and TTTOOLINFOW_V1_SIZE
(offsetof lParam = 56). If V2 works, the tests use V2 for v5. gala's tooltip should then
refuse what v5 refuses (or count what it counts).

## dbghelp_test (check 23, line 197, got -)
class: probe needed
asserts: SymFromAddr(proc, &SymFromAddr + 1) gives Name "SymFromAddr", disp 1 and ModBase == dbghelp's base
reading: the name matched (the "SymFromAddr gave" line was not printed), so disp or
ModBase differs. System dbghelp may find a public symbol, or ModBase may be the loaded
module image from SymLoadModuleExW with a different base (unsure).
fix or probe: print disp, ModBase, Address, NameLen, Flags and Size.

## gdi32_test (check 70, line 250, got -)
class: probe needed (likely windows-behaviour or font version)
asserts: Segoe UI, lfHeight -12, on CreateDC("DISPLAY"): tmHeight 16, tmAscent 13, tmDescent 3
(line 239, internal leading == height - 12, passed)
reading: one of the three differs. Rounding of Segoe UI's descender may give 4 / 17
(unsure). lfHeight is in pixels, so the display's DPI should not matter.
fix or probe: record tmHeight, tmAscent, tmDescent, tmInternalLeading, tmExternalLeading
and GetTextFaceW. gala: user/gdi32's metric scaling for Segoe UI.

## richedit_test (check 12, line 151, got -)
class: probe needed
asserts: `EM_GETTEXTLENGTHEX(GTL_USECRLF, 1200) == 12` for "Hello\r\nworld" (GTL_DEFAULT gave 11, which passed)
reading: possibly the final paragraph mark counted, or GTL_CLOSE's approximation (the
default without GTL_PRECISE). Unsure.
fix or probe: record the result for GTL_USECRLF, GTL_USECRLF|GTL_PRECISE,
GTL_USECRLF|GTL_CLOSE and GTL_NUMCHARS|GTL_USECRLF, and the window class used (RICHEDIT50W?).

## shell32_test (check 56, line 520, got -)
class: probe needed
asserts: `ExtractIconW(NULL, "sh_text.txt", 0) == (HICON)1` (the docs: 1 for a file that is not an exe, DLL or icon)
reading: Windows returns something else, likely NULL (unsure). The file is created in the
current directory, user/bin.
fix or probe: record the return value and GetLastError, and try the same with an absolute path (the
documented behaviour may only hold for found files).

## wow64_import_test (check 12, line 281, got 0)
class: probe needed
asserts: testdll32's TLS callback saw DLL_PROCESS_ATTACH once (`gala_seen32(0, 1) == 1`)
reading: it never ran, or ran with another reason. The DLL's TLS directory looks right
in user/bin/testdll32.dll: data dir 0x200c/0x18, AddressOfCallBacks 0x402004 ->
{0x401000, 0}, AddressOfIndex 0x40302c, Characteristics 0x00300000. It is relocated
(preferred 0x400000 = the exe's). DllMain's counters were not checked yet.
fix or probe: print gala_seen32(0, 0..3), gala_seen32(1, 0..3), gala_order32(),
gala_attach_reserved32() and gala_tls_get32(0) before the checks. Outside probe: from 32-bit
PowerShell (SysWOW64\WindowsPowerShell), LoadLibrary("testdll32.dll") and call
gala_seen32(0,1) and gala_seen32(1,1). That separates static-import TLS callbacks
from dynamic loads.

## clusapi_test (crash 0xc0000005, resutils+0x2a724) (unchanged from Round 3)
class: probe needed
reading: the read address 0x753399fb14 looks like a stack address, so resutils walks
off the stack-held list. The list built by push_value looks documented (count 2;
NAME 0x00040003; LIST_VALUE_DWORD 0x00010002; SZ 0x00010003; lengths padded to 4;
ENDMARK written as 8 bytes, syntax 0 and length 0).
fix or probe: P/Invoke from PowerShell: build the same 112-byte list in a byte[] and call
ResUtilFindDwordProperty(list, 112, "port", out v). Then try with ENDMARK as 4 bytes,
and with name "Port". That says whether the list or the machine (no Failover
Clustering) is the cause. In-test alternative: a line before each ResUtil call.

## Still open from Round 3, same line (see round3-analysis.md for the probe)
fpu_test 250 (MXCSR of a new thread; FltSave.MxCsr is left 0 by Context::for_thread),
ioapc_test 130, registry_test 813, tls_test 298, process_test 683, uipi_test 345 (the
child's "check 30 (line 200), error 5" is only the parent having died before go3),
winsta_test 470 (handle 0xd0 now), gui_test 815, winsock_async_test 483, uia_test 1665,
wow64_thread_test 261 (was 260; still a raw x86 NtCreateThread 0x4e giving
c0000022).

---------------------------------------------------------------------------------------
# 4. environment (settled; listed for completeness)

- access_test 389, logon_test 320, profile_test 210 (no galatest account, 0x52e);
  restricted_test 367 (not admin; 0x3e6 is a stale last error); system_test 287 (not
  SYSTEM); session_test 436 (session 1); schannel_test (skipped, admin).
- disk_test: line changed 355 -> 395 only because the scratch-disk guard was added. It
  is the same CreateFileW(PhysicalDrive0) error 5. The Round 3 "skip when access is denied and not elevated" is still not done.
- wow64_adv_test 505 (HKLM, admin), tds_test (no C:\sql), uia_chromium_test 116,
  hostnet_test step 8, console_window_test, winhttp_test and wininet_test (timeouts).
- tcc, tcc_make, tcc_self, lua, sqlite (126: Defender quarantines tcc.exe). bench passes now.
- persist_test 489 (state across runs; Round 3 test-bug, unchanged: it needs the runner to
  clear C:\persist and HKCU\Software\GalaPersist and judge the second run).

# 5. gala-only

- handover_test (check 7, was 6): its next blocker is the literal
  `\??\C:\gala\handover_test.exe` (main.rs:303). It could use PEB ImagePathName (the test
  already reads ProcessParameters at line 209), but GalaRequestHandover follows, so it
  stays gala-only.
- clock_test, handover_k32_test, handover_net_test (0xc000041d, gala private services);
  gala_cli_test 936 and pk_install_test 507 (`gala` commands, 0xfffe0003);
  gpu_stub_test 2; unimplemented_test 3; cmd, conhost, gala (0xc0000139, Gala* exports).

# 6. not a divergence

- crash (AV on purpose), missing_import_test (0xc0000139 expected), go_exit 3, busybox 3,
  busybox_script 4 (the scenarios' expected codes), statedump_test (never ends by design).
