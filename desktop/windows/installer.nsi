; Codex Mixin per-user installer (NSIS / MUI2), built by package.ps1.
; Installs under %LOCALAPPDATA%\Programs so the in-app updater can replace the
; app folder without elevation. User configuration in %USERPROFILE%\.codex-mixin
; is never touched.
Unicode true
SetCompressor /SOLID lzma

!include "MUI2.nsh"
!include "LogicLib.nsh"

!ifndef VERSION
  !define VERSION "0.0.0.0"
!endif
!ifndef DISPLAY_VERSION
  !define DISPLAY_VERSION "${VERSION}"
!endif
!ifndef STAGING
  !error "STAGING (assembled app folder) must be defined: /DSTAGING=<path>"
!endif
!ifndef OUTFILE
  !define OUTFILE "CodexMixin-Setup.exe"
!endif

!define APPNAME "Codex Mixin"
!define UI_EXE "Codex Mixin.exe"
!define LEGACY_UI_EXE "codex_mixin_ui.exe"
!define CLI_EXE "codex-mixin.exe"
!define UNINST_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\CodexMixin"
!define RUN_KEY "Software\Microsoft\Windows\CurrentVersion\Run"
!define RUN_VALUE "CodexMixin"
Var RestoreGateway
Var LegacyInstall

Name "${APPNAME}"
OutFile "${OUTFILE}"
RequestExecutionLevel user
InstallDir "$LOCALAPPDATA\Programs\${APPNAME}"
ShowInstDetails show
ShowUninstDetails show

VIProductVersion "${VERSION}"
VIAddVersionKey "ProductName" "${APPNAME}"
VIAddVersionKey "FileDescription" "${APPNAME} Installer"
VIAddVersionKey "FileVersion" "${DISPLAY_VERSION}"
VIAddVersionKey "ProductVersion" "${DISPLAY_VERSION}"
VIAddVersionKey "LegalCopyright" "Copyright (C) 2026"

!ifdef ICONFILE
  !define MUI_ICON "${ICONFILE}"
  !define MUI_UNICON "${ICONFILE}"
!endif

!define MUI_ABORTWARNING
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
LangString GatewayTaskPermission ${LANG_SIMPCHINESE} "旧版网关的登录启动任务需要管理员授权才能更新。是否授权修复此任务并重试启动网关？"
LangString GatewayTaskPermission ${LANG_ENGLISH} "The previous gateway startup task needs administrator approval to be updated. Repair permissions on this task and retry starting the gateway?"
LangString GatewaySessionOnly ${LANG_SIMPCHINESE} "网关已为本次登录启动。旧启动任务的权限仍需修复，之后才能使用新版网关登录启动。"
LangString GatewaySessionOnly ${LANG_ENGLISH} "The gateway is running for this session. Repair the previous startup task permissions to start the new gateway at sign-in."
LangString DesktopStartupHandoffFailed ${LANG_SIMPCHINESE} "无法将 Codex Mixin 的桌面登录启动项更新为新版。"
LangString DesktopStartupHandoffFailed ${LANG_ENGLISH} "Failed to move Codex Mixin desktop startup to the new installation."

Function StopRunning
  nsExec::Exec 'taskkill /IM "${UI_EXE}" /F'
  nsExec::Exec 'taskkill /IM ${LEGACY_UI_EXE} /F'
  nsExec::Exec 'taskkill /IM ${CLI_EXE} /T /F'
  Sleep 600
FunctionEnd

Function HandoffDesktopStartup
  ReadRegStr $0 HKCU "${RUN_KEY}" "${RUN_VALUE}"
  ${If} $0 == ""
    Return
  ${EndIf}
  StrCpy $1 ""
  ${If} $LegacyInstall != ""
    StrCpy $1 '$\"$LegacyInstall\${LEGACY_UI_EXE}$\" --background'
  ${EndIf}
  ; Preserve the existing value name so Windows keeps its Startup Apps choice.
  ; A fresh install or an unrelated command is left alone.
  ${If} $0 == '$\"$PROGRAMFILES64\${APPNAME}\${LEGACY_UI_EXE}$\" --background'
  ${OrIf} $0 == '$\"$INSTDIR\${LEGACY_UI_EXE}$\" --background'
  ${OrIf} $0 == $1
    ClearErrors
    WriteRegStr HKCU "${RUN_KEY}" "${RUN_VALUE}" '$\"$INSTDIR\${UI_EXE}$\" --background'
    IfErrors 0 +2
    Abort "$(DesktopStartupHandoffFailed)"
  ${EndIf}
FunctionEnd

; Remembers whether the gateway of an existing install should run again
; after the files are replaced.
Function CaptureGatewayState
  StrCpy $RestoreGateway "0"
  SetRegView 64
  ReadRegStr $LegacyInstall HKLM "${UNINST_KEY}" "InstallLocation"
  SetRegView lastused
  StrCpy $3 "$INSTDIR\${CLI_EXE}"
  ${IfNot} ${FileExists} "$3"
  ${AndIf} $LegacyInstall != ""
    StrCpy $3 "$LegacyInstall\${CLI_EXE}"
  ${EndIf}
  ${IfNot} ${FileExists} "$3"
    StrCpy $3 "$PROGRAMFILES64\${APPNAME}\${CLI_EXE}"
  ${EndIf}
  ${If} ${FileExists} "$3"
    InitPluginsDir
    SetOutPath "$PLUGINSDIR"
    File "/oname=gateway-install-state.ps1" "${__FILEDIR__}\gateway-install-state.ps1"
    nsExec::ExecToStack '"$SYSDIR\WindowsPowerShell\v1.0\powershell.exe" -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "$PLUGINSDIR\gateway-install-state.ps1" -CliPath "$3"'
    Pop $1
    Pop $2
    ${If} $1 == "0"
      StrCpy $RestoreGateway "1"
    ${ElseIf} $1 == "3"
      StrCpy $RestoreGateway "2"
    ${ElseIf} $1 != "1"
      Abort "$(GatewayStateUnknown)"
    ${EndIf}
  ${EndIf}
