# Round 5 Windows failures, classified

Source: user/windows/windows.json and windows-output.txt (Windows 11 25H2, 26200,
non-admin "a", session 1, Defender on), binaries from 68638d3. Every line below was
read from `git show 68638d3:<path>` at the report's own `(line L)`; triage.md's
`asserts:` is again off in several entries (browser_api_test: 670 is the SACL check,
not the DACL one; comctl32_test: 319 is TTM_ADDTOOLW itself; shlwapi_test: url.c:250 is
the `.galanone` query, not the short buffer; disk_test, kernel32_file_test and
kernel32_thread_test quote older rounds' text).
wait_test and wow64_test are skipped, as asked (both fixed in 10876a7).

Round 4's 13 probes (notes-from-linux "New probes from the fixes") were not run this
round, but this sweep answers two of them: #13 (seh_test: the unwind sees 0x02, not
0x82) and, in part, #9 (security_test: a file under a "D:PAI" directory does not get
SE_DACL_AUTO_INHERITED).

On on_gala: `ntrt::on_gala()` exists only in user/ntrt/src/x86.rs, and only at HEAD
(10876a7, after 68638d3). **user/ntrt/src/x64.rs has none.** x64.rs does have
`find_ntdll()` and `find_export()` (and `ntdll_export()`), so the x64 version is the
same expression: `find_ntdll().is_none_or(|b| find_export(b, b"GalaConsoleCreate").is_some())`.
Every x64 test listed below as "skip off gala" needs it added first.

Likely cause of the three 0xc000041d crashes (clock_test, handover_k32_test,
handover_net_test), inferred and unsure: gala's private numbers 0x7f4c, 0x7f5a and
0x7f5b have bit 12 set. On x64 Windows that bit picks the win32k (shadow) service table.
The first such call converts the thread to a GUI thread, and that takes a user-mode
callback through KernelCallbackTable, which is NULL without user32. So the fault lands in
ntdll's callback dispatcher (ntdll+0x165d8b, the same address in both), and the result
is STATUS_FATAL_USER_CALLBACK_EXCEPTION. That fits triage's fault.py reading. Round 5's
wow64_test crash (0x7f02) has the same root. The fix is the same as for any gala-only
service: do not call it off gala.

=======================================================================================
# 1. test-bug

## advapi32_test (check 98, line 1265, got 5)
class: test-bug (path: a file in C:\ root)
asserts: `CreateFileW("C:\\gala-advapi32-sec.txt", GENERIC_READ|GENERIC_WRITE|WRITE_DAC, CREATE_ALWAYS)` succeeds
reading: ERROR_ACCESS_DENIED. C:\'s DACL lets Authenticated Users create folders but not
files, so a standard user cannot create a file in the drive root. The service_sids part
before it skipped correctly ("needs an administrator").
fix or probe: create the file in %TEMP% (GetTempPathW + "gala-advapi32-sec.txt"), as
Round 3 did for the other tests. Grep the test for other `C:\\gala-*` file paths.

## kernel32_file_test (check 120, line 701, got 25)
class: test-bug (machine-dependent value)
asserts: `QueryDosDeviceW("C:") == "\\Device\\HarddiskVolume1\0\0"` (n == 25)
reading: n is 25, which is exactly the length of "\Device\HarddiskVolumeN\0\0" with one
digit. The length matched (the check reports n), so the text differs: the volume number.
On this machine C: is HarddiskVolume3 or similar (the EFI and recovery partitions come
first). triage.md's "leftover %TEMP%\gala_k32" note is from Round 1 and does not apply.
fix or probe: assert the prefix "\\Device\\HarddiskVolume", then at least one digit, then
two NULs, with n equal to the length plus 2. Keep the exact `1` only under gala (or
compare with what the test reads from \??\C: through NtQuerySymbolicLinkObject, which is
machine-independent). NUL -> \Device\Null at the next line stays exact.

