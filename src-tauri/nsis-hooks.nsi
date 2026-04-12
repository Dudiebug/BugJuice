; BugJuice NSIS installer hooks
;
; Installs/removes the bugjuice-svc Windows service alongside the main app.
; The service handles privileged EMI reads; the app connects via named pipe.

!macro NSIS_HOOK_PREINSTALL
  ; Stop and remove any existing BugJuice service BEFORE extracting files.
  ; The service spawns bugjuice-lhm.exe as a child process — if the service
  ; is still running, the installer can't overwrite the locked LHM binary.
  DetailPrint "Stopping existing BugJuice service..."

  ; Force-kill the LHM helper first — it's a child of the service and holds
  ; a file lock on bugjuice-lhm.exe. taskkill /F sends TerminateProcess.
  nsExec::ExecToLog 'taskkill /F /IM bugjuice-lhm.exe'
  Pop $0

  ; Stop the service gracefully via SCM.
  nsExec::ExecToLog 'sc stop BugJuice'
  Pop $0

  ; Wait for the service process to fully exit.
  Sleep 2000

  ; Force-kill the service process if it's still hanging.
  nsExec::ExecToLog 'taskkill /F /IM bugjuice-svc.exe'
  Pop $0

  ; Brief pause so the OS releases file handles.
  Sleep 1000

  ; Remove the service registration from SCM.
  nsExec::ExecToLog 'sc delete BugJuice'
  Pop $0

  DetailPrint "Pre-install cleanup complete"
!macroend

!macro NSIS_HOOK_POSTINSTALL
  ; Strip Mark-of-the-Web (Zone.Identifier) from binaries so Windows
  ; Defender / SmartScreen doesn't block unsigned executables extracted
  ; from a downloaded installer.
  nsExec::ExecToLog 'powershell -NoProfile -Command "Get-ChildItem ''$INSTDIR\*.exe'' | Unblock-File"'
  Pop $0

  ; Install and start the BugJuice power monitoring service
  nsExec::ExecToLog '"$INSTDIR\bugjuice-svc.exe" install'
  Pop $0
  DetailPrint "Service install exit code: $0"

  ; On x64 (Intel/AMD) only, show an informational note about LHM.
  ; The actual LHM setup is now handled in-app with a guided wizard.
  ;
  ; Check PROCESSOR_ARCHITECTURE to distinguish real x64 from ARM64
  ; (${RunningX64} can be true on ARM64 under emulation).
  ReadEnvStr $1 PROCESSOR_ARCHITECTURE
  ${If} $1 == "AMD64"
    MessageBox MB_OK|MB_ICONINFORMATION \
      "For full CPU and GPU power monitoring, BugJuice will guide$\n\
       you through a quick one-time setup on first launch."
  ${EndIf}
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  ; Stop and remove the service before uninstalling files
  nsExec::ExecToLog '"$INSTDIR\bugjuice-svc.exe" uninstall'
  Pop $0
  DetailPrint "Service uninstall exit code: $0"

  ; Remove the LHM auto-start scheduled task if it exists
  nsExec::ExecToLog 'schtasks /Delete /TN "BugJuice-LHM" /F'
  Pop $0
  DetailPrint "LHM task cleanup exit code: $0"
!macroend
