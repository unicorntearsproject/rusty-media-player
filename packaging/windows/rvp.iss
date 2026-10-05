; Rusty Video Player: Windows installer (Inno Setup 6.3 or newer).
;
; Packages the tree `cargo xtask dist windows` stages (rvp.exe, the licences, the icon). Build it by hand with
;
;   ISCC.exe /DAppVersion=0.1.0 /DStageDir=..\..\target\dist\windows\stage packaging\windows\rvp.iss
;
; or let `cargo xtask dist installer` do it (it passes the version from Cargo.toml).
;
; What it installs: the program for the current user (no administrator needed) or for all users (the wizard offers it), a Start menu
; entry, an optional desktop shortcut, an optional "add to PATH", and file associations. The associations are *registered*, not forced:
; Rusty Video Player shows up under "Open with" and in Settings > Default apps for each media type, and the user picks the default
; (Windows 10 and 11 do not let an installer take over a default). The uninstaller removes all of it and, if the user says so, the
; settings and the library index under %APPDATA%\rvp.
;
; Signing: pass /S"rvpsign=<command with $f>" to ISCC and /DSign=1 (CI does this when a code-signing certificate is available;
; see docs/packaging.md). Until then the installer and rvp.exe are unsigned and SmartScreen asks for "More info > Run anyway".

#define AppName "Rusty Video Player"
#define AppExe "rvp.exe"
#define AppId "io.github.idometeor.RustyVideoPlayer"
#define AppPublisher "Rusty Video Player contributors"
#define AppURL "https://github.com/iDoMeteor/rusty-video-player"
#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#ifndef StageDir
  #define StageDir "..\..\target\dist\windows\stage"
#endif
#ifndef OutDir
  #define OutDir "..\..\target\dist\windows"
#endif
; A numeric version for the file properties: 0.1.0-rc1 -> 0.1.0.0
#define NumVersion Copy(AppVersion, 1, Pos("-", AppVersion + "-") - 1) + ".0"