## kernel32_locale_test (check 88, line 314, got 0x401)
class: test-bug (machine-dependent value)
asserts: `GetSystemDefaultLCID() == 0x409`
reading: 0x401 is ar-SA. The user locale (line 313) is 0x409, and the system locale (the
"language for non-Unicode programs") is Arabic on this machine. console_window_test's
mojibake ("├ح├ج├╢" for äöü) is the OEM code page 720 that goes with it.
fix or probe: assert `IsValidLocale(GetSystemDefaultLCID(), LCID_INSTALLED)`, and assert
that it equals `LocaleNameToLCID(GetSystemDefaultLocaleName())`. Keep 0x409 for gala
only. Check the rest of the test for ACP/OEMCP == 1252/437 assumptions (GetACP,
GetOEMCP), which follow the system locale too.

## kernel32_system_test (check 96, line 664, got 0)
class: test-bug (machine-dependent value: the build)
asserts: `RtlVerifyVersionInfo({10, 0, 26101}, MAJOR|MINOR|BUILD, >= mask) == STATUS_REVISION_MISMATCH`
reading: STATUS_SUCCESS, because the machine is build 26200, which is >= 26101. The test
assumes the pinned 26100.
fix or probe: read the running build (RtlGetVersion, or PEB.OSBuildNumber) and use
build + 1 for the "too new" case and build for the "equal" one. The checks after it
(major 11) are machine-independent.

## loader_test (check 182, line 432, got 0xc0000135)
class: test-bug (gala path)
asserts: `LdrLoadDll(path flags 0, "C:\\gala\\sub\\testdll2.dll")` succeeds
reading: STATUS_DLL_NOT_FOUND. There is no C:\gala\sub on Windows. The loop's second
case (0x101, LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR) and its "\\gala\\testdll.dll" expectation
have the same problem.
fix or probe: at run time, make %TEMP%\gala_loader\sub, copy testdll2.dll and testdll.dll
there from the exe's directory (GetModuleFileNameW), use that path, and expect the
dependency from "<exe dir>\\testdll.dll" (flags 0) or from "\\sub\\testdll.dll" (0x101).
Remove the directory at the end (leftovers moved checks in earlier rounds).

## shell32_test (check 87, line 225, got -)
class: test-bug (COM not initialised before the shell calls), likely; unsure
asserts: `SUCCEEDED(CoInitialize(NULL))` at the start of links()
reading: CoInitialize fails, so the thread already has an apartment of the other kind,
most likely RPC_E_CHANGED_MODE (0x80010106). main() calls ShellExecuteExW, ShellExecuteW,
SHFileOperationW and ShellExecuteA first without initialising COM. ShellExecuteEx's
documentation asks the caller to initialise COM first, because it may go through shell
extensions. Evidently one of them leaves the thread in the MTA. That is unsure: which
call, and whether it is the MTA.
fix or probe: `CoInitializeEx(NULL, COINIT_APARTMENTTHREADED)` at the top of main
(asserted S_OK), with links() accepting S_FALSE. Optional confirmation: print the
HRESULT at line 225, and `CoGetApartmentType` after each shell call. gala: if gala's
shell32 never touches the apartment, nothing needs to change.

## persist_test (check 14, line 489, got 0xb359ad23) (unchanged; Round 3 test-bug)
class: test-bug (state across runs, plus last-access updates)
asserts: second boot: a.txt's last-access time == ACCESSED (0x01d80000_12345678, 2022)
reading: the same 0xb359ad23 in Rounds 3, 4 and 5. C:\persist is left over, so every
sweep takes the second-boot path. The access time moved once, after the first boot, and
has stayed since. NTFS updates the last-access time on a read when the stored one is more
than an hour old, so any reader of a.txt moves it: Defender scans new files, and so does
the indexer. Even a clean pair of runs on Windows can fail this check.
fix or probe: in run-all.ps1, delete C:\persist and HKCU\Software\GalaPersist, run twice,
and judge the second run. In the test (off gala): assert `accessed >= ACCESSED`, not
equality, or set ACCESSED to the current time minus a few minutes (inside NTFS's one-hour
granularity) and compare that.

