# Round 3 Windows failures, classified

Source: origin/windows-triage user/windows/windows.json (Windows 11 25H2, 26200,
non-admin "a" on DESKTOP-0GMLOBR, session 1, Defender on), binaries from 73e727e. The
lines are at HEAD (ccr-3715ad98-k2vzr6), whose test sources are the same.

Notes that hold for several entries:

- **Raw syscalls with gala's own UNVERIFIED numbers.** WORKFLOW says tests must reach
  build-dependent services through ntdll's exports, but some tests make raw syscalls:
  `mov eax, 0xcf` (NtCreateSemaphore) in smpsem_test, safepoint_test and xstate_test;
  `nr::NT_CREATE_SEMAPHORE` (0xcf, x86) in wow64_test; and ntrt's own
  `ntrt_nt_create_timer = 0xd9` / `ntrt_nt_set_timer = 0x19e` (user/ntrt/src/x64.rs:75-78),
  which `ntrt::create_timer` / `set_timer` use (clock_test, handover_test, smprace_test).
  On Windows those numbers are other services, so the calls fail with C000000D or
  C0000005. The fix is to resolve the function with `find_ntdll()` + `find_export()`
  (and on gala, through testntdll's stub).
- **gala private syscalls (0x7fxx) on Windows.** clock_test (GalaClockControl 0x7f4c)
  and handover_k32_test (0x7f5a/0x7f5b) die with C000041D after a first-chance AV at
  ntdll+0x165d8b. A plausible cause, not checked: a service number at or above 0x1000
  goes to the win32k table, which turns the thread into a GUI thread and calls back into
  user mode, and this process loaded no user32, so it has no KernelCallbackTable. This
  answers triage's open question "what makes these two take a callback at all". They
  are gala-only either way.
- **The C tests that include url.c** (shlwapi_test): `__LINE__` belongs to whichever
  file holds the CHECK, so "line 229" is url.c:229, not main.c:229.
- Several triage.md entries are wrong for Round 3: sspi_test (the reason given is the
  user name; the actual got is SEC_E_NO_CREDENTIALS), kuser_test (it names NtBuildNumber;
  the check is NtSystemRoot's case), wow64_test (it names OSBuildNumber; the failing call
  is the raw NtCreateSemaphore), disk_test (it names the geometry check; the open fails
  with error 5), kernel32_file_test (it says leftover state; the check is a test bug).

---------------------------------------------------------------------------------------
# 1. test-bug (fixable now)

## kernel32_file_test (check 40, line 415, got 0)
class: test-bug
asserts: `let mut iosb = [usize::MAX; 2]; c.eq(NtFlushBuffersFileEx(h, 4, ..).into(), 0)?; c.eq(iosb[0] as u64, 0)?;`
reading: Windows writes IO_STATUS_BLOCK.Status as a 32-bit NTSTATUS. That leaves the
upper half of the 8-byte union at the 0xffffffff sentinel, so iosb[0] = 0xffffffff00000000.
`Checks::ok` prints `got as u32`, which is why the report shows 0. Not leftover state.
fix or probe: `c.eq(iosb[0] as u32 as u64, 0)` (or start iosb at 0). Grep the other
tests for `[usize::MAX; 2]` IOSBs compared as u64. gala stays as it is, since it writes 8 bytes
(arguably it should write only 4 bytes to match Windows; harmless).

## kernel32_clr_test (check 10, line 292, got 5)
class: test-bug (hard-coded CPU count)
asserts: `GetCurrentProcessorNumberEx(&mut n); c.ok(n.group == 0 && n.number < 2, ..)`
reading: the thread ran on processor 5 of a machine with more than 2 CPUs.
fix or probe: `n.number < GetActiveProcessorCount(0)` (or `< GetSystemInfo().dwNumberOfProcessors`).

## kernel32_pool_test (check 53, line 511, got 5)
class: test-bug (admin assumption)
asserts: `CreateFileW("C:\\pool-a.txt", GENERIC_READ|GENERIC_WRITE, 0, .., CREATE_ALWAYS ..) != INVALID_HANDLE_VALUE`
reading: ERROR_ACCESS_DENIED. Standard users cannot create files in C:\ (they may create folders).
fix or probe: build the three paths from GetTempPathW (`%TEMP%\gala_pool_a.txt` ...),
deleting them first.

## kernel32_system_test (check 52, line 448, got 0xb7)
class: test-bug (leftover state / root path)
asserts: `CreateDirectoryA("C:\\gala-system-test", NULL) != 0`
reading: ERROR_ALREADY_EXISTS (183). A user may create folders in C:\, so an earlier
run made it and did not remove it (this also explains the 0x58 -> 0x34 movement between sweeps).
fix or probe: use `%TEMP%\gala-system-test`, run RemoveDirectory (and delete its contents)
before starting, and clean up at the end.

## security_test (check 4, line 367, got 0xb7)
class: test-bug (leftover state)
asserts: `CreateDirectoryW("C:\\security_test", &sa /* D:P(A;OICI;FA;;;WD)(A;;FR;;;SY) */) != 0`
reading: ERROR_ALREADY_EXISTS. Left by an earlier run (the sweeps moved 0x21 -> 0x4).
fix or probe: use %TEMP% and remove the directory first (Everyone:FA in the DACL lets the
user delete it); remove it at the end too.

## cmd_test (check 4, line 298, got 1)
class: test-bug (gala path)
asserts: `run("cmd.exe /c \"<me>\" print > C:\\gala\\cmd_test_out.txt") == 0`
reading: cmd exits 1 because C:\gala does not exist ("The system cannot find the path specified").
fix or probe: build `out_file`/`in_file` and the redirection text from GetTempPathW
(lines 295-297 and the later `<`, `>>` and `2>&1` lines).

## kernel32_test (check 38, line 201, got 0)
class: test-bug (gala layout)
asserts: `GetDllDirectoryW(..)==0; c.ok(LoadLibraryA("testdll2.dll").is_null(), 0)` (only in C:\gala\lib)
reading: run-all.ps1 runs from user/bin, where testdll2.dll sits next to the exe, so the
application directory finds it.
fix or probe: derive the lib directory from the exe's own directory (`<exe dir>\lib`)
instead of the literal C:\gala\lib, and export/copy testdll2.dll into user/bin/lib for the
Windows run (xtask already puts it in gala/lib, where the exe dir is C:\gala). loader_test
also uses testdll2.dll from gala and gala/sub, so keep its copy.

## kuser_test (check 18, line 96, got 0x49)
class: test-bug (case of NtSystemRoot)
asserts: `for (i, ch) in "C:\\Windows" { c.eq(u16_at(0x30 + 2*i), ch) } // NtSystemRoot`
reading: got 'I' (0x49) where it expected 'i', at index 4. This machine's KUSER_SHARED_DATA
NtSystemRoot is "C:\WINDOWS" (upper case, as setup and upgrades often leave it). The
triage's "NtBuildNumber" note is wrong.
fix or probe: compare case-insensitively (as wow64_k32_test already does with `| 0x20`).
gala needs no change.

## kernel32_locale_test (check 85, line 551, got 0x61)
class: test-bug (machine-dependent value), cheap probe to confirm
asserts: `GetSystemPreferredUILanguages(MUI_LANGUAGE_NAME, ..) != 0; c.text(&buf, "en-US")`
reading: the user's preferred UI language is en-US (lines 538-540 passed), but the system's
first one starts with 'a' (0x61). The install/system UI language is not en-US. Which one is
unknown (unsure).
fix or probe: compare with `LCIDToLocaleName(GetSystemDefaultUILanguage())` instead of a
literal. Probe: print the whole multi-sz and `Get-WinSystemLocale`/`GetSystemDefaultUILanguage()`.

## kernel32_py_test (check 23, line 276, got 0)
class: test-bug (inherited environment), likely; confirm with a one-line probe
asserts: `c.ok(NeedCurrentDirectoryForExePathW("python.exe") != 0, 0)` (before the test sets
NoDefaultCurrentDirectoryInExePath itself)
reading: the docs say FALSE only when NoDefaultCurrentDirectoryInExePath exists in the
environment (whatever its value). So the variable is almost certainly inherited from
run-all.ps1's or the agent's environment.
fix or probe: clear it first, `SetEnvironmentVariableW("NoDefaultCurrentDirectoryInExePath", NULL)`.
Probe: `GetEnvironmentVariableW` of it in the run's environment. If it is absent and the
call still gives FALSE, this becomes windows-behaviour.

## seh_test (check 15, line 577, got 0)
class: test-bug (likely; unsure)
asserts: `RtlAddFunctionTable(&INDIRECT[0x40], 2, INDIRECT.as_ptr()); RtlLookupFunctionEntry(region + 0x18) == region + 0x10c`
reading: NULL. INDIRECT is a `static` inside this exe's image, so the "dynamic" code
region lies inside a loaded image. The lookup evidently searches that image's own
.pdata (no entry) and never reaches the dynamic table, which the docs describe as being
for code outside images (JIT). gala's lookup checks dynamic tables first, or for any
address.
fix or probe: put the fake region in VirtualAlloc'd memory (copy INDIRECT there, then
register). If it still fails on Windows, probe the indirect (UnwindData|1) entry
separately, with a direct entry as the control. gala: make RtlLookupFunctionEntry prefer
the image covering the PC (user/testntdll).

