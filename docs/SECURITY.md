# Security model

The design of NT security in gala: tokens, security descriptors, the
access check, accounts and logons, sessions, integrity levels and UIPI.
The implementation detail (layouts, the groups each token gets, every
service's rules) is in ARCHITECTURE: "Access tokens", "Security
descriptors", "Access checks", "Accounts and logon", "Windows across
processes: UIPI", "Plan: sessions and remote shasha".

Clean-room sources: Microsoft's documentation (Access Control, the
security descriptor and ACE formats, AccessCheck's algorithm, the SDDL
format, token information classes, privileges, LogonUser/LsaLogonUser),
"Windows Internals" (chapter on security), Wine and ReactOS behaviour,
and black-box results on Windows. No Windows code.

## Status

The model is built in phases S1-S6, each from a test program with
Windows' results. (These are security phases; ROADMAP's SMP items S1-S4
are unrelated.)

| Phase | Tests | State | Left |
|---|---|---|---|
| **S1** Tokens in the kernel | `token_test` | done | NtAccessCheck, NtImpersonateThread, NtAdjustGroupsToken |
| **S2** Descriptors kept on objects | `security_test` | done | SetSecurityInfo of an unprotected DACL does not re-apply inheritance; SACLs kept, no auditing |
| **S3** Access checks at open, a second account | `access_test` | done for files, keys, named objects | processes and threads (NtOpenProcess/NtOpenThread grant what is asked), pipes and other devices, traversal |
| **S4** Logons and per-user processes | `logon_test`, `system_test`, `lsa_test`, `profile_test`; `sshd_login`, `sshd_password` | sshd's key and password logins work | logon rights enforced for ordinary accounts; NetUserAdd; KERB_S4U_LOGON; untrusted callers' S4U tokens should be identification-level; the account key is readable by programs |
| **S5** Restricted tokens and integrity | `restricted_test` | done | labels on objects that low tokens create |
| **S6** The desktop's windows (UIPI, sessions) | `uipi_test`, `gala_cli_test`, `session_test` | UIPI and per-session desktops done | SeCreateGlobalPrivilege for `Global\` file mappings; process DACLs; uiAccess; hooks across levels; window handle validation from lower levels |

