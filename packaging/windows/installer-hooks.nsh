; Windows registration for the NSIS setup.exe, consumed via
; bundle.windows.nsis.installerHooks.
;
; Its twin is packaging/windows/registration.wxs, which does the same for the
; MSI. The two are asserted to declare identical keys by
; crates/termlab_tauri/tests/windows_registration_parity.rs — edit both or
; neither.
;
; All keys are HKCU: the installer is per-user and takes no UAC prompt.

!macro NSIS_HOOK_POSTINSTALL
  ; Explorer "Open TermLab here". Capital %V is required: it is the
  ; substitution that resolves for all three roots below.
  WriteRegStr HKCU "Software\Classes\Directory\shell\TermLab" "MUIVerb" "Open TermLab here"
  WriteRegStr HKCU "Software\Classes\Directory\shell\TermLab" "Icon" "$INSTDIR\termlab.exe"
  WriteRegStr HKCU "Software\Classes\Directory\shell\TermLab\command" "" '"$INSTDIR\termlab.exe" --working-directory "%V"'

  WriteRegStr HKCU "Software\Classes\Directory\Background\shell\TermLab" "MUIVerb" "Open TermLab here"
  WriteRegStr HKCU "Software\Classes\Directory\Background\shell\TermLab" "Icon" "$INSTDIR\termlab.exe"
  WriteRegStr HKCU "Software\Classes\Directory\Background\shell\TermLab\command" "" '"$INSTDIR\termlab.exe" --working-directory "%V"'

  WriteRegStr HKCU "Software\Classes\Drive\shell\TermLab" "MUIVerb" "Open TermLab here"
  WriteRegStr HKCU "Software\Classes\Drive\shell\TermLab" "Icon" "$INSTDIR\termlab.exe"
  WriteRegStr HKCU "Software\Classes\Drive\shell\TermLab\command" "" '"$INSTDIR\termlab.exe" --working-directory "%V"'

  ; Default Programs: Settings > Default apps and "Open with".
  WriteRegStr HKCU "Software\Clients\Terminal\TermLab\Capabilities" "ApplicationName" "TermLab"
  WriteRegStr HKCU "Software\Clients\Terminal\TermLab\Capabilities" "ApplicationDescription" "A cross-platform terminal emulator and SSH client"
  WriteRegStr HKCU "Software\Clients\Terminal\TermLab\Capabilities" "ApplicationIcon" "$INSTDIR\termlab.exe,0"
  WriteRegStr HKCU "Software\RegisteredApplications" "TermLab" "Software\Clients\Terminal\TermLab\Capabilities"

  ; App Paths: makes Win+R "termlab" work without touching PATH.
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\App Paths\termlab.exe" "" "$INSTDIR\termlab.exe"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\App Paths\termlab.exe" "Path" "$INSTDIR"
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  DeleteRegKey HKCU "Software\Classes\Directory\shell\TermLab"
  DeleteRegKey HKCU "Software\Classes\Directory\Background\shell\TermLab"
  DeleteRegKey HKCU "Software\Classes\Drive\shell\TermLab"
  DeleteRegKey HKCU "Software\Clients\Terminal\TermLab"
  DeleteRegValue HKCU "Software\RegisteredApplications" "TermLab"
  DeleteRegKey HKCU "Software\Microsoft\Windows\CurrentVersion\App Paths\termlab.exe"
!macroend