[Setup]
AppId={{D79C1CAA-7471-4519-9D40-4C0DB70B0FE1}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher={#AppPublisher}
AppPublisherURL={#AppURL}
AppSupportURL={#AppURL}/issues
AppUpdatesURL={#AppURL}/releases
VersionInfoVersion={#NumVersion}
VersionInfoDescription={#AppName} setup
DefaultDirName={autopf}\{#AppName}
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
LicenseFile={#StageDir}\LICENSE.txt
OutputDir={#OutDir}
OutputBaseFilename=RustyVideoPlayer-{#AppVersion}-x64-Setup
SetupIconFile=..\icons\rvp.ico
UninstallDisplayIcon={app}\{#AppExe}
UninstallDisplayName={#AppName}
WizardStyle=modern
WizardImageFile=wizard-164.bmp,wizard-246.bmp,wizard-328.bmp
WizardSmallImageFile=wizard-small-55.bmp,wizard-small-83.bmp,wizard-small-110.bmp
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
Compression=lzma2/ultra64
SolidCompression=yes
CloseApplications=yes
RestartApplications=no
ChangesAssociations=yes
ChangesEnvironment=yes
#ifdef Sign
SignTool=rvpsign
SignedUninstaller=yes
#endif

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "assoc"; Description: "Show {#AppName} under ""Open with"" and in Default apps for audio and video files"; GroupDescription: "File types:"
Name: "desktopicon"; Description: "Create a desktop shortcut"; GroupDescription: "Shortcuts:"; Flags: unchecked
Name: "addtopath"; Description: "Add rvp to PATH (run it from any terminal)"; GroupDescription: "Command line:"; Flags: unchecked

[Files]
Source: "{#StageDir}\{#AppExe}"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#StageDir}\LICENSE.txt"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#StageDir}\THIRD_PARTY_LICENSES.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#StageDir}\README.txt"; DestDir: "{app}"; Flags: ignoreversion isreadme
Source: "..\icons\rvp.ico"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{group}\{#AppName}"; Filename: "{app}\{#AppExe}"; AppUserModelID: "{#AppId}"
Name: "{group}\Uninstall {#AppName}"; Filename: "{uninstallexe}"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#AppExe}"; AppUserModelID: "{#AppId}"; Tasks: desktopicon

[Registry]
; The program itself: "Open with" and the Default apps page.
Root: HKA; Subkey: "Software\Classes\Applications\{#AppExe}"; ValueType: string; ValueName: "FriendlyAppName"; ValueData: "{#AppName}"; Flags: uninsdeletekey; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\Applications\{#AppExe}\shell\open\command"; ValueType: string; ValueData: """{app}\{#AppExe}"" ""%1"""; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\Applications\{#AppExe}\DefaultIcon"; ValueType: string; ValueData: "{app}\{#AppExe},0"; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\{#AppId}.Media"; ValueType: string; ValueData: "Media file (Rusty Video Player)"; Flags: uninsdeletekey; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\{#AppId}.Media\DefaultIcon"; ValueType: string; ValueData: "{app}\{#AppExe},0"; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\{#AppId}.Media\shell\open\command"; ValueType: string; ValueData: """{app}\{#AppExe}"" ""%1"""; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\{#AppId}.Media\shell\enqueue"; ValueType: string; ValueName: "MUIVerb"; ValueData: "Add to Rusty Video Player queue"; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\{#AppId}.Media\shell\enqueue\command"; ValueType: string; ValueData: """{app}\{#AppExe}"" ""%1"""; Tasks: assoc
Root: HKA; Subkey: "Software\RustyVideoPlayer\Capabilities"; ValueType: string; ValueName: "ApplicationName"; ValueData: "{#AppName}"; Flags: uninsdeletekey; Tasks: assoc
Root: HKA; Subkey: "Software\RustyVideoPlayer\Capabilities"; ValueType: string; ValueName: "ApplicationDescription"; ValueData: "Plays video and music, with a library, a queue and a visualizer."; Tasks: assoc
Root: HKA; Subkey: "Software\RegisteredApplications"; ValueType: string; ValueName: "RustyVideoPlayer"; ValueData: "Software\RustyVideoPlayer\Capabilities"; Flags: uninsdeletevalue; Tasks: assoc
; Three entries per file type: the capability (what Default apps lists), the type's OpenWithProgids, the program's SupportedTypes.
; (Generated from the list in docs/packaging.md; keep the two in step with MimeType= in the .desktop file.)
Root: HKA; Subkey: "Software\RustyVideoPlayer\Capabilities\FileAssociations"; ValueType: string; ValueName: ".mp4"; ValueData: "{#AppId}.Media"; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\.mp4\OpenWithProgids"; ValueType: string; ValueName: "{#AppId}.Media"; ValueData: ""; Flags: uninsdeletevalue; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\Applications\{#AppExe}\SupportedTypes"; ValueType: string; ValueName: ".mp4"; ValueData: ""; Tasks: assoc
Root: HKA; Subkey: "Software\RustyVideoPlayer\Capabilities\FileAssociations"; ValueType: string; ValueName: ".m4v"; ValueData: "{#AppId}.Media"; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\.m4v\OpenWithProgids"; ValueType: string; ValueName: "{#AppId}.Media"; ValueData: ""; Flags: uninsdeletevalue; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\Applications\{#AppExe}\SupportedTypes"; ValueType: string; ValueName: ".m4v"; ValueData: ""; Tasks: assoc
Root: HKA; Subkey: "Software\RustyVideoPlayer\Capabilities\FileAssociations"; ValueType: string; ValueName: ".mkv"; ValueData: "{#AppId}.Media"; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\.mkv\OpenWithProgids"; ValueType: string; ValueName: "{#AppId}.Media"; ValueData: ""; Flags: uninsdeletevalue; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\Applications\{#AppExe}\SupportedTypes"; ValueType: string; ValueName: ".mkv"; ValueData: ""; Tasks: assoc
Root: HKA; Subkey: "Software\RustyVideoPlayer\Capabilities\FileAssociations"; ValueType: string; ValueName: ".webm"; ValueData: "{#AppId}.Media"; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\.webm\OpenWithProgids"; ValueType: string; ValueName: "{#AppId}.Media"; ValueData: ""; Flags: uninsdeletevalue; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\Applications\{#AppExe}\SupportedTypes"; ValueType: string; ValueName: ".webm"; ValueData: ""; Tasks: assoc
Root: HKA; Subkey: "Software\RustyVideoPlayer\Capabilities\FileAssociations"; ValueType: string; ValueName: ".mka"; ValueData: "{#AppId}.Media"; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\.mka\OpenWithProgids"; ValueType: string; ValueName: "{#AppId}.Media"; ValueData: ""; Flags: uninsdeletevalue; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\Applications\{#AppExe}\SupportedTypes"; ValueType: string; ValueName: ".mka"; ValueData: ""; Tasks: assoc
Root: HKA; Subkey: "Software\RustyVideoPlayer\Capabilities\FileAssociations"; ValueType: string; ValueName: ".mp3"; ValueData: "{#AppId}.Media"; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\.mp3\OpenWithProgids"; ValueType: string; ValueName: "{#AppId}.Media"; ValueData: ""; Flags: uninsdeletevalue; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\Applications\{#AppExe}\SupportedTypes"; ValueType: string; ValueName: ".mp3"; ValueData: ""; Tasks: assoc
Root: HKA; Subkey: "Software\RustyVideoPlayer\Capabilities\FileAssociations"; ValueType: string; ValueName: ".flac"; ValueData: "{#AppId}.Media"; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\.flac\OpenWithProgids"; ValueType: string; ValueName: "{#AppId}.Media"; ValueData: ""; Flags: uninsdeletevalue; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\Applications\{#AppExe}\SupportedTypes"; ValueType: string; ValueName: ".flac"; ValueData: ""; Tasks: assoc
Root: HKA; Subkey: "Software\RustyVideoPlayer\Capabilities\FileAssociations"; ValueType: string; ValueName: ".ogg"; ValueData: "{#AppId}.Media"; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\.ogg\OpenWithProgids"; ValueType: string; ValueName: "{#AppId}.Media"; ValueData: ""; Flags: uninsdeletevalue; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\Applications\{#AppExe}\SupportedTypes"; ValueType: string; ValueName: ".ogg"; ValueData: ""; Tasks: assoc
Root: HKA; Subkey: "Software\RustyVideoPlayer\Capabilities\FileAssociations"; ValueType: string; ValueName: ".oga"; ValueData: "{#AppId}.Media"; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\.oga\OpenWithProgids"; ValueType: string; ValueName: "{#AppId}.Media"; ValueData: ""; Flags: uninsdeletevalue; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\Applications\{#AppExe}\SupportedTypes"; ValueType: string; ValueName: ".oga"; ValueData: ""; Tasks: assoc
Root: HKA; Subkey: "Software\RustyVideoPlayer\Capabilities\FileAssociations"; ValueType: string; ValueName: ".opus"; ValueData: "{#AppId}.Media"; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\.opus\OpenWithProgids"; ValueType: string; ValueName: "{#AppId}.Media"; ValueData: ""; Flags: uninsdeletevalue; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\Applications\{#AppExe}\SupportedTypes"; ValueType: string; ValueName: ".opus"; ValueData: ""; Tasks: assoc
Root: HKA; Subkey: "Software\RustyVideoPlayer\Capabilities\FileAssociations"; ValueType: string; ValueName: ".wav"; ValueData: "{#AppId}.Media"; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\.wav\OpenWithProgids"; ValueType: string; ValueName: "{#AppId}.Media"; ValueData: ""; Flags: uninsdeletevalue; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\Applications\{#AppExe}\SupportedTypes"; ValueType: string; ValueName: ".wav"; ValueData: ""; Tasks: assoc
Root: HKA; Subkey: "Software\RustyVideoPlayer\Capabilities\FileAssociations"; ValueType: string; ValueName: ".m4a"; ValueData: "{#AppId}.Media"; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\.m4a\OpenWithProgids"; ValueType: string; ValueName: "{#AppId}.Media"; ValueData: ""; Flags: uninsdeletevalue; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\Applications\{#AppExe}\SupportedTypes"; ValueType: string; ValueName: ".m4a"; ValueData: ""; Tasks: assoc
Root: HKA; Subkey: "Software\RustyVideoPlayer\Capabilities\FileAssociations"; ValueType: string; ValueName: ".m4b"; ValueData: "{#AppId}.Media"; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\.m4b\OpenWithProgids"; ValueType: string; ValueName: "{#AppId}.Media"; ValueData: ""; Flags: uninsdeletevalue; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\Applications\{#AppExe}\SupportedTypes"; ValueType: string; ValueName: ".m4b"; ValueData: ""; Tasks: assoc
Root: HKA; Subkey: "Software\RustyVideoPlayer\Capabilities\FileAssociations"; ValueType: string; ValueName: ".aac"; ValueData: "{#AppId}.Media"; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\.aac\OpenWithProgids"; ValueType: string; ValueName: "{#AppId}.Media"; ValueData: ""; Flags: uninsdeletevalue; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\Applications\{#AppExe}\SupportedTypes"; ValueType: string; ValueName: ".aac"; ValueData: ""; Tasks: assoc
Root: HKA; Subkey: "Software\RustyVideoPlayer\Capabilities\FileAssociations"; ValueType: string; ValueName: ".m3u"; ValueData: "{#AppId}.Media"; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\.m3u\OpenWithProgids"; ValueType: string; ValueName: "{#AppId}.Media"; ValueData: ""; Flags: uninsdeletevalue; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\Applications\{#AppExe}\SupportedTypes"; ValueType: string; ValueName: ".m3u"; ValueData: ""; Tasks: assoc
Root: HKA; Subkey: "Software\RustyVideoPlayer\Capabilities\FileAssociations"; ValueType: string; ValueName: ".m3u8"; ValueData: "{#AppId}.Media"; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\.m3u8\OpenWithProgids"; ValueType: string; ValueName: "{#AppId}.Media"; ValueData: ""; Flags: uninsdeletevalue; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\Applications\{#AppExe}\SupportedTypes"; ValueType: string; ValueName: ".m3u8"; ValueData: ""; Tasks: assoc
Root: HKA; Subkey: "Software\RustyVideoPlayer\Capabilities\FileAssociations"; ValueType: string; ValueName: ".pls"; ValueData: "{#AppId}.Media"; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\.pls\OpenWithProgids"; ValueType: string; ValueName: "{#AppId}.Media"; ValueData: ""; Flags: uninsdeletevalue; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\Applications\{#AppExe}\SupportedTypes"; ValueType: string; ValueName: ".pls"; ValueData: ""; Tasks: assoc
Root: HKA; Subkey: "Environment"; ValueType: expandsz; ValueName: "Path"; ValueData: "{olddata};{app}"; Tasks: addtopath; Check: NeedsAddPath(ExpandConstant('{app}'))

[Run]
Filename: "{app}\{#AppExe}"; Description: "Launch {#AppName}"; Flags: nowait postinstall skipifsilent

[Code]
function NeedsAddPath(Param: string): Boolean;
var
  OrigPath: string;
begin
  if not RegQueryStringValue(HKA, 'Environment', 'Path', OrigPath) then
  begin
    Result := True;
    exit;
  end;
  Result := Pos(';' + Uppercase(Param) + ';', ';' + Uppercase(OrigPath) + ';') = 0;
end;

{ Take the install folder out of PATH again. }
procedure RemoveFromPath();
var
  OrigPath, App: string;
  P: Integer;
begin
  if not RegQueryStringValue(HKA, 'Environment', 'Path', OrigPath) then exit;
  App := ExpandConstant('{app}');
  P := Pos(';' + Uppercase(App), Uppercase(OrigPath));
  if P > 0 then
  begin
    Delete(OrigPath, P, Length(App) + 1);
    RegWriteExpandStringValue(HKA, 'Environment', 'Path', OrigPath);
  end;
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  Data: string;
begin
  if CurUninstallStep = usUninstall then
    RemoveFromPath();
  if CurUninstallStep = usPostUninstall then
  begin
    Data := ExpandConstant('{userappdata}\rvp');
    if DirExists(Data) and not UninstallSilent then
      if MsgBox('Also remove your settings, library index and saved queue?' + #13#10 + Data, mbConfirmation, MB_YESNO or MB_DEFBUTTON2) = IDYES then
        DelTree(Data, True, True, True);
  end;
end;
