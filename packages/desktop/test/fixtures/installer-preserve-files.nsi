Unicode true
Name "ESCode preserve-files smoke"
OutFile "${OUTPUT_FILE}"
InstallDir "$TEMP\escode-preserve-files-smoke"
RequestExecutionLevel user
SilentInstall silent

!addincludedir "${NSIS_INCLUDE_DIR}"
!addincludedir "${ELECTRON_BUILDER_NSIS_INCLUDE_DIR}"
!addplugindir /x86-unicode "${NSIS_PLUGIN_DIR}"
!include "LogicLib.nsh"

!define APP_FILENAME "ESCode"
!define APP_ID "dev.escode.preserve-files-smoke"
!define UNINSTALL_FILENAME "uninstaller.exe"
!ifdef FULL_UNINSTALL
  ; 普通卸载（非覆盖更新）路径：验证安装目录树内 .escode 的保留语义。
  !define isUpdated `1 == 0`
!else
  !define isUpdated `1 == 1`
!endif
; This fixture has no electron-builder uninstaller.nsh section; use a normal function for log smoke coverage.
!ifdef BUILD_UNINSTALLER
  !define ESCODE_UNINSTALLER_FUNCTION_PREFIX ""
!endif

Var appExe

!include "${ESCODE_INSTALLER_NSH}"

Section
  SetOutPath "$INSTDIR"
  File /oname=ESCode.exe "${FIXTURE_EXE}"
  StrCpy $appExe "$INSTDIR\ESCode.exe"
  !ifndef OMIT_MANIFEST
    FileOpen $0 "$INSTDIR\.escode-install-manifest" w
    FileWrite $0 "owned-by-escode.txt$\r$\n"
    FileWrite $0 "..\user-data.txt$\r$\n"
    FileClose $0
  !endif
  !ifdef FAIL_DELETE
    ; Keep the file handle open so Windows denies the manifest cleanup delete.
    FileOpen $0 "$INSTDIR\owned-by-escode.txt" w
    FileWrite $0 "locked app payload"
  !else
    FileOpen $0 "$INSTDIR\owned-by-escode.txt" w
    FileWrite $0 "old app payload"
    FileClose $0
  !endif
  FileOpen $0 "$INSTDIR\user-data.txt" w
  FileWrite $0 "keep me"
  FileClose $0
  !ifdef NESTED_DATA_DIR
    CreateDirectory "$INSTDIR\data\.escode\cli\db"
    FileOpen $0 "$INSTDIR\data\.escode\cli\db\db.sqlite" w
    FileWrite $0 "session data"
    FileClose $0
  !endif
  !insertmacro customRemoveFiles
  IfFileExists "$INSTDIR\owned-by-escode.txt" 0 escodeOwnedRemoved
    StrCpy $1 "present"
    Goto escodeWriteMarker
  escodeOwnedRemoved:
    StrCpy $1 "removed"
  escodeWriteMarker:
    FileOpen $0 "$INSTDIR\remove-result.txt" w
    FileWrite $0 "$1"
    FileClose $0
SectionEnd
