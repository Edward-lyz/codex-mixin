; Codex Mixin - standard Windows installer (NSIS / MUI2).
; Installs under Program Files. Any previous install is uninstalled first, using
; the location recorded in the uninstall registry key. Built by
; scripts/package-windows.ps1.
Unicode true

!include "MUI2.nsh"
!include "LogicLib.nsh"

!ifndef VERSION
  !define VERSION "1.0.0.0"
!endif
!ifndef STAGING
  !error "STAGING (assembled app dir) must be defined: /DSTAGING=<path>"
!endif
!ifndef OUTFILE
  !define OUTFILE "CodexMixin-Setup.exe"
!endif

!define APPNAME "Codex Mixin"
!define UI_EXE "codex_mixin_ui.exe"
!define CLI_EXE "codex-mixin.exe"
!define UNINST_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\CodexMixin"

Name "${APPNAME}"
OutFile "${OUTFILE}"
; Installing under Program Files needs elevation. The inner "generate
; uninstaller" pass only writes uninstall.exe to disk, so it runs unelevated to
; avoid a UAC prompt during the build.
!ifdef GEN_UNINSTALLER
  RequestExecutionLevel user
!else
  RequestExecutionLevel admin
!endif
; Fixed install location. A prior install is removed in .onInit via its recorded
; uninstaller, so we always land here regardless of where it used to live.
InstallDir "$PROGRAMFILES64\${APPNAME}"
ShowInstDetails show
ShowUninstDetails show

VIProductVersion "${VERSION}"
VIAddVersionKey "ProductName" "${APPNAME}"
VIAddVersionKey "FileDescription" "${APPNAME} Installer"
VIAddVersionKey "FileVersion" "${VERSION}"
VIAddVersionKey "ProductVersion" "${VERSION}"
VIAddVersionKey "CompanyName" "Baidu"
VIAddVersionKey "LegalCopyright" "Copyright (C) 2026"

!ifdef ICONFILE
  !define MUI_ICON "${ICONFILE}"
  !define MUI_UNICON "${ICONFILE}"
!endif

!define MUI_ABORTWARNING
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!define MUI_FINISHPAGE_RUN "$INSTDIR\${UI_EXE}"
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES

!insertmacro MUI_LANGUAGE "SimpChinese"
!insertmacro MUI_LANGUAGE "English"

Function .onInit
!ifdef GEN_UNINSTALLER
  ; Inner build pass: emit the uninstaller to disk so it can be Authenticode
  ; signed, then quit without installing. The outer build ships this signed
  ; uninstall.exe via File instead of WriteUninstaller.
  SetSilent silent
  WriteUninstaller "${UNINST_OUT}"
  SetErrorLevel 0
  Quit
!endif
  ; Remove any previous install first. Its actual location was recorded in the
  ; uninstall registry key at install time, so this cleans up old versions that
  ; lived elsewhere even though we now install under Program Files. Run the old
  ; uninstaller in place with _?= so ExecWait blocks until it finishes; _?= keeps
  ; it from self-deleting, so we remove the leftover uninstaller and (if empty)
  ; its directory afterwards.
  ReadRegStr $0 HKLM "${UNINST_KEY}" "InstallLocation"
  ${If} $0 != ""
  ${AndIf} ${FileExists} "$0\uninstall.exe"
    ClearErrors
    ExecWait '"$0\uninstall.exe" /S _?=$0'
    Delete "$0\uninstall.exe"
    RMDir "$0"
  ${EndIf}
FunctionEnd

Function StopRunning
  nsExec::Exec 'taskkill /IM ${UI_EXE} /F'
  nsExec::Exec 'taskkill /IM ${CLI_EXE} /T /F'
  Sleep 600
FunctionEnd

Section "Install"
  Call StopRunning
  SetOutPath "$INSTDIR"
  File /r "${STAGING}\*.*"

  CreateShortCut "$SMPROGRAMS\${APPNAME}.lnk" "$INSTDIR\${UI_EXE}" "" "$INSTDIR\${UI_EXE}" 0

  ; Ship the pre-signed uninstaller when provided; otherwise generate it here.
!ifdef PRESIGNED_UNINSTALLER
  File "/oname=uninstall.exe" "${PRESIGNED_UNINSTALLER}"
!else
  WriteUninstaller "$INSTDIR\uninstall.exe"
!endif
  WriteRegStr   HKLM "${UNINST_KEY}" "DisplayName"     "${APPNAME}"
  WriteRegStr   HKLM "${UNINST_KEY}" "DisplayVersion"  "${VERSION}"
  WriteRegStr   HKLM "${UNINST_KEY}" "Publisher"       "Baidu"
  WriteRegStr   HKLM "${UNINST_KEY}" "DisplayIcon"     "$INSTDIR\${UI_EXE}"
  WriteRegStr   HKLM "${UNINST_KEY}" "InstallLocation" "$INSTDIR"
  WriteRegStr   HKLM "${UNINST_KEY}" "UninstallString" "$\"$INSTDIR\uninstall.exe$\""
  WriteRegStr   HKLM "${UNINST_KEY}" "QuietUninstallString" "$\"$INSTDIR\uninstall.exe$\" /S"
  WriteRegDWORD HKLM "${UNINST_KEY}" "NoModify" 1
  WriteRegDWORD HKLM "${UNINST_KEY}" "NoRepair" 1
SectionEnd

Section "Uninstall"
  Call un.StopRunning
  Delete "$SMPROGRAMS\${APPNAME}.lnk"
  DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "CodexMixin"
  RMDir /r "$INSTDIR"
  DeleteRegKey HKLM "${UNINST_KEY}"
  ; User configuration under %USERPROFILE%\.codex-mixin is intentionally preserved.
SectionEnd

Function un.StopRunning
  nsExec::Exec 'taskkill /IM ${UI_EXE} /F'
  nsExec::Exec 'taskkill /IM ${CLI_EXE} /T /F'
  Sleep 600
FunctionEnd