FunctionEnd

Section "Install"
  Call CaptureGatewayState
  Call StopRunning
  SetOutPath "$INSTDIR"
  ClearErrors
  File /r "${STAGING}\*.*"
  ${If} ${Errors}
    Abort "Failed to extract the complete application package."
  ${EndIf}

  ExecWait '"$SYSDIR\WindowsPowerShell\v1.0\powershell.exe" -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "$INSTDIR\update-user-path.ps1" -InstallRoot "$INSTDIR"' $0
  ${If} $0 != 0
    Abort "Failed to add Codex Mixin to the user PATH (exit $0)."
  ${EndIf}

  CreateShortCut "$SMPROGRAMS\${APPNAME}.lnk" "$INSTDIR\${UI_EXE}" "" "$INSTDIR\${UI_EXE}" 0
  WriteUninstaller "$INSTDIR\uninstall.exe"
  WriteRegStr   HKCU "${UNINST_KEY}" "DisplayName"     "${APPNAME}"
  WriteRegStr   HKCU "${UNINST_KEY}" "DisplayVersion"  "${DISPLAY_VERSION}"
  WriteRegStr   HKCU "${UNINST_KEY}" "DisplayIcon"     "$INSTDIR\${UI_EXE}"
  WriteRegStr   HKCU "${UNINST_KEY}" "InstallLocation" "$INSTDIR"
  WriteRegStr   HKCU "${UNINST_KEY}" "UninstallString" "$\"$INSTDIR\uninstall.exe$\""
  WriteRegStr   HKCU "${UNINST_KEY}" "QuietUninstallString" "$\"$INSTDIR\uninstall.exe$\" /S"
  WriteRegDWORD HKCU "${UNINST_KEY}" "NoModify" 1
  WriteRegDWORD HKCU "${UNINST_KEY}" "NoRepair" 1
  ${If} $RestoreGateway != "0"
    DetailPrint "Restoring the gateway..."
    InitPluginsDir
    SetOutPath "$PLUGINSDIR"
    File "/oname=restore-gateway.ps1" "${__FILEDIR__}\restore-gateway.ps1"
    SetOutPath "$INSTDIR"
    StrCpy $3 ""
    ${If} $RestoreGateway == "2"
      StrCpy $3 "-RestoreAutostart"
    ${EndIf}
    nsExec::ExecToStack '"$SYSDIR\WindowsPowerShell\v1.0\powershell.exe" -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "$PLUGINSDIR\restore-gateway.ps1" -CliPath "$INSTDIR\${CLI_EXE}" $3'
    Pop $0
    Pop $1
    ${If} $0 == "5"
      IfSilent start_session_gateway
      MessageBox MB_ICONQUESTION|MB_YESNO|MB_DEFBUTTON2 "$(GatewayTaskPermission)" IDNO start_session_gateway
      nsExec::ExecToStack '"$SYSDIR\WindowsPowerShell\v1.0\powershell.exe" -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "$PLUGINSDIR\restore-gateway.ps1" -CliPath "$INSTDIR\${CLI_EXE}" $3 -RepairLegacyTask'
      Pop $0
      Pop $1
      ${If} $0 == "0"
        Goto gateway_restore_done
      ${EndIf}
      DetailPrint "$1"
      start_session_gateway:
      nsExec::ExecToStack '"$SYSDIR\WindowsPowerShell\v1.0\powershell.exe" -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "$PLUGINSDIR\restore-gateway.ps1" -CliPath "$INSTDIR\${CLI_EXE}"'
      Pop $0
      Pop $1
      ${If} $0 == "0"
        DetailPrint "$(GatewaySessionOnly)"
        IfSilent gateway_restore_done
        MessageBox MB_ICONINFORMATION "$(GatewaySessionOnly)"
      ${EndIf}
    ${EndIf}
    ${If} $0 != "0"
      IfSilent +2
      MessageBox MB_ICONEXCLAMATION "$(GatewayRestoreFailed)"
      SetErrorLevel 1
    ${EndIf}
    gateway_restore_done:
  ${EndIf}

  ; Restore the running gateway even if moving the desktop Run entry fails.
  Call HandoffDesktopStartup
SectionEnd

Section "Uninstall"
  nsExec::Exec 'taskkill /IM "${UI_EXE}" /F'
  nsExec::Exec 'taskkill /IM ${CLI_EXE} /T /F'
  Sleep 600
  ExecWait '"$SYSDIR\WindowsPowerShell\v1.0\powershell.exe" -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "$INSTDIR\update-user-path.ps1" -InstallRoot "$INSTDIR" -Remove'
  Delete "$SMPROGRAMS\${APPNAME}.lnk"
  ReadRegStr $0 HKCU "${RUN_KEY}" "${RUN_VALUE}"
  ${If} $0 == '$\"$INSTDIR\${UI_EXE}$\" --background'
    DeleteRegValue HKCU "${RUN_KEY}" "${RUN_VALUE}"
  ${EndIf}
  RMDir /r "$INSTDIR"
  DeleteRegKey HKCU "Software\Classes\AppUserModelId\com.edward.codex-mixin"
  DeleteRegKey HKCU "${UNINST_KEY}"
SectionEnd
