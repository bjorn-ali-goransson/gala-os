; A small NSIS installer for gala's install test (pk_install_test, scenario
; pk_install_nsis; ROADMAP PK). Built by make_source.py with makensis
; (NSIS 3, zlib/libpng licence: THIRD_PARTY.md) for x64 (gala has no
; WOW64 for 32-bit installers yet). It does what a typical per-user NSIS
; package does: files into $INSTDIR, an uninstaller, the ARP entry under
; HKCU with an UninstallString only (winget then adds /S itself), and a
; normal UI (a directory page) that /S skips.
;
;   makensis -DVERSION=1.0.0 -DHELLO=<hello.exe> -DOUTFILE=<setup.exe> pknsis.nsi

Unicode true
Target amd64-unicode
SetCompressor /SOLID lzma
RequestExecutionLevel user
ManifestDPIAware true

!define ARP "Software\Microsoft\Windows\CurrentVersion\Uninstall\GalaTest.PkNsis"

Name "PkNsis"
OutFile "${OUTFILE}"
InstallDir "$LOCALAPPDATA\Programs\PkNsis"
ShowInstDetails show

Page directory
Page instfiles
UninstPage uninstConfirm
UninstPage instfiles

Section "PkNsis"
  SetOutPath "$INSTDIR"
  File "/oname=pknsis.exe" "${HELLO}"
  FileOpen $0 "$INSTDIR\README.txt" w
  FileWrite $0 "PkNsis ${VERSION}$\r$\nA test package for gala install (NSIS).$\r$\n"
  FileClose $0
  WriteUninstaller "$INSTDIR\uninstall.exe"
  WriteRegStr HKCU "${ARP}" "DisplayName" "PkNsis"
  WriteRegStr HKCU "${ARP}" "DisplayVersion" "${VERSION}"
  WriteRegStr HKCU "${ARP}" "Publisher" "gala-os"
  WriteRegStr HKCU "${ARP}" "InstallLocation" "$INSTDIR"
  WriteRegStr HKCU "${ARP}" "UninstallString" '"$INSTDIR\uninstall.exe"'
  WriteRegDWORD HKCU "${ARP}" "NoModify" 1
  WriteRegDWORD HKCU "${ARP}" "NoRepair" 1
SectionEnd

Section "Uninstall"
  Delete "$INSTDIR\pknsis.exe"
  Delete "$INSTDIR\README.txt"
  Delete "$INSTDIR\uninstall.exe"
  RMDir "$INSTDIR"
  DeleteRegKey HKCU "${ARP}"
SectionEnd