## threadinfo_test (check 14, line 128, got 0x80000002)
class: test-bug (alignment)
asserts: `set_thread(CURRENT_THREAD, THREAD_HIDE_FROM_DEBUGGER, &[0u8; 4]) == STATUS_INFO_LENGTH_MISMATCH`
reading: STATUS_DATATYPE_MISALIGNMENT. A `[u8; 4]` on the stack is not necessarily
4-aligned, and NtSetInformationThread checks alignment before length. This explains
the 23 <-> 14 flip between builds (it depends on stack layout).
fix or probe: pass `&0u32.to_le_bytes()` from an aligned `u32`/`[u32;1]` local. gala could
also check alignment first (ULONG alignment for this class) to match; a test with a
deliberately odd address would settle the required alignment.

## object_test (check 17, line 136, got 0xc0000022)
class: test-bug (SYSTEM/admin access assumed)
asserts: `NtOpenDirectoryObject(DIRECTORY_ALL_ACCESS, "\\BaseNamedObjects") == STATUS_SUCCESS`
reading: STATUS_ACCESS_DENIED. A standard user is not granted DIRECTORY_ALL_ACCESS on the
global BNO. Creating events there earlier in the test worked.
fix or probe: ask for `DIRECTORY_TRAVERSE | DIRECTORY_CREATE_OBJECT` (what the relative
creates need). It is unsure whether CREATE_OBJECT is granted globally; if it is not, use
`\Sessions\<PEB.SessionId>\BaseNamedObjects`. gala divergence: it evidently grants ALL_ACCESS
to a non-admin. Check the BNO directory's DACL in kernel/src/nt/namespace.rs.

