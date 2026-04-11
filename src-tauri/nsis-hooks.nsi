; BugJuice NSIS installer hooks
;
; Installs/removes the bugjuice-svc Windows service alongside the main app.
; The service handles privileged EMI reads; the app connects via named pipe.

!macro NSIS_HOOK_POSTINSTALL
  ; Install and start the BugJuice power monitoring service
  nsExec::ExecToLog '"$INSTDIR\bugjuice-svc.exe" install'
  Pop $0
  DetailPrint "Service install exit code: $0"

  ; On x64 builds, offer to open the LibreHardwareMonitor download page.
  ; LHM provides enhanced power monitoring (per-core RAPL, AMD GPU power)
  ; via its signed PawnIO driver. ARM64 builds don't need this — EMI
  ; already provides rich power channels on Snapdragon X.
  ${If} ${RunningX64}
    MessageBox MB_YESNO|MB_ICONQUESTION \
      "For enhanced power monitoring, BugJuice recommends installing$\n\
       LibreHardwareMonitor (free, open source).$\n$\n\
       This enables per-core CPU power, AMD GPU power, and additional$\n\
       sensors. BugJuice works without it, but with less detail on Intel/AMD.$\n$\n\
       Open the download page now?" \
      IDYES lhm_yes IDNO lhm_no
    lhm_yes:
      ExecShell "open" "https://github.com/LibreHardwareMonitor/LibreHardwareMonitor/releases"
    lhm_no:
  ${EndIf}
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  ; Stop and remove the service before uninstalling files
  nsExec::ExecToLog '"$INSTDIR\bugjuice-svc.exe" uninstall'
  Pop $0
  DetailPrint "Service uninstall exit code: $0"
!macroend
