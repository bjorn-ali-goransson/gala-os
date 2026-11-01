# Security model (design, 2026-11-01)

Status: phases S1 (tokens in the kernel; `token_test`, ARCHITECTURE
"Access tokens"), S2 (descriptors kept; `security_test`, ARCHITECTURE
"Security descriptors") and S3 (enforcement at open for files, keys and
named objects, a second account; `access_test`, ARCHITECTURE "Access
checks", "Accounts and logon") are done (2026-11-01); S4 is begun
(`logon_test`: CreateProcessAsUserW's token rules, pipes impersonating
their client's token, GetUserProfileDirectoryW; `system_test`: programs
run as SYSTEM with the boot option `as=system`; `lsa_test`: LSA logons,
S4U and interactive, as sshd makes them, and virtual accounts;
`profile_test`: LoadUserProfileW, profiles placed at boot). sshd's
key-authenticated login works (`sshd_login`, 2026-11-01); S5 is done
(`restricted_test`: PostgreSQL's restricted token, deny-only groups,
SetTokenInformation, integrity levels and no-write-up). Before S1, gala had the handle half of NT
security (each handle carries a granted access mask, mapped through the
object type's generic mapping, and services check it), and descriptions
of security data in user mode: advapi32 builds and reads SIDs, ACLs and
security descriptors (also SDDL), describes one user's token, and
synthesizes the descriptor a file "has". Nothing is kept or enforced:
there is no token object in the kernel, objects keep no descriptors, and
no open checks who is asking. This document is the plan for the real
model, driven by the programs that need it (ROADMAP, agreed order).

Clean-room sources: Microsoft's documentation (Access Control, the
security descriptor and ACE formats, AccessCheck's algorithm, the SDDL
format, token information classes, privileges, LogonUser/LsaLogonUser),
"Windows Internals" (chapter on security), Wine and ReactOS behaviour,
and black-box results on Windows. No Windows code.

## Driving programs

- **Win32-OpenSSH (sshd)**, first. Now: `ssh-keygen -A` and `sshd -t` run
  (the `sshd` scenario). It checks that its host keys and
  `authorized_keys` are owned by the user, Administrators or SYSTEM and
  writable by nobody else (GetNamedSecurityInfo, ACEs), so the answers
  must be the real descriptors. A login needs a token for the user
  (LogonUserW with a password, or an S4U logon through LsaLogonUser for
  key authentication, after LsaRegisterLogonProcess), then
  CreateProcessAsUserW of the session process under it, impersonation
  while reading the user's files, and per-user profile directories.
- **PostgreSQL**: refuses to run with administrative rights and drops
  them with CreateRestrictedToken; checks its data directory's
  permissions; needs a persistent disk.
- **SQL Server LocalDB kept across reboots**: the instance's files and
  registration on a disk; SqlUserInstance sets ACLs on the instance
  folders.

## Design

### 1. Tokens (kernel objects)

A `Token` object type, as documented in "Access Tokens": user SID;
groups (SID and attributes: mandatory, enabled, owner, logon id,
deny-only, integrity); privileges (LUID and attributes); owner and
primary group indexes; default DACL; integrity level (the mandatory
label group); logon session (AuthenticationId LUID); type (primary,
impersonation) and impersonation level; token and modified ids; restricted
SIDs (CreateRestrictedToken); source; elevation type. A process holds a
primary token (inherited from its creator unless created with one: 
CreateProcessAsUserW); a thread may hold an impersonation token.

System calls (numbers in the pinned build's table, gala range until
verified): NtOpenProcessToken(Ex), NtOpenThreadToken(Ex),
NtQueryInformationToken, NtSetInformationToken, NtDuplicateToken,
NtAdjustPrivilegesToken, NtAdjustGroupsToken, NtFilterToken,
NtSetInformationThread(ThreadImpersonationToken), NtImpersonateThread,
NtAccessCheck, NtPrivilegeCheck, and a gala-specific privileged call to
create a token for a logon (NtCreateToken's role; needs SeTcbPrivilege).
advapi32's token functions become wrappers over them; its stand-in token
(a duplicate of the process handle) goes.

### 2. Accounts and logon

A local account database (the SAM's role), kept by the kernel in the
registry (`HKLM\SAM`-like gala key, then on disk): users (name, SID,
NT password hash: MD4 of the UTF-16 password, which secur32's NTLM uses
too), groups and aliases (Administrators, Users...), and the well-known
service identities (SYSTEM, LocalService, NetworkService). The first
user stays `gala` (RID 1001, empty password) until accounts can be
managed (NetUserAdd). LogonUserW/LsaLogonUser (interactive, network,
batch, service, S4U) check the password or the caller's privilege and
make a logon session and token through the kernel. Processes started at
boot run as `gala` (later: the boot configuration names the account;
services as SYSTEM).

### 3. Security descriptors on objects

Every securable object keeps a self-relative descriptor in the kernel
heap: named objects (events, mutants, semaphores, sections, timers,
directories, symbolic links), files and directories (a field of the
ramfs node, and of the on-disk format later), registry keys, processes,
threads, tokens. At creation the descriptor is the caller's
(SECURITY_ATTRIBUTES), completed by the documented inheritance rules:
the parent's inheritable ACEs (OI/CI/NP/IO, CREATOR OWNER/GROUP
replaced, the auto-inheritance flags), else the creator token's default
DACL; owner and group from the token. NtQuerySecurityObject and
NtSetSecurityObject read and change them (READ_CONTROL, WRITE_DAC,
WRITE_OWNER, ACCESS_SYSTEM_SECURITY for the SACL); advapi32's
Get/SetFileSecurity, Get/SetSecurityInfo, Get/SetNamedSecurityInfo,
Get/SetKernelObjectSecurity and Reg{Get,Set}KeySecurity use them
instead of synthesizing.

### 4. One access check

`access_check(descriptor, token, desired, mapping) -> Result<granted>`
from AccessCheck's documented algorithm: generic rights mapped first;
MAXIMUM_ALLOWED; a NULL DACL grants all, an empty one nothing; ACEs in
order, inherit-only ones skipped, deny ACEs before the request is
satisfied deny; restricted tokens checked twice (both must grant); the
owner gets READ_CONTROL and WRITE_DAC implicitly (unless an OWNER
RIGHTS ACE says otherwise); the mandatory label (no write up, no read
up when asked); privileges: SeBackup (read), SeRestore (write),
SeTakeOwnership (WRITE_OWNER), SeSecurity (ACCESS_SYSTEM_SECURITY).
It runs wherever an object is opened by name or id: NtCreateFile and
NtOpenFile (and the parent directory for creation and deletion:
FILE_ADD_FILE, FILE_DELETE_CHILD), NtCreateKey/NtOpenKey, the NtOpen*
of named objects, NtOpenProcess/NtOpenThread, NtCreateSection of a file
(the file handle's access already decided). Handle duplication never
grants more than the source handle has. The effective token is the
thread's impersonation token if any, else the process's.

### 5. Persistence

The disk work (virtio-blk, a file system, registry hives) keeps the
descriptors: the file system stores one descriptor per file or
directory (shared ones stored once by id, as NTFS's $Secure does), the
hives store each key's. The format is designed with that field from the
start.

## Phases (each from a test program with Windows' results)

1. **S1 Tokens in the kernel** (done): `token_test` (GetTokenInformation of the
   process token and of duplicates, impersonation and RevertToSelf,
   OpenThreadToken's ERROR_NO_TOKEN, AdjustTokenPrivileges, a filtered
   token from CreateRestrictedToken) passing on the kernel's tokens.
2. **S2 Descriptors kept** (done): `security_test` (create with a descriptor and
   read it back, inheritance into a directory's children, SetFileSecurity
   and SetSecurityInfo kept, registry keys, named objects); sshd's
   permission checks then see real answers.
3. **S3 Enforcement** (done; processes, threads and pipes not yet): access checks at open, with a second account
   (LogonUserW of a test user): denied and allowed opens, privileges,
   deny ACEs, the owner's implicit rights, integrity levels.
4. **S4 Logons and per-user processes** (sshd's login done, 2026-11-01):
   LsaLogonUser S4U, CreateProcessAsUserW, profiles, virtual accounts
   (LsaManageSidNameMapping, account rights, LOGON32_PROVIDER_VIRTUAL);
   sshd accepts a key-authenticated login from the host and runs a
   command (`sshd_login`), also with a password (`sshd_password`). Left:
   logon rights enforced for ordinary accounts, NetUserAdd.
5. **S5 Restricted tokens and integrity** (done, 2026-11-01;
   `restricted_test`): pg_ctl's CreateRestrictedProcess (Administrators
   and Power Users deny-only, privileges deleted, the default DACL
   extended), CheckTokenMembership, SetTokenInformation, mandatory labels
   (no write up). Left: labels on objects low tokens create.
Then the disk: D1 a virtio-blk driver, D2 a file system with
descriptors, D3 registry hives on it.
