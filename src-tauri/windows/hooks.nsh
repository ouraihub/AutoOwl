; Tauri NSIS installer hooks (bundle.windows.nsis.installerHooks).
;
; AutoOwl keeps its data beside the exe in $INSTDIR\data (src/storage.rs), which
; the stock uninstaller does not know about: it only clears %APPDATA% and
; %LOCALAPPDATA%\<identifier>. This hook removes $INSTDIR\data and the location
; file only when the user ticks "delete application data", and never during an
; update (/UPDATE reruns the old uninstaller before installing the new version).
;
; A data directory the user moved elsewhere (autoowl.paths.json dataDir) is not
; touched: that location was the user's choice.

!macro NSIS_HOOK_POSTUNINSTALL
  ${If} $DeleteAppDataCheckboxState = 1
  ${AndIf} $UpdateMode <> 1
    RMDir /r "$INSTDIR\data"
    Delete "$INSTDIR\autoowl.paths.json"
    ; The stock RMDir ran while data\ was still there; retry now it is gone.
    RMDir "$INSTDIR"
  ${EndIf}
!macroend
