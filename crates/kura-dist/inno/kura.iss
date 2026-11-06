; Kura's per-user Inno Setup script.
;
; The default AppendDefaultDirName appends "Kura" to the folder the user picks,
; so D:\Apps installs into D:\Apps\Kura. `mise run package` passes the version,
; stage, output and icon paths with /D; the defaults here are for a manual run.

#define AppName "Kura"
#define AppPublisher "hu-ja-ja"
#define AppId "{{52762645-69AD-46B4-9CEE-4BA2E9D3A6E9}"
#ifndef AppVersion
  #define AppVersion "0.1.0"
#endif
#ifndef StageDir
  #define StageDir "..\..\..\dist\stage"
#endif
#ifndef OutputDir
  #define OutputDir "..\..\..\dist"
#endif
#ifndef IconFile
  #define IconFile "..\..\kura-app\assets\kura.ico"
#endif

[Setup]
AppId={#AppId}
AppName={#AppName}
AppVersion={#AppVersion}
AppPublisher={#AppPublisher}
AppPublisherURL=https://github.com/hu-ja-ja/kura
AppSupportURL=https://github.com/hu-ja-ja/kura/issues
DefaultDirName={autopf}\{#AppName}
DisableDirPage=no
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=commandline
CloseApplications=force
RestartApplications=no
SetupMutex=KuraSetupMutex
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir={#OutputDir}
OutputBaseFilename=Kura-{#AppVersion}-setup
SetupIconFile={#IconFile}
UninstallDisplayIcon={app}\kura.exe
WizardStyle=modern
SolidCompression=yes
VersionInfoVersion={#AppVersion}.0

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"
Name: "japanese"; MessagesFile: "compiler:Languages\Japanese.isl"

[CustomMessages]
english.RemoveUserData=Kura's settings, saved login, update preferences, logs and any other files under its AppData folders are kept unless you remove them.%n%nRemove them now?
japanese.RemoveUserData=Kuraの設定、保存済みログイン、更新の設定、ログなど、AppData内のKuraのファイルは削除しない限り残ります。%n%n今すぐ削除しますか?
english.ReinstallPageTitle=Kura is already installed
japanese.ReinstallPageTitle=Kura は既にインストールされています
english.ReinstallPageText=Kura %1 is already installed. Click Next to reinstall it, or Cancel to exit.
japanese.ReinstallPageText=Kura %1 は既にインストールされています。再インストールするには「次へ」、終了するには「キャンセル」をクリックしてください。

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#StageDir}\kura.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#StageDir}\LICENSE"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#StageDir}\README.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#StageDir}\THIRD-PARTY-NOTICES.txt"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\{#AppName}"; Filename: "{app}\kura.exe"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\kura.exe"; Tasks: desktopicon

[Run]
Filename: "{app}\kura.exe"; Description: "{cm:LaunchProgram,{#AppName}}"; Flags: nowait postinstall

[Code]
var
  RemoveUserDataOnUninstall: Boolean;
  SameVersionInstalled: Boolean;

// Inno 7 has no maintenance mode, so running the same version again would
// quietly reinstall. Ask on a wizard page instead of an InitializeSetup MsgBox:
// no wizard exists yet at that point, so the message box would have no owner
// and Windows would place it somewhere other than the centered wizard.
function InitializeSetup: Boolean;
var
  Root: Integer;
  Key: String;
  Installed: String;
begin
  Result := True;
  SameVersionInstalled := False;
  if WizardSilent then
    Exit;
  if IsAdminInstallMode then
    Root := HKLM
  else
    Root := HKCU;
  Key := 'Software\Microsoft\Windows\CurrentVersion\Uninstall\' +
    ExpandConstant('{#AppId}') + '_is1';
  SameVersionInstalled := RegQueryStringValue(Root, Key, 'DisplayVersion', Installed)
    and (Installed = '{#AppVersion}');
end;

procedure InitializeWizard;
begin
  if not SameVersionInstalled then
    Exit;
  CreateCustomPage(wpWelcome,
    ExpandConstant('{cm:ReinstallPageTitle}'),
    ExpandConstant('{cm:ReinstallPageText,{#AppVersion}}'));
end;

procedure DeleteUserData;
begin
  DelTree(ExpandConstant('{userappdata}\Kura'), True, True, True);
  DelTree(ExpandConstant('{localappdata}\Kura'), True, True, True);
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usUninstall then
    RemoveUserDataOnUninstall := not UninstallSilent and
      (MsgBox(ExpandConstant('{cm:RemoveUserData}'), mbConfirmation,
        MB_YESNO or MB_DEFBUTTON2) = IDYES)
  else if CurUninstallStep = usPostUninstall then
  begin
    RegDeleteValue(HKCU, 'Software\Microsoft\Windows\CurrentVersion\Run', 'Kura');
    if RemoveUserDataOnUninstall then
      DeleteUserData;
  end;
end;