## section_test (check 39, line 130, got 0xc0000022)
class: test-bug (session 0 / SeCreateGlobalPrivilege assumed)
asserts: `NtCreateSection(\\BaseNamedObjects\\gala_section_test, 0x1000, PAGE_READWRITE) == ok`
reading: documented: creating a file mapping in the global namespace from a session other
than 0 needs SeCreateGlobalPrivilege, which a standard user does not have.
fix or probe: name it under `\Sessions\<PEB.SessionId>\BaseNamedObjects\...` (session 0:
`\BaseNamedObjects`). Better still, add a check that the global create is
STATUS_ACCESS_DENIED for a non-admin in session 1 on gala too. gala: check whether
NtCreateSection enforces SeCreateGlobalPrivilege (kernel/src/nt/section.rs and namespace.rs);
it evidently does not.

## symlink_test (check 8, line 123, got 0xc0000022)
class: test-bug (same family as section_test); unsure of the exact rule
asserts: `NtCreateSymbolicLinkObject(\\BaseNamedObjects\\gala_symlink_test_link -> \\BaseNamedObjects, ALL_ACCESS) == ok`
reading: ACCESS_DENIED for a standard user in session 1 (the event at line 115 in the same
directory was allowed).
fix or probe: move the links to the session's BNO. Probe on Windows: the same create under
`\Sessions\1\BaseNamedObjects`, and the global one with SYMBOLIC_LINK_QUERY only, to see
whether the rule is the directory's DACL or SeCreateGlobalPrivilege.

## wow64_test (check 71, line 394 = 0x18a, got 00000000 c0000005)
class: test-bug (raw unverified syscall number)
asserts: `sys(NT_CREATE_SEMAPHORE /* 0xcf, gala's own */, &[&mut sem, 0x1f0003, 0, 2, 5]) == 0`
reading: 0xcf is some other service on Windows; it read the 2 or the 5 as a pointer and
gave STATUS_ACCESS_VIOLATION. Triage's "OSBuildNumber" note is wrong.
fix or probe: call 32-bit ntdll's `NtCreateSemaphore` export (find_export in the WOW64
ntdll). The same applies to the other unverified numbers in user/ntrt/src/x86.rs that
wow64_test uses (NT_CREATE_MUTANT 0xc3, NT_QUERY_SEMAPHORE/MUTANT, NT_CREATE_TIMER ...).

## smpsem_test (check 2, no line, got 0xc000000d)
class: test-bug (raw unverified syscall number)
asserts: `smpsem_create_semaphore(..)` = `mov eax, 0xcf; syscall` (global_asm, line ~43); `status(2, s)`
reading: on Windows, 0xcf is not NtCreateSemaphore, hence INVALID_PARAMETER.
fix or probe: resolve NtCreateSemaphore (and its release/query stubs, if they are also
raw) from ntdll.

## safepoint_test (exit 3, no line)
class: test-bug (raw unverified syscall number)
asserts: `ok(3, sp_create_semaphore(&mut h, ..) == STATUS_SUCCESS)` (main.rs ~677, a raw `mov eax, 0xcf` stub)
reading: as smpsem_test. xstate_test has the same kind of stub (it passed, but check it).
fix or probe: use ntdll's export.