Persistence (the disk items): D1 virtio-blk (`disk_test`;
`\\.\PhysicalDrive0` for Administrators and SYSTEM only), D2 a file
system that keeps descriptors (boot option `persist`: changes to C:,
descriptors included, are a log on the disk replayed at boot;
`persist_test`), D3 the registry kept (its changes, key descriptors
included, are a log in `C:\Windows\System32\config\gala.reglog`,
replayed after the seed; not Windows' hive format).

## Driving programs

- **Win32-OpenSSH (sshd)**: checks that its host keys and
  `authorized_keys` are owned by the user, Administrators or SYSTEM and
  writable by nobody else (GetNamedSecurityInfo, ACEs), so the answers
  must be the real descriptors. A login needs a token for the user
  (LogonUserW with a password, or an S4U logon through LsaLogonUser for
  key authentication, after LsaRegisterLogonProcess), CreateProcessAsUserW
  of the session process under it, impersonation while reading the
  user's files, per-user profile directories, and a virtual account for
  its privilege-separation child.
- **PostgreSQL**: refuses to run with administrative rights and drops
  them with CreateRestrictedToken (pg_ctl's CreateRestrictedProcess);
  checks its data directory's permissions; needs a persistent disk.
- **SQL Server LocalDB kept across reboots**: the instance's files and
  registration on a disk; SqlUserInstance sets ACLs on the instance
  folders.

## Tokens

A `Token` kernel object type, as documented in "Access Tokens": user
SID; groups (SID and attributes: mandatory, enabled, owner, logon id,
deny-only, integrity); privileges (LUID and attributes); owner and
primary group; default DACL; integrity level (the mandatory label
group); logon session (AuthenticationId LUID); type (primary,
impersonation) and impersonation level; token and modified ids;
restricting SIDs (CreateRestrictedToken); source; elevation type.

- A process holds a primary token: inherited from its creator unless
  created with one (CreateProcessAsUserW). Processes started at boot run
  as the boot user `gala`; the boot option `as=system` starts the `run=`
  programs as SYSTEM.
- A thread may hold an impersonation token
  (NtSetInformationThread(ThreadImpersonationToken), or a pipe server
  impersonating its client).
- Tokens carry their lineage (the logon token they derive from).
  Creating a process with a token needs either the caller's own lineage
  or SeAssignPrimaryTokenPrivilege, as CreateProcessAsUser documents.
- System calls, at gala numbers until verified against the pinned
  build: NtOpenProcessToken(Ex), NtOpenThreadToken(Ex),
  NtQueryInformationToken, NtAdjustPrivilegesToken, NtDuplicateToken,
  NtFilterToken, NtPrivilegeCheck (0x7f16-0x7f1e), NtSetInformationToken
  (0x7f23), NtAllocateLocallyUniqueId (0x7f22), and gala's GalaLogonUser
  (0x7f21) in the role of NtCreateToken. advapi32's token functions are
  wrappers over them (`user/advapi32/src/token.rs`).

## Accounts and logon

A local account database in the SAM's role, kept by the kernel in the
registry (`kernel/src/nt/accounts.rs`): users under
`\Registry\Machine\SAM\Gala\Users\<name>` (`Rid`, `NtHash`: MD4 of the
UTF-16LE password, the hash NTLM uses too), the well-known groups and
aliases (Administrators, Users, ...) and service identities (SYSTEM,
LocalService, NetworkService). Virtual-account mappings and account
rights live beside them (`HKLM\SAM\Gala\Mappings`, `HKLM\SAM\Gala\Rights`).
With `persist` the registry log keeps them across reboots.

- The boot user is `gala` (RID 1001, empty password). Scenarios add
  accounts with a `registry` module; there is no NetUserAdd yet.
- LogonUserW and LsaLogonUser (interactive, network, batch, service,
  S4U; MSV1_0/Negotiate) check the password, or for S4U and virtual
  accounts the caller's SeTcbPrivilege, and make a logon session and a
  primary token through the kernel, with the groups Windows gives that
  logon type.
- There is no LSA process: secur32's and advapi32's LSA functions run in
  the caller, over GalaLogonUser and the registry keys above.

## Security descriptors on objects

Every securable object keeps a self-relative descriptor in the kernel:
named objects (events, mutants, semaphores, sections, timers,
directories, symbolic links), files and directories (on the ramfs node,
and on disk with `persist`), registry keys, processes, threads, tokens.

- At creation the descriptor is the caller's (SECURITY_ATTRIBUTES),
  completed by the documented inheritance rules: the parent's
  inheritable ACEs (OI/CI/NP/IO, CREATOR OWNER/GROUP replaced, the
  auto-inheritance flags), else the creator token's default DACL; owner
  and group from the token.
- NtQuerySecurityObject and NtSetSecurityObject (0x7f1f, 0x7f20) read
  and change them (READ_CONTROL, WRITE_DAC, WRITE_OWNER,
  ACCESS_SYSTEM_SECURITY for the SACL). advapi32's
  Get/SetFileSecurity, Get/SetSecurityInfo, Get/SetNamedSecurityInfo,
  Get/SetKernelObjectSecurity and Reg{Get,Set}KeySecurity use them.
- The disk keeps descriptors with their objects: the file system stores
  one per file or directory, the registry log each key's. A format that
  stores files stores their descriptor from the start (shared ones once
  by id, as NTFS's $Secure does).

## The access check

One function, `access_check(descriptor, token, desired, mapping) ->
Result<granted>` (`security::access_check`), from AccessCheck's
documented algorithm:

1. Generic rights mapped first; MAXIMUM_ALLOWED grants all the DACL
   allows.
2. The mandatory label first: a token below the object's integrity level
   may not write (no write up), nor read or execute when the label asks.
3. A NULL DACL grants all, an empty one nothing; ACEs in order,
   inherit-only ones skipped; a deny ACE wins for rights not yet granted;
   deny-only groups match deny ACEs only.
4. Restricted tokens are checked twice (both must grant).
5. The owner gets READ_CONTROL and WRITE_DAC implicitly (unless an
   OWNER RIGHTS ACE says otherwise).
6. Privileges: SeBackup (read) and SeRestore (write) with backup intent,
   SeTakeOwnership (WRITE_OWNER), SeSecurity (ACCESS_SYSTEM_SECURITY).

The effective token is the thread's impersonation token if any (below
impersonation level: STATUS_BAD_IMPERSONATION_LEVEL), else the
process's. The check runs wherever an object is opened by name or id:
NtCreateFile and NtOpenFile (and the parent directory for creation and
deletion: FILE_ADD_FILE, FILE_DELETE_CHILD), NtCreateKey/NtOpenKey, the
NtOpen* of named objects and NtCreate* opening an existing one, and
(not yet) NtOpenProcess/NtOpenThread. The handle gets the rights
granted; handle duplication never grants more than the source handle
has; services then check the handle's access mask.

## Sessions

A process's session is its primary token's TokenSessionId at creation,
fixed for its life: SYSTEM is session 0, the boot user session 1, a
logon takes its caller's session. Moving a token to another session
(SetTokenInformation(TokenSessionId)) needs SeTcbPrivilege; a service
starts a program in the console's session by CreateProcessAsUserW with
such a token. `Local\` named objects are per session, `Global\` ones
shared. Each session has its own desktop: windows, hooks and the shell
are per session, and another session's HWND is invalid to a process
(ROADMAP RS steps B1, B2; ARCHITECTURE "Plan: sessions and remote
shasha").

## Integrity levels and UIPI

Integrity levels come from the token's mandatory label group (Medium
without one); SetTokenInformation(TokenIntegrityLevel) lowers it
(raising needs SeTcbPrivilege). User Interface Privilege Isolation (S6)
follows Microsoft's documentation for PostMessage, PostThreadMessage,
SendMessage and ChangeWindowMessageFilter(Ex):

- A process may post or send to another process's windows and threads
  only at an equal or higher integrity level (its primary token's
  label), except the messages that always pass and those the receiver
  lets through, for one window (ChangeWindowMessageFilterEx) or
  process-wide (ChangeWindowMessageFilter). A blocked message fails with
  ERROR_ACCESS_DENIED.
- Always passing: WM_NULL, WM_MOVE, WM_SIZE, WM_GETTEXT,
  WM_GETTEXTLENGTH, WM_GETHOTKEY, WM_GETICON, WM_RENDERFORMAT,
  WM_DRAWCLIPBOARD, WM_CHANGECBCHAIN, WM_THEMECHANGED. Microsoft says
  only "certain messages below WM_USER"; this commonly given list is
  still to verify on Windows.
- SetWindowPos and ShowWindow of another process's window (sent as
  user32's own messages) are blocked the same way, and no filter opens
  them. Processes at or below Low may not change filters, nor may anyone
  change another process's window's (ERROR_ACCESS_DENIED).
- The shell's actions for other programs (GalaDesktopAction: activate,
  minimize, close, snap, move, ...) need the same level as the window's
  process (STATUS_ACCESS_DENIED; `gala` says "access denied", exit 4);
  shasha's own mouse and keys act on any window, as Windows' taskbar
  does. `gala start` runs the program under a copy of the caller's
  primary token, as CreateProcess would; shasha's start menu starts
  programs as the boot user.