## win32_misc_test (check 73, line 1213, got 5)
class: test-bug (shared per-user state: the user's clipboard); unsure whether it was one-off
asserts: `OpenClipboard(NULL)` succeeds (Round 4's fix: count == what EnumClipboardFormats yields)
reading: ERROR_ACCESS_DENIED. OpenClipboard fails while another window has the clipboard
open, and on a user's desktop another process may (clipboard history, a terminal, a
remote-desktop or VM clipboard agent). That is not under the test's control.
fix or probe: on failure, record GetOpenClipboardWindow() and its process
(GetWindowThreadProcessId, then QueryFullProcessImageNameW), so the cause is named. Then
off gala, when the error is 5 and another window holds it, print "clipboard part skipped
(held by <image>)" rather than fail. A re-run says whether it is persistent.

## access_test (check 1, line 389), logon_test (check 1, line 320), profile_test (check 1, line 210) (got 0x52e)
class: test-bug (account), was "environment"
asserts: `LogonUserW("galatest", ".", "Gala-Test-1", ...)` succeeds
reading: ERROR_LOGON_FAILURE. No galatest account exists, by the agent's choice.
fix or probe: when LogonUserW fails and LookupAccountNameW("galatest") gives
ERROR_NONE_MAPPED (1332), print "<test>: skipped (needs the galatest account)" and exit 0,
as lsa_test and advapi32_test's service_sids already do.

## restricted_test (check 2, line 367, got 0x3e6 stale) / system_test (check 3, line 287, got 0) / session_test (check 1, line 436, got 1)
class: test-bug (privileges an ordinary user lacks), was "environment"
asserts: restricted_test: the process is in BUILTIN\Administrators. system_test: the
token user is S-1-5-18. session_test: SYSTEM in session 0 (check_own_session(&c, 0) at
run()'s start).
reading: an ordinary user in session 1, as the brief's non-admin case.
fix or probe: check the precondition first and skip with a printed reason, as lsa_test
does ("skipped: needs SYSTEM"). restricted_test: CheckTokenMembership(NULL, Admins) false
and TokenElevation 0 -> "skipped (needs an elevated administrator)". system_test and
session_test: TokenUser != S-1-5-18 -> "skipped (needs SYSTEM)".

## disk_test (check 1, line 395, got 5) / wow64_adv_test (check 145, line 505 = 0x1f9, got 5)
class: test-bug (privileges), was "environment"
asserts: disk_test: CreateFileW("\\\\.\\PhysicalDrive0", GENERIC_READ|GENERIC_WRITE).
wow64_adv_test: RegCreateKeyExW(HKLM, "SOFTWARE\\gala_wow64_view", KEY_ALL_ACCESS) from a
32-bit process.
reading: ERROR_ACCESS_DENIED for a standard user in both cases (Round 3 asked for the skip
for disk_test; it is still not done).
fix or probe: on 5 with TokenElevation 0, print "skipped (needs an administrator)" and
return Ok. (wow64_adv_test's view checks could move to HKCU\Software\Classes, a
redirected key, so that they run as a standard user; whether 25H2 still redirects it is
unsure, so skip for now.)

## clock_test (crash 0xc000041d), handover_test (check 7), handover_k32_test (crash 0xc000041d), handover_net_test (crash 0xc000041d after "listening on port 7")
class: test-bug (gala-private service used on Windows), was "gala-only"
asserts: GalaClockControl (0x7f4c), GalaRequestHandover (0x7f5a) and GalaHandoverPause
(0x7f5b) are gala's test-only services. handover_test fails earlier, at check 7, on
`\??\C:\gala\handover_test.exe` (main.rs:304); GalaRequestHandover follows.
reading: see the note at the top. On Windows these numbers go to the win32k table, and
the crash follows.
fix or probe: skip off gala via ntrt::on_gala(). These are x64 programs, and **x64.rs has
no on_gala**: add it (the expression at the top) and print "<test>: skipped (Gala* is
gala's own)", as presenter_test does. handover_test: also take its own path from
PEB.ProcessParameters.ImagePathName rather than C:\gala, if any part should still run
off gala.

## gala_cli_test (check 1, line 936, got 0xfffe0003) / pk_install_test (check 1, line 507, got 0xfffe0003; "no install log")
class: test-bug (gala.exe used on Windows), was "gala-only"
asserts: `C:\gala\gala.exe help` prints "usage: gala". pk_install_test runs
`gala search hello --source C:\pk_source`.
reading: 0xfffe0003 is the test's own "could not start" code with error 3
(ERROR_PATH_NOT_FOUND): there is no C:\gala\gala.exe. user/bin's gala.exe would not
load either (0xc0000139: ntdll has no GalaSession, GalaRecordMissing or
GalaDesktopAction).
fix or probe: skip off gala (the x64 on_gala above) with a printed reason. The whole
program is about gala's own CLI.

=======================================================================================
# 2. windows-behaviour, enough evidence

## seh_test (check 62, line 717, got 2) (answers Round 4 probe #13)
class: windows-behaviour
asserts: during the unwind the inner handler sees `0x80 | EXCEPTION_UNWINDING` (0x82)
reading: 0x02. The dispatch phase saw 0x80 (line 714 passed), but RtlUnwindEx does not
keep EXCEPTION_SOFTWARE_ORIGINATE in the flags it hands the handlers.
fix or probe: test: expect 0x02 at line 717 and 0x22 at line 720 (outer(2), the target
frame: UNWINDING | TARGET_UNWIND; that one is inferred, since the run stopped first).
Fix the comment at lines 714-716. gala: user/testntdll/src/seh.rs:1024,
`let mut flags = flags | EXCEPTION_UNWINDING;`, to
`(flags & !EXCEPTION_SOFTWARE_ORIGINATE) | EXCEPTION_UNWINDING`. Whether Windows also
drops other bits (EXCEPTION_NONCONTINUABLE 0x1) is unknown. A narrow probe: the same with
RaiseException(code, EXCEPTION_NONCONTINUABLE, ..): does the unwind see 0x03 or 0x02? If
0x02, gala should set the flags anew rather than mask.

## cert_test (check 88, line 812, got 1)
class: windows-behaviour
asserts: `PFXIsPFXBlob` of the minimal PFX with its version INTEGER changed from 3 to 1 is FALSE
reading: TRUE. The same bytes with version 3 gave TRUE (line 808), so Windows does not
look at the version.
fix or probe: test: expect 1 for v1, and keep the CERT (not a SEQUENCE {INTEGER, SEQUENCE
{OID}}) case expecting 0, unverified until it runs. gala: user/crypt32/src/misc.rs:222,
drop the `outer[vs..ve] != [3]` condition and keep only "tag 0x02". Optional probe for
how loose it is: a SEQUENCE { INTEGER 3 } alone, and a wrong OID (not 1.2.840.113549.1.7.1).

## browser_api_test (check 23, line 670, got -)
class: windows-behaviour (the flag); a narrow probe for the SACL's form
asserts: BuildSecurityDescriptorW(NULL, NULL, 1 entry, 0 audit entries, NULL old) ->
`GetSecurityDescriptorSacl(sd).present == 0`
reading: line 669 (the call) passed and present is TRUE, so Windows' descriptor has
SE_SACL_PRESENT even with no audit entries. The DACL checks (667-668) passed. gala builds
an absolute descriptor without a SACL (user/advapi32/src/trustee.rs:110-190).
fix or probe: probe: the same call, then record the control word
(GetSecurityDescriptorControl), the SACL pointer from GetSecurityDescriptorSacl (NULL, or
an ACL) and, if not NULL, its AclRevision, AclSize and AceCount. Then test and gala
follow: likely `SetSecurityDescriptorSacl(abs, TRUE, NULL or empty ACL, FALSE)` in
trustee.rs, before RtlAbsoluteToSelfRelativeSD.

## cmd_test (check 18, line 377, got 1)
class: windows-behaviour (the exit code with a redirection); unsure on the general rule
asserts: `cmd.exe /c nosuchprogram 2> "%TEMP%cmd_test_out.txt"` exits 9009
reading: exit 1. cmd sets 9009 for a command it cannot find when there is no redirection.
With a redirection on the line, its error path gives 1. That is the long-known batch
quirk, and it is what was measured here. The message check (line 379) has not run yet.
gala: user/cmd/src/main.rs:120 (NOT_FOUND = 9009) is used either way.
fix or probe: test: expect 1 here, and add the same command with stderr passed through
`run(&mut line, null, file_handle)` (STARTF_USESTDHANDLES, no `2>`) expecting 9009 and the
message in the file. Cheap confirmation: `cmd /c nosuchprogram` with and without `2> f`,
recording both codes. gala: when the not-found command carries any redirection, exit 1.

## dbghelp_test (check 42, line 248, got -)
class: windows-behaviour
asserts: `StackWalk64(IMAGE_FILE_MACHINE_I386, proc, GetCurrentThread(), &frame(zeroed), &ctx(x64 RtlCaptureContext), ...)` returns FALSE
reading: TRUE. x64 dbghelp supports I386 (that is how it walks WOW64 threads and x86
dumps; the StackWalk64 documentation lists IMAGE_FILE_MACHINE_I386), so "another machine
type" does not fail. gala refuses everything but AMD64 (user/dbghelp/src/stack.rs:202).
fix or probe: test: use a machine dbghelp does not walk (e.g. 0x1234, or
IMAGE_FILE_MACHINE_POWERPC 0x1f0) for the failure case. Optional probe: after the I386
call, record frame.AddrPC/AddrFrame/AddrStack and whether a second call returns FALSE,
to decide whether gala should walk I386 too or only not assert it.

## gdi32_test (check 79, line 267, got -)
class: windows-behaviour (documented), with a confirming probe; unsure which part
asserts: `GetTextExtentExPointW("iiiiWWWW", 8, nMaxExtent = narrow.cx + 1, &fit, dx, &ex)`
gives fit 4, dx[3] == narrow.cx, dx[7] == both.cx, ex.cx == both.cx, ex.cy == tmHeight
reading: line 266 (the call) passed. The documentation says lpnDx receives the partial
extents of the characters that fit in nMaxExtent, so with fit 4 Windows fills dx[0..3]
only. dx[7] is then the stack's garbage. gala fills all 8 (user/gdi32/src/draw.rs:704).
ex (the whole string's size) is likely right.
fix or probe: probe: dx pre-filled with -1, then record fit, dx[0..8], ex.cx and ex.cy.
Test: assert dx[0..fit] only. gala: write dx only while `total <= max` when lpnFit/max
are given, if the probe shows dx[4..7] untouched.

=======================================================================================
# 3. probe needed

## kernel32_thread_test (check 81, line 458, got 31)
class: probe needed
asserts: `UpdateProcThreadAttribute(list, 0, PROC_THREAD_ATTRIBUTE_IDEAL_PROCESSOR, &ideal, 2)` fails with ERROR_BAD_LENGTH (24)
reading: it fails with ERROR_GEN_FAILURE (31). The list was initialised for 2
attributes, and both (0x30005 and 0x30003) are already in it, so this third call hits a
full list and a duplicate attribute as well as a wrong size. Windows evidently checks one
of those before the size. gala checks the size first (user/kernel32/src/process.rs:913),
then the capacity (ERROR_INSUFFICIENT_BUFFER, :931).
fix or probe: on fresh lists, record GetLastError for: (a) a 1-slot empty list, 0x30005
with size 2; (b) a 2-slot list holding 0x30005, 0x30005 again with size 4 (duplicate);
(c) a 1-slot full list, a new attribute (0x20002 HANDLE_LIST, size 8) (full). Test: do
the wrong-size case on a fresh list. gala: follow the measured order and codes.

## controls_test (check 136, line 417, got -)
class: probe needed
asserts: a CBS_DROPDOWN combo dropped with F4 on its edit (line 413 passed): WM_KEYDOWN
VK_ESCAPE sent to the edit closes it, `CB_GETDROPPEDSTATE == FALSE && last_note() == CBN_CLOSEUP`
reading: either the list stays dropped (the edit does not pass Escape on through
WM_KEYDOWN), or the last notification is not CBN_CLOSEUP (e.g. CBN_SELENDCANCEL after
it; the documentation says the order is not predictable). The same check on a
CBS_DROPDOWNLIST (line 395) passed.
fix or probe: on that combo, after SendMessageW(edit, WM_KEYDOWN, VK_ESCAPE, 0): record
CB_GETDROPPEDSTATE and every WM_COMMAND notification code in order. Then repeat with
WM_CHAR 0x1b to the edit, and with WM_KEYDOWN VK_ESCAPE to the combo itself. gala:
user/user32's combo edit subclass (where F4 is passed on).

## ole32_test (check 52, line 500, got -)
class: probe needed
asserts: a second `IBindCtx_RevokeObjectParam(ctx, L"gala")` returns S_FALSE (the documented "no object registered with the key")
reading: the first revoke returned S_OK with the refcount back to 1 (line 499), so
Windows returns something else for a key that is not there: E_FAIL is a guess.
fix or probe: record the HRESULT of RevokeObjectParam for the key revoked a moment ago,
and for a key never registered (L"never"). gala: user/ole32/src/bindctx.rs:317/325
(S_FALSE).

## shlwapi_test (check 237, url.c line 250, got -)
class: probe needed
asserts: `AssocQueryStringW(0, ASSOCSTR_COMMAND, L".galanone", NULL, out, &len = MAX_PATH) == HRESULT_FROM_WIN32(ERROR_NO_ASSOCIATION)` (0x80070483)
reading: the short-buffer checks before it (url.c 245-248, Round 4's probe) pass now.
For an extension with no key at all, Windows returns another HRESULT, likely
HRESULT_FROM_WIN32(ERROR_FILE_NOT_FOUND) 0x80070002 (unsure).
fix or probe: record the HRESULT, len after the call, and out[0], for ".galanone" (no
key), for ".galaempty" (a key with no default value and no shell\open), and for a
ProgID-less extension with only "Content Type". gala: user/shlwapi/src/assoc.rs:63/177.

## user32_test (check 148, line 1102, got -)
class: probe needed
asserts: `SetWindowPos(w, NULL, 0, 0, 320, 220, SWP_NOMOVE|SWP_NOZORDER)` on a new
hidden WS_OVERLAPPEDWINDOW gives exactly WM_WINDOWPOSCHANGING, WM_WINDOWPOSCHANGED,
WM_SIZE (pos_n == 3)
reading: a different count or order. Candidates: a second WINDOWPOSCHANGING/CHANGED
pair (e.g. from a frame change or DPI scaling on this machine), or WM_SIZE arriving
before CHANGED (unsure).
fix or probe: log every message the window procedure gets during that SetWindowPos
(msg, the WINDOWPOS flags for 0x46/0x47, the wParam for WM_SIZE). Test: count the three
in order and ignore others, if extra messages are what Windows sends. gala:
user/user32's SetWindowPos message sequence.

## security_test (check 53, line 444, got 0) (answers part of Round 4 probe #9)
class: probe needed (one half is measured)
asserts: a file created under a directory made with "D:PAI(A;OICI;FA;;;WD)(A;;FR;;;SY)"
has `control & (SE_DACL_PROTECTED | SE_DACL_AUTO_INHERITED) == 0x400`
reading: 0. The file's DACL is not marked auto-inherited even though the parent's SDDL
says AI. Either the directory itself did not keep AI (it was created with an explicit
descriptor, and the kernel may compute its control without SEF_DACL_AUTO_INHERIT), or
files created through NtCreateFile never get the bit (the Win32 propagation in advapi
sets it, not the kernel). Normal profile files do show "D:AI(...ID...)", so the first is
more likely. That is unsure.
fix or probe: record the control word and each ACE's flags for: ai_dir itself, ai_file,
ai_sub; and the same under a directory created with a NULL SA inside %TEMP% (which
inherits %TEMP%'s AI DACL). gala: kernel/src/nt/security.rs (inherited_aces and the
control bits). Then persist_test's and accounts.rs's AI descriptors follow.

## conpty_test (check 9, got -; 10.3 s)
class: probe needed (the program's own run with one change); likely test-bug, unsure
asserts: 0x03 written to the pseudo console's input calls the child's
SetConsoleCtrlHandler handler ("ctrl:handled")
reading: Round 4's STARTF_USESTDHANDLES fix works: checks 4-8 pass (ready, line editing,
resize to 100x30, raw keys). After 0x03 nothing comes back. The likely cause: whether a
process ignores Ctrl+C is inherited by its children (the SetConsoleCtrlHandler
documentation), and the harness that started run-all/PowerShell probably started it
with CREATE_NEW_PROCESS_GROUP, which disables Ctrl+C for the group. So conpty_test, and
then its child, ignore Ctrl+C, and the handler is never called. Unsure.
fix or probe: in child(), call `SetConsoleCtrlHandler(NULL, FALSE)` (documented: it
restores normal Ctrl+C processing) before adding on_ctrl, then re-run. If it passes, keep
the change: it makes the test independent of how it was started. gala:
user/kernel32/src/console.rs keeps the ignore flag per process. Check that it is
inherited by children and set by CREATE_NEW_PROCESS_GROUP (process.rs:20), with a
conpty_test case started that way.

## kernel32_misc_test (check 34, line 547, got 0x3c860000) (Round 4 probe #10, still open)
class: probe needed
asserts: `RtlPcToFileHeader(ExceptionAddress after RaiseException) == kernel32`
fix or probe: GetModuleFileNameW of that base (very likely KernelBase.dll). The test then
compares with GetModuleHandleW of that module.

## Still open from Rounds 3-4, same line and same reading (the probes there stand)
- comctl32_test (check 56, line 319: TTM_ADDTOOLW itself returns FALSE) and gui_test
  (non-fatal check 7, line 685; same ADDTOOL): cbSize = sizeof(TOOLINFOW) = 72 (V3,
  with lpReserved) against comctl32 5.82 (no manifest). That v5 refuses the V3 size is
  the well-known _WIN32_WINNT >= 0x0501 gotcha, so this is very likely windows-behaviour.
  Probe: the return with cbSize 72, 64 (TTTOOLINFOW_V2_SIZE) and 48 (V1). gala:
  user/comctl32/src/tooltips.rs:790 accepts every size.
- gui_test (check 23, line 815): SetGestureConfig(hwnd, 0, 0, NULL, 0) -> FALSE. Record
  GetLastError, and the VCL form ({0, GC_ALLGESTURES, 0}, 1, 12). The check passes 0 as
  its value; pass GetLastError instead.
- fpu_test (check 21, line 250, got 0x1f80): NtCreateThread with CONTEXT.MxCsr 0x5f80
  runs at 0x1f80. Probe with FltSave.MxCsr set as well, and with CONTEXT_FLOATING_POINT
  cleared.
- ioapc_test (check 22, line 130, got 0): NtReadFile with an ApcRoutine on a synchronous
  handle succeeds. Record the IOSB, and whether the APC runs after an alertable delay.
- registry_test (check 100, line 813, got 0x24): RegGetValueW(RRF_RT_REG_SZ) of a
  REG_EXPAND_SZ returns pcbData 36 (the stored size), not the expanded 20. Probe with a
  60-character expansion.
- tls_test (check 6, line 298, got 0x0496b7f6): the SizeOfZeroFill part of the exe's
  TLS block is not zeroed. Probe HeapSize of the block against End - Start. Likely the
  test should not assert zero-fill.
- process_test (check 74, line 683, got 0): a new job's JOBOBJECT_BASIC_LIMIT_INFORMATION
  is not all zero. Hex-dump the 64 bytes. Likely SchedulingClass (+60) = 5, the
  documented default, unsure. gala: kernel/src/nt/job.rs.
- uipi_test (check 38, line 345, last error 87): ChangeWindowMessageFilterEx(top,
  WM_APP+1, MSGFLT_RESET, &cfs). Probe with message 0, and with cfs NULL. gala:
  user/user32/src/foreign.rs:1131.
- winsta_test (check 58, line 470, got 0xd0): OpenWindowStationW(L"") succeeds (0xd0 is
  a handle). Record GetUserObjectInformationW(UOI_NAME, UOI_TYPE) on it, and the
  "foo\\bar" case.
- winsock_async_test (check 26, line 483): mswsock's GetAcceptExSockaddrs leaves both
  pointers NULL. Hex-dump accept_buf, and call it through
  WSAID_GETACCEPTEXSOCKADDRS (WSAIoctl), the documented route, as the control.
- uia_test (check 76, line 1665): record each of the window's four children's
  ControlType, Name and NativeWindowHandle.
- richedit_test (check 12, line 151): EM_GETTEXTLENGTHEX(GTL_USECRLF, 1200) for
  "Hello\r\nworld" is not 12. Record GTL_USECRLF, |GTL_PRECISE, |GTL_CLOSE and
  GTL_NUMCHARS|GTL_USECRLF, and the class (RICHEDIT50W).
- wow64_import_test (check 12, line 281 = 0x119, got 0): testdll32's TLS callback did not
  see DLL_PROCESS_ATTACH. Print gala_seen32(0/1, 0..3), gala_order32() and
  gala_attach_reserved32(); or LoadLibrary("testdll32.dll") from 32-bit PowerShell.
- wow64_thread_test (check 1, line 261 = 0x105, got c0000022): raw x86 NtCreateThread
  (0x4e) under WOW64 gives STATUS_ACCESS_DENIED. Try 32-bit ntdll's NtCreateThread
  export, then NtCreateThreadEx and CreateThread.
- clusapi_test (crash 0xc0000005 in resutils+0x2a724, no output): build the same 112-byte
  property list in PowerShell and call ResUtilFindDwordProperty(list, 112, "port").
  Also try ENDMARK as 4 bytes. Or add a line before each ResUtil call.

=======================================================================================
# 4. environment

- tcc, tcc_make, tcc_self, lua, sqlite (126): Defender quarantines the tcc.exe each
  script builds (Behavior:Win32/DefenseEvasion.A!ml); "./tcc.exe: Invalid argument".
- winhttp_test, wininet_test (timed out at 60 s): they need the nginx on 127.0.0.1:8080
  and 8443 and the test CA in LocalMachine\Root, which run-all.ps1 sets up only for
  schannel_test, and only as an administrator. Runner fix: skip both with
  $schannelSkip. Test fix (observability): print each numbered step before it runs,
  and bound the waits (WinHttpSetTimeouts; INTERNET_OPTION_CONNECT/RECEIVE_TIMEOUT), so
  a hang names its step. Why a refused localhost connect hangs rather than fails is
  unknown (something may be listening on 8080; unsure).
- schannel_test (skipped: needs an administrator for the CA).
- console_window_test (exit 1, "no line"): needs xtask to type into the console window.
  Skip when not driven.
- tds_test (step 1, 0x10b ERROR_DIRECTORY): no SQL Server under C:\sql. Locate it, or
  skip when C:\sql\Binn\sqlservr.exe is absent.
- uia_chromium_test (line 116, CreateProcessW): no C:\chromium\chrome.exe. Take the path
  from the environment and skip when it is absent.

# 5. gala-only

- cmd, conhost, gala (0xc0000139): gala's own programs, with imports of Gala* exports
  from ntdll.
- gpu_stub_test (check 2): gala's no-adapter contract. On a machine with a GPU,
  D3D11CreateDevice succeeds.
- unimplemented_test (exit 3): gala's missing-export contract. Windows' xolehlp exports
  the function for real.
- hostnet_test (step 8): needs xtask's gala-host.test -> 10.0.2.100 and page server.
  Skip when the name does not resolve off gala.

# 6. not a divergence

- crash (AV at address 0 on purpose); missing_import_test (0xc0000139 is what it
  asserts); go_exit 3, busybox 3, busybox_script 4 (their scenarios' expected codes);
  statedump_test (never ends, by design).
- Settled in Round 4, fix not yet in at 68638d3 (same line, same value):
  wow64_k32_test (check 28, line 207 = 0xcf, got 0x73 = 's'). Under WOW64,
  GetSystemDirectoryW returns "C:\WINDOWS\system32" (the redirector maps it), not
  SysWOW64 (windows-behaviour; the test and user/kernel32/src/x86.rs:80 follow).
- Skipped as asked: wait_test, wow64_test (fixed in 10876a7).
