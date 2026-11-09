; The online app's installer: dx's NSIS template (dioxus-cli 0.7.10,
; src/bundler/windows.rs) with a components page that asks whether to
; register the browser bridge, and an uninstaller that removes it again.
; dx renders it with Handlebars; keep it in step with dx's own on a dx update.
; See docs/releasing.md.
!include "MUI2.nsh"
!include "FileFunc.nsh"
!include "x64.nsh"

; Basic installer attributes
Name "{{product_name}}"
OutFile "{{output_path}}"
Unicode true
{{#if install_mode_per_machine}}
InstallDir "$PROGRAMFILES\{{product_name}}"
{{else}}
InstallDir "$LOCALAPPDATA\Programs\{{product_name}}"
{{/if}}

; Request appropriate privileges
{{#if install_mode_per_machine}}
RequestExecutionLevel admin
{{else if install_mode_both}}
RequestExecutionLevel admin
{{else}}
RequestExecutionLevel user
{{/if}}

; Version information
VIProductVersion "{{version}}.0"
VIAddVersionKey "ProductName" "{{product_name}}"
VIAddVersionKey "FileVersion" "{{version}}"
VIAddVersionKey "ProductVersion" "{{version}}"
VIAddVersionKey "FileDescription" "{{short_description}}"
{{#if publisher}}
VIAddVersionKey "CompanyName" "{{publisher}}"
{{/if}}
{{#if copyright}}
VIAddVersionKey "LegalCopyright" "{{copyright}}"
{{/if}}

; MUI settings
!define MUI_ABORTWARNING
{{#if installer_icon}}
!define MUI_ICON "{{installer_icon}}"
{{/if}}
{{#if header_image}}
!define MUI_HEADERIMAGE
!define MUI_HEADERIMAGE_BITMAP "{{header_image}}"
{{/if}}
{{#if sidebar_image}}
!define MUI_WELCOMEFINISHPAGE_BITMAP "{{sidebar_image}}"
{{/if}}

; Pages
{{#if license}}
!insertmacro MUI_PAGE_LICENSE "{{license}}"
{{/if}}
!define MUI_COMPONENTSPAGE_TEXT_TOP "Choose whether websites can reach Airlock through its browser extension. You can also do this later from the Extension status in the app."
!insertmacro MUI_PAGE_COMPONENTS
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH

; Uninstaller pages
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES

; Language
!insertmacro MUI_LANGUAGE "English"
{{#each additional_languages}}
!insertmacro MUI_LANGUAGE "{{this}}"
{{/each}}

; Installer section
Section "{{product_name}}" SecApp
    SectionIn RO
    SetOutPath $INSTDIR

    ; Install main binary
    File "{{main_binary_path}}"

    ; Install resources
    {{#each staged_files}}
    SetOutPath "$INSTDIR{{#if this.target_dir}}\{{this.target_dir}}{{/if}}"
    File "{{this.source}}"
    {{/each}}

    SetOutPath $INSTDIR

    ; Create uninstaller
    WriteUninstaller "$INSTDIR\uninstall.exe"

    ; Create Start Menu shortcuts
    CreateDirectory "$SMPROGRAMS\{{start_menu_folder}}"
    CreateShortcut "$SMPROGRAMS\{{start_menu_folder}}\{{product_name}}.lnk" "$INSTDIR\{{main_binary_name}}"
    CreateShortcut "$SMPROGRAMS\{{start_menu_folder}}\Uninstall {{product_name}}.lnk" "$INSTDIR\uninstall.exe"

    ; Create Desktop shortcut
    CreateShortcut "$DESKTOP\{{product_name}}.lnk" "$INSTDIR\{{main_binary_name}}"

    ; Write registry keys for Add/Remove Programs
    WriteRegStr SHCTX "Software\Microsoft\Windows\CurrentVersion\Uninstall\{{bundle_id}}" \
        "DisplayName" "{{product_name}}"
    WriteRegStr SHCTX "Software\Microsoft\Windows\CurrentVersion\Uninstall\{{bundle_id}}" \
        "UninstallString" '"$INSTDIR\uninstall.exe"'
    WriteRegStr SHCTX "Software\Microsoft\Windows\CurrentVersion\Uninstall\{{bundle_id}}" \
        "DisplayVersion" "{{version}}"
    {{#if publisher}}
    WriteRegStr SHCTX "Software\Microsoft\Windows\CurrentVersion\Uninstall\{{bundle_id}}" \
        "Publisher" "{{publisher}}"
    {{/if}}
    WriteRegStr SHCTX "Software\Microsoft\Windows\CurrentVersion\Uninstall\{{bundle_id}}" \
        "InstallLocation" "$INSTDIR"

    ; Get installed size
    ${GetSize} "$INSTDIR" "/S=0K" $0 $1 $2
    IntFmt $0 "0x%08X" $0
    WriteRegDWORD SHCTX "Software\Microsoft\Windows\CurrentVersion\Uninstall\{{bundle_id}}" \
        "EstimatedSize" "$0"

    {{#if install_webview}}
    ; WebView2 installation
    {{webview_install_code}}
    {{/if}}

SectionEnd

; Registers bridge-host.exe, which the app's section installed beside the app,
; as the browsers' native messaging host for this user.
Section "Browser bridge" SecBridge
    IfFileExists "$INSTDIR\bridge-host.exe" 0 bridge_missing
    DetailPrint "Registering the browser bridge"
    nsExec::ExecToLog '"$INSTDIR\bridge-host.exe" install'
    Pop $0
    StrCmp $0 "0" bridge_done
    DetailPrint "The browser bridge was not registered ($0). Install it from the Extension status in Airlock."
    Goto bridge_done
bridge_missing:
    DetailPrint "This installer has no browser bridge."
bridge_done:
SectionEnd

!insertmacro MUI_FUNCTION_DESCRIPTION_BEGIN
    !insertmacro MUI_DESCRIPTION_TEXT ${SecApp} "The Airlock app."
    !insertmacro MUI_DESCRIPTION_TEXT ${SecBridge} "Lets the Airlock browser extension pass a website's requests to this app, for you to review and sign. Holds no keys. Leave it off on a machine that should never talk to a browser."
!insertmacro MUI_FUNCTION_DESCRIPTION_END

{{#if installer_hooks}}
!include "{{installer_hooks}}"
{{/if}}

; Uninstaller section
Section "Uninstall"
    ; Remove the browser bridge's registration, if it names this copy
    IfFileExists "$INSTDIR\bridge-host.exe" 0 +3
    nsExec::ExecToLog '"$INSTDIR\bridge-host.exe" uninstall --if-this-exe'
    Pop $0

    ; Remove files
    RMDir /r "$INSTDIR"

    ; Remove Start Menu items
    RMDir /r "$SMPROGRAMS\{{start_menu_folder}}"

    ; Remove Desktop shortcut
    Delete "$DESKTOP\{{product_name}}.lnk"

    ; Remove registry keys
    DeleteRegKey SHCTX "Software\Microsoft\Windows\CurrentVersion\Uninstall\{{bundle_id}}"
SectionEnd