## smprace_test (check 20, no line, got 0xc000000d)
class: test-bug (ntrt's raw timer numbers)
asserts: round2: `status(20, pin(0)); create_timer(..); set_timer(..)` (lines 215-226,
all check 20)
reading: ntrt::create_timer is `syscall 0xd9`, gala's unverified number. Check 20 is shared
by four calls, but pin(0) is valid, so the timer call is the one failing.
fix or probe: make ntrt's create_timer/set_timer resolve NtCreateTimer/NtSetTimer from ntdll
(user/ntrt/src/x64.rs:75-78, 478-492). That fixes clock_test's and handover_test's timer
calls too, though both stay gala-only. Give the four check-20 calls distinct numbers.

## setupapi_test (check 2, line 163, got 1)
class: test-bug (assumes no hardware)
asserts: `SetupDiEnumDeviceInterfaces(set, NULL, &GUID_DEVICE_BATTERY, 0, ..) == 0 && GetLastError()==ERROR_NO_MORE_ITEMS`
reading: this machine has a battery interface (a laptop), so index 0 exists.
fix or probe: enumerate until ERROR_NO_MORE_ITEMS and check the end condition at index n.
Or use a class GUID no machine has for the "empty" case and keep the battery GUID for
the "enumerates" case. Check that later checks do not assume zero devices.

## persist_test (check 14, line 489, got 0xb359ad23)
class: test-bug (state across runs; a gala reboot scenario)
asserts: second boot: `GetFileTime(C:\persist\a.txt, .., &accessed, ..); c.eq(accessed, ACCESSED)`
reading: C:\persist exists from an earlier sweep, so every later run takes the
"second boot" path. Each such run reads a.txt, and NTFS last-access updates are on, so the
access time no longer equals what the first boot set. Runs 2 and up cannot pass.
fix or probe: in run-all.ps1, delete C:\persist and HKCU\Software\GalaPersist, run the
program twice, and judge only the second run. Or have the test delete C:\persist at the end
of a successful second boot. Not a divergence.

## oleaut32_test (crash 0xc0000005: OLEAUT32+0x9c5f writing address 0)
class: test-bug (likely; confirm)
asserts: probably line 288: `CHECK(VarBstrFromCy(cy0, EN_US, 0, NULL) == E_INVALIDARG);`.
It is the only call in the test that passes a NULL out-pointer to a function that writes
a result.
reading: the real VarBstrFromCy stores through pbstrOut without a NULL check, hence the
write to 0.
fix or probe: probe: a `printf` + `fflush` before line 288 (or run with that line removed).
If that is it, drop the NULL-out case, since the docs do not promise E_INVALIDARG.

---------------------------------------------------------------------------------------
# 2. windows-behaviour, enough evidence to change gala and the test now

## advapi32_test (check 59, line 1391, got 0x60)
class: windows-behaviour
asserts: `GetTokenInformation(token, TokenIntegrityLevel, ..); c.eq(label[1] as u32, 0x20) // SE_GROUP_INTEGRITY`
reading: Windows reports the label's Attributes as SE_GROUP_INTEGRITY |
SE_GROUP_INTEGRITY_ENABLED (0x60). gala keeps 0x60 in the token's groups but strips it to
0x20 in this query.
fix or probe: test: expect 0x60. gala: kernel/src/nt/token.rs:952-955
(`attributes: SE_GROUP_INTEGRITY` -> `SE_GROUP_INTEGRITY | SE_GROUP_INTEGRITY_ENABLED`).

## cert_test (check 8, line 509, got 1)
class: windows-behaviour
asserts: `CertCreateCertificateContext(X509_ASN_ENCODING|PKCS_7_ASN_ENCODING, ..); c.eq(ctx.encoding, 0x10001)`
reading: CERT_CONTEXT.dwCertEncodingType is 1. Windows keeps only the certificate
encoding part (low word, X509_ASN_ENCODING) and drops PKCS_7.
fix or probe: test: expect `X509_ASN_ENCODING`. gala: user/crypt32/src/cert.rs
CertCreateCertificateContext (line 428 on) stores the caller's value. Store `encoding & 0xffff`.
Check the CRL/CTL contexts the same way if they exist.

## kernel32_misc_test (check 30, line 535, got 0x80)
class: windows-behaviour
asserts: `RaiseException(CODE_RAISED, 0, 17, args); c.eq(raised(0) /* ExceptionFlags */, 0)`
reading: the flags are 0x80, EXCEPTION_SOFTWARE_ORIGINATE in the current SDK's winnt.h:
Windows marks software-raised exceptions.
fix or probe: test: expect 0x80. gala: user/kernel32/src/mapping.rs:680
(`flags: flags & EXCEPTION_NONCONTINUABLE` -> `| 0x80`). Unsure whether RtlRaiseException itself
sets it or only kernel32's RaiseException: a probe calling RtlRaiseException directly
with flags 0 would say whether testntdll should set it instead.

## kernel32_thread_test (check 50, line 348, got 0x10000000)
class: windows-behaviour (rtl_test check 101, line 570 is the same thing; kernel32_clr_test:599 will hit it next)
asserts: `c.eq(SetThreadDescription(t, name) as u64, 0)`
reading: SetThreadDescription returns 0x10000000 = HRESULT_FROM_NT(STATUS_SUCCESS)
(FACILITY_NT_BIT | 0). That is a success HRESULT, but not S_OK.
fix or probe: tests: expect 0x1000_0000 (or `>= 0`) in kernel32_thread_test:348,
rtl_test:570 and kernel32_clr_test:599. gala: user/kernel32/src/runtime.rs:594
returns 0 on success; return `nt_hresult(STATUS_SUCCESS)`, i.e. 0x10000000. Failures already
use nt_hresult.

## rtl_test (check 101, line 570, got 0x10000000)
class: windows-behaviour (see kernel32_thread_test)
asserts: `describe(GetCurrentThread(), "sig") == 0 // S_OK` (kernelbase!SetThreadDescription)
reading: as above.
fix or probe: as above.

## loader_test (check 17, line 309, got 0xc0000139)
class: windows-behaviour
asserts: `LdrGetProcedureAddress(dll, "no_such_export", 0, &addr) == STATUS_PROCEDURE_NOT_FOUND (0xc000007a)`
reading: by name, a missing export is STATUS_ENTRYPOINT_NOT_FOUND (0xc0000139). Both map
to ERROR_PROC_NOT_FOUND, so GetProcAddress callers see no difference.
fix or probe: test: expect 0xc0000139. gala: user/testntdll/src/lib.rs:1799-1805, the
by-name branch -> STATUS_ENTRYPOINT_NOT_FOUND. Keep STATUS_ORDINAL_NOT_FOUND for
ordinals unless probed otherwise. Check that the 0xc0000139 -> 127 mapping exists in
testntdll's table (line ~1955).

## image_test (check 26, line 184, got 0x4000000e)
class: windows-behaviour
asserts: mapping a PE32 IL-only DLL's image section in a 64-bit process: `s == STATUS_SUCCESS || s == STATUS_IMAGE_NOT_AT_BASE`
reading: 0x4000000E is STATUS_IMAGE_MACHINE_TYPE_MISMATCH, an informational status: the view
is mapped but the image is for another machine.
fix or probe: test: accept 0x4000000e for the PE32 case (and assert it is that exact
status). gala: kernel/src/nt/syscalls.rs:4490 (the map-view result). Return
IMAGE_MACHINE_TYPE_MISMATCH when the image's Machine is not the process's (it outranks
NOT_AT_BASE? unsure; probe a relocated PE32 view if it matters).

## pipe_test (check 331, line 656, got 0xfffffffd)
class: windows-behaviour
asserts: byte-mode pipe, 5 bytes written, `PeekNamedPipe(r, buf, 3, &got, &avail, &left); c.eq(left, 0)`
reading: lpBytesLeftThisMessage = -3. kernel32 evidently computes MessageLength - BytesRead
without looking at the mode, and a byte pipe's message length is 0. The docs say zero,
but this is what Windows does.
fix or probe: test: expect `0u32.wrapping_sub(3)`. gala: user/kernel32/src/pipe.rs:360
(PeekNamedPipe `left`). To confirm the formula before changing gala, run a one-line probe:
peek 1 byte (expect 0xffffffff) and peek with a NULL buffer/size 0 (expect 0).

## thread_test (check 14, line 90, got 0)
class: windows-behaviour
asserts: after the thread exited: `ThreadBasicInformation.TebBaseAddress == teb` (the TEB the thread saw)
reading: for a terminated thread, Windows reports TebBaseAddress = NULL (the TEB is freed
on exit).
fix or probe: test: expect 0 after exit. If the TEB check matters, query a suspended or
waiting thread before it exits. gala: kernel/src/nt/syscalls.rs ~3375
(THREAD_BASIC_INFORMATION): report 0 once the thread has terminated, and free/forget the TEB
then.

## token_test (check 111, line 519, got 1)
class: windows-behaviour
asserts: `PrivilegeCheck(primary token opened TOKEN_QUERY, {SeChangeNotify}) fails with ERROR_NO_IMPERSONATION_TOKEN`
reading: it succeeded (`fails` reports 1 when the call returned TRUE). Windows accepts a
primary token here, although the docs speak of a client's impersonation token.
fix or probe: test: `c.ok(r != 0)` and the result TRUE (SeChangeNotify is enabled). gala:
user/advapi32/src/token.rs:448 and kernel/src/nt/token_calls.rs:325 (NtPrivilegeCheck
refusing a primary token). Probe that would be nice: the pfResult value and the privilege's
attributes (SE_PRIVILEGE_USED_FOR_ACCESS) on Windows.

## win32_misc_test (check 70, line 1201, got 0x57)
class: windows-behaviour
asserts: `GetGestureInfo(0x1234, NULL) == 0; GetLastError() == ERROR_INVALID_HANDLE (6)`
reading: ERROR_INVALID_PARAMETER (87). The NULL output is refused before the handle is
looked at.
fix or probe: test: either expect 87 for the NULL case, or pass a GESTUREINFO with
`cbSize = sizeof` to test the bad handle (expected 6, unverified). gala:
user/user32/src/window.rs:3472 GetGestureInfo: NULL/short info -> 87 first.

## vmcycle_test (check 39, no line, got 0xc000009f)
class: windows-behaviour
asserts: `NtFreeVirtualMemory(base + 2 MiB, size 0, MEM_DECOMMIT) == STATUS_SUCCESS` ("decommit to the end of the reservation")
reading: STATUS_FREE_VM_NOT_AT_BASE. Size 0 means "the whole region" only at the
allocation base, and is refused elsewhere (VirtualFree's docs describe size 0 only with the
base).
fix or probe: test: assert 0xc000009f there, then decommit with an explicit size (6 MiB)
and continue. gala: kernel/src/nt/vm.rs (NtFreeVirtualMemory, MEM_DECOMMIT with size 0 at
an address that is not the allocation base -> FREE_VM_NOT_AT_BASE).

## sspi_test (check 48, line 582, got 0x8009030e)
class: windows-behaviour (the triage's reason, the user name, is wrong)
asserts: `InitializeSecurityContextW(kerberos creds, "MSSQLSvc/gala", ..) == SEC_E_NO_AUTHENTICATING_AUTHORITY (0x80090311)`
reading: SEC_E_NO_CREDENTIALS (0x8009030E). On a workgroup (non-domain) machine, the
Kerberos package's ISC fails for want of credentials, not of an authority.
fix or probe: test: expect 0x8009030e for a non-domain machine. gala:
user/secur32/src/sspi.rs, where 0x80090311 is returned. Note: on a domain-joined machine
with no KDC, the answer may well differ (unsure); gala models a workgroup machine.

## wow64_import_test (check 11, line 275 = 0x113, got 0x74ca0000)
class: windows-behaviour
asserts: `read_u32(pe + 24 + 28) == 0x40_0000 // mapped header's ImageBase: its preferred base` (testdll32 relocated)
reading: in the mapped image, OptionalHeader.ImageBase holds the actual load address
(0x74ca0000, the DLL's base; line 271 checked base != image). Windows rewrites ImageBase
in the mapped header when it relocates.
fix or probe: test: `c.eq(read_u32(pe+24+28), base as u32)`. gala: the section mapper's
relocation (kernel/src/nt/section.rs `relocated_frame`, ~237) must also patch the header
page's ImageBase, for PE32 and PE32+ alike. This probably matters for 64-bit images too:
add the same check to image_test/loader_test.

## lsa_test (check 6, line 510, got 0xc0000041)
class: environment for the test (it needs SYSTEM, SeTcbPrivilege and the galatest account),
but the result is also windows-behaviour evidence for gala
asserts: `LsaRegisterLogonProcess(&"lsa_test", ..) == 0`
reading: without SeTcbPrivilege Windows gives STATUS_PORT_CONNECTION_REFUSED (0xc0000041);
gala gives STATUS_PRIVILEGE_NOT_HELD.
fix or probe: gala: user/secur32/src/lsa.rs:131-132 -> 0xc0000041. Test: when the token lacks
SeTcbPrivilege, assert 0xc0000041 and stop with "skipped: needs SYSTEM". Run the rest under
`psexec -s` with galatest present.

---------------------------------------------------------------------------------------
# 3. windows-behaviour, a probe is needed first

## ioapc_test (check 22, line 130, got 0)
class: windows-behaviour (needs a probe)
asserts: `NtReadFile(synchronous handle, ApcRoutine=done, ..) == STATUS_INVALID_PARAMETER`
reading: Windows accepted it (STATUS_SUCCESS).
fix or probe: after that call, `delay_alertable(10 ms)` and record `calls()` and the IOSB:
does the APC get queued and run (calls 4 -> 5)? Then test and gala
(NtReadFile in kernel/src/nt/file.rs/syscalls.rs) follow: accept, and queue or ignore the APC.

## registry_test (check 100, line 813, got 0x24)
class: windows-behaviour (needs a probe)
asserts: `RegGetValueW(test, NULL, "exp", RRF_RT_REG_SZ, ..)` expands "%GALA_REG_TEST%\x"
-> "C:\gala\x"; `c.eq(size, 20)`
reading: pcbData came back 36, the raw REG_EXPAND_SZ size (line 825 sees 36 with
RRF_NOEXPAND), not the 20 bytes of the expanded string. It could be "always the stored
size" or "max(stored, expanded)" (unsure).
fix or probe: set GALA_REG_TEST to a 60-character value and record the size, the data, and
what a NULL-buffer size query returns. gala: user/advapi32/src/registry.rs:933 RegGetValueW.

## fpu_test (check 21, line 250, got 0x1f80)
class: windows-behaviour (needs a probe)
asserts: `NtCreateThread` with CONTEXT_FULL and `MxCsr = 0x5f80` (round toward +inf); the thread's stmxcsr == 0x5f80
reading: the new thread runs with the default 0x1f80. Either Windows ignores the CONTEXT's
FP state for a new thread, or it takes MXCSR from FltSave.MxCsr (offset 0x118), which this
test leaves at 0. NtContinue in check (2) does honour MxCsr.
fix or probe: three runs: MxCsr=0x5f80 with FltSave.MxCsr=0x5f80; MxCsr=0x1f80 with
FltSave.MxCsr=0x5f80; and CONTEXT_FLOATING_POINT cleared. Record stmxcsr in each. gala:
kernel/src/nt/thread.rs `start_mxcsr`.

## tls_test (check 6, line 298, got 0x6f5e2ed3)
class: windows-behaviour (strong hint, probe to settle)
asserts: the exe's TLS directory has SizeOfZeroFill = 16; slot 3 (the first zero-fill
qword, at End-Start) == 0
reading: garbage. Windows' loader evidently does not provide or zero SizeOfZeroFill
(it copies only the raw data, and the bytes after are uninitialised heap). Public LLVM
notes say the same. gala allocates and zeroes the zero-fill.
fix or probe: probe `HeapSize(GetProcessHeap(), 0, block)` for the TLS block, against
`End - Start`. Test: drop the zero-fill checks (slots 3-4 here and in the DLL), or move
those values into raw data. gala may keep zeroing (harmless), but should not be asserted.

## uipi_test (check 38, line 345, last error 87)
class: windows-behaviour (needs a probe)
asserts: `ChangeWindowMessageFilterEx(top, WM_APP + 1, MSGFLT_RESET, &cfs)` succeeds
reading: ERROR_INVALID_PARAMETER for MSGFLT_RESET with a non-zero message. It probably
requires message 0 for RESET (it resets the whole window), but that is unsure.
fix or probe: probe (top, 0, MSGFLT_RESET, &cfs), and with cfs NULL. Record the result,
the error and ExtStatus. gala: user/user32/src/foreign.rs:1131.

## winsta_test (check 58, line 470, got 0xe8)
class: windows-behaviour (needs a probe)
asserts: `OpenWindowStationW(L"", 0, WINSTA_ALL_ACCESS)` fails with ERROR_FILE_NOT_FOUND
reading: it succeeded and returned handle 0xe8.
fix or probe: GetUserObjectInformationW(h, UOI_NAME/UOI_TYPE) on it: is it the process's
window station (WinSta0), or the WindowStations directory? Also check "foo\bar" (line 471).
gala: user/user32/src/winsta.rs:137.

## process_test (check 74, line 683, got 0)
class: windows-behaviour (needs a probe)
asserts: a new job's JOBOBJECT_BASIC_LIMIT_INFORMATION (64 bytes) is all zero
reading: some byte is not zero. Most likely SchedulingClass (offset 60) = 5: the docs give
5 as the default scheduling class (unsure).
fix or probe: hex-dump the 64 bytes. gala: kernel/src/nt/job_calls.rs (basic-limit query).
Test: expect the dump field by field.

## gui_test (check 23, line 815, got 0)
class: windows-behaviour (needs a probe); the test's arguments are also questionable
asserts: `SetGestureConfig(hwnd, 0, 0, NULL, 0) != 0`
reading: Windows refuses zero configs, a NULL array and cbSize 0 (consistent with
win32_misc_test, where the NULL arguments are checked first).
fix or probe: record GetLastError (87 expected). Better: call it as the VCL does, with one
GESTURECONFIG {dwID=0, dwWant=GC_ALLGESTURES, dwBlock=0}, cIDs=1 and cbSize=12, and assert
TRUE. gala: user/user32/src/window.rs:3455.

## com_test (check 110, line 736, got 0)
class: probe needed (and it is unstable: 0x6c -> 0x6e between sweeps)
asserts: `SafeArrayAllocDescriptorEx(VT_I4, 1, &sa); sa.count=3; sa.lower=1; SafeArrayAllocData(sa)==S_OK; data != NULL && cbElements == 4`
reading: either pvData is NULL or cbElements != 4 after AllocDescriptorEx/AllocData.
fix or probe: after each call, record cbElements, fFeatures, the VARTYPE at -4 and pvData.
gala: user/oleaut32/src/safearray.rs:103-130.

## comctl32_test (check 13, line 581)
class: probe needed
asserts: `FlatSB_GetScrollProp(w, WSB_PROP_WINSTYLE, &v) && v == WS_VSCROLL`
reading: either it returns FALSE or v is something else (both WS_*SCROLL bits? 0?).
Which comctl32 is loaded also matters (v5 vs v6: is there a manifest?).
fix or probe: record the return value, v, and GetModuleFileName(comctl32).

## controls_test (check 46, line 221)
class: probe needed
asserts: `CB_SETCURSEL(7) == CB_ERR && CB_GETCURSEL == CB_ERR` (three items; the docs say the selection is cleared)
fix or probe: record both return values separately. If the selection is kept (1), change
the test and gala's combobox.

## dbghelp_test (check 7, line 137)
class: probe needed
asserts: `SymSrvGetFileIndexInfoW(<own exe>, &info /* sizeofstruct set */, 0)` returns TRUE
reading: FALSE. Possibly because the exe has no debug directory (mingw, -s), possibly
because of the system dbghelp's rules.
fix or probe: GetLastError, plus the same call on C:\Windows\System32\kernel32.dll as the
control.

## gdi32_test (check 41, line 193)
class: probe needed
asserts: after `SetBoundsRect(dmem, NULL, DCB_ENABLE|DCB_RESET)`, `GetBoundsRect(dmem, &b, 0) == DCB_RESET|DCB_ENABLE`
fix or probe: record the return value (the guess is DCB_RESET alone, which would mean
GetBoundsRect does not report the enable state; unsure). Record the later two calls as well.

## ole32_test (check 31, line 447)
class: probe needed
asserts: `CoCreateInstanceEx(CLSID_CUIAutomation, inproc, 3 MULTI_QIs incl. an unknown IID) == CO_S_NOTALLINTERFACES`
fix or probe: record the HRESULT and each mqi[i].hr/pItf.

## quota_test (check 10, line 620, got 16711680 = 0xff0000)
class: probe needed
asserts: `at_limit /* GetProcessHandleCount */ == base + 2*2 + 1 + n`
reading: the handle table is full at 0xff0000. The count does not match base + 5 + n, so
something else in the process holds handles that the formula does not account for
(threads/handles the system opened during the run? unsure).
fix or probe: print base, n, at_limit, and the handle count right after the two start_child
calls. Then fix the formula, or assert `>=`, if the extra handles are Windows' own.

## shell32_test (check 46, line 492)
class: probe needed
asserts: `SHGetIconOverlayIndexW(L"C:\\nothing.ico", 0) == -1`
fix or probe: record the return value. Note that the run took 2001 ms, a timeout somewhere?
Also time the SHChangeNotify on the line before.

## shlwapi_test (check 228, url.c line 229)
class: probe needed (likely windows-behaviour)
asserts: url.c:229 `AssocQueryStringW(0, ASSOCSTR_EXECUTABLE, L".galatest", L"open", out, &len) == S_OK`
(the HKCU\Software\Classes command is `"C:\Gala Tools\viewer.exe" "%1"`)
reading: it fails on Windows, probably because the executable does not exist
(ASSOCSTR_EXECUTABLE looks for the file; unsure).
fix or probe: record the HRESULT and the out text. Then point the command at an existing
exe (C:\Windows\System32\notepad.exe) as the control. gala: user/shlwapi AssocQueryStringW.

## sysinfo_test (check 19, line 108, got 0xc00000bb)
class: probe needed
asserts: `NtQuerySystemInformation(124 /* SystemQueryPerformanceCounterInformation */, 12 bytes) == ok`
reading: STATUS_NOT_SUPPORTED. It may need its input (Version = 1) filled in, or it may be
unsupported on this machine's timer source.
fix or probe: the same call with u32 Version=1 pre-filled, and also 16/24-byte buffers.
Record the status and ret.

## udp_test (check 84, line 509, got 0xffffffff)
class: probe needed
asserts: `setsockopt(b /* bound UDP socket */, SOL_SOCKET, SO_RANDOMIZE_PORT, &1, 4) == 0`
reading: SOCKET_ERROR. It may have to be set before bind (unsure).
fix or probe: WSAGetLastError, plus the same call on an unbound socket.

## user32_test (check 42, line 289)
class: probe needed
asserts: one WH_CBT hook, one CreateWindowExW: `cbt_creates == 1 && cbt_created == w && title_seen == 1`
reading: the guess is that Windows also creates the thread's "Default IME" (and possibly
MSCTFIME UI) window, which the hook sees, so cbt_creates is 2 and cbt_created is not w.
fix or probe: log each HCBT_CREATEWND (hwnd, class via GetClassNameW, title) and
title_seen. If it is the IME window, count only windows of class User32TestHooks.

## winsock_async_test (check 26, line 483)
class: probe needed
asserts: after AcceptEx(.., 0, ADDR_BLOCK, ADDR_BLOCK) completes, `GetAcceptExSockaddrs(buf, 0, ADDR_BLOCK, ADDR_BLOCK, ..)` gives non-NULL pointers
fix or probe: hex-dump accept_buf, and record local_len/remote_len. Also call the function
through WSAID_GETACCEPTEXSOCKADDRS (WSAIoctl), the documented route, as the control.

## uia_test (check 76, line 1665)
class: probe needed
asserts: the first child of the window (FindAll children, n==4) is a TitleBar named "UIA Test" with no NativeWindowHandle
fix or probe: record each child's ControlType, Name and NativeWindowHandle.

## wow64_thread_test (check 1, line 260 = 0x104, got 00000000 c0000022)
class: probe needed
asserts: NtCreateThread (x86 0x4e, a long-stable number) from 32-bit code with an x86
CONTEXT and INITIAL_TEB succeeds
reading: STATUS_ACCESS_DENIED from WOW64's NtCreateThread.
fix or probe: the same call through 32-bit ntdll's NtCreateThread export, then
NtCreateThreadEx and CreateThread, to see whether the legacy NtCreateThread is refused for
WOW64 callers or whether the raw syscall path is the problem.

## conpty_test (check 4, 20 s)
class: probe needed
asserts: the child started on the pseudo console prints "child: ready" within 20 s
fix or probe: capture the program's full stdout. It prints the escaped output pipe contents
on failure, but run-all.ps1 kept only the last line. Look at what conhost sent: a DSR
request (ESC[6n) the "terminal" must answer, or the text broken up by VT sequences.

## browser_api_test (check 23)
class: probe needed (undiagnosable as written)
asserts: one of seven `check(.., 23)` at lines 614-635: BuildExplicitAccessWithNameW
layout, BuildSecurityDescriptorW, the DACL/SACL queries
fix or probe: give each its own number (23a..g) and re-run.

## wow64_k32_test (exit 28, the line kept is "wow64_k32_test: hello")
class: probe needed (unsure which check)
asserts: check 28. Triage counted it to line 207 (GetSystemDirectoryW == C:\Windows\SysWOW64,
case-insensitive), but that check compares case-insensitively and should pass even on
"C:\WINDOWS".
fix or probe: capture the full stdout. The failure line begins "(line 0x...)" and was not
kept.

## clusapi_test (crash 0xc0000005: resutils+0x2a724, reading 0x753399fb14)
class: probe needed (likely the test's CLUSPROP list)
reading: real resutils walks off the property list the test built. The usual causes are a
missing CLUSPROP_SYNTAX_ENDMARK, value lengths not rounded to 4, or a wrong cbProplist.
fix or probe: print a line before each ResUtil* call to find it; then check the list
against the documented CLUSPROP layout.

---------------------------------------------------------------------------------------
# 4. environment

## disk_test (check 1, line 355, got 5)
class: environment (needs an administrator and a scratch disk)
asserts: `CreateFileW(\\.\PhysicalDrive0, GENERIC_READ|GENERIC_WRITE, ..) != INVALID_HANDLE_VALUE`
reading: ERROR_ACCESS_DENIED for a standard user. The triage's geometry-check mapping is
wrong.
fix or probe: skip with a reason when access is denied and the token is not elevated.
Important: it must never run elevated on a real machine against PhysicalDrive0. It writes
sector 2, and only ioctls()'s "16 MiB length" check happens to stop it first. Make that
safety explicit: refuse unless the length is 16 MiB and sector 1 starts with the xtask
marker, read before any write. Or open a VHD attached for the test.

## session_test (check 1, line 436, got 1)
class: environment
asserts: `check_own_session(0)`: PEB.SessionId == 0 (the test runs as SYSTEM in session 0)
reading: this process is in session 1 (the interactive user), as Windows does.
fix or probe: run under `psexec -s` (with galatest); otherwise skip when the token is not
SYSTEM.

## system_test (check 3, line 287) / restricted_test (check 2, line 367)
class: environment (settled in triage: needs LocalSystem / elevation). Skip when the token
is not SYSTEM / not elevated.

## wow64_adv_test (check 145, line 505 = 0x1f9, got 5)
class: environment (admin assumed)
asserts: `RegCreateKeyExW(HKLM, "SOFTWARE\\gala_wow64_view", KEY_ALL_ACCESS) == 0`
reading: ERROR_ACCESS_DENIED for a standard user on HKLM\SOFTWARE.
fix or probe: skip this block when not elevated. Alternatively, test redirection without
admin through a redirected HKCU key: HKCU\Software\Classes\CLSID\{test guid} <->
HKCU\Software\Classes\WOW6432Node\CLSID\{...}. That is documented as redirected, but unsure
whether it behaves the same on 26200.

## tds_test (step 1 CreateProcessW, got 0x10b)
class: environment (and a gala path)
reading: ERROR_DIRECTORY (267): the current directory C:\sql\Binn does not exist. SQL
Server LocalDB is not installed there.
fix or probe: skip unless sqlservr.exe exists, or locate LocalDB through its registry key
instead of C:\sql.

## uia_chromium_test (check at line 116: CreateProcessW)
class: environment (and gala paths)
reading: no C:\chromium\chrome.exe and no C:\gala\window.html on this machine.
fix or probe: take the browser path and the page from arguments/environment, and skip when
absent.

## hostnet_test (step 8, name resolution)
class: environment: needs the scenario's hosts entry gala-host.test -> 10.0.2.100 and
xtask's page server. Skip on Windows.

## console_window_test (exit 1, "no line")
class: environment: needs xtask to type into the console window. Skip unless driven, or feed
stdin.

## winhttp_test, wininet_test (timeouts)
class: environment: they need nginx on 127.0.0.1:8080/8443/8444 with the test CA trusted
(admin, as schannel_test). Skip.

## schannel_test (skipped) / access_test, logon_test, profile_test (0x52e)
class: environment, settled in triage (admin / the galatest account).

## bench, tcc, tcc_make, tcc_self, lua, sqlite
class: environment, settled in triage (Defender).

---------------------------------------------------------------------------------------
# 5. gala-only

- cmd, conhost, gala (0xc0000139): import Gala* exports from ntdll (triage).
- gala_cli_test (check 1, line 936, got 0xfffe0003): runs `gala help`, which is gala's own
  command.
- pk_install_test ("no install log"): `gala install`.
- unimplemented_test (exit 3): asserts that gala's xolehlp stub hides
  DtcGetTransactionManagerExW from GetProcAddress. On Windows it is a real export, so exit 3 by
  design.
- gpu_stub_test (check 2): gala's no-adapter contract for d3d11; Windows has a device.
- clock_test (0xc000041d), handover_k32_test (0xc000041d), handover_net_test (0xc000041d),
  handover_test (check 6): gala's private services (0x7fxx; see the top note on the
  callback crash).

# 6. not divergences

- crash (0xc0000005, writing 0): crashes on purpose.
- missing_import_test (0xc0000139): the expected result.
- go_exit (3), busybox (3, its `sh -c ... exit 3`), busybox_script (4, `exit 4` at the end
  of script.sh): the exit codes are the scenario's expected ones. run-all.ps1 should compare
  each against xtask's expected exit status instead of treating non-zero as failure.
- statedump_test (timeout): never ends, by design.
