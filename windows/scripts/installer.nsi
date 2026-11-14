; Codex Mixin - standard Windows installer (NSIS / MUI2).
; Installs under Program Files. A previous install is removed only after the
; user starts the install section. Built by
; scripts/package-windows.ps1.
Unicode true
; Compress the self-contained runtimes together so shared data is encoded once.
SetCompressor /SOLID lzma
SetCompressorDictSize 32

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
Var PreviousStartup
Var RestoreGateway

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
; Fixed install location. A prior install is removed in Install via its recorded
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
LangString GatewayStateUnknown ${LANG_SIMPCHINESE} "无法读取原网关状态，安装已在修改文件前停止。"
LangString GatewayStateUnknown ${LANG_ENGLISH} "Cannot determine the previous gateway state; installation stopped before changing files."
LangString GatewayRestoreFailed ${LANG_SIMPCHINESE} "安装已完成，但无法重启网关。请打开 Codex Mixin 检查错误并重新启动网关。"
LangString GatewayRestoreFailed ${LANG_ENGLISH} "Installation completed, but the gateway could not be restarted. Open Codex Mixin to inspect the error and start it again."

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
  ReadRegStr $PreviousStartup HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "CodexMixin"
FunctionEnd

Function RemovePrevious
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
    ExecWait '"$0\uninstall.exe" /S _?=$0' $1
    ${If} ${Errors}
      Abort "Failed to start the previous uninstaller."
    ${EndIf}
    ${If} $1 != 0
      Abort "Previous uninstall failed (exit $1)."
    ${EndIf}
    ${If} ${FileExists} "$0\${UI_EXE}"
      Abort "Previous application files could not be removed."
    ${EndIf}
    Delete "$0\uninstall.exe"
    RMDir "$0"
  ${EndIf}
FunctionEnd

Function StopRunning
  nsExec::Exec 'taskkill /IM ${UI_EXE} /F'
  nsExec::Exec 'taskkill /IM ${CLI_EXE} /T /F'
  Sleep 600
FunctionEnd

Function CaptureGatewayState
  StrCpy $RestoreGateway "0"
  ReadRegStr $0 HKLM "${UNINST_KEY}" "InstallLocation"
  ${If} $0 == ""
    StrCpy $0 "$INSTDIR"
  ${EndIf}
  ${If} ${FileExists} "$0\${CLI_EXE}"
    InitPluginsDir
    SetOutPath "$PLUGINSDIR"
    File /oname=gateway-install-state.ps1 "${__FILEDIR__}\gateway-install-state.ps1"
    nsExec::ExecToStack '"$SYSDIR\WindowsPowerShell\v1.0\powershell.exe" -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "$PLUGINSDIR\gateway-install-state.ps1" -CliPath "$0\${CLI_EXE}"'
    Pop $1
    Pop $2
    ${If} $1 == "0"
      StrCpy $RestoreGateway "1"
    ${ElseIf} $1 != "1"
      Abort "$(GatewayStateUnknown)"
    ${EndIf}
  ${EndIf}
FunctionEnd

Section "Install"
  Call CaptureGatewayState
  Call RemovePrevious
  Call StopRunning
  SetOutPath "$INSTDIR"
  ClearErrors
  File /r "${STAGING}\*.*"
  ${If} ${Errors}
    Abort "Failed to extract the complete application package."
  ${EndIf}

  ClearErrors
  ExecWait '"$SYSDIR\WindowsPowerShell\v1.0\powershell.exe" -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "$INSTDIR\update-user-path.ps1" -InstallRoot "$INSTDIR"' $0
  ${If} $0 != 0
    Abort "Failed to add Codex Mixin to the current user PATH (exit $0)."
  ${EndIf}

  ${If} $PreviousStartup != ""
    WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "CodexMixin" '$\"$INSTDIR\${UI_EXE}$\" --background'
  ${EndIf}

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
  ${If} $RestoreGateway == "1"
    DetailPrint "Restoring the gateway..."
    nsExec::ExecToStack '"$INSTDIR\${CLI_EXE}" --no-tui --json-errors service start --managed --json'
    Pop $0
    Pop $1
    ${If} $0 != "0"
      DetailPrint "Installation completed, but the gateway could not be restarted."
      IfSilent +2
      MessageBox MB_ICONEXCLAMATION "$(GatewayRestoreFailed)"
      SetErrorLevel 1
    ${EndIf}
  ${EndIf}
SectionEnd

Section "Uninstall"
  Call un.StopRunning
  ExecWait '"$SYSDIR\WindowsPowerShell\v1.0\powershell.exe" -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "$INSTDIR\update-user-path.ps1" -InstallRoot "$INSTDIR" -Remove'
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
